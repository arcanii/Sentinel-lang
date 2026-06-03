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
//! split (the enum table is checked FIRST). (3b-3) (A6): the effect table →
//! `(effect #id …)` heads + `perform` → `(perform #E op_index …)`; effect op
//! params consume global VarIds. (3b-4) (A7): the trait + impl tables →
//! `(trait #id …)` / `(impl #id … (method #selfvid …))` heads + the `qcall-impl`
//! split (method bodies bind a synthetic `self`; method-body VarIds are
//! GROUP-ordered — all fns before any impl). (3b-5) (A8): the class table →
//! `(class #id … (init #selfvid …)? (method …)…)` heads + `class-init` +
//! `resume-kont` — completing (3b) (every decl kind + every `::` form). (3c): the
//! SCOPED bodies — `match` arm-pattern payload bindings, `while`, and `handle`
//! (handler-arm params + the return arm) — with the D5 length-truncation
//! snapshot/restore (a name-blob scope + during-walk binding). (3c) closes the
//! whole expression/decl grammar. The full-corpus differential (3e) is the
//! phase-go (D9).

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
    // (3b-3): the effect table + `(effect #id Name (op name (params) ret)…)` heads
    // + `perform` → `(perform #E op_index Eff op args)`. The KEY subtlety: effect
    // op params consume GLOBAL VarIds in the Rust resolver (during effect-table
    // construction, before any fn body), so every fn's param/let VarIds are OFFSET
    // by the total effect-op-param count — regardless of source order. The Sentinel
    // side reproduces this with phantom scope slots. (No `handle` here — handler-arm
    // VarIds are (3c).)
    "effect State { get() -> i64; put(v: i64); }\nfn main() -> i64 { 0 }\n",
    "effect A { op1() -> i64; }\neffect B { op2(x: i64) -> i64; }\nfn f() -> i64 ! { B } { perform B.op2(7) }\nfn main() -> i64 { 0 }\n",
    "effect Log { emit(msg: i64) -> i64; }\nfn f(a: i64) -> i64 { a }\nfn main() -> i64 { f(3) }\n",
    "effect St { get() -> i64; put(v: i64); }\nfn g(x: i64) -> i64 { let y = x; y }\nfn main() -> i64 { g(1) }\n",
    "effect E { ping() -> i64; }\nfn h(a: i64) -> i64 { a }\nfn main() -> i64 { h(1) }\n",
    "fn f(a: i64) -> i64 { a }\neffect Log { emit(msg: i64) -> i64; }\nfn main() -> i64 { f(3) }\n",
    "fn p(x: i64) -> i64 { x }\neffect Ev { op(a: i64, b: i64); }\nfn q(y: i64) -> i64 { y }\nfn main() -> i64 { 0 }\n",
    "effect Two { a(x: i64) -> i64; b(y: i64) -> i64; }\nfn doit() -> i64 ! { Two } { perform Two.b(5) }\nfn main() -> i64 { 0 }\n",
    "struct S { n: i64 }\neffect Ef { go(s: i64) -> i64; }\nfn use_it(p: i64) -> i64 ! { Ef } { perform Ef.go(p) }\nfn main() -> i64 { 0 }\n",
    // (3b-4): the trait + impl tables + `(trait #id …)` heads (sigs, no bodies) +
    // `(impl #id name #trait_id Trait struct#tid Type (method #selfvid …))` heads
    // with method BODIES (synthetic `self` VarId) + `Impl::method(args)` →
    // `(qcall-impl #I method_index ImplName method args)`. THE KEY SUBTLETY:
    // method-body VarIds are GROUP-ordered (all fns resolve before any impl
    // method), so the impl-region VarIds start at `voff + total-fn-bindings` — a
    // fn AFTER an impl in source still gets the LOWER VarIds (the resolver does
    // explicit-VarId bookkeeping, not scope-index = VarId).
    "struct P { x: i64 }\ntrait Show { fn show(self: &Self) -> i64; }\nimpl Sh as Show for P { fn show(self: &Self) -> i64 { self.x } }\nfn main() -> i64 { let p = P { x: 5 }; Sh::show(p) }\n",
    "struct P { x: i64 }\nfn helper(a: i64) -> i64 { a }\ntrait T { fn m(self: &Self, y: i64) -> i64; }\nimpl I as T for P { fn m(self: &Self, y: i64) -> i64 { self.x + y } }\nfn main() -> i64 { 0 }\n",
    "struct P { x: i64 }\ntrait T { fn a(self: &Self) -> i64; fn b(self: &Self, n: i64) -> i64; }\nimpl I1 as T for P { fn a(self: &Self) -> i64 { self.x } fn b(self: &Self, n: i64) -> i64 { let t = n; self.x + t } }\nfn main() -> i64 { 0 }\n",
    "struct P { x: i64 }\ntrait T { fn a(self: &Self) -> i64; fn b(self: &Self) -> i64; }\nimpl I as T for P { fn a(self: &Self) -> i64 { 1 } fn b(self: &Self) -> i64 { 2 } }\nfn main() -> i64 { let p = P { x: 0 }; I::b(p) }\n",
    "trait Greet { fn hi(self: &Self) -> i64; }\nfn main() -> i64 { 0 }\n",
    "struct A { v: i64 }\nstruct B { w: i64 }\ntrait T { fn get(self: &Self) -> i64; }\nimpl IA as T for A { fn get(self: &Self) -> i64 { self.v } }\nimpl IB as T for B { fn get(self: &Self) -> i64 { self.w } }\nfn main() -> i64 { let a = A { v: 1 }; let b = B { w: 2 }; IA::get(a) + IB::get(b) }\n",
    "struct P { x: i64 }\ntrait T { fn m(self: &mut Self) -> i64; }\nimpl I as T for P { fn m(self: &mut Self) -> i64 { self.x } }\nfn main() -> i64 { 0 }\n",
    // (3b-5): the class table + `(class #id Name (field …) (init #selfvid …)?
    // (method #selfvid …)…)` heads (BUCKETED fields/init/methods, init+method
    // BODIES binding `self` — the init's is a synthetic sentinel) + `Name::init` →
    // `(class-init #C Name args)` + `resume-kont` (a let-bound name called). VarId
    // group order: fns, then classes (`classvid` from `voff + total-fn-bindings`),
    // then impls — verified by a fn + class + impl in one program.
    "class Counter { let n: i64; init(start: i64) { self.n = start; 0 } fn get(self: &Self) -> i64 { self.n } }\nfn main() -> i64 { let c = Counter::init(5); 0 }\n",
    "struct S { v: i64 }\nfn fnc(a: i64) -> i64 { a }\nclass C { let m: i64; init(p: i64) { self.m = p; 0 } fn go(self: &Self) -> i64 { self.m } }\ntrait T { fn t(self: &Self) -> i64; }\nimpl I as T for S { fn t(self: &Self) -> i64 { self.v } }\nfn main() -> i64 { 0 }\n",
    "class K { let z: i64; init() { self.z = 0; 0 } }\ntrait T { fn f(self: &Self) -> i64; }\nimpl I as T for K { fn f(self: &Self) -> i64 { self.z } }\nfn main() -> i64 { 0 }\n",
    "class Point { let x: i64; let y: i64; pub init(x: i64, y: i64) { self.x = x; self.y = y; 0 } pub fn manhattan(self: &Self) -> i64 { self.x + self.y } pub fn translate(self: &mut Self, dx: i64, dy: i64) -> i64 { self.x = self.x + dx; self.y = self.y + dy; self.manhattan() } }\nfn main() -> i64 { let mut p = Point::init(10, 20); p.translate(3, 9) }\n",
    "fn main() -> i64 { let k = 5; k(3) }\n",
    "class Two { let a: i64; let b: i64; init(x: i64) { self.a = x; self.b = x; 0 } fn sum(self: &Self) -> i64 { let s = self.a + self.b; s } }\nfn main() -> i64 { let t = Two::init(7); 0 }\n",
    // (3c) match + while: scoped bindings via length-truncation. match arm
    // payloads bind sequential VarIds (across arms) but each arm is popped after
    // (a payload name reused in two arms gets distinct VarIds). Bindings are
    // DURING-WALK: `let b = match …{E::A(x)=>x}` binds x (lower VarId) before b.
    "enum E { A(i64), B(i64, i64) }\nfn f(e: E) -> i64 { match e { E::A(x) => x, E::B(p, q) => p + q, _ => 0 } }\nfn main() -> i64 { 0 }\n",
    "enum E { A(i64) }\nfn f(e: E) -> i64 { let a = 1; let b = match e { E::A(x) => x, _ => 0 }; let c = 3; a + b + c }\nfn main() -> i64 { 0 }\n",
    "enum Opt { None, Some(i64) }\nfn unwrap(o: Opt) -> i64 { match o { Opt::Some(v) => v, Opt::None => 0 } }\nfn main() -> i64 { unwrap(Opt::Some(5)) }\n",
    "enum E { A(i64), B(i64, i64) }\nfn f(e: E) -> i64 { match e { E::A(x) => x, E::B(x, y) => x + y, _ => 0 } }\nfn main() -> i64 { 0 }\n",
    "fn f(n: i64) -> i64 { let mut i = 0; while i < n { let t = i; i = t + 1; } i }\nfn main() -> i64 { 0 }\n",
    // (3c) handle: handler-arm params + the return arm bind VarIds scoped to their
    // arm (truncated after) → `(arm #eid op_index Eff op (#vids) body)` /
    // `(return #vid body)`. (Needed a parser change: handler-arm params were
    // parsed-and-dropped; now stored as a Binds.) The return arm dumps last + its
    // VarId comes after the handler arms regardless of source order.
    "effect Log { emit(msg: i64) -> i64; }\nfn doit() -> i64 ! { Log } { perform Log.emit(42) }\nfn main() -> i64 { handle doit() with { Log.emit(m) => 0, return v => v } }\n",
    "effect St { get() -> i64; put(v: i64); }\nfn run() -> i64 ! { St } { perform St.get() }\nfn main() -> i64 { handle run() with { St.get() => 1, St.put(x) => 2, return r => r } }\n",
    "effect Log { emit(msg: i64) -> i64; }\nfn doit() -> i64 ! { Log } { perform Log.emit(1) }\nfn main() -> i64 { handle doit() with { return v => v, Log.emit(m) => m } }\n",
    "effect Log { emit(msg: i64) -> i64; }\nfn doit() -> i64 ! { Log } { perform Log.emit(1) }\nfn main() -> i64 { let a = 5; let b = handle doit() with { Log.emit(m) => m, return v => v }; a + b }\n",
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
