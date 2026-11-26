#[path = "direct.rs"]
mod direct;
#[path = "remote.rs"]
mod remote;

use std::{
    fs::File,
    io::{self, Write},
    os::{
        fd::{AsFd, AsRawFd, FromRawFd, OwnedFd},
        unix::{
            fs::FileExt,
            net::{UnixListener, UnixStream},
        },
    },
    sync::{Arc, Mutex},
    time::Instant,
};
use wayland_protocols::xdg::{
    shell::server::{xdg_popup, xdg_positioner, xdg_surface, xdg_toplevel, xdg_wm_base},
    xdg_output::zv1::server::{zxdg_output_manager_v1, zxdg_output_v1},
};
use wayland_server::{
    Client, DataInit, Dispatch, Display, DisplayHandle, GlobalDispatch, New, Resource, WEnum,
    protocol::{
        wl_buffer, wl_callback, wl_compositor, wl_data_device, wl_data_device_manager,
        wl_data_offer, wl_data_source, wl_keyboard, wl_output, wl_pointer, wl_region, wl_seat,
        wl_shm, wl_shm_pool, wl_subcompositor, wl_subsurface, wl_surface, wl_touch,
    },
};

const WIDTH: i32 = 1920;
const HEIGHT: i32 = 1200;
const MAX_EDGE: i32 = 4096;

struct Pool {
    file: Arc<File>,
    size: Mutex<i32>,
}

struct Buffer {
    file: Arc<File>,
    offset: u64,
    width: i32,
    height: i32,
    stride: i32,
    opaque: bool,
}

#[derive(Default, Clone, Copy)]
struct Positioner {
    width: i32,
    height: i32,
    anchor_x: i32,
    anchor_y: i32,
    anchor_w: i32,
    anchor_h: i32,
    offset_x: i32,
    offset_y: i32,
    anchor: u32,
    gravity: u32,
}

impl Positioner {
    fn place(&self) -> (i32, i32, i32, i32) {
        let (w, h) = (self.width.clamp(1, MAX_EDGE), self.height.clamp(1, MAX_EDGE));
        let ax = match self.anchor {
            3 | 5 | 6 => self.anchor_x,
            4 | 7 | 8 => self.anchor_x + self.anchor_w,
            _ => self.anchor_x + self.anchor_w / 2,
        };
        let ay = match self.anchor {
            1 | 5 | 7 => self.anchor_y,
            2 | 6 | 8 => self.anchor_y + self.anchor_h,
            _ => self.anchor_y + self.anchor_h / 2,
        };
        let x = match self.gravity {
            3 | 5 | 6 => ax - w,
            4 | 7 | 8 => ax,
            _ => ax - w / 2,
        };
        let y = match self.gravity {
            1 | 5 | 7 => ay - h,
            2 | 6 | 8 => ay,
            _ => ay - h / 2,
        };
        (x + self.offset_x, y + self.offset_y, w, h)
    }

    fn flipped(&self, horizontal: bool) -> Self {
        let swap = |v: u32| match (horizontal, v) {
            (true, 3) => 4,
            (true, 4) => 3,
            (true, 5) => 7,
            (true, 7) => 5,
            (true, 6) => 8,
            (true, 8) => 6,
            (false, 1) => 2,
            (false, 2) => 1,
            (false, 5) => 6,
            (false, 6) => 5,
            (false, 7) => 8,
            (false, 8) => 7,
            (_, v) => v,
        };
        Self {
            anchor: swap(self.anchor),
            gravity: swap(self.gravity),
            offset_x: if horizontal { -self.offset_x } else { self.offset_x },
            offset_y: if horizontal { self.offset_y } else { -self.offset_y },
            ..*self
        }
    }
}

#[derive(Default)]
struct Surface {
    layer_id: u32,
    pending: Option<Option<wl_buffer::WlBuffer>>,
    callbacks: Vec<wl_callback::WlCallback>,
    role: Option<xdg_surface::XdgSurface>,
    top: Option<xdg_toplevel::XdgToplevel>,
    popup: Option<xdg_popup::XdgPopup>,
    popup_rect: Option<(i32, i32, i32, i32)>,
    pending_configures: Vec<u32>,
    acknowledged: bool,
    image: Option<Picture>,
    hardware: Option<(i32, i32)>,
    acquire: Option<OwnedFd>,
    cached: Option<Option<Picture>>,
    cached_hardware: Option<Option<(wl_buffer::WlBuffer, Option<OwnedFd>)>>,
    parent: Option<wl_surface::WlSurface>,
    children: Vec<wl_surface::WlSurface>,
    position: (i32, i32),
    pending_position: Option<(i32, i32)>,
    synchronous: bool,
    below: bool,
    window_id: u32,
    buffer_scale: i32,
    pending_scale: Option<i32>,
    geometry: Option<(i32, i32, i32, i32)>,
    title: String,
    viewport: Option<(i32, i32)>,
    pending_viewport: Option<Option<(i32, i32)>>,
    source: Option<[f64; 4]>,
    pending_source: Option<Option<[f64; 4]>>,
    input: Option<Rects>,
    pending_input: Option<Option<Rects>>,
}

type Rects = Vec<(i32, i32, i32, i32)>;

struct Picture {
    width: i32,
    height: i32,
    pixels: Vec<u8>,
}

struct Subsurface {
    child: wl_surface::WlSurface,
    parent: wl_surface::WlSurface,
}

impl Surface {
    fn configure_xdg(&mut self, serial: u32) {
        let role = self.role.as_ref().unwrap();
        if self.pending_configures.len() >= 128 {
            role.post_error(xdg_surface::Error::InvalidSerial, "too many unacknowledged configures");
            return;
        }
        role.configure(serial);
        self.pending_configures.push(serial);
    }
}

fn info(surface: &wl_surface::WlSurface) -> &Mutex<Surface> {
    surface.data::<Mutex<Surface>>().unwrap()
}

fn synchronized(surface: &wl_surface::WlSurface) -> bool {
    let state = info(surface).lock().unwrap();
    let parent = state.parent.clone();
    let sync = state.synchronous;
    drop(state);
    parent.is_some_and(|p| sync || synchronized(&p))
}

enum HardwareCommit {
    Present(wl_surface::WlSurface, wl_buffer::WlBuffer, Option<OwnedFd>),
    Remove(u32),
    Relayout(wl_surface::WlSurface),
}

