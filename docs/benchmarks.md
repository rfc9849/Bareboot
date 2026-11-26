# Benchmarks

Comparison of Bareboot against native [Termux](https://github.com/termux/termux-app) as an Android kernel baseline.

Both ran the exact same statically linked `aarch64-unknown-linux-musl` binary ([`tools/bench`](../tools/bench)) on the same phone, eliminating compiler and C library differences.

## Test setup

- **Device:** Google Pixel 10 Pro (`blazer`), Tensor G5 (2x 2.25 GHz + 5x 3.05 GHz + 1x 3.78 GHz), 15 GiB RAM
- **OS / kernel:** Android 17 (API 37), Linux `6.12.81-4k-g002ec8901757`, 4 KiB pages, SELinux Enforcing
- **Bareboot 0.1.0:** Arch Linux ARM on `ext4` loop image (`dio=1`, `relaxed_sync=false`), `cpus=0-7`, `threads=8`
- **Termux 0.118.3:** foreground `top-app`, native Android `/data` `f2fs` (`inlinecrypt`, `lz4`), `cpus=0-7`, `threads=8`
- **Method:** warm-up pass followed by 5 measured trials per benchmark; median and min-max range reported. Disk tests ran in `$HOME` (persistent storage, not `tmpfs`). Termux stayed in the foreground (`top-app`, `cpus=0-7`) during measurement because Android restricts background apps to `cpus=0-2` and non-top foreground processes to `cpus=0-6`.

## Results

| Benchmark | Unit | Bareboot | Termux |
| --- | --- | ---: | ---: |
| SHA-256 (1 thread) | MiB/s (higher) | 303.7 (303.0-306.0) | 304.7 (303.3-305.9) |
| SHA-256 (all threads) | MiB/s (higher) | 1,500 (1,495-1,518) | 1,512 (1,481-1,515) |
| Integer sort (250k u64) | M elem/s (higher) | 94.78 (94.58-94.94) | 95.03 (94.74-95.26) |
| FP64 matmul (256x256) | GFLOP/s (higher) | 13.55 (13.45-13.68) | 13.71 (13.62-13.75) |
| Memory copy (64 MiB) | GiB/s (higher) | 25.21 (24.80-25.61) | 25.39 (24.79-25.70) |
| Pointer chase (64 MiB) | ns/load (lower) | 102.6 (101.1-107.0) | 96.95 (94.98-101.5) |
| Syscall (getppid) | ns/call (lower) | 91.15 (91.10-91.29) | 91.98 (91.95-92.02) |
| Process spawn (exec) | ms/spawn (lower) | 0.56 (0.55-0.69) | 2.21 (1.44-2.67) |
| Pipe IPC ping-pong | us/RTT (lower) | 2.92 (2.90-2.93) | 3.07 (3.07-3.08) |
| Small files (1000x4K CRUD) | files/s (higher) | 70,595 (69,148-72,277) | 47,248 (43,271-48,948) |
| Seq write + fsync (64 MiB) | MiB/s (higher) | 436.3 (398.6-635.5) | 100.8 (84.79-103.1) |
| Random 4K I/O (75R/25W) | kIOPS (higher) | 1,090 (1,085-1,094) | 1,084 (1,082-1,086) |

## Reading the results

- **Compute, memory, syscalls, IPC, and random 4K I/O:** Bareboot sits within 0.95x-1.05x of native Termux, showing no execution overhead over running directly on the Android kernel.
- **Process spawn:** Bareboot spawns processes in 0.56 ms vs 2.21 ms in Termux because Bareboot runs outside Android's `untrusted_app` seccomp and cgroup path with `cpu.uclamp.min=30`.
- **Storage:** Bareboot uses `ext4` on a direct-I/O loop device, while Termux writes directly to Android `/data` F2FS (`inlinecrypt`, `lz4`).

## Reproduce on your device

The benchmark tool has zero external Rust dependencies. Build the static ARM64 binary:

```sh
cargo build --release --manifest-path tools/bench/Cargo.toml --target aarch64-unknown-linux-musl
```

Binary output: `tools/bench/target/aarch64-unknown-linux-musl/release/bareboot-bench`.

Copy the same binary into each environment and run it from persistent storage (`$HOME`, not `/tmp`) while the target app stays in the foreground with the screen awake:

```sh
# Inside Bareboot
./bareboot-bench --label Bareboot --dir "$HOME" --trials 5 --out bareboot.json

# Inside Termux (keep Termux in the foreground)
./bareboot-bench --label Termux --dir "$HOME" --trials 5 --out termux.json
```

Check that both JSON files report the same `cpus_allowed` (for example `0-7`) and `threads`, then format a Markdown table:

```sh
python3 tools/bench/report.py bareboot.json termux.json
python3 tools/bench/report.py --ranges bareboot.json termux.json
```
