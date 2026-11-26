use std::{
    collections::HashMap,
    ffi::CStr,
    fs::File,
    io,
    os::fd::{AsRawFd, FromRawFd, OwnedFd, RawFd},
    os::unix::fs::MetadataExt,
    sync::{
        Mutex, OnceLock,
        atomic::{AtomicU64, Ordering},
    },
};

use magma_gpu::util::{AsRawDescriptor, FromRawDescriptor, Handle, OwnedDescriptor};
use rutabaga_gfx::AhbInfo;

#[allow(dead_code)]
#[path = "bareboot_ahb_wire.rs"]
mod wire;

const TOKEN_HANDLE: u32 = 0x04000000;

type Key = (u64, u64);
static BUFFERS: OnceLock<Mutex<HashMap<Key, AhbInfo>>> = OnceLock::new();
static NEXT_REG_ID: AtomicU64 = AtomicU64::new(1);

pub struct Registration {
    key: Key,
    _file: File,
}

impl Drop for Registration {
    fn drop(&mut self) {
        if let Some(buffers) = BUFFERS.get() {
            buffers.lock().unwrap().remove(&self.key);
        }
    }
}

pub fn register(info: AhbInfo, resource: Option<(u32, u64)>) -> io::Result<(Handle, Registration)> {
    let buffers = BUFFERS.get().ok_or(io::ErrorKind::NotConnected)?;
    if info.metadata.is_empty()
        || info.metadata.len() > wire::MAX_METADATA
        || info.fds.len() > wire::MAX_FDS - 2
    {
        return Err(io::ErrorKind::InvalidData.into());
    }
    let name = CStr::from_bytes_with_nul(b"bareboot-ahb\0").unwrap();
    let fd =
        unsafe { libc::memfd_create(name.as_ptr(), libc::MFD_CLOEXEC | libc::MFD_ALLOW_SEALING) };
    if fd < 0 {
        return Err(io::Error::last_os_error());
    }
    let file = unsafe { File::from_raw_fd(fd) };
    let mut seals = libc::F_SEAL_GROW | libc::F_SEAL_SHRINK | libc::F_SEAL_SEAL;
    if let Some((resource_id, size)) = resource {
        use std::os::unix::fs::FileExt;
        file.set_len(size.max(4096))?;
        file.write_all_at(&resource_id.to_le_bytes(), 0)?;
    } else {
        seals |= libc::F_SEAL_WRITE;
    }
    if unsafe { libc::fcntl(fd, libc::F_ADD_SEALS, seals) } != 0 {
        return Err(io::Error::last_os_error());
    }
    let meta = file.metadata()?;
    let key = (meta.dev(), meta.ino());
    let guest = unsafe { libc::fcntl(fd, libc::F_DUPFD_CLOEXEC, 0) };
    if guest < 0 {
        return Err(io::Error::last_os_error());
    }
    let descriptor = unsafe { OwnedDescriptor::from_raw_descriptor(guest) };
    buffers.lock().unwrap().insert(key, info);
    Ok((
        Handle {
            os_handle: descriptor,
            handle_type: TOKEN_HANDLE,
        },
        Registration { key, _file: file },
    ))
}

fn start_media_bridge() {
    let Some(val) = std::env::var_os("BAREBOOT_MEDIA_AHB_FD") else {
        return;
    };
    let Some(fd) = val.to_str().and_then(|v| v.parse::<RawFd>().ok()).filter(|&f| f > 2) else {
        return;
    };
    let sock = unsafe { OwnedFd::from_raw_fd(fd) };
    let _ = std::thread::Builder::new()
        .name("bareboot-media-ahb".into())
        .spawn(move || {
            let mut regs: HashMap<u64, Registration> = HashMap::new();
            let mut buf = vec![0u8; 1 + wire::MAX_METADATA];
            while let Ok((n, fds)) = wire::recv(sock.as_raw_fd(), &mut buf) {
                if n == 0 {
                    break;
                }
                match buf[0] {
                    1 if n > 1 && !fds.is_empty() => {
                        let owned: Vec<OwnedDescriptor> = fds
                            .into_iter()
                            .map(|f| unsafe {
                                OwnedDescriptor::from_raw_descriptor(std::os::fd::IntoRawFd::into_raw_fd(f))
                            })
                            .collect();
                        if let Ok((handle, reg)) = register(
                            AhbInfo {
                                metadata: buf[1..n].to_vec(),
                                fds: owned,
                            },
                            None,
                        ) {
                            let id = NEXT_REG_ID.fetch_add(1, Ordering::Relaxed);
                            regs.insert(id, reg);
                            let mut resp = [0u8; 9];
                            resp[0] = 1;
                            resp[1..9].copy_from_slice(&id.to_le_bytes());
                            let raw = handle.os_handle.as_raw_descriptor();
                            let _ = wire::send(sock.as_raw_fd(), &resp, &[raw]);
                        } else {
                            let _ = wire::send(sock.as_raw_fd(), &[0], &[]);
                        }
                    }
                    2 if n == 9 => {
                        let id = u64::from_le_bytes(buf[1..9].try_into().unwrap());
                        regs.remove(&id);
                    }
                    _ => {}
                }
            }
        });
}

pub fn start() -> io::Result<()> {
    let Some(value) = std::env::var_os("BAREBOOT_AHB_FD") else {
        return Ok(());
    };
    let fd: RawFd = value
        .to_str()
        .and_then(|v| v.parse().ok())
        .filter(|fd| *fd > 2)
        .ok_or(io::ErrorKind::InvalidInput)?;
    let socket = unsafe { OwnedFd::from_raw_fd(fd) };
    let buffers = BUFFERS.get_or_init(|| Mutex::new(HashMap::new()));
    start_media_bridge();
    std::thread::Builder::new()
        .name("bareboot-ahb-export".into())
        .spawn(move || {
            let mut request = [0u8; 1];
            while let Ok((1, mut fds)) = wire::recv(socket.as_raw_fd(), &mut request) {
                let output = (|| {
                    if request[0] != 1 || fds.len() != 1 {
                        return Err(io::ErrorKind::InvalidData.into());
                    }
                    let file = File::from(fds.pop().unwrap());
                    let meta = file.metadata()?;
                    let buffers = buffers.lock().unwrap();
                    let info = buffers
                        .get(&(meta.dev(), meta.ino()))
                        .ok_or(io::ErrorKind::NotFound)?;
                    let mut packet = Vec::with_capacity(17 + info.metadata.len());
                    packet.push(1);
                    packet.extend_from_slice(&meta.dev().to_le_bytes());
                    packet.extend_from_slice(&meta.ino().to_le_bytes());
                    packet.extend_from_slice(&info.metadata);
                    let fds: Vec<_> = info
                        .fds
                        .iter()
                        .map(AsRawDescriptor::as_raw_descriptor)
                        .collect();
                    wire::send(socket.as_raw_fd(), &packet, &fds)
                })();
                if output.is_err() && wire::send(socket.as_raw_fd(), &[0], &[]).is_err() {
                    break;
                }
            }
        })?;
    Ok(())
}
