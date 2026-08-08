//! Storage-layer throughput: coven-afs versus the host filesystem.
//!
//! ```text
//! cargo run --release -p coven-afs --example afs_bench -- [--scale N] [--json <path>]
//! ```
//!
//! Answers the question RESEARCH.md flagged as gating the architecture: what
//! a repository-checkout-shaped and a `pnpm install`-shaped write workload
//! cost through SQLite chunks instead of the host filesystem, and what
//! full-file copy-up costs on first write to a base file.
//!
//! This measures the storage engine, not a mount. Numbers here are the floor
//! any mount backend inherits.

use std::io::Write as _;
use std::path::Path;
use std::time::{Duration, Instant};

use coven_afs::{AgentFs, OverlayFs};

/// Deterministic filler so runs are comparable. Content must not be constant:
/// SQLite and the host filesystem both handle incompressible bytes
/// differently from long runs of one byte.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }

    fn bytes(&mut self, len: usize) -> Vec<u8> {
        let mut out = vec![0_u8; len];
        for slot in out.iter_mut() {
            *slot = (self.next() & 0xff) as u8;
        }
        out
    }
}

/// One synthetic tree: paths plus the bytes each file holds.
fn tree(scale: usize, shape: Shape) -> Vec<(String, Vec<u8>)> {
    let mut rng = Rng(0x5eed_1234_abcd_0001);
    let mut files = Vec::with_capacity(scale);
    for index in 0..scale {
        let (path, size) = match shape {
            // Source-checkout shape: a few directory levels, mostly small
            // text-sized files, an occasional big one.
            Shape::Checkout => {
                let dir = index % 32;
                let sub = (index / 32) % 8;
                let size = if index % 97 == 0 {
                    256 * 1024
                } else {
                    1024 + (index % 7) * 1024
                };
                (format!("/src/mod{dir}/part{sub}/file{index}.rs"), size)
            }
            // Dependency-tree shape: very many very small files, deeply nested.
            Shape::Packages => {
                let pkg = index % 200;
                let size = 200 + (index % 5) * 150;
                (
                    format!("/node_modules/pkg{pkg}/dist/lib/unit{index}.js"),
                    size,
                )
            }
        };
        files.push((path, rng.bytes(size)));
    }
    files
}

#[derive(Clone, Copy)]
enum Shape {
    Checkout,
    Packages,
}

fn total_bytes(files: &[(String, Vec<u8>)]) -> usize {
    files.iter().map(|(_, data)| data.len()).sum()
}

fn millis(duration: Duration) -> f64 {
    (duration.as_secs_f64() * 1000.0 * 1000.0).round() / 1000.0
}

fn throughput(bytes: usize, duration: Duration) -> f64 {
    let mb = bytes as f64 / (1024.0 * 1024.0);
    ((mb / duration.as_secs_f64()) * 100.0).round() / 100.0
}

// ---- host filesystem ----------------------------------------------------

fn host_write(root: &Path, files: &[(String, Vec<u8>)]) -> Duration {
    let start = Instant::now();
    for (path, data) in files {
        let full = root.join(path.trim_start_matches('/'));
        std::fs::create_dir_all(full.parent().unwrap()).unwrap();
        let mut handle = std::fs::File::create(&full).unwrap();
        handle.write_all(data).unwrap();
    }
    start.elapsed()
}

fn host_read(root: &Path, files: &[(String, Vec<u8>)]) -> Duration {
    let start = Instant::now();
    for (path, _) in files {
        let full = root.join(path.trim_start_matches('/'));
        let data = std::fs::read(&full).unwrap();
        std::hint::black_box(data.len());
    }
    start.elapsed()
}

fn host_stat(root: &Path, files: &[(String, Vec<u8>)]) -> Duration {
    let start = Instant::now();
    for (path, _) in files {
        let full = root.join(path.trim_start_matches('/'));
        std::hint::black_box(std::fs::metadata(&full).unwrap().len());
    }
    start.elapsed()
}

// ---- coven-afs ----------------------------------------------------------

fn afs_write(fs: &mut AgentFs, files: &[(String, Vec<u8>)]) -> Duration {
    let start = Instant::now();
    for (path, data) in files {
        fs.write_file(path, data).unwrap();
    }
    start.elapsed()
}

