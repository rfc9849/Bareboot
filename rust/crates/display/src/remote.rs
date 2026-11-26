use std::{
    collections::VecDeque,
    io::{self, Write},
    os::{fd::AsRawFd, unix::net::UnixStream},
    sync::{Arc, Condvar, Mutex},
};

pub const VERSION: u32 = 1;
const CAP_HEVC: u32 = 1;
const MAX_INPUT: usize = 64;
const MAX_BACKLOG: usize = 8 << 20;

pub const OPEN: u32 = 0x10;
pub const CLOSE: u32 = 0x11;
pub const TITLE: u32 = 0x12;
pub const SIZE: u32 = 0x13;
pub const MOVE: u32 = 0x14;
pub const RESIZE: u32 = 0x15;
pub const VIDEO: u32 = 0x20;
pub const PIXELS: u32 = 0x21;

pub enum Event {
    Hello { scale: i32, width: i32, height: i32, caps: u32 },
    Configure { id: u32, width: i32, height: i32 },
    Close { id: u32 },
    Focus { id: u32 },
    Motion { id: u32, x: f64, y: f64 },
    Button { id: u32, button: u32, pressed: bool },
    Axis { axis: u32, value: f64 },
    Key { key: u32, pressed: bool },
    Leave,
    Keyframe { id: u32 },
}

#[derive(Default)]
struct Queue {
    items: VecDeque<Vec<u8>>,
    bytes: usize,
    closed: bool,
}

pub struct Window {
    pub id: u32,
    pub size: (i32, i32),
    pub frame: Vec<u8>,
    pub frame_size: (i32, i32),
    pub dirty: bool,
}

pub struct Remote {
    socket: UnixStream,
    queue: Arc<(Mutex<Queue>, Condvar)>,
    input: Vec<u8>,
    pub scale: Option<i32>,
    pub windows: Vec<Window>,
    pub pointer: Option<wayland_server::protocol::wl_surface::WlSurface>,
}

impl Remote {
    pub fn new(socket: UnixStream) -> io::Result<Self> {
        let mut writer = socket.try_clone()?;
        writer.set_write_timeout(Some(std::time::Duration::from_secs(5)))?;
        let queue: Arc<(Mutex<Queue>, Condvar)> = Arc::default();
        let shared = queue.clone();
        std::thread::Builder::new()
            .name("display-remote".into())
            .spawn(move || {
                let (lock, ready) = &*shared;
                loop {
                    let item = {
                        let mut q = lock.lock().unwrap();
                        while q.items.is_empty() && !q.closed {
                            q = ready.wait(q).unwrap();
                        }
                        let Some(item) = q.items.pop_front() else { break };
                        q.bytes -= item.len();
                        item
                    };
                    if writer.write_all(&item).is_err() {
                        break;
                    }
                }
                let _ = writer.shutdown(std::net::Shutdown::Both);
                let mut q = lock.lock().unwrap();
                q.closed = true;
                q.items.clear();
                q.bytes = 0;
            })?;
        Ok(Self {
            socket,
            queue,
            input: Vec::new(),
            scale: None,
            windows: Vec::new(),
            pointer: None,
        })
    }

    pub fn fd(&self) -> i32 {
        self.socket.as_raw_fd()
    }

    pub fn congested(&self) -> bool {
        self.queue.0.lock().unwrap().bytes > MAX_BACKLOG
    }

    pub fn send(&self, kind: u32, parts: &[&[u8]]) {
        let len: usize = parts.iter().map(|p| p.len()).sum();
        let mut msg = Vec::with_capacity(8 + len);
        msg.extend_from_slice(&kind.to_le_bytes());
        msg.extend_from_slice(&(len as u32).to_le_bytes());
        for part in parts {
            msg.extend_from_slice(part);
        }
        let (lock, ready) = &*self.queue;
        let mut q = lock.lock().unwrap();
        if q.closed {
            return;
        }
        q.bytes += msg.len();
        q.items.push_back(msg);
        ready.notify_one();
    }

    pub fn window(&mut self, id: u32) -> Option<&mut Window> {
        self.windows.iter_mut().find(|w| w.id == id)
    }

    pub fn open(&mut self, id: u32, title: &str) {
        if self.window(id).is_some() {
            return;
        }
        self.windows.push(Window {
            id,
            size: (0, 0),
            frame: Vec::new(),
            frame_size: (0, 0),
            dirty: true,
        });
        self.send(OPEN, &[&id.to_le_bytes()]);
        self.title(id, title);
    }

    pub fn close(&mut self, id: u32) {
        let before = self.windows.len();
        self.windows.retain(|w| w.id != id);
        if self.windows.len() != before {
            self.send(CLOSE, &[&id.to_le_bytes()]);
        }
    }

    pub fn title(&self, id: u32, title: &str) {
        let text = title.as_bytes();
        let text = &text[..text.len().min(512)];
        self.send(TITLE, &[&id.to_le_bytes(), text]);
    }

