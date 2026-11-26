use crate::MAX_FRAME_SIZE;
use std::io;
use std::mem::{size_of, zeroed};
use std::os::fd::{FromRawFd, OwnedFd, RawFd};

pub fn send_frame(sock_fd: RawFd, payload: &[u8], fd: Option<RawFd>) -> io::Result<()> {
    if payload.len() > MAX_FRAME_SIZE {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "payload exceeds max frame size",
        ));
    }
    let header = (payload.len() as u32).to_le_bytes();
    let mut iov = [
        libc::iovec {
            iov_base: header.as_ptr() as *mut libc::c_void,
            iov_len: header.len(),
        },
        libc::iovec {
            iov_base: payload.as_ptr() as *mut libc::c_void,
            iov_len: payload.len(),
        },
    ];
    let iov_len = if payload.is_empty() { 1 } else { 2 };

    let mut cmsg_buf = [0u8; 64];
    let mut msg: libc::msghdr = unsafe { zeroed() };
    msg.msg_iov = iov.as_mut_ptr();
    msg.msg_iovlen = iov_len as _;

    if let Some(raw_fd) = fd {
        let cmsg_space = unsafe { libc::CMSG_SPACE(size_of::<RawFd>() as u32) } as usize;
        assert!(cmsg_space <= cmsg_buf.len());
        msg.msg_control = cmsg_buf.as_mut_ptr() as *mut libc::c_void;
        msg.msg_controllen = cmsg_space as _;
        unsafe {
            let cmsg = libc::CMSG_FIRSTHDR(&msg);
            (*cmsg).cmsg_level = libc::SOL_SOCKET;
            (*cmsg).cmsg_type = libc::SCM_RIGHTS;
            (*cmsg).cmsg_len = libc::CMSG_LEN(size_of::<RawFd>() as u32) as _;
            std::ptr::copy_nonoverlapping(
                &raw_fd as *const RawFd as *const u8,
                libc::CMSG_DATA(cmsg),
                size_of::<RawFd>(),
            );
        }
    }

    let mut sent_total = 0usize;
    let total_len = header.len() + payload.len();
    loop {
        let n = unsafe { libc::sendmsg(sock_fd, &msg, SEND_FLAGS) };
        if n < 0 {
            let err = io::Error::last_os_error();
            if err.kind() == io::ErrorKind::Interrupted {
                continue;
            }
            return Err(err);
        }
        sent_total += n as usize;
        if sent_total >= total_len {
            return Ok(());
        }
        let (head_rem, remaining) = if sent_total < header.len() {
            (&header[sent_total..], payload)
        } else {
            (&[][..], &payload[sent_total - header.len()..])
        };
        msg.msg_control = std::ptr::null_mut();
        msg.msg_controllen = 0;
        if !head_rem.is_empty() {
            iov[0] = libc::iovec {
                iov_base: head_rem.as_ptr() as *mut libc::c_void,
                iov_len: head_rem.len(),
            };
            iov[1] = libc::iovec {
                iov_base: remaining.as_ptr() as *mut libc::c_void,
                iov_len: remaining.len(),
            };
            msg.msg_iov = iov.as_mut_ptr();
            msg.msg_iovlen = if remaining.is_empty() { 1 } else { 2 } as _;
        } else {
            iov[0] = libc::iovec {
                iov_base: remaining.as_ptr() as *mut libc::c_void,
                iov_len: remaining.len(),
            };
            msg.msg_iov = iov.as_mut_ptr();
            msg.msg_iovlen = 1 as _;
        }
    }
}

#[cfg(target_os = "linux")]
const SEND_FLAGS: libc::c_int = libc::MSG_NOSIGNAL;
#[cfg(not(target_os = "linux"))]
const SEND_FLAGS: libc::c_int = 0;

#[cfg(target_os = "linux")]
const RECV_FLAGS: libc::c_int = libc::MSG_CMSG_CLOEXEC;
#[cfg(not(target_os = "linux"))]
const RECV_FLAGS: libc::c_int = 0;

