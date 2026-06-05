//! Phase D self-host port (7/N) / ADR 0044 D8: the MIR differential test.
//! Compile `selfhost/mir.sentinel` (which REUSES `selfhost/types.sentinel` via a D.6
//! `use`, mode 2 — which in turn `use`s the parser, a 3-deep module chain) with the
//! Rust `snc`, then assert its lowered-form dump is byte-identical to `snc mir`.
//!
//! (7a) covers the STRAIGHT-LINE grammar (const / var / unary / binop / cmp / `let`
//! → one block + Return); (7b) adds CONTROL FLOW (`if` / `&&` / `||` → branch + a
//! merge block whose params reconcile diverged vars, VarId-sorted); (7c) adds the
//! `Opaque` catch-all (struct/array/field/widen/char/string/null), `Load`
//! (index/`*`-deref), `declassify`, and calls (plain + builtin/generic via the
//! `mir_args` operand stack). The effect/class/enum forms + the full-corpus phase-go
//! (`sentinel_mir_matches_oracle_on_corpus`) close (7c)/(7e). Fixtures the oracle
//! rejects (parse/resolve/type errors) exit nonzero and are skipped — error parity is
//! out of scope (ADR 0044 D7), as in every prior stage.

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
    // (7b) control flow: if/branch + merge param, && / || short-circuit (incl. secret
    // — a secret-dependent branch), a var diverged across arms (1 and 2 vars), nested
    // ifs, and a logic chain.
    "fn pick(c: bool) -> i64 { if c { 1 } else { 2 } }\nfn main() -> i64 { 0 }\n",
    "fn both(a: bool, b: bool) -> bool { a && b }\nfn main() -> i64 { 0 }\n",
    "fn either(a: bool, b: bool) -> bool { a || b }\nfn main() -> i64 { 0 }\n",
    "fn sec(s: secret bool, t: secret bool) -> secret bool { s && t }\nfn main() -> i64 { 0 }\n",
    "fn g(c: bool) -> i64 { let mut x = 1; if c { x = 2; 0 } else { 0 }; x }\nfn main() -> i64 { 0 }\n",
    "fn nested(a: bool, b: bool) -> i64 { if a { if b { 1 } else { 2 } } else { 3 } }\nfn main() -> i64 { 0 }\n",
    "fn chain(a: bool, b: bool, cc: bool) -> bool { a && b || cc }\nfn main() -> i64 { 0 }\n",
    "fn two(c: bool, d: bool) -> i64 { let mut x = 0; let mut y = 10; if c { x = 1; 0 } else { y = 20; 0 }; x + y }\nfn main() -> i64 { 0 }\n",
    // (7c) the Opaque catch-all + Load + calls: a plain call with args, a nested call, a
    // struct-lit + field read, an array-lit + index, a `*`-deref load, a widen-to-nullable
    // Opaque, declassify, a char/string Opaque, and generic/builtin calls (vec_new/push).
    "fn id(x: i64) -> i64 { x }\nfn use_it(a: i64, b: i64) -> i64 { id(a) + id(b) }\nfn main() -> i64 { 0 }\n",
    "fn add3(a: i64, b: i64, cc: i64) -> i64 { a + b + cc }\nfn nest(x: i64) -> i64 { add3(x, add3(x, x, x), x) }\nfn main() -> i64 { 0 }\n",
    "struct P { x: i64, y: i64 }\nfn mk(a: i64, b: i64) -> P { P { x: a, y: b } }\nfn getx(p: P) -> i64 { p.x }\nfn main() -> i64 { 0 }\n",
    "fn arr() -> [i64] { [1, 2, 3] }\nfn at(a: [i64], i: i64) -> i64 { a[i] / 2 }\nfn main() -> i64 { 0 }\n",
    "fn deref(r: &i64) -> i64 { *r }\nfn main() -> i64 { 0 }\n",
    "fn widen(x: i64) -> ?i64 { x }\nfn wb(b: i64) -> ?i64 { let y: ?i64 = b + 1; y }\nfn main() -> i64 { 0 }\n",
    "fn unwrap(s: secret i64) -> i64 { declassify(s) }\nfn main() -> i64 { 0 }\n",
    "fn ch() -> u8 { 'a' }\nfn st() -> [u8] { \"hi\" }\nfn main() -> i64 { 0 }\n",
    "fn pushy(n: i64) -> [i64] { let mut v: Vec<i64> = vec_new(); push(&mut v, n); vec_to_array(v) }\nfn ln(a: [i64]) -> i64 { len(a) }\nfn main() -> i64 { 0 }\n",
    "fn id<T>(x: T) -> T { x }\nfn use_g() -> i64 { id(5) }\nfn main() -> i64 { 0 }\n",
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

/// (7e) the D8 PHASE-GO: the Sentinel MIR lowerer matches `snc mir` byte-for-byte
/// over the ENTIRE clean-lowering corpus (= the type-clean set — lowering is total).
/// Mirrors `sentinel_borrow_checker_matches_oracle_on_corpus`; fixtures the oracle
/// rejects (parse/resolve/type errors) exit nonzero and are skipped.
#[test]
fn sentinel_mir_matches_oracle_on_corpus() {
    let tmp = std::env::temp_dir().join(format!("snc_selfhost_mir_corpus_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(&tmp).expect("create temp dir");
    let lowerer = build_sentinel_mir_lowerer(&tmp);

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
            .arg("mir")
            .arg(&input)
            .output()
            .expect("run snc mir");
        if !oracle.status.success() {
            continue;
        }
        clean += 1;
        let sentinel = Command::new(&lowerer)
            .current_dir(&work)
            .output()
            .expect("run the Sentinel MIR lowerer");
        if oracle.stdout != sentinel.stdout {
            mismatches.push(format!(
                "  {} (oracle {} bytes vs sentinel {} bytes)",
                fixture.file_name().unwrap().to_string_lossy(),
                oracle.stdout.len(),
                sentinel.stdout.len()
            ));
        }
    }

    assert!(clean > 100, "expected >100 clean-lowering fixtures, got {clean}");
    assert!(
        mismatches.is_empty(),
        "the Sentinel MIR lowerer diverged from `snc mir` on {}/{} clean-lowering fixture(s):\n{}",
        mismatches.len(),
        clean,
        mismatches.join("\n")
    );
}
