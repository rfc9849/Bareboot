import gzip
import subprocess
import sys

ADB = ["adb", "-d"]
FEATURES = {
    "PID-isolated guest": ("NAMESPACES", "PID_NS", "UTS_NS"),
    "Distribution init (systemd)": ("PID_NS", "CGROUPS"),
    "systemd extras": ("TMPFS_XATTR", "FHANDLE"),
    "IPC namespace": ("IPC_NS",),
    "System V IPC": ("SYSVIPC",),
    "POSIX message queues": ("POSIX_MQUEUE",),
    "Rootless containers": ("USER_NS",),
    "Docker bridge and pids controller": (
        "NET_NS", "VETH", "BRIDGE", "BRIDGE_NETFILTER", "NETFILTER_XT_MATCH_ADDRTYPE",
        "CGROUPS", "CGROUP_BPF", "CGROUP_PIDS", "OVERLAY_FS", "SECCOMP",
    ),
    "Debian nftables": ("NF_TABLES", "NFT_COMPAT"),
}


def main():
    if len(sys.argv) == 3 and sys.argv[1] == "--serial":
        ADB[1:] = ["-s", sys.argv[2]]
    page_size = subprocess.check_output((*ADB, "shell", "getconf", "PAGE_SIZE"), text=True, timeout=12).strip()
    raw = subprocess.check_output((*ADB, "exec-out", "cat", "/proc/config.gz"), timeout=12)
    config = dict(line.split("=", 1) for line in gzip.decompress(raw).decode().splitlines()
                  if line.startswith("CONFIG_") and "=" in line)
    print(f"Android page size: {page_size} bytes")
    for feature, options in FEATURES.items():
        missing = ["CONFIG_" + opt for opt in options if config.get("CONFIG_" + opt) not in ("y", "m")]
        print(f"{feature}: {'missing ' + ', '.join(missing) if missing else 'config options present'}")
    print("This check reads kernel config; it does not boot a guest or load kernel modules.")


if __name__ == "__main__":
    try:
        main()
    except (OSError, ValueError, subprocess.SubprocessError) as e:
        raise SystemExit(str(e))
