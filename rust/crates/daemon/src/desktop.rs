use std::{
    fs::{self, File, Permissions},
    io,
    os::{
        fd::{AsRawFd, FromRawFd, OwnedFd},
        unix::{
            ffi::OsStrExt,
            fs::{FileTypeExt, MetadataExt, PermissionsExt, chown},
            net::UnixStream,
            process::CommandExt,
        },
    },
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    time::{Duration, Instant},
};

const SOCKET: &str = "wayland-0";
const DISPLAY_UID: u32 = 65534;
const GUEST_GROUP: u32 = runtime::validate::ANDROID_MEDIA_GID;

pub struct Desktop {
    pub dir: PathBuf,
    program: PathBuf,
    compositor: Option<(Child, UnixStream)>,
    presentation: Option<OwnedFd>,
    encoder: Option<OwnedFd>,
}

impl Desktop {
    pub fn prepare(
        presentation: Option<&OwnedFd>,
        encoder: Option<&OwnedFd>,
    ) -> io::Result<Option<Self>> {
        let program = std::env::current_exe()?.with_file_name("bareboot-display");
        if !fs::symlink_metadata(&program)
            .is_ok_and(|m| m.is_file() && m.uid() == 0 && m.mode() & 0o022 == 0)
        {
            return Ok(None);
        }
        let mut template = *b"/dev/socket/bareboot-display-XXXXXX\0";
        if unsafe { libc::mkdtemp(template.as_mut_ptr().cast()) }.is_null() {
            return Err(io::Error::last_os_error());
        }
        let dir = PathBuf::from(std::ffi::OsStr::from_bytes(&template[..template.len() - 1]));
        let mut endpoint = Self {
            dir,
            program,
            compositor: None,
            presentation: None,
            encoder: None,
        };
        super::linux_impl::set_selinux_label(&endpoint.dir, "u:object_r:bareboot_display_socket:s0")?;
        chown(&endpoint.dir, Some(DISPLAY_UID), Some(GUEST_GROUP))?;
        fs::set_permissions(&endpoint.dir, Permissions::from_mode(0o750))?;
        endpoint.presentation = presentation.map(OwnedFd::try_clone).transpose()?;
        endpoint.encoder = encoder.map(OwnedFd::try_clone).transpose()?;
        Ok(Some(endpoint))
    }

    pub fn attach(&mut self) -> io::Result<OwnedFd> {
        if let Some((child, _)) = &mut self.compositor
            && child.try_wait()?.is_some()
        {
            self.compositor = None;
        }
        if self.compositor.is_none() {
            self.start()?;
        }
        let (display, viewer) = UnixStream::pair()?;
        let control = &self.compositor.as_ref().unwrap().1;
        protocol::send_frame(control.as_raw_fd(), &[0], Some(display.as_raw_fd()))?;
        Ok(viewer.into())
    }

