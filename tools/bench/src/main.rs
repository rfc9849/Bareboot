use std::env;
use std::ffi::c_void;
use std::fs::{self, File, OpenOptions};
use std::hint::black_box;
use std::io::{Read, Write};
use std::os::unix::io::AsRawFd;
use std::path::{Path, PathBuf};
use std::process::{Command, exit};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Barrier};
use std::thread;
use std::time::{Duration, Instant};

unsafe extern "C" {
    fn getppid() -> i32;
    fn pipe(fds: *mut i32) -> i32;
    fn fork() -> i32;
    fn read(fd: i32, buf: *mut c_void, count: usize) -> isize;
    fn write(fd: i32, buf: *const c_void, count: usize) -> isize;
    fn close(fd: i32) -> i32;
    fn _exit(status: i32) -> !;
    fn waitpid(pid: i32, status: *mut i32, options: i32) -> i32;
    fn pread(fd: i32, buf: *mut c_void, count: usize, offset: i64) -> isize;
    fn pwrite(fd: i32, buf: *const c_void, count: usize, offset: i64) -> isize;
    #[cfg(target_os = "linux")]
    fn statfs(path: *const std::ffi::c_char, buf: *mut u64) -> i32;
}

const TARGET_TRIAL_SECS: f64 = 0.30;

#[derive(Clone, Copy)]
enum Direction {
    Higher,
    Lower,
}

impl Direction {
    fn as_str(self) -> &'static str {
        match self {
            Direction::Higher => "higher",
            Direction::Lower => "lower",
        }
    }
}

struct BenchResult {
    id: &'static str,
    name: &'static str,
    unit: &'static str,
    better: Direction,
    median: f64,
    min: f64,
    max: f64,
    samples: Vec<f64>,
}

struct Config {
    label: String,
    dir: PathBuf,
    trials: usize,
    json: bool,
    out: Option<PathBuf>,
    filter: Option<String>,
}

fn parse_args() -> Config {
    let mut args = env::args().skip(1);
    let mut cfg = Config {
        label: String::from("unknown"),
        dir: env::current_dir().unwrap_or_else(|_| PathBuf::from(".")),
        trials: 5,
        json: false,
        out: None,
        filter: None,
    };
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--noop" => exit(0),
            "--json" => cfg.json = true,
            "--out" => {
                let p = args.next().unwrap_or_else(|| usage_exit("--out needs a path"));
                cfg.out = Some(PathBuf::from(p));
            }
            "--label" => {
                cfg.label = args.next().unwrap_or_else(|| usage_exit("--label needs a value"));
            }
            "--dir" => {
                let p = args.next().unwrap_or_else(|| usage_exit("--dir needs a path"));
                cfg.dir = PathBuf::from(p);
            }
            "--trials" => {
                let n = args.next().unwrap_or_else(|| usage_exit("--trials needs a number"));
                cfg.trials = n.parse().unwrap_or_else(|_| usage_exit("invalid --trials"));
                if cfg.trials == 0 {
                    usage_exit("--trials must be >= 1");
                }
            }
            "--only" => {
                cfg.filter = Some(args.next().unwrap_or_else(|| usage_exit("--only needs a filter")));
            }
            "-h" | "--help" => {
                eprintln!(
                    "Usage: bareboot-bench [--label NAME] [--dir PATH] [--trials N] [--json] [--out FILE] [--only SUBSTR]"
                );
                exit(0);
            }
            other => usage_exit(&format!("unknown argument: {other}")),
        }
    }
    cfg
}

fn usage_exit(msg: &str) -> ! {
    eprintln!("error: {msg}");
    exit(2);
}

fn summarize(
    id: &'static str,
    name: &'static str,
    unit: &'static str,
    better: Direction,
    mut samples: Vec<f64>,
) -> BenchResult {
    let raw = samples.clone();
    samples.sort_by(|a, b| a.total_cmp(b));
    let mid = samples.len() / 2;
    let median = if samples.len() % 2 == 1 {
        samples[mid]
    } else {
        0.5 * (samples[mid - 1] + samples[mid])
    };
    BenchResult {
        id,
        name,
        unit,
        better,
        median,
        min: samples[0],
        max: *samples.last().unwrap(),
        samples: raw,
    }
}

