use crate::net::Frame;
use ffmpeg_next::ffi;
use glow::HasContext;
use glutin::{
    config::{Config, ConfigTemplateBuilder},
    context::{ContextApi, ContextAttributesBuilder, NotCurrentGlContext, PossiblyCurrentContext, PossiblyCurrentGlContext, Version},
    display::{GetGlDisplay, GlDisplay},
    surface::{GlSurface, Surface, SwapInterval, WindowSurface},
};
use glutin_winit::{DisplayBuilder, GlWindow};
use std::{ffi::CString, num::NonZeroU32, sync::Arc};
use winit::{
    event_loop::ActiveEventLoop,
    raw_window_handle::HasWindowHandle,
    window::{Window, WindowAttributes},
};

const VERTEX: &str = r#"
out vec2 uv;
uniform vec2 crop;
void main() {
    vec2 p = vec2(float((gl_VertexID << 1) & 2), float(gl_VertexID & 2));
    uv = p * crop;
    gl_Position = vec4(p.x * 2.0 - 1.0, 1.0 - p.y * 2.0, 0.0, 1.0);
}
"#;

const FRAGMENT: &str = r#"
in vec2 uv;
out vec4 color;
uniform sampler2D plane0;
uniform sampler2D plane1;
uniform int mode;
uniform float m[6];
void main() {
    vec4 a = texture(plane0, uv);
    if (mode == 0) {
        color = vec4(a.b, a.g, a.r, 1.0);
    } else {
        float y = (a.r - m[1]) * m[0];
        vec2 c = texture(plane1, uv).rg - vec2(0.5);
        color = vec4(y + m[2] * c.y, y - m[3] * c.x - m[4] * c.y, y + m[5] * c.x, 1.0);
    }
}
"#;

#[cfg(target_os = "linux")]
mod dmabuf {
    use super::*;
    use glutin::display::{AsRawDisplay, Display, RawDisplay};
    use std::ffi::c_void;

    pub type CreateImage = unsafe extern "C" fn(*const c_void, *const c_void, u32, *const c_void, *const isize) -> *const c_void;
    pub type DestroyImage = unsafe extern "C" fn(*const c_void, *const c_void) -> u32;
    pub type TargetTexture = unsafe extern "C" fn(u32, *const c_void);

    pub struct Egl {
        display: *const c_void,
        create: CreateImage,
        destroy: DestroyImage,
        target: TargetTexture,
    }

    impl Egl {
        pub fn new(display: &Display) -> Option<Self> {
            let RawDisplay::Egl(raw) = display.raw_display() else { return None };
            let load = |name: &str| {
                let ptr = display.get_proc_address(&CString::new(name).unwrap());
                (!ptr.is_null()).then_some(ptr)
            };
            unsafe {
                Some(Self {
                    display: raw,
                    create: std::mem::transmute::<*const c_void, CreateImage>(load("eglCreateImageKHR")?),
                    destroy: std::mem::transmute::<*const c_void, DestroyImage>(load("eglDestroyImageKHR")?),
                    target: std::mem::transmute::<*const c_void, TargetTexture>(load("glEGLImageTargetTexture2DOES")?),
                })
            }
        }

        pub fn destroy(&self, images: &mut Vec<*const c_void>) {
            for image in images.drain(..) {
                unsafe { (self.destroy)(self.display, image) };
            }
        }

        pub fn import(&self, gl: &glow::Context, frame: &Frame, textures: [glow::Texture; 2], images: &mut Vec<*const c_void>) -> Option<Frame> {
            unsafe {
                let mapped = ffi::av_frame_alloc();
                (*mapped).format = ffi::AVPixelFormat::AV_PIX_FMT_DRM_PRIME as i32;
                if ffi::av_hwframe_map(mapped, frame.raw(), ffi::AV_HWFRAME_MAP_READ as i32) < 0 {
                    ffi::av_frame_free(&mut { mapped });
                    return None;
                }
                let mapped = Frame::from_raw(mapped);
                let desc = &*((*mapped.raw()).data[0] as *const ffi::AVDRMFrameDescriptor);
                let (w, h) = frame.size();
                let mut planes = Vec::new();
                for layer in &desc.layers[..desc.nb_layers as usize] {
                    for (index, plane) in layer.planes[..layer.nb_planes as usize].iter().enumerate() {
                        let fourcc = match (layer.format, index) {
                            (0x3231_564e, 0) => 0x2020_3852,
                            (0x3231_564e, _) => 0x3838_5247,
                            (format, _) => format,
                        };
                        planes.push((fourcc, plane));
                    }
                }
                if planes.len() != 2 {
                    return None;
                }
                for (i, (fourcc, plane)) in planes.into_iter().enumerate() {
                    let object = &desc.objects[plane.object_index as usize];
                    let (pw, ph) = if i == 0 { (w, h) } else { (w.div_ceil(2), h.div_ceil(2)) };
                    let mut attrs: Vec<isize> = vec![
                        0x3057, pw as isize,
                        0x3056, ph as isize,
                        0x3271, fourcc as isize,
                        0x3272, object.fd as isize,
                        0x3273, plane.offset,
                        0x3274, plane.pitch,
                    ];
                    if object.format_modifier != 0x00ff_ffff_ffff_ffff {
                        attrs.extend_from_slice(&[
                            0x3443, (object.format_modifier & 0xffff_ffff) as isize,
                            0x3444, (object.format_modifier >> 32) as isize,
                        ]);
                    }
                    attrs.push(0x3038);
                    let image = (self.create)(self.display, std::ptr::null(), 0x3270, std::ptr::null(), attrs.as_ptr());
                    if image.is_null() {
                        self.destroy(images);
                        return None;
                    }
                    images.push(image);
                    gl.bind_texture(glow::TEXTURE_2D, Some(textures[i]));
                    (self.target)(glow::TEXTURE_2D, image);
                }
                Some(mapped)
            }
        }
    }
}