    pub fn resize(&mut self, id: u32, size: (i32, i32)) {
        let Some(window) = self.window(id) else { return };
        if window.size == size {
            return;
        }
        window.size = size;
        let mut data = [0u8; 12];
        data[..4].copy_from_slice(&id.to_le_bytes());
        data[4..8].copy_from_slice(&size.0.to_le_bytes());
        data[8..].copy_from_slice(&size.1.to_le_bytes());
        self.send(SIZE, &[&data]);
    }

    pub fn pixels(&mut self, id: u32, width: i32, height: i32, frame: Vec<u8>) {
        let Some(window) = self.windows.iter_mut().find(|w| w.id == id) else { return };
        let row = width as usize * 4;
        let (y0, y1) = if window.frame_size == (width, height) && window.frame.len() == frame.len() {
            let changed = |y: usize| window.frame[y * row..(y + 1) * row] != frame[y * row..(y + 1) * row];
            let Some(first) = (0..height as usize).find(|&y| changed(y)) else { return };
            let last = (first..height as usize).rev().find(|&y| changed(y)).unwrap_or(first);
            (first, last + 1)
        } else {
            (0, height as usize)
        };
        let packed = lz4_flex::block::compress_prepend_size(&frame[y0 * row..y1 * row]);
        let mut head = [0u8; 20];
        head[..4].copy_from_slice(&id.to_le_bytes());
        head[4..8].copy_from_slice(&(width as u32).to_le_bytes());
        head[8..12].copy_from_slice(&(height as u32).to_le_bytes());
        head[12..16].copy_from_slice(&(y0 as u32).to_le_bytes());
        head[16..20].copy_from_slice(&((y1 - y0) as u32).to_le_bytes());
        window.frame = frame;
        window.frame_size = (width, height);
        self.send(PIXELS, &[&head, &packed]);
    }

    pub fn read_events(&mut self) -> io::Result<Vec<Event>> {
        let mut buf = [0u8; 4096];
        let n = unsafe {
            libc::recv(self.socket.as_raw_fd(), buf.as_mut_ptr().cast(), buf.len(), libc::MSG_DONTWAIT)
        };
        if n < 0 {
            let e = io::Error::last_os_error();
            return if matches!(e.kind(), io::ErrorKind::Interrupted | io::ErrorKind::WouldBlock) {
                Ok(Vec::new())
            } else {
                Err(e)
            };
        }
        if n == 0 {
            return Err(io::ErrorKind::UnexpectedEof.into());
        }
        self.input.extend_from_slice(&buf[..n as usize]);
        let mut events = Vec::new();
        let mut pos = 0;
        while self.input.len() - pos >= 8 {
            let kind = u32::from_le_bytes(self.input[pos..pos + 4].try_into().unwrap());
            let len = u32::from_le_bytes(self.input[pos + 4..pos + 8].try_into().unwrap()) as usize;
            if len > MAX_INPUT {
                return Err(io::ErrorKind::InvalidData.into());
            }
            if self.input.len() - pos < 8 + len {
                break;
            }
            let body = &self.input[pos + 8..pos + 8 + len];
            pos += 8 + len;
            let word = |i: usize| body.get(i * 4..i * 4 + 4).map(|b| u32::from_le_bytes(b.try_into().unwrap()));
            let float = |i: usize| word(i).map(|w| f64::from(f32::from_bits(w))).filter(|v| v.is_finite());
            let event = match kind {
                1 if word(0) == Some(VERSION) => word(1).zip(word(2)).zip(word(3)).map(|((s, w), h)| Event::Hello {
                    scale: (s as i32).clamp(1, 4),
                    width: (w as i32).clamp(320, 16384),
                    height: (h as i32).clamp(240, 16384),
                    caps: word(4).unwrap_or(0),
                }),
                1 => return Err(io::Error::new(io::ErrorKind::InvalidData, "unsupported viewer version")),
                2 => word(0).zip(word(1)).zip(word(2)).map(|((id, w), h)| Event::Configure {
                    id,
                    width: (w as i32).clamp(1, 16384),
                    height: (h as i32).clamp(1, 16384),
                }),
                3 => word(0).map(|id| Event::Close { id }),
                4 => word(0).map(|id| Event::Focus { id }),
                5 => word(0).zip(float(1)).zip(float(2)).map(|((id, x), y)| Event::Motion { id, x, y }),
                6 => word(0).zip(word(1)).zip(word(2)).map(|((id, button), p)| Event::Button { id, button, pressed: p != 0 }),
                7 => word(1).zip(float(2)).map(|(axis, value)| Event::Axis { axis, value }),
                8 => word(0).zip(word(1)).map(|(key, p)| Event::Key { key, pressed: p != 0 }),
                9 => Some(Event::Leave),
                10 => word(0).map(|id| Event::Keyframe { id }),
                _ => None,
            };
            if let Some(event) = event {
                events.push(event);
            }
        }
        self.input.drain(..pos);
        Ok(events)
    }
}

impl Drop for Remote {
    fn drop(&mut self) {
        let (lock, ready) = &*self.queue;
        lock.lock().unwrap().closed = true;
        ready.notify_one();
        let _ = self.socket.shutdown(std::net::Shutdown::Both);
    }
}

