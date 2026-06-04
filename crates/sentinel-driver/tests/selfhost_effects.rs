//! Phase D self-host port (5/N) / ADR 0042 D8: the effect-check differential
//! test. Compile `selfhost/effects.sentinel` (which `use`s the parser as a D.6
//! module) with the Rust `snc`, then assert its effect-row dump is byte-identical
//! to `snc effects` — first for a seed set, then over the ENTIRE clean-effect
//! corpus (the phase-go). Each fn dumps `(fn #<id> <name> <effect-name>…)` (its
//! effective effect row). Fixtures the oracle rejects (parse/resolve/type errors OR
//! an effect error — annotation-mismatch / unhandled-effect) exit nonzero and are
//! skipped — error parity is out of scope (ADR 0042 D5/D7), as in every prior stage.

use std::path::{Path, PathBuf};
use std::process::Command;

fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .canonicalize()
        .expect("canonicalize workspace root")
}

/// Stage `parser.sentinel` + `effects.sentinel` into `tmp` (so `use parser::…`
/// resolves to the sibling file) and compile the entry `effects.sentinel`.
fn build_sentinel_effect_checker(tmp: &Path) -> PathBuf {
    let root = workspace_root();
    std::fs::copy(root.join("selfhost/parser.sentinel"), tmp.join("parser.sentinel"))
        .expect("stage parser.sentinel");
    let entry = tmp.join("effects.sentinel");
    std::fs::copy(root.join("selfhost/effects.sentinel"), &entry).expect("stage effects.sentinel");
    let bin = tmp.join("seffects");
    let out = Command::new(env!("CARGO_BIN_EXE_snc"))
        .arg("build")
        .arg(&entry)
        .arg("-o")
        .arg(&bin)
        .output()
        .expect("run snc build");
    assert!(
        out.status.success(),
        "compiling selfhost/effects.sentinel failed:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    bin
}

/// Seeds: effect-free, an annotated+handled effect, inference through the call graph
/// (an unannotated caller picks up a callee's row), concurrency (scope discharges the
/// Async from spawn/await), and a multi-effect fn with nested handlers.
const SEEDS: &[&str] = &[
    "fn add(a: i64, b: i64) -> i64 { a + b }\nfn main() -> i64 { add(1, 2) }\n",
    "effect Io { read() -> i64; }\nfn do_work() -> i64 ! { Io } { perform Io.read() }\nfn main() -> i64 { handle do_work() with { Io.read(k) => k(1) } }\n",
    "effect Io { read() -> i64; }\nfn a() -> i64 ! { Io } { perform Io.read() }\nfn b() -> i64 { a() }\nfn main() -> i64 { handle b() with { Io.read(k) => k(1) } }\n",
    "fn dbl(x: i64) -> i64 ! { Async } { x * 2 }\nfn main() -> i64 { let r: i64 = scope concurrent { let t = spawn dbl(21); t.await }; r }\n",
    "effect A { fa() -> i64; }\neffect B { fb() -> i64; }\nfn both() -> i64 ! { A, B } { perform A.fa() + perform B.fb() }\nfn main() -> i64 { handle (handle both() with { A.fa(k) => k(1) }) with { B.fb(k) => k(2) } }\n",
];

#[test]
fn sentinel_effect_checker_matches_oracle_on_seeds() {
    let tmp = std::env::temp_dir().join(format!("snc_selfhost_effects_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(&tmp).expect("create temp dir");
    let checker = build_sentinel_effect_checker(&tmp);

    let work = tmp.join("work");
    std::fs::create_dir_all(&work).expect("create work dir");
    let input = work.join("input.sentinel");

    let mut mismatches: Vec<String> = Vec::new();
    for seed in SEEDS {
        std::fs::write(&input, seed).expect("stage seed");
        let oracle = Command::new(env!("CARGO_BIN_EXE_snc"))
            .arg("effects")
            .arg(&input)
            .output()
            .expect("run snc effects");
        let sentinel = Command::new(&checker)
            .current_dir(&work)
            .output()
            .expect("run the Sentinel effect-checker");
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
        "the Sentinel effect-checker diverged from `snc effects` on {}/{} seed(s):\n{}",
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

/// (5b) the D8 PHASE-GO: the Sentinel effect-checker matches `snc effects`
/// byte-for-byte over the ENTIRE clean-effect corpus (every tests/pass + tests/ui
/// fixture the oracle accepts). Mirrors `sentinel_typer_matches_oracle_on_corpus`.
/// Rejected fixtures (parse/resolve/type/effect errors) are skipped (ADR 0042 D5/D7).
#[test]
fn sentinel_effect_checker_matches_oracle_on_corpus() {
    let tmp =
        std::env::temp_dir().join(format!("snc_selfhost_effects_corpus_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(&tmp).expect("create temp dir");
    let checker = build_sentinel_effect_checker(&tmp);

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
            .arg("effects")
            .arg(&input)
            .output()
            .expect("run snc effects");
        if !oracle.status.success() {
            continue;
        }
        clean += 1;
        let sentinel = Command::new(&checker)
            .current_dir(&work)
            .output()
            .expect("run the Sentinel effect-checker");
        if oracle.stdout != sentinel.stdout {
            mismatches.push(format!(
                "  {} (oracle {} bytes vs sentinel {} bytes)",
                fixture.file_name().unwrap().to_string_lossy(),
                oracle.stdout.len(),
                sentinel.stdout.len()
            ));
        }
    }

    assert!(clean > 100, "expected >100 clean-effect fixtures, got {clean}");
    assert!(
        mismatches.is_empty(),
        "the Sentinel effect-checker diverged from `snc effects` on {}/{} clean-effect fixture(s):\n{}",
        mismatches.len(),
        clean,
        mismatches.join("\n")
    );
}
