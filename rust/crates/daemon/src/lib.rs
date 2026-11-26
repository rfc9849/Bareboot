#[cfg(any(target_os = "linux", test))]
mod dns;
#[cfg(target_os = "linux")]
mod desktop;
#[cfg(target_os = "linux")]
mod gpu;
#[cfg(target_os = "linux")]
mod linux_impl;

use std::io;
use std::path::PathBuf;

pub struct DaemonConfig {
    pub socket_path: PathBuf,
    pub state_dir: PathBuf,
    pub dns_servers: Vec<String>,
}

impl Default for DaemonConfig {
    fn default() -> Self {
        Self {
            socket_path: PathBuf::from(protocol::DEFAULT_SOCKET_PATH),
            state_dir: PathBuf::from(protocol::DEFAULT_STATE_DIR),
            dns_servers: Vec::new(),
        }
    }
}

#[cfg(target_os = "linux")]
pub fn run_daemon(config: DaemonConfig) -> io::Result<()> {
    linux_impl::run(config)
}

#[cfg(not(target_os = "linux"))]
pub fn run_daemon(_config: DaemonConfig) -> io::Result<()> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "bareboot daemon requires Linux",
    ))
}