fn afs_read(fs: &AgentFs, files: &[(String, Vec<u8>)]) -> Duration {
    let start = Instant::now();
    for (path, _) in files {
        std::hint::black_box(fs.read_file(path).unwrap().len());
    }
    start.elapsed()
}

fn afs_stat(fs: &AgentFs, files: &[(String, Vec<u8>)]) -> Duration {
    let start = Instant::now();
    for (path, _) in files {
        std::hint::black_box(fs.stat(path).unwrap().size);
    }
    start.elapsed()
}

/// Chunked writes at increasing offsets, i.e. what a mount client actually
/// issues for one large file.
fn afs_chunked_write(fs: &mut AgentFs, path: &str, data: &[u8], chunk: usize) -> Duration {
    let ino = fs.write_file(path, &[]).unwrap();
    let start = Instant::now();
    for (index, piece) in data.chunks(chunk).enumerate() {
        fs.write_ino_at(ino, (index * chunk) as u64, piece).unwrap();
    }
    start.elapsed()
}

fn host_chunked_write(path: &Path, data: &[u8], chunk: usize) -> Duration {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    let mut handle = std::fs::File::create(path).unwrap();
    let start = Instant::now();
    for piece in data.chunks(chunk) {
        handle.write_all(piece).unwrap();
    }
    handle.flush().unwrap();
    start.elapsed()
}

