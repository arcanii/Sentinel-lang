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
//! (3b-1) (A4): top-level decl dispatch (a brace-depth pass-1 scan + a source-
//! order pass-2 walk) + the struct symbol table → `(struct #id Name (field …))`
//! heads + struct-lit `#id`; the symbol tables are bundled in a `RCtx` struct
//! threaded as `&mut RCtx` (the A4 probe). (3b-2) (A5): the enum table →
//! `(enum #id Name (variant …))` heads + the `Enum::Variant` → `enum-construct`
//! split (the enum table is checked FIRST). The remaining `::`-path / decl forms
//! are slices (3b-3)–(3b-5); the corpus differential is the phase-go (D9).

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
    // (3b-1): top-level decl dispatch + the struct table + struct-lit `#id`.
    // Structs dump `(struct #id Name (field ...))` in SOURCE ORDER interleaved
    // with fns; struct-lit gains its StructId; VarIds still flow globally across
    // fns (the struct decl consumes no varid). Multiple structs prove the
    // source-order StructId counter; a `Vec<i64>` field proves field-type dump.
    "struct Point { x: i64, y: i64 }\nfn main() -> i64 { let p = Point { x: 1, y: 2 }; 0 }\n",
    "fn main() -> i64 { 0 }\nstruct S { a: i64 }\nstruct T { b: bool, c: i64 }\n",
    "struct V { n: i64 }\nfn get(v: V) -> i64 { v.n }\nfn main() -> i64 { let q = V { n: 7 }; get(q) }\n",
    "struct Pair { a: i64, b: i64 }\nfn mk(x: i64, y: i64) -> Pair { Pair { a: x, b: y } }\nfn main() -> i64 { let p = mk(3, 4); p.a + p.b }\n",
    "struct Empty { z: bool }\nstruct Wrap { inner: i64 }\nfn main() -> i64 { let w = Wrap { inner: 9 }; w.inner }\n",
    "struct Nested { v: Vec<i64> }\nfn main() -> i64 { 0 }\n",
    // (3b-2): the enum table + `(enum #id Name (variant …))` heads + the
    // `Enum::Variant(args)` → `(enum-construct #E Enum Variant args)` split (the
    // enum table is checked FIRST in the parser's uniform `Qcall`/`ClassInit`).
    // EnumId is its own source-order namespace (an enum #0 coexists with a struct
    // #0); variant payloads dump as types; a construct consumes no VarId.
    "enum Color { Red, Green, Blue }\nfn main() -> i64 { let c = Color::Red; 0 }\n",
    "enum Opt { None, Some(i64) }\nfn main() -> i64 { let x = Opt::Some(5); let y = Opt::None; 0 }\n",
    "struct S { a: i64 }\nenum En { X, Y(bool, i64) }\nfn main() -> i64 { let e = En::Y(true, 3); 0 }\n",
    "enum A { P }\nenum B { Q, R(i64) }\nfn main() -> i64 { let p = A::P; let r = B::R(9); 0 }\n",
    "enum Tri { One, Two, Three }\nfn pick(n: i64) -> Tri { Tri::Two }\nfn main() -> i64 { let t = pick(1); 0 }\n",
    "enum Wrap { W(i64) }\nfn main() -> i64 { let w = Wrap::W(1 + 2); 0 }\n",
    "fn main() -> i64 { 0 }\nenum Late { L, M(bool) }\n",
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
