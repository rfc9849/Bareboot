use daemon::{DaemonConfig, run_daemon};
use protocol::{
    CheckState, DEFAULT_SHARED_DIR, DEFAULT_SOCKET_PATH, DEFAULT_STATE_DIR, Request, Response, WinSize, decode_response,
    encode_request, recv_frame, send_frame,
};
use std::env;
use std::io::{self, Write};
use std::os::fd::{AsRawFd, OwnedFd};
#[cfg(target_os = "linux")]
use std::os::linux::net::SocketAddrExt;
#[cfg(target_os = "linux")]
use std::os::unix::net::SocketAddr;
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::atomic::{AtomicBool, Ordering};

static WINCH_FLAG: AtomicBool = AtomicBool::new(false);

extern "C" fn handle_sigwinch(_sig: libc::c_int) {
    WINCH_FLAG.store(true, Ordering::SeqCst);
}

fn usage() {
    eprintln!(
        "bareboot [--socket PATH] <command>\n\
         Commands:\n  \
           daemon [--state-dir DIR] [--dns IP]\n  \
           ping\n  \
           status\n  \
           check\n  \
           images\n  \
           create <name> <size_gb> <tarball_path> <username> <os_label>\n  \
           resize <name> <size_gb>\n  \
           delete <name>\n  \
           gpu <name> <on|off>\n  \
           configure <name> <os_label> <username> <keep_awake 0|1> <init none|system> <isolated_net 0|1> [ssh_port] [port_forwards]\n  \
           rename <name> <new_name>\n  \
           export <name> <dest_path.tar.gz>\n  \
           options <purge_on_uninstall 0|1> <relaxed_sync 0|1>\n  \
           log daemon | log <autostart|sshd> <distro>\n  \
           start <name> <image_path> [--shared-dir DIR | --no-shared]\n  \
           stop <name>\n  \
           shell <name> [--uid N] [--gid N] [-- <cmd...>]\n  \
           attach <session_id>\n  \
           display <name>\n  \
           sessions\n  \
           kill <session_id> [signal]"
    );
}

fn send_rpc(stream: &UnixStream, req: &Request) -> io::Result<(Response, Option<OwnedFd>)> {
    let payload = encode_request(req)?;
    send_frame(stream.as_raw_fd(), &payload, None)?;
    let (resp_bytes, fd) = recv_frame(stream.as_raw_fd())?;
    let resp = decode_response(&resp_bytes)?;
    Ok((resp, fd))
}

fn query_tty_size() -> WinSize {
    let mut ws: libc::winsize = unsafe { std::mem::zeroed() };
    let rc = unsafe { libc::ioctl(libc::STDIN_FILENO, libc::TIOCGWINSZ, &mut ws) };
    if rc == 0 && ws.ws_col > 0 && ws.ws_row > 0 {
        WinSize { cols: ws.ws_col, rows: ws.ws_row, xpixel: ws.ws_xpixel, ypixel: ws.ws_ypixel }
    } else {
        WinSize { cols: 80, rows: 24, xpixel: 0, ypixel: 0 }
    }
}

struct RawTermGuard {
    orig: Option<libc::termios>,
}

impl RawTermGuard {
    fn enter() -> Self {
        if unsafe { libc::isatty(libc::STDIN_FILENO) } != 1 {
            return Self { orig: None };
        }
        let mut orig: libc::termios = unsafe { std::mem::zeroed() };
        if unsafe { libc::tcgetattr(libc::STDIN_FILENO, &mut orig) } != 0 {
            return Self { orig: None };
        }
        let mut raw = orig;
        unsafe { libc::cfmakeraw(&mut raw) };
        if unsafe { libc::tcsetattr(libc::STDIN_FILENO, libc::TCSANOW, &raw) } != 0 {
            return Self { orig: None };
        }
        Self { orig: Some(orig) }
    }
}

impl Drop for RawTermGuard {
    fn drop(&mut self) {
        if let Some(orig) = self.orig {
            unsafe {
                let _ = libc::tcsetattr(libc::STDIN_FILENO, libc::TCSANOW, &orig);
            }
        }
    }
}

