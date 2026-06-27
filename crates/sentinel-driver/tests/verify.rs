//! ADR 0061 code signing — `snc verify <file> [--sig <sig>]`.
//!
//! Drives the real `snc` binary against a **detached** signature carrier over a
//! source file. `snc verify` is pure Rust (read the file + the carrier, run the
//! in-process Ed25519 verifier) — no compilation or linking — so these run under
//! plain `cargo test` with no MSVC/vcvars environment.
//!
//! The golden carrier below was produced by `sentinel-trust`'s Ed25519 signer
//! (itself validated against the RFC 8032 §7.1 vectors) over the exact `BODY`
//! bytes, with grant `alloc`, under a fixed test seed — so it is a genuine
//! signature the shipped verifier accepts, checked in as a fixture.

use std::path::PathBuf;
use std::process::Command;

/// The exact bytes the golden signature covers.
const BODY: &str = "pub fn add(a: i64, b: i64) -> i64 { a + b }\n";

/// A valid detached carrier over `BODY` (key derived from a fixed test seed).
const GOLDEN_SIG: &str = "\
sentinel-signature v1
algorithm: ed25519
key: 197f6b23e16c8532c6abc838facd5ea789be0c76b2920334039bfa8b3d368d61
grants: alloc
signature: ef9d6015359a0ae01091996ff661e70f4fe18ee82a3ae5248e9b26876f84a0ac6c85c81e7bea4548ba4ae65b0e421aa85ca4903970bc088e5bf418b3004e4d0b
";

fn temp_dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("snc_verify_{}_{}", std::process::id(), name));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("create temp dir");
    dir
}

/// Run `snc verify <file>` (default `<file>.sig`) → (success, stdout, stderr).
fn verify(file: &PathBuf) -> (bool, String, String) {
    let out = Command::new(env!("CARGO_BIN_EXE_snc"))
        .arg("verify")
        .arg(file)
        .output()
        .expect("run snc verify");
    (
        out.status.success(),
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    )
}

#[test]
fn valid_detached_signature_verifies() {
    let dir = temp_dir("ok");
    let body = dir.join("lib.sentinel");
    std::fs::write(&body, BODY).unwrap();
    std::fs::write(dir.join("lib.sentinel.sig"), GOLDEN_SIG).unwrap();

    let (ok, stdout, stderr) = verify(&body);
    assert!(ok, "expected a valid signature to verify; stderr:\n{stderr}");
    assert!(stdout.contains("signature OK"), "stdout:\n{stdout}");
    assert!(stdout.contains("grants: alloc"), "expected the granted capability; stdout:\n{stdout}");
}

#[test]
fn a_tampered_source_byte_fails_verification() {
    let dir = temp_dir("tamper");
    let body = dir.join("lib.sentinel");
    // One byte different from the signed BODY.
    std::fs::write(&body, "pub fn add(a: i64, b: i64) -> i64 { a - b }\n").unwrap();
    std::fs::write(dir.join("lib.sentinel.sig"), GOLDEN_SIG).unwrap();

    let (ok, _stdout, stderr) = verify(&body);
    assert!(!ok, "a tampered source must fail verification");
    assert!(stderr.contains("FAILED"), "stderr:\n{stderr}");
}

#[test]
fn missing_signature_is_a_clean_error() {
    let dir = temp_dir("nosig");
    let body = dir.join("lib.sentinel");
    std::fs::write(&body, BODY).unwrap();
    // no .sig written.
    let (ok, _stdout, stderr) = verify(&body);
    assert!(!ok, "a missing signature must fail");
    assert!(stderr.contains("cannot read signature"), "stderr:\n{stderr}");
}

#[test]
fn explicit_sig_path_is_honored() {
    let dir = temp_dir("explicit");
    let body = dir.join("lib.sentinel");
    let sig = dir.join("detached.sig");
    std::fs::write(&body, BODY).unwrap();
    std::fs::write(&sig, GOLDEN_SIG).unwrap();

    let out = Command::new(env!("CARGO_BIN_EXE_snc"))
        .arg("verify")
        .arg(&body)
        .arg("--sig")
        .arg(&sig)
        .output()
        .expect("run snc verify");
    assert!(out.status.success(), "stderr:\n{}", String::from_utf8_lossy(&out.stderr));
}