fn apply_children(surface: &wl_surface::WlSurface, commits: &mut Vec<HardwareCommit>) {
    let children = info(surface).lock().unwrap().children.clone();
    for child in children {
        let mut data = info(&child).lock().unwrap();
        let mut moved = false;
        if let Some(position) = data.pending_position.take()
            && data.position != position
        {
            data.position = position;
            moved = true;
        }
        if let Some(image) = data.cached.take() {
            data.image = image;
        }
        let mut hardware_committed = false;
        if let Some(cached_hw) = data.cached_hardware.take() {
            hardware_committed = true;
            match cached_hw {
                Some((buf, fence)) => {
                    if let Some(hw) = buf.data::<direct::Buffer>() {
                        data.hardware = Some((hw.width, hw.height));
                        data.image = None;
                        commits.push(HardwareCommit::Present(child.clone(), buf, fence));
                    } else {
                        buf.release();
                    }
                }
                None => {
                    if data.hardware.take().is_some() {
                        commits.push(HardwareCommit::Remove(data.layer_id));
                    }
                }
            }
        } else if moved && data.hardware.is_some() {
            commits.push(HardwareCommit::Relayout(child.clone()));
        }
        let _ = hardware_committed;
        drop(data);
        apply_children(&child, commits);
    }
}

fn root_of(surface: &wl_surface::WlSurface) -> (wl_surface::WlSurface, (i32, i32)) {
    let mut cur = surface.clone();
    let mut x = 0i32;
    let mut y = 0i32;
    for _ in 0..30 {
        let data = info(&cur).lock().unwrap();
        let parent = data.parent.clone();
        if let Some(p) = parent {
            x = x.saturating_add(data.position.0);
            y = y.saturating_add(data.position.1);
            drop(data);
            cur = p;
        } else {
            break;
        }
    }
    (cur, (x, y))
}

fn has_any_content(surface: &wl_surface::WlSurface) -> bool {
    let data = info(surface).lock().unwrap();
    if data.image.is_some() || data.hardware.is_some() {
        return true;
    }
    let children = data.children.clone();
    drop(data);
    children.iter().any(has_any_content)
}

fn hardware_z_order(root: &wl_surface::WlSurface, target: &wl_surface::WlSurface) -> i32 {
    fn walk(
        surface: &wl_surface::WlSurface,
        target: &wl_surface::WlSurface,
        rank: &mut i32,
    ) -> Option<i32> {
        let data = info(surface).lock().unwrap();
        let has_hw = data.hardware.is_some();
        let children = data.children.clone();
        drop(data);
        let under = |sub: &wl_surface::WlSurface| info(sub).lock().unwrap().below;
        for child in children.iter().filter(|s| under(s)) {
            if let Some(found) = walk(child, target, rank) {
                return Some(found);
            }
        }
        if has_hw {
            *rank += 1;
            if surface == target {
                return Some(*rank);
            }
        }
        for child in children.iter().filter(|s| !under(s)) {
            if let Some(found) = walk(child, target, rank) {
                return Some(found);
            }
        }
        None
    }
    let mut rank = 0;
    walk(root, target, &mut rank).unwrap_or(1)
}

struct State {
    direct: Option<direct::Direct>,
    keys: Vec<wl_keyboard::WlKeyboard>,
    pointers: Vec<wl_pointer::WlPointer>,
    focus: Option<wl_surface::WlSurface>,
    toplevels: Vec<wl_surface::WlSurface>,
    remote: Option<remote::Remote>,
    decorations: Vec<wayland_protocols::xdg::decoration::zv1::server::zxdg_toplevel_decoration_v1::ZxdgToplevelDecorationV1>,
    next_window: u32,
    scale: i32,
    callbacks: Vec<wl_callback::WlCallback>,
    serial: u32,
    next_layer: u32,
    keymap: File,
    keymap_size: u32,
    down: Vec<u32>,
    modifiers: u32,
    output_w: i32,
    output_h: i32,
    outputs: Vec<wl_output::WlOutput>,
    xdg_outputs: Vec<(zxdg_output_v1::ZxdgOutputV1, wl_output::WlOutput)>,
    pressed_button: Option<u32>,
    display: DisplayHandle,
    data_devices: Vec<wl_data_device::WlDataDevice>,
    selection: Option<wl_data_source::WlDataSource>,
}

fn keymap() -> io::Result<(File, u32)> {
    let fd = unsafe {
        libc::memfd_create(
            c"bareboot-keymap".as_ptr(),
            libc::MFD_CLOEXEC | libc::MFD_ALLOW_SEALING,
        )
    };
    if fd < 0 {
        return Err(io::Error::last_os_error());
    }
    let mut file = unsafe { File::from_raw_fd(fd) };
    let text = concat!(
        "xkb_keymap {",
        "xkb_keycodes { include \"evdev+aliases(qwerty)\" };",
        "xkb_types { include \"complete\" };",
        "xkb_compatibility { include \"complete\" };",
        "xkb_symbols { include \"pc+us+inet(evdev)\" };",
        "};\0"
    );
    file.write_all(text.as_bytes())?;
    if unsafe {
        libc::fcntl(
            fd,
            libc::F_ADD_SEALS,
            libc::F_SEAL_SEAL | libc::F_SEAL_SHRINK | libc::F_SEAL_GROW | libc::F_SEAL_WRITE,
        )
    } < 0
    {
        return Err(io::Error::last_os_error());
    }
    Ok((file, text.len() as u32))
}

impl State {
    fn next_serial(&mut self) -> u32 {
        self.serial = self.serial.wrapping_add(1).max(1);
        self.serial
    }

    fn next_layer_id(&mut self) -> u32 {
        self.next_layer = self.next_layer.wrapping_add(1).max(1);
        self.next_layer
    }

    fn present_hardware(
        &mut self,
        surface: &wl_surface::WlSurface,
        buffer: wl_buffer::WlBuffer,
        fence: Option<OwnedFd>,
    ) {
        self.remote_present(surface, buffer, fence);
    }

    fn time(&self) -> u32 {
        let mut now = libc::timespec { tv_sec: 0, tv_nsec: 0 };
        unsafe { libc::clock_gettime(libc::CLOCK_MONOTONIC, &mut now) };
        (now.tv_sec as u64 * 1000 + now.tv_nsec as u64 / 1_000_000) as u32
    }

    fn unmap(&mut self, surface: &wl_surface::WlSurface) {
        self.toplevels
            .retain(|window| window != surface && window.is_alive());
        self.remote_unmap(surface);
    }

    fn offer_selection(&self, device: &wl_data_device::WlDataDevice) {
        let Some(source) = self.selection.as_ref().filter(|s| s.is_alive()) else {
            device.selection(None);
            return;
        };
        if let Some(client) = device.client()
            && let Ok(offer) = client.create_resource::<wl_data_offer::WlDataOffer, _, State>(
                &self.display,
                device.version(),
                source.clone(),
            )
        {
            device.data_offer(&offer);
            for mime in source
                .data::<Mutex<Vec<String>>>()
                .unwrap()
                .lock()
                .unwrap()
                .iter()
            {
                offer.offer(mime.clone());
            }
            device.selection(Some(&offer));
        }
    }

