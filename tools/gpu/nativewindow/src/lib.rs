use std::ffi::c_void;
use std::io;
use std::mem::{size_of, zeroed};
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd, RawFd};
use std::ptr::NonNull;

#[repr(C)]
pub struct AHardwareBuffer {
    _p: [u8; 0],
}

#[link(name = "nativewindow")]
extern "C" {
    fn AHardwareBuffer_acquire(buffer: *mut AHardwareBuffer);
    fn AHardwareBuffer_release(buffer: *mut AHardwareBuffer);
    fn AHardwareBuffer_sendHandleToUnixSocket(buffer: *const AHardwareBuffer, fd: i32) -> i32;
    fn AHardwareBuffer_recvHandleFromUnixSocket(fd: i32, out: *mut *mut AHardwareBuffer) -> i32;
}

const MAX_FDS: usize = 64;
const MAX_DATA: usize = 4096;

pub struct HardwareBuffer(NonNull<AHardwareBuffer>);

unsafe impl Send for HardwareBuffer {}
unsafe impl Sync for HardwareBuffer {}

impl HardwareBuffer {
    pub unsafe fn clone_from_raw(ptr: NonNull<AHardwareBuffer>) -> Self {
        AHardwareBuffer_acquire(ptr.as_ptr());
        HardwareBuffer(ptr)
    }

    pub unsafe fn from_raw(ptr: NonNull<AHardwareBuffer>) -> Self {
        HardwareBuffer(ptr)
    }

    pub fn into_raw(self) -> NonNull<AHardwareBuffer> {
        let ptr = self.0;
        std::mem::forget(self);
        ptr
    }
}

impl Drop for HardwareBuffer {
    fn drop(&mut self) {
        unsafe { AHardwareBuffer_release(self.0.as_ptr()) }
    }
}

pub struct AhbInfo {
    pub fds: Vec<OwnedFd>,
    pub data: Vec<u8>,
}

fn socketpair() -> io::Result<(OwnedFd, OwnedFd)> {
    let mut fds = [0 as RawFd; 2];
    let rc = unsafe {
        libc::socketpair(
            libc::AF_UNIX,
            libc::SOCK_SEQPACKET | libc::SOCK_CLOEXEC,
            0,
            fds.as_mut_ptr(),
        )
    };
    if rc != 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(unsafe { (OwnedFd::from_raw_fd(fds[0]), OwnedFd::from_raw_fd(fds[1])) })
}

fn cmsg_space() -> usize {
    unsafe { libc::CMSG_SPACE((MAX_FDS * size_of::<RawFd>()) as u32) as usize }
}

impl TryFrom<HardwareBuffer> for AhbInfo {
    type Error = io::Error;

    fn try_from(buffer: HardwareBuffer) -> io::Result<AhbInfo> {
        let (a, b) = socketpair()?;
        let rc =
            unsafe { AHardwareBuffer_sendHandleToUnixSocket(buffer.0.as_ptr(), a.as_raw_fd()) };
        if rc != 0 {
            return Err(io::Error::from_raw_os_error(-rc));
        }
        let mut data = vec![0u8; MAX_DATA];
        let mut control = vec![0usize; cmsg_space().div_ceil(size_of::<usize>())];
        let mut iov = libc::iovec {
            iov_base: data.as_mut_ptr() as *mut c_void,
            iov_len: data.len(),
        };
        let mut msg: libc::msghdr = unsafe { zeroed() };
        msg.msg_iov = &mut iov;
        msg.msg_iovlen = 1;
        msg.msg_control = control.as_mut_ptr() as *mut c_void;
        msg.msg_controllen = (control.len() * size_of::<usize>()) as _;
        let n = unsafe { libc::recvmsg(b.as_raw_fd(), &mut msg, libc::MSG_CMSG_CLOEXEC) };
        if n < 0 {
            return Err(io::Error::last_os_error());
        }
        let mut fds = Vec::new();
        unsafe {
            let mut cmsg = libc::CMSG_FIRSTHDR(&msg);
            while !cmsg.is_null() {
                if (*cmsg).cmsg_level == libc::SOL_SOCKET && (*cmsg).cmsg_type == libc::SCM_RIGHTS {
                    let count = ((*cmsg).cmsg_len as usize - libc::CMSG_LEN(0) as usize)
                        / size_of::<RawFd>();
                    let ptr = libc::CMSG_DATA(cmsg) as *const RawFd;
                    for i in 0..count {
                        fds.push(OwnedFd::from_raw_fd(std::ptr::read_unaligned(ptr.add(i))));
                    }
                }
                cmsg = libc::CMSG_NXTHDR(&msg, cmsg);
            }
        }
        if msg.msg_flags & (libc::MSG_TRUNC | libc::MSG_CTRUNC) != 0 || n == 0 {
            return Err(io::Error::from_raw_os_error(libc::EMSGSIZE));
        }
        data.truncate(n as usize);
        Ok(AhbInfo { fds, data })
    }
}

impl TryFrom<AhbInfo> for HardwareBuffer {
    type Error = io::Error;

    fn try_from(info: AhbInfo) -> io::Result<HardwareBuffer> {
        if info.fds.len() > MAX_FDS || info.data.is_empty() || info.data.len() > MAX_DATA {
            return Err(io::Error::from_raw_os_error(libc::EINVAL));
        }
        let (a, b) = socketpair()?;
        let mut data = info.data;
        let mut control = vec![0usize; cmsg_space().div_ceil(size_of::<usize>())];
        let mut iov = libc::iovec {
            iov_base: data.as_mut_ptr() as *mut c_void,
            iov_len: data.len(),
        };
        let mut msg: libc::msghdr = unsafe { zeroed() };
        msg.msg_iov = &mut iov;
        msg.msg_iovlen = 1;
        if !info.fds.is_empty() {
            let len = info.fds.len() * size_of::<RawFd>();
            msg.msg_control = control.as_mut_ptr() as *mut c_void;
            msg.msg_controllen = unsafe { libc::CMSG_SPACE(len as u32) } as _;
            unsafe {
                let cmsg = libc::CMSG_FIRSTHDR(&msg);
                (*cmsg).cmsg_level = libc::SOL_SOCKET;
                (*cmsg).cmsg_type = libc::SCM_RIGHTS;
                (*cmsg).cmsg_len = libc::CMSG_LEN(len as u32) as _;
                let ptr = libc::CMSG_DATA(cmsg) as *mut RawFd;
                for (i, fd) in info.fds.iter().enumerate() {
                    std::ptr::write_unaligned(ptr.add(i), fd.as_raw_fd());
                }
            }
        }
        let n = unsafe { libc::sendmsg(a.as_raw_fd(), &msg, libc::MSG_NOSIGNAL) };
        if n < 0 {
            return Err(io::Error::last_os_error());
        }
        if n as usize != data.len() {
            return Err(io::Error::from_raw_os_error(libc::EIO));
        }
        drop(info.fds);
        let mut out: *mut AHardwareBuffer = std::ptr::null_mut();
        let rc = unsafe { AHardwareBuffer_recvHandleFromUnixSocket(b.as_raw_fd(), &mut out) };
        if rc != 0 {
            return Err(io::Error::from_raw_os_error(-rc));
        }
        NonNull::new(out)
            .map(HardwareBuffer)
            .ok_or_else(|| io::Error::from_raw_os_error(libc::EINVAL))
    }
}
