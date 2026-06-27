//! ADR 0061 — the Sentinel signing core ⇄ the Rust verifier twin.
//!
//! `tools/trust/sign_core.sentinel` is the dogfooded Ed25519 signer (it signs
//! the opaque payload bytes `snc` hands it, using the verified-constant-time
//! `std::security::ed25519`). This builds + runs it and asserts its output is
//! **byte-identical** to the Rust twin (`sentinel-trust`). Ed25519 is
//! deterministic (RFC 8032), and both implementations reproduce the RFC vectors,
//! so equality is exact — this is the cross-implementation guard that the
//! Sentinel signer and the Rust verifier never diverge: a signature the Sentinel
//! tool produces is exactly one `snc verify` accepts.
//!
//! Building the tool requires a host linker (it reaches `std/` via `--lib-path`),
//! so on a box without one (e.g. Windows outside an MSVC env) the test skips
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

/// The exact body the golden vector covers, the grant it carries, and the
/// Rust-twin pubkey/signature for seed `[42; 32]` over its canonical payload
/// (the same fixture the `snc verify` test uses).
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

#[test]
fn sentinel_sign_core_is_byte_identical_to_the_rust_twin() {
    let root = repo_root();
    let src = root.join("tools").join("trust").join("sign_core.sentinel");
    let work = std::env::temp_dir().join(format!("snc_signcore_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&work);
    std::fs::create_dir_all(&work).expect("create work dir");
    let exe = work.join(format!("sign_core{}", std::env::consts::EXE_SUFFIX));

    // Build the tool. `std::security::ed25519` lives under the repo `std/`, so
    // point `--lib-path` at the repo root (ADR 0037).
    let build = Command::new(env!("CARGO_BIN_EXE_snc"))
        .arg("build")
        .arg(&src)
        .arg("-o")
        .arg(&exe)
        .arg("--lib-path")
        .arg(&root)
        .output()
        .expect("run snc build");
    if !build.status.success() {
        let err = String::from_utf8_lossy(&build.stderr);
        // No host linker (e.g. Windows without an MSVC env) → skip; a real
        // compile error in the tool still fails.
        if err.contains("link") {
            eprintln!("skipping sign_core twin test — no host linker:\n{err}");
            return;
        }
        panic!("snc build of sign_core failed:\n{err}");
    }

    // payload = the canonical bytes for (pubkey, grant, SHA-512(body)). The
    // Sentinel tool re-derives the pubkey from the seed; we use the known golden
    // pubkey to build the same payload bytes it will sign.
    let pk: [u8; 32] = unhex(GOLDEN_PK).try_into().unwrap();
    let payload = canonical_payload(&pk, &[GRANT.to_string()], &sha512(BODY));
    std::fs::write(work.join("seed.bin"), [42u8; 32]).expect("write seed.bin");
    std::fs::write(work.join("payload.bin"), &payload).expect("write payload.bin");

    // The tool reads seed.bin/payload.bin from its CWD and writes sigout.bin there.
    let run = Command::new(&exe)
        .current_dir(&work)
        .output()
        .expect("run sign_core");
    assert!(run.status.success(), "sign_core run failed:\n{}", String::from_utf8_lossy(&run.stderr));

    let out = std::fs::read(work.join("sigout.bin")).expect("read sigout.bin");
    assert_eq!(out.len(), 96, "expected pk(32) || sig(64)");
    assert_eq!(out[..32], unhex(GOLDEN_PK)[..], "Sentinel pubkey must match the Rust twin");
    assert_eq!(
        out[32..],
        unhex(GOLDEN_SIG)[..],
        "Sentinel signature must be byte-identical to the Rust twin"
    );
}