fn run_pty_bridge(
    ctrl: &UnixStream,
    session_id: u32,
    master: OwnedFd,
    output: &[u8],
) -> io::Result<()> {
    let _raw_guard = RawTermGuard::enter();
    unsafe {
        libc::signal(
            libc::SIGWINCH,
            handle_sigwinch as *const () as libc::sighandler_t,
        );
    }

    let mfd = master.as_raw_fd();
    let stdin_fd = libc::STDIN_FILENO;
    let stdout_fd = libc::STDOUT_FILENO;
    let mut stdin_open = true;
    let mut buf = [0u8; 4096];

    write_all_fd(stdout_fd, output)?;

    loop {
        if WINCH_FLAG.swap(false, Ordering::SeqCst) {
            let _ = send_rpc(
                ctrl,
                &Request::ResizePty {
                    session_id,
                    size: query_tty_size(),
                },
            );
        }

        let mut fds = [
            libc::pollfd {
                fd: mfd,
                events: libc::POLLIN,
                revents: 0,
            },
            libc::pollfd {
                fd: if stdin_open { stdin_fd } else { -1 },
                events: libc::POLLIN,
                revents: 0,
            },
        ];

        let n = unsafe { libc::poll(fds.as_mut_ptr(), 2, 200) };
        if n < 0 {
            let err = io::Error::last_os_error();
            if err.kind() == io::ErrorKind::Interrupted {
                continue;
            }
            return Err(err);
        }
        if n == 0 {
            continue;
        }

        if (fds[0].revents & (libc::POLLIN | libc::POLLHUP | libc::POLLERR)) != 0 {
            let r = unsafe { libc::read(mfd, buf.as_mut_ptr() as *mut libc::c_void, buf.len()) };
            if r <= 0 {
                break;
            }
            write_all_fd(stdout_fd, &buf[..r as usize])?;
        }

        if stdin_open && (fds[1].revents & (libc::POLLIN | libc::POLLHUP)) != 0 {
            let r =
                unsafe { libc::read(stdin_fd, buf.as_mut_ptr() as *mut libc::c_void, buf.len()) };
            if r <= 0 {
                stdin_open = false;
            } else if write_all_fd(mfd, &buf[..r as usize]).is_err() {
                break;
            }
        }
    }

    let _ = io::stdout().flush();
    Ok(())
}

fn connect_socket(path: &PathBuf) -> io::Result<UnixStream> {
    let s = path.to_string_lossy();
    if let Some(abs_name) = s.strip_prefix('@') {
        #[cfg(target_os = "linux")]
        {
            let addr = SocketAddr::from_abstract_name(abs_name.as_bytes())?;
            return UnixStream::connect_addr(&addr);
        }
        #[cfg(not(target_os = "linux"))]
        {
            let _ = abs_name;
            return Err(io::Error::new(
                io::ErrorKind::Unsupported,
                "abstract unix sockets require Linux",
            ));
        }
    }
    UnixStream::connect(path)
}

fn write_all_fd(fd: libc::c_int, mut data: &[u8]) -> io::Result<()> {
    while !data.is_empty() {
        let w = unsafe { libc::write(fd, data.as_ptr() as *const libc::c_void, data.len()) };
        if w < 0 {
            let err = io::Error::last_os_error();
            if err.kind() == io::ErrorKind::Interrupted {
                continue;
            }
            return Err(err);
        }
        data = &data[w as usize..];
    }
    Ok(())
}

