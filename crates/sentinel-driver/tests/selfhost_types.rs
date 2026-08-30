//! Phase D self-host port (4/N) / ADR 0041 D9: the types differential test.
//! Compile `selfhost/types.sentinel` (which `use`s `selfhost/parser.sentinel` as a
//! D.6 module) with the Rust `snc`, then assert its canonical typed-AST dump is
//! byte-identical to `snc types` for a seed set of programs.
//!
//! (4b) the SCALAR skeleton + (4c) COMPOUND types. Paramful `fn`s over the scalar
//! grammar (int/bool literals, var refs, unary, binop/cmp/logic, `if`, blocks, `let`
//! with inference, calls) PLUS structs (struct-lit + field access), arrays (literal +
//! index + the generic `len`), (4c-3) nullable (`?T` + `null` + the implicit
//! `T → ?T` widening via expected-type threading through `dump_texpr`), and (4d)
//! secret (`secret T` + `declassify` + the `T → secret T` widening + the
//! secret-preserving operators), (4e) enums + match (variant construction with
//! `variant_index`, `match` with payload binding types + the wildcard), and (4f)
//! classes/traits/impls (the receiver-typed method-dispatch split, class-init, `self`
//! typing, named-impl qualified calls, `&`/`&mut`/`*` ref typing), and (4g)
//! effects/handlers (`perform`, `handle` arms, resume-kont, the effect-op-param VarId
//! offset), and (4h) generics (the `TypeParam` / `GenericInstance` interner kinds, a
//! per-decl type-param scope rendering `<T#i>`, `unify_one` bidirectional inference at
//! a generic-fn call emitting `(targs …)` + the substituted return type, and
//! substitution on a generic-instance field access) — each expression node annotated
//! with its inferred `Type` (a trailing ` :<type>`); the `let`'s inferred type replaces
//! resolve's `_`. The full-corpus differential (4i) is the phase-go (D9).

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

