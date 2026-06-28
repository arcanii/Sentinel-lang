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

use std::path::{Path, PathBuf};
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

/// Build a `tools/trust/<name>.sentinel` core into `work`. Returns `Some(exe)`,
/// or `None` if no host linker is available (the test then skips). A compile
/// error panics.
fn build_core(work: &Path, name: &str) -> Option<PathBuf> {
    let root = repo_root();
    let src = root.join("tools").join("trust").join(format!("{name}.sentinel"));
    let exe = work.join(format!("{name}{}", std::env::consts::EXE_SUFFIX));
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
    panic!("snc build of {name} failed:\n{err}");
}

fn build_sign_core(work: &Path) -> Option<PathBuf> {
    build_core(work, "sign_core")
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

#[test]
fn signed_trusted_module_builds_and_runs_under_strict() {
    // The gate's success path (D7): a real program, signed by a trusted key,
    // passes `--require-signatures strict` and builds + runs. add → exit 7.
    let work = fresh_dir("gatebuild");
    let Some(signer) = build_sign_core(&work) else { return };

    let mut key = vec![42u8; 32];
    key.extend_from_slice(&unhex(GOLDEN_PK));
    std::fs::write(work.join("my.key"), &key).unwrap();

    let entry = work.join("app.sentinel");
    std::fs::write(&entry, "fn main() -> i64 { 7 }\n").unwrap();

    // Sign the program, then trust its key in the default manifest.
    let sign = Command::new(env!("CARGO_BIN_EXE_snc"))
        .arg("sign")
        .arg(&entry)
        .arg("--key")
        .arg(work.join("my.key"))
        .arg("--signer")
        .arg(&signer)
        .output()
        .expect("run snc sign");
    assert!(sign.status.success(), "snc sign failed:\n{}", String::from_utf8_lossy(&sign.stderr));
    std::fs::write(work.join("sentinel-trust.toml"), format!("[[keys]]\npubkey = \"{GOLDEN_PK}\"\n")).unwrap();

    // Build under strict (cwd = work so the default trust manifest is found).
    let exe = work.join(format!("app{}", std::env::consts::EXE_SUFFIX));
    let build = Command::new(env!("CARGO_BIN_EXE_snc"))
        .current_dir(&work)
        .arg("build")
        .arg("app.sentinel")
        .arg("-o")
        .arg(&exe)
        .arg("--require-signatures")
        .arg("strict")
        .output()
        .expect("run snc build");
    assert!(build.status.success(), "strict build of a trusted module failed:\n{}", String::from_utf8_lossy(&build.stderr));

    let run = Command::new(&exe).output().expect("run app");
    assert_eq!(run.status.code(), Some(7), "the gated program must run normally");
}

#[test]
fn snc_keygen_generates_a_key_then_signs_and_verifies() {
    // The full author flow with a freshly GENERATED key — exercises
    // `std::sys::random` (getentropy on unix / RtlGenRandom on Windows, selected
    // by ADR 0062 conditional compilation), which closes the Windows keygen gap.
    let work = fresh_dir("keygen");
    let Some(keygen) = build_core(&work, "keygen_core") else { return };
    let Some(signer) = build_core(&work, "sign_core") else { return };

    // snc keygen -o my.key --keygen-tool <keygen_core>
    let key = work.join("my.key");
    let kg = Command::new(env!("CARGO_BIN_EXE_snc"))
        .arg("keygen")
        .arg("-o")
        .arg(&key)
        .arg("--keygen-tool")
        .arg(&keygen)
        .output()
        .expect("run snc keygen");
    assert!(kg.status.success(), "snc keygen failed:\n{}", String::from_utf8_lossy(&kg.stderr));
    assert_eq!(std::fs::read(&key).unwrap().len(), 64, "key file is seed(32)||pubkey(32)");

    // A second keygen must differ (real OS entropy, not zeros).
    let key2 = work.join("my2.key");
    let _ = Command::new(env!("CARGO_BIN_EXE_snc"))
        .arg("keygen")
        .arg("-o")
        .arg(&key2)
        .arg("--keygen-tool")
        .arg(&keygen)
        .output()
        .expect("run snc keygen");
    assert_ne!(std::fs::read(&key).unwrap(), std::fs::read(&key2).unwrap(), "two keygens must differ");

    // Sign a file with the generated key, then verify.
    let body = work.join("app.sentinel");
    std::fs::write(&body, "fn main() -> i64 { 0 }\n").unwrap();
    let sign = Command::new(env!("CARGO_BIN_EXE_snc"))
        .arg("sign")
        .arg(&body)
        .arg("--key")
        .arg(&key)
        .arg("--signer")
        .arg(&signer)
        .output()
        .expect("run snc sign");
    assert!(sign.status.success(), "snc sign failed:\n{}", String::from_utf8_lossy(&sign.stderr));

    let verify = Command::new(env!("CARGO_BIN_EXE_snc"))
        .arg("verify")
        .arg(&body)
        .output()
        .expect("run snc verify");
    assert!(verify.status.success(), "snc verify failed:\n{}", String::from_utf8_lossy(&verify.stderr));
}