pub struct Gpu {
    config: Option<Config>,
    context: Option<PossiblyCurrentContext>,
    gl: Option<glow::Context>,
    program: Option<glow::Program>,
    array: Option<glow::VertexArray>,
    #[cfg(target_os = "linux")]
    egl: Option<dmabuf::Egl>,
}

type Format = (i32, u32, usize);

const R8: Format = (glow::R8 as i32, glow::RED, 1);
const RG8: Format = (glow::RG8 as i32, glow::RG, 2);
const RGBA8: Format = (glow::RGBA8 as i32, glow::RGBA, 4);

pub struct View {
    surface: Surface<WindowSurface>,
    textures: [glow::Texture; 2],
    mode: i32,
    size: (u32, u32),
    crop: (u32, u32),
    matrix: [f32; 6],
    scale: u32,
    held: Vec<Frame>,
    #[cfg(target_os = "linux")]
    images: Vec<*const std::ffi::c_void>,
}

impl Gpu {
    pub fn new() -> Self {
        Self {
            config: None,
            context: None,
            gl: None,
            program: None,
            array: None,
            #[cfg(target_os = "linux")]
            egl: None,
        }
    }

    fn setup(&mut self, window: &Window) -> Option<()> {
        let config = self.config.as_ref()?;
        let display = config.display();
        let attributes = ContextAttributesBuilder::new()
            .with_context_api(ContextApi::Gles(Some(Version::new(3, 0))))
            .build(Some(window.window_handle().ok()?.as_raw()));
        let fallback = ContextAttributesBuilder::new()
            .with_context_api(ContextApi::OpenGl(Some(Version::new(3, 3))))
            .build(Some(window.window_handle().ok()?.as_raw()));
        let context = unsafe {
            display
                .create_context(config, &attributes)
                .or_else(|_| display.create_context(config, &fallback))
                .ok()?
        };
        let context = context.treat_as_possibly_current();
        let gl = unsafe {
            glow::Context::from_loader_function_cstr(|name| display.get_proc_address(name))
        };
        let header = if gl.version().is_embedded { "#version 300 es\nprecision mediump float;\n" } else { "#version 330 core\n" };
        let program = unsafe {
            let program = gl.create_program().ok()?;
            for (kind, source) in [(glow::VERTEX_SHADER, VERTEX), (glow::FRAGMENT_SHADER, FRAGMENT)] {
                let shader = gl.create_shader(kind).ok()?;
                gl.shader_source(shader, &format!("{header}{source}"));
                gl.compile_shader(shader);
                if !gl.get_shader_compile_status(shader) {
                    eprintln!("shader: {}", gl.get_shader_info_log(shader));
                    return None;
                }
                gl.attach_shader(program, shader);
            }
            gl.link_program(program);
            if !gl.get_program_link_status(program) {
                eprintln!("shader: {}", gl.get_program_info_log(program));
                return None;
            }
            program
        };
        #[cfg(target_os = "linux")]
        {
            self.egl = dmabuf::Egl::new(&display);
        }
        self.array = unsafe { gl.create_vertex_array().ok() };
        self.context = Some(context);
        self.gl = Some(gl);
        self.program = Some(program);
        Some(())
    }