fn calibrate<F>(mut run_n: F, init_n: usize, min_n: usize, max_n: usize) -> usize
where
    F: FnMut(usize) -> Duration,
{
    let mut n = init_n.clamp(min_n, max_n);
    for _ in 0..4 {
        let dt = run_n(n).as_secs_f64();
        if dt >= 0.08 {
            let scaled = ((n as f64) * (TARGET_TRIAL_SECS / dt)).round() as usize;
            return scaled.clamp(min_n, max_n);
        }
        let next = n.saturating_mul(4).min(max_n);
        if next == n {
            break;
        }
        n = next;
    }
    n
}

fn xorshift64(state: &mut u64) -> u64 {
    let mut x = *state;
    x ^= x << 13;
    x ^= x >> 7;
    x ^= x << 17;
    *state = x;
    x
}

// --- Pure Rust FIPS 180-4 SHA-256 ---

const SHA256_K: [u32; 64] = [
    0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1, 0x923f82a4, 0xab1c5ed5,
    0xd807aa98, 0x12835b01, 0x243185be, 0x550c7dc3, 0x72be5d74, 0x80deb1fe, 0x9bdc06a7, 0xc19bf174,
    0xe49b69c1, 0xefbe4786, 0x0fc19dc6, 0x240ca1cc, 0x2de92c6f, 0x4a7484aa, 0x5cb0a9dc, 0x76f988da,
    0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7, 0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967,
    0x27b70a85, 0x2e1b2138, 0x4d2c6dfc, 0x53380d13, 0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85,
    0xa2bfe8a1, 0xa81a664b, 0xc24b8b70, 0xc76c51a3, 0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070,
    0x19a4c116, 0x1e376c08, 0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a, 0x5b9cca4f, 0x682e6ff3,
    0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208, 0x90befffa, 0xa4506ceb, 0xbef9a3f7, 0xc67178f2,
];

fn sha256_compress(state: &mut [u32; 8], block: &[u8; 64]) {
    let mut w = [0u32; 64];
    for i in 0..16 {
        w[i] = u32::from_be_bytes([
            block[i * 4],
            block[i * 4 + 1],
            block[i * 4 + 2],
            block[i * 4 + 3],
        ]);
    }
    for i in 16..64 {
        let s0 = w[i - 15].rotate_right(7) ^ w[i - 15].rotate_right(18) ^ (w[i - 15] >> 3);
        let s1 = w[i - 2].rotate_right(17) ^ w[i - 2].rotate_right(19) ^ (w[i - 2] >> 10);
        w[i] = w[i - 16].wrapping_add(s0).wrapping_add(w[i - 7]).wrapping_add(s1);
    }

    let [mut a, mut b, mut c, mut d, mut e, mut f, mut g, mut h] = *state;
    for i in 0..64 {
        let s1 = e.rotate_right(6) ^ e.rotate_right(11) ^ e.rotate_right(25);
        let ch = (e & f) ^ ((!e) & g);
        let t1 = h
            .wrapping_add(s1)
            .wrapping_add(ch)
            .wrapping_add(SHA256_K[i])
            .wrapping_add(w[i]);
        let s0 = a.rotate_right(2) ^ a.rotate_right(13) ^ a.rotate_right(22);
        let maj = (a & b) ^ (a & c) ^ (b & c);
        let t2 = s0.wrapping_add(maj);

        h = g;
        g = f;
        f = e;
        e = d.wrapping_add(t1);
        d = c;
        c = b;
        b = a;
        a = t1.wrapping_add(t2);
    }

    state[0] = state[0].wrapping_add(a);
    state[1] = state[1].wrapping_add(b);
    state[2] = state[2].wrapping_add(c);
    state[3] = state[3].wrapping_add(d);
    state[4] = state[4].wrapping_add(e);
    state[5] = state[5].wrapping_add(f);
    state[6] = state[6].wrapping_add(g);
    state[7] = state[7].wrapping_add(h);
}

fn sha256(data: &[u8]) -> [u8; 32] {
    let mut state: [u32; 8] = [
        0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a, 0x510e527f, 0x9b05688c, 0x1f83d9ab,
        0x5be0cd19,
    ];
    let (chunks, rem) = data.as_chunks::<64>();
    for block in chunks {
        sha256_compress(&mut state, block);
    }
    let mut tail = [0u8; 128];
    tail[..rem.len()].copy_from_slice(rem);
    tail[rem.len()] = 0x80;
    let bit_len = (data.len() as u64).wrapping_mul(8);
    let pad_len = if rem.len() < 56 { 64 } else { 128 };
    tail[pad_len - 8..pad_len].copy_from_slice(&bit_len.to_be_bytes());
    sha256_compress(&mut state, tail[..64].try_into().unwrap());
    if pad_len == 128 {
        sha256_compress(&mut state, tail[64..128].try_into().unwrap());
    }
    let mut out = [0u8; 32];
    for (i, word) in state.iter().enumerate() {
        out[i * 4..i * 4 + 4].copy_from_slice(&word.to_be_bytes());
    }
    out
}