    fn update_selection(&self) {
        if let Some(focus) = &self.focus {
            for device in self
                .data_devices
                .iter()
                .filter(|d| d.id().same_client_as(&focus.id()))
            {
                self.offer_selection(device);
            }
        }
    }

    fn release_input(&mut self) {
        self.release_keys();
        self.pointer_release();
    }

    fn release_keys(&mut self) {
        for key in std::mem::take(&mut self.down) {
            self.key(key, false);
        }
    }

    fn key(&mut self, key: u32, pressed: bool) {
        if key > 255 {
            return;
        }
        if pressed {
            if self.down.contains(&key) {
                return;
            }
            self.down.push(key);
        } else {
            self.down.retain(|&k| k != key);
        }
        let mut mask = 0;
        for &k in &self.down {
            mask |= match k {
                42 | 54 => 1,
                29 | 97 => 4,
                56 | 100 => 8,
                125 | 126 => 64,
                _ => 0,
            };
        }
        let serial = self.next_serial();
        let when = self.time();
        if let Some(target) = self.focus.as_ref() {
            for keyboard in &self.keys {
                if !keyboard.id().same_client_as(&target.id()) {
                    continue;
                }
                keyboard.key(
                    serial,
                    when,
                    key,
                    if pressed {
                        wl_keyboard::KeyState::Pressed
                    } else {
                        wl_keyboard::KeyState::Released
                    },
                );
                if self.modifiers != mask {
                    keyboard.modifiers(serial, mask, 0, 0, 0);
                }
            }
        }
        self.modifiers = mask;
    }
}

fn picture(surface: &wl_surface::WlSurface, buffer: &wl_buffer::WlBuffer) -> Option<Picture> {
    let buffer = buffer.data::<Buffer>()?;
    let row = buffer.width as usize * 4;
    let mut data = vec![0; row * buffer.height as usize];
    for (y, pixels) in data.chunks_exact_mut(row).enumerate() {
        let offset = buffer.offset + y as u64 * buffer.stride as u64;
        if buffer.file.read_exact_at(pixels, offset).is_err() {
            surface.post_error(0u32, "could not read shared buffer");
            return None;
        }
        if buffer.opaque {
            for pixel in pixels.as_chunks_mut::<4>().0 {
                pixel[3] = 255;
            }
        }
    }
    Some(Picture {
        width: buffer.width,
        height: buffer.height,
        pixels: data,
    })
}

macro_rules! global {
    ($ty:path, |$r:ident| $on_bind:block) => {
        impl GlobalDispatch<$ty, ()> for State {
            fn bind(
                _: &mut Self,
                _: &DisplayHandle,
                _: &Client,
                id: New<$ty>,
                _: &(),
                init: &mut DataInit<'_, Self>,
            ) {
                let $r = init.init(id, ());
                $on_bind
            }
        }
    };
}

global!(wl_compositor::WlCompositor, |_r| {});
global!(wl_data_device_manager::WlDataDeviceManager, |_r| {});
global!(wl_subcompositor::WlSubcompositor, |_r| {});
global!(xdg_wm_base::XdgWmBase, |_r| {});
global!(wl_shm::WlShm, |r| {
    r.format(wl_shm::Format::Argb8888);
    r.format(wl_shm::Format::Xrgb8888);
});
global!(wl_seat::WlSeat, |r| {
    r.capabilities(wl_seat::Capability::Pointer | wl_seat::Capability::Keyboard);
    if r.version() >= 2 {
        r.name("bareboot".into());
    }
});
impl GlobalDispatch<wl_output::WlOutput, ()> for State {
    fn bind(
        state: &mut Self,
        _: &DisplayHandle,
        _: &Client,
        resource: New<wl_output::WlOutput>,
        _: &(),
        init: &mut DataInit<'_, Self>,
    ) {
        let r = init.init(resource, ());
        r.geometry(
            0,
            0,
            190,
            119,
            wl_output::Subpixel::Unknown,
            "Bareboot".into(),
            "Display".into(),
            wl_output::Transform::Normal,
        );
        r.mode(
            wl_output::Mode::Current | wl_output::Mode::Preferred,
            state.output_w,
            state.output_h,
            120_000,
        );
        if r.version() >= 2 {
            r.scale(state.scale);
        }
        if r.version() >= 4 {
            r.name("Bareboot-1".into());
            r.description("Bareboot Display".into());
        }
        if r.version() >= 2 {
            r.done();
        }
        for top in &state.toplevels {
            if top.is_alive() && top.id().same_client_as(&r.id()) {
                top.enter(&r);
            }
        }
        state.outputs.push(r);
    }
}
global!(zxdg_output_manager_v1::ZxdgOutputManagerV1, |_r| {});

macro_rules! noop {
    ($m:ident :: $t:ident, $data:ty) => {
        impl Dispatch<$m::$t, $data> for State {
            fn request(
                _: &mut Self,
                _: &Client,
                _: &$m::$t,
                _: $m::Request,
                _: &$data,
                _: &DisplayHandle,
                _: &mut DataInit<'_, Self>,
            ) {
            }
        }
    };
}
impl Dispatch<wl_region::WlRegion, Mutex<Vec<(i32, i32, i32, i32)>>> for State {
    fn request(
        _: &mut Self,
        _: &Client,
        _: &wl_region::WlRegion,
        req: wl_region::Request,
        data: &Mutex<Vec<(i32, i32, i32, i32)>>,
        _: &DisplayHandle,
        _: &mut DataInit<'_, Self>,
    ) {
        match req {
            wl_region::Request::Add {
                x,
                y,
                width,
                height,
            } if width > 0 && height > 0 => {
                data.lock().unwrap().push((x, y, width, height));
            }
            wl_region::Request::Subtract { width, height, .. } if width > 0 && height > 0 => {
                data.lock().unwrap().clear();
            }
            _ => {}
        }
    }
}
noop!(wl_callback::WlCallback, ());
noop!(wl_buffer::WlBuffer, Buffer);
noop!(wl_pointer::WlPointer, ());
noop!(wl_keyboard::WlKeyboard, ());
noop!(wl_touch::WlTouch, ());
noop!(wl_output::WlOutput, ());
noop!(zxdg_output_v1::ZxdgOutputV1, ());

impl Dispatch<zxdg_output_manager_v1::ZxdgOutputManagerV1, ()> for State {
    fn request(
        state: &mut Self,
        _: &Client,
        _: &zxdg_output_manager_v1::ZxdgOutputManagerV1,
        req: zxdg_output_manager_v1::Request,
        _: &(),
        _: &DisplayHandle,
        init: &mut DataInit<'_, Self>,
    ) {
        if let zxdg_output_manager_v1::Request::GetXdgOutput { id, output } = req {
            let xdg_out = init.init(id, ());
            xdg_out.logical_position(0, 0);
            xdg_out.logical_size(state.output_w, state.output_h);
            if xdg_out.version() >= 2 {
                xdg_out.name("Bareboot-1".into());
                xdg_out.description("Bareboot Display".into());
            }
            if xdg_out.version() < 3 {
                xdg_out.done();
            } else {
                output.done();
            }
            state.xdg_outputs.push((xdg_out, output));
        }
    }
}


