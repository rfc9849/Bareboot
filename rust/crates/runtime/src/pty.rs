use protocol::WinSize;
use std::ffi::CString;
use std::io;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd, RawFd};

pub struct GuestPty {
    pub master: OwnedFd,
    pub slave: OwnedFd,
}

pub fn open_guest_pty(size: WinSize) -> io::Result<GuestPty> {
    let ptmx_path = CString::new("/dev/pts/ptmx").unwrap();
    let master_raw = unsafe {
        libc::open(
            ptmx_path.as_ptr(),
            libc::O_RDWR | libc::O_NOCTTY | libc::O_CLOEXEC,
        )
    };
    if master_raw < 0 {
        return Err(io::Error::last_os_error());
    }
    let master = unsafe { OwnedFd::from_raw_fd(master_raw) };

    let unlock: libc::c_int = 0;
    if unsafe { libc::ioctl(master.as_raw_fd(), libc::TIOCSPTLCK, &unlock) } != 0 {
        return Err(io::Error::last_os_error());
    }

    let mut ptn: libc::c_uint = 0;
    if unsafe { libc::ioctl(master.as_raw_fd(), libc::TIOCGPTN, &mut ptn) } != 0 {
        return Err(io::Error::last_os_error());
    }

    let slave_path = CString::new(format!("/dev/pts/{ptn}")).unwrap();
    let slave_raw = unsafe { libc::open(slave_path.as_ptr(), libc::O_RDWR | libc::O_NOCTTY) };
    if slave_raw < 0 {
        return Err(io::Error::last_os_error());
    }
    let slave = unsafe { OwnedFd::from_raw_fd(slave_raw) };

    set_utf8_no_flow_control(master.as_raw_fd())?;
    resize_pty(master.as_raw_fd(), size)?;
    Ok(GuestPty { master, slave })
}

fn set_utf8_no_flow_control(fd: RawFd) -> io::Result<()> {
    let mut tios: libc::termios = unsafe { std::mem::zeroed() };
    if unsafe { libc::tcgetattr(fd, &mut tios) } != 0 {
        return Err(io::Error::last_os_error());
    }
    tios.c_iflag |= libc::IUTF8;
    tios.c_iflag &= !(libc::IXON | libc::IXOFF);
    if unsafe { libc::tcsetattr(fd, libc::TCSANOW, &tios) } != 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

pub fn resize_pty(master_fd: RawFd, size: WinSize) -> io::Result<()> {
    let ws = libc::winsize {
        ws_row: if size.rows == 0 { 24 } else { size.rows },
        ws_col: if size.cols == 0 { 80 } else { size.cols },
        ws_xpixel: size.xpixel,
        ws_ypixel: size.ypixel,
    };
    if unsafe { libc::ioctl(master_fd, libc::TIOCSWINSZ, &ws) } != 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}
