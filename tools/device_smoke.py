import argparse
import ipaddress
from pathlib import Path
import re
import shlex
import subprocess
import time
from urllib.parse import quote
from uuid import uuid4
from xml.etree import ElementTree

ADB = ["adb", "-d"]
APP = "ch.ausfuhrli.bareboot"
BAREBOOT = "/data/adb/modules/bareboot/bin/bareboot"
FILES = f"content://{APP}.files"
NAME = "bareboot-smoke-" + uuid4().hex[:12]
STAGE = f"/data/local/tmp/{NAME}"
IMAGE = f"/data/adb/bareboot/distros/{NAME}/rootfs.img"


def command(args, *, check=True, timeout=30, input=None):
    return subprocess.run(args, text=True, capture_output=True, check=check, timeout=timeout, input=input)


def adb(*args, timeout=30):
    return command((*ADB, *args), timeout=timeout).stdout.strip()


def android(*args):
    return adb("shell", "-T", shlex.join(args))


def root(*args):
    return adb("shell", "-T", f"su -c {shlex.quote(shlex.join(args))}", timeout=90)


def lin(*args):
    return root(BAREBOOT, *args)


def require(condition, message):
    if not condition:
        raise RuntimeError(message)


def session_ids():
    return {
        int(sid): (name, int(pid), int(uid))
        for sid, name, pid, uid in re.findall(r"#(\d+) distro=(\S+) pid=(\d+) uid=(\d+)", lin("sessions"))
    }


def rejected(result):
    return result.returncode != 0 and any(reason in result.stderr for reason in (
        "unauthorized peer", "rpc failed: Broken pipe", "rpc failed: Connection reset",
    ))


def check_ipc_and_image(archive):
    adb("push", "dist/bareboot-aarch64-musl", f"{STAGE}/bareboot")
    android("chmod", "0755", f"{STAGE}/bareboot")
    unauthorized = command((*ADB, "shell", "-T", f"{STAGE}/bareboot ping"), check=False)
    require(rejected(unauthorized) or
            ("failed to connect" in unauthorized.stderr and "Permission denied" in unauthorized.stderr),
            "Unlisted Android shell reached the daemon or could not run the client")
    outside_host = command((*ADB, "shell", "-T", f"su -c 'unshare -m {BAREBOOT} ping'"), check=False)
    require(rejected(outside_host), "Root in a different mount namespace reached the daemon")
    report = lin("check")
    require(re.search(r"(?m)^  loop_control present$", report) and re.search(r"(?m)^  ns_mnt present$", report),
            f"Kernel report is missing required features:\n{report}")
    adb("push", str(archive), f"{STAGE}/rootfs.tar.xz" if archive.name.endswith(".xz") else f"{STAGE}/rootfs.tar.gz")
    remote = f"{STAGE}/rootfs.tar.xz" if archive.name.endswith(".xz") else f"{STAGE}/rootfs.tar.gz"
    backup = f"/data/media/0/Download/{NAME}.tar.gz"
    restored = NAME + "-r"
    try:
        lin("create", NAME, "4", remote, "debian", "Debian smoke test")
        lin("start", NAME, IMAGE, "--no-shared")
        output = lin("shell", NAME, "--", "/bin/sh", "-ec",
                     'test "$(id -un)" = debian; test "$(id -u)" = 1000; '
                     'test "$(sudo -n whoami)" = root; echo fresh-image-ok')
        require(output == "fresh-image-ok", f"Fresh Debian login or sudo failed: {output}")
        caps = lin("shell", NAME, "--uid", "0", "--gid", "0", "--", "/bin/sh", "-ec",
                   "grep -E '^Cap(Inh|Prm|Eff|Bnd):' /proc/self/status")
        values = [int(v, 16) for v in re.findall(r":\s*([0-9a-f]+)", caps)]
        require(len(values) == 4 and all(not v & ((1 << 19) | (1 << 21)) for v in values),
                "A guest shell still has SYS_PTRACE or SYS_ADMIN")
        lin("shell", NAME, "--", "/bin/sh", "-ec", f"echo {NAME} > ~/backup-probe")
        lin("stop", NAME)
        lin("export", NAME, backup)
        require(android("ls", "-l", f"/sdcard/Download/{NAME}.tar.gz").split()[4] ==
                root("stat", "-c", "%s", backup), "Files sees a different backup size than the daemon wrote")
        lin("create", restored, "4", backup, "debian", "Debian smoke restore")
        lin("start", restored, f"/data/adb/bareboot/distros/{restored}/rootfs.img", "--no-shared")
        output = lin("shell", restored, "--", "/bin/sh", "-ec", "cat ~/backup-probe")
        require(output == NAME, f"The restored image lost the home folder: {output}")
    finally:
        for image in (NAME, restored):
            command((*ADB, "shell", "-T", f"su -c '{BAREBOOT} stop {image}; {BAREBOOT} delete {image}'"), check=False)
        command((*ADB, "shell", "-T", f"su -c 'rm -f {backup}'"), check=False)
    require(NAME not in lin("images") and NAME not in lin("status"), "Disposable image or holder leaked")
    print("Pass: peer authorization, kernel report, fresh Debian, passwordless sudo, guest capabilities, "
          "backup and restore, deletion", flush=True)