fn main() {
    let mut scale = 2000_usize;
    let mut json_path: Option<String> = None;
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--scale" => scale = args.next().and_then(|v| v.parse().ok()).unwrap_or(scale),
            "--json" => json_path = args.next(),
            other => {
                eprintln!("unknown argument: {other}");
                std::process::exit(2);
            }
        }
    }

    let temp = tempfile::tempdir().unwrap();
    let mut report = Vec::new();

    for (label, shape) in [("checkout", Shape::Checkout), ("packages", Shape::Packages)] {
        let files = tree(scale, shape);
        let bytes = total_bytes(&files);

        let host_root = temp.path().join(format!("host-{label}"));
        std::fs::create_dir_all(&host_root).unwrap();
        let host_w = host_write(&host_root, &files);
        let host_r = host_read(&host_root, &files);
        let host_s = host_stat(&host_root, &files);

        let db = temp.path().join(format!("afs-{label}.db"));
        let mut fs = AgentFs::create(&db).unwrap();
        let afs_w = afs_write(&mut fs, &files);
        let afs_r = afs_read(&fs, &files);
        let afs_s = afs_stat(&fs, &files);
        drop(fs);
        let db_bytes = std::fs::metadata(&db).unwrap().len();

        // Same workload with write-ahead logging: the default rollback
        // journal syncs on every file, which the host path does not do.
        let wal_db = temp.path().join(format!("afs-wal-{label}.db"));
        let mut wal_fs = AgentFs::create(&wal_db).unwrap();
        wal_fs.enable_wal().unwrap();
        let afs_wal_w = afs_write(&mut wal_fs, &files);
        let afs_wal_r = afs_read(&wal_fs, &files);
        drop(wal_fs);

        println!(
            "{label}: {} files, {:.1} MiB payload",
            files.len(),
            bytes as f64 / (1024.0 * 1024.0)
        );
        println!(
            "  write  host {:>9.3} ms ({:>7.2} MiB/s)   afs {:>9.3} ms ({:>7.2} MiB/s)   {:.2}x",
            millis(host_w),
            throughput(bytes, host_w),
            millis(afs_w),
            throughput(bytes, afs_w),
            afs_w.as_secs_f64() / host_w.as_secs_f64()
        );
        println!(
            "  write(wal) {:>28} afs {:>9.3} ms ({:>7.2} MiB/s)   {:.2}x",
            "",
            millis(afs_wal_w),
            throughput(bytes, afs_wal_w),
            afs_wal_w.as_secs_f64() / host_w.as_secs_f64()
        );
        println!(
            "  read   host {:>9.3} ms ({:>7.2} MiB/s)   afs {:>9.3} ms ({:>7.2} MiB/s)   {:.2}x",
            millis(host_r),
            throughput(bytes, host_r),
            millis(afs_r),
            throughput(bytes, afs_r),
            afs_r.as_secs_f64() / host_r.as_secs_f64()
        );
        println!(
            "  stat   host {:>9.3} ms                  afs {:>9.3} ms                  {:.2}x",
            millis(host_s),
            millis(afs_s),
            afs_s.as_secs_f64() / host_s.as_secs_f64()
        );
        println!(
            "  on-disk payload {:.1} MiB, afs db {:.1} MiB ({:.2}x)",
            bytes as f64 / (1024.0 * 1024.0),
            db_bytes as f64 / (1024.0 * 1024.0),
            db_bytes as f64 / bytes as f64
        );

        report.push(format!(
            r#"    "{label}": {{
      "files": {}, "payloadBytes": {}, "afsDbBytes": {},
      "hostWriteMs": {}, "afsWriteMs": {}, "afsWalWriteMs": {}, "afsWalReadMs": {},
      "hostReadMs": {}, "afsReadMs": {},
      "hostStatMs": {}, "afsStatMs": {}
    }}"#,
            files.len(),
            bytes,
            db_bytes,
            millis(host_w),
            millis(afs_w),
            millis(afs_wal_w),
            millis(afs_wal_r),
            millis(host_r),
            millis(afs_r),
            millis(host_s),
            millis(afs_s),
        ));
    }

    // Large-file chunked write: what an NFS/FUSE client issues for one big
    // file, and the case where whole-file rewrite semantics would be fatal.
    let mut rng = Rng(0xfeed_0000_0000_0001);
    let big = rng.bytes(64 * 1024 * 1024);
    let chunk = 64 * 1024;
    let host_big = host_chunked_write(&temp.path().join("big/host.bin"), &big, chunk);
    let mut fs = AgentFs::create(temp.path().join("big.db")).unwrap();
    fs.enable_wal().unwrap();
    let afs_big = afs_chunked_write(&mut fs, "/big.bin", &big, chunk);
    drop(fs);
    println!(
        "large file (wal): 64 MiB in {} KiB writes — host {:.3} ms ({:.2} MiB/s), afs {:.3} ms ({:.2} MiB/s), {:.2}x",
        chunk / 1024,
        millis(host_big),
        throughput(big.len(), host_big),
        millis(afs_big),
        throughput(big.len(), afs_big),
        afs_big.as_secs_f64() / host_big.as_secs_f64()
    );
    report.push(format!(
        r#"    "largeFileChunkedWrite": {{
      "bytes": {}, "chunkBytes": {}, "hostMs": {}, "afsMs": {}
    }}"#,
        big.len(),
        chunk,
        millis(host_big),
        millis(afs_big)
    ));

    // Copy-up: first write to a file that lives in the read-only base.
    let base_path = temp.path().join("base.db");
    let mut base = AgentFs::create(&base_path).unwrap();
    let mut copy_up_rows = Vec::new();
    for size_mib in [1_usize, 8, 32] {
        let payload = rng.bytes(size_mib * 1024 * 1024);
        base.write_file(&format!("/base{size_mib}.bin"), &payload)
            .unwrap();
    }
    drop(base);
    for size_mib in [1_usize, 8, 32] {
        let delta_path = temp.path().join(format!("delta{size_mib}.db"));
        let mut overlay = OverlayFs::open(&delta_path, &base_path).unwrap();
        overlay.delta().enable_wal().unwrap();
        // copy_up is the real cost: a whole-file replace never reads the base,
        // so measuring write_file here would report ~1 ms and mean nothing.
        let start = Instant::now();
        overlay.copy_up(&format!("/base{size_mib}.bin")).unwrap();
        let elapsed = start.elapsed();
        println!(
            "copy-up: first write to a {size_mib} MiB base file took {:.3} ms",
            millis(elapsed)
        );
        copy_up_rows.push(format!(
            r#"      {{ "baseFileMiB": {size_mib}, "firstWriteMs": {} }}"#,
            millis(elapsed)
        ));
    }
    report.push(format!(
        "    \"copyUp\": [\n{}\n    ]",
        copy_up_rows.join(",\n")
    ));

    if let Some(path) = json_path {
        let json = format!(
            "{{\n  \"scale\": {scale},\n  \"results\": {{\n{}\n  }}\n}}\n",
            report.join(",\n")
        );
        std::fs::write(&path, json).unwrap();
        println!("wrote {path}");
    }
}