    pub fn create_window(&mut self, event_loop: &ActiveEventLoop, attributes: WindowAttributes) -> Option<(Arc<Window>, View)> {
        let window = match &self.config {
            None => {
                let template = ConfigTemplateBuilder::new();
                let (window, config) = DisplayBuilder::new()
                    .with_window_attributes(Some(attributes))
                    .build(event_loop, template, |mut configs| configs.next().unwrap())
                    .ok()?;
                self.config = Some(config);
                window?
            }
            Some(config) => glutin_winit::finalize_window(event_loop, attributes, config).ok()?,
        };
        let window = Arc::new(window);
        if self.context.is_none() {
            self.setup(&window)?;
        }
        let config = self.config.as_ref()?;
        let surface_attributes = window.build_surface_attributes(Default::default()).ok()?;
        let surface = unsafe { config.display().create_window_surface(config, &surface_attributes).ok()? };
        let context = self.context.as_ref()?;
        context.make_current(&surface).ok()?;
        let _ = surface.set_swap_interval(context, SwapInterval::DontWait);
        let gl = self.gl.as_ref()?;
        let textures = unsafe {
            let mut textures = [gl.create_texture().ok()?, gl.create_texture().ok()?];
            for texture in &mut textures {
                gl.bind_texture(glow::TEXTURE_2D, Some(*texture));
                gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_MIN_FILTER, glow::LINEAR as i32);
                gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_MAG_FILTER, glow::LINEAR as i32);
                gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_WRAP_S, glow::CLAMP_TO_EDGE as i32);
                gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_WRAP_T, glow::CLAMP_TO_EDGE as i32);
            }
            textures
        };
        Some((
            window,
            View {
                surface,
                textures,
                mode: -1,
                size: (0, 0),
                crop: (0, 0),
                matrix: [1.0, 0.0, 0.0, 0.0, 0.0, 0.0],
                scale: 1,
                held: Vec::new(),
                #[cfg(target_os = "linux")]
                images: Vec::new(),
            },
        ))
    }
}