def ssh_config(path):
    runs = ("ssh", "-F", str(path), "-o", "BatchMode=yes", "-o", "ConnectTimeout=5", "bareboot")
    effective = command(("ssh", "-G", "-F", str(path), "bareboot")).stdout
    fields = dict(line.split(" ", 1) for line in effective.splitlines() if " " in line)
    require(fields.get("hostname") == "127.0.0.1", "SSH config must point the bareboot host at loopback")
    serial = adb("get-serialno")
    local_port = f"tcp:{fields.get('port')}"
    forwards = [line.split() for line in adb("forward", "--list").splitlines()]
    require(any(len(f) == 3 and f[:2] == [serial, local_port] and f[2].startswith("tcp:") for f in forwards),
            "No SSH forward for this USB device")
    return runs


def ssh(runs, script, input=None):
    return command((*runs, script), timeout=24, input=input).stdout.strip()


def check_files(runs):
    roots = android("content", "query", "--uri", f"{FILES}/root", "--projection", "document_id")
    ids = re.findall(r"document_id=([^\r\n]+)", roots)
    require(len(ids) == 1, "Connect the shared folder to Android Files first")
    doc = quote(f"{ids[0]}/{NAME}.txt", safe="")
    uri = f"{FILES}/document/{doc}"
    path = f"/mnt/android/{NAME}.txt"
    try:
        require(ssh(runs, f"mountpoint -q /mnt/android && sudo -n sh -ec "
                          f"{shlex.quote(f'umask 002; set -C; printf from-debian > {path}')} && echo created")
                == "created", "Could not create a probe file in the mounted share")
        require(android("content", "read", "--uri", uri) == "from-debian", "Files could not read the Debian file")
        command((*ADB, "shell", "-T", "content", "write", "--uri", uri), input="from-android")
        require(ssh(runs, f"sudo -n cat {path}") == "from-android", "Debian did not receive the Files write")
    finally:
        ssh(runs, f"sudo -n rm -f {path}")
    print("Pass: USB SSH and Android Files read/write", flush=True)


def require_foreground():
    lines = [line for line in android("dumpsys", "window").splitlines() if "mCurrentFocus=" in line]
    require(any(re.search(rf"\b{re.escape(APP)}/", line) for line in lines),
            "Bareboot lost the foreground during the UI test")


def terminal_input(*args):
    require_foreground()
    return android("input", *args)


def ui():
    require_foreground()
    path = f"{STAGE}/window.xml"
    android("uiautomator", "dump", path)
    return [node for node in ElementTree.fromstring(adb("exec-out", "cat", path)).iter("node")
            if node.get("package") == APP]


def find_node(nodes, label):
    labels = (label,) if isinstance(label, str) else tuple(label)
    return next((node for node in nodes
                 if (node.get("text") in labels or node.get("content-desc") in labels) and node.get("bounds")), None)


