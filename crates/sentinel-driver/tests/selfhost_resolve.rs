//! Phase D self-host port (3/N) / ADR 0040 D9: the resolve differential test.
//! Compile `selfhost/resolve.sentinel` (which `use`s `selfhost/parser.sentinel`
//! as a D.6 module) with the Rust `snc`, then assert its canonical resolved-AST
//! dump is byte-identical to `snc resolve` for a seed set of programs.
//!
//! MILESTONE-1 (ADR 0040 A2): paramful fns + free calls + arithmetic +
//! comparisons + unary + variable references over the params (the body is a
//! block). VarIds are GLOBAL (never reset across fns); builtins occupy FnId
//! 0..=20 so user fns start at #21; the built-in `Async` effect dumps last.
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

/// ADR 0067: stage a multi-file module's `<module>/` parts dir alongside the
/// staged `<module>.sentinel`. A no-op if the module has no parts dir.
fn stage_module_parts(root: &Path, dst: &Path, module: &str) {
    let src = root.join("selfhost").join(module);
    if !src.is_dir() {
        return;
    }
    let pd = dst.join(module);
    std::fs::create_dir_all(&pd).expect("create parts dir");
    for ent in std::fs::read_dir(&src).expect("read parts dir") {
        let p = ent.expect("dir entry").path();
        if p.extension().and_then(|x| x.to_str()) == Some("sentinel") {
            std::fs::copy(&p, pd.join(p.file_name().unwrap())).expect("stage a part");
        }
    }
}