fn make_buffer(bytes: usize) -> Vec<u8> {
    let mut buf = vec![0u8; bytes];
    let mut s = 0x9e37_79b9_7f4a_7c15u64;
    for chunk in buf.as_chunks_mut::<8>().0 {
        *chunk = xorshift64(&mut s).to_le_bytes();
    }
    buf
}

fn bench_cpu_sha256_1t(trials: usize) -> BenchResult {
    const CHUNK: usize = 2 * 1024 * 1024;
    let buf = make_buffer(CHUNK);
    let mut run = |iters: usize| -> Duration {
        let t0 = Instant::now();
        let mut acc = 0u8;
        for _ in 0..iters {
            let d = sha256(black_box(&buf));
            acc ^= d[0];
        }
        black_box(acc);
        t0.elapsed()
    };
    let iters = calibrate(&mut run, 4, 2, 256);
    let bytes = (CHUNK * iters) as f64;
    let mut samples = Vec::with_capacity(trials);
    for _ in 0..trials {
        let dt = run(iters).as_secs_f64();
        samples.push((bytes / (1024.0 * 1024.0)) / dt);
    }
    summarize(
        "cpu_sha256_1t",
        "SHA-256 (1 thread)",
        "MiB/s",
        Direction::Higher,
        samples,
    )
}

fn bench_cpu_sha256_mt(trials: usize, threads: usize) -> BenchResult {
    const CHUNK: usize = 256 * 1024;
    let buf = Arc::new(make_buffer(CHUNK));
    let mut run = |total_chunks: usize| -> Duration {
        let next = Arc::new(AtomicUsize::new(0));
        let barrier = Arc::new(Barrier::new(threads + 1));
        let done = Arc::new(Barrier::new(threads + 1));
        let mut handles = Vec::with_capacity(threads);
        for _ in 0..threads {
            let b = Arc::clone(&buf);
            let q = Arc::clone(&next);
            let start = Arc::clone(&barrier);
            let finish = Arc::clone(&done);
            handles.push(thread::spawn(move || {
                start.wait();
                let mut acc = 0u8;
                while q.fetch_add(1, Ordering::Relaxed) < total_chunks {
                    let d = sha256(black_box(&b));
                    acc ^= d[0];
                }
                black_box(acc);
                finish.wait();
            }));
        }
        barrier.wait();
        let t0 = Instant::now();
        done.wait();
        let elapsed = t0.elapsed();
        for h in handles {
            h.join().unwrap();
        }
        elapsed
    };
    let chunks = calibrate(&mut run, threads * 32, threads * 8, threads * 1024);
    let total_bytes = (CHUNK * chunks) as f64;
    let mut samples = Vec::with_capacity(trials);
    for _ in 0..trials {
        let dt = run(chunks).as_secs_f64();
        samples.push((total_bytes / (1024.0 * 1024.0)) / dt);
    }
    summarize(
        "cpu_sha256_mt",
        "SHA-256 (all threads)",
        "MiB/s",
        Direction::Higher,
        samples,
    )
}

fn bench_cpu_sort_1t(trials: usize) -> BenchResult {
    const N: usize = 250_000;
    let mut seed = 0x1234_5678_9abc_def0u64;
    let template: Vec<u64> = (0..N).map(|_| xorshift64(&mut seed)).collect();
    let mut work = vec![0u64; N];
    let mut run = |iters: usize| -> Duration {
        let mut total = Duration::ZERO;
        for _ in 0..iters {
            work.copy_from_slice(&template);
            let t0 = Instant::now();
            black_box(&mut work).sort_unstable();
            total += t0.elapsed();
            assert!(work[0] <= work[N - 1]);
        }
        total
    };
    let iters = calibrate(&mut run, 4, 2, 64);
    let elems = (N * iters) as f64;
    let mut samples = Vec::with_capacity(trials);
    for _ in 0..trials {
        let dt = run(iters).as_secs_f64();
        samples.push((elems / 1e6) / dt);
    }
    summarize(
        "cpu_sort_1t",
        "Integer sort (250k u64)",
        "M elem/s",
        Direction::Higher,
        samples,
    )
}

