//! ADR 0061 D7 — the build-time trust gate (`snc build --require-signatures`).
//!
//! These exercise the gate's **rejection** paths, which fail *before* any
//! compilation or linking (the gate runs right after module discovery) — so they
//! need no MSVC/vcvars environment and run under plain `cargo test`. The
//! gate-passes-then-builds success path (which does link) lives in `sign_core.rs`.

use std::path::Path;
use std::process::Command;

/// A valid sig carrier over `GOLDEN_BODY`, signed by `GOLDEN_PK` (the fixture the
/// `snc verify` tests share). Used to drive the "verified but untrusted" and
/// "tampered body" gate paths.
const GOLDEN_BODY: &str = "pub fn add(a: i64, b: i64) -> i64 { a + b }\n";
const GOLDEN_PK: &str = "197f6b23e16c8532c6abc838facd5ea789be0c76b2920334039bfa8b3d368d61";
const GOLDEN_SIG: &str = "\
sentinel-signature v1
algorithm: ed25519
key: 197f6b23e16c8532c6abc838facd5ea789be0c76b2920334039bfa8b3d368d61
grants: alloc
signature: ef9d6015359a0ae01091996ff661e70f4fe18ee82a3ae5248e9b26876f84a0ac6c85c81e7bea4548ba4ae65b0e421aa85ca4903970bc088e5bf418b3004e4d0b
";

fn temp_dir(name: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("snc_gate_{}_{}", std::process::id(), name));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("create temp dir");
    dir
}

/// `snc build <entry> [args…]` with the working dir set to `dir` (so the default
/// `sentinel-trust.toml` lookup is scoped to the test) → (success, stderr).
fn build_in(dir: &Path, entry: &str, args: &[&str]) -> (bool, String) {
    let out = Command::new(env!("CARGO_BIN_EXE_snc"))
        .current_dir(dir)
        .arg("build")
        .arg(entry)
        .args(args)
        .output()
        .expect("run snc build");
    (out.status.success(), String::from_utf8_lossy(&out.stderr).into_owned())
}

#[test]
fn strict_refuses_an_unsigned_module() {
    let dir = temp_dir("unsigned");
    std::fs::write(dir.join("main.sentinel"), "fn main() -> i64 { 0 }\n").unwrap();
    // No `.sig`, no trust manifest.
    let (ok, stderr) = build_in(&dir, "main.sentinel", &["--require-signatures", "strict"]);
    assert!(!ok, "strict must refuse an unsigned module; stderr:\n{stderr}");
    assert!(stderr.contains("unsigned"), "stderr:\n{stderr}");
    assert!(stderr.contains("build refused"), "stderr:\n{stderr}");
}

#[test]
fn strict_refuses_a_valid_signature_from_an_untrusted_key() {
    let dir = temp_dir("untrusted");
    std::fs::write(dir.join("lib.sentinel"), GOLDEN_BODY).unwrap();
    std::fs::write(dir.join("lib.sentinel.sig"), GOLDEN_SIG).unwrap();
    // No trust manifest → the (validly signing) key is not trusted.
    let (ok, stderr) = build_in(&dir, "lib.sentinel", &["--require-signatures", "strict"]);
    assert!(!ok, "strict must refuse an untrusted key; stderr:\n{stderr}");
    assert!(stderr.contains("UNTRUSTED"), "stderr:\n{stderr}");
}

#[test]
fn strict_refuses_a_tampered_module_even_from_a_trusted_key() {
    let dir = temp_dir("tampered");
    // The key IS trusted, but the body was changed after signing.
    std::fs::write(dir.join("trust.toml"), format!("[[keys]]\npubkey = \"{GOLDEN_PK}\"\n")).unwrap();
    std::fs::write(dir.join("lib.sentinel"), "pub fn add(a: i64, b: i64) -> i64 { a - b }\n").unwrap();
    std::fs::write(dir.join("lib.sentinel.sig"), GOLDEN_SIG).unwrap();
    let (ok, stderr) = build_in(
        &dir,
        "lib.sentinel",
        &["--require-signatures", "strict", "--trust", "trust.toml"],
    );
    assert!(!ok, "strict must refuse a tampered module; stderr:\n{stderr}");
    assert!(stderr.contains("does not verify"), "stderr:\n{stderr}");
}

#[test]
fn warn_reports_but_does_not_refuse_the_gate() {
    // In warn mode an unsigned module produces a warning, not a gate failure. The
    // build then proceeds (and here fails later for an unrelated reason — no
    // `main` — but NOT with a gate refusal), proving warn never refuses.
    let dir = temp_dir("warn");
    std::fs::write(dir.join("lib.sentinel"), GOLDEN_BODY).unwrap();
    let (_ok, stderr) = build_in(&dir, "lib.sentinel", &["--require-signatures", "warn"]);
    assert!(stderr.contains("warning"), "expected a warning; stderr:\n{stderr}");
    assert!(!stderr.contains("build refused"), "warn must not refuse; stderr:\n{stderr}");
}