use super::{Positioner, State, direct, hardware_z_order, info, root_of};
use wayland_protocols::{
    wp::viewporter::server::{wp_viewport, wp_viewporter},
    xdg::{
        decoration::zv1::server::{zxdg_decoration_manager_v1, zxdg_toplevel_decoration_v1},
        shell::server::xdg_toplevel,
    },
};
use wayland_server::{
    Client, DataInit, Dispatch, DisplayHandle, GlobalDispatch, New, Resource,
    protocol::{wl_output, wl_pointer, wl_surface},
};

impl GlobalDispatch<wp_viewporter::WpViewporter, ()> for State {
    fn bind(
        _: &mut Self,
        _: &DisplayHandle,
        _: &Client,
        resource: New<wp_viewporter::WpViewporter>,
        _: &(),
        init: &mut DataInit<'_, Self>,
    ) {
        init.init(resource, ());
    }
}

impl Dispatch<wp_viewporter::WpViewporter, ()> for State {
    fn request(
        _: &mut Self,
        _: &Client,
        _: &wp_viewporter::WpViewporter,
        request: wp_viewporter::Request,
        _: &(),
        _: &DisplayHandle,
        init: &mut DataInit<'_, Self>,
    ) {
        if let wp_viewporter::Request::GetViewport { id, surface } = request {
            init.init(id, surface);
        }
    }
}

impl Dispatch<wp_viewport::WpViewport, wl_surface::WlSurface> for State {
    fn request(
        _: &mut Self,
        _: &Client,
        resource: &wp_viewport::WpViewport,
        request: wp_viewport::Request,
        surface: &wl_surface::WlSurface,
        _: &DisplayHandle,
        _: &mut DataInit<'_, Self>,
    ) {
        let Some(data) = surface.data::<std::sync::Mutex<super::Surface>>() else { return };
        match request {
            wp_viewport::Request::SetDestination { width: -1, height: -1 } => {
                data.lock().unwrap().pending_viewport = Some(None);
            }
            wp_viewport::Request::SetDestination { width, height } if width > 0 && height > 0 => {
                data.lock().unwrap().pending_viewport = Some(Some((width.min(super::MAX_EDGE), height.min(super::MAX_EDGE))));
            }
            wp_viewport::Request::SetDestination { .. } => {
                resource.post_error(wp_viewport::Error::BadValue, "invalid destination size");
            }
            wp_viewport::Request::SetSource { x, y, width, height } if x == -1.0 && y == -1.0 && width == -1.0 && height == -1.0 => {
                data.lock().unwrap().pending_source = Some(None);
            }
            wp_viewport::Request::SetSource { x, y, width, height } if x >= 0.0 && y >= 0.0 && width > 0.0 && height > 0.0 => {
                data.lock().unwrap().pending_source = Some(Some([x, y, width, height]));
            }
            wp_viewport::Request::SetSource { .. } => {
                resource.post_error(wp_viewport::Error::BadValue, "invalid source rectangle");
            }
            wp_viewport::Request::Destroy => {
                let mut data = data.lock().unwrap();
                data.pending_viewport = Some(None);
                data.pending_source = Some(None);
            }
            _ => {}
        }
    }
}

impl GlobalDispatch<zxdg_decoration_manager_v1::ZxdgDecorationManagerV1, ()> for State {
    fn bind(
        _: &mut Self,
        _: &DisplayHandle,
        _: &Client,
        resource: New<zxdg_decoration_manager_v1::ZxdgDecorationManagerV1>,
        _: &(),
        init: &mut DataInit<'_, Self>,
    ) {
        init.init(resource, ());
    }
}

impl Dispatch<zxdg_decoration_manager_v1::ZxdgDecorationManagerV1, ()> for State {
    fn request(
        state: &mut Self,
        _: &Client,
        _: &zxdg_decoration_manager_v1::ZxdgDecorationManagerV1,
        request: zxdg_decoration_manager_v1::Request,
        _: &(),
        _: &DisplayHandle,
        init: &mut DataInit<'_, Self>,
    ) {
        if let zxdg_decoration_manager_v1::Request::GetToplevelDecoration { id, .. } = request {
            let decoration = init.init(id, ());
            decoration.configure(state.decoration_mode());
            state.decorations.push(decoration);
        }
    }
}

impl Dispatch<zxdg_toplevel_decoration_v1::ZxdgToplevelDecorationV1, ()> for State {
    fn request(
        state: &mut Self,
        _: &Client,
        resource: &zxdg_toplevel_decoration_v1::ZxdgToplevelDecorationV1,
        request: zxdg_toplevel_decoration_v1::Request,
        _: &(),
        _: &DisplayHandle,
        _: &mut DataInit<'_, Self>,
    ) {
        match request {
            zxdg_toplevel_decoration_v1::Request::SetMode { .. } | zxdg_toplevel_decoration_v1::Request::UnsetMode => {
                resource.configure(state.decoration_mode());
            }
            _ => state.decorations.retain(|d| d != resource && d.is_alive()),
        }
    }
}

fn logical_size(surface: &wl_surface::WlSurface) -> Option<(i32, i32)> {
    let data = info(surface).lock().unwrap();
    let b = data.buffer_scale.max(1);
    let buffer = data.image.as_ref().map(|p| (p.width, p.height)).or(data.hardware)?;
    let source = data.source.map(|s| (s[2].round().max(1.0) as i32, s[3].round().max(1.0) as i32));
    Some(data.viewport.or(source).unwrap_or((buffer.0 / b, buffer.1 / b)))
}