fn bench_cpu_matmul_1t(trials: usize) -> BenchResult {
    const N: usize = 256;
    let mut a = vec![0.0f64; N * N];
    let mut b = vec![0.0f64; N * N];
    let mut c = vec![0.0f64; N * N];
    for i in 0..N * N {
        a[i] = ((i % 97) as f64) * 0.01 + 0.5;
        b[i] = ((i % 89) as f64) * 0.01 + 0.25;
    }
    let mut run = |iters: usize| -> Duration {
        let t0 = Instant::now();
        for _ in 0..iters {
            c.fill(0.0);
            for i in 0..N {
                let a_row = &a[i * N..(i + 1) * N];
                let c_row = &mut c[i * N..(i + 1) * N];
                for k in 0..N {
                    let aik = a_row[k];
                    let b_row = &b[k * N..(k + 1) * N];
                    for j in 0..N {
                        c_row[j] += aik * b_row[j];
                    }
                }
            }
            black_box(c[0] + c[N * N - 1]);
        }
        t0.elapsed()
    };
    let iters = calibrate(&mut run, 4, 2, 256);
    let flops = 2.0 * (N as f64).powi(3) * (iters as f64);
    let mut samples = Vec::with_capacity(trials);
    for _ in 0..trials {
        let dt = run(iters).as_secs_f64();
        samples.push((flops / 1e9) / dt);
    }
    summarize(
        "cpu_matmul_1t",
        "FP64 matmul (256x256)",
        "GFLOP/s",
        Direction::Higher,
        samples,
    )
}

fn bench_mem_copy_bw(trials: usize) -> BenchResult {
    const SIZE: usize = 64 * 1024 * 1024;
    let src = make_buffer(SIZE);
    let mut dst = vec![0u8; SIZE];
    dst.copy_from_slice(&src);
    let mut run = |iters: usize| -> Duration {
        let t0 = Instant::now();
        for i in 0..iters {
            dst.copy_from_slice(black_box(&src));
            black_box(dst[i & 4095]);
        }
        t0.elapsed()
    };
    let iters = calibrate(&mut run, 4, 2, 256);
    let bytes_rw = 2.0 * (SIZE as f64) * (iters as f64);
    let mut samples = Vec::with_capacity(trials);
    for _ in 0..trials {
        let dt = run(iters).as_secs_f64();
        samples.push((bytes_rw / (1024.0 * 1024.0 * 1024.0)) / dt);
    }
    summarize(
        "mem_copy_bw",
        "Memory copy (64 MiB)",
        "GiB/s",
        Direction::Higher,
        samples,
    )
}

fn build_cycle(lines: usize, stride: usize) -> Vec<usize> {
    let mut perm: Vec<usize> = (0..lines).collect();
    let mut rng = 0xfeed_face_cafe_beefu64;
    for i in (1..lines).rev() {
        let j = (xorshift64(&mut rng) as usize) % (i + 1);
        perm.swap(i, j);
    }
    let mut buf = vec![0usize; lines * stride];
    for i in 0..lines {
        buf[perm[i] * stride] = perm[(i + 1) % lines] * stride;
    }
    buf
}

fn bench_mem_ptr_chase(trials: usize) -> BenchResult {
    const BYTES: usize = 64 * 1024 * 1024;
    const STRIDE: usize = 64 / std::mem::size_of::<usize>();
    const LINES: usize = BYTES / 64;
    let buf = build_cycle(LINES, STRIDE);

    const BATCH: usize = 250_000;
    let mut run = |batches: usize| -> Duration {
        let mut idx = 0usize;
        let t0 = Instant::now();
        for _ in 0..batches {
            for _ in 0..(BATCH / 8) {
                idx = unsafe { *buf.get_unchecked(idx) };
                idx = unsafe { *buf.get_unchecked(idx) };
                idx = unsafe { *buf.get_unchecked(idx) };
                idx = unsafe { *buf.get_unchecked(idx) };
                idx = unsafe { *buf.get_unchecked(idx) };
                idx = unsafe { *buf.get_unchecked(idx) };
                idx = unsafe { *buf.get_unchecked(idx) };
                idx = unsafe { *buf.get_unchecked(idx) };
            }
        }
        let dt = t0.elapsed();
        black_box(idx);
        dt
    };
    let batches = calibrate(&mut run, 4, 2, 128);
    let hops = (batches * BATCH) as f64;
    let mut samples = Vec::with_capacity(trials);
    for _ in 0..trials {
        let dt = run(batches).as_secs_f64();
        samples.push((dt * 1e9) / hops);
    }
    summarize(
        "mem_ptr_chase",
        "Pointer chase (64 MiB)",
        "ns/load",
        Direction::Lower,
        samples,
    )
}