impl Dispatch<wl_data_device_manager::WlDataDeviceManager, ()> for State {
    fn request(
        state: &mut Self,
        _: &Client,
        _: &wl_data_device_manager::WlDataDeviceManager,
        req: wl_data_device_manager::Request,
        _: &(),
        _: &DisplayHandle,
        init: &mut DataInit<'_, Self>,
    ) {
        match req {
            wl_data_device_manager::Request::CreateDataSource { id } => {
                init.init(id, Mutex::new(Vec::<String>::new()));
            }
            wl_data_device_manager::Request::GetDataDevice { id, .. } => {
                let device = init.init(id, ());
                if state
                    .focus
                    .as_ref()
                    .is_some_and(|s| s.id().same_client_as(&device.id()))
                {
                    state.offer_selection(&device);
                }
                state.data_devices.push(device);
            }
            _ => {}
        }
    }
}

impl Dispatch<wl_data_device::WlDataDevice, ()> for State {
    fn request(
        state: &mut Self,
        _: &Client,
        device: &wl_data_device::WlDataDevice,
        req: wl_data_device::Request,
        _: &(),
        _: &DisplayHandle,
        _: &mut DataInit<'_, Self>,
    ) {
        match req {
            wl_data_device::Request::SetSelection { source, serial } => {
                if serial == 0
                    || state.serial.wrapping_sub(serial) > 256
                    || !state
                        .focus
                        .as_ref()
                        .is_some_and(|s| s.id().same_client_as(&device.id()))
                {
                    return;
                }
                if state.selection != source {
                    if let Some(old) = state.selection.take() {
                        old.cancelled();
                    }
                    state.selection = source;
                    state.update_selection();
                }
            }
            wl_data_device::Request::StartDrag {
                source: Some(source),
                ..
            } => source.cancelled(),
            _ => {}
        }
    }
}

impl Dispatch<wl_data_source::WlDataSource, Mutex<Vec<String>>> for State {
    fn request(
        _: &mut Self,
        _: &Client,
        _: &wl_data_source::WlDataSource,
        req: wl_data_source::Request,
        data: &Mutex<Vec<String>>,
        _: &DisplayHandle,
        _: &mut DataInit<'_, Self>,
    ) {
        if let wl_data_source::Request::Offer { mime_type } = req {
            let mut types = data.lock().unwrap();
            if mime_type.len() <= 256 && types.len() < 32 && !types.contains(&mime_type) {
                types.push(mime_type);
            }
        }
    }

    fn destroyed(
        state: &mut Self,
        _: wayland_server::backend::ClientId,
        source: &wl_data_source::WlDataSource,
        _: &Mutex<Vec<String>>,
    ) {
        if state.selection.as_ref() == Some(source) {
            state.selection = None;
            state.update_selection();
        }
    }
}

impl Dispatch<wl_data_offer::WlDataOffer, wl_data_source::WlDataSource> for State {
    fn request(
        _: &mut Self,
        _: &Client,
        _: &wl_data_offer::WlDataOffer,
        req: wl_data_offer::Request,
        source: &wl_data_source::WlDataSource,
        _: &DisplayHandle,
        _: &mut DataInit<'_, Self>,
    ) {
        if let wl_data_offer::Request::Receive { mime_type, fd } = req
            && source.is_alive()
            && source
                .data::<Mutex<Vec<String>>>()
                .unwrap()
                .lock()
                .unwrap()
                .contains(&mime_type)
        {
            source.send(mime_type, fd.as_fd());
        }
    }
}

impl Dispatch<wl_compositor::WlCompositor, ()> for State {
    fn request(
        state: &mut Self,
        _: &Client,
        _: &wl_compositor::WlCompositor,
        req: wl_compositor::Request,
        _: &(),
        _: &DisplayHandle,
        init: &mut DataInit<'_, Self>,
    ) {
        match req {
            wl_compositor::Request::CreateSurface { id } => {
                let surface = Surface {
                    layer_id: state.next_layer_id(),
                    ..Default::default()
                };
                init.init(id, Mutex::new(surface));
            }
            wl_compositor::Request::CreateRegion { id } => {
                init.init(id, Mutex::new(Vec::new()));
            }
            _ => {}
        }
    }
}

impl Dispatch<wl_shm::WlShm, ()> for State {
    fn request(
        _: &mut Self,
        _: &Client,
        resource: &wl_shm::WlShm,
        req: wl_shm::Request,
        _: &(),
        _: &DisplayHandle,
        init: &mut DataInit<'_, Self>,
    ) {
        if let wl_shm::Request::CreatePool { id, fd, size } = req {
            let file = File::from(fd);
            if size <= 0
                || !file
                    .metadata()
                    .is_ok_and(|m| m.is_file() && m.len() >= size as u64)
            {
                init.post_error(id, wl_shm::Error::InvalidFd, "invalid shared pool size");
                return;
            }
            init.init(
                id,
                Pool {
                    file: Arc::new(file),
                    size: Mutex::new(size),
                },
            );
        }
        let _ = resource;
    }
}

impl Dispatch<wl_shm_pool::WlShmPool, Pool> for State {
    fn request(
        _: &mut Self,
        _: &Client,
        resource: &wl_shm_pool::WlShmPool,
        req: wl_shm_pool::Request,
        pool: &Pool,
        _: &DisplayHandle,
        init: &mut DataInit<'_, Self>,
    ) {
        match req {
            wl_shm_pool::Request::CreateBuffer {
                id,
                offset,
                width,
                height,
                stride,
                format,
            } => {
                let opaque = match format {
                    WEnum::Value(wl_shm::Format::Xrgb8888) => true,
                    WEnum::Value(wl_shm::Format::Argb8888) => false,
                    _ => {
                        init.post_error(
                            id,
                            wl_shm::Error::InvalidFormat,
                            "expected XRGB8888 or ARGB8888",
                        );
                        return;
                    }
                };
                let end = i64::from(offset)
                    + i64::from(height.saturating_sub(1)) * i64::from(stride)
                    + 4 * i64::from(width);
                if offset < 0
                    || !(1..=MAX_EDGE).contains(&width)
                    || !(1..=MAX_EDGE).contains(&height)
                    || stride < width.saturating_mul(4)
                    || end > i64::from(*pool.size.lock().unwrap())
                {
                    init.post_error(
                        id,
                        wl_shm::Error::InvalidStride,
                        "invalid shared buffer dimensions",
                    );
                    return;
                }
                init.init(
                    id,
                    Buffer {
                        file: pool.file.clone(),
                        offset: offset as u64,
                        width,
                        height,
                        stride,
                        opaque,
                    },
                );
            }
            wl_shm_pool::Request::Resize { size } => {
                let mut old = pool.size.lock().unwrap();
                if size < *old || !pool.file.metadata().is_ok_and(|m| m.len() >= size as u64) {
                    resource.post_error(wl_shm::Error::InvalidFd, "invalid shared pool resize");
                } else {
                    *old = size;
                }
            }
            _ => {}
        }
    }
}