fn source_rect(data: &super::Surface, buffer: (i32, i32)) -> [f64; 4] {
    let b = f64::from(data.buffer_scale.max(1));
    let (w, h) = (f64::from(buffer.0.max(1)), f64::from(buffer.1.max(1)));
    match data.source {
        Some([x, y, sw, sh]) => [
            (x * b / w).clamp(0.0, 1.0),
            (y * b / h).clamp(0.0, 1.0),
            ((x + sw) * b / w).clamp(0.0, 1.0),
            ((y + sh) * b / h).clamp(0.0, 1.0),
        ],
        None => [0.0, 0.0, 1.0, 1.0],
    }
}

fn bounds(surface: &wl_surface::WlSurface, origin: (i32, i32)) -> (i32, i32) {
    let data = info(surface).lock().unwrap();
    let x = origin.0 + data.position.0;
    let y = origin.1 + data.position.1;
    let children: Vec<_> = data.children.clone();
    drop(data);
    let (mut w, mut h) = logical_size(surface).map_or((0, 0), |(w, h)| (x + w, y + h));
    for child in children {
        if info(&child).lock().unwrap().popup.is_some() {
            continue;
        }
        let (cw, ch) = bounds(&child, (x, y));
        w = w.max(cw);
        h = h.max(ch);
    }
    (w, h)
}

fn has_hardware(surface: &wl_surface::WlSurface) -> bool {
    let data = info(surface).lock().unwrap();
    if data.hardware.is_some() {
        return true;
    }
    let children = data.children.clone();
    drop(data);
    children.iter().any(has_hardware)
}

fn popups(surface: &wl_surface::WlSurface, out: &mut Vec<wl_surface::WlSurface>) {
    let children = info(surface).lock().unwrap().children.clone();
    for child in children {
        if info(&child).lock().unwrap().popup.is_some() {
            out.push(child.clone());
        }
        popups(&child, out);
    }
}

fn hit(surface: &wl_surface::WlSurface, origin: (f64, f64), x: f64, y: f64) -> Option<(wl_surface::WlSurface, f64, f64)> {
    let data = info(surface).lock().unwrap();
    let ox = origin.0 + f64::from(data.position.0);
    let oy = origin.1 + f64::from(data.position.1);
    let children = data.children.clone();
    drop(data);
    let below = |s: &wl_surface::WlSurface| info(s).lock().unwrap().below;
    for child in children.iter().rev().filter(|s| !below(s)) {
        if let Some(found) = hit(child, (ox, oy), x, y) {
            return Some(found);
        }
    }
    let input = info(surface).lock().unwrap().input.clone();
    let accepts = |lx: f64, ly: f64| {
        input.as_ref().is_none_or(|rects| {
            rects.iter().any(|&(rx, ry, rw, rh)| {
                lx >= f64::from(rx) && ly >= f64::from(ry) && lx < f64::from(rx + rw) && ly < f64::from(ry + rh)
            })
        })
    };
    if let Some((w, h)) = logical_size(surface)
        && x >= ox
        && y >= oy
        && x < ox + f64::from(w)
        && y < oy + f64::from(h)
        && accepts(x - ox, y - oy)
    {
        return Some((surface.clone(), x - ox, y - oy));
    }
    for child in children.iter().rev().filter(|s| below(s)) {
        if let Some(found) = hit(child, (ox, oy), x, y) {
            return Some(found);
        }
    }
    None
}

fn compose(surface: &wl_surface::WlSurface, origin: (i32, i32), s: i32, out: &mut [u8], width: i32, height: i32) {
    let data = info(surface).lock().unwrap();
    let x = origin.0 + data.position.0 * s;
    let y = origin.1 + data.position.1 * s;
    let below = |sub: &wl_surface::WlSurface| info(sub).lock().unwrap().below;
    for child in data.children.iter().filter(|c| below(c)) {
        compose(child, (x, y), s, out, width, height);
    }
    if let Some(img) = &data.image {
        let b = data.buffer_scale.max(1);
        let source = data.source.map(|s| (s[2].round().max(1.0) as i32, s[3].round().max(1.0) as i32));
        let (lw, lh) = data.viewport.or(source).unwrap_or((img.width / b, img.height / b));
        let [u0, v0, u1, v1] = source_rect(&data, (img.width, img.height));
        let (iw, ih) = (f64::from(img.width), f64::from(img.height));
        let dw = (lw * s).max(1);
        let dh = (lh * s).max(1);
        for dy in y.max(0)..height.min(y + dh) {
            let fy = (v0 + (v1 - v0) * (f64::from(dy - y) + 0.5) / f64::from(dh)) * ih;
            let sy = (fy as i32).clamp(0, img.height - 1) as usize;
            for dx in x.max(0)..width.min(x + dw) {
                let fx = (u0 + (u1 - u0) * (f64::from(dx - x) + 0.5) / f64::from(dw)) * iw;
                let sx = (fx as i32).clamp(0, img.width - 1) as usize;
                let i = (sy * img.width as usize + sx) * 4;
                let j = (dy as usize * width as usize + dx as usize) * 4;
                let src = &img.pixels[i..i + 4];
                let dst = &mut out[j..j + 4];
                if src[3] == 255 {
                    dst.copy_from_slice(src);
                } else if src[3] != 0 {
                    let inv = u16::from(255 - src[3]);
                    for n in 0..4 {
                        dst[n] = (u16::from(src[n]) + (u16::from(dst[n]) * inv + 127) / 255).min(255) as u8;
                    }
                }
            }
        }
    }
    for child in data.children.iter().filter(|c| !below(c)) {
        compose(child, (x, y), s, out, width, height);
    }
}