fn bench_syscall_getppid(trials: usize) -> BenchResult {
    const BATCH: usize = 16_384;
    let mut run = |batches: usize| -> Duration {
        let t0 = Instant::now();
        let mut acc = 0i32;
        for _ in 0..batches {
            for _ in 0..(BATCH / 8) {
                unsafe {
                    acc ^= getppid();
                    acc ^= getppid();
                    acc ^= getppid();
                    acc ^= getppid();
                    acc ^= getppid();
                    acc ^= getppid();
                    acc ^= getppid();
                    acc ^= getppid();
                }
            }
        }
        let dt = t0.elapsed();
        black_box(acc);
        dt
    };
    let batches = calibrate(&mut run, 8, 1, 2048);
    let calls = (batches * BATCH) as f64;
    let mut samples = Vec::with_capacity(trials);
    for _ in 0..trials {
        let dt = run(batches).as_secs_f64();
        samples.push((dt * 1e9) / calls);
    }
    summarize(
        "syscall_getppid",
        "Syscall (getppid)",
        "ns/call",
        Direction::Lower,
        samples,
    )
}

fn bench_proc_spawn(trials: usize) -> BenchResult {
    let exe = env::current_exe().unwrap_or_else(|_| PathBuf::from(env::args().next().unwrap()));
    let mut run = |spawns: usize| -> Duration {
        let t0 = Instant::now();
        for _ in 0..spawns {
            let st = Command::new(&exe)
                .arg("--noop")
                .status()
                .expect("failed to spawn self");
            assert!(st.success());
        }
        t0.elapsed()
    };
    let spawns = calibrate(&mut run, 10, 5, 400);
    let mut samples = Vec::with_capacity(trials);
    for _ in 0..trials {
        let dt = run(spawns).as_secs_f64();
        samples.push((dt * 1e3) / (spawns as f64));
    }
    summarize(
        "proc_spawn",
        "Process spawn (exec)",
        "ms/spawn",
        Direction::Lower,
        samples,
    )
}

fn bench_ipc_pipe_rtt(trials: usize) -> BenchResult {
    let mut p2c = [0i32; 2];
    let mut c2p = [0i32; 2];
    unsafe {
        assert_eq!(pipe(p2c.as_mut_ptr()), 0, "pipe p2c failed");
        assert_eq!(pipe(c2p.as_mut_ptr()), 0, "pipe c2p failed");
    }
    let pid = unsafe { fork() };
    assert!(pid >= 0, "fork failed");
    if pid == 0 {
        unsafe {
            close(p2c[1]);
            close(c2p[0]);
            let mut b = 0u8;
            loop {
                let n = read(p2c[0], (&mut b as *mut u8).cast::<c_void>(), 1);
                if n <= 0 || b == 0 {
                    break;
                }
                let w = write(c2p[1], (&b as *const u8).cast::<c_void>(), 1);
                if w != 1 {
                    break;
                }
            }
            close(p2c[0]);
            close(c2p[1]);
            _exit(0);
        }
    }
    unsafe {
        close(p2c[0]);
        close(c2p[1]);
    }
    let mut run = |rounds: usize| -> Duration {
        let tok = 1u8;
        let mut resp = 0u8;
        let t0 = Instant::now();
        for _ in 0..rounds {
            unsafe {
                assert_eq!(write(p2c[1], (&tok as *const u8).cast::<c_void>(), 1), 1);
                assert_eq!(read(c2p[0], (&mut resp as *mut u8).cast::<c_void>(), 1), 1);
            }
        }
        let dt = t0.elapsed();
        black_box(resp);
        dt
    };
    let rounds = calibrate(&mut run, 1_000, 200, 100_000);
    let mut samples = Vec::with_capacity(trials);
    for _ in 0..trials {
        let dt = run(rounds).as_secs_f64();
        samples.push((dt * 1e6) / (rounds as f64));
    }
    unsafe {
        let stop = 0u8;
        let _ = write(p2c[1], (&stop as *const u8).cast::<c_void>(), 1);
        close(p2c[1]);
        close(c2p[0]);
        let mut status = 0i32;
        waitpid(pid, &mut status as *mut i32, 0);
    }
    summarize(
        "ipc_pipe_rtt",
        "Pipe IPC ping-pong",
        "us/RTT",
        Direction::Lower,
        samples,
    )
}