impl Dispatch<wl_surface::WlSurface, Mutex<Surface>> for State {
    fn request(
        state: &mut Self,
        _: &Client,
        resource: &wl_surface::WlSurface,
        req: wl_surface::Request,
        data: &Mutex<Surface>,
        _: &DisplayHandle,
        init: &mut DataInit<'_, Self>,
    ) {
        let mut surface = data.lock().unwrap();
        match req {
            wl_surface::Request::Attach { buffer, .. } => surface.pending = Some(buffer),
            wl_surface::Request::SetBufferScale { scale } => {
                if scale < 1 {
                    resource.post_error(wl_surface::Error::InvalidScale, "buffer scale must be positive");
                    return;
                }
                surface.pending_scale = Some(scale.min(8));
            }
            wl_surface::Request::SetInputRegion { region } => {
                surface.pending_input = Some(
                    region.and_then(|r| r.data::<Mutex<Vec<(i32, i32, i32, i32)>>>().map(|m| m.lock().unwrap().clone())),
                );
            }
            wl_surface::Request::Frame { callback } => {
                surface.callbacks.push(init.init(callback, ()))
            }
            wl_surface::Request::Commit => {
                if let Some(role) = &surface.role
                    && !surface.acknowledged
                    && surface.pending_configures.is_empty()
                {
                    if let Some(top) = &surface.top {
                        if matches!(surface.pending, Some(Some(_))) {
                            role.post_error(
                                xdg_surface::Error::UnconfiguredBuffer,
                                "buffer committed before configure",
                            );
                            return;
                        }
                        let serial = state.next_serial();
                        top.configure(0, 0, Vec::new());
                        surface.configure_xdg(serial);
                    } else if let Some(popup) = &surface.popup {
                        if matches!(surface.pending, Some(Some(_))) {
                            role.post_error(
                                xdg_surface::Error::UnconfiguredBuffer,
                                "buffer committed before configure",
                            );
                            return;
                        }
                        let (x, y, w, h) = surface.popup_rect.unwrap_or((0, 0, 200, 200));
                        let serial = state.next_serial();
                        popup.configure(x, y, w, h);
                        surface.configure_xdg(serial);
                    }
                }
                drop(surface);
                let sync = synchronized(resource);
                let was_mapped = info(resource).lock().unwrap().top.is_some() && has_any_content(resource);
                let mut surface = data.lock().unwrap();
                let layer_id = surface.layer_id;
                let was_hardware = surface.hardware.is_some();
                let mut direct_frame = None;
                let mut removed_hardware = false;
                if let Some(scale) = surface.pending_scale.take() {
                    surface.buffer_scale = scale;
                }
                let mut viewport_changed = false;
                if let Some(viewport) = surface.pending_viewport.take() {
                    viewport_changed |= surface.viewport != viewport;
                    surface.viewport = viewport;
                }
                if let Some(source) = surface.pending_source.take() {
                    viewport_changed |= surface.source != source;
                    surface.source = source;
                }
                if let Some(input) = surface.pending_input.take() {
                    surface.input = input;
                }
                if let Some(buffer) = surface.pending.take() {
                    let acquire = surface.acquire.take();
                    if let Some(buffer) = buffer {
                        if let Some(role) = &surface.role
                            && !surface.acknowledged
                        {
                            role.post_error(
                                xdg_surface::Error::UnconfiguredBuffer,
                                "buffer committed before ack",
                            );
                            return;
                        }
                        if let Some(hardware) = buffer.data::<direct::Buffer>() {
                            if sync {
                                if let Some(Some((old, _))) = surface.cached_hardware.take() {
                                    old.release();
                                }
                                surface.cached_hardware = Some(Some((buffer, acquire)));
                                surface.cached = Some(None);
                            } else {
                                surface.hardware = Some((hardware.width, hardware.height));
                                surface.image = None;
                                direct_frame = Some((buffer, acquire));
                            }
                        } else {
                            let picture = picture(resource, &buffer);
                            buffer.release();
                            if sync {
                                if let Some(Some((old, _))) = surface.cached_hardware.take() {
                                    old.release();
                                }
                                surface.cached_hardware = Some(None);
                                surface.cached = Some(picture);
                            } else {
                                if surface.hardware.take().is_some() {
                                    removed_hardware = true;
                                }
                                surface.image = picture;
                            }
                        }
                    } else if sync {
                        if let Some(Some((old, _))) = surface.cached_hardware.take() {
                            old.release();
                        }
                        surface.cached_hardware = Some(None);
                        surface.cached = Some(None);
                    } else {
                        if surface.hardware.take().is_some() {
                            removed_hardware = true;
                        }
                        if surface.image.take().is_some() {
                        }
                    }
                }
                let size = surface.hardware;
                let is_top = surface.top.is_some();
                state.callbacks.append(&mut surface.callbacks);
                drop(surface);
                let mut child_commits = Vec::new();
                if !sync {
                    apply_children(resource, &mut child_commits);
                }
                let mapped = is_top && has_any_content(resource);
                if mapped && !was_mapped {
                    if !state.toplevels.contains(resource) {
                        state.toplevels.push(resource.clone());
                    }
                    for out in &state.outputs {
                        if out.is_alive() && out.id().same_client_as(&resource.id()) {
                            resource.enter(out);
                        }
                    }
                    state.remote_map(resource);
                } else if !mapped && was_mapped {
                    state.unmap(resource);
                }
                if removed_hardware || (was_hardware && size.is_none() && !sync) {
                    state.remote_remove_layer(resource, layer_id);
                }
                if let Some((buffer, fence)) = direct_frame {
                    state.present_hardware(resource, buffer, fence);
                } else if viewport_changed && size.is_some() {
                    state.remote_relayout(resource);
                }
                for commit in child_commits {
                    match commit {
                        HardwareCommit::Present(child, buf, fence) => {
                            state.present_hardware(&child, buf, fence);
                        }
                        HardwareCommit::Remove(id) => state.remote_remove_layer(resource, id),
                        HardwareCommit::Relayout(child) => state.remote_relayout(&child),
                    }
                }
                if !sync {
                    state.remote_dirty(resource);
                }
            }
            _ => {}
        }
    }

