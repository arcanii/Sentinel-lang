//! Phase D self-host port (6/N) / ADR 0043 D8: the borrow-check differential test.
//! Compile `selfhost/borrow.sentinel` (which REUSES `selfhost/types.sentinel` via a
//! D.6 `use`, which in turn `use`s the parser — a 3-deep module chain) with the Rust
//! `snc`, then assert its moved-sources dump is byte-identical to `snc borrow` —
//! first for a seed set, then over the ENTIRE clean-borrow corpus (the phase-go).
//! Each fn dumps `(fn #<id> <name> #<vid>…)` (its moved-source VarIds). Fixtures the
//! oracle rejects (parse/resolve/type/borrow errors) exit nonzero and are skipped —
//! error parity is out of scope (ADR 0043 D5/D7), as in every prior stage.

use std::path::{Path, PathBuf};
use std::process::Command;

fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .canonicalize()
        .expect("canonicalize workspace root")
}

/// Stage `parser.sentinel` + `types.sentinel` + `borrow.sentinel` into `tmp` (so the
/// `use parser::…` / `use types::…` edges resolve) and compile the entry
/// `borrow.sentinel`.
fn build_sentinel_borrow_checker(tmp: &Path) -> PathBuf {
    let root = workspace_root();
    std::fs::copy(root.join("selfhost/parser.sentinel"), tmp.join("parser.sentinel"))
        .expect("stage parser.sentinel");
    std::fs::copy(root.join("selfhost/types.sentinel"), tmp.join("types.sentinel"))
        .expect("stage types.sentinel");
    let entry = tmp.join("borrow.sentinel");
    std::fs::copy(root.join("selfhost/borrow.sentinel"), &entry).expect("stage borrow.sentinel");
    let bin = tmp.join("sborrow");
    let out = Command::new(env!("CARGO_BIN_EXE_snc"))
        .arg("build")
        .arg(&entry)
        .arg("-o")
        .arg(&bin)
        .output()
        .expect("run snc build");
    assert!(
        out.status.success(),
        "compiling selfhost/borrow.sentinel failed:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    bin
}

/// Seeds: all-Copy (no moves), a struct moved by-value into a call (field read is
/// non-consuming), an array moved (index read non-consuming), the conservative
/// if-branch union (moved in EITHER arm → included), and enum-construct + match
/// (an arg moved into a construct, an arm payload moved out, the scrutinee NOT moved).
const SEEDS: &[&str] = &[
    "fn add(a: i64, b: i64) -> i64 { a + b }\nfn main() -> i64 { add(1, 2) }\n",
    "struct P { x: i64 }\nfn take(p: P) -> i64 { p.x }\nfn main() -> i64 { let a = P { x: 5 }; take(a) }\n",
    "fn take(a: [i64]) -> i64 { a[0] }\nfn main() -> i64 { let xs = [1, 2, 3]; take(xs) }\n",
    "struct P { x: i64 }\nfn pick(c: bool, p: P, q: P) -> P { if c { p } else { q } }\nfn main() -> i64 { let r = pick(true, P { x: 1 }, P { x: 2 }); r.x }\n",
    "struct P { x: i64 }\nenum W { Wrap(P) }\nfn unwrap(w: W) -> P { match w { W::Wrap(p) => p } }\nfn main() -> i64 { let pp: P = P { x: 5 }; let r: P = unwrap(W::Wrap(pp)); r.x }\n",
];

#[test]
fn sentinel_borrow_checker_matches_oracle_on_seeds() {
    let tmp = std::env::temp_dir().join(format!("snc_selfhost_borrow_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(&tmp).expect("create temp dir");
    let checker = build_sentinel_borrow_checker(&tmp);

    let work = tmp.join("work");
    std::fs::create_dir_all(&work).expect("create work dir");
    let input = work.join("input.sentinel");

    let mut mismatches: Vec<String> = Vec::new();
    for seed in SEEDS {
        std::fs::write(&input, seed).expect("stage seed");
        let oracle = Command::new(env!("CARGO_BIN_EXE_snc"))
            .arg("borrow")
            .arg(&input)
            .output()
            .expect("run snc borrow");
        let sentinel = Command::new(&checker)
            .current_dir(&work)
            .output()
            .expect("run the Sentinel borrow-checker");
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
        "the Sentinel borrow-checker diverged from `snc borrow` on {}/{} seed(s):\n{}",
        mismatches.len(),
        SEEDS.len(),
        mismatches.join("\n")
    );
}

/// Every `.sentinel` fixture under tests/pass + tests/ui, sorted.
fn collect_fixtures() -> Vec<PathBuf> {
    let root = workspace_root();
    let mut fixtures = Vec::new();
    for sub in ["tests/pass", "tests/ui"] {
        for entry in std::fs::read_dir(root.join(sub)).expect("read fixture dir") {
            let path = entry.expect("dir entry").path();
            if path.extension().and_then(|e| e.to_str()) == Some("sentinel") {
                fixtures.push(path);
            }
        }
    }
    fixtures.sort();
    fixtures
}

/// (6b) the D8 PHASE-GO: the Sentinel borrow-checker matches `snc borrow`
/// byte-for-byte over the ENTIRE clean-borrow corpus. Mirrors
/// `sentinel_effect_checker_matches_oracle_on_corpus`; rejected fixtures are skipped.
#[test]
fn sentinel_borrow_checker_matches_oracle_on_corpus() {
    let tmp =
        std::env::temp_dir().join(format!("snc_selfhost_borrow_corpus_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(&tmp).expect("create temp dir");
    let checker = build_sentinel_borrow_checker(&tmp);

    let work = tmp.join("work");
    std::fs::create_dir_all(&work).expect("create work dir");
    let input = work.join("input.sentinel");

    let fixtures = collect_fixtures();
    assert!(fixtures.len() > 100, "expected a substantial corpus, got {}", fixtures.len());

    let mut clean = 0usize;
    let mut mismatches: Vec<String> = Vec::new();
    for fixture in &fixtures {
        let bytes = std::fs::read(fixture).expect("read fixture");
        std::fs::write(&input, &bytes).expect("stage input");
        let oracle = Command::new(env!("CARGO_BIN_EXE_snc"))
            .arg("borrow")
            .arg(&input)
            .output()
            .expect("run snc borrow");
        if !oracle.status.success() {
            continue;
        }
        clean += 1;
        let sentinel = Command::new(&checker)
            .current_dir(&work)
            .output()
            .expect("run the Sentinel borrow-checker");
        if oracle.stdout != sentinel.stdout {
            mismatches.push(format!(
                "  {} (oracle {} bytes vs sentinel {} bytes)",
                fixture.file_name().unwrap().to_string_lossy(),
                oracle.stdout.len(),
                sentinel.stdout.len()
            ));
        }
    }

    assert!(clean > 100, "expected >100 clean-borrow fixtures, got {clean}");
    assert!(
        mismatches.is_empty(),
        "the Sentinel borrow-checker diverged from `snc borrow` on {}/{} clean-borrow fixture(s):\n{}",
        mismatches.len(),
        clean,
        mismatches.join("\n")
    );
}