def wait_for(label, timeout=15):
    until = time.monotonic() + timeout
    while True:
        nodes = ui()
        if find_node(nodes, label) is not None:
            return nodes
        require(time.monotonic() < until, f"Could not find {label!r} in the Bareboot UI")
        time.sleep(1)


def tap_label(nodes, label):
    match = find_node(nodes, label)
    require(match is not None, f"Could not find {label!r} in the Bareboot UI")
    x, y, x2, y2 = map(int, re.findall(r"\d+", match.get("bounds")))
    terminal_input("tap", str((x + x2)//2), str((y + y2)//2))


def focus_terminal():
    width, height = map(int, re.findall(r"(\d+)x(\d+)", android("wm", "size"))[-1])
    terminal_input("tap", str(width//2), str(height//3))
    ui()


def enter(line):
    terminal_input("text", line.replace(" ", "%s"))
    terminal_input("keyevent", "KEYCODE_ENTER")


NEW_SHELL = ("New shell", "Новая оболочка")
SETTINGS = ("Settings", "Настройки")
OPEN_TERMINAL = ("Open terminal", "Открыть терминал")


def open_app():
    android("am", "start", "-W", "-n", f"{APP}/.MainActivity")
    nodes = ui()
    require(any(find_node(nodes, label) is not None for label in (NEW_SHELL, SETTINGS)),
            "Bareboot is not visible on the unlocked phone, or its setup is not finished")
    return nodes


def check_reattachment(runs):
    prefix = f"/tmp/{NAME}"
    sid = pid = None
    try:
        nodes = open_app()
        if find_node(nodes, NEW_SHELL) is None:
            tap_label(nodes, OPEN_TERMINAL)
            nodes = wait_for(NEW_SHELL)
        before = session_ids()
        tap_label(nodes, NEW_SHELL)
        until = time.monotonic() + 15
        while not session_ids().keys() - before.keys() and time.monotonic() < until:
            time.sleep(1)
        added = session_ids().keys() - before.keys()
        require(len(added) == 1, "The app did not create one new tab")
        sid = next(iter(added))
        distro, pid, uid = session_ids()[sid]
        require(distro == "debian" and uid == 1000, "Select Debian and the default user in Bareboot")
        ns_line = root("grep", "^NSpid:", f"/proc/{pid}/status")
        guest_pid = int(ns_line.split()[-1])
        focus_terminal()
        enter(f"export BAREBOOT_SMOKE={NAME}; echo $$:$BAREBOOT_SMOKE >{prefix}-before; sleep 90")
        until = time.monotonic() + 5
        marker = ""
        while time.monotonic() < until:
            marker = ssh(runs, f"cat {prefix}-before 2>/dev/null || true")
            if marker:
                break
            time.sleep(0.3)
        require(marker == f"{guest_pid}:{NAME}", f"Could not start the probe in the new tab (got {marker!r})")
        child = re.fullmatch(r"(\d+)\s+sleep", ssh(runs, f"ps -o pid=,comm= --ppid {guest_pid}"))
        require(child is not None, "Foreground command did not start")
        require_foreground()
        android("am", "force-stop", APP)
        open_app()
        wait_for(NEW_SHELL)
        require(session_ids().get(sid) == (distro, pid, uid), "The original shell did not reattach")
        require(re.fullmatch(rf"{child[1]}\s+sleep", ssh(runs, f"ps -o pid=,comm= --ppid {guest_pid}")) is not None,
                "The foreground command did not survive force-stop")
        focus_terminal()
        terminal_input("keycombination", "113", "31")
        enter(f"echo $$:$BAREBOOT_SMOKE >{prefix}-after")
        require(ssh(runs, f"cat {prefix}-after") == marker, "Reopened tab has a different shell")
        require(ssh(runs, f"test ! -e /proc/{child[1]} && echo interrupted") == "interrupted",
                "Ctrl+C did not interrupt the reattached command")
    finally:
        if sid is not None and session_ids().get(sid, (None, None, None))[1] == pid:
            lin("kill", str(sid), "1")
        ssh(runs, f"rm -f {prefix}-before {prefix}-after")
    print("Pass: tab reattachment, foreground command, and Ctrl+C", flush=True)


def default_network():
    dump = android("dumpsys", "-t", "3", "connectivity")
    net = re.search(r"(?m)^\s*Active default network: (\d+)$", dump)
    if not net:
        return None, []
    line = next((s for s in dump.splitlines() if s.lstrip().startswith(f"NetworkAgentInfo{{network{{{net[1]}}}")), "")
    transport = re.search(r"Transports: ([A-Z_]+)", line)
    dns = re.search(r"DnsAddresses: \[([^]]*)\]", line)
    if not transport or not dns:
        return None, []
    servers = []
    for address in dns[1].split(","):
        try:
            address = ipaddress.ip_address(address.strip().lstrip("/").split("%")[0])
        except ValueError:
            continue
        if address not in servers:
            servers.append(address)
    return transport[1], servers[:3]


def debian_isolated_net():
    for line in lin("images").splitlines():
        if line.startswith("debian "):
            return "isolated_net=true" in line
    return False


def check_transport(runs, transport, ipv4_only=False):
    until = time.monotonic() + 35
    while time.monotonic() < until:
        found, expected = default_network()
        if ipv4_only:
            expected = [a for a in expected if a.version == 4]
        if found == transport and expected:
            actual = [ipaddress.ip_address(a.split("%")[0]) for a in
                      ssh(runs, 'awk \'$1 == "nameserver" {print $2}\' /etc/resolv.conf').splitlines()]
            if actual == expected:
                ssh(runs, "timeout 7 getent ahostsv4 deb.debian.org >/dev/null && "
                          "curl -4 -fsSI --connect-timeout 4 --max-time 7 https://deb.debian.org/debian/ >/dev/null")
                return actual
        time.sleep(1)
    raise RuntimeError(f"Debian DNS did not match Android on {transport}")


def check_dns(runs):
    require(android("settings", "get", "global", "wifi_on") == "1" and
            android("settings", "get", "global", "mobile_data") == "1", "Enable Wi-Fi and mobile data before the smoke test")
    ipv4_only = debian_isolated_net()
    wifi = check_transport(runs, "WIFI", ipv4_only=ipv4_only)
    try:
        android("svc", "wifi", "disable")
        cellular = check_transport(runs, "CELLULAR", ipv4_only=ipv4_only)
    finally:
        android("svc", "wifi", "enable")
    require(check_transport(runs, "WIFI", ipv4_only=ipv4_only) == wifi, "Original Wi-Fi DNS did not return")
    require(cellular != wifi, "The networks use the same DNS servers; this test needs different addresses")
    print("Pass: DNS and HTTPS on Wi-Fi, cellular, then Wi-Fi", flush=True)


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--rootfs", type=Path, required=True)
    parser.add_argument("--ssh-config", type=Path, default=Path.home() / ".ssh/bareboot-usb/config")
    parser.add_argument("--serial", help="adb serial of the test device instead of the only USB device")
    args = parser.parse_args()
    if args.serial:
        ADB[1:] = ["-s", args.serial]
    require(args.rootfs.is_file() and args.rootfs.name.endswith((".tar.xz", ".tar.gz")), "Pass a trusted Debian ARM64 rootfs archive")
    require(Path("dist/bareboot-aarch64-musl").is_file(), "Run ./build.sh from the repository root")
    require(adb("get-state") == "device" and android("getenforce") == "Enforcing", "Use a USB test device with SELinux Enforcing")
    require("proto=18" in lin("ping"), "Install the current daemon before running the smoke test")
    runs = ssh_config(args.ssh_config)
    android("mkdir", STAGE)
    try:
        check_ipc_and_image(args.rootfs)
        check_files(runs)
        check_reattachment(runs)
        check_dns(runs)
    finally:
        android("rm", "-r", STAGE)
    print("Device smoke test passed", flush=True)


if __name__ == "__main__":
    try:
        main()
    except (OSError, RuntimeError, subprocess.SubprocessError) as e:
        raise SystemExit(str(e))
