use ffmpeg_next as ffmpeg;
use ffmpeg::ffi;
use std::{
    collections::HashMap,
    io::{self, BufRead, BufReader, Read, Write},
    net::TcpStream,
    process::{Child, Command, Stdio},
    sync::{Arc, Mutex},
};

pub const VERSION: u32 = 1;

const OPEN: u32 = 0x10;
const CLOSE: u32 = 0x11;
const TITLE: u32 = 0x12;
const SIZE: u32 = 0x13;
const MOVE: u32 = 0x14;
const RESIZE: u32 = 0x15;
const VIDEO: u32 = 0x20;
const PIXELS: u32 = 0x21;

pub const HELLO: u32 = 1;
pub const CONFIGURE: u32 = 2;
pub const CLOSE_WINDOW: u32 = 3;
pub const FOCUS: u32 = 4;
pub const MOTION: u32 = 5;
pub const BUTTON: u32 = 6;
pub const AXIS: u32 = 7;
pub const KEY: u32 = 8;
pub const LEAVE: u32 = 9;
pub const KEYFRAME: u32 = 10;

pub struct Frame(*mut ffi::AVFrame);

unsafe impl Send for Frame {}

impl Drop for Frame {
    fn drop(&mut self) {
        unsafe { ffi::av_frame_free(&mut self.0) }
    }
}

impl Frame {
    #[cfg(target_os = "linux")]
    pub unsafe fn from_raw(frame: *mut ffi::AVFrame) -> Self {
        Self(frame)
    }

    #[cfg(not(target_os = "macos"))]
    pub fn raw(&self) -> *mut ffi::AVFrame {
        self.0
    }

    pub fn size(&self) -> (u32, u32) {
        unsafe { ((*self.0).width as u32, (*self.0).height as u32) }
    }

    #[cfg(not(target_os = "macos"))]
    pub fn format(&self) -> i32 {
        unsafe { (*self.0).format }
    }

    #[cfg(not(target_os = "macos"))]
    pub fn plane(&self, index: usize, rows: usize) -> (&[u8], usize) {
        unsafe {
            let stride = (*self.0).linesize[index] as usize;
            (std::slice::from_raw_parts((*self.0).data[index], stride * rows), stride)
        }
    }

    #[cfg(target_os = "macos")]
    pub fn pixel_buffer(&self) -> Option<*const std::ffi::c_void> {
        unsafe {
            ((*self.0).format == ffi::AVPixelFormat::AV_PIX_FMT_VIDEOTOOLBOX as i32 && !(*self.0).data[3].is_null())
                .then(|| (*self.0).data[3] as *const std::ffi::c_void)
        }
    }
}

pub enum Event {
    Open(u32),
    Close(u32),
    Title(u32, String),
    Size(u32, u32, u32),
    Move(u32),
    Resize(u32, u32),
    Video { id: u32, frame: Frame, crop: (u32, u32), matrix: [f32; 6] },
    Pixels { id: u32, width: u32, height: u32, y0: u32, rows: u32, data: Vec<u8> },
    Disconnected(String),
}

#[derive(Clone)]
pub struct Sender(Arc<Mutex<TcpStream>>);

impl Sender {
    pub fn send(&self, kind: u32, words: &[u32]) {
        let mut msg = Vec::with_capacity(8 + words.len() * 4);
        msg.extend_from_slice(&kind.to_le_bytes());
        msg.extend_from_slice(&(words.len() as u32 * 4).to_le_bytes());
        for word in words {
            msg.extend_from_slice(&word.to_le_bytes());
        }
        let _ = self.0.lock().unwrap().write_all(&msg);
    }
}

pub struct Connection {
    pub sender: Sender,
    pub reader: TcpStream,
    child: Child,
    port: u16,
}