    fn destroyed(
        state: &mut Self,
        _: wayland_server::backend::ClientId,
        resource: &wl_surface::WlSurface,
        data: &Mutex<Surface>,
    ) {
        state.remote_forget(resource);
        let mut surface = data.lock().unwrap();
        surface.hardware.take();
        let parent = surface.parent.take();
        let children = std::mem::take(&mut surface.children);
        surface.role.take();
        surface.top.take();
        surface.popup.take();
        surface.cached.take();
        if let Some(Some((old, _))) = surface.cached_hardware.take() {
            old.release();
        }
        surface.image.take();
        drop(surface);
        if let Some(parent) = parent {
            info(&parent)
                .lock()
                .unwrap()
                .children
                .retain(|s| s != resource);
        }
        for child in children {
            info(&child).lock().unwrap().parent.take();
        }
        state.unmap(resource);
    }
}

impl Dispatch<wl_subcompositor::WlSubcompositor, ()> for State {
    fn request(
        _: &mut Self,
        _: &Client,
        _: &wl_subcompositor::WlSubcompositor,
        req: wl_subcompositor::Request,
        _: &(),
        _: &DisplayHandle,
        init: &mut DataInit<'_, Self>,
    ) {
        if let wl_subcompositor::Request::GetSubsurface {
            id,
            surface,
            parent,
        } = req
        {
            let mut ancestor = parent.clone();
            let mut valid = false;
            for _ in 0..30 {
                if ancestor == surface {
                    break;
                }
                let next = info(&ancestor).lock().unwrap().parent.clone();
                match next {
                    Some(p) => ancestor = p,
                    None => {
                        valid = true;
                        break;
                    }
                }
            }
            let mut child = info(&surface).lock().unwrap();
            if !valid || child.role.is_some() || child.parent.is_some() {
                init.post_error(
                    id,
                    wl_subcompositor::Error::BadSurface,
                    "invalid subsurface parent or role",
                );
                return;
            }
            child.parent = Some(parent.clone());
            child.synchronous = true;
            drop(child);
            info(&parent).lock().unwrap().children.push(surface.clone());
            init.init(
                id,
                Subsurface {
                    child: surface,
                    parent,
                },
            );
        }
    }
}

impl Dispatch<wl_subsurface::WlSubsurface, Subsurface> for State {
    fn request(
        state: &mut Self,
        _: &Client,
        resource: &wl_subsurface::WlSubsurface,
        req: wl_subsurface::Request,
        sub: &Subsurface,
        _: &DisplayHandle,
        _: &mut DataInit<'_, Self>,
    ) {
        match req {
            wl_subsurface::Request::SetPosition { x, y } => {
                info(&sub.child).lock().unwrap().pending_position = Some((x, y));
            }
            wl_subsurface::Request::SetSync => info(&sub.child).lock().unwrap().synchronous = true,
            wl_subsurface::Request::SetDesync => {
                info(&sub.child).lock().unwrap().synchronous = false;
                if !synchronized(&sub.child) {
                    let mut hw_commit = None;
                    {
                        let mut child = info(&sub.child).lock().unwrap();
                        if let Some(image) = child.cached.take() {
                            child.image = image;
                        }
                        if let Some(cached_hw) = child.cached_hardware.take() {
                            match cached_hw {
                                Some((buf, fence)) => {
                                    if let Some(hw) = buf.data::<direct::Buffer>() {
                                        child.hardware = Some((hw.width, hw.height));
                                        child.image = None;
                                        hw_commit = Some(HardwareCommit::Present(
                                            sub.child.clone(),
                                            buf,
                                            fence,
                                        ));
                                    } else {
                                        buf.release();
                                    }
                                }
                                None => {
                                    if child.hardware.take().is_some() {
                                        hw_commit = Some(HardwareCommit::Remove(child.layer_id));
                                    }
                                }
                            }
                        }
                    }
                    match hw_commit {
                        Some(HardwareCommit::Present(child, buf, fence)) => {
                            state.present_hardware(&child, buf, fence);
                        }
                        Some(HardwareCommit::Remove(id)) => state.remote_remove_layer(&sub.child, id),
                        _ => {}
                    }
                }
            }
            wl_subsurface::Request::PlaceAbove { ref sibling }
            | wl_subsurface::Request::PlaceBelow { ref sibling } => {
                let above = matches!(req, wl_subsurface::Request::PlaceAbove { .. });
                if sibling == &sub.child {
                    resource.post_error(
                        wl_subsurface::Error::BadSurface,
                        "cannot stack a surface against itself",
                    );
                    return;
                }
                let relative_parent = sibling == &sub.parent;
                let mut parent = info(&sub.parent).lock().unwrap();
                if !relative_parent && !parent.children.contains(sibling) {
                    resource.post_error(
                        wl_subsurface::Error::BadSurface,
                        "stacking target has another parent",
                    );
                    return;
                }
                parent.children.retain(|s| s != &sub.child);
                let (index, below) = if relative_parent {
                    (
                        parent
                            .children
                            .iter()
                            .position(|s| !info(s).lock().unwrap().below)
                            .unwrap_or(parent.children.len()),
                        !above,
                    )
                } else {
                    let index = parent.children.iter().position(|s| s == sibling).unwrap();
                    (
                        index + usize::from(above),
                        info(sibling).lock().unwrap().below,
                    )
                };
                parent.children.insert(index, sub.child.clone());
                info(&sub.child).lock().unwrap().below = below;
            }
            _ => {}
        }
    }

    fn destroyed(
        state: &mut Self,
        _: wayland_server::backend::ClientId,
        _: &wl_subsurface::WlSubsurface,
        sub: &Subsurface,
    ) {
        state.remote_forget(&sub.child);
        info(&sub.parent)
            .lock()
            .unwrap()
            .children
            .retain(|s| s != &sub.child);
        let mut child = info(&sub.child).lock().unwrap();
        child.hardware.take();
        child.parent = None;
        child.image = None;
        child.cached = None;
        if let Some(Some((old, _))) = child.cached_hardware.take() {
            old.release();
        }
        drop(child);
    }
}

impl Dispatch<xdg_wm_base::XdgWmBase, ()> for State {
    fn request(
        _: &mut Self,
        _: &Client,
        resource: &xdg_wm_base::XdgWmBase,
        req: xdg_wm_base::Request,
        _: &(),
        _: &DisplayHandle,
        init: &mut DataInit<'_, Self>,
    ) {
        match req {
            xdg_wm_base::Request::CreatePositioner { id } => {
                init.init(id, Mutex::new(Positioner::default()));
            }
            xdg_wm_base::Request::GetXdgSurface { id, surface } => {
                let Some(data) = surface.data::<Mutex<Surface>>() else {
                    init.post_error(id, xdg_wm_base::Error::Role, "invalid surface");
                    return;
                };
                let mut info = data.lock().unwrap();
                if info.role.is_some() || info.parent.is_some() {
                    resource.post_error(xdg_wm_base::Error::Role, "surface already has a role");
                    init.post_error(id, xdg_wm_base::Error::Role, "surface already has a role");
                    return;
                }
                let role = init.init(id, surface.clone());
                info.role = Some(role);
            }
            _ => {}
        }
    }
}

