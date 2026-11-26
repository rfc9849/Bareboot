use crate::net::Frame;
use objc2::{rc::Retained, runtime::AnyObject};
use objc2_app_kit::NSView;
use objc2_core_foundation::{CGPoint, CGRect, CGSize};
use objc2_quartz_core::{CALayer, CATransaction};
use std::{collections::VecDeque, ffi::c_void, sync::Arc};
use winit::{
    event_loop::ActiveEventLoop,
    raw_window_handle::{HasWindowHandle, RawWindowHandle},
    window::{Window, WindowAttributes},
};

type CFTypeRef = *const c_void;

#[link(name = "CoreFoundation", kind = "framework")]
unsafe extern "C" {
    fn CFRelease(cf: CFTypeRef);
    fn CFDictionaryCreate(
        allocator: CFTypeRef,
        keys: *const CFTypeRef,
        values: *const CFTypeRef,
        count: isize,
        key_callbacks: *const c_void,
        value_callbacks: *const c_void,
    ) -> CFTypeRef;
    fn CFNumberCreate(allocator: CFTypeRef, number_type: isize, value: *const c_void) -> CFTypeRef;
    static kCFTypeDictionaryKeyCallBacks: u8;
    static kCFTypeDictionaryValueCallBacks: u8;
}

#[link(name = "IOSurface", kind = "framework")]
unsafe extern "C" {
    fn IOSurfaceCreate(properties: CFTypeRef) -> CFTypeRef;
    fn IOSurfaceLock(surface: CFTypeRef, options: u32, seed: *mut u32) -> i32;
    fn IOSurfaceUnlock(surface: CFTypeRef, options: u32, seed: *mut u32) -> i32;
    fn IOSurfaceGetBaseAddress(surface: CFTypeRef) -> *mut u8;
    fn IOSurfaceGetBytesPerRow(surface: CFTypeRef) -> usize;
    static kIOSurfaceWidth: CFTypeRef;
    static kIOSurfaceHeight: CFTypeRef;
    static kIOSurfaceBytesPerElement: CFTypeRef;
    static kIOSurfacePixelFormat: CFTypeRef;
}

#[link(name = "CoreVideo", kind = "framework")]
unsafe extern "C" {
    fn CVPixelBufferGetIOSurface(buffer: CFTypeRef) -> CFTypeRef;
}

struct Surface(CFTypeRef);

impl Drop for Surface {
    fn drop(&mut self) {
        unsafe { CFRelease(self.0) }
    }
}

fn number(value: i32) -> CFTypeRef {
    unsafe { CFNumberCreate(std::ptr::null(), 3, (&value as *const i32).cast()) }
}

fn surface(width: u32, height: u32) -> Option<Surface> {
    unsafe {
        let keys = [kIOSurfaceWidth, kIOSurfaceHeight, kIOSurfaceBytesPerElement, kIOSurfacePixelFormat];
        let values = [number(width as i32), number(height as i32), number(4), number(0x4247_5241)];
        let dict = CFDictionaryCreate(
            std::ptr::null(),
            keys.as_ptr(),
            values.as_ptr(),
            4,
            (&raw const kCFTypeDictionaryKeyCallBacks).cast(),
            (&raw const kCFTypeDictionaryValueCallBacks).cast(),
        );
        for value in values {
            CFRelease(value);
        }
        let surface = IOSurfaceCreate(dict);
        CFRelease(dict);
        (!surface.is_null()).then_some(Surface(surface))
    }
}

fn write(target: &Surface, y0: u32, rows: u32, row: usize, data: &[u8]) {
    unsafe {
        IOSurfaceLock(target.0, 0, std::ptr::null_mut());
        let base = IOSurfaceGetBaseAddress(target.0);
        let stride = IOSurfaceGetBytesPerRow(target.0);
        for y in 0..rows as usize {
            std::ptr::copy_nonoverlapping(data[y * row..].as_ptr(), base.add((y0 as usize + y) * stride), row);
        }
        IOSurfaceUnlock(target.0, 0, std::ptr::null_mut());
    }
}

fn copy_rows(from: &Surface, to: &Surface, y0: u32, rows: u32, row: usize) {
    unsafe {
        IOSurfaceLock(from.0, 1, std::ptr::null_mut());
        IOSurfaceLock(to.0, 0, std::ptr::null_mut());
        let (src, dst) = (IOSurfaceGetBaseAddress(from.0), IOSurfaceGetBaseAddress(to.0));
        let (ss, ds) = (IOSurfaceGetBytesPerRow(from.0), IOSurfaceGetBytesPerRow(to.0));
        for y in y0 as usize..(y0 + rows) as usize {
            std::ptr::copy_nonoverlapping(src.add(y * ss), dst.add(y * ds), row);
        }
        IOSurfaceUnlock(to.0, 0, std::ptr::null_mut());
        IOSurfaceUnlock(from.0, 1, std::ptr::null_mut());
    }
}

