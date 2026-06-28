//! ADR 0061 — the Sentinel signing core ⇄ the Rust verifier twin, and the
//! `snc sign` → `snc verify` round trip over the real Sentinel signer.
//!
//! `tools/trust/sign_core.sentinel` is the dogfooded Ed25519 signer (it signs the
//! opaque payload bytes `snc` hands it, using the verified-constant-time
//! `std::security::ed25519`). These tests build + run it.
//!
//! Building the tool requires a host linker (it reaches `std/` via `--lib-path`),
//! so on a box without one (e.g. Windows outside an MSVC env) the tests skip
//! cleanly. A *compile* error in the tool still fails loudly.

use std::path::PathBuf;
use std::process::Command;

use sentinel_trust::{canonical_payload, sha512};

/// The repo root (two levels up from this crate's manifest dir).
fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .canonicalize()
        .expect("canonicalize repo root")
}

/// The body the golden vector covers, the grant it carries, and the Rust-twin
/// pubkey/signature for seed `[42; 32]` over its canonical payload (the same
/// fixture the `snc verify` test uses).
const BODY: &[u8] = b"pub fn add(a: i64, b: i64) -> i64 { a + b }\n";
const GRANT: &str = "alloc";
const GOLDEN_PK: &str = "197f6b23e16c8532c6abc838facd5ea789be0c76b2920334039bfa8b3d368d61";
const GOLDEN_SIG: &str = "ef9d6015359a0ae01091996ff661e70f4fe18ee82a3ae5248e9b26876f84a0ac6c85c81e7bea4548ba4ae65b0e421aa85ca4903970bc088e5bf418b3004e4d0b";

fn unhex(s: &str) -> Vec<u8> {
    (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&s[i..i + 2], 16).unwrap())
        .collect()
}

fn fresh_dir(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("snc_signcore_{}_{}", std::process::id(), tag));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("create work dir");
    dir
}

/// Build `sign_core` into `work`. Returns `Some(exe)`, or `None` if no host
/// linker is available (the test then skips). A compile error panics.
fn build_sign_core(work: &PathBuf) -> Option<PathBuf> {
    let root = repo_root();
    let src = root.join("tools").join("trust").join("sign_core.sentinel");
    let exe = work.join(format!("sign_core{}", std::env::consts::EXE_SUFFIX));
    let build = Command::new(env!("CARGO_BIN_EXE_snc"))
        .arg("build")
        .arg(&src)
        .arg("-o")
        .arg(&exe)
        .arg("--lib-path")
        .arg(&root)
        .output()
        .expect("run snc build");
    if build.status.success() {
        return Some(exe);
    }
    let err = String::from_utf8_lossy(&build.stderr);
    if err.contains("link") {
        eprintln!("skipping — no host linker:\n{err}");
        return None;
    }
    panic!("snc build of sign_core failed:\n{err}");
}

#[test]
fn sentinel_sign_core_is_byte_identical_to_the_rust_twin() {
    let work = fresh_dir("twin");
    let Some(exe) = build_sign_core(&work) else { return };

    // payload = canonical bytes for (pubkey, grant, SHA-512(body)). We use the
    // known golden pubkey to build the same payload the tool will sign.
    let pk: [u8; 32] = unhex(GOLDEN_PK).try_into().unwrap();
    let payload = canonical_payload(&pk, &[GRANT.to_string()], &sha512(BODY));
    std::fs::write(work.join("seed.bin"), [42u8; 32]).unwrap();
    std::fs::write(work.join("payload.bin"), &payload).unwrap();

    let run = Command::new(&exe).current_dir(&work).output().expect("run sign_core");
    assert!(run.status.success(), "sign_core failed:\n{}", String::from_utf8_lossy(&run.stderr));

    let out = std::fs::read(work.join("sigout.bin")).expect("read sigout.bin");
    assert_eq!(out.len(), 96, "expected pk(32) || sig(64)");
    assert_eq!(out[..32], unhex(GOLDEN_PK)[..], "Sentinel pubkey must match the Rust twin");
    assert_eq!(out[32..], unhex(GOLDEN_SIG)[..], "Sentinel signature must be byte-identical to the Rust twin");
}

#[test]
fn snc_sign_then_verify_round_trips() {
    let work = fresh_dir("signcmd");
    let Some(exe) = build_sign_core(&work) else { return };

    // A key file is seed(32) || pubkey(32). Fixed seed [42;32] + the golden pk.
    let mut key = vec![42u8; 32];
    key.extend_from_slice(&unhex(GOLDEN_PK));
    std::fs::write(work.join("my.key"), &key).unwrap();
    let body = work.join("lib.sentinel");
    std::fs::write(&body, BODY).unwrap();

    // snc sign <body> --key my.key --grant alloc --signer <built sign_core>
    let sign = Command::new(env!("CARGO_BIN_EXE_snc"))
        .arg("sign")
        .arg(&body)
        .arg("--key")
        .arg(work.join("my.key"))
        .arg("--grant")
        .arg(GRANT)
        .arg("--signer")
        .arg(&exe)
        .output()
        .expect("run snc sign");
    assert!(sign.status.success(), "snc sign failed:\n{}", String::from_utf8_lossy(&sign.stderr));

    // The produced carrier must be the golden carrier (deterministic Ed25519).
    let carrier = std::fs::read_to_string(work.join("lib.sentinel.sig")).expect("read .sig");
    assert!(carrier.contains(&format!("key: {GOLDEN_PK}")), "carrier:\n{carrier}");
    assert!(carrier.contains(&format!("signature: {GOLDEN_SIG}")), "carrier:\n{carrier}");

    // snc verify <body> → OK.
    let verify = Command::new(env!("CARGO_BIN_EXE_snc"))
        .arg("verify")
        .arg(&body)
        .output()
        .expect("run snc verify");
    assert!(verify.status.success(), "snc verify failed:\n{}", String::from_utf8_lossy(&verify.stderr));
    assert!(String::from_utf8_lossy(&verify.stdout).contains("signature OK"));
}
