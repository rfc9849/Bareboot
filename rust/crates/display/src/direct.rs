use super::{MAX_EDGE, State};
use protocol::ahb_wire as wire;
use std::{
    fs::File,
    io,
    os::{
        fd::{AsFd, AsRawFd, FromRawFd, OwnedFd},
        unix::fs::MetadataExt,
    },
};
use wayland_protocols::wp::linux_dmabuf::zv1::server::{
    zwp_linux_buffer_params_v1, zwp_linux_dmabuf_feedback_v1, zwp_linux_dmabuf_v1,
};
use wayland_server::{
    Client, DataInit, Dispatch, DisplayHandle, GlobalDispatch, New, Resource, protocol::wl_buffer,
};

pub(super) mod generated {
    use wayland_server::{self, protocol::*};

    mod __interfaces {
        use wayland_server::protocol::__interfaces::*;
        wayland_scanner::generate_interfaces!("bareboot-ahb-v1.xml");
    }

    use self::__interfaces::*;
    wayland_scanner::generate_server_code!("bareboot-ahb-v1.xml");
}
pub(super) use generated::bareboot_ahb_v1;

const LIMIT: usize = 64;

#[repr(C)]
struct SyncFileInfo {
    name: [u8; 32],
    status: i32,
    flags: u32,
    count: u32,
    pad: u32,
    fences: u64,
}

pub(super) fn is_fence(fd: &OwnedFd) -> bool {
    let mut info: SyncFileInfo = unsafe { std::mem::zeroed() };
    const QUERY: libc::Ioctl = libc::_IOWR::<SyncFileInfo>(b'>' as u32, 4);
    (unsafe { libc::ioctl(fd.as_raw_fd(), QUERY, &mut info) }) == 0
        && info.count > 0
        && info.status >= 0
}

#[derive(Clone, Copy)]
pub(super) struct Layout {
    pub layer_id: u32,
    pub dst_x: i32,
    pub dst_y: i32,
    pub dst_w: i32,
    pub dst_h: i32,
    pub desktop_w: i32,
    pub desktop_h: i32,
    pub z_order: i32,
}

pub(super) struct Buffer {
    pub width: i32,
    pub height: i32,
    metadata: Vec<u8>,
    fds: Vec<OwnedFd>,
}

impl Buffer {
    fn invalid() -> Self {
        Self { width: 0, height: 0, metadata: Vec::new(), fds: Vec::new() }
    }
}

enum Release {
    Encoder,
    Viewer,
    Fence(OwnedFd),
}

pub(super) struct Hold {
    pub id: u32,
    pub metadata: Vec<u8>,
    pub fds: Vec<OwnedFd>,
    pub fence: Option<OwnedFd>,
}

struct Flight {
    id: u32,
    buffer: wl_buffer::WlBuffer,
    release: Release,
}

pub(super) enum Target {
    Stream(u32, [u32; 4]),
    Discard,
}

pub(super) struct Direct {
    bridge: OwnedFd,
    encoder: Option<OwnedFd>,
    streams: Vec<(u32, u32, usize, Vec<u8>)>,
    flights: Vec<Flight>,
    serial: u32,
}