impl Drop for Connection {
    fn drop(&mut self) {
        let _ = adb().args(["forward", "--remove", &format!("tcp:{}", self.port)]).status();
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

pub fn connect(distro: &str) -> io::Result<Connection> {
    let mut child = adb()
        .args(["shell", "-T", "su", "-c"])
        .arg(format!("'/data/adb/modules/bareboot/bin/bareboot display {distro}'"))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()?;
    let mut line = String::new();
    BufReader::new(child.stdout.take().unwrap()).read_line(&mut line)?;
    let Some(remote) = line.trim().strip_prefix("PORT=").and_then(|p| p.parse::<u16>().ok()) else {
        let _ = child.kill();
        return Err(io::Error::other(format!("unexpected reply from the phone: {}", line.trim())));
    };
    let out = adb().args(["forward", "tcp:0", &format!("tcp:{remote}")]).output()?;
    let port = String::from_utf8_lossy(&out.stdout).trim().parse::<u16>().ok().filter(|_| out.status.success());
    let Some(port) = port else {
        let _ = child.kill();
        return Err(io::Error::other(format!("adb forward failed: {}", String::from_utf8_lossy(&out.stderr).trim())));
    };
    let stream = match TcpStream::connect(("127.0.0.1", port)) {
        Ok(stream) => stream,
        Err(e) => {
            let _ = adb().args(["forward", "--remove", &format!("tcp:{port}")]).status();
            let _ = child.kill();
            return Err(e);
        }
    };
    stream.set_nodelay(true)?;
    Ok(Connection {
        sender: Sender(Arc::new(Mutex::new(stream.try_clone()?))),
        reader: stream,
        child,
        port,
    })
}

struct Decoder {
    decoder: ffmpeg::decoder::Video,
    failed: bool,
}

fn hw_device() -> Option<*mut ffi::AVBufferRef> {
    #[cfg(target_os = "macos")]
    let kind = ffi::AVHWDeviceType::AV_HWDEVICE_TYPE_VIDEOTOOLBOX;
    #[cfg(target_os = "windows")]
    let kind = ffi::AVHWDeviceType::AV_HWDEVICE_TYPE_D3D11VA;
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    let kind = ffi::AVHWDeviceType::AV_HWDEVICE_TYPE_VAAPI;
    let mut device = std::ptr::null_mut();
    let ret = unsafe { ffi::av_hwdevice_ctx_create(&mut device, kind, std::ptr::null(), std::ptr::null_mut(), 0) };
    (ret >= 0 && !device.is_null()).then_some(device)
}

impl Decoder {
    fn new(device: Option<*mut ffi::AVBufferRef>) -> Option<Self> {
        let codec = ffmpeg::decoder::find(ffmpeg::codec::Id::H264)?;
        let mut context = ffmpeg::codec::Context::new_with_codec(codec);
        unsafe {
            let raw = context.as_mut_ptr();
            (*raw).flags |= ffi::AV_CODEC_FLAG_LOW_DELAY as i32;
            (*raw).thread_count = 1;
            if let Some(device) = device {
                (*raw).hw_device_ctx = ffi::av_buffer_ref(device);
            }
        }
        let decoder = context.decoder().video().ok()?;
        Some(Self { decoder, failed: false })
    }

    fn decode(&mut self, data: &[u8], mut out: impl FnMut(Frame, [f32; 6])) -> bool {
        let packet = ffmpeg::Packet::copy(data);
        if self.decoder.send_packet(&packet).is_err() {
            return false;
        }
        let mut frame = ffmpeg::frame::Video::empty();
        while self.decoder.receive_frame(&mut frame).is_ok() {
            let matrix = matrix(&frame);
            unsafe {
                let hw = !(*frame.as_ptr()).hw_frames_ctx.is_null();
                let keep = !hw || cfg!(any(target_os = "macos", target_os = "linux"));
                let owned = if keep {
                    ffi::av_frame_clone(frame.as_ptr())
                } else {
                    let soft = ffi::av_frame_alloc();
                    if ffi::av_hwframe_transfer_data(soft, frame.as_ptr(), 0) < 0 {
                        ffi::av_frame_free(&mut { soft });
                        return false;
                    }
                    soft
                };
                if !owned.is_null() {
                    out(Frame(owned), matrix);
                }
            }
        }
        true
    }
}

fn matrix(frame: &ffmpeg::frame::Video) -> [f32; 6] {
    use ffmpeg::util::color::{Range, Space};
    let full = frame.color_range() == Range::JPEG;
    let bt709 = frame.color_space() == Space::BT709;
    match (bt709, full) {
        (false, false) => [255.0 / 219.0, 16.0 / 255.0, 1.596, 0.392, 0.813, 2.017],
        (true, false) => [255.0 / 219.0, 16.0 / 255.0, 1.793, 0.213, 0.533, 2.112],
        (false, true) => [1.0, 0.0, 1.402, 0.344, 0.714, 1.772],
        (true, true) => [1.0, 0.0, 1.5748, 0.1873, 0.4681, 1.8556],
    }
}

fn word(body: &[u8], index: usize) -> u32 {
    body.get(index * 4..index * 4 + 4).map_or(0, |b| u32::from_le_bytes(b.try_into().unwrap()))
}

pub fn receive(stream: TcpStream, sender: Sender, mut emit: impl FnMut(Event)) {
    let _ = ffmpeg::init();
    let device = hw_device();
    if device.is_none() {
        eprintln!("hardware video decoding is unavailable; using software decoding");
    }
    let mut decoders: HashMap<u32, Decoder> = HashMap::new();
    let mut reader = BufReader::with_capacity(1 << 20, stream);
    let result: io::Result<()> = (|| loop {
        let mut head = [0u8; 8];
        reader.read_exact(&mut head)?;
        let kind = word(&head, 0);
        let len = word(&head, 1) as usize;
        if len > 64 << 20 {
            return Err(io::Error::new(io::ErrorKind::InvalidData, "message too large"));
        }
        let mut body = vec![0u8; len];
        reader.read_exact(&mut body)?;
        let id = word(&body, 0);
        match kind {
            OPEN => emit(Event::Open(id)),
            CLOSE => {
                decoders.remove(&id);
                emit(Event::Close(id));
            }
            TITLE => emit(Event::Title(id, String::from_utf8_lossy(&body[4.min(len)..]).into_owned())),
            SIZE => emit(Event::Size(id, word(&body, 1), word(&body, 2))),
            MOVE => emit(Event::Move(id)),
            RESIZE => emit(Event::Resize(id, word(&body, 1))),
            VIDEO if len > 16 => {
                let crop = (word(&body, 2), word(&body, 3));
                let decoder = match decoders.get_mut(&id) {
                    Some(decoder) => decoder,
                    None => match Decoder::new(device) {
                        Some(decoder) => decoders.entry(id).or_insert(decoder),
                        None => return Err(io::Error::other("H.264 decoder is unavailable")),
                    },
                };
                let ok = decoder.decode(&body[16..], |frame, matrix| emit(Event::Video { id, frame, crop, matrix }));
                if !ok && !decoder.failed {
                    sender.send(KEYFRAME, &[id]);
                }
                decoder.failed = !ok;
            }
            PIXELS if len >= 20 => {
                let (width, height, y0, rows) = (word(&body, 1), word(&body, 2), word(&body, 3), word(&body, 4));
                if width == 0 || width > 16384 || y0 + rows > height {
                    continue;
                }
                let Ok(data) = lz4_flex::block::decompress_size_prepended(&body[20..]) else { continue };
                if data.len() == width as usize * rows as usize * 4 {
                    emit(Event::Pixels { id, width, height, y0, rows, data });
                }
            }
            _ => {}
        }
    })();
    let reason = match result {
        Err(e) if e.kind() == io::ErrorKind::UnexpectedEof => "the phone closed the connection".to_string(),
        Err(e) => e.to_string(),
        Ok(()) => String::new(),
    };
    emit(Event::Disconnected(reason));
}

/// adb for the test phone: `ANDROID_SERIAL` picks one when several devices are attached, else the only USB one.
fn adb() -> Command {
    let mut command = Command::new("adb");
    match std::env::var("ANDROID_SERIAL") {
        Ok(serial) if !serial.is_empty() => command.args(["-s", &serial]),
        _ => command.arg("-d"),
    };
    command
}