/// Stage `parser.sentinel` + `resolve.sentinel` into `tmp` (so `use parser::…`
/// resolves to the sibling file) and compile the entry `resolve.sentinel`.
fn build_sentinel_resolver(tmp: &Path) -> PathBuf {
    let root = workspace_root();
    std::fs::copy(root.join("selfhost/parser.sentinel"), tmp.join("parser.sentinel"))
        .expect("stage parser.sentinel");
    stage_module_parts(&root, tmp, "parser");
    let entry = tmp.join("resolve.sentinel");
    std::fs::copy(root.join("selfhost/resolve.sentinel"), &entry).expect("stage resolve.sentinel");
    stage_module_parts(&root, tmp, "resolve");
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
    // side reproduces this by starting the fn-region VarId counter at that offset
    // (`voff`). (No `handle` here — handler-arm VarIds are (3c).)
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

/// All `tests/pass` + `tests/ui` `.sentinel` fixtures (sorted).
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

/// ADR 0040 D9 — the phase-go: `selfhost/resolve.sentinel` matches `snc resolve`
/// byte-for-byte over **every** clean-RESOLVING `tests/pass` + `tests/ui` fixture
/// (including `delegate`-bearing classes — the synthesised forwarding impls).
/// Skipped only: fixtures the oracle rejects (parse-/resolve-error fixtures — the
/// Sentinel resolver mirrors happy-path resolution, like the parser test); `use`
/// fixtures are among those (the Rust resolve rejects a non-empty `uses` with
/// `UseDeclNotYet`).
#[test]
fn sentinel_resolver_matches_oracle_on_corpus() {
    let tmp =
        std::env::temp_dir().join(format!("snc_selfhost_resolve_corpus_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(&tmp).expect("create temp dir");
    let resolver = build_sentinel_resolver(&tmp);

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
            .arg("resolve")
            .arg(&input)
            .output()
            .expect("run snc resolve");
        // Skip fixtures the oracle rejects (parse-/resolve-error fixtures).
        if !oracle.status.success() {
            continue;
        }
        clean += 1;

        let sentinel = Command::new(&resolver)
            .current_dir(&work)
            .output()
            .expect("run the Sentinel resolver");

        if oracle.stdout != sentinel.stdout {
            mismatches.push(format!(
                "  {} (oracle {} bytes vs sentinel {} bytes)",
                fixture.file_name().unwrap().to_string_lossy(),
                oracle.stdout.len(),
                sentinel.stdout.len()
            ));
        }
    }

    assert!(clean > 100, "expected >100 clean-resolving fixtures, got {clean}");
    assert!(
        mismatches.is_empty(),
        "the Sentinel resolver diverged from `snc resolve` on {}/{} clean-resolving fixture(s):\n{}",
        mismatches.len(),
        clean,
        mismatches.join("\n")
    );
}

// ---------------------------------------------------------------------------
// The EXTENDED program differential: REAL programs, not just the curated
// single-file fixture corpus.
//
// `collect_fixtures` above sweeps only `tests/pass` + `tests/ui` — single-file
// fixtures written to exercise ONE construct each. Until this test, only
// `selfhost_codegen.rs` swept `examples/` + `sentinel_library/` + `tools/`,
// which left every UPSTREAM stage differential structurally blind to divergence
// in real programs. That is not hypothetical: when this test was written NOTHING
// in the fixture corpus used `export "C"`, a float literal, or any of the four
// reserved-name intrinsics (`sqrt` / `ptr_of` / `ptr_of_mut` / `is_null`) — so
// three separate unmirrored front-end surfaces had never once been compared, and
// one of them (`2.0`) is SILENTLY misparsed by the self-hosted parser as a field
// access. The same blind spot previously hid the ADR 0067 `module`/`part` lex
// gap. `export "C"` is no longer among them, and that IS the intended lifecycle:
// the sweep found the hole, the fix landed, and `tests/pass/c59_export_call`
// now pins it in the FIXTURE corpus. A float literal and the four reserved names
// remain fixture-uncovered — `snc lex` still reports zero `FloatLit` tokens
// across the whole corpus.
//
// TWO FORMS of each program are compared, because the stage oracles
// (`snc lex`/`ast`/`resolve`/`types`/`effects`/`borrow`/`mir`/`ctverify`) do NO
// module discovery — only `snc llvm` merges. A program with a `use` edge is
// rejected outright ("`use` imports are not yet wired"), so the direct form
// alone would leave the semantic stages seeing 9 of 116 programs:
//   (a) DIRECT — the program as written;
//   (b) MERGED — `snc merge`'s single-file collapse of its module graph, which
//       is exactly the shape the self-hosted `scg` consumes at the bootstrap
//       fixed point. This is what puts REAL multi-module programs
//       (`delegation`, `rect_demo`, `process_ids`, `sort_search`, …) through
//       the stage at all. It is not redundant at `lex`/`ast` either, where the
//       direct form already covers all 116: the merge qualifies every top-level
//       item as `module$path$item`, and `$` is an identifier-CONTINUATION
//       character only so that text lexes (ADR 0045 8g) — no file under
//       `examples/` + `sentinel_library/` + `tools/` and no fixture in
//       `tests/pass` + `tests/ui` contains a `$`, so these 20 comparisons are
//       the only place either lexer meets one outside the fixed point itself.
//
// Every divergence must be either FIXED or listed below with the ADR that
// defers it (`DEFERRED_PROGRAMS`) or its diagnosis (`KNOWN_SCG_BUGS`). The list
// IS the deliverable — it converts an invisible gap into an auditable one, and
// a listed program that starts matching FAILS the test, so the list cannot rot.
//
// ONE LIMITATION, stated because it is not obvious and is inherited from
// `selfhost_codegen.rs`: registration is keyed by PROGRAM PATH and is
// cause-BLIND. A listed program's byte-difference is excused wholesale, so a
// SECOND, unrelated divergence introduced into an already-listed file would NOT
// fail this test — though the identical construct added to any unlisted file
// does. What still fires for a listed program: a crash, the entry going stale
// by matching, and the entry never being reached. So keep the lists short,
// prefer fixing a listed program over letting it accumulate causes, and when a
// program does have several, NAME them all (`examples/math/quadratic.sentinel`
// carries two).

/// Programs whose divergence is a KNOWN, deliberately-deferred feature gap,
/// each with the ADR that defers it. A listed program is still COMPARED — the
/// self-hosted stage is RUN, so a crash still fails the test — but its
/// byte-difference is not a failure. Deleting an entry is how a mirror slice
/// records that it closed the gap; an entry the sweep never REACHES fails the
/// test too, because an unreached entry is as stale as a matching one.
///
/// THE LINE BETWEEN THE TWO LISTS, stated because `deferred_reason` chains
/// them: the classification changes NOTHING about whether the test passes, so
/// it is pure documentation — which is exactly why a reader has to be able to
/// apply it to a NEW divergence. A divergence is DEFERRED when reproducing the
/// oracle needs a shape `scg` does not have (a token, an AST node, a type
/// handle) that a fix would then have to thread through every downstream stage:
/// closing it is a MIRROR SLICE, and the ADR cited says the mirror is deferred.
/// It is a BUG when `scg` already has every shape involved and the divergence
/// is a HOLE in dispatch it has already ported: closing it is a FIX that
/// changes nothing downstream.
const DEFERRED_PROGRAMS: &[(&str, &str)] = &[
    // ADR 0058 floats are snc-only in the self-host. The divergence is INHERITED
    // from the parser (no `FloatLit`, so `2.0` parses as a field access — see
    // selfhost_parse.rs); resolve itself is faithful, and the `f64` TYPE
    // renders correctly here because the resolve dump prints type EXPRESSIONS
    // syntactically. Only the two programs with a float LITERAL diverge.
    ("examples/lang/fn_value_generic.sentinel", "ADR 0058: inherited from the parser — `2.0` resolves as `(field (int 2) 0)`"),
    ("examples/lang/task_generic.sentinel", "ADR 0058: inherited from the parser — `2.0` resolves as `(field (int 2) 0)`"),
];

/// Programs whose divergence is a REAL BUG in the self-hosted stage, not a
/// deferred feature — kept separate on purpose. Conflating "we chose not to
/// port this yet" with "this is wrong" is precisely the invisible-gap problem
/// this test exists to end, so an entry here must carry its DIAGNOSIS and is a
/// debt marker to be deleted by a fix, never by a re-label.
///
/// A BUG entry may still cite an ADR that DEFERS a mirror; that is not a
/// contradiction. ADR 0057 / 0059 defer the FEATURE mirror (typing, codegen,
/// `--lib`, the multi-module symbol policy), never the dump arm — the same
/// reason `selfhost/lexer.sentinel` already carries the `export` / `module` /
/// `part` keywords whose features are unported.
const KNOWN_SCG_BUGS: &[(&str, &str)] = &[];

/// The deferral reason for `key` (a repo-relative, forward-slashed path, with a
/// ` (merged)` suffix for the merged form) — `None` when the program is not
/// registered and must therefore match the oracle byte-for-byte.
fn deferred_reason(key: &str) -> Option<&'static str> {
    DEFERRED_PROGRAMS
        .iter()
        .chain(KNOWN_SCG_BUGS.iter())
        .find(|(p, _)| *p == key)
        .map(|(_, why)| *why)
}

/// Recursively collect `.sentinel` files under `dir`.
fn collect_under(dir: &Path, out: &mut Vec<PathBuf>) {
    if !dir.is_dir() {
        return;
    }
    for entry in std::fs::read_dir(dir).expect("read dir") {
        let path = entry.expect("dir entry").path();
        if path.is_dir() {
            collect_under(&path, out);
        } else if path.extension().and_then(|e| e.to_str()) == Some("sentinel") {
            out.push(path);
        }
    }
}

/// The real-program corpus: every `.sentinel` under `examples/`,
/// `sentinel_library/` and `tools/`. Programs the oracle rejects (a library
/// module with no `main`, an unwired `use`, a not-yet-ported construct) are
/// simply skipped, exactly as in the fixture differential above.
fn collect_programs() -> Vec<PathBuf> {
    let root = workspace_root();
    let mut out = Vec::new();
    for sub in ["examples", "sentinel_library", "tools"] {
        collect_under(&root.join(sub), &mut out);
    }
    out.sort();
    out
}

/// Copy `src`'s CONTENTS into `dst` recursively (so `sentinel_library/std`
/// lands at `<dst>/std`). Mirrors `selfhost_codegen.rs`'s staging, which is
/// what makes `use std::…` / `use Sentinel::…` resolve for `snc merge`: module
/// discovery roots at the entry file's parent directory.
fn copy_tree_contents(src: &Path, dst: &Path) {
    std::fs::create_dir_all(dst).expect("create dst");
    for entry in std::fs::read_dir(src).expect("read_dir") {
        let entry = entry.expect("dir entry");
        let from = entry.path();
        let to = dst.join(entry.file_name());
        if from.is_dir() {
            copy_tree_contents(&from, &to);
        } else {
            std::fs::copy(&from, &to).expect("copy file");
        }
    }
}

/// Compare `bin` (a compiled self-hosted stage, which reads `./input.sentinel`)
/// against `snc <oracle_cmd>` over every real program, in both the direct and
/// the merged form.
///
/// The two coverage guards exist so that a staging regression which silently
/// stops emitting fails loudly instead of passing vacuously, and they are
/// deliberately of different KINDS:
///   * `expect_direct` is EXACT. The direct form does no module discovery at
///     all, so the set of programs the oracle emits for is platform-invariant;
///     an exact count therefore catches a comparison disappearing even when it
///     was not registered (a floor with slack would absorb it). Growing the
///     corpus bumps this number — the same deliberate act `examples.rs` already
///     requires of a new example.
///   * `min_merged` is a FLOOR. `snc merge` selects target-conditional modules
///     through `host_target_os()` (ADR 0062), so in principle another host can
///     merge a different set. In practice it is exact: every ADR-0062
///     conditional module in the tree (`std/sys/random_unix` /
///     `random_windows`) fails merge-to-source identically on both, so the
///     floor is set to today's actual count.
fn real_program_differential(
    oracle_cmd: &str,
    bin: &Path,
    work: &Path,
    expect_direct: usize,
    min_merged: usize,
) {
    let root = workspace_root();
    // Stage the first-party libraries next to the entry so `use std::…` /
    // `use Sentinel::…` resolves when `snc merge` collapses the module graph.
    copy_tree_contents(&root.join("sentinel_library"), work);
    let input = work.join("input.sentinel");

    let programs = collect_programs();
    assert!(
        programs.len() > 50,
        "expected a substantial program corpus, got {}",
        programs.len()
    );

    let mut direct_emitted = 0usize;
    let mut merged_emitted = 0usize;
    let mut mismatches: Vec<String> = Vec::new();
    let mut stale: Vec<String> = Vec::new();
    let mut crashed: Vec<String> = Vec::new();
    let mut silent: Vec<String> = Vec::new();
    let mut reached: Vec<String> = Vec::new();
    for program in &programs {
        let rel = program
            .strip_prefix(&root)
            .expect("under root")
            .to_string_lossy()
            .replace('\\', "/");
        let bytes = std::fs::read(program).expect("read program");
        for merged_form in [false, true] {
            std::fs::write(&input, &bytes).expect("stage input");
            let key = if merged_form {
                format!("{rel} (merged)")
            } else {
                rel.clone()
            };
            if merged_form {
                let merged = Command::new(env!("CARGO_BIN_EXE_snc"))
                    .arg("merge")
                    .arg(&input)
                    .output()
                    .expect("run snc merge");
                // `snc merge`'s merge-to-source is a Bar-A subset printer, so a
                // program outside it simply has no merged form.
                if !merged.status.success() {
                    continue;
                }
                std::fs::write(&input, &merged.stdout).expect("stage the merged input");
            }
            let oracle = Command::new(env!("CARGO_BIN_EXE_snc"))
                .arg(oracle_cmd)
                .arg(&input)
                .output()
                .expect("run the oracle");
            if !oracle.status.success() {
                continue; // not in the emitted subset
            }
            if merged_form {
                merged_emitted += 1;
            } else {
                direct_emitted += 1;
            }
            reached.push(key.clone());
            let sentinel = Command::new(bin)
                .current_dir(work)
                .output()
                .expect("run the self-hosted stage");
            // Checked INDEPENDENTLY of byte-equality, and — unlike anything in
            // `selfhost_codegen` — NOT excused by registration: a registered
            // entry buys different BYTES, never a stage that aborts and never a
            // stage that emits NOTHING (the empty-output guard below).
            // `selfhost_codegen`'s llvm-validity check is the ancestor of both,
            // but note it EXEMPTS registered programs, which is the hole an
            // adversarial review demonstrated here: a plausible half-fix that
            // made a registered construct emit nothing would exit 0, differ
            // from the oracle, and be waved through. A text dump has no
            // `llvm-as` to validate its shape; non-emptiness is the part of
            // that check which does transfer.
            if !sentinel.status.success() {
                crashed.push(format!(
                    "  {key}: exit {:?} — {}",
                    sentinel.status.code(),
                    String::from_utf8_lossy(&sentinel.stderr)
                        .lines()
                        .next()
                        .unwrap_or("<no stderr>")
                ));
            }
            let registered = deferred_reason(&key).is_some();
            if oracle.stdout == sentinel.stdout {
                if registered {
                    stale.push(format!(
                        "  {key} now MATCHES the oracle — delete it from \
                         DEFERRED_PROGRAMS / KNOWN_SCG_BUGS"
                    ));
                }
                continue;
            }
            if registered {
                // A registration buys DIFFERENT bytes, never NO bytes. Every
                // registered key emits a non-empty dump today, and a change
                // that made one emit nothing would otherwise be invisible:
                // exit 0 (no crash), bytes differ (no staleness), key present
                // (no unreached), registered (no mismatch).
                if sentinel.stdout.is_empty() {
                    silent.push(format!(
                        "  {key}: the self-hosted stage emitted NOTHING (the oracle emitted {} bytes)",
                        oracle.stdout.len()
                    ));
                }
                continue; // a registered gap: an ADR-deferred feature or a tracked bug
            }
            mismatches.push(format!(
                "  {key} (oracle {} bytes vs sentinel {} bytes)",
                oracle.stdout.len(),
                sentinel.stdout.len()
            ));
        }
    }

    // An entry the sweep never REACHED is as stale as one that now matches, and
    // it fails in a way `stale` cannot see: when the oracle stops emitting for a
    // program the loop `continue`s BEFORE the comparison, so the entry is
    // neither exercised nor flagged and quietly becomes dead weight. Neither
    // count guard substitutes for it: `expect_direct` is exact but reports only
    // that a NUMBER moved (and a compensating corpus addition hides even that),
    // `min_merged` is a `>=` floor so a lost merged comparison can be absorbed
    // by a host that merges one more, and neither notices a registry key whose
    // PATH is simply wrong. This is the guard that names the dead entry.
    let unreached: Vec<String> = DEFERRED_PROGRAMS
        .iter()
        .chain(KNOWN_SCG_BUGS.iter())
        .filter(|(p, _)| !reached.iter().any(|r| r == p))
        .map(|(p, _)| format!("  {p} was never compared (the oracle no longer emits for it, or the path is wrong)"))
        .collect();
    assert!(
        unreached.is_empty(),
        "{} registered program(s) were never reached by the sweep — an unreached \
         entry is as stale as a matching one:\n{}",
        unreached.len(),
        unreached.join("\n")
    );
    assert_eq!(
        direct_emitted, expect_direct,
        "the DIRECT-form comparison count changed ({direct_emitted} vs the expected \
         {expect_direct}) — the direct form does no module discovery, so this is \
         platform-invariant: either a program stopped being emitted (a regression, \
         even for an unregistered one) or the corpus grew (bump the number)"
    );
    assert!(
        merged_emitted >= min_merged,
        "the MERGED-form comparison count fell to {merged_emitted}, below the \
         floor of {min_merged} — `snc merge` stopped collapsing programs it used to"
    );
    assert!(
        crashed.is_empty(),
        "the self-hosted stage exited nonzero on {} real program(s) — a registered \
         byte-divergence excuses different bytes, never a crash:\n{}",
        crashed.len(),
        crashed.join("\n")
    );
    assert!(
        silent.is_empty(),
        "the self-hosted stage emitted EMPTY output for {} REGISTERED program(s) — a \
         registered byte-divergence excuses different bytes, never no bytes:\n{}",
        silent.len(),
        silent.join("\n")
    );
    assert!(
        stale.is_empty(),
        "DEFERRED_PROGRAMS / KNOWN_SCG_BUGS is stale:\n{}",
        stale.join("\n")
    );
    assert!(
        mismatches.is_empty(),
        "the self-hosted stage diverged from `snc {oracle_cmd}` on {}/{} emitted \
         real program(s) NOT registered as deferred:\n{}",
        mismatches.len(),
        direct_emitted + merged_emitted,
        mismatches.join("\n")
    );
}

#[test]
fn sentinel_resolver_matches_oracle_on_real_programs() {
    let tmp =
        std::env::temp_dir().join(format!("snc_selfhost_resolve_prog_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(&tmp).expect("create temp dir");
    let resolver = build_sentinel_resolver(&tmp);
    let work = tmp.join("work");
    std::fs::create_dir_all(&work).expect("create work dir");
    // The semantic stages do no module discovery, so the MERGED form is what
    // supplies the multi-module programs (`delegation`, `rect_demo`,
    // `process_ids`, `sort_search`, …) — 9 direct + 10 merged.
    real_program_differential("resolve", &resolver, &work, 9, 10);
    let _ = std::fs::remove_dir_all(&tmp);
}
