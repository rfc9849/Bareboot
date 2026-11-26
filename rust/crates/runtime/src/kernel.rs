use crate::busybox;
use protocol::{CheckState, KernelCheck, KernelInfo};
use std::ffi::CStr;
use std::fs;
use std::path::Path;
use std::process::{Command, Stdio};

const NAMESPACES: &[&str] = &["mnt", "uts", "pid", "ipc", "net", "user", "cgroup"];
const CGROUP_CONTROLLERS: &[&str] = &["cpu", "memory", "pids"];
const FILESYSTEMS: &[&str] = &["ext4", "devpts", "overlay"];
const CONFIG_OPTIONS: &[&str] = &[
    "SYSVIPC",
    "POSIX_MQUEUE",
    "TUN",
    "VETH",
    "BRIDGE",
    "BRIDGE_NETFILTER",
    "NETFILTER_XT_MATCH_ADDRTYPE",
    "NF_TABLES",
    "NFT_COMPAT",
    "SECCOMP_FILTER",
    "TMPFS_XATTR",
    "FHANDLE",
];

pub fn probe_kernel() -> KernelInfo {
    let page_size = unsafe { libc::sysconf(libc::_SC_PAGESIZE) };
    let page_size = u32::try_from(page_size).unwrap_or(4096);

    let pid = Path::new("/proc/self/ns/pid").exists();
    let net = Path::new("/proc/self/ns/net").exists();
    #[cfg(target_os = "linux")]
    let isolated_network = pid && net && crate::uplink::available();
    #[cfg(not(target_os = "linux"))]
    let isolated_network = false;

    KernelInfo {
        page_size,
        selinux_enforcing: selinux_enforcing(),
        has_mnt_ns: Path::new("/proc/self/ns/mnt").exists(),
        has_uts_ns: Path::new("/proc/self/ns/uts").exists(),
        has_pid_ns: pid,
        has_ipc_ns: Path::new("/proc/self/ns/ipc").exists(),
        has_net_ns: net,
        has_user_ns: Path::new("/proc/self/ns/user").exists(),
        has_cgroup_ns: Path::new("/proc/self/ns/cgroup").exists(),
        isolated_network,
    }
}

pub fn kernel_report() -> (String, Vec<KernelCheck>) {
    let mut checks = Vec::new();
    let mut add = |id: String, present: Option<bool>| {
        let state = match present {
            Some(true) => CheckState::Present,
            Some(false) => CheckState::Missing,
            None => CheckState::Unknown,
        };
        checks.push(KernelCheck { id, state });
    };

    add("loop_control".into(), Some(Path::new("/dev/loop-control").exists()));

    let filesystems = fs::read_to_string("/proc/filesystems").ok();
    for name in FILESYSTEMS {
        add(
            format!("fs_{name}"),
            filesystems.as_deref().map(|text| filesystem_listed(text, name)),
        );
    }
    for name in NAMESPACES {
        add(format!("ns_{name}"), Some(Path::new("/proc/self/ns").join(name).exists()));
    }

    let controllers = fs::read_to_string("/sys/fs/cgroup/cgroup.controllers").ok();
    add("cgroup2".into(), Some(controllers.is_some()));
    for name in CGROUP_CONTROLLERS {
        add(
            format!("cgroup_{name}"),
            controllers.as_deref().map(|text| text.split_whitespace().any(|c| c == *name)),
        );
    }

    let config = read_kernel_config();
    for option in CONFIG_OPTIONS {
        add(
            format!("config_{option}"),
            config.as_deref().map(|text| config_enabled(text, option)),
        );
    }
    (kernel_release(), checks)
}

fn selinux_enforcing() -> i8 {
    fs::read_to_string("/sys/fs/selinux/enforce")
        .ok()
        .and_then(|s| s.trim().parse::<i8>().ok())
        .unwrap_or(-1)
}

fn kernel_release() -> String {
    let mut uts: libc::utsname = unsafe { std::mem::zeroed() };
    if unsafe { libc::uname(&mut uts) } != 0 {
        return String::new();
    }
    unsafe { CStr::from_ptr(uts.release.as_ptr()) }
        .to_string_lossy()
        .into_owned()
}

fn read_kernel_config() -> Option<String> {
    if !Path::new("/proc/config.gz").exists() {
        return None;
    }
    let out = Command::new(busybox().ok()?)
        .args(["zcat", "/proc/config.gz"])
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .ok()?;
    out.status
        .success()
        .then(|| String::from_utf8_lossy(&out.stdout).into_owned())
}

fn filesystem_listed(text: &str, name: &str) -> bool {
    text.lines()
        .any(|line| line.split_whitespace().last() == Some(name))
}

fn config_enabled(config: &str, option: &str) -> bool {
    config.lines().any(|line| {
        line.strip_prefix("CONFIG_")
            .and_then(|rest| rest.strip_prefix(option))
            .and_then(|rest| rest.strip_prefix('='))
            .is_some_and(|value| value == "y" || value == "m")
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn config_option_matching() {
        let config = "CONFIG_BRIDGE=y\nCONFIG_BRIDGE_NETFILTER=m\n# CONFIG_SYSVIPC is not set\nCONFIG_VETH=n\n";
        assert!(config_enabled(config, "BRIDGE"));
        assert!(config_enabled(config, "BRIDGE_NETFILTER"));
        assert!(!config_enabled(config, "SYSVIPC"));
        assert!(!config_enabled(config, "VETH"));
        assert!(!config_enabled("CONFIG_BRIDGE_NETFILTER=y\n", "BRIDGE"));
    }

    #[test]
    fn filesystem_matching() {
        let text = "nodev\tsysfs\nnodev\tdevpts\n\text4\nnodev\toverlay\n";
        assert!(filesystem_listed(text, "ext4"));
        assert!(filesystem_listed(text, "devpts"));
        assert!(!filesystem_listed(text, "f2fs"));
    }
}
