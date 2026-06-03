//! Phase D self-host port (3/N) / ADR 0040 D9: the resolve differential test.
//! Compile `selfhost/resolve.sentinel` (which `use`s `selfhost/parser.sentinel`
//! as a D.6 module) with the Rust `snc`, then assert its canonical resolved-AST
//! dump is byte-identical to `snc resolve` for a seed set of programs.
//!
//! MILESTONE-1 (ADR 0040 A2): paramful fns + free calls + arithmetic +
//! comparisons + unary + variable references over the params (the body is a
//! block). VarIds are GLOBAL (never reset across fns); builtins occupy FnId
//! 0..=13 so user fns start at #14; the built-in `Async` effect dumps last.
//! MILESTONE-2 (A3): `let` bindings — the binding set is pre-scanned from the
//! body tokens into the (immutable) scope so a `let` resolves its own VarId by
//! name (continuing after the params), with `mut` + optional type annotations.
//! The decl tables + the `::`-path disambiguation are slices (3b)–(3d); the
//! corpus differential is the phase-go (D9) and lands once those close.

use std::path::{Path, PathBuf};
use std::process::Command;

fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .canonicalize()
        .expect("canonicalize workspace root")
}

/// Stage `parser.sentinel` + `resolve.sentinel` into `tmp` (so `use parser::…`
/// resolves to the sibling file) and compile the entry `resolve.sentinel`.
fn build_sentinel_resolver(tmp: &Path) -> PathBuf {
    let root = workspace_root();
    std::fs::copy(root.join("selfhost/parser.sentinel"), tmp.join("parser.sentinel"))
        .expect("stage parser.sentinel");
    let entry = tmp.join("resolve.sentinel");
    std::fs::copy(root.join("selfhost/resolve.sentinel"), &entry).expect("stage resolve.sentinel");
    let bin = tmp.join("sresolve");
    let out = Command::new(env!("CARGO_BIN_EXE_snc"))
        .arg("build")
        .arg(&entry)
        .arg("-o")
        .arg(&bin)
        .output()
        .expect("run snc build");
    assert!(
        out.status.success(),
        "compiling selfhost/resolve.sentinel failed:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    bin
}

/// Milestone-1 seeds: paramful fns, free calls, arithmetic, comparisons, unary,
/// var refs over params, multi-fn programs (proving global VarId numbering).
const SEEDS: &[&str] = &[
    "fn main() -> i64 { 1 + 2 * 3 }\n",
    "fn add(a: i64, b: i64) -> i64 { a + b }\nfn main() -> i64 { add(1, 2) }\n",
    "fn main() -> i64 { print(42) }\n",
    "fn f(x: i64) -> i64 { x }\nfn g(y: i64) -> i64 { f(y) }\nfn main() -> i64 { g(7) }\n",
    "fn cmp(a: i64, b: i64) -> bool { a < b }\nfn main() -> i64 { 0 }\n",
    "fn f(p: i64) -> i64 { p }\nfn g(q: i64, r: i64) -> i64 { q + r }\nfn main() -> i64 { f(1) + g(2, 3) }\n",
    "fn len2(s: [u8]) -> i64 { len(s) }\nfn main() -> i64 { 0 }\n",
    "fn h(a: i64, b: i64, c: i64) -> i64 { a + b * c - a }\nfn main() -> i64 { h(1, 2, 3) }\n",
    "fn neg(x: i64) -> i64 { -x }\nfn main() -> i64 { neg(5) }\n",
    "fn mut_p(mut x: i64) -> i64 { x }\nfn main() -> i64 { mut_p(1) }\n",
    // milestone-2: `let` bindings — VarIds continue after the params (global +
    // sequential), `mut` + type annotations, a let value referencing earlier
    // bindings, lets mixed with params + calls across fns.
    "fn main() -> i64 { let x: i64 = 5; let y = x + 1; y }\n",
    "fn add(a: i64, b: i64) -> i64 { a + b }\nfn main() -> i64 { let z = add(1, 2); z }\n",
    "fn main() -> i64 { let mut s = 0; s }\n",
    "fn main() -> i64 { let mut acc: i64 = 0; let n: i64 = 5; acc + n }\n",
    "fn f(p: i64) -> i64 { let q = p + 1; let r = q * 2; r }\nfn main() -> i64 { f(3) }\n",
    "fn add(a: i64, b: i64) -> i64 { let s = a + b; s }\nfn main() -> i64 { add(1, 2) }\n",
    "fn g(x: i64) -> i64 { x }\nfn main() -> i64 { let a = g(1); let b = g(a); let c = g(b); c }\n",
];

#[test]
fn sentinel_resolver_matches_oracle_on_seeds() {
    let tmp = std::env::temp_dir().join(format!("snc_selfhost_resolve_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(&tmp).expect("create temp dir");
    let resolver = build_sentinel_resolver(&tmp);

    let work = tmp.join("work");
    std::fs::create_dir_all(&work).expect("create work dir");
    let input = work.join("input.sentinel");

    let mut mismatches: Vec<String> = Vec::new();
    for seed in SEEDS {
        std::fs::write(&input, seed).expect("stage seed");

        let oracle = Command::new(env!("CARGO_BIN_EXE_snc"))
            .arg("resolve")
            .arg(&input)
            .output()
            .expect("run snc resolve");
        let sentinel = Command::new(&resolver)
            .current_dir(&work)
            .output()
            .expect("run the Sentinel resolver");

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
        "the Sentinel resolver diverged from `snc resolve` on {}/{} seed(s):\n{}",
        mismatches.len(),
        SEEDS.len(),
        mismatches.join("\n")
    );
}
