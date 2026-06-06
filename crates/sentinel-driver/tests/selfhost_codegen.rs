//! Phase D self-host port (8/N) / ADR 0045 D11: the codegen differential test.
//! Compile `selfhost/codegen.sentinel` (which REUSES `selfhost/types.sentinel` via a
//! D.6 `use`, mode 4 — which in turn `use`s the parser, a 3-deep module chain) with
//! the Rust `snc`, then assert its emitted textual LLVM IR (`.ll`) is byte-identical
//! to the `snc llvm` oracle.
//!
//! (8a) covers the STRAIGHT-LINE subset (const, var, unary neg/not, binary and cmp,
//! `let`, assign-to-var, value-block, user-fn calls, the u8<->i64 width builtins);
//! (8b) adds control flow (if/else, while/break/continue, &&/||); (8c-1) adds
//! STRUCTS (Pass-0 `%Struct.N` type decls, `insertvalue` literals, `extractvalue`
//! field reads, pass-by-value params/returns); (8c-2) adds ARRAYS (`[T]` =
//! `{ i64, ptr }`, heap-alloc literals via `sentinel_alloc`, `a[i]` with a
//! `sentinel_panic_oob` bounds-check, `len`) — all alloca/load/store, NO phi. The
//! oracle is PARTIAL-by-Err (it Errs + exits nonzero on
//! a not-yet-ported construct), so the differential skips those fixtures, exactly as it
//! skips upstream parse/resolve/type rejects; the supported subset grows per sub-slice
//! (8b..8l). Behavioural correctness of the `.ll` is covered by `tests/llvm.rs` (the
//! oracle compiles + runs identically to inkwell); this test asserts the Sentinel side
//! reproduces the oracle's bytes.

use std::path::{Path, PathBuf};
use std::process::Command;

fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .canonicalize()
        .expect("canonicalize workspace root")
}