fn timeout(fd: &OwnedFd, option: i32) -> io::Result<()> {
    let value = libc::timeval {
        tv_sec: 0,
        tv_usec: 100_000,
    };
    if unsafe {
        libc::setsockopt(
            fd.as_raw_fd(),
            libc::SOL_SOCKET,
            option,
            (&value as *const libc::timeval).cast(),
            std::mem::size_of_val(&value) as _,
        )
    } != 0
    {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

fn set_sockbuf(fd: &OwnedFd, bytes: libc::c_int) {
    unsafe {
        libc::setsockopt(
            fd.as_raw_fd(),
            libc::SOL_SOCKET,
            libc::SO_SNDBUF,
            (&bytes as *const libc::c_int).cast(),
            std::mem::size_of_val(&bytes) as _,
        );
        libc::setsockopt(
            fd.as_raw_fd(),
            libc::SOL_SOCKET,
            libc::SO_RCVBUF,
            (&bytes as *const libc::c_int).cast(),
            std::mem::size_of_val(&bytes) as _,
        );
    }
}

impl Direct {
    pub fn new(bridge: OwnedFd, encoder: Option<OwnedFd>) -> io::Result<Self> {
        timeout(&bridge, libc::SO_RCVTIMEO)?;
        timeout(&bridge, libc::SO_SNDTIMEO)?;
        if let Some(ref enc) = encoder {
            set_sockbuf(enc, 1024 * 1024);
            timeout(enc, libc::SO_SNDTIMEO)?;
        }
        Ok(Self {
            bridge,
            encoder,
            streams: Vec::new(),
            flights: Vec::new(),
            serial: 0,
        })
    }

    pub fn encoder_fd(&self) -> libc::c_int {
        self.encoder.as_ref().map_or(-1, AsRawFd::as_raw_fd)
    }

    fn command(&self, words: &[u32]) {
        if let Some(ref enc) = self.encoder {
            let data: Vec<u8> = words.iter().flat_map(|w| w.to_le_bytes()).collect();
            let _ = wire::send(enc.as_raw_fd(), &data, &[]);
        }
    }

    pub fn reset_encoder(&mut self) {
        self.streams.clear();
        self.command(&[2]);
    }

    pub fn close_stream(&mut self, stream: u32) {
        self.streams.retain(|s| s.0 != stream);
        self.command(&[2, stream]);
    }

    pub fn remove_stream_layer(&self, stream: u32, layer: u32) {
        self.command(&[4, stream, layer]);
    }

    pub fn relayout_stream(&self, stream: u32, layout: Layout, source: [u32; 4]) {
        self.command(&[
            5,
            stream,
            layout.layer_id,
            layout.dst_x as u32,
            layout.dst_y as u32,
            layout.dst_w as u32,
            layout.dst_h as u32,
            layout.desktop_w as u32,
            layout.desktop_h as u32,
            layout.z_order as u32,
            source[0],
            source[1],
            source[2],
            source[3],
        ]);
    }

    pub fn set_hevc(&self, hevc: bool) {
        self.command(&[9, u32::from(hevc)]);
    }

    pub fn keyframe(&self, stream: u32) {
        self.command(&[8, stream]);
    }

    fn get(&self, token: OwnedFd, width: i32, height: i32) -> io::Result<Buffer> {
        let token = File::from(token);
        let meta = token.metadata()?;
        let mut data = [0u8; 17 + wire::MAX_METADATA];
        wire::send(self.bridge.as_raw_fd(), &[1], &[token.as_raw_fd()])?;
        let (n, fds) = wire::recv(self.bridge.as_raw_fd(), &mut data)?;
        if n < 18
            || data[0] != 1
            || data[1..9] != meta.dev().to_le_bytes()
            || data[9..17] != meta.ino().to_le_bytes()
            || fds.is_empty()
            || fds.len() >= wire::MAX_FDS
        {
            return Err(io::ErrorKind::InvalidData.into());
        }
        Ok(Buffer {
            width,
            height,
            metadata: data[17..n].to_vec(),
            fds,
        })
    }

    pub fn present(
        &mut self,
        buffer: wl_buffer::WlBuffer,
        fence: Option<OwnedFd>,
        layout: Layout,
        target: Target,
    ) -> io::Result<bool> {
        if self.flights.len() >= LIMIT || self.flights.iter().any(|f| f.buffer == buffer) {
            return Err(io::ErrorKind::InvalidData.into());
        }
        self.serial = self.serial.wrapping_add(1);
        let id = self.serial;
        let kind: u32 = if fence.is_some() { 1 } else { 7 };
        let image = buffer.data::<Buffer>().ok_or(io::ErrorKind::InvalidData)?;
        if image.fds.is_empty() {
            buffer.release();
            return Ok(false);
        }
        let mut data = Vec::with_capacity(52 + image.metadata.len());
        let sent = match target {
            Target::Stream(stream, source) => {
                for word in [
                    kind,
                    id,
                    stream,
                    layout.layer_id,
                    layout.dst_x as u32,
                    layout.dst_y as u32,
                    layout.dst_w as u32,
                    layout.dst_h as u32,
                    layout.desktop_w as u32,
                    layout.desktop_h as u32,
                    layout.z_order as u32,
                    0,
                    source[0],
                    source[1],
                    source[2],
                    source[3],
                ] {
                    data.extend_from_slice(&word.to_le_bytes());
                }
                self.encoder.as_ref().map(|e| (e.as_raw_fd(), Release::Encoder))
            }
            Target::Discard => None,
        };
        let release = match sent {
            Some((fd, release)) => {
                data.extend_from_slice(&image.metadata);
                let mut fds: Vec<_> = image.fds.iter().map(AsRawFd::as_raw_fd).collect();
                if let Some(ref f) = fence {
                    fds.push(f.as_raw_fd());
                }
                if wire::send(fd, &data, &fds).is_ok() {
                    Some(release)
                } else {
                    fence.map(Release::Fence)
                }
            }
            None => fence.map(Release::Fence),
        };
        match release {
            Some(release) => {
                let shown = matches!(&release, Release::Encoder);
                self.flights.push(Flight { id, buffer, release });
                Ok(shown)
            }
            None => {
                buffer.release();
                Ok(false)
            }
        }
    }

    pub fn hold(&mut self, buffer: wl_buffer::WlBuffer, fence: Option<OwnedFd>) -> io::Result<Hold> {
        if self.flights.len() >= LIMIT || self.flights.iter().any(|f| f.buffer == buffer) {
            return Err(io::ErrorKind::InvalidData.into());
        }
        let image = buffer.data::<Buffer>().ok_or(io::ErrorKind::InvalidData)?;
        if image.fds.is_empty() {
            buffer.release();
            return Err(io::ErrorKind::InvalidData.into());
        }
        let fds = image.fds.iter().map(OwnedFd::try_clone).collect::<io::Result<Vec<_>>>()?;
        let metadata = image.metadata.clone();
        self.serial = self.serial.wrapping_add(1);
        let id = self.serial;
        self.flights.push(Flight { id, buffer, release: Release::Viewer });
        Ok(Hold { id, metadata, fds, fence })
    }

    pub fn release_flight(&mut self, id: u32) {
        if let Some(index) = self.flights.iter().position(|f| f.id == id && matches!(f.release, Release::Viewer)) {
            self.flights.swap_remove(index).buffer.release();
        }
    }

    pub fn release_viewer(&mut self) {
        self.flights.retain(|flight| {
            if matches!(flight.release, Release::Viewer) {
                flight.buffer.release();
                false
            } else {
                true
            }
        });
    }

    pub fn tick(&mut self, mut on_video: impl FnMut(u32, Vec<u8>)) {
        if let Some(ref enc) = self.encoder {
            let mut buf = vec![0u8; 64 * 1024];
            for _ in 0..256 {
                if poll(enc) == 0 {
                    break;
                }
                let Ok((n, mut fds)) = wire::recv(enc.as_raw_fd(), &mut buf) else {
                    break;
                };
                if n < 8 {
                    continue;
                }
                let word = |i: usize| u32::from_le_bytes(buf[i * 4..i * 4 + 4].try_into().unwrap());
                if word(0) == 1 && n == 8 {
                    let id = word(1);
                    if let Some(idx) = self
                        .flights
                        .iter()
                        .position(|f| f.id == id && matches!(f.release, Release::Encoder))
                    {
                        if let Some(rel_fence) = fds.pop().filter(is_fence) {
                            self.flights[idx].release = Release::Fence(rel_fence);
                        } else {
                            self.flights.swap_remove(idx).buffer.release();
                        }
                    }
                } else if word(0) == 2 && n > 20 {
                    let (stream, seq, total, pos) = (word(1), word(2), word(3) as usize, word(4) as usize);
                    let chunk = &buf[20..n];
                    if total <= 12 || total > 4 * 1024 * 1024 || pos + chunk.len() > total {
                        continue;
                    }
                    let index = match self.streams.iter().position(|s| s.0 == stream) {
                        Some(index) => index,
                        None => {
                            self.streams.push((stream, 0, 0, Vec::new()));
                            self.streams.len() - 1
                        }
                    };
                    let entry = &mut self.streams[index];
                    if pos == 0 {
                        *entry = (stream, seq, total, Vec::with_capacity(total));
                    } else if entry.1 != seq || entry.3.len() != pos {
                        entry.3.clear();
                        continue;
                    }
                    entry.3.extend_from_slice(chunk);
                    if entry.3.len() == entry.2 {
                        on_video(stream, std::mem::take(&mut entry.3));
                    }
                }
            }
        }
        self.flights.retain(|flight| match &flight.release {
            Release::Encoder | Release::Viewer => true,
            Release::Fence(fence) => {
                let events = poll(fence);
                if events & libc::POLLIN != 0 && events & libc::POLLERR == 0 {
                    flight.buffer.release();
                    false
                } else if events != 0 {
                    flight.buffer.post_error(0u32, "invalid buffer fence");
                    false
                } else {
                    true
                }
            }
        });
    }
}

fn poll(fd: &OwnedFd) -> i16 {
    let mut pfd = libc::pollfd {
        fd: fd.as_raw_fd(),
        events: libc::POLLIN,
        revents: 0,
    };
    unsafe { libc::poll(&mut pfd, 1, 0) };
    pfd.revents
}

impl GlobalDispatch<bareboot_ahb_v1::BarebootAhbV1, ()> for State {
    fn bind(
        _: &mut Self,
        _: &DisplayHandle,
        _: &Client,
        resource: New<bareboot_ahb_v1::BarebootAhbV1>,
        _: &(),
        init: &mut DataInit<'_, Self>,
    ) {
        init.init(resource, ());
    }
}

impl Dispatch<bareboot_ahb_v1::BarebootAhbV1, ()> for State {
    fn request(
        state: &mut Self,
        _: &Client,
        resource: &bareboot_ahb_v1::BarebootAhbV1,
        request: bareboot_ahb_v1::Request,
        _: &(),
        _: &DisplayHandle,
        init: &mut DataInit<'_, Self>,
    ) {
        match request {
            bareboot_ahb_v1::Request::CreateBuffer {
                id,
                token,
                width,
                height,
            } => {
                if let Some(direct) = &state.direct
                    && (1..=MAX_EDGE).contains(&width)
                    && (1..=MAX_EDGE).contains(&height)
                    && let Ok(buffer) = direct.get(token, width, height)
                {
                    init.init(id, buffer);
                } else {
                    init.post_error(
                        id,
                        bareboot_ahb_v1::Error::InvalidBuffer,
                        "invalid hardware buffer",
                    );
                }
            }
            bareboot_ahb_v1::Request::SetAcquireFence { surface, fence } => {
                if let Some(data) = surface.data::<std::sync::Mutex<super::Surface>>() {
                    let mut surface = data.lock().unwrap();
                    if surface.acquire.is_none() && is_fence(&fence) {
                        surface.acquire = Some(fence);
                        return;
                    }
                }
                resource.post_error(
                    bareboot_ahb_v1::Error::InvalidFence,
                    "duplicate or invalid acquire fence",
                );
            }
            _ => {}
        }
    }
}

impl Dispatch<wl_buffer::WlBuffer, Buffer> for State {
    fn request(
        _: &mut Self,
        _: &Client,
        _: &wl_buffer::WlBuffer,
        _: wl_buffer::Request,
        _: &Buffer,
        _: &DisplayHandle,
        _: &mut DataInit<'_, Self>,
    ) {
    }
}

const DMABUF_FORMATS: [u32; 4] = [
    u32::from_le_bytes(*b"AR24"),
    u32::from_le_bytes(*b"XR24"),
    u32::from_le_bytes(*b"AB24"),
    u32::from_le_bytes(*b"XB24"),
];
const MOD_LINEAR: u64 = 0;
const MOD_INVALID: u64 = 0x00ff_ffff_ffff_ffff;

#[derive(Default)]
pub(super) struct DmabufParams {
    plane: std::sync::Mutex<Option<(OwnedFd, u64)>>,
}

impl DmabufParams {
    fn take(&self, format: u32) -> Option<OwnedFd> {
        let (fd, modifier) = self.plane.lock().unwrap().take()?;
        let supported = DMABUF_FORMATS.contains(&format) && (modifier == MOD_LINEAR || modifier == MOD_INVALID);
        if !supported {
            eprintln!("rejected dmabuf format {format:#x} modifier {modifier:#x}");
        }
        supported.then_some(fd)
    }
}

fn dmabuf_buffer(state: &State, fd: OwnedFd, width: i32, height: i32) -> Option<Buffer> {
    let direct = state.direct.as_ref()?;
    if !(1..=MAX_EDGE).contains(&width) || !(1..=MAX_EDGE).contains(&height) {
        return None;
    }
    direct
        .get(fd, width, height)
        .map_err(|e| eprintln!("could not import a {width}x{height} dmabuf: {e}"))
        .ok()
}

impl GlobalDispatch<zwp_linux_dmabuf_v1::ZwpLinuxDmabufV1, ()> for State {
    fn bind(
        _: &mut Self,
        _: &DisplayHandle,
        _: &Client,
        resource: New<zwp_linux_dmabuf_v1::ZwpLinuxDmabufV1>,
        _: &(),
        init: &mut DataInit<'_, Self>,
    ) {
        let dmabuf = init.init(resource, ());
        if dmabuf.version() >= 4 {
            return;
        }
        for format in DMABUF_FORMATS {
            if dmabuf.version() >= 3 {
                dmabuf.modifier(format, (MOD_LINEAR >> 32) as u32, MOD_LINEAR as u32);
            } else {
                dmabuf.format(format);
            }
        }
    }
}

impl Dispatch<zwp_linux_dmabuf_v1::ZwpLinuxDmabufV1, ()> for State {
    fn request(
        _: &mut Self,
        _: &Client,
        _: &zwp_linux_dmabuf_v1::ZwpLinuxDmabufV1,
        request: zwp_linux_dmabuf_v1::Request,
        _: &(),
        _: &DisplayHandle,
        init: &mut DataInit<'_, Self>,
    ) {
        match request {
            zwp_linux_dmabuf_v1::Request::CreateParams { params_id } => {
                init.init(params_id, DmabufParams::default());
            }
            zwp_linux_dmabuf_v1::Request::GetDefaultFeedback { id }
            | zwp_linux_dmabuf_v1::Request::GetSurfaceFeedback { id, .. } => {
                let feedback = init.init(id, ());
                send_feedback(&feedback);
            }
            _ => {}
        }
    }
}

impl Dispatch<zwp_linux_dmabuf_feedback_v1::ZwpLinuxDmabufFeedbackV1, ()> for State {
    fn request(
        _: &mut Self,
        _: &Client,
        _: &zwp_linux_dmabuf_feedback_v1::ZwpLinuxDmabufFeedbackV1,
        _: zwp_linux_dmabuf_feedback_v1::Request,
        _: &(),
        _: &DisplayHandle,
        _: &mut DataInit<'_, Self>,
    ) {
    }
}

fn render_device() -> Option<u64> {
    (128..192).find_map(|minor| std::fs::metadata(format!("/dev/dri/renderD{minor}")).ok().map(|m| m.rdev()))
}

fn format_table() -> io::Result<(File, u32)> {
    let table: Vec<u8> = DMABUF_FORMATS
        .iter()
        .flat_map(|format| {
            let mut entry = [0u8; 16];
            entry[..4].copy_from_slice(&format.to_ne_bytes());
            entry[8..].copy_from_slice(&MOD_LINEAR.to_ne_bytes());
            entry
        })
        .collect();
    let fd = unsafe { libc::memfd_create(c"bareboot-dmabuf-formats".as_ptr(), libc::MFD_CLOEXEC | libc::MFD_ALLOW_SEALING) };
    if fd < 0 {
        return Err(io::Error::last_os_error());
    }
    let mut file = unsafe { File::from_raw_fd(fd) };
    io::Write::write_all(&mut file, &table)?;
    let seals = libc::F_SEAL_SEAL | libc::F_SEAL_SHRINK | libc::F_SEAL_GROW | libc::F_SEAL_WRITE;
    if unsafe { libc::fcntl(fd, libc::F_ADD_SEALS, seals) } < 0 {
        return Err(io::Error::last_os_error());
    }
    Ok((file, table.len() as u32))
}

fn send_feedback(feedback: &zwp_linux_dmabuf_feedback_v1::ZwpLinuxDmabufFeedbackV1) {
    let Ok((table, size)) = format_table() else { return };
    let device = render_device().unwrap_or(0).to_ne_bytes().to_vec();
    feedback.format_table(table.as_fd(), size);
    feedback.main_device(device.clone());
    feedback.tranche_target_device(device);
    feedback.tranche_flags(zwp_linux_dmabuf_feedback_v1::TrancheFlags::empty());
    let indices = (0..DMABUF_FORMATS.len() as u16).flat_map(u16::to_ne_bytes).collect();
    feedback.tranche_formats(indices);
    feedback.tranche_done();
    feedback.done();
}

impl Dispatch<zwp_linux_buffer_params_v1::ZwpLinuxBufferParamsV1, DmabufParams> for State {
    fn request(
        state: &mut Self,
        client: &Client,
        resource: &zwp_linux_buffer_params_v1::ZwpLinuxBufferParamsV1,
        request: zwp_linux_buffer_params_v1::Request,
        params: &DmabufParams,
        dh: &DisplayHandle,
        init: &mut DataInit<'_, Self>,
    ) {
        use zwp_linux_buffer_params_v1::{Error, Request};
        match request {
            Request::Add { fd, plane_idx, offset, modifier_hi, modifier_lo, .. } => {
                let mut plane = params.plane.lock().unwrap();
                if plane_idx != 0 || offset != 0 || plane.is_some() {
                    resource.post_error(Error::PlaneIdx, "only one plane at offset 0 is supported");
                    return;
                }
                *plane = Some((fd, (u64::from(modifier_hi) << 32) | u64::from(modifier_lo)));
            }
            Request::Create { width, height, format, .. } => {
                let buffer = params
                    .take(format)
                    .and_then(|fd| dmabuf_buffer(state, fd, width, height))
                    .and_then(|buffer| {
                        client
                            .create_resource::<wl_buffer::WlBuffer, Buffer, State>(dh, 1, buffer)
                            .ok()
                    });
                match buffer {
                    Some(buffer) => resource.created(&buffer),
                    None => resource.failed(),
                }
            }
            Request::CreateImmed { buffer_id, width, height, format, .. } => {
                match params.take(format).and_then(|fd| dmabuf_buffer(state, fd, width, height)) {
                    Some(buffer) => {
                        init.init(buffer_id, buffer);
                    }
                    None => {
                        init.init(buffer_id, Buffer::invalid());
                        resource.failed();
                    }
                }
            }
            _ => {}
        }
    }
}
