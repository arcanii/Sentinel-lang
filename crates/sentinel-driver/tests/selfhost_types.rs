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