fn main() -> ExitCode {
    let mut args = env::args().skip(1).peekable();
    let mut socket_path = PathBuf::from(DEFAULT_SOCKET_PATH);

    while let Some(arg) = args.peek() {
        if arg == "--socket" {
            args.next();
            let Some(p) = args.next() else {
                usage();
                return ExitCode::from(2);
            };
            socket_path = PathBuf::from(p);
        } else {
            break;
        }
    }

    let Some(cmd) = args.next() else {
        usage();
        return ExitCode::from(2);
    };

    if cmd == "daemon" {
        let mut cfg = DaemonConfig {
            socket_path,
            state_dir: PathBuf::from(DEFAULT_STATE_DIR),
            dns_servers: Vec::new(),
        };
        while let Some(flag) = args.next() {
            match flag.as_str() {
                "--state-dir" => {
                    let Some(dir) = args.next() else {
                        usage();
                        return ExitCode::from(2);
                    };
                    cfg.state_dir = PathBuf::from(dir);
                }
                "--dns" => {
                    let Some(ip) = args.next() else {
                        usage();
                        return ExitCode::from(2);
                    };
                    cfg.dns_servers.push(ip);
                }
                _ => {
                    usage();
                    return ExitCode::from(2);
                }
            }
        }
        return match run_daemon(cfg) {
            Ok(()) => ExitCode::SUCCESS,
            Err(e) => {
                eprintln!("daemon error: {e}");
                ExitCode::FAILURE
            }
        };
    }

    let stream = match connect_socket(&socket_path) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("failed to connect to {}: {e}", socket_path.display());
            return ExitCode::FAILURE;
        }
    };

    match cmd.as_str() {
        "ping" => match send_rpc(&stream, &Request::Ping) {
            Ok((
                Response::Pong {
                    version,
                    daemon_pid,
                    selinux_enforcing,
                },
                _,
            )) => {
                println!(
                    "pong proto={version} daemon_pid={daemon_pid} selinux_enforcing={selinux_enforcing}"
                );
                ExitCode::SUCCESS
            }
            Ok((Response::Error { code, message }, _)) => {
                eprintln!("error({code}): {message}");
                ExitCode::FAILURE
            }
            Ok((other, _)) => {
                eprintln!("unexpected response: {other:?}");
                ExitCode::FAILURE
            }
            Err(e) => {
                eprintln!("rpc failed: {e}");
                ExitCode::FAILURE
            }
        },
        "status" => match send_rpc(&stream, &Request::Status) {
            Ok((
                Response::Status {
                    kernel,
                    distros,
                    sessions,
                    options,
                },
                _,
            )) => {
                println!(
                    "kernel: page={} selinux={} ns=[mnt:{} uts:{} pid:{} ipc:{} net:{} user:{} cgroup:{}]",
                    kernel.page_size,
                    kernel.selinux_enforcing,
                    kernel.has_mnt_ns,
                    kernel.has_uts_ns,
                    kernel.has_pid_ns,
                    kernel.has_ipc_ns,
                    kernel.has_net_ns,
                    kernel.has_user_ns,
                    kernel.has_cgroup_ns,
                );
                println!(
                    "options: purge_on_uninstall={} relaxed_sync={}",
                    options.purge_on_uninstall, options.relaxed_sync
                );
                println!("distros: {}", distros.len());
                for d in distros {
                    println!(
                        "  {} holder_pid={} init_pid={} image={}",
                        d.name, d.holder_pid, d.init_pid, d.image_path
                    );
                }
                println!("sessions: {}", sessions.len());
                for s in sessions {
                    println!("  #{} distro={} pid={} uid={}", s.session_id, s.distro, s.pid, s.uid);
                }
                ExitCode::SUCCESS
            }
            Ok((Response::Error { code, message }, _)) => {
                eprintln!("error({code}): {message}");
                ExitCode::FAILURE
            }
            Ok((other, _)) => {
                eprintln!("unexpected response: {other:?}");
                ExitCode::FAILURE
            }
            Err(e) => {
                eprintln!("rpc failed: {e}");
                ExitCode::FAILURE
            }
        },
        "check" => match send_rpc(&stream, &Request::KernelReport) {
            Ok((Response::KernelReport { release, checks }, _)) => {
                println!("kernel: {release}");
                for check in checks {
                    let state = match check.state {
                        CheckState::Present => "present",
                        CheckState::Missing => "missing",
                        CheckState::Unknown => "unknown",
                    };
                    println!("  {} {state}", check.id);
                }
                ExitCode::SUCCESS
            }
            Ok((Response::Error { code, message }, _)) => {
                eprintln!("error({code}): {message}");
                ExitCode::FAILURE
            }
            Ok((other, _)) => {
                eprintln!("unexpected response: {other:?}");
                ExitCode::FAILURE
            }
            Err(e) => {
                eprintln!("rpc failed: {e}");
                ExitCode::FAILURE
            }
        },
        "images" => match send_rpc(&stream, &Request::ListImages) {
            Ok((Response::Images(images), _)) => {
                for img in images {
                    let used_mb = img.used_bytes / (1024 * 1024);
                    let size_gb = img.size_bytes / (1024 * 1024 * 1024);
                    let forwards = runtime::validate::format_port_forwards(&img.port_forwards);
                    println!(
                        "{} used={}MB size={}GB free={}MB path={} os={:?} user={:?} keep_awake={} init={} isolated_net={} ssh_port={} forwards={} busy={:?} gpu_enabled={} gpu_available={} gpu_running={}",
                        img.name,
                        used_mb,
                        size_gb,
                        img.fs_free_bytes / (1024 * 1024),
                        img.image_path,
                        img.os_label,
                        img.username,
                        img.keep_awake,
                        img.init,
                        img.isolated_net,
                        img.ssh_port,
                        if forwards.is_empty() { "-" } else { &forwards },
                        img.busy,
                        img.gpu_enabled,
                        img.gpu_available,
                        img.gpu_running
                    );
                }
                ExitCode::SUCCESS
            }
            Ok((Response::Error { code, message }, _)) => {
                eprintln!("error({code}): {message}");
                ExitCode::FAILURE
            }
            Ok((other, _)) => {
                eprintln!("unexpected response: {other:?}");
                ExitCode::FAILURE
            }
            Err(e) => {
                eprintln!("rpc failed: {e}");
                ExitCode::FAILURE
            }
        },
        "create" => {
            let (Some(name), Some(size_gb), Some(tarball_path), Some(username), Some(os_label)) = (
                args.next(),
                args.next().and_then(|s| s.parse::<u32>().ok()),
                args.next(),
                args.next(),
                args.next(),
            ) else {
                usage();
                return ExitCode::from(2);
            };
            expect_ok(send_rpc(
                &stream,
                &Request::CreateDistro {
                    name,
                    size_gb,
                    tarball_path,
                    username,
                    os_label,
                },
            ))
        }
        "delete" => {
            let Some(name) = args.next() else {
                usage();
                return ExitCode::from(2);
            };
            expect_ok(send_rpc(&stream, &Request::DeleteDistro { name }))
        }
        "gpu" => {
            let (Some(name), Some(setting), None) = (args.next(), args.next(), args.next()) else {
                usage();
                return ExitCode::from(2);
            };
            let enabled = match setting.as_str() {
                "on" => true,
                "off" => false,
                _ => {
                    usage();
                    return ExitCode::from(2);
                }
            };
            expect_ok(send_rpc(&stream, &Request::ConfigureGpu { name, enabled }))
        }
        "configure" => {
            let (Some(name), Some(os_label), Some(username), Some(keep_awake), Some(init), Some(isolated_net)) =
                (args.next(), args.next(), args.next(), args.next(), args.next(), args.next())
            else {
                usage();
                return ExitCode::from(2);
            };
            let ssh_port = match args.next() {
                Some(s) => match s.parse::<u16>() {
                    Ok(p) => p,
                    Err(_) => {
                        usage();
                        return ExitCode::from(2);
                    }
                },
                None => 0,
            };
            let port_forwards = match args.next() {
                Some(s) => match runtime::validate::parse_port_forwards(&s) {
                    Ok(f) => f,
                    Err(e) => {
                        eprintln!("invalid port forwards: {e}");
                        return ExitCode::from(2);
                    }
                },
                None => Vec::new(),
            };
            expect_ok(send_rpc(
                &stream,
                &Request::ConfigureImage {
                    name,
                    os_label,
                    username,
                    keep_awake: keep_awake == "1",
                    init,
                    isolated_net: isolated_net == "1",
                    ssh_port,
                    port_forwards,
                },
            ))
        }
        "rename" => {
            let (Some(name), Some(new_name)) = (args.next(), args.next()) else {
                usage();
                return ExitCode::from(2);
            };
            expect_ok(send_rpc(&stream, &Request::RenameImage { name, new_name }))
        }
        "export" => {
            let (Some(name), Some(dest_path)) = (args.next(), args.next()) else {
                usage();
                return ExitCode::from(2);
            };
            expect_ok(send_rpc(&stream, &Request::ExportImage { name, dest_path }))
        }
        "options" => {
            let (Some(purge), Some(relaxed_sync)) = (args.next(), args.next()) else {
                usage();
                return ExitCode::from(2);
            };
            expect_ok(send_rpc(
                &stream,
                &Request::SetDaemonOptions {
                    purge_on_uninstall: purge == "1",
                    relaxed_sync: relaxed_sync == "1",
                },
            ))
        }
        "log" => {
            let (Some(name), distro) = (args.next(), args.next().unwrap_or_default()) else {
                usage();
                return ExitCode::from(2);
            };
            match send_rpc(&stream, &Request::ReadLog { distro, name }) {
                Ok((Response::Log(bytes), _)) => {
                    let _ = io::stdout().write_all(&bytes);
                    ExitCode::SUCCESS
                }
                Ok((Response::Error { code, message }, _)) => {
                    eprintln!("error({code}): {message}");
                    ExitCode::FAILURE
                }
                Ok((other, _)) => {
                    eprintln!("unexpected response: {other:?}");
                    ExitCode::FAILURE
                }
                Err(e) => {
                    eprintln!("rpc failed: {e}");
                    ExitCode::FAILURE
                }
            }
        }
        "resize" => {
            let (Some(name), Some(size_gb)) = (
                args.next(),
                args.next().and_then(|s| s.parse::<u32>().ok()),
            ) else {
                usage();
                return ExitCode::from(2);
            };
            expect_ok(send_rpc(&stream, &Request::ResizeDistro { name, size_gb }))
        }
        "start" => {
            let (Some(name), Some(image_path)) = (args.next(), args.next()) else {
                usage();
                return ExitCode::from(2);
            };
            let mut shared_dir = DEFAULT_SHARED_DIR.to_string();
            while let Some(flag) = args.next() {
                match flag.as_str() {
                    "--shared-dir" => {
                        let Some(dir) = args.next() else {
                            usage();
                            return ExitCode::from(2);
                        };
                        shared_dir = dir;
                    }
                    "--no-shared" => shared_dir.clear(),
                    _ => {
                        usage();
                        return ExitCode::from(2);
                    }
                }
            }
            expect_ok(send_rpc(
                &stream,
                &Request::StartDistro {
                    name,
                    image_path,
                    shared_dir,
                },
            ))
        }
        "stop" => {
            let Some(name) = args.next() else {
                usage();
                return ExitCode::from(2);
            };
            expect_ok(send_rpc(&stream, &Request::StopDistro { name }))
        }
        "display" => {
            let (Some(name), None) = (args.next(), args.next()) else {
                usage();
                return ExitCode::from(2);
            };
            let listener = match std::net::TcpListener::bind("127.0.0.1:0") {
                Ok(l) => l,
                Err(e) => {
                    eprintln!("could not bind display tcp listener: {e}");
                    return ExitCode::FAILURE;
                }
            };
            let Ok(addr) = listener.local_addr() else {
                return ExitCode::FAILURE;
            };
            println!("PORT={}", addr.port());
            let _ = io::stdout().flush();
            let Ok((mut tcp, _)) = listener.accept() else {
                return ExitCode::FAILURE;
            };
            drop(listener);
            let _ = tcp.set_nodelay(true);
            match send_rpc(&stream, &Request::AttachDesktop { name }) {
                Ok((Response::Ok, Some(fd))) => {
                    let mut display = UnixStream::from(fd);
                    let (Ok(mut tcp_in), Ok(mut disp_in)) =
                        (tcp.try_clone(), display.try_clone())
                    else {
                        return ExitCode::FAILURE;
                    };
                    let _ = std::thread::Builder::new().spawn(move || {
                        let _ = io::copy(&mut tcp_in, &mut disp_in);
                        let _ = disp_in.shutdown(std::net::Shutdown::Write);
                    });
                    if let Ok(tcp_watch) = tcp.try_clone() {
                        let _ = std::thread::Builder::new().spawn(move || {
                            let mut sink = io::sink();
                            let _ = io::copy(&mut io::stdin().lock(), &mut sink);
                            let _ = tcp_watch.shutdown(std::net::Shutdown::Both);
                        });
                    }
                    let forwarded = io::copy(&mut display, &mut tcp);
                    let _ = tcp.shutdown(std::net::Shutdown::Both);
                    match forwarded {
                        Ok(_) => ExitCode::SUCCESS,
                        Err(e)
                            if matches!(
                                e.kind(),
                                io::ErrorKind::BrokenPipe
                                    | io::ErrorKind::ConnectionReset
                                    | io::ErrorKind::NotConnected
                            ) =>
                        {
                            ExitCode::SUCCESS
                        }
                        Err(e) => {
                            eprintln!("could not forward display stream: {e}");
                            ExitCode::FAILURE
                        }
                    }
                }
                Ok((Response::Error { code, message }, _)) => {
                    eprintln!("error({code}): {message}");
                    ExitCode::FAILURE
                }
                Ok(_) => {
                    eprintln!("display connection did not arrive");
                    ExitCode::FAILURE
                }
                Err(e) => {
                    eprintln!("rpc failed: {e}");
                    ExitCode::FAILURE
                }
            }
        }
        "shell" => {
            let Some(distro) = args.next() else {
                usage();
                return ExitCode::from(2);
            };
            let mut uid = 1000u32;
            let mut gid = 1000u32;
            let mut argv = Vec::new();
            while let Some(tok) = args.next() {
                match tok.as_str() {
                    "--uid" => {
                        let Some(v) = args.next().and_then(|s| s.parse().ok()) else {
                            usage();
                            return ExitCode::from(2);
                        };
                        uid = v;
                    }
                    "--gid" => {
                        let Some(v) = args.next().and_then(|s| s.parse().ok()) else {
                            usage();
                            return ExitCode::from(2);
                        };
                        gid = v;
                    }
                    "--" => {
                        argv.extend(args);
                        break;
                    }
                    other => {
                        argv.push(other.to_string());
                        argv.extend(args);
                        break;
                    }
                }
            }
            let term = env::var("TERM").unwrap_or_else(|_| "xterm-256color".into());
            let req = Request::OpenSession {
                distro,
                uid,
                gid,
                size: query_tty_size(),
                argv,
                env: vec![format!("TERM={term}")],
            };
            match send_rpc(&stream, &req) {
                Ok((Response::SessionOpened { session_id, .. }, Some(master_fd))) => {
                    match run_pty_bridge(&stream, session_id, master_fd, &[]) {
                        Ok(()) => ExitCode::SUCCESS,
                        Err(e) => {
                            eprintln!("pty bridge error: {e}");
                            ExitCode::FAILURE
                        }
                    }
                }
                Ok((Response::Error { code, message }, _)) => {
                    eprintln!("error({code}): {message}");
                    ExitCode::FAILURE
                }
                Ok((other, _)) => {
                    eprintln!("unexpected response: {other:?}");
                    ExitCode::FAILURE
                }
                Err(e) => {
                    eprintln!("rpc failed: {e}");
                    ExitCode::FAILURE
                }
            }
        }
        "attach" => {
            let Some(session_id) = args.next().and_then(|s| s.parse::<u32>().ok()) else {
                usage();
                return ExitCode::from(2);
            };
            let result = send_rpc(&stream, &Request::AttachTab { session_id, size: query_tty_size() })
                .and_then(|(response, fd)| match (response, fd) {
                    (Response::TabAttached(output), Some(master_fd)) => {
                        run_pty_bridge(&stream, session_id, master_fd, &output)
                    }
                    (Response::Error { message, .. }, _) => Err(io::Error::other(message)),
                    _ => Err(io::Error::other("unexpected response when attaching terminal tab")),
                });
            if let Err(e) = result {
                eprintln!("could not attach terminal tab: {e}");
                ExitCode::FAILURE
            } else {
                ExitCode::SUCCESS
            }
        }
        "sessions" => match send_rpc(&stream, &Request::ListSessions) {
            Ok((Response::Sessions(sessions), _)) => {
                for s in sessions {
                    println!("#{} distro={} pid={} uid={}", s.session_id, s.distro, s.pid, s.uid);
                }
                ExitCode::SUCCESS
            }
            Ok((Response::Error { code, message }, _)) => {
                eprintln!("error({code}): {message}");
                ExitCode::FAILURE
            }
            Ok((other, _)) => {
                eprintln!("unexpected response: {other:?}");
                ExitCode::FAILURE
            }
            Err(e) => {
                eprintln!("rpc failed: {e}");
                ExitCode::FAILURE
            }
        },
        "kill" => {
            let Some(session_id) = args.next().and_then(|s| s.parse::<u32>().ok()) else {
                usage();
                return ExitCode::from(2);
            };
            let signal = args
                .next()
                .and_then(|s| s.parse::<i32>().ok())
                .unwrap_or(libc::SIGTERM);
            expect_ok(send_rpc(&stream, &Request::KillSession { session_id, signal }))
        }
        _ => {
            usage();
            ExitCode::from(2)
        }
    }
}

fn expect_ok(res: io::Result<(Response, Option<OwnedFd>)>) -> ExitCode {
    match res {
        Ok((Response::Ok, _)) => {
            println!("ok");
            ExitCode::SUCCESS
        }
        Ok((Response::Error { code, message }, _)) => {
            eprintln!("error({code}): {message}");
            ExitCode::FAILURE
        }
        Ok((other, _)) => {
            eprintln!("unexpected response: {other:?}");
            ExitCode::FAILURE
        }
        Err(e) => {
            eprintln!("rpc failed: {e}");
            ExitCode::FAILURE
        }
    }
}