impl State {
    fn decoration_mode(&self) -> zxdg_toplevel_decoration_v1::Mode {
        if self.remote.is_some() {
            zxdg_toplevel_decoration_v1::Mode::ServerSide
        } else {
            zxdg_toplevel_decoration_v1::Mode::ClientSide
        }
    }

    fn announce_decorations(&mut self) {
        self.decorations.retain(Resource::is_alive);
        let mode = self.decoration_mode();
        for decoration in &self.decorations {
            decoration.configure(mode);
        }
    }

    fn window_geometry(&self, root: &wl_surface::WlSurface) -> (i32, i32, i32, i32) {
        if let Some(g) = info(root).lock().unwrap().geometry {
            return g;
        }
        let (w, h) = bounds(root, (0, 0));
        (0, 0, w.max(1), h.max(1))
    }

    fn window_root(&self, id: u32) -> Option<wl_surface::WlSurface> {
        self.toplevels
            .iter()
            .find(|s| s.is_alive() && info(s).lock().unwrap().window_id == id)
            .cloned()
    }

    fn window_of(&self, surface: &wl_surface::WlSurface) -> Option<(wl_surface::WlSurface, (i32, i32), u32)> {
        let (root, offset) = root_of(surface);
        let id = info(&root).lock().unwrap().window_id;
        let remote = self.remote.as_ref()?;
        remote.scale?;
        remote.windows.iter().any(|w| w.id == id).then_some((root, offset, id))
    }

    pub(super) fn remote_attach(&mut self, socket: UnixStream) {
        if let Some(direct) = &mut self.direct {
            direct.reset_encoder();
        }
        self.keyboard_focus(None);
        if let Ok(remote) = Remote::new(socket) {
            self.remote = Some(remote);
            if let Some(boost) = &mut self.cpu_boost {
                boost.set_attached(true);
            }
        }
    }

    pub(super) fn remote_detach(&mut self) {
        if self.remote.is_none() {
            return;
        }
        self.keyboard_focus(None);
        self.pointer_leave();
        self.remote = None;
        if let Some(boost) = &mut self.cpu_boost {
            boost.set_attached(false);
        }
        if let Some(direct) = &mut self.direct {
            direct.reset_encoder();
        }
        self.set_scale(1, self.output_w.max(1), self.output_h.max(1));
        self.announce_decorations();
        self.output_w = 0;
        self.output_h = 0;
    }

    fn set_scale(&mut self, scale: i32, width: i32, height: i32) {
        self.scale = scale;
        self.output_w = width;
        self.output_h = height;
        self.outputs.retain(Resource::is_alive);
        for out in &self.outputs {
            out.mode(wl_output::Mode::Current | wl_output::Mode::Preferred, width * scale, height * scale, 120_000);
            if out.version() >= 2 {
                out.scale(scale);
            }
        }
        self.xdg_outputs.retain(|(x, o)| x.is_alive() && o.is_alive());
        for (xdg, _) in &self.xdg_outputs {
            xdg.logical_size(width, height);
            if xdg.version() < 3 {
                xdg.done();
            }
        }
        for out in &self.outputs {
            if out.version() >= 2 {
                out.done();
            }
        }
    }

    fn configure_window(&mut self, root: &wl_surface::WlSurface, size: (i32, i32)) {
        let serial = self.next_serial();
        let mut states = Vec::new();
        if self.focus.as_ref() == Some(root) {
            states.extend_from_slice(&(xdg_toplevel::State::Activated as u32).to_ne_bytes());
        }
        let mut data = info(root).lock().unwrap();
        if let Some(top) = data.top.clone().filter(|_| data.role.is_some()) {
            top.configure(size.0, size.1, states);
            data.configure_xdg(serial);
        }
    }

    fn remote_hello(&mut self, scale: i32, width: i32, height: i32) {
        let Some(remote) = &mut self.remote else { return };
        remote.scale = Some(scale);
        self.set_scale(scale, width, height);
        self.announce_decorations();
        let roots: Vec<_> = self.toplevels.iter().filter(|s| s.is_alive()).cloned().collect();
        for root in roots {
            let (_, _, w, h) = self.window_geometry(&root);
            let size = (w.min(width * 9 / 10), h.min(height * 9 / 10));
            self.remote_map(&root);
            self.configure_window(&root, size);
        }
    }

