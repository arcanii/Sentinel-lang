//! ADR 0076 — the compiler's two-part version, and the build id that closes register D73.
//!
//! The version is a hand-maintained semver plus a computed build id:
//! `snc 0.1.0 (0x3f2a1c9d4e5b6a70)`. The semver encodes a judgement, so a human keeps it;
//! the build id encodes a fact, so it is computed (D4/D5).
//!
//! Two things are worth testing that look like they do not need it. A version QUERY is not
//! a usage error, so it goes to stdout and exits 0 — the code path it would otherwise fall
//! into prints usage to stderr and exits 2, which would look like it worked from a shell.
//! And the build id must be present, because the whole of D5 is that `unit_fingerprint` has
//! a compiler identity that MOVES; a build that quietly lost it would pass every other test
//! in the tree and silently restore D73.

use std::process::Command;

fn run(args: &[&str]) -> (String, String, i32) {
    let out = Command::new(env!("CARGO_BIN_EXE_snc"))
        .args(args)
        .output()
        .expect("run snc");
    (
        String::from_utf8_lossy(&out.stdout).to_string(),
        String::from_utf8_lossy(&out.stderr).to_string(),
        out.status.code().expect("process killed by signal"),
    )
}

/// The shape, verbatim: `snc <semver> (0x<16 hex>)`, on stdout, exit 0, stderr silent.
#[test]
fn version_flag_prints_the_two_part_version_to_stdout() {
    for flag in ["--version", "-V", "version"] {
        let (stdout, stderr, code) = run(&[flag]);
        assert_eq!(code, 0, "`snc {flag}` must exit 0, not report a usage error");
        assert!(
            stderr.is_empty(),
            "`snc {flag}` must not write to stderr, got:\n{stderr}"
        );
        let line = stdout.trim_end();
        let expected_prefix = format!("snc {} (0x", env!("CARGO_PKG_VERSION"));
        assert!(
            line.starts_with(&expected_prefix),
            "`snc {flag}` printed {line:?}, expected it to start {expected_prefix:?}"
        );
        assert!(line.ends_with(')'), "`snc {flag}` printed {line:?}");
        // The id itself: exactly 16 lowercase hex digits, the same width
        // `unit_fingerprint` writes into a `.o.fp` sidecar.
        let id = &line[expected_prefix.len()..line.len() - 1];
        assert_eq!(id.len(), 16, "build id {id:?} is not 16 digits");
        assert!(
            id.chars().all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase()),
            "build id {id:?} is not lowercase hex"
        );
        // Not a constant stand-in. A zeroed id is what a fail-open fallback would produce.
        assert_ne!(id, "0000000000000000", "the build id must be computed");
    }
}

/// The semver is the one in `Cargo.toml`, and it is past the `0.0.1` placeholder ADR 0076
/// replaced. A workspace-wide version bump that missed the driver would show up here.
#[test]
fn version_is_the_workspace_semver_and_not_the_placeholder() {
    let v = env!("CARGO_PKG_VERSION");
    assert_ne!(v, "0.0.1", "`0.0.1` is the cargo-init placeholder ADR 0076 replaced");
    let parts: Vec<&str> = v.split('.').collect();
    assert_eq!(parts.len(), 3, "expected major.minor.patch, got {v:?}");
    for p in &parts {
        assert!(
            !p.is_empty() && p.chars().all(|c| c.is_ascii_digit()),
            "{v:?} is not major.minor.patch"
        );
    }
}

/// ADR 0076 D6: the version must never reach emitted LLVM IR. `scg` has no access to a Rust
/// crate's version, and the codegen differential compares the oracle against it byte for
/// byte, so a stamp there breaks both bootstrap fixed points. This is the cheap guard that
/// names the rule at the place someone would break it; the differentials are the real one.
#[test]
fn the_version_never_reaches_emitted_ir() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .canonicalize()
        .expect("workspace root");
    let fixture = root.join("tests/pass/c02_arithmetic.sentinel");
    let (stdout, _, code) = run(&["llvm", fixture.to_str().expect("utf-8 path")]);
    assert_eq!(code, 0, "the oracle must emit for this fixture");
    assert!(
        !stdout.contains(env!("CARGO_PKG_VERSION")),
        "the compiler version leaked into emitted IR"
    );
    for needle in ["snc ", "0x"] {
        assert!(
            !stdout.contains(needle),
            "{needle:?} appears in emitted IR — if that is a version stamp, it breaks the \
             `scg` differential and both bootstrap fixed points (ADR 0076 D6)"
        );
    }
}