impl Dispatch<xdg_surface::XdgSurface, wl_surface::WlSurface> for State {
    fn request(
        state: &mut Self,
        _: &Client,
        resource: &xdg_surface::XdgSurface,
        req: xdg_surface::Request,
        data: &wl_surface::WlSurface,
        _: &DisplayHandle,
        init: &mut DataInit<'_, Self>,
    ) {
        let Some(info) = data.data::<Mutex<Surface>>() else {
            return;
        };
        let mut surface = info.lock().unwrap();
        match req {
            xdg_surface::Request::GetToplevel { id } if surface.top.is_none() => {
                surface.top = Some(init.init(id, data.clone()));
            }
            xdg_surface::Request::GetToplevel { id } => {
                init.post_error(
                    id,
                    xdg_surface::Error::AlreadyConstructed,
                    "surface already has a toplevel",
                );
            }
            xdg_surface::Request::GetPopup {
                id,
                parent,
                positioner,
            } => {
                let placer = positioner
                    .data::<Mutex<Positioner>>()
                    .map(|p| *p.lock().unwrap())
                    .unwrap_or_default();
                let pos = placer.place();
                let parent_surface = parent.and_then(|p| p.data::<wl_surface::WlSurface>().cloned());
                let pos = match &parent_surface {
                    Some(parent) if state.remote.is_some() => {
                        drop(surface);
                        let pos = state.remote_place_popup(parent, &placer);
                        surface = info.lock().unwrap();
                        pos
                    }
                    _ => pos,
                };
                surface.popup = Some(init.init(id, data.clone()));
                surface.popup_rect = Some(pos);
                surface.position = (pos.0, pos.1);
                if let Some(parent_surface) = parent_surface {
                    surface.parent = Some(parent_surface.clone());
                    drop(surface);
                    self::info(&parent_surface)
                        .lock()
                        .unwrap()
                        .children
                        .push(data.clone());
                }
            }
            xdg_surface::Request::SetWindowGeometry { x, y, width, height } if width > 0 && height > 0 => {
                surface.geometry = Some((x, y, width, height));
            }
            xdg_surface::Request::AckConfigure { serial } => {
                if let Some(index) = surface.pending_configures.iter().position(|s| *s == serial) {
                    surface.pending_configures.drain(..=index);
                    surface.acknowledged = true;
                } else {
                    resource.post_error(xdg_surface::Error::InvalidSerial, "invalid configure serial");
                }
            }
            _ => {}
        }
    }
}

impl Dispatch<xdg_positioner::XdgPositioner, Mutex<Positioner>> for State {
    fn request(
        _: &mut Self,
        _: &Client,
        _: &xdg_positioner::XdgPositioner,
        req: xdg_positioner::Request,
        data: &Mutex<Positioner>,
        _: &DisplayHandle,
        _: &mut DataInit<'_, Self>,
    ) {
        let mut p = data.lock().unwrap();
        match req {
            xdg_positioner::Request::SetSize { width, height } => {
                p.width = width;
                p.height = height;
            }
            xdg_positioner::Request::SetAnchorRect {
                x,
                y,
                width,
                height,
            } => {
                p.anchor_x = x;
                p.anchor_y = y;
                p.anchor_w = width;
                p.anchor_h = height;
            }
            xdg_positioner::Request::SetOffset { x, y } => {
                p.offset_x = x;
                p.offset_y = y;
            }
            xdg_positioner::Request::SetAnchor { anchor: WEnum::Value(anchor) } => p.anchor = anchor as u32,
            xdg_positioner::Request::SetGravity { gravity: WEnum::Value(gravity) } => p.gravity = gravity as u32,
            _ => {}
        }
    }
}

impl Dispatch<xdg_popup::XdgPopup, wl_surface::WlSurface> for State {
    fn request(
        _: &mut Self,
        _: &Client,
        _: &xdg_popup::XdgPopup,
        _: xdg_popup::Request,
        _: &wl_surface::WlSurface,
        _: &DisplayHandle,
        _: &mut DataInit<'_, Self>,
    ) {
    }

    fn destroyed(
        state: &mut Self,
        _: wayland_server::backend::ClientId,
        _: &xdg_popup::XdgPopup,
        surface: &wl_surface::WlSurface,
    ) {
        state.remote_forget(surface);
        if let Some(mutex) = surface.data::<Mutex<Surface>>() {
            let mut data = mutex.lock().unwrap();
            data.hardware.take();
            let parent = data.parent.take();
            data.popup = None;
            data.popup_rect = None;
            data.image = None;
            data.pending_configures.clear();
            data.acknowledged = false;
            drop(data);
            if let Some(parent) = parent {
                info(&parent)
                    .lock()
                    .unwrap()
                    .children
                    .retain(|s| s != surface);
            }
        }
    }
}

impl Dispatch<xdg_toplevel::XdgToplevel, wl_surface::WlSurface> for State {
    fn request(
        state: &mut Self,
        _: &Client,
        _: &xdg_toplevel::XdgToplevel,
        request: xdg_toplevel::Request,
        surface: &wl_surface::WlSurface,
        _: &DisplayHandle,
        _: &mut DataInit<'_, Self>,
    ) {
        state.remote_toplevel_request(surface, request);
    }

    fn destroyed(
        state: &mut Self,
        _: wayland_server::backend::ClientId,
        _: &xdg_toplevel::XdgToplevel,
        surface: &wl_surface::WlSurface,
    ) {
        if let Some(info) = surface.data::<Mutex<Surface>>() {
            let mut data = info.lock().unwrap();
            data.top = None;
            data.image = None;
            data.hardware = None;
            data.acquire = None;
            data.pending_configures.clear();
            data.acknowledged = false;
        }
        state.unmap(surface);
    }
}