    pub(super) fn remote_map(&mut self, root: &wl_surface::WlSurface) {
        if self.remote.as_ref().is_none_or(|r| r.scale.is_none()) {
            return;
        }
        let (id, title) = {
            let mut data = info(root).lock().unwrap();
            if data.window_id == 0 {
                self.next_window = self.next_window.wrapping_add(1).max(1);
                data.window_id = self.next_window;
            }
            (data.window_id, data.title.clone())
        };
        let (_, _, w, h) = self.window_geometry(root);
        let remote = self.remote.as_mut().unwrap();
        remote.open(id, &title);
        remote.resize(id, (w, h));
    }

    pub(super) fn remote_unmap(&mut self, surface: &wl_surface::WlSurface) {
        let id = info(surface).lock().unwrap().window_id;
        if id != 0 {
            if let Some(remote) = &mut self.remote {
                remote.close(id);
            }
            if let Some(direct) = &mut self.direct {
                direct.close_stream(id);
            }
        }
        if self.focus.as_ref().is_some_and(|f| f == surface || !f.is_alive()) {
            self.keyboard_focus(None);
        }
        if let Some(remote) = &mut self.remote
            && remote.pointer.as_ref().is_some_and(|p| !p.is_alive() || root_of(p).0 == *surface)
        {
            remote.pointer = None;
        }
    }

    fn ensure_open(&mut self, surface: &wl_surface::WlSurface) {
        let root = root_of(surface).0;
        if info(&root).lock().unwrap().top.is_none() || !super::has_any_content(&root) {
            return;
        }
        if !self.toplevels.contains(&root) {
            self.toplevels.push(root.clone());
        }
        if self.window_of(&root).is_none() {
            self.remote_map(&root);
        }
    }

    pub(super) fn remote_dirty(&mut self, surface: &wl_surface::WlSurface) {
        self.ensure_open(surface);
        let Some((_, _, id)) = self.window_of(surface) else { return };
        if let Some(window) = self.remote.as_mut().and_then(|r| r.window(id)) {
            window.dirty = true;
        }
    }

    fn remote_layout(&self, surface: &wl_surface::WlSurface) -> Option<(u32, direct::Layout, [u32; 4])> {
        let (root, (ox, oy), id) = self.window_of(surface)?;
        let s = self.scale;
        let (gx, gy, gw, gh) = self.window_geometry(&root);
        let (layer_id, source) = {
            let data = info(surface).lock().unwrap();
            let buffer = data.hardware?;
            (data.layer_id, source_rect(&data, buffer).map(|v| (v * 65536.0).round() as u32))
        };
        let (w, h) = logical_size(surface)?;
        Some((
            id,
            direct::Layout {
                layer_id,
                dst_x: (ox - gx) * s,
                dst_y: (oy - gy) * s,
                dst_w: w * s,
                dst_h: h * s,
                desktop_w: gw * s,
                desktop_h: gh * s,
                z_order: hardware_z_order(&root, surface),
            },
            source,
        ))
    }

    pub(super) fn remote_present(
        &mut self,
        surface: &wl_surface::WlSurface,
        buffer: wayland_server::protocol::wl_buffer::WlBuffer,
        fence: Option<std::os::fd::OwnedFd>,
    ) {
        self.ensure_open(surface);
        let layout = self.remote_layout(surface);
        let Some(direct) = &mut self.direct else { return };
        let result = match layout {
            Some((id, layout, source)) => direct.present(buffer, fence, layout, direct::Target::Stream(id, source)),
            None => {
                let layout = direct::Layout {
                    layer_id: 0,
                    dst_x: 0,
                    dst_y: 0,
                    dst_w: 1,
                    dst_h: 1,
                    desktop_w: 1,
                    desktop_h: 1,
                    z_order: 1,
                    };
                direct.present(buffer, fence, layout, direct::Target::Discard)
            }
        };
        if result.is_err() {
            surface.post_error(0u32, "hardware buffer is already in use");
        }
    }

    pub(super) fn remote_relayout(&mut self, surface: &wl_surface::WlSurface) {
        if let Some((id, layout, source)) = self.remote_layout(surface)
            && let Some(direct) = &self.direct
        {
            direct.relayout_stream(id, layout, source);
        }
    }

    pub(super) fn remote_remove_layer(&mut self, surface: &wl_surface::WlSurface, layer: u32) {
        if let Some((_, _, id)) = self.window_of(surface)
            && let Some(direct) = &self.direct
        {
            direct.remove_stream_layer(id, layer);
        }
    }

    pub(super) fn remote_forget(&mut self, surface: &wl_surface::WlSurface) {
        let Some(data) = surface.data::<std::sync::Mutex<super::Surface>>() else { return };
        let (hardware, layer) = {
            let data = data.lock().unwrap();
            (data.hardware.is_some(), data.layer_id)
        };
        if hardware {
            self.remote_remove_layer(surface, layer);
        }
        if let Some(remote) = &mut self.remote
            && remote.pointer.as_ref() == Some(surface)
        {
            remote.pointer = None;
        }
    }