/// Stage `parser.sentinel` + `types.sentinel` into `tmp` (so `use parser::…`
/// resolves to the sibling file) and compile the entry `types.sentinel`.
fn build_sentinel_typer(tmp: &Path) -> PathBuf {
    let root = workspace_root();
    std::fs::copy(root.join("selfhost/parser.sentinel"), tmp.join("parser.sentinel"))
        .expect("stage parser.sentinel");
    stage_module_parts(&root, tmp, "parser");
    let entry = tmp.join("types.sentinel");
    std::fs::copy(root.join("selfhost/types.sentinel"), &entry).expect("stage types.sentinel");
    stage_module_parts(&root, tmp, "types");
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
    // (4d) secret: `secret T` annotation, the implicit `T → secret T` widening
    // (`widen-secret`), `declassify` (strips one secret layer), and the
    // secret-preserving operators (`secret + secret → secret`, `secret == secret →
    // secret bool`, `secret && secret → secret bool`, bitwise `^`).
    "fn unwrap(s: secret i64) -> i64 { declassify(s) }\nfn main() -> i64 { let p: secret i64 = 42; declassify(p) }\n",
    "fn add(a: secret i64, b: secret i64) -> secret i64 { a + b }\nfn main() -> i64 { let x: secret i64 = 1; let y: secret i64 = 2; declassify(add(x, y)) }\n",
    "fn eq(a: secret i64, b: secret i64) -> secret bool { a == b }\nfn main() -> i64 { let x: secret i64 = 5; if declassify(eq(x, x)) { 1 } else { 0 } }\n",
    "fn both(a: secret bool, b: secret bool) -> secret bool { a && b }\nfn main() -> i64 { 0 }\n",
    "fn main() -> i64 { let s: secret i64 = 7; let t: secret i64 = s ^ s; declassify(t) }\n",
    // (4e) enums + match: enum decl heads, `Enum::Variant(args)` construction (with
    // the computed `variant_index` + `:Enum` type), and `match` (the scrutinee's
    // EnumId, each arm's `variant_index`, payload `(bind #vid name ty)` from the
    // variant's declared payload types, the wildcard `_`, and the shared arm type).
    "enum Color { Red, Green, Blue }\nfn f(c: Color) -> i64 { match c { Color::Red => 1, Color::Green => 2, Color::Blue => 3 } }\nfn main() -> i64 { f(Color::Green) }\n",
    "enum Opt { None, Some(i64) }\nfn g(o: Opt) -> i64 { match o { Opt::None => 0, Opt::Some(x) => x } }\nfn main() -> i64 { g(Opt::Some(42)) }\n",
    "enum E { A, B, C }\nfn h(e: E) -> i64 { match e { E::A => 1, _ => 0 } }\nfn main() -> i64 { h(E::A) }\n",
    "enum Pair { P(i64, i64) }\nfn s(p: Pair) -> i64 { match p { Pair::P(a, b) => a + b } }\nfn main() -> i64 { s(Pair::P(3, 4)) }\n",
    "enum Tag { On(bool), Off }\nfn t(x: Tag) -> i64 { match x { Tag::On(b) => if b { 1 } else { 0 }, Tag::Off => 9 } }\nfn main() -> i64 { t(Tag::Off) }\n",
    // (4f) classes / traits / impls: class decl + `Name::init` (class-init) + the
    // receiver-typed method-dispatch split (a class's OWN method → `(method #cid …)`,
    // a default-impl method → `(impl-method #iid …)`), `self` typing + class field
    // access, named-impl qualified calls (`Impl::m(args)`), and `&`/`&mut`/`*` ref
    // typing.
    "class P { let x: i64; pub init(x: i64) { self.x = x; 0 } pub fn get(self: &Self) -> i64 { self.x } }\nfn main() -> i64 { let p: P = P::init(5); p.get() }\n",
    "trait Sh { fn m(self: &Self) -> i64; }\nclass C { let v: i64; pub init() { self.v = 7; 0 } }\nimpl as Sh for C { fn m(self: &Self) -> i64 { self.v } }\nfn main() -> i64 { let c: C = C::init(); c.m() }\n",
    "trait W { fn w(self: &mut Self, n: i64) -> i64; }\nclass B { let v: i64; pub init() { self.v = 0; 0 } }\nimpl Add as W for B { fn w(self: &mut Self, n: i64) -> i64 { self.v = self.v + n; self.v } }\nfn main() -> i64 { let mut b: B = B::init(); Add::w(&mut b, 9) }\n",
    "fn main() -> i64 { let x: i64 = 5; let r: &i64 = &x; *r }\n",
    // (4f-delegate) a `delegate f: T to Tr` synthesises a forwarding `impl _ as Tr for
    // C` whose method forwards to `self.f.m(args)` (a dispatch on the delegate field);
    // the synth impl's ImplId + VarIds continue after the user impls (group D).
    "trait W { fn w(self: &mut Self, n: i64) -> i64; }\nclass Inner { let v: i64; pub init() { self.v = 0; 0 } }\nimpl as W for Inner { fn w(self: &mut Self, n: i64) -> i64 { self.v = self.v + n; self.v } }\nclass Outer { delegate inner: Inner to W; pub init(i: Inner) { self.inner = i; 0 } }\nfn main() -> i64 { let x: Inner = Inner::init(); let mut o: Outer = Outer::init(x); o.w(42) }\n",
    // (4g) effects/handlers: an effect decl, `perform Eff.op(args)` (typed by the op's
    // return type), `handle … with { … }` (handler arms binding op params + the
    // continuation, the resume-kont `k(v)`, the optional `return v => …` arm), and the
    // effect-op-param VarId offset (every fn VarId shifts by the op-param count).
    "effect Io { read() -> i64; }\nfn main() -> i64 { handle perform Io.read() with { Io.read(k) => k(42) } }\n",
    "effect Io { write(msg: i64) -> i64; }\nfn w() -> i64 ! { Io } { perform Io.write(7) }\nfn main() -> i64 { handle w() with { Io.write(m, k) => k(m + 1) } }\n",
    "effect Io { read() -> i64; }\nfn r() -> i64 ! { Io } { perform Io.read() }\nfn main() -> i64 { handle r() with { Io.read(k) => k(0), return v => v * 2 } }\n",
    // (4h) generics: a generic fn (`<T>`) typed as `<T#0>` in its params/body/return,
    // type-arg inference at call sites (`(targs …)` + the substituted return), two
    // distinct instantiations of one fn, generic structs (`Box<T>` / `Pair<A,B>` →
    // a GenericInstance, with field-type substitution + struct-lit under bidirectional
    // context), `[T]` / `?T` type-param params (the generic builtins' targ is `<T#0>`),
    // and a second type-param whose substitution differs (`snd<A,B> -> B`).
    "fn id<T>(x: T) -> T { x }\nfn main() -> i64 { id(42) }\n",
    "fn pick<T>(c: bool, a: T, b: T) -> T { if c { a } else { b } }\nfn main() -> i64 { let n: i64 = pick(true, 1, 2); let bb: bool = pick(false, true, false); if bb { n } else { 0 } }\n",
    "struct Box<T> { value: T }\nfn main() -> i64 { let b: Box<i64> = Box { value: 7 }; b.value }\n",
    "struct Pair<A, B> { fst: A, snd: B }\nfn mk<A, B>(a: A, b: B) -> Pair<A, B> { Pair { fst: a, snd: b } }\nfn firstof(p: Pair<i64, i64>) -> i64 { p.fst }\nfn main() -> i64 { let p: Pair<i64, i64> = mk(3, 4); firstof(p) }\n",
    "fn count<T>(a: [T]) -> i64 { len(a) }\nfn main() -> i64 { count([1, 2, 3]) }\n",
    "fn first_or<T>(x: ?T, d: T) -> T { unwrap_or(x, d) }\nfn main() -> i64 { let a: ?i64 = 9; first_or(a, 0) }\n",
    "fn snd<A, B>(a: A, b: B) -> B { b }\nfn main() -> i64 { let r: bool = snd(1, true); if r { 1 } else { 0 } }\n",
    // (4i) char + string literals: `'c'` → `(char N :u8)` (the decoded byte), `\"…\"` →
    // `(str b0 b1 … :[u8])` (a string IS a `[u8]`), with char comparison, `len`, and
    // a string index + char-eq.
    "fn dig(c: u8) -> bool { c >= '0' }\nfn main() -> i64 { if dig('5') { 1 } else { 0 } }\n",
    "fn main() -> i64 { let s: [u8] = \"hello\"; len(s) }\n",
    "fn main() -> i64 { let s: [u8] = \"hi\"; if s[0] == 'h' { 1 } else { 0 } }\n",
    // (4i) Vec<T> (D.3 collections, interner kind 11 → `Vec<T>`): the generic builtins
    // vec_new (T from the expected type), push/pop (`&mut Vec<T>`), index + `len` over a
    // Vec, vec_to_array (`Vec<T> → [T]`), a Vec returned from a fn, the `String` =
    // `Vec<u8>` alias, and `read_file → [u8]`.
    "fn main() -> i64 { let mut v: Vec<i64> = vec_new(); push(&mut v, 5); push(&mut v, 7); let a: i64 = v[0]; let p: i64 = pop(&mut v); let n: i64 = len(v); a + p + n }\n",
    "fn build() -> Vec<i64> { let mut v: Vec<i64> = vec_new(); push(&mut v, 1); v }\nfn main() -> i64 { let w: Vec<i64> = build(); len(w) }\n",
    "fn main() -> i64 { let mut s: String = vec_new(); push(&mut s, 'h'); let a: [u8] = vec_to_array(s); len(a) }\n",
    "fn main() -> i64 { let d: [u8] = read_file(\"x\"); len(d) }\n",
    // (4i) concurrency (C4.4): `scope concurrent { … }` → the block's tail type,
    // `spawn e` → `Task<U>` (interner kind 12), `t.await` → the Task's element U.
    "fn dbl(x: i64) -> i64 ! { Async } { x * 2 }\nfn main() -> i64 { let r: i64 = scope concurrent { let t = spawn dbl(21); t.await }; r }\n",
    // (4i) a `delegate` field declared BEFORE a regular `let` field: the oracle orders
    // regular fields first (tag=0), delegate fields last (inner=1) regardless of source.
    "trait W { fn w(self: &Self) -> i64; }\nclass Inner { let v: i64; pub init() { self.v = 0; 0 } }\nimpl as W for Inner { fn w(self: &Self) -> i64 { self.v } }\nclass Outer { delegate inner: Inner to W; let tag: i64; pub init(i: Inner) { self.inner = i; self.tag = 7; 0 } pub fn t(self: &Self) -> i64 { self.tag } }\nfn main() -> i64 { 0 }\n",
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

/// (4i) the D9 PHASE-GO: the Sentinel typer matches `snc types` byte-for-byte over
/// the ENTIRE clean-typing corpus (every tests/pass + tests/ui fixture the oracle
/// accepts). Mirrors `sentinel_resolver_matches_oracle_on_corpus`. Fixtures the oracle
/// rejects (parse-/resolve-/type-error fixtures) are skipped — type-error parity is
/// out of scope (ADR 0041 D7), as with the parser/resolve corpus differentials.
#[test]
fn sentinel_typer_matches_oracle_on_corpus() {
    let tmp =
        std::env::temp_dir().join(format!("snc_selfhost_types_corpus_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(&tmp).expect("create temp dir");
    let typer = build_sentinel_typer(&tmp);

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
            .arg("types")
            .arg(&input)
            .output()
            .expect("run snc types");
        // Skip fixtures the oracle rejects (parse-/resolve-/type-error fixtures).
        if !oracle.status.success() {
            continue;
        }
        clean += 1;

        let sentinel = Command::new(&typer)
            .current_dir(&work)
            .output()
            .expect("run the Sentinel typer");

        if oracle.stdout != sentinel.stdout {
            mismatches.push(format!(
                "  {} (oracle {} bytes vs sentinel {} bytes)",
                fixture.file_name().unwrap().to_string_lossy(),
                oracle.stdout.len(),
                sentinel.stdout.len()
            ));
        }
    }

    assert!(clean > 100, "expected >100 clean-typing fixtures, got {clean}");
    assert!(
        mismatches.is_empty(),
        "the Sentinel typer diverged from `snc types` on {}/{} clean-typing fixture(s):\n{}",
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
    // ADR 0070 D3-revisit: a `Fn<T,R>`-typed local called DIRECTLY (`op(x)`).
    // scg's dispatch keeps ADR 0020 D5's "vars win over fns" unconditionally,
    // so it types the call as a kont RESUME (`(resume-kont #4 …)`) where the
    // oracle types the `apply` builtin (`(call #37 …)`). Already registered at
    // codegen (selfhost_codegen.rs); this test shows the divergence is present
    // three stages EARLIER, which is where a fix has to land.
    ("examples/lang/fn_value.sentinel", "ADR 0070 D3-revisit: a direct call of a Fn-typed var types as a kont resume in scg"),
    ("examples/lang/fn_value.sentinel (merged)", "ADR 0070 D3-revisit: a direct call of a Fn-typed var types as a kont resume in scg"),
    // TWO causes at once, and the reason both must be named: the float literal
    // (ADR 0058 — inherited from the parser) AND the ADR 0070 D3-revisit direct
    // call in `apply_bool`. scg additionally has no `f64` TYPE handle, so every
    // `f64` annotation renders `i64` and an unresolved operand renders `?T` —
    // the same missing handle documented at `selfhost/types/interner.sentinel`
    // for `Channel<f64>`.
    ("examples/lang/fn_value_generic.sentinel", "ADR 0058 (no FloatLit + no f64 type handle) AND ADR 0070 D3-revisit (`apply_bool` uses direct-call sugar)"),
    ("examples/lang/task_generic.sentinel", "ADR 0058: no FloatLit + no f64 type handle in scg (`f64` renders `i64`, unresolved renders `?T`)"),
    // ADR 0066 M2.3b: generic word-scalar elements for the PROCESS channel
    // (process_send/process_recv) are snc-only — scg types `process_recv` as
    // `?i64` where the oracle types `?u8` / `?i32`.
    ("examples/lang/process_channel_typed.sentinel", "ADR 0066 M2.3b: generic process-channel elements are snc-only (scg types process_recv `?i64`)"),
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
fn sentinel_typer_matches_oracle_on_real_programs() {
    let tmp = std::env::temp_dir().join(format!("snc_selfhost_types_prog_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(&tmp).expect("create temp dir");
    let typer = build_sentinel_typer(&tmp);
    let work = tmp.join("work");
    std::fs::create_dir_all(&work).expect("create work dir");
    // The semantic stages do no module discovery, so the MERGED form is what
    // supplies the multi-module programs (`delegation`, `rect_demo`,
    // `process_ids`, `sort_search`, …) — 9 direct + 10 merged.
    real_program_differential("types", &typer, &work, 9, 10);
    let _ = std::fs::remove_dir_all(&tmp);
}