fn bench_fs_small_files(trials: usize, base: &Path) -> BenchResult {
    const COUNT: usize = 1_000;
    let payload = make_buffer(4096);
    let mut read_buf = vec![0u8; 4096];
    let dir = base.join(format!(".bareboot-bench-small-{}", std::process::id()));

    let mut one_run = || -> Duration {
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).expect("create_dir_all failed");
        let names: Vec<PathBuf> = (0..COUNT).map(|i| dir.join(format!("f{i:04}"))).collect();

        let t0 = Instant::now();
        for p in &names {
            let mut f = File::create(p).expect("create failed");
            f.write_all(&payload).expect("write failed");
        }
        let mut meta_acc = 0u64;
        for p in &names {
            meta_acc = meta_acc.wrapping_add(fs::metadata(p).expect("stat failed").len());
        }
        let mut data_acc = 0u8;
        for p in &names {
            let mut f = File::open(p).expect("open failed");
            f.read_exact(&mut read_buf).expect("read failed");
            data_acc ^= read_buf[0];
        }
        for p in &names {
            fs::remove_file(p).expect("unlink failed");
        }
        let dt = t0.elapsed();
        black_box((meta_acc, data_acc));
        let _ = fs::remove_dir_all(&dir);
        dt
    };

    let _ = one_run();
    let mut samples = Vec::with_capacity(trials);
    for _ in 0..trials {
        let dt = one_run().as_secs_f64();
        samples.push((COUNT as f64) / dt);
    }
    summarize(
        "fs_small_files",
        "Small files (1000x4K CRUD)",
        "files/s",
        Direction::Higher,
        samples,
    )
}

fn bench_fs_seq_write_sync(trials: usize, base: &Path) -> BenchResult {
    const TOTAL_MIB: usize = 64;
    const CHUNK: usize = 1024 * 1024;
    let chunk = make_buffer(CHUNK);
    let path = base.join(format!(".bareboot-bench-seq-{}", std::process::id()));

    let one_run = || -> Duration {
        let _ = fs::remove_file(&path);
        let t0 = Instant::now();
        {
            let mut f = File::create(&path).expect("create seq file failed");
            for _ in 0..TOTAL_MIB {
                f.write_all(&chunk).expect("seq write failed");
            }
            f.sync_all().expect("fsync failed");
        }
        let dt = t0.elapsed();
        let _ = fs::remove_file(&path);
        dt
    };

    let _ = one_run();
    let mut samples = Vec::with_capacity(trials);
    for _ in 0..trials {
        let dt = one_run().as_secs_f64();
        samples.push((TOTAL_MIB as f64) / dt);
    }
    summarize(
        "fs_seq_write_sync",
        "Seq write + fsync (64 MiB)",
        "MiB/s",
        Direction::Higher,
        samples,
    )
}

fn bench_fs_rand_4k(trials: usize, base: &Path) -> BenchResult {
    const FILE_SIZE: usize = 32 * 1024 * 1024;
    const BLOCK: usize = 4096;
    const BLOCKS: usize = FILE_SIZE / BLOCK;
    let path = base.join(format!(".bareboot-bench-rand-{}", std::process::id()));
    let _ = fs::remove_file(&path);

    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(true)
        .open(&path)
        .expect("open rand file failed");
    let init_chunk = make_buffer(1024 * 1024);
    {
        let mut f = &file;
        for _ in 0..(FILE_SIZE / init_chunk.len()) {
            f.write_all(&init_chunk).expect("init rand file failed");
        }
        file.sync_all().expect("sync rand file failed");
    }

    let fd = file.as_raw_fd();
    let mut buf = make_buffer(BLOCK);
    let mut run = |ops: usize| -> Duration {
        let mut rng = 0x55aa_33cc_9966_11eeu64;
        let mut acc = 0u8;
        let t0 = Instant::now();
        for i in 0..ops {
            let blk = (xorshift64(&mut rng) as usize) % BLOCKS;
            let off = (blk * BLOCK) as i64;
            if (i & 3) == 0 {
                let n = unsafe { pwrite(fd, buf.as_ptr().cast::<c_void>(), BLOCK, off) };
                assert_eq!(n, BLOCK as isize, "pwrite failed");
            } else {
                let n = unsafe { pread(fd, buf.as_mut_ptr().cast::<c_void>(), BLOCK, off) };
                assert_eq!(n, BLOCK as isize, "pread failed");
                acc ^= buf[0];
            }
        }
        let dt = t0.elapsed();
        black_box(acc);
        dt
    };

    let ops = calibrate(&mut run, 4_000, 1_000, 200_000);
    let mut samples = Vec::with_capacity(trials);
    for _ in 0..trials {
        let dt = run(ops).as_secs_f64();
        samples.push((ops as f64 / 1000.0) / dt);
    }
    drop(file);
    let _ = fs::remove_file(&path);
    summarize(
        "fs_rand_4k",
        "Random 4K I/O (75R/25W)",
        "kIOPS",
        Direction::Higher,
        samples,
    )
}