    pub(super) fn remote_place_popup(&self, parent: &wl_surface::WlSurface, placer: &Positioner) -> (i32, i32, i32, i32) {
        let (root, (px, py)) = root_of(parent);
        let (gx, gy, gw, gh) = self.window_geometry(&root);
        let fits = |(x, y, w, h): (i32, i32, i32, i32)| {
            (x + px >= gx && x + px + w <= gx + gw, y + py >= gy && y + py + h <= gy + gh)
        };
        let mut pos = placer.place();
        let (fit_x, fit_y) = fits(pos);
        if !fit_x {
            let flipped = placer.flipped(true).place();
            if fits(flipped).0 {
                pos.0 = flipped.0;
            }
        }
        if !fit_y {
            let flipped = placer.flipped(false).place();
            if fits(flipped).1 {
                pos.1 = flipped.1;
            }
        }
        let x = (pos.0 + px).min(gx + gw - pos.2).max(gx) - px;
        let y = (pos.1 + py).min(gy + gh - pos.3).max(gy) - py;
        (x, y, pos.2, pos.3)
    }

    pub(super) fn remote_toplevel_request(&mut self, surface: &wl_surface::WlSurface, request: xdg_toplevel::Request) {
        match request {
            xdg_toplevel::Request::SetTitle { title } => {
                let id = {
                    let mut data = info(surface).lock().unwrap();
                    data.title = title.chars().take(256).collect();
                    data.window_id
                };
                if let Some(remote) = &self.remote
                    && remote.windows.iter().any(|w| w.id == id)
                {
                    remote.title(id, &info(surface).lock().unwrap().title);
                }
            }
            xdg_toplevel::Request::Move { .. } => {
                if let Some((_, _, id)) = self.window_of(surface) {
                    self.pointer_release();
                    self.remote.as_ref().unwrap().send(MOVE, &[&id.to_le_bytes()]);
                }
            }
            xdg_toplevel::Request::Resize { edges, .. } => {
                if let Some((_, _, id)) = self.window_of(surface) {
                    let edges: u32 = edges.into();
                    self.pointer_release();
                    self.remote.as_ref().unwrap().send(RESIZE, &[&id.to_le_bytes(), &edges.to_le_bytes()]);
                }
            }
            _ => {}
        }
    }

    pub(super) fn remote_render(&mut self) {
        let Some(remote) = &self.remote else { return };
        if remote.scale.is_none() {
            return;
        }
        let ids: Vec<u32> = remote.windows.iter().map(|w| w.id).collect();
        let s = self.scale;
        for id in ids {
            let Some(root) = self.window_root(id) else { continue };
            let (gx, gy, gw, gh) = self.window_geometry(&root);
            let remote = self.remote.as_mut().unwrap();
            remote.resize(id, (gw, gh));
            let congested = remote.congested();
            let Some(window) = remote.window(id) else { continue };
            if !window.dirty || congested {
                continue;
            }
            window.dirty = false;
            if has_hardware(&root) {
                continue;
            }
            let (width, height) = ((gw * s).min(8192), (gh * s).min(8192));
            let mut frame = vec![0u8; width as usize * height as usize * 4];
            compose(&root, (-gx * s, -gy * s), s, &mut frame, width, height);
            for pixel in frame.as_chunks_mut::<4>().0 {
                pixel[3] = 255;
            }
            remote.pixels(id, width, height, frame);
        }
    }

    fn keyboard_focus(&mut self, surface: Option<wl_surface::WlSurface>) {
        if self.focus == surface {
            return;
        }
        self.release_keys();
        let serial = self.next_serial();
        if let Some(old) = self.focus.take() {
            for k in &self.keys {
                if old.is_alive() && k.id().same_client_as(&old.id()) {
                    k.leave(serial, &old);
                }
            }
            if self.remote.is_some() && old.is_alive() {
                self.configure_window(&old, (0, 0));
            }
        }
        if let Some(new) = surface {
            for k in &self.keys {
                if k.id().same_client_as(&new.id()) {
                    k.enter(serial, &new, Vec::new());
                    k.modifiers(serial, self.modifiers, 0, 0, 0);
                }
            }
            self.focus = Some(new.clone());
            if self.remote.is_some() {
                self.configure_window(&new, (0, 0));
            }
        }
        self.update_selection();
    }

    fn pointer_leave(&mut self) {
        let Some(old) = self.remote.as_mut().and_then(|r| r.pointer.take()) else { return };
        self.pressed_button = None;
        if !old.is_alive() {
            return;
        }
        let serial = self.next_serial();
        for p in &self.pointers {
            if p.id().same_client_as(&old.id()) {
                p.leave(serial, &old);
                if p.version() >= 5 {
                    p.frame();
                }
            }
        }
    }

    pub(super) fn pointer_release(&mut self) {
        if let Some(button) = self.pressed_button.take()
            && let Some(target) = self.remote.as_ref().and_then(|r| r.pointer.clone())
        {
            let serial = self.next_serial();
            let when = self.time();
            for p in &self.pointers {
                if p.id().same_client_as(&target.id()) {
                    p.button(serial, when, button, wl_pointer::ButtonState::Released);
                    if p.version() >= 5 {
                        p.frame();
                    }
                }
            }
        }
    }