pub fn recv_frame(sock_fd: RawFd) -> io::Result<(Vec<u8>, Option<OwnedFd>)> {
    let mut header = [0u8; 4];
    let mut got = 0usize;
    let mut received_fd: Option<OwnedFd> = None;

    while got < 4 {
        let mut iov = libc::iovec {
            iov_base: header[got..].as_mut_ptr() as *mut libc::c_void,
            iov_len: 4 - got,
        };
        let mut cmsg_buf = [0u8; 64];
        let mut msg: libc::msghdr = unsafe { zeroed() };
        msg.msg_iov = &mut iov;
        msg.msg_iovlen = 1;
        msg.msg_control = cmsg_buf.as_mut_ptr() as *mut libc::c_void;
        msg.msg_controllen = cmsg_buf.len() as _;

        let n = unsafe { libc::recvmsg(sock_fd, &mut msg, RECV_FLAGS) };
        if n < 0 {
            let err = io::Error::last_os_error();
            if err.kind() == io::ErrorKind::Interrupted {
                continue;
            }
            return Err(err);
        }
        if n == 0 {
            return Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "peer closed connection",
            ));
        }
        if received_fd.is_none() {
            received_fd = extract_fd(&msg);
        }
        got += n as usize;
    }

    let len = u32::from_le_bytes(header) as usize;
    if len > MAX_FRAME_SIZE {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "incoming frame exceeds max size",
        ));
    }

    let mut payload = vec![0u8; len];
    let mut read_pos = 0usize;
    while read_pos < len {
        let mut iov = libc::iovec {
            iov_base: payload[read_pos..].as_mut_ptr() as *mut libc::c_void,
            iov_len: len - read_pos,
        };
        let mut cmsg_buf = [0u8; 64];
        let mut msg: libc::msghdr = unsafe { zeroed() };
        msg.msg_iov = &mut iov;
        msg.msg_iovlen = 1;
        if received_fd.is_none() {
            msg.msg_control = cmsg_buf.as_mut_ptr() as *mut libc::c_void;
            msg.msg_controllen = cmsg_buf.len() as _;
        }
        let n = unsafe { libc::recvmsg(sock_fd, &mut msg, RECV_FLAGS) };
        if n < 0 {
            let err = io::Error::last_os_error();
            if err.kind() == io::ErrorKind::Interrupted {
                continue;
            }
            return Err(err);
        }
        if n == 0 {
            return Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "peer closed mid-frame",
            ));
        }
        if received_fd.is_none() {
            received_fd = extract_fd(&msg);
        }
        read_pos += n as usize;
    }

    Ok((payload, received_fd))
}

fn extract_fd(msg: &libc::msghdr) -> Option<OwnedFd> {
    unsafe {
        let mut cmsg = libc::CMSG_FIRSTHDR(msg);
        while !cmsg.is_null() {
            if (*cmsg).cmsg_level == libc::SOL_SOCKET && (*cmsg).cmsg_type == libc::SCM_RIGHTS {
                let mut fd: RawFd = -1;
                std::ptr::copy_nonoverlapping(
                    libc::CMSG_DATA(cmsg),
                    &mut fd as *mut RawFd as *mut u8,
                    size_of::<RawFd>(),
                );
                if fd >= 0 {
                    #[cfg(not(target_os = "linux"))]
                    {
                        let flags = libc::fcntl(fd, libc::F_GETFD);
                        if flags >= 0 {
                            libc::fcntl(fd, libc::F_SETFD, flags | libc::FD_CLOEXEC);
                        }
                    }
                    return Some(OwnedFd::from_raw_fd(fd));
                }
            }
            cmsg = libc::CMSG_NXTHDR(msg, cmsg);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};
    use std::os::fd::AsRawFd;
    use std::os::unix::net::UnixStream;

    #[test]
    fn passes_payload_and_fd_over_socketpair() {
        let (a, b) = UnixStream::pair().unwrap();
        let (pipe_r, mut pipe_w) = UnixStream::pair().unwrap();
        pipe_w.write_all(b"hello-fd").unwrap();

        send_frame(a.as_raw_fd(), b"control-frame", Some(pipe_r.as_raw_fd())).unwrap();
        drop(pipe_r);

        let (payload, fd) = recv_frame(b.as_raw_fd()).unwrap();
        assert_eq!(payload, b"control-frame");
        let passed_fd = fd.expect("expected passed fd");
        let mut passed_stream = UnixStream::from(passed_fd);
        let mut buf = [0u8; 8];
        passed_stream.read_exact(&mut buf).unwrap();
        assert_eq!(&buf, b"hello-fd");
    }
}
