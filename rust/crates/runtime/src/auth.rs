use crate::validate::parse_allowed_uids;
use std::fs::{self, OpenOptions};
use std::io::{self, Read};
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd, RawFd};
use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};

pub struct PeerPolicy {
    pid_namespace: Option<PathBuf>,
    mount_namespace: PathBuf,
}

impl PeerPolicy {
    pub fn new() -> io::Result<Self> {
        {
            let (socket, _peer) = UnixStream::pair()?;
            peer_pidfd(socket.as_raw_fd()).map_err(|e| {
                io::Error::new(e.kind(), format!("SO_PEERPIDFD check failed: {e}"))
            })?;
        }
        let pid_namespace = match fs::read_link("/proc/self/ns/pid") {
            Ok(namespace) => Some(namespace),
            Err(e) if e.kind() == io::ErrorKind::NotFound => {
                match fs::read_link("/proc/1/ns/pid") {
                    Err(e) if e.kind() == io::ErrorKind::NotFound => None,
                    Err(e) => return Err(e),
                    Ok(_) => return Err(io::Error::other("daemon PID namespace unavailable")),
                }
            }
            Err(e) => return Err(e),
        };
        Ok(Self {
            pid_namespace,
            mount_namespace: fs::read_link("/proc/self/ns/mnt")?,
        })
    }

    pub fn authorized_uid(&self, fd: RawFd, allowed_uids_path: &Path) -> Option<u32> {
        let cred = peer_credentials(fd)?;
        if cred.pid <= 0 {
            return None;
        }
        let peer = peer_pidfd(fd).ok()?;
        if !process_alive(&peer) {
            return None;
        }

        let proc_ns = PathBuf::from(format!("/proc/{}/ns", cred.pid));
        match (&self.pid_namespace, fs::read_link(proc_ns.join("pid"))) {
            (Some(host), Ok(namespace)) if *host == namespace => {}
            (None, Err(e)) if e.kind() == io::ErrorKind::NotFound => {}
            _ => return None,
        }
        let mount_namespace = fs::read_link(proc_ns.join("mnt")).ok()?;

        let allowed = if cred.uid == 0 {
            mount_namespace == self.mount_namespace
        } else {
            android_app_peer(fd) && uid_allowed(cred.uid, allowed_uids_path)
        };
        (allowed && process_alive(&peer)).then_some(cred.uid)
    }
}

fn peer_credentials(fd: RawFd) -> Option<libc::ucred> {
    let mut cred = libc::ucred {
        pid: 0,
        uid: u32::MAX,
        gid: u32::MAX,
    };
    let mut len = size_of::<libc::ucred>() as libc::socklen_t;
    let rc = unsafe {
        libc::getsockopt(
            fd,
            libc::SOL_SOCKET,
            libc::SO_PEERCRED,
            (&mut cred as *mut libc::ucred).cast(),
            &mut len,
        )
    };
    (rc == 0 && len as usize == size_of::<libc::ucred>()).then_some(cred)
}

fn peer_pidfd(fd: RawFd) -> io::Result<OwnedFd> {
    let mut peer: RawFd = -1;
    let mut len = size_of::<RawFd>() as libc::socklen_t;
    let rc = unsafe {
        libc::getsockopt(
            fd,
            libc::SOL_SOCKET,
            libc::SO_PEERPIDFD,
            (&mut peer as *mut RawFd).cast(),
            &mut len,
        )
    };
    if rc != 0 {
        return Err(io::Error::last_os_error());
    }
    if peer < 0 {
        return Err(io::Error::new(io::ErrorKind::InvalidData, "invalid peer pidfd"));
    }
    let owned = unsafe { OwnedFd::from_raw_fd(peer) };
    if len as usize != size_of::<RawFd>() {
        return Err(io::Error::new(io::ErrorKind::InvalidData, "invalid peer pidfd size"));
    }
    Ok(owned)
}

fn process_alive(peer: &OwnedFd) -> bool {
    let mut pfd = libc::pollfd {
        fd: peer.as_raw_fd(),
        events: libc::POLLIN,
        revents: 0,
    };
    (unsafe { libc::poll(&mut pfd, 1, 0) }) == 0
}

fn android_app_peer(fd: RawFd) -> bool {
    let mut context = [0u8; 1024];
    let mut len = context.len() as libc::socklen_t;
    let rc = unsafe {
        libc::getsockopt(
            fd,
            libc::SOL_SOCKET,
            libc::SO_PEERSEC,
            context.as_mut_ptr().cast(),
            &mut len,
        )
    };
    if rc != 0 || len as usize > context.len() {
        return false;
    }
    let Ok(label) = std::str::from_utf8(&context[..len as usize]) else {
        return false;
    };
    let Some(domain) = label.trim_end_matches('\0').split(':').nth(2) else {
        return false;
    };
    domain == "untrusted_app"
        || domain
            .strip_prefix("untrusted_app_")
            .is_some_and(|n| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()))
}

fn uid_allowed(uid: u32, path: &Path) -> bool {
    let Ok(file) = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(path)
    else {
        return false;
    };
    let Ok(meta) = file.metadata() else {
        return false;
    };
    if !meta.is_file() || meta.uid() != 0 || meta.mode() & 0o022 != 0 {
        return false;
    }
    let mut text = String::new();
    file.take(8193).read_to_string(&mut text).is_ok()
        && text.len() <= 8192
        && parse_allowed_uids(&text).contains(&uid)
}