    fn pointer_motion(&mut self, id: u32, x: f64, y: f64) {
        let Some(root) = self.window_root(id) else { return };
        let (gx, gy, _, _) = self.window_geometry(&root);
        let (rx, ry) = (x + f64::from(gx), y + f64::from(gy));
        let current = self.remote.as_ref().and_then(|r| r.pointer.clone()).filter(|p| p.is_alive());
        let target = match current {
            Some(p) if self.pressed_button.is_some() && root_of(&p).0 == root => {
                let (_, (ox, oy)) = root_of(&p);
                Some((p, rx - f64::from(ox), ry - f64::from(oy)))
            }
            _ => hit(&root, (0.0, 0.0), rx, ry),
        };
        let Some((surface, lx, ly)) = target else {
            self.pointer_leave();
            return;
        };
        let serial = self.next_serial();
        let when = self.time();
        let changed = self.remote.as_ref().unwrap().pointer.as_ref() != Some(&surface);
        if changed {
            self.pointer_leave();
            self.remote.as_mut().unwrap().pointer = Some(surface.clone());
        }
        for p in &self.pointers {
            if !p.id().same_client_as(&surface.id()) {
                continue;
            }
            if changed {
                p.enter(serial, &surface, lx, ly);
            } else {
                p.motion(when, lx, ly);
            }
            if p.version() >= 5 {
                p.frame();
            }
        }
    }

    fn pointer_button(&mut self, id: u32, button: u32, pressed: bool) {
        if !(272..=276).contains(&button) {
            return;
        }
        let Some(root) = self.window_root(id) else { return };
        let target = self.remote.as_ref().and_then(|r| r.pointer.clone()).filter(|p| p.is_alive());
        if pressed {
            let mut open = Vec::new();
            popups(&root, &mut open);
            let inside = target.as_ref().is_some_and(|t| {
                let mut cur = Some(t.clone());
                while let Some(s) = cur {
                    if open.contains(&s) {
                        return true;
                    }
                    cur = info(&s).lock().unwrap().parent.clone();
                }
                false
            });
            if !inside {
                for popup in open.iter().rev() {
                    if let Some(p) = info(popup).lock().unwrap().popup.clone() {
                        p.popup_done();
                    }
                }
            }
        }
        let Some(target) = target else { return };
        if pressed {
            self.pressed_button = Some(button);
        } else if self.pressed_button == Some(button) {
            self.pressed_button = None;
        }
        let serial = self.next_serial();
        let when = self.time();
        for p in &self.pointers {
            if p.id().same_client_as(&target.id()) {
                let state = if pressed { wl_pointer::ButtonState::Pressed } else { wl_pointer::ButtonState::Released };
                p.button(serial, when, button, state);
                if p.version() >= 5 {
                    p.frame();
                }
            }
        }
    }

    fn pointer_axis(&mut self, axis: u32, value: f64) {
        let Some(target) = self.remote.as_ref().and_then(|r| r.pointer.clone()).filter(|p| p.is_alive()) else {
            return;
        };
        let when = self.time();
        let axis = if axis == 0 { wl_pointer::Axis::VerticalScroll } else { wl_pointer::Axis::HorizontalScroll };
        for p in &self.pointers {
            if p.id().same_client_as(&target.id()) {
                p.axis(when, axis, value.clamp(-1000.0, 1000.0));
                if p.version() >= 5 {
                    p.frame();
                }
            }
        }
    }

    pub(super) fn remote_events(&mut self) {
        let Some(remote) = &mut self.remote else { return };
        let events = match remote.read_events() {
            Ok(events) => events,
            Err(_) => {
                self.remote_detach();
                return;
            }
        };
        for event in events {
            if self.remote.is_none() {
                return;
            }
            if matches!(
                event,
                Event::Motion { .. } | Event::Button { .. } | Event::Axis { .. } | Event::Key { .. }
            ) && let Some(boost) = &mut self.cpu_boost
            {
                boost.input();
            }
            match event {
                Event::Hello { scale, width, height, caps } => {
                    if let Some(direct) = &self.direct {
                        direct.set_hevc(caps & CAP_HEVC != 0);
                    }
                    self.remote_hello(scale, width, height);
                }
                Event::Configure { id, width, height } => {
                    if let Some(root) = self.window_root(id) {
                        self.configure_window(&root, (width.min(super::MAX_EDGE), height.min(super::MAX_EDGE)));
                    }
                }
                Event::Close { id } => {
                    if let Some(top) = self.window_root(id).and_then(|r| info(&r).lock().unwrap().top.clone()) {
                        top.close();
                    }
                }
                Event::Focus { id } => {
                    let root = self.window_root(id);
                    self.keyboard_focus(root);
                }
                Event::Motion { id, x, y } => self.pointer_motion(id, x, y),
                Event::Button { id, button, pressed } => self.pointer_button(id, button, pressed),
                Event::Axis { axis, value } => self.pointer_axis(axis, value),
                Event::Key { key, pressed } => self.key(key, pressed),
                Event::Leave => {
                    if self.pressed_button.is_none() {
                        self.pointer_leave();
                    }
                }
                Event::Keyframe { id } => {
                    if let Some(direct) = &self.direct {
                        direct.keyframe(id);
                    }
                }
            }
        }
    }
}
