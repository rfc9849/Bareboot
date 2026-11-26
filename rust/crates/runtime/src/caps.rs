use std::io;

const CAP_SYS_MODULE: libc::c_ulong = 16;
const CAP_SYS_RAWIO: libc::c_ulong = 17;
const CAP_SYS_PTRACE: libc::c_ulong = 19;
const CAP_SYS_ADMIN: libc::c_ulong = 21;
const CAP_SYS_BOOT: libc::c_ulong = 22;
const CAP_SYS_TIME: libc::c_ulong = 25;
const CAP_MAC_OVERRIDE: libc::c_ulong = 32;
const CAP_MAC_ADMIN: libc::c_ulong = 33;
const CAP_SYSLOG: libc::c_ulong = 34;
const CAP_PERFMON: libc::c_ulong = 38;
const CAP_BPF: libc::c_ulong = 39;
const CAP_CHECKPOINT_RESTORE: libc::c_ulong = 40;

const BLOCKED_CAPS: &[libc::c_ulong] = &[
    CAP_SYS_MODULE,
    CAP_SYS_RAWIO,
    CAP_SYS_PTRACE,
    CAP_SYS_ADMIN,
    CAP_SYS_BOOT,
    CAP_SYS_TIME,
    CAP_MAC_OVERRIDE,
    CAP_MAC_ADMIN,
    CAP_SYSLOG,
    CAP_PERFMON,
    CAP_BPF,
    CAP_CHECKPOINT_RESTORE,
];

#[repr(C)]
struct CapabilityHeader {
    version: u32,
    pid: i32,
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct CapabilityWords {
    effective: u32,
    permitted: u32,
    inheritable: u32,
}

const INIT_BLOCKED_CAPS: &[libc::c_ulong] = &[
    CAP_SYS_MODULE,
    CAP_SYS_RAWIO,
    CAP_SYS_TIME,
    CAP_MAC_OVERRIDE,
    CAP_MAC_ADMIN,
    CAP_SYSLOG,
];

pub fn restrict_guest_capabilities() -> io::Result<()> {
    drop_capabilities(BLOCKED_CAPS)
}

pub fn restrict_init_capabilities() -> io::Result<()> {
    drop_capabilities(INIT_BLOCKED_CAPS)
}

fn drop_capabilities(blocked: &[libc::c_ulong]) -> io::Result<()> {
    let mut header = CapabilityHeader {
        version: 0x2008_0522,
        pid: 0,
    };
    let mut words = [CapabilityWords::default(); 2];
    if unsafe { libc::syscall(libc::SYS_capget, &mut header, words.as_mut_ptr()) } < 0 {
        return Err(io::Error::last_os_error());
    }
    for &cap in blocked {
        if unsafe { libc::prctl(libc::PR_CAPBSET_DROP, cap, 0, 0, 0) } < 0 {
            return Err(io::Error::last_os_error());
        }
        let word = &mut words[cap as usize / 32];
        let keep = !(1u32 << (cap % 32));
        word.effective &= keep;
        word.permitted &= keep;
        word.inheritable &= keep;
    }
    if unsafe { libc::syscall(libc::SYS_capset, &header, words.as_ptr()) } < 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

pub fn switch_credentials(uid: u32, gid: u32, extra_groups: &[u32]) -> io::Result<()> {
    if uid == 0 && gid == 0 {
        return Ok(());
    }
    let mut groups: Vec<libc::gid_t> = vec![gid];
    for &g in extra_groups {
        if !groups.contains(&g) && groups.len() < 32 {
            groups.push(g);
        }
    }
    if unsafe { libc::setgroups(groups.len(), groups.as_ptr()) } != 0 {
        return Err(io::Error::last_os_error());
    }
    if unsafe { libc::setgid(gid) } != 0 {
        return Err(io::Error::last_os_error());
    }
    if unsafe { libc::setuid(uid) } != 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}