impl Dispatch<wl_seat::WlSeat, ()> for State {
    fn request(
        state: &mut Self,
        _: &Client,
        _: &wl_seat::WlSeat,
        req: wl_seat::Request,
        _: &(),
        _: &DisplayHandle,
        init: &mut DataInit<'_, Self>,
    ) {
        match req {
            wl_seat::Request::GetPointer { id } => {
                let p = init.init(id, ());
                if let Some(surface) = state.focus.clone()
                    && p.id().same_client_as(&surface.id())
                {
                    p.enter(state.next_serial(), &surface, 0., 0.);
                    if p.version() >= 5 {
                        p.frame();
                    }
                }
                state.pointers.push(p);
            }
            wl_seat::Request::GetKeyboard { id } => {
                let k = init.init(id, ());
                k.keymap(
                    wl_keyboard::KeymapFormat::XkbV1,
                    state.keymap.as_fd(),
                    state.keymap_size,
                );
                if k.version() >= 4 {
                    k.repeat_info(25, 400);
                }
                if let Some(surface) = state.focus.clone()
                    && k.id().same_client_as(&surface.id())
                {
                    let serial = state.next_serial();
                    k.enter(serial, &surface, Vec::new());
                    k.modifiers(serial, state.modifiers, 0, 0, 0);
                }
                state.keys.push(k);
            }
            wl_seat::Request::GetTouch { id } => {
                init.post_error::<wl_touch::WlTouch>(
                    id,
                    wl_seat::Error::MissingCapability,
                    "touch is sent as pointer input",
                );
            }
            _ => {}
        }
    }
}

pub fn run() -> io::Result<()> {
    let mut args = std::env::args().skip(1);
    let parent: i32 = args
        .next()
        .and_then(|s| s.parse().ok())
        .filter(|p| *p > 1)
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "missing display parent"))?;
    if unsafe { libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGKILL) } != 0 {
        return Err(io::Error::last_os_error());
    }
    if unsafe { libc::getppid() } != parent {
        return Err(io::Error::other("display parent exited"));
    }
    let fd = args
        .next()
        .and_then(|s| s.parse().ok())
        .filter(|&fd| fd >= 3)
        .ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                "missing display control socket",
            )
        })?;
    let control = unsafe { UnixStream::from_raw_fd(fd) };
    let bridge_fd = args
        .next()
        .and_then(|arg| arg.parse::<i32>().ok())
        .filter(|&fd| fd >= 3);
    let encoder_fd = args
        .next()
        .and_then(|arg| arg.parse::<i32>().ok())
        .filter(|&fd| fd >= 3)
        .map(|fd| unsafe { OwnedFd::from_raw_fd(fd) });
    let direct = bridge_fd
        .map(|fd| direct::Direct::new(unsafe { OwnedFd::from_raw_fd(fd) }, encoder_fd))
        .transpose()?;
    let listener = UnixListener::bind("wayland-0")?;
    listener.set_nonblocking(true)?;
    let mut display = Display::<State>::new().map_err(io::Error::other)?;
    let mut handle = display.handle();
    handle.create_global::<State, wl_compositor::WlCompositor, _>(4, ());
    handle.create_global::<State, wl_shm::WlShm, _>(1, ());
    handle.create_global::<State, wl_data_device_manager::WlDataDeviceManager, _>(3, ());
    handle.create_global::<State, wl_subcompositor::WlSubcompositor, _>(1, ());
    handle.create_global::<State, wl_seat::WlSeat, _>(5, ());
    handle.create_global::<State, wl_output::WlOutput, _>(4, ());
    handle.create_global::<State, zxdg_output_manager_v1::ZxdgOutputManagerV1, _>(3, ());
    handle.create_global::<State, xdg_wm_base::XdgWmBase, _>(1, ());
    handle.create_global::<State, wayland_protocols::xdg::decoration::zv1::server::zxdg_decoration_manager_v1::ZxdgDecorationManagerV1, _>(1, ());
    handle.create_global::<State, wayland_protocols::wp::viewporter::server::wp_viewporter::WpViewporter, _>(1, ());
    if direct.is_some() {
        handle.create_global::<State, direct::bareboot_ahb_v1::BarebootAhbV1, _>(1, ());
    }
    let (keymap, keymap_size) = keymap()?;
    let mut state = State {
        direct,
        keymap,
        keymap_size,
        keys: Vec::new(),
        pointers: Vec::new(),
        focus: None,
        toplevels: Vec::new(),
        remote: None,
        decorations: Vec::new(),
        next_window: 0,
        scale: 1,
        callbacks: Vec::new(),
        serial: 0,
        next_layer: 0,
        down: Vec::new(),
        modifiers: 0,
        output_w: WIDTH,
        output_h: HEIGHT,
        outputs: Vec::new(),
        xdg_outputs: Vec::new(),
        pressed_button: None,
        display: handle.clone(),
        data_devices: Vec::new(),
        selection: None,
    };
    let mut last_frame = Instant::now();
    loop {
        let mut fds = [
            libc::pollfd {
                fd: control.as_raw_fd(),
                events: libc::POLLIN,
                revents: 0,
            },
            libc::pollfd {
                fd: listener.as_raw_fd(),
                events: libc::POLLIN,
                revents: 0,
            },
            libc::pollfd {
                fd: display.as_fd().as_raw_fd(),
                events: libc::POLLIN,
                revents: 0,
            },
            libc::pollfd {
                fd: state.direct.as_ref().map_or(-1, direct::Direct::encoder_fd),
                events: libc::POLLIN,
                revents: 0,
            },
            libc::pollfd {
                fd: state.remote.as_ref().map_or(-1, remote::Remote::fd),
                events: libc::POLLIN,
                revents: 0,
            },
        ];
        let ret = unsafe { libc::poll(fds.as_mut_ptr(), fds.len() as _, 1) };
        if ret < 0 {
            let err = io::Error::last_os_error();
            if err.kind() == io::ErrorKind::Interrupted {
                continue;
            }
            return Err(err);
        }
        if fds[4].revents != 0 {
            state.remote_events();
        }
        if fds[0].revents != 0 {
            let (_, fd) = protocol::recv_frame(control.as_raw_fd())?;
            if let Some(fd) = fd {
                state.release_input();
                state.remote_detach();
                state.remote_attach(UnixStream::from(fd));
            }
        }
        for _ in 0..16 {
            match listener.accept() {
                Ok((client, _)) => {
                    let _ = handle.insert_client(client, Arc::new(()));
                }
                Err(e) if e.kind() == io::ErrorKind::WouldBlock => break,
                Err(e) => return Err(e),
            }
        }
        if let Some(direct) = &mut state.direct {
            let remote = state.remote.as_ref();
            direct.tick(|stream, packet| {
                if let Some(remote) = remote {
                    remote.send(remote::VIDEO, &[&stream.to_le_bytes(), &packet]);
                }
            });
        }
        display.dispatch_clients(&mut state)?;
        state.data_devices.retain(Resource::is_alive);
        state.keys.retain(Resource::is_alive);
        state.pointers.retain(Resource::is_alive);
        state.outputs.retain(Resource::is_alive);
        state.xdg_outputs.retain(|(x, o)| x.is_alive() && o.is_alive());
        let interval_us = if state.remote.is_some() { 7_800 } else { 100_000 };
        if last_frame.elapsed().as_micros() >= interval_us {
            last_frame = Instant::now();
            state.remote_render();
            let now = state.time();
            for callback in state.callbacks.drain(..) {
                callback.done(now);
            }
        }
        display.flush_clients()?;
    }
}