/// Stage `parser.sentinel` + `types.sentinel` + `codegen.sentinel` into `tmp` (so the
/// `use parser::…` / `use types::…` edges resolve) and compile the entry.
fn build_sentinel_codegen(tmp: &Path) -> PathBuf {
    let root = workspace_root();
    std::fs::copy(root.join("selfhost/parser.sentinel"), tmp.join("parser.sentinel"))
        .expect("stage parser.sentinel");
    std::fs::copy(root.join("selfhost/types.sentinel"), tmp.join("types.sentinel"))
        .expect("stage types.sentinel");
    let entry = tmp.join("codegen.sentinel");
    std::fs::copy(root.join("selfhost/codegen.sentinel"), &entry).expect("stage codegen.sentinel");
    let bin = tmp.join("scg");
    let out = Command::new(env!("CARGO_BIN_EXE_snc"))
        .arg("build")
        .arg(&entry)
        .arg("-o")
        .arg(&bin)
        .output()
        .expect("run snc build");
    assert!(
        out.status.success(),
        "compiling selfhost/codegen.sentinel failed:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    bin
}

/// Straight-line seeds (8a): const + main-trunc, params + arith + call, a `let` chain,
/// cmp + unary-not + bool, unary negate, bitwise ops, and mut + assign-to-var.
const SEEDS: &[&str] = &[
    "fn main() -> i64 { 42 }\n",
    "fn add(a: i64, b: i64) -> i64 { a + b }\nfn main() -> i64 { add(20, 22) }\n",
    "fn compute(x: i64) -> i64 { let y: i64 = x * 3; let z: i64 = y - 4; z }\nfn main() -> i64 { compute(16) }\n",
    "fn f(a: i64, b: i64) -> bool { let c: bool = a < b; !c }\nfn main() -> i64 { 0 }\n",
    "fn neg(x: i64) -> i64 { -x }\nfn main() -> i64 { 0 }\n",
    "fn bits(a: i64, b: i64) -> i64 { a & b | (a ^ b) }\nfn main() -> i64 { 0 }\n",
    "fn m() -> i64 { let mut x: i64 = 1; x = x + 41; x }\nfn main() -> i64 { m() }\n",
    // (8b) control flow: if/else, a while loop, &&/||, and a conditional break.
    "fn pick(c: bool, a: i64, b: i64) -> i64 { if c { a } else { b } }\nfn main() -> i64 { pick(true, 7, 9) }\n",
    "fn nested(a: bool, b: bool) -> i64 { if a { if b { 1 } else { 2 } } else { 3 } }\nfn main() -> i64 { 0 }\n",
    "fn sum(n: i64) -> i64 { let mut i: i64 = 0; let mut s: i64 = 0; while i < n { i = i + 1; s = s + i; } s }\nfn main() -> i64 { sum(5) }\n",
    "fn t(a: bool, b: bool) -> bool { a && b || a }\nfn main() -> i64 { 0 }\n",
    "fn brk(n: i64) -> i64 { let mut i: i64 = 0; while i < 100 { i = i + 1; if i == n { break; 0 } else { 0 }; } i }\nfn main() -> i64 { brk(7) }\n",
    "fn cont(n: i64) -> i64 { let mut i: i64 = 0; let mut s: i64 = 0; while i < n { i = i + 1; if i == 2 { continue; 0 } else { 0 }; s = s + i; } s }\nfn main() -> i64 { cont(4) }\n",
    // (8c-1) aggregates — structs: Pass-0 `%Struct.N` decls, `insertvalue` literals,
    // `extractvalue` field reads, pass-by-value params/returns (alloca/store/load).
    "struct Point { x: i64, y: i64 }\nfn dist(p: Point) -> i64 { p.x + p.y }\nfn main() -> i64 { let p = Point { x: 30, y: 12 }; dist(p) }\n",
    "struct Inner { x: i64 }\nstruct Outer { inner: Inner, y: i64 }\nfn main() -> i64 { let o = Outer { inner: Inner { x: 7 }, y: 3 }; o.inner.x + o.y }\n",
    "struct Tagged { value: i64, valid: bool, count: i64 }\nfn main() -> i64 { let t = Tagged { value: 5, valid: true, count: 9 }; if t.valid { t.value + t.count } else { 0 } }\n",
    "struct Pair { a: i64, b: i64 }\nfn pick(c: bool) -> Pair { if c { Pair { a: 1, b: 2 } } else { Pair { a: 10, b: 20 } } }\nfn main() -> i64 { let p = pick(true); p.a + p.b }\n",
    // (8c-2) aggregates — arrays: `{ i64, ptr }`, heap alloc + GEP-stores +
    // insertvalue; `a[i]` bounds-check + GEP+load; `len` = extractvalue 0; the
    // module declares only the runtime symbols actually used.
    "fn main() -> i64 { let xs = [10, 20, 30]; xs[1] + len(xs) }\n",
    "fn first(xs: [i64]) -> i64 { xs[0] }\nfn main() -> i64 { first([7, 8, 9]) }\n",
    "fn main() -> i64 { let xs: [i64] = []; len(xs) }\n",
    // (8c-3) [u8]/string literals (heap-copied byte arrays — a u8 array literal of
    // constant bytes) + char literals (u8 constants).
    "fn main() -> i64 { let s: [u8] = \"hi\"; len(s) }\n",
    "fn main() -> i64 { let c: u8 = 'Z'; u8_to_i64(c) }\n",
    // (8d) runtime builtins: str_eq + print_bytes decompose a [u8] into (ptr, len)
    // and call the sentinel_* symbol. (read_file/write_file need a real file at
    // runtime → behaviourally covered by the c5d4_file_io corpus fixture, not a seed.)
    "fn eq(a: [u8], b: [u8]) -> bool { str_eq(a, b) }\nfn main() -> i64 { let x: [u8] = \"hi\"; let y: [u8] = \"hi\"; if eq(x, y) { 1 } else { 0 } }\n",
    "fn show(b: [u8]) -> i64 { print_bytes(b) }\nfn main() -> i64 { let s: [u8] = \"hi\"; show(s) }\n",
    // (8d-refs) &/&mut/*/*p=x: a ref is an opaque ptr; `&v` is v's slot (no
    // instruction), `*r` loads through it, `*x = v` stores through it.
    "fn add(a: &i64, b: &i64) -> i64 { *a + *b }\nfn main() -> i64 { let a: i64 = 10; let b: i64 = 32; add(&a, &b) }\n",
    "fn inc(x: &mut i64) -> i64 { let n: i64 = *x + 1; *x = n; *x }\nfn main() -> i64 { let mut a: i64 = 10; let r: i64 = inc(&mut a); a + r }\n",
    // (8d-Vec) Vec<T> = {len,cap,ptr}: vec_new (constant), push (the len==cap
    // realloc grow CFG via &mut Vec), v[i] (index, data = field 2), len, pop.
    "fn main() -> i64 { let mut v: Vec<i64> = vec_new(); push(&mut v, 10); push(&mut v, 20); let a: i64 = v[1]; let l: i64 = len(v); let p: i64 = pop(&mut v); a + l + p }\n",
    // (8d-Vec-2) vec_to_array(v): the Vec -> [T] bridge — extract len(0)/data(2),
    // sentinel_alloc(len*sizeof), llvm.memcpy the live prefix, build [T] {len,dest}.
    // A Vec<u8> bridge (elem = i8, the lexer/String use case) and a Vec<i64> bridge
    // (elem = i64, a wider GEP-sizeof stride).
    "fn main() -> i64 { let mut v: Vec<u8> = vec_new(); push(&mut v, 'h'); push(&mut v, 'i'); let a: [u8] = vec_to_array(v); len(a) }\n",
    "fn main() -> i64 { let mut v: Vec<i64> = vec_new(); push(&mut v, 10); push(&mut v, 20); push(&mut v, 30); let a: [i64] = vec_to_array(v); a[0] + a[2] }\n",
    // (8d-drops) scope-exit `sentinel_free`: a heap binding is freed in reverse decl
    // order at its block's exit, EXCEPT a moved-out one (consuming call) — the callee
    // frees its param. arr is moved into consume (no double-free); tmp drops at the
    // nested block; the [u8] param of show drops at show's exit.
    "fn consume(xs: [i64]) -> i64 { xs[0] }\nfn main() -> i64 { let arr: [i64] = [1, 2, 3]; let inner: i64 = { let tmp: [i64] = [4, 5]; tmp[0] }; consume(arr) + inner }\n",
    "fn show(b: [u8]) -> i64 { len(b) }\nfn main() -> i64 { let s: [u8] = \"hi\"; let v: Vec<i64> = vec_new(); show(s) + len(v) }\n",
    // (8d-drops-2) recursive struct-field drop: dropping the struct GEPs into each
    // heap-backed field and frees it (the [i64] field); scalar fields are skipped.
    "struct Box { data: [i64], tag: i64 }\nfn main() -> i64 { let b = Box { data: [9, 8], tag: 5 }; b.data[0] + b.tag }\n",
    // (8d-drops-3) loop-exit drops: a per-iteration heap binding is drained on the
    // break path (before branching to loop_after) AND on the fall-through (body end);
    // mutually exclusive blocks → freed once per runtime path.
    "fn main() -> i64 { let mut i: i64 = 0; let mut n: i64 = 0; while i < 3 { let s: [u8] = \"ab\"; n = n + len(s); if i == 1 { break; 0 } else { 0 }; i = i + 1; } n }\n",
    "fn main() -> i64 { let mut i: i64 = 0; let mut n: i64 = 0; while i < 4 { i = i + 1; let w: [u8] = \"xy\"; n = n + len(w); if (i / 2) * 2 != i { continue; 0 } else { 0 }; n = n + 1; } n }\n",
];

#[test]
fn sentinel_codegen_matches_oracle_on_seeds() {
    let tmp = std::env::temp_dir().join(format!("snc_selfhost_cg_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(&tmp).expect("create temp dir");
    let cg = build_sentinel_codegen(&tmp);

    let work = tmp.join("work");
    std::fs::create_dir_all(&work).expect("create work dir");
    let input = work.join("input.sentinel");

    let mut mismatches: Vec<String> = Vec::new();
    for seed in SEEDS {
        std::fs::write(&input, seed).expect("stage seed");
        let oracle = Command::new(env!("CARGO_BIN_EXE_snc"))
            .arg("llvm")
            .arg(&input)
            .output()
            .expect("run snc llvm");
        assert!(
            oracle.status.success(),
            "snc llvm rejected a straight-line seed:\n{seed}\n{}",
            String::from_utf8_lossy(&oracle.stderr)
        );
        let sentinel = Command::new(&cg)
            .current_dir(&work)
            .output()
            .expect("run the Sentinel codegen");
        if oracle.stdout != sentinel.stdout {
            mismatches.push(format!(
                "  seed {seed:?}\n    oracle:\n{}\n    sentinel:\n{}",
                String::from_utf8_lossy(&oracle.stdout),
                String::from_utf8_lossy(&sentinel.stdout)
            ));
        }
    }
    assert!(
        mismatches.is_empty(),
        "the Sentinel codegen diverged from `snc llvm` on {}/{} seed(s):\n{}",
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

/// (8a) the corpus differential: the Sentinel codegen matches `snc llvm` byte-for-byte
/// over every fixture the oracle EMITS (the straight-line subset at 8a — the oracle is
/// partial-by-Err, so fixtures using a not-yet-ported construct exit nonzero and are
/// skipped, as are upstream rejects). The emitting subset grows each sub-slice; the floor
/// guards against a regression that stops the Sentinel side from reproducing it.
#[test]
fn sentinel_codegen_matches_oracle_on_corpus() {
    let tmp =
        std::env::temp_dir().join(format!("snc_selfhost_cg_corpus_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(&tmp).expect("create temp dir");
    let cg = build_sentinel_codegen(&tmp);

    let work = tmp.join("work");
    std::fs::create_dir_all(&work).expect("create work dir");
    let input = work.join("input.sentinel");

    let fixtures = collect_fixtures();
    assert!(fixtures.len() > 100, "expected a substantial corpus, got {}", fixtures.len());

    let mut emitted = 0usize;
    let mut mismatches: Vec<String> = Vec::new();
    for fixture in &fixtures {
        let bytes = std::fs::read(fixture).expect("read fixture");
        std::fs::write(&input, &bytes).expect("stage input");
        let oracle = Command::new(env!("CARGO_BIN_EXE_snc"))
            .arg("llvm")
            .arg(&input)
            .output()
            .expect("run snc llvm");
        if !oracle.status.success() {
            continue; // not in the emitted subset (a deferred construct / upstream reject)
        }
        emitted += 1;
        let sentinel = Command::new(&cg)
            .current_dir(&work)
            .output()
            .expect("run the Sentinel codegen");
        if oracle.stdout != sentinel.stdout {
            mismatches.push(format!(
                "  {} (oracle {} bytes vs sentinel {} bytes)",
                fixture.file_name().unwrap().to_string_lossy(),
                oracle.stdout.len(),
                sentinel.stdout.len()
            ));
        }
    }

    assert!(
        emitted >= 15,
        "expected the straight-line subset (~16) to emit, got {emitted}"
    );
    assert!(
        mismatches.is_empty(),
        "the Sentinel codegen diverged from `snc llvm` on {}/{} emitted fixture(s):\n{}",
        mismatches.len(),
        emitted,
        mismatches.join("\n")
    );
}