pub struct Gpu;

impl Gpu {
    pub fn new() -> Self {
        Self
    }

    pub fn create_window(&mut self, event_loop: &ActiveEventLoop, attributes: WindowAttributes) -> Option<(Arc<Window>, View)> {
        let window = Arc::new(event_loop.create_window(attributes).ok()?);
        let RawWindowHandle::AppKit(handle) = window.window_handle().ok()?.as_raw() else { return None };
        let view: &NSView = unsafe { handle.ns_view.cast().as_ref() };
        view.setWantsLayer(true);
        let root = view.layer()?;
        root.setGeometryFlipped(true);
        let layer = CALayer::new();
        layer.setOpaque(true);
        root.addSublayer(&layer);
        Some((
            window,
            View {
                layer,
                frames: VecDeque::new(),
                pixels: Vec::new(),
                size: (0, 0),
                crop: (0, 0),
                last_band: None,
                scale: 1,
            },
        ))
    }
}

pub struct View {
    layer: Retained<CALayer>,
    frames: VecDeque<Frame>,
    pixels: Vec<Surface>,
    size: (u32, u32),
    crop: (u32, u32),
    last_band: Option<(u32, u32)>,
    scale: u32,
}

impl View {
    fn place(&self) {
        let s = f64::from(self.scale.max(1));
        let (w, h) = (f64::from(self.size.0.max(1)), f64::from(self.size.1.max(1)));
        let (cw, ch) = (f64::from(self.crop.0), f64::from(self.crop.1));
        self.layer.setFrame(CGRect::new(CGPoint::new(0.0, 0.0), CGSize::new(cw / s, ch / s)));
        self.layer.setContentsRect(CGRect::new(CGPoint::new(0.0, 0.0), CGSize::new(cw / w, ch / h)));
    }

    fn show(&self, contents: CFTypeRef) {
        CATransaction::begin();
        CATransaction::setDisableActions(true);
        unsafe { self.layer.setContents(Some(&*(contents as *const AnyObject))) };
        self.place();
        CATransaction::commit();
    }

    pub fn video(&mut self, _: &mut Gpu, frame: Frame, crop: (u32, u32), _: [f32; 6], scale: u32) -> bool {
        let Some(buffer) = frame.pixel_buffer() else { return false };
        let surface = unsafe { CVPixelBufferGetIOSurface(buffer) };
        if surface.is_null() {
            return false;
        }
        self.pixels.clear();
        self.last_band = None;
        self.size = frame.size();
        self.crop = crop;
        self.scale = scale;
        self.show(surface);
        self.frames.push_back(frame);
        while self.frames.len() > 3 {
            self.frames.pop_front();
        }
        true
    }

    pub fn pixels(&mut self, _: &mut Gpu, size: (u32, u32), y0: u32, rows: u32, data: &[u8], scale: u32) {
        let row = size.0 as usize * 4;
        if self.size != size || self.pixels.len() != 2 {
            if y0 != 0 || rows != size.1 {
                return;
            }
            let (Some(a), Some(b)) = (surface(size.0, size.1), surface(size.0, size.1)) else { return };
            write(&b, 0, rows, row, data);
            self.pixels = vec![a, b];
            self.frames.clear();
            self.last_band = Some((0, rows));
        } else {
            write(&self.pixels[1], y0, rows, row, data);
            if let Some((py, pr)) = self.last_band {
                let (lo, hi) = (py.max(y0 + rows).min(py + pr), (py + pr));
                if py < y0 {
                    copy_rows(&self.pixels[0], &self.pixels[1], py, y0.min(py + pr) - py, row);
                }
                if lo < hi {
                    copy_rows(&self.pixels[0], &self.pixels[1], lo, hi - lo, row);
                }
            }
            self.last_band = Some((y0, rows));
        }
        self.size = size;
        self.crop = size;
        self.scale = scale;
        self.show(self.pixels[1].0);
        self.pixels.swap(0, 1);
    }

    pub fn resized(&mut self, _: &mut Gpu, _: &Window) {}

    pub fn draw(&mut self, _: &mut Gpu, _: &Window) {}
}
