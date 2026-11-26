use std::io;
use std::path::Path;

#[repr(C)]
struct SchedAttr {
    base: libc::sched_attr,
    util_min: u32,
    util_max: u32,
}

impl SchedAttr {
    fn new() -> Self {
        let mut attr: Self = unsafe { std::mem::zeroed() };
        attr.base.size = size_of::<Self>() as u32;
        attr
    }
}

pub fn foreground_util_min() -> u32 {
    let capacities: Vec<u32> = std::fs::read_dir("/sys/devices/system/cpu")
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|entry| {
            std::fs::read_to_string(entry.path().join("cpu_capacity"))
                .ok()?
                .trim()
                .parse()
                .ok()
        })
        .collect();
    let Some(&top) = capacities.iter().max() else {
        return 1024;
    };
    let Some(&second) = capacities.iter().filter(|&&c| c < top).max() else {
        return 1024;
    };
    let cluster = capacities
        .iter()
        .copied()
        .filter(|&c| c < top && c * 100 >= second * 95)
        .min()
        .unwrap_or(second);
    cluster * 97 / 100
}

pub fn set_cpu_util_min(min: u32) -> io::Result<()> {
    set_task_min(0, min)
}

fn set_task_min(tid: i32, min: u32) -> io::Result<()> {
    let mut attr = SchedAttr::new();
    attr.base.sched_flags = (libc::SCHED_FLAG_KEEP_ALL | libc::SCHED_FLAG_UTIL_CLAMP_MIN) as u64;
    attr.util_min = min;
    if unsafe { libc::syscall(libc::SYS_sched_setattr, tid, &attr, 0) } == 0 {
        Ok(())
    } else {
        Err(io::Error::last_os_error())
    }
}

fn task_attr(tid: i32) -> io::Result<SchedAttr> {
    let mut attr = SchedAttr::new();
    let size = attr.base.size;
    if unsafe { libc::syscall(libc::SYS_sched_getattr, tid, &mut attr, size, 0) } == 0 {
        Ok(attr)
    } else {
        Err(io::Error::last_os_error())
    }
}

pub(crate) fn current_cpu_min() -> io::Result<u32> {
    Ok(task_attr(0)?.util_min)
}

pub(crate) fn sync_task_min(tid: i32, previous: u32, min: u32) -> io::Result<()> {
    let result = task_attr(tid).and_then(|attr| {
        if attr.util_min == previous.min(attr.util_max) {
            set_task_min(tid, min.min(attr.util_max))
        } else {
            Ok(())
        }
    });
    match result {
        Err(e) if e.raw_os_error() == Some(libc::ESRCH) => Ok(()),
        result => result,
    }
}

pub(crate) fn sync_guest_min(previous: u32, min: u32) -> io::Result<()> {
    if unsafe { libc::getppid() } != 0 {
        return Err(io::Error::other(
            "CPU scheduling update needs the guest PID namespace",
        ));
    }
    let mut stat = std::mem::MaybeUninit::<libc::statfs>::uninit();
    if unsafe { libc::statfs(c"/proc".as_ptr(), stat.as_mut_ptr()) } != 0 {
        return Err(io::Error::last_os_error());
    }
    if unsafe { stat.assume_init() }.f_type != libc::PROC_SUPER_MAGIC as _ {
        return Err(io::Error::other("guest proc is not procfs"));
    }
    let proc_path = Path::new("/proc");
    let self_pid = unsafe { libc::getpid() }.to_string();
    if std::fs::read_link(proc_path.join("self"))? != Path::new(&self_pid) {
        return Err(io::Error::other(
            "proc does not show the guest PID namespace",
        ));
    }

    sync_task_min(1, previous, min)?;
    let mut first_error = None;
    for proc_entry in std::fs::read_dir(proc_path)? {
        let proc_entry = proc_entry?;
        if !proc_entry
            .file_name()
            .as_encoded_bytes()
            .iter()
            .all(u8::is_ascii_digit)
        {
            continue;
        }
        let tasks = match std::fs::read_dir(proc_entry.path().join("task")) {
            Ok(tasks) => tasks,
            Err(e) if e.kind() == io::ErrorKind::NotFound => continue,
            Err(e) => {
                first_error.get_or_insert(e);
                continue;
            }
        };
        for task in tasks.flatten() {
            let Some(tid) = task
                .file_name()
                .to_str()
                .and_then(|name| name.parse::<i32>().ok())
            else {
                continue;
            };
            if tid > 1
                && let Err(e) = sync_task_min(tid, previous, min)
            {
                first_error.get_or_insert(e);
            }
        }
    }
    first_error.map_or(Ok(()), Err)
}


const CPUCTL: &str = "/dev/cpuctl";
const VENDOR_TOP_APP: &str = "/proc/vendor_sched/groups/ta/set_proc_group";

pub struct CpuGroup {
    dir: std::path::PathBuf,
}

impl CpuGroup {
    pub fn create(name: &str) -> io::Result<Option<Self>> {
        crate::validate::validate_distro_name(name)?;
        let root = Path::new(CPUCTL);
        if !root.join("top-app").join("cpu.uclamp.min").exists() {
            return Ok(None);
        }
        let dir = root.join(format!("bareboot-{name}"));
        match std::fs::create_dir(&dir) {
            Ok(()) => {}
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {}
            Err(e) => return Err(e),
        }
        let group = Self { dir };
        group.set_min(0)?;
        let _ = std::fs::write(group.dir.join("cpu.uclamp.latency_sensitive"), "1");
        Ok(Some(group))
    }

    pub fn procs(&self) -> std::path::PathBuf {
        self.dir.join("cgroup.procs")
    }

    pub fn set_min(&self, percent: u32) -> io::Result<()> {
        std::fs::write(self.dir.join("cpu.uclamp.min"), percent.min(100).to_string())
    }

    pub fn min_file(&self) -> io::Result<std::fs::File> {
        std::fs::OpenOptions::new().write(true).open(self.dir.join("cpu.uclamp.min"))
    }
}

impl Drop for CpuGroup {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir(&self.dir);
    }
}

pub fn enter_cpu_group(procs: &Path) -> io::Result<()> {
    let pid = unsafe { libc::getpid() }.to_string();
    std::fs::write(procs, &pid)?;
    if Path::new(VENDOR_TOP_APP).exists() {
        let _ = std::fs::write(VENDOR_TOP_APP, &pid);
    }
    Ok(())
}
