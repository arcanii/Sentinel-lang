//! Phase D self-host port (4/N) / ADR 0041 D9: the types differential test.
//! Compile `selfhost/types.sentinel` (which `use`s `selfhost/parser.sentinel` as a
//! D.6 module) with the Rust `snc`, then assert its canonical typed-AST dump is
//! byte-identical to `snc types` for a seed set of programs.
//!
//! (4b) the SCALAR skeleton + (4c) COMPOUND types. Paramful `fn`s over the scalar
//! grammar (int/bool literals, var refs, unary, binop/cmp/logic, `if`, blocks, `let`
//! with inference, calls) PLUS structs (struct-lit + field access), arrays (literal +
//! index + the generic `len`), and (4c-3) nullable (`?T` + `null` + the implicit
//! `T → ?T` widening via expected-type threading through `dump_texpr`) — each
//! expression node annotated with its inferred `Type` (a trailing ` :<type>`); the
//! `let`'s inferred type replaces resolve's `_`. The full-corpus differential (4i) is
//! the phase-go (D9); secret / enum / dispatch / generics land at 4d..4h.

use std::path::{Path, PathBuf};
use std::process::Command;

fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .canonicalize()
        .expect("canonicalize workspace root")
}

/// Stage `parser.sentinel` + `types.sentinel` into `tmp` (so `use parser::…`
/// resolves to the sibling file) and compile the entry `types.sentinel`.
fn build_sentinel_typer(tmp: &Path) -> PathBuf {
    let root = workspace_root();
    std::fs::copy(root.join("selfhost/parser.sentinel"), tmp.join("parser.sentinel"))
        .expect("stage parser.sentinel");
    let entry = tmp.join("types.sentinel");
    std::fs::copy(root.join("selfhost/types.sentinel"), &entry).expect("stage types.sentinel");
    let bin = tmp.join("stypes");
    let out = Command::new(env!("CARGO_BIN_EXE_snc"))
        .arg("build")
        .arg(&entry)
        .arg("-o")
        .arg(&bin)
        .output()
        .expect("run snc build");
    assert!(
        out.status.success(),
        "compiling selfhost/types.sentinel failed:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    bin
}

/// Milestone-1 seeds: the scalar grammar fully typed — arithmetic, comparisons,
/// logical, unary, `if`/blocks, `let` (annotated + inferred), multi-fn programs
/// (global VarIds + user-fn call return types), and scalar builtins.
const SEEDS: &[&str] = &[
    "fn main() -> i64 { 1 + 2 * 3 }\n",
    "fn add(a: i64, b: i64) -> i64 { a + b }\nfn main() -> i64 { add(1, 2) }\n",
    "fn main() -> i64 { print(42) }\n",
    "fn f(x: i64) -> i64 { x }\nfn g(y: i64) -> i64 { f(y) }\nfn main() -> i64 { g(7) }\n",
    "fn cmp(a: i64, b: i64) -> bool { a < b }\nfn main() -> i64 { 0 }\n",
    "fn h(a: i64, b: i64, c: i64) -> i64 { a + b * c - a }\nfn main() -> i64 { h(1, 2, 3) }\n",
    "fn neg(x: i64) -> i64 { -x }\nfn main() -> i64 { neg(5) }\n",
    "fn notb(b: bool) -> bool { !b }\nfn main() -> i64 { 0 }\n",
    "fn mut_p(mut x: i64) -> i64 { x }\nfn main() -> i64 { mut_p(1) }\n",
    // `let` inference: the binding shows its inferred type (resolve dumped `_`).
    "fn main() -> i64 { let x: i64 = 5; let y = x + 1; y }\n",
    "fn add(a: i64, b: i64) -> i64 { a + b }\nfn main() -> i64 { let z = add(1, 2); z }\n",
    "fn main() -> i64 { let mut s = 0; s }\n",
    "fn main() -> i64 { let mut acc: i64 = 0; let n: i64 = 5; acc + n }\n",
    // `if`/blocks + bool conditions; both branches share the result type.
    "fn pick(c: bool, x: i64, y: i64) -> i64 { if c { x } else { y } }\nfn main() -> i64 { pick(true, 1, 2) }\n",
    "fn main() -> i64 { let r = if 1 < 2 { 10 } else { 20 }; r }\n",
    "fn classify(n: i64) -> i64 { if n < 0 { 0 - 1 } else if n > 0 { 1 } else { 0 } }\nfn main() -> i64 { classify(5) }\n",
    // logical operators -> bool.
    "fn both(a: bool, b: bool) -> bool { a && b }\nfn main() -> i64 { 0 }\n",
    "fn main() -> i64 { let t = true || false; if t { 1 } else { 0 } }\n",
    // (4c-1) structs: decl head (with structural field types), struct-lit
    // (positional values typed `:StructName`), field access (with `field_index`).
    "struct Box { v: i64 }\nfn main() -> i64 { let b = Box { v: 42 }; b.v }\n",
    "struct Tagged { value: i64, valid: bool }\nfn main() -> i64 { let t = Tagged { value: 5, valid: true }; if t.valid { t.value } else { 0 } }\n",
    "struct Inner { x: i64 }\nstruct Outer { inner: Inner, y: i64 }\nfn main() -> i64 { let o = Outer { inner: Inner { x: 7 }, y: 3 }; o.inner.x + o.y }\n",
    "struct Point { x: i64, y: i64 }\nfn manhattan(p: Point) -> i64 { p.x + p.y }\nfn main() -> i64 { let p = Point { x: 3, y: 4 }; manhattan(p) }\n",
    // (4c-2) arrays: literal (typed `:[T]`), postfix index (`:T`), and the
    // generic `len` builtin (with its inferred `(targs T)`).
    "fn main() -> i64 { let a = [10, 20, 30]; a[1] + len(a) }\n",
    "fn sum_two(a: [i64]) -> i64 { a[0] + a[1] }\nfn main() -> i64 { sum_two([5, 6]) }\n",
    // (4c-3) nullable: `?T` annotation, `null` literal (typed from context), the
    // implicit `T → ?T` widening (`widen-null`), `is_some` / `unwrap_or`, and
    // `x == null` (the operand-type-sharing cmp).
    "fn main() -> i64 { let x: ?i64 = null; if is_some(x) { 1 } else { 0 } }\n",
    "fn main() -> i64 { let x: ?i64 = 42; unwrap_or(x, 0) }\n",
    "fn main() -> i64 { let x: ?i64 = null; if x == null { 1 } else { 0 } }\n",
    // widening at a struct-field boundary (`?i64` field, `i64` literal value) +
    // nullable-field access.
    "struct P { a: ?i64, b: i64 }\nfn main() -> i64 { let p = P { a: 5, b: 8 }; unwrap_or(p.a, 0) + p.b }\n",
    // widening through a `?T`-returning fn (the if-branch / block-tail threading):
    // the `* 2` branch widens `i64 → ?i64`, the `null` branch types from the return.
    "fn md(x: ?i64) -> ?i64 { if is_some(x) { unwrap_or(x, 0) * 2 } else { null } }\nfn main() -> i64 { let v: ?i64 = 5; unwrap_or(md(v), 0) }\n",
    "fn first(x: ?i64) -> ?i64 { x }\nfn main() -> i64 { let n: ?i64 = null; unwrap_or(first(n), 7) }\n",
];

#[test]
fn sentinel_typer_matches_oracle_on_seeds() {
    let tmp = std::env::temp_dir().join(format!("snc_selfhost_types_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(&tmp).expect("create temp dir");
    let typer = build_sentinel_typer(&tmp);

    let work = tmp.join("work");
    std::fs::create_dir_all(&work).expect("create work dir");
    let input = work.join("input.sentinel");

    let mut mismatches: Vec<String> = Vec::new();
    for seed in SEEDS {
        std::fs::write(&input, seed).expect("stage seed");

        let oracle = Command::new(env!("CARGO_BIN_EXE_snc"))
            .arg("types")
            .arg(&input)
            .output()
            .expect("run snc types");
        let sentinel = Command::new(&typer)
            .current_dir(&work)
            .output()
            .expect("run the Sentinel typer");

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
        "the Sentinel typer diverged from `snc types` on {}/{} seed(s):\n{}",
        mismatches.len(),
        SEEDS.len(),
        mismatches.join("\n")
    );
}