impl View {
    fn current<'a>(&self, gpu: &'a Gpu) -> Option<&'a glow::Context> {
        gpu.context.as_ref()?.make_current(&self.surface).ok()?;
        gpu.gl.as_ref()
    }

    fn upload(gl: &glow::Context, texture: glow::Texture, format: Format, size: (u32, u32), y0: u32, (data, stride): (&[u8], usize), allocate: bool) {
        unsafe {
            gl.bind_texture(glow::TEXTURE_2D, Some(texture));
            gl.pixel_store_i32(glow::UNPACK_ALIGNMENT, 1);
            gl.pixel_store_i32(glow::UNPACK_ROW_LENGTH, (stride / format.2) as i32);
            if allocate {
                gl.tex_image_2d(
                    glow::TEXTURE_2D,
                    0,
                    format.0,
                    size.0 as i32,
                    size.1 as i32,
                    0,
                    format.1,
                    glow::UNSIGNED_BYTE,
                    glow::PixelUnpackData::Slice(Some(data)),
                );
            } else {
                gl.tex_sub_image_2d(
                    glow::TEXTURE_2D,
                    0,
                    0,
                    y0 as i32,
                    size.0 as i32,
                    size.1 as i32,
                    format.1,
                    glow::UNSIGNED_BYTE,
                    glow::PixelUnpackData::Slice(Some(data)),
                );
            }
            gl.pixel_store_i32(glow::UNPACK_ROW_LENGTH, 0);
        }
    }

    pub fn video(&mut self, gpu: &mut Gpu, frame: Frame, crop: (u32, u32), matrix: [f32; 6], scale: u32) -> bool {
        let Some(gl) = self.current(gpu) else { return false };
        let (w, h) = frame.size();
        let half = (w.div_ceil(2), h.div_ceil(2));
        let format = frame.format();
        #[cfg(target_os = "linux")]
        if format == ffi::AVPixelFormat::AV_PIX_FMT_VAAPI as i32 {
            let Some(egl) = gpu.egl.as_ref() else { return false };
            let mut images = Vec::new();
            let Some(mapped) = egl.import(gl, &frame, self.textures, &mut images) else { return false };
            egl.destroy(&mut self.images);
            self.images = images;
            self.held = vec![mapped, frame];
            self.finish(crop, (w, h), matrix, scale, 1);
            return true;
        }
        if format == ffi::AVPixelFormat::AV_PIX_FMT_NV12 as i32 {
            let (y, ys) = frame.plane(0, h as usize);
            let (uv, uvs) = frame.plane(1, half.1 as usize);
            let allocate = self.mode != 1 || self.size != (w, h);
            Self::upload(gl, self.textures[0], R8, (w, h), 0, (y, ys), allocate);
            Self::upload(gl, self.textures[1], RG8, half, 0, (uv, uvs), allocate);
        } else if format == ffi::AVPixelFormat::AV_PIX_FMT_YUV420P as i32 || format == ffi::AVPixelFormat::AV_PIX_FMT_YUVJ420P as i32 {
            let (y, ys) = frame.plane(0, h as usize);
            let (u, us) = frame.plane(1, half.1 as usize);
            let (v, vs) = frame.plane(2, half.1 as usize);
            let mut uv = vec![0u8; half.0 as usize * 2 * half.1 as usize];
            for row in 0..half.1 as usize {
                for col in 0..half.0 as usize {
                    uv[(row * half.0 as usize + col) * 2] = u[row * us + col];
                    uv[(row * half.0 as usize + col) * 2 + 1] = v[row * vs + col];
                }
            }
            let allocate = self.mode != 1 || self.size != (w, h);
            Self::upload(gl, self.textures[0], R8, (w, h), 0, (y, ys), allocate);
            Self::upload(gl, self.textures[1], RG8, half, 0, (&uv, half.0 as usize * 2), allocate);
        } else {
            return false;
        }
        self.held.clear();
        self.finish(crop, (w, h), matrix, scale, 1);
        true
    }

    fn finish(&mut self, crop: (u32, u32), size: (u32, u32), matrix: [f32; 6], scale: u32, mode: i32) {
        self.crop = crop;
        self.size = size;
        self.matrix = matrix;
        self.scale = scale;
        self.mode = mode;
    }

    pub fn pixels(&mut self, gpu: &mut Gpu, size: (u32, u32), y0: u32, rows: u32, data: &[u8], scale: u32) {
        let Some(gl) = self.current(gpu) else { return };
        let allocate = self.mode != 0 || self.size != size;
        if allocate && (y0 != 0 || rows != size.1) {
            return;
        }
        #[cfg(target_os = "linux")]
        if let Some(egl) = gpu.egl.as_ref() {
            egl.destroy(&mut self.images);
        }
        self.held.clear();
        Self::upload(gl, self.textures[0], RGBA8, (size.0, rows), y0, (data, size.0 as usize * 4), allocate);
        self.finish(size, size, [1.0, 0.0, 0.0, 0.0, 0.0, 0.0], scale, 0);
    }

    pub fn resized(&mut self, gpu: &mut Gpu, window: &Window) {
        let size = window.inner_size();
        if let (Some(context), Some(w), Some(h)) = (gpu.context.as_ref(), NonZeroU32::new(size.width), NonZeroU32::new(size.height)) {
            self.surface.resize(context, w, h);
        }
    }

    pub fn draw(&mut self, gpu: &mut Gpu, window: &Window) {
        if self.mode < 0 {
            return;
        }
        let Some(gl) = self.current(gpu) else { return };
        let Some(program) = gpu.program else { return };
        let surface = window.inner_size();
        let k = window.scale_factor() as f32 / self.scale.max(1) as f32;
        let (cw, ch) = ((self.crop.0 as f32 * k) as i32, (self.crop.1 as f32 * k) as i32);
        unsafe {
            gl.viewport(0, 0, surface.width as i32, surface.height as i32);
            gl.clear_color(0.0, 0.0, 0.0, 1.0);
            gl.clear(glow::COLOR_BUFFER_BIT);
            gl.viewport(0, surface.height as i32 - ch, cw, ch);
            gl.use_program(Some(program));
            gl.bind_vertex_array(gpu.array);
            let crop = [self.crop.0 as f32 / self.size.0.max(1) as f32, self.crop.1 as f32 / self.size.1.max(1) as f32];
            gl.uniform_2_f32(gl.get_uniform_location(program, "crop").as_ref(), crop[0], crop[1]);
            gl.uniform_1_i32(gl.get_uniform_location(program, "mode").as_ref(), self.mode);
            gl.uniform_1_f32_slice(gl.get_uniform_location(program, "m").as_ref(), &self.matrix);
            gl.uniform_1_i32(gl.get_uniform_location(program, "plane0").as_ref(), 0);
            gl.uniform_1_i32(gl.get_uniform_location(program, "plane1").as_ref(), 1);
            gl.active_texture(glow::TEXTURE0);
            gl.bind_texture(glow::TEXTURE_2D, Some(self.textures[0]));
            gl.active_texture(glow::TEXTURE1);
            gl.bind_texture(glow::TEXTURE_2D, Some(self.textures[1]));
            gl.draw_arrays(glow::TRIANGLES, 0, 3);
        }
        window.pre_present_notify();
        if let Some(context) = gpu.context.as_ref() {
            let _ = self.surface.swap_buffers(context);
        }
    }
}
