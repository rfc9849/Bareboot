use std::fs::{File, OpenOptions};
use std::io;
use std::os::fd::{AsRawFd, OwnedFd};
use std::os::unix::fs::{FileExt, MetadataExt};
use std::path::{Path, PathBuf};

const LOOP_GET_STATUS64: libc::c_ulong = 0x4C05;
const LOOP_CONFIGURE: libc::c_ulong = 0x4C0A;
const LOOP_CTL_GET_FREE: libc::c_ulong = 0x4C82;
const LO_FLAGS_AUTOCLEAR: u32 = 4;
const LO_FLAGS_DIRECT_IO: u32 = 16;
const DIRECT_IO_BLOCK_SIZE: u32 = 4096;

#[repr(C)]
struct LoopInfo64 {
    lo_device: u64,
    lo_inode: u64,
    lo_rdevice: u64,
    lo_offset: u64,
    lo_sizelimit: u64,
    lo_number: u32,
    lo_encrypt_type: u32,
    lo_encrypt_key_size: u32,
    lo_flags: u32,
    lo_file_name: [u8; 64],
    lo_crypt_name: [u8; 64],
    lo_encrypt_key: [u8; 32],
    lo_init: [u64; 2],
}

#[repr(C)]
struct LoopConfig {
    fd: u32,
    block_size: u32,
    info: LoopInfo64,
    reserved: [u64; 8],
}

pub struct AttachedLoop {
    pub device_path: PathBuf,
    _loop_fd: OwnedFd,
}

pub fn attach_image(image_path: &Path) -> io::Result<AttachedLoop> {
    let ctl = OpenOptions::new()
        .read(true)
        .write(true)
        .open("/dev/loop-control")?;
    let img = OpenOptions::new().read(true).write(true).open(image_path)?;
    let block_size = loop_block_size(&img)?;
    let mut config: LoopConfig = unsafe { std::mem::zeroed() };
    config.fd = img.as_raw_fd() as u32;
    config.block_size = block_size;
    config.info.lo_flags = LO_FLAGS_AUTOCLEAR;
    if block_size != 0 {
        config.info.lo_flags |= LO_FLAGS_DIRECT_IO;
    }
    let name = image_path.as_os_str().as_encoded_bytes();
    let name_len = name.len().min(config.info.lo_file_name.len() - 1);
    config.info.lo_file_name[..name_len].copy_from_slice(&name[..name_len]);

    for _ in 0..8 {
        let idx = unsafe { libc::ioctl(ctl.as_raw_fd(), LOOP_CTL_GET_FREE as _) };
        if idx < 0 {
            return Err(io::Error::last_os_error());
        }

        let dev_path = wait_for_loop_path(idx)?;
        let loop_file = OpenOptions::new().read(true).write(true).open(&dev_path)?;

        let rc = unsafe { libc::ioctl(loop_file.as_raw_fd(), LOOP_CONFIGURE as _, &config) };
        if rc < 0 {
            let err = io::Error::last_os_error();
            if err.raw_os_error() == Some(libc::EBUSY) {
                continue;
            }
            return Err(err);
        }

        return Ok(AttachedLoop {
            device_path: dev_path,
            _loop_fd: loop_file.into(),
        });
    }

    Err(io::Error::new(
        io::ErrorKind::ResourceBusy,
        "failed to claim free loop device",
    ))
}

pub fn image_attached(image_path: &Path) -> io::Result<bool> {
    let meta = std::fs::metadata(image_path)?;
    for entry in std::fs::read_dir("/sys/block")?.flatten() {
        let name = entry.file_name();
        let Some(idx) = name
            .to_str()
            .and_then(|n| n.strip_prefix("loop"))
            .and_then(|n| n.parse::<i32>().ok())
        else {
            continue;
        };
        let Ok(dev) = resolve_loop_path(idx).and_then(File::open) else {
            continue;
        };
        let mut info: LoopInfo64 = unsafe { std::mem::zeroed() };
        if unsafe { libc::ioctl(dev.as_raw_fd(), LOOP_GET_STATUS64 as _, &mut info) } == 0
            && info.lo_device == meta.dev()
            && info.lo_inode == meta.ino()
        {
            return Ok(true);
        }
    }
    Ok(false)
}

fn loop_block_size(image: &File) -> io::Result<u32> {
    let mut superblock = [0u8; 0x3a];
    if image.read_at(&mut superblock, 1024)? != superblock.len()
        || superblock[0x38..0x3a] != [0x53, 0xef]
        || !image.metadata()?.len().is_multiple_of(u64::from(DIRECT_IO_BLOCK_SIZE))
    {
        return Ok(0);
    }
    let log_block_size = u32::from_le_bytes(superblock[0x18..0x1c].try_into().unwrap());
    Ok(if (2..=6).contains(&log_block_size) {
        DIRECT_IO_BLOCK_SIZE
    } else {
        0
    })
}

fn wait_for_loop_path(idx: i32) -> io::Result<PathBuf> {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
    loop {
        match resolve_loop_path(idx) {
            Err(e) if e.kind() == io::ErrorKind::NotFound && std::time::Instant::now() < deadline => {
                std::thread::sleep(std::time::Duration::from_millis(20));
            }
            result => return result,
        }
    }
}

fn resolve_loop_path(idx: i32) -> io::Result<PathBuf> {
    let android_path = PathBuf::from(format!("/dev/block/loop{idx}"));
    if android_path.exists() {
        return Ok(android_path);
    }
    let linux_path = PathBuf::from(format!("/dev/loop{idx}"));
    if linux_path.exists() {
        return Ok(linux_path);
    }
    Err(io::Error::new(
        io::ErrorKind::NotFound,
        format!("loop device node for index {idx} not found"),
    ))
}
