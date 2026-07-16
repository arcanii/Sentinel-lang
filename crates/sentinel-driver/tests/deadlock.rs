//! ADR 0071 M1.4b slice 4 (D5a): the opt-in `Deadlock` wait-for-graph tier,
//! end-to-end. A compiled Sentinel program that re-locks a mutex it already holds
//! (`let g = lock(m); let g2 = lock(m);`) is the minimal hold/wait cycle. Run with
//! `SENTINEL_DEADLOCK_DETECT=1`, the second `lock()` must return the null arm
//! IMMEDIATELY (not wait out the always-on 10s `LockTimeout` deadline) and the
//! runtime must report the cycle on stderr — proving the env gate, the graph
//! bookkeeping, and the fold-into-the-timeout-arm outcome all wire through a real
//! binary. (The no-env default path is covered by every other c71 fixture; running
//! this program without the var would genuinely block ~10s, so it is not exercised
//! here.) Like examples.rs, the build links (needs the host link toolchain).

use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

fn temp_dir() -> PathBuf {
    let dir = std::env::temp_dir().join(format!("snc_deadlock_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("create temp dir");
    dir
}

fn build(src: &str, dir: &Path, stem: &str) -> PathBuf {
    let entry = dir.join(format!("{stem}.sentinel"));
    std::fs::write(&entry, src).expect("write source");
    let exe = dir.join(format!("{stem}{}", std::env::consts::EXE_SUFFIX));
    let res = Command::new(env!("CARGO_BIN_EXE_snc"))
        .arg("build")
        .arg(&entry)
        .arg("-o")
        .arg(&exe)
        .output()
        .expect("run snc build");
    assert!(
        res.status.success(),
        "build of {stem} failed; stderr:\n{}",
        String::from_utf8_lossy(&res.stderr),
    );
    exe
}

#[test]
fn deadlock_detection_returns_the_null_arm_with_a_cycle_report() {
    let dir = temp_dir();
    // The guard `g` holds the lock; the second `lock(m)` waits on it from the
    // same thread — a self-cycle. Detection folds into the existing null arm, so
    // the program takes the `else` branch and exits 42.
    let src = "fn main() -> i64 {\n    let m = mutex_new(7);\n    let g = lock(m);\n    let g2 = lock(m);\n    if is_some(g2) { 1 } else { 42 }\n}\n";
    let exe = build(src, &dir, "self_deadlock");
    let start = Instant::now();
    let out = Command::new(&exe)
        .env("SENTINEL_DEADLOCK_DETECT", "1")
        .output()
        .expect("run the compiled program");
    let elapsed = start.elapsed();
    assert_eq!(
        out.status.code(),
        Some(42),
        "the detected deadlock is the null arm (is_some(g2) == false); stderr:\n{}",
        String::from_utf8_lossy(&out.stderr),
    );
    // The stderr assert below is the primary detection-vs-timeout discriminator
    // (the report is emitted only on the cycle path); the elapsed bound adds the
    // immediacy claim with margin for first-spawn overhead on a loaded box while
    // staying strictly under the 10s deadline.
    assert!(
        elapsed < Duration::from_secs(8),
        "detection must return instantly, not wait out the 10s LockTimeout \
         deadline (took {elapsed:?})"
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("deadlock detected"),
        "stderr must carry the cycle report; got:\n{stderr}"
    );
}
