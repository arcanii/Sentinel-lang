//! Phase D self-host port (7/N) / ADR 0044 D8: the MIR differential test.
//! Compile `selfhost/mir.sentinel` (which REUSES `selfhost/types.sentinel` via a D.6
//! `use`, mode 2 — which in turn `use`s the parser, a 3-deep module chain) with the
//! Rust `snc`, then assert its lowered-form dump is byte-identical to `snc mir`.
//!
//! (7a) covers the STRAIGHT-LINE grammar (const / var / unary / binop / cmp / `let`
//! → one block + Return); control flow (if / `&&` / `||` → branch + merge) is (7b),
//! the `Opaque` catch-all + `Load` + calls are (7c), and the full-corpus phase-go
//! (`sentinel_mir_matches_oracle_on_corpus`) is (7e). Fixtures the oracle rejects
//! (parse/resolve/type errors) exit nonzero and are skipped — error parity is out of
//! scope (ADR 0044 D7), as in every prior stage.

use std::path::{Path, PathBuf};
use std::process::Command;

fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .canonicalize()
        .expect("canonicalize workspace root")
}

/// Stage `parser.sentinel` + `types.sentinel` + `mir.sentinel` into `tmp` (so the
/// `use parser::…` / `use types::…` edges resolve) and compile the entry
/// `mir.sentinel`.
fn build_sentinel_mir_lowerer(tmp: &Path) -> PathBuf {
    let root = workspace_root();
    std::fs::copy(root.join("selfhost/parser.sentinel"), tmp.join("parser.sentinel"))
        .expect("stage parser.sentinel");
    std::fs::copy(root.join("selfhost/types.sentinel"), tmp.join("types.sentinel"))
        .expect("stage types.sentinel");
    let entry = tmp.join("mir.sentinel");
    std::fs::copy(root.join("selfhost/mir.sentinel"), &entry).expect("stage mir.sentinel");
    let bin = tmp.join("smir");
    let out = Command::new(env!("CARGO_BIN_EXE_snc"))
        .arg("build")
        .arg(&entry)
        .arg("-o")
        .arg(&bin)
        .output()
        .expect("run snc build");
    assert!(
        out.status.success(),
        "compiling selfhost/mir.sentinel failed:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    bin
}

/// Straight-line seeds (7a): arithmetic + comparison + a `let` + a unary negate, each
/// lowering to a single block ending in Return. The trailing `main` is lowered too.
const SEEDS: &[&str] = &[
    "fn dbl(x: i64) -> i64 { x * 2 }\nfn main() -> i64 { 0 }\n",
    "fn add(a: i64, b: i64) -> i64 { a + b }\nfn main() -> i64 { 0 }\n",
    "fn poly(a: i64, b: i64, c: i64) -> i64 { a * b + c }\nfn main() -> i64 { 0 }\n",
    "fn lt(a: i64, b: i64) -> bool { a < b }\nfn main() -> i64 { 0 }\n",
    "fn t() -> bool { true }\nfn main() -> i64 { 0 }\n",
    "fn lets(x: i64) -> i64 { let y = x + 1; y * 2 }\nfn main() -> i64 { 0 }\n",
    "fn neg(x: i64) -> i64 { let n = -x; n }\nfn main() -> i64 { 0 }\n",
    "fn cmp_let(a: i64, b: i64) -> bool { let d = a - b; d < 0 }\nfn main() -> i64 { 0 }\n",
];

#[test]
fn sentinel_mir_matches_oracle_on_seeds() {
    let tmp = std::env::temp_dir().join(format!("snc_selfhost_mir_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(&tmp).expect("create temp dir");
    let lowerer = build_sentinel_mir_lowerer(&tmp);

    let work = tmp.join("work");
    std::fs::create_dir_all(&work).expect("create work dir");
    let input = work.join("input.sentinel");

    let mut mismatches: Vec<String> = Vec::new();
    for seed in SEEDS {
        std::fs::write(&input, seed).expect("stage seed");
        let oracle = Command::new(env!("CARGO_BIN_EXE_snc"))
            .arg("mir")
            .arg(&input)
            .output()
            .expect("run snc mir");
        let sentinel = Command::new(&lowerer)
            .current_dir(&work)
            .output()
            .expect("run the Sentinel MIR lowerer");
        if oracle.stdout != sentinel.stdout {
            mismatches.push(format!(
                "  seed {seed:?}\n    oracle:   {}\n    sentinel: {}",
                String::from_utf8_lossy(&oracle.stdout).trim_end(),
                String::from_utf8_lossy(&sentinel.stdout).trim_end()
            ));
        }
    }
    assert!(
        mismatches.is_empty(),
        "the Sentinel MIR lowerer diverged from `snc mir` on {}/{} seed(s):\n{}",
        mismatches.len(),
        SEEDS.len(),
        mismatches.join("\n")
    );
}