fn read_cpus_allowed() -> String {
    if let Ok(text) = fs::read_to_string("/proc/self/status") {
        for line in text.lines() {
            if let Some(rest) = line.strip_prefix("Cpus_allowed_list:") {
                return rest.trim().to_string();
            }
        }
    }
    String::from("unknown")
}

fn read_kernel_release() -> String {
    if let Ok(text) = fs::read_to_string("/proc/sys/kernel/osrelease") {
        return text.trim().to_string();
    }
    String::from("unknown")
}

#[cfg(target_os = "linux")]
fn detect_fs_type(dir: &Path) -> String {
    use std::ffi::CString;
    let c = match CString::new(dir.as_os_str().as_encoded_bytes()) {
        Ok(c) => c,
        Err(_) => return String::from("unknown"),
    };
    let mut buf = [0u64; 32];
    let rc = unsafe { statfs(c.as_ptr(), buf.as_mut_ptr()) };
    if rc != 0 {
        return String::from("unknown");
    }
    match buf[0] {
        0xef53 => String::from("ext4"),
        0xf2f5_2010 => String::from("f2fs"),
        0x0102_1994 => String::from("tmpfs"),
        0x794c_7630 => String::from("overlayfs"),
        0x6573_5546 => String::from("fuse"),
        magic => format!("0x{magic:x}"),
    }
}

#[cfg(not(target_os = "linux"))]
fn detect_fs_type(_dir: &Path) -> String {
    String::from(env::consts::OS)
}

fn json_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for ch in s.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

fn format_json(
    cfg: &Config,
    kernel: &str,
    cpus_allowed: &str,
    threads: usize,
    fs_type: &str,
    results: &[BenchResult],
) -> String {
    use std::fmt::Write as _;
    let mut s = String::new();
    let _ = writeln!(s, "{{");
    let _ = writeln!(s, "  \"label\": {},", json_escape(&cfg.label));
    let _ = writeln!(s, "  \"arch\": {},", json_escape(env::consts::ARCH));
    let _ = writeln!(s, "  \"kernel\": {},", json_escape(kernel));
    let _ = writeln!(s, "  \"cpus_allowed\": {},", json_escape(cpus_allowed));
    let _ = writeln!(s, "  \"threads\": {threads},");
    let _ = writeln!(s, "  \"dir\": {},", json_escape(&cfg.dir.display().to_string()));
    let _ = writeln!(s, "  \"fs_type\": {},", json_escape(fs_type));
    let _ = writeln!(s, "  \"trials\": {},", cfg.trials);
    let _ = writeln!(s, "  \"results\": [");
    for (i, r) in results.iter().enumerate() {
        let comma = if i + 1 < results.len() { "," } else { "" };
        let samples_str: Vec<String> = r.samples.iter().map(|v| format!("{v:.4}")).collect();
        let _ = writeln!(
            s,
            "    {{\"id\": {}, \"name\": {}, \"unit\": {}, \"better\": {}, \"median\": {:.4}, \"min\": {:.4}, \"max\": {:.4}, \"samples\": [{}]}}{comma}",
            json_escape(r.id),
            json_escape(r.name),
            json_escape(r.unit),
            json_escape(r.better.as_str()),
            r.median,
            r.min,
            r.max,
            samples_str.join(", "),
        );
    }
    let _ = writeln!(s, "  ]");
    let _ = writeln!(s, "}}");
    s
}