    fn start(&mut self) -> io::Result<()> {
        let socket = self.dir.join(SOCKET);
        let _ = fs::remove_file(&socket);
        let (parent_socket, child_socket) = UnixStream::pair()?;
        parent_socket.set_write_timeout(Some(Duration::from_millis(250)))?;
        let parent = unsafe { libc::getpid() };
        let parent_arg = std::ffi::CString::new(parent.to_string()).unwrap();
        let binary = File::open(&self.program)?;
        let exec = duplicate(&binary)?;
        let channel = duplicate(&child_socket)?;
        let channel_arg = std::ffi::CString::new(channel.as_raw_fd().to_string()).unwrap();
        let presentation = self.presentation.as_ref().map(duplicate).transpose()?;
        let presentation_arg = std::ffi::CString::new(
            presentation.as_ref().map_or(-1, AsRawFd::as_raw_fd).to_string()
        ).unwrap();
        let encoder = self.encoder.as_ref().map(duplicate).transpose()?;
        let encoder_arg = std::ffi::CString::new(
            encoder.as_ref().map_or(-1, AsRawFd::as_raw_fd).to_string()
        ).unwrap();
        let mut command = Command::new(&self.program);
        command
            .env_clear()
            .current_dir(&self.dir)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        let boost = runtime::scheduler::foreground_util_min();
        unsafe {
            command.pre_exec(move || {
                let _ = runtime::scheduler::set_cpu_util_min(boost);
                let ctx = b"u:r:bareboot_display:s0\0";
                let attr = libc::open(
                    c"/proc/thread-self/attr/exec".as_ptr(),
                    libc::O_WRONLY | libc::O_CLOEXEC,
                );
                if attr < 0 {
                    return Err(io::Error::last_os_error());
                }
                let n = libc::write(attr, ctx.as_ptr().cast(), ctx.len());
                let error = io::Error::last_os_error();
                libc::close(attr);
                if n != ctx.len() as isize {
                    return Err(error);
                }
                libc::umask(0o117);
                if libc::unshare(libc::CLONE_NEWNET) != 0
                    || libc::setgroups(0, std::ptr::null()) != 0
                    || libc::setgid(GUEST_GROUP) != 0
                    || libc::setuid(DISPLAY_UID) != 0
                    || libc::syscall(
                        libc::SYS_close_range,
                        3u32,
                        u32::MAX,
                        libc::CLOSE_RANGE_CLOEXEC,
                    ) != 0
                    || libc::fcntl(channel.as_raw_fd(), libc::F_SETFD, 0) != 0
                    || presentation.as_ref().is_some_and(|fd| libc::fcntl(fd.as_raw_fd(), libc::F_SETFD, 0) != 0)
                    || encoder.as_ref().is_some_and(|fd| libc::fcntl(fd.as_raw_fd(), libc::F_SETFD, 0) != 0)
                {
                    return Err(io::Error::last_os_error());
                }
                let argv = [
                    c"bareboot-display".as_ptr(),
                    parent_arg.as_ptr(),
                    channel_arg.as_ptr(),
                    presentation_arg.as_ptr(),
                    encoder_arg.as_ptr(),
                    std::ptr::null(),
                ];
                let env = [std::ptr::null::<libc::c_char>()];
                libc::syscall(
                    libc::SYS_execveat,
                    exec.as_raw_fd(),
                    c"".as_ptr(),
                    argv.as_ptr(),
                    env.as_ptr(),
                    libc::AT_EMPTY_PATH,
                );
                Err(io::Error::last_os_error())
            });
        }
        let mut child = command.spawn()?;
        drop(command);
        drop(child_socket);
        let deadline = Instant::now() + Duration::from_secs(3);
        let ready = loop {
            match child.try_wait() {
                Ok(Some(status)) => {
                    break Err(io::Error::other(format!("display server exited: {status}")));
                }
                Err(e) => break Err(e),
                Ok(None) => (),
            }
            if fs::symlink_metadata(&socket).is_ok_and(|m| m.file_type().is_socket()) {
                break Ok(());
            }
            if Instant::now() >= deadline {
                break Err(io::Error::new(
                    io::ErrorKind::TimedOut,
                    "display socket did not start",
                ));
            }
            std::thread::sleep(Duration::from_millis(20));
        };
        if let Err(e) = ready {
            let _ = child.kill();
            let _ = child.wait();
            return Err(e);
        }
        self.compositor = Some((child, parent_socket));
        Ok(())
    }
}

fn duplicate(fd: &impl AsRawFd) -> io::Result<OwnedFd> {
    let n = unsafe { libc::fcntl(fd.as_raw_fd(), libc::F_DUPFD_CLOEXEC, 64) };
    if n < 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(unsafe { OwnedFd::from_raw_fd(n) })
    }
}

impl Drop for Desktop {
    fn drop(&mut self) {
        if let Some((mut child, _)) = self.compositor.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
        let _ = fs::remove_file(Path::new(&self.dir).join(SOCKET));
        let _ = fs::remove_dir(&self.dir);
    }
}
