use std::{
    io,
    mem::{size_of, size_of_val},
    os::fd::{FromRawFd, OwnedFd, RawFd},
};

pub const MAX_FDS: usize = 16;
pub const MAX_METADATA: usize = 4096;

pub fn pair() -> io::Result<[OwnedFd; 2]> {
    let mut fds = [-1; 2];
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
    Ok(fds.map(|fd| unsafe { OwnedFd::from_raw_fd(fd) }))
}

const CONTROL_WORDS: usize = (unsafe { libc::CMSG_SPACE((MAX_FDS * size_of::<RawFd>()) as u32) }
    as usize)
    .div_ceil(size_of::<usize>());

pub fn send(fd: RawFd, data: &[u8], fds: &[RawFd]) -> io::Result<()> {
    if data.is_empty() || fds.len() > MAX_FDS {
        return Err(io::ErrorKind::InvalidInput.into());
    }
    let mut control = [0usize; CONTROL_WORDS];
    let mut iov = libc::iovec {
        iov_base: data.as_ptr().cast_mut().cast(),
        iov_len: data.len(),
    };
    let mut msg: libc::msghdr = unsafe { std::mem::zeroed() };
    msg.msg_iov = &mut iov;
    msg.msg_iovlen = 1;
    if !fds.is_empty() {
        let len = size_of_val(fds) as u32;
        msg.msg_control = control.as_mut_ptr().cast();
        msg.msg_controllen = unsafe { libc::CMSG_SPACE(len) } as _;
        let cmsg = unsafe { libc::CMSG_FIRSTHDR(&msg) };
        unsafe {
            (*cmsg).cmsg_level = libc::SOL_SOCKET;
            (*cmsg).cmsg_type = libc::SCM_RIGHTS;
            (*cmsg).cmsg_len = libc::CMSG_LEN(len) as _;
            std::ptr::copy_nonoverlapping(
                fds.as_ptr().cast::<u8>(),
                libc::CMSG_DATA(cmsg),
                len as usize,
            );
        }
    }
    let n = unsafe { libc::sendmsg(fd, &msg, libc::MSG_NOSIGNAL) };
    if n < 0 {
        return Err(io::Error::last_os_error());
    }
    if n as usize != data.len() {
        return Err(io::ErrorKind::WriteZero.into());
    }
    Ok(())
}

pub fn recv(fd: RawFd, data: &mut [u8]) -> io::Result<(usize, Vec<OwnedFd>)> {
    let mut control = [0usize; CONTROL_WORDS];
    let mut iov = libc::iovec {
        iov_base: data.as_mut_ptr().cast(),
        iov_len: data.len(),
    };
    let mut msg: libc::msghdr = unsafe { std::mem::zeroed() };
    msg.msg_iov = &mut iov;
    msg.msg_iovlen = 1;
    msg.msg_control = control.as_mut_ptr().cast();
    msg.msg_controllen = size_of_val(&control) as _;
    let n = unsafe { libc::recvmsg(fd, &mut msg, libc::MSG_CMSG_CLOEXEC) };
    if n < 0 {
        return Err(io::Error::last_os_error());
    }
    let mut out = Vec::new();
    unsafe {
        let mut cmsg = libc::CMSG_FIRSTHDR(&msg);
        while !cmsg.is_null() {
            if (*cmsg).cmsg_level == libc::SOL_SOCKET && (*cmsg).cmsg_type == libc::SCM_RIGHTS {
                let bytes = (*cmsg).cmsg_len as usize - libc::CMSG_LEN(0) as usize;
                let ptr = libc::CMSG_DATA(cmsg).cast::<RawFd>();
                for i in 0..bytes / size_of::<RawFd>() {
                    out.push(OwnedFd::from_raw_fd(ptr.add(i).read_unaligned()));
                }
            }
            cmsg = libc::CMSG_NXTHDR(&msg, cmsg);
        }
    }
    if msg.msg_flags & (libc::MSG_TRUNC | libc::MSG_CTRUNC) != 0 {
        return Err(io::ErrorKind::InvalidData.into());
    }
    Ok((n as usize, out))
}