fn print_table(
    cfg: &Config,
    kernel: &str,
    cpus_allowed: &str,
    threads: usize,
    fs_type: &str,
    results: &[BenchResult],
) {
    println!(
        "label={} arch={} kernel={} cpus={} threads={} fs={} trials={}",
        cfg.label,
        env::consts::ARCH,
        kernel,
        cpus_allowed,
        threads,
        fs_type,
        cfg.trials
    );
    println!("{:<30} {:>12} {:>12} {:>12}  Unit", "Benchmark", "Median", "Min", "Max");
    println!("{}", "-".repeat(78));
    for r in results {
        println!(
            "{:<30} {:>12.2} {:>12.2} {:>12.2}  {}",
            r.name, r.median, r.min, r.max, r.unit
        );
    }
}

fn main() {
    if env::args().nth(1).as_deref() == Some("--noop") {
        exit(0);
    }
    let cfg = parse_args();
    let threads = thread::available_parallelism().map(|n| n.get()).unwrap_or(1);
    let kernel = read_kernel_release();
    let cpus_allowed = read_cpus_allowed();
    let fs_type = detect_fs_type(&cfg.dir);

    if fs_type == "tmpfs" {
        eprintln!(
            "warning: --dir {} is on tmpfs; pass a persistent directory for disk benchmarks",
            cfg.dir.display()
        );
    }

    let should_run = |id: &str| -> bool {
        match &cfg.filter {
            Some(f) => id.contains(f.as_str()),
            None => true,
        }
    };

    let mut results = Vec::new();
    let mut run_one = |id: &'static str, f: &dyn Fn() -> BenchResult| {
        if !should_run(id) {
            return;
        }
        eprint!("running {id}...");
        let r = f();
        eprintln!(" {:.2} {}", r.median, r.unit);
        results.push(r);
    };

    run_one("cpu_sha256_1t", &|| bench_cpu_sha256_1t(cfg.trials));
    run_one("cpu_sha256_mt", &|| bench_cpu_sha256_mt(cfg.trials, threads));
    run_one("cpu_sort_1t", &|| bench_cpu_sort_1t(cfg.trials));
    run_one("cpu_matmul_1t", &|| bench_cpu_matmul_1t(cfg.trials));
    run_one("mem_copy_bw", &|| bench_mem_copy_bw(cfg.trials));
    run_one("mem_ptr_chase", &|| bench_mem_ptr_chase(cfg.trials));
    run_one("syscall_getppid", &|| bench_syscall_getppid(cfg.trials));
    run_one("proc_spawn", &|| bench_proc_spawn(cfg.trials));
    run_one("ipc_pipe_rtt", &|| bench_ipc_pipe_rtt(cfg.trials));
    run_one("fs_small_files", &|| bench_fs_small_files(cfg.trials, &cfg.dir));
    run_one("fs_seq_write_sync", &|| bench_fs_seq_write_sync(cfg.trials, &cfg.dir));
    run_one("fs_rand_4k", &|| bench_fs_rand_4k(cfg.trials, &cfg.dir));

    if cfg.json || cfg.out.is_some() {
        let json = format_json(&cfg, &kernel, &cpus_allowed, threads, &fs_type, &results);
        if let Some(path) = &cfg.out {
            fs::write(path, &json).expect("failed to write --out file");
        }
        if cfg.json {
            print!("{json}");
        } else {
            print_table(&cfg, &kernel, &cpus_allowed, threads, &fs_type, &results);
        }
    } else {
        print_table(&cfg, &kernel, &cpus_allowed, threads, &fs_type, &results);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sha256_known_vector() {
        let d = sha256(b"abc");
        let hex: String = d.iter().map(|b| format!("{b:02x}")).collect();
        assert_eq!(
            hex,
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }

    #[test]
    fn sha256_empty_vector() {
        let d = sha256(b"");
        let hex: String = d.iter().map(|b| format!("{b:02x}")).collect();
        assert_eq!(
            hex,
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
    }

    #[test]
    fn ptr_chase_is_single_cycle() {
        let lines = 1024;
        let stride = 8;
        let buf = build_cycle(lines, stride);
        let mut idx = 0usize;
        for step in 0..lines {
            idx = buf[idx];
            if step + 1 < lines {
                assert_ne!(idx, 0, "cycle closed early at step {step}");
            }
        }
        assert_eq!(idx, 0);
    }
}
