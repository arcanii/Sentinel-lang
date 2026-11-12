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

mod common;

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
    // Register D150 (ADR 0041 A15): a handler arm's parameters take the operation's
    // declared types (`u8`, `bool`, `secret i64`, an array, a struct; two operations,
    // one with two parameters), where the typer bound every one as `i64`.
    "effect Io { ask(b: u8) -> i64; }\nfn one() -> i64 ! { Io } { perform Io.ask(3 as u8) }\nfn main() -> i64 { handle one() with { Io.ask(b, k) => k(b as i64) } }\n",
    "effect Io { ask(flag: bool) -> i64; }\nfn one(f: bool) -> i64 ! { Io } { perform Io.ask(f) }\nfn main() -> i64 { handle one(true) with { Io.ask(flag, k) => if flag { k(1) } else { k(2) } } }\n",
    "effect Io { w(x: secret i64) -> i64; }\nfn body(s: secret i64) -> i64 ! { Io } { perform Io.w(s) }\nfn run(s: secret i64) -> i64 { handle body(s) with { Io.w(x, k) => { let y: secret i64 = x * 2; k(declassify(y)) } } }\nfn main() -> i64 { run(21) }\n",
    "effect S { put(xs: [i64]) -> i64; }\nfn one(v: [i64]) -> i64 ! { S } { perform S.put(v) }\nfn main() -> i64 { handle one([1, 2, 3]) with { S.put(xs, k) => k(len(xs)) } }\n",
    "struct P { a: i64, b: i64 }\neffect Io { w(p: P) -> i64; }\nfn one() -> i64 ! { Io } { perform Io.w(P { a: 1, b: 2 }) }\nfn main() -> i64 { handle one() with { Io.w(p, k) => k(p.a + p.b) } }\n",
    "effect Io { a(x: u8) -> i64; b(n: i64, s: secret i64) -> i64; }\nfn one(s: secret i64) -> i64 ! { Io } { perform Io.b(1, s) }\nfn main() -> i64 { handle one(5) with { Io.a(x, k) => k(x as i64), Io.b(n, s, k) => { let y: secret i64 = s + n; k(n) } } }\n",
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
    // (ADR 0041 A16) places the typer now types as the oracle does: an enum payload, a
    // `perform`'s argument and an array element with the type expected there (registers D118,
    // D132, D136), a `Vec<T>` a generic call or a generic instance's field gives, substituted
    // by the oracle's rule (D147; an enum, `f64` or `?i64` element, which keeps `Vec<T>`, is
    // refused at the call since ADR 0016 A3: see the refusal shapes), and a `handle` by its
    // `return` arm (D151).
    "struct S1 { a: i64 }\nenum H { A(?S1), B }\nfn main() -> i64 { let h = H::A(null); 0 }\n",
    "enum G { A(?i64), B }\nfn main() -> i64 { let g = G::A(40); 0 }\n",
    "effect Io { write(x: secret i64) -> i64; }\nfn w() -> i64 ! { Io } { perform Io.write(40) }\nfn main() -> i64 { handle w() with { Io.write(x, k) => k(1) } }\n",
    "struct Box<T> { v: T }\nfn main() -> i64 { let a: [Box<i64>] = [Box { v: 40 }]; a[0].v + 2 }\n",
    "fn pass<T>(v: Vec<T>) -> Vec<T> { v }\nfn main() -> i64 { let mut v: Vec<i64> = vec_new(); push(&mut v, 4); let a: [i64] = vec_to_array(pass(v)); a[0] }\n",
    "struct Stack<T> { items: Vec<T> }\nfn main() -> i64 { let mut v: Vec<i64> = vec_new(); push(&mut v, 3); let s: Stack<i64> = Stack { items: v }; let a: [i64] = vec_to_array(s.items); a[0] }\n",
    "fn main() -> i64 { let x: ?i64 = handle 5 with { return v => v }; 0 }\n",
    "struct Q { a: i64 }\nstruct B<T> { v: T }\nfn mk<T>(x: T) -> Vec<T> { vec_new() }\nfn pass<T>(v: Vec<T>) -> Vec<T> { v }\nfn main() -> i64 {\n    let c = mk(Q { a: 1 });\n    let s: secret i64 = 2;\n    let d = mk(s);\n    let h = pass(c);\n    let u: secret u8 = 7 as u8;\n    let i = mk(u);\n    0\n}\n",
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
// one of them, the float literal `2.0`, was SILENTLY misparsed by the
// self-hosted parser as a field access. The same blind spot previously hid the
// ADR 0067 `module`/`part` lex gap.
//
// ALL THREE are now closed, each by the intended lifecycle — the sweep found the
// hole, the fix landed, and a FIXTURE now pins it so the corpus cannot lose sight
// of it again: `export "C"` by `tests/pass/c59_export_call.sentinel`, the ADR 0058
// float literal (with `sqrt`, which A1 makes part of the same feature) by
// `tests/pass/c58_float_math.sentinel`, and ADR 0057's `ptr_of` / `ptr_of_mut` /
// `is_null` by `tests/pass/c57_ptr_of.sentinel`. That is the point of this test
// arriving at an empty or near-empty registry: the list was never the deliverable
// on its own — converting an invisible gap into an auditable one was, and an
// auditable gap is one somebody closes.
//
// TWO FORMS of each program are compared, because the stage oracles
// (`snc lex`/`ast`/`resolve`/`types`/`effects`/`borrow`/`mir`/`ctverify`) do NO
// module discovery — only `snc llvm` merges. A program with a `use` edge is
// rejected outright ("`use` imports are not yet wired"), so the direct form
// alone would leave the semantic stages seeing 11 of 119 programs:
//   (a) DIRECT — the program as written;
//   (b) MERGED — `snc merge`'s single-file collapse of its module graph, which
//       is exactly the shape the self-hosted `scg` consumes at the bootstrap
//       fixed point. This is what puts REAL multi-module programs
//       (`delegation`, `rect_demo`, `process_ids`, `sort_search`, …) through
//       the stage at all. It is not redundant at `lex`/`ast` either, where the
//       direct form already covers all 119: the merge qualifies every top-level
//       item as `module$path$item`, and `$` is an identifier-CONTINUATION
//       character only so that text lexes (ADR 0045 8g) — no file under
//       `demos/` + `examples/` + `sentinel_library/` + `tools/` and no fixture in
//       `tests/pass` + `tests/ui` contains a `$`, so these 21 comparisons are
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
// program does have several, NAME them all — and when a slice closes only ONE of
// them, EDIT the entry down rather than deleting it. (`quadratic.sentinel` was
// the worked example of a two-cause entry until the ADR 0058 mirror closed both
// of its causes at once; `fn_value_generic.sentinel` was the next, narrowed by
// that same slice from float+ADR-0070 down to ADR 0070 alone, until the ADR 0070
// D14 direct-call mirror closed that cause too.)

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
    // (ADR 0066 M2.3b / register D34) The process-channel entry that used to sit here
    // is GONE, deleted 2026-09-03 rather than re-labelled. `scg` now types and lowers
    // `process_recv` with a context-typed element, so this program is byte-identical to
    // the oracle at THIS stage too. ⚠ The entry existed in THREE lists (types, mir,
    // codegen); deleting one and running the suite is how the other two surfaced.
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

/// The real-program corpus: every `.sentinel` under `demos/`, `examples/`,
/// `sentinel_library/` and `tools/`. Programs the oracle rejects (a library
/// module with no `main`, an unwired `use`, a not-yet-ported construct) are
/// simply skipped, exactly as in the fixture differential above.
///
/// `demos/` was MISSING from this list until a review caught it, and the omission
/// is worth remembering because it is the same species of hole this whole test
/// exists to close — a directory of real programs nothing compared. It is small
/// (three Win32 FFI demos) but not redundant: `demos/win32/messagebox.sentinel`
/// is a SELF-CONTAINED single-file caller of `ptr_of`, so unlike the five
/// `sentinel_library/std/**` modules that also use the intrinsic — which have no
/// `main`, and so are rejected by every semantic oracle — it runs the whole
/// pipeline. Enumerating the roots by hand is what made the omission possible;
/// if a fifth root ever appears, it will need adding here too, in eight files.
fn collect_programs() -> Vec<PathBuf> {
    let root = workspace_root();
    let mut out = Vec::new();
    for sub in ["demos", "examples", "sentinel_library", "tools"] {
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
    // `process_ids`, `sort_search`, …) — 12 direct + 12 merged (the 12th of
    // each is `examples/lang/phantom_type_param.sentinel`, ADR 0016 A1 — it merges, so
    // the merged FLOOR moves with the direct count, per this helper's own
    // "the floor is set to today's actual count" rule).
    real_program_differential("types", &typer, &work, 13, 13);
    let _ = std::fs::remove_dir_all(&tmp);
}

/// ADR 0041 A14 (register D97): the self-hosted typer refuses what the oracle refuses,
/// for the refusals `scg` ports (`common::assert_refuses_what_the_oracle_refuses`).
#[test]
fn sentinel_typer_refuses_what_the_oracle_refuses() {
    let tmp = std::env::temp_dir()
        .join(format!("snc_selfhost_typer_refusals_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(&tmp).expect("create temp dir");
    let driver = build_sentinel_typer(&tmp);
    let checked = common::assert_refuses_what_the_oracle_refuses(&driver, &tmp.join("refusals"));
    assert!(checked >= 10, "expected at least ten pinned refusals, got {checked}");
}

/// ADR 0041 A17: the deref check reads a type handle out of a type's interner entry only for a
/// `secret` or a nullable; a struct's entry holds the struct's id, which is no handle. So a
/// deref of a value of the 120th struct is refused with the oracle's message rather than
/// read past the interner.
#[test]
fn sentinel_typer_refuses_a_deref_of_a_late_declared_struct() {
    let tmp = std::env::temp_dir()
        .join(format!("snc_selfhost_typer_late_struct_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(&tmp).expect("create temp dir");
    let typer = build_sentinel_typer(&tmp);
    let work = tmp.join("work");
    std::fs::create_dir_all(&work).expect("create work dir");
    let mut prog: String = (0..120).map(|i| format!("struct S{i} {{ a: i64 }}\n")).collect();
    prog.push_str("fn f(p: S119) -> i64 { *p }\nfn main() -> i64 { 0 }\n");
    let input = work.join("input.sentinel");
    std::fs::write(&input, &prog).expect("stage program");
    let want = "dereference of non-reference type `<struct#119>`";
    let oracle = Command::new(env!("CARGO_BIN_EXE_snc"))
        .arg("types")
        .arg(&input)
        .output()
        .expect("run snc types");
    assert!(
        String::from_utf8_lossy(&oracle.stderr).lines().any(|l| l.strip_prefix("snc: ") == Some(want)),
        "the oracle no longer refuses the program with {want:?}"
    );
    let out = Command::new(&typer).current_dir(&work).output().expect("run the Sentinel typer");
    assert_eq!(
        String::from_utf8_lossy(&out.stdout),
        format!("sentinel::types::deref_of_non_ref\nscg: {want}\n"),
        "the typer did not refuse the deref"
    );
    let _ = std::fs::remove_dir_all(&tmp);
}

/// ADR 0041 A14 (register D97): the self-hosted typer's refusals agree with the oracle's,
/// program by program, on the shapes each ported rule turns on: every `init`
/// definite-assignment case of the oracle's own unit tests (`sentinel-types`, ADR 0022 A3),
/// a struct literal whose fields are written out of declaration order, a nested field, a
/// reference to `self` or to a field, `delegate` fields and their order, the struct-literal
/// field-set refusals, an unknown field access, and `vec_to_array`'s element. For each, the
/// same outcome: both accept, or both refuse with the same message (the oracle's line after
/// `snc: `, the typer's after `scg: `). The few programs here with more than one refused
/// construct are ones where the two name the same one; in general they need not (register
/// D145), and only the verdict is the same.
#[test]
fn sentinel_typer_refusals_match_the_oracle_on_shapes() {
    const DA_CLASS: &str = "enum E { A, B(i64) }\nclass K { let e: E; let n: i64;\n pub init(c: bool, m: E) { @ }\n pub fn get(self: &Self) -> i64 { self.n } }\nfn main() -> i64 { 0 }";
    const DA_BODIES: &[&str] = &[
        "if c { self.e = E::A; 0 } else { 0 }; self.n = 1; 0",
        "let mut i = 0; while i < 1 { self.e = E::A; i = i + 1; } self.n = 1; 0",
        "match m { E::A => { self.e = E::A; 0 }, E::B(_) => 0 }; self.n = 1; 0",
        "let b = c && { self.e = E::A; true }; self.n = 1; 0",
        "scope concurrent { self.e = E::A; 0 }; self.n = 1; 0",
        "self.n = 1; { self.e = E::A; 0 }",
        "let x = self.n; self.e = E::A; self.n = x; 0",
        "if c { self.n = 1; 0 } else { 0 }; self.e = E::A; self.n = self.n + 1; 0",
        "self.n = 1; let x = self.get(); self.e = E::A; 0",
        "self.e = E::A; self.n = self.n + 1; 0",
        "let mut a = [0]; a[{ self.n = 1; 0 }] = self.n; self.e = E::A; 0",
        "let mut a = [0]; a[self.n] = { self.n = 0; 0 }; self.e = E::A; 0",
        "let mut a = [0]; a[{ self.n = 1; 0 }] = 5; self.e = E::A; 0",
        "let mut a = [0]; a[0] = { self.n = 1; 0 }; self.e = E::A; 0",
        "self.e = E::A; self.n = 1; let x = self.n; self.n = x + 1; 0",
        "if c { self.e = E::A; 0 } else { self.e = E::B(2); 0 }; self.n = 1; 0",
        "match m { E::A => { self.e = E::A; 0 }, E::B(x) => { self.e = E::B(x); 0 } }; self.n = 1; 0",
        "self.e = E::A; self.n = 1; let x = self.get(); self.n = x; 0",
        "self.e = E::A; self.n = 1; let mut i = 0; while i < 2 { self.n = self.n + i; i = i + 1; } 0",
        "match m { E::A => { self.e = E::A; 0 }, _ => { self.e = E::B(1); 0 } }; self.n = 1; 0",
        "let b = c || { self.e = E::A; true }; self.n = 1; 0",
        "while self.n < 1 { self.n = 1; } self.e = E::A; 0",
        "self.e = E::A; self.n = 1; self.n",
        "self.e = E::A; self.n + 0",
        "{ self.e = E::A; 0 }; self.n = 1; 0",
        "if { self.e = E::A; true } { self.n = 1; 0 } else { self.n = 2; 0 }; 0",
        "self.e = E::A; self.n = { self.n = 5; self.n + 1 }; 0",
        "self.e = E::A; if c { return 0 } else { 0 }; self.n = 1; 0",
    ];
    const OTHER: &[&str] = &[
        "class J { let xs: [i64];\n pub init() { self.xs[0] = 1; self.xs = [1]; 0 } }\nfn main() -> i64 { 0 }",
        "effect Io { read() -> i64; }\nclass K { let n: i64;\n pub init() { handle { self.n = 1; 0 } with { Io.read(k) => k(0) }; 0 } }\nfn main() -> i64 { 0 }",
        "effect Io { read() -> i64; }\nclass K { let n: i64;\n pub init() { let v = handle { perform Io.read() } with { Io.read(k) => 0, return r => { self.n = r; r } }; 0 } }\nfn main() -> i64 { 0 }",
        "fn f(x: i64) -> i64 { x }\nclass K { let n: i64; let m: i64;\n pub init() { let t = spawn f({ self.n = 1; 0 }); self.m = t.await; 0 } }\nfn main() -> i64 { 0 }",
        "struct P { a: i64, b: i64 }\nclass K { let n: i64; let p: P;\n pub init() { self.p = P { b: { self.n = 1; 0 }, a: self.n }; 0 } }\nfn main() -> i64 { 0 }",
        "struct P { a: i64, b: i64 }\nclass K { let n: i64; let p: P;\n pub init() { self.p = P { b: self.n, a: { self.n = 1; 0 } }; 0 } }\nfn main() -> i64 { 0 }",
        "struct I { x: i64 }\nclass K { let i: I; let n: i64;\n pub init() { self.n = self.i.x; self.i = I { x: 1 }; 0 } }\nfn main() -> i64 { 0 }",
        "struct I { x: i64 }\nclass K { let i: I;\n pub init() { self.i.x = 1; self.i = I { x: 2 }; 0 } }\nfn main() -> i64 { 0 }",
        "fn look(k: &K) -> i64 { 0 }\nclass K { let n: i64; let m: i64;\n pub init() { self.n = 1; let r = look(&self); self.m = 2; 0 } }\nfn main() -> i64 { 0 }",
        "fn look(k: &i64) -> i64 { 0 }\nclass K { let n: i64; let m: i64;\n pub init() { self.m = look(&self.n); self.n = 2; 0 } }\nfn main() -> i64 { 0 }",
        "class K { let v: [i64]; let n: i64;\n pub init() { self.n = self.v[0]; self.v = [1]; 0 } }\nfn main() -> i64 { 0 }",
        "class K { pub init() { 0 } }\nfn main() -> i64 { 0 }",
        "class K { let n: i64;\n pub init() { self.n = 1; 0 }\n pub fn get(self: &Self, c: bool) -> i64 { if c { return 5 } else { 0 }; self.n } }\nfn later(c: bool) -> i64 { if c { return 7 } else { 0 }; 1 }\nfn main() -> i64 { 0 }",
        "trait Writer { fn write(self: &mut Self, data: i64) -> i64; }\nclass FileSink { let count: i64;\n pub init() { self.count = 0; 0 } }\nimpl as Writer for FileSink { fn write(self: &mut Self, data: i64) -> i64 { self.count = self.count + data; self.count } }\nclass Logger { delegate writer: FileSink to Writer; let n: i64;\n pub init(w: FileSink) { self.n = 1; 0 } }\nfn main() -> i64 { 0 }",
        "trait Writer { fn write(self: &mut Self, data: i64) -> i64; }\nclass FileSink { let count: i64;\n pub init() { self.count = 0; 0 } }\nimpl as Writer for FileSink { fn write(self: &mut Self, data: i64) -> i64 { self.count = self.count + data; self.count } }\nclass Logger { delegate writer: FileSink to Writer; let n: i64;\n pub init(w: FileSink) { 0 } }\nfn main() -> i64 { 0 }",
        "trait Writer { fn write(self: &mut Self, data: i64) -> i64; }\nclass FileSink { let count: i64;\n pub init() { self.count = 0; 0 } }\nimpl as Writer for FileSink { fn write(self: &mut Self, data: i64) -> i64 { self.count = self.count + data; self.count } }\nclass Logger { delegate writer: FileSink to Writer; let n: i64;\n pub init(w: FileSink) { self.writer = w; let z = self.write(1); self.n = z; 0 } }\nfn main() -> i64 { 0 }",
        "struct P { lo: i64, hi: i64 }\nfn main() -> i64 { let p = P { lo: 1, lo: 2, hi: 3 }; p.lo }",
        "struct P { lo: i64, hi: i64 }\nfn main() -> i64 { let p = P { zzz: 1, lo: 1, lo: 2 }; p.lo }",
        "struct P { lo: i64, hi: i64 }\nfn main() -> i64 { let p = P { lo: 1, lo: 2, zzz: 3 }; p.lo }",
        "struct P { lo: i64, hi: i64 }\nfn main() -> i64 { let p = P { hi: 1 }; p.hi }",
        "struct B<T> { v: T, w: i64 }\nfn main() -> i64 { let b: B<i64> = B { v: 1 }; b.v }",
        "struct P { lo: i64, hi: i64 }\nfn main() -> i64 { let mut p = P { lo: 1, hi: 2 }; p.zzz = 5; p.lo }",
        "class K { let n: i64;\n pub init() { self.n = 1; 0 }\n pub fn get(self: &Self) -> i64 { self.zzz } }\nfn main() -> i64 { 0 }",
        "struct H { e: [i64] }\nfn main() -> i64 { let v: Vec<H> = vec_new(); let a = vec_to_array(v); 0 }",
        "fn main() -> i64 { let v: Vec<bool> = vec_new(); let a = vec_to_array(v); 0 }",
        "struct H { e: [i64] }\nstruct H2 { n: i64, h: H }\nfn main() -> i64 { let v: Vec<H2> = vec_new(); let a = vec_to_array(v); 0 }",
        "struct Q { a: i64, b: ?i64, c: secret i64 }\nfn main() -> i64 { let v: Vec<Q> = vec_new(); let a = vec_to_array(v); 0 }",
        "fn first<T>(v: Vec<T>) -> i64 { let a = vec_to_array(v); 0 }\nfn main() -> i64 { 0 }",
        // `vec_to_array` over a `Vec<T>` a generic call returns: both substitute `T` where it
        // can be a `Vec` element and refuse an element that is not plain (a struct holding an
        // array), and refuse the call where it cannot be (an enum, `f64`, `?i64`: ADR 0016 A3).
        "struct E2 { e: [i64] }\nfn pass<T>(v: Vec<T>) -> Vec<T> { v }\nfn main() -> i64 { let v: Vec<E2> = vec_new(); let a = vec_to_array(pass(v)); len(a) }",
        "enum E { A, B }\nfn mk<T>(x: T) -> Vec<T> { vec_new() }\nfn main() -> i64 { let a = vec_to_array(mk(E::A)); len(a) }",
        "fn mk<T>(x: T) -> Vec<T> { vec_new() }\nfn main() -> i64 { let b = mk(1.5); 0 }",
        "fn mk<T>(x: T) -> Vec<T> { vec_new() }\nfn main() -> i64 { let n: ?i64 = null; let e = mk(n); 0 }",
        // An `if` or `while` condition is a `bool`: a `secret bool` is `secret_branch`, in any
        // body, a method's and a handler arm's among them, and any other type a mismatch.
        "fn main() -> i64 { if 1 { 1 } else { 0 } }",
        "fn main() -> i64 { let mut n: i64 = 0; while n { n = n + 1; } n }",
        "fn main() -> i64 { let s: secret i64 = 5; if s { 1 } else { 0 } }",
        "fn main() -> i64 { let c: ?bool = null; if c { 1 } else { 0 } }",
        "fn main() -> i64 { let s: secret i64 = 5; let mut n: i64 = 0; while s > n { n = n + 1; } n }",
        "class K { let n: i64;\n pub init() { self.n = 1; 0 }\n pub fn pick(self: &Self, s: secret i64) -> i64 { if s > 5 { 1 } else { 2 } } }\nfn main() -> i64 { 0 }",
        "effect Q { q(s: secret i64) -> i64; }\nfn a() -> i64 ! { Q } { perform Q.q(7) }\nfn main() -> i64 { handle a() with { Q.q(s, k) => if s > 5 { k(1) } else { k(2) } } }",
        "effect Q { q(b: bool) -> i64; }\nfn a() -> i64 ! { Q } { perform Q.q(true) }\nfn main() -> i64 { handle a() with { Q.q(b, k) => if b { k(1) } else { k(2) } } }",
        "fn main() -> i64 { let s: secret i64 = 5; if declassify(s > 2) { 1 } else { 0 } }",
        // (ADR 0041 A16) a value that does not fit the place it flows to, at each kind of
        // place: a `let`, an assignment (to a variable, a field, an element, through a
        // reference), a `return` and a body's value, a struct field, an array element, a call's,
        // `apply`'s, a `Fn` value's, a method's, an `init`'s, an enum payload's, a `perform`'s and
        // a resume's argument, a builtin's, a generic fn's and a generic builtin's; an operand; an
        // `if`'s join; the arity of a call, a pattern and a construct; a call through a variable
        // that cannot be called; and a `handle` typed by its `return` arm and a resume by its
        // `handle` (registers D151 and D7), which several of these accepted shapes hold.
        "fn main() -> i64 { let b: bool = 5; if b { 1 } else { 0 } }",
        "fn f(x: i64) -> i64 { x }\nfn main() -> i64 { f(true) }",
        "fn f() -> i64 { true }\nfn main() -> i64 { f() }",
        "fn main() -> i64 { if 1 { 1 } else { 0 } }",
        "fn main() -> i64 { let mut n: i64 = 0; while 1 { n = n + 1; } n }",
        "fn main() -> i64 { 1 + true }",
        "fn main() -> i64 { let mut x: i64 = 0; x = true; x }",
        "fn f(x: i64) -> i64 { x }\nfn main() -> i64 { f(1, 2) }",
        "fn main() -> i64 { let b: u8 = 'a'; let x: i64 = b; x }",
        "struct P { v: i64 }\nfn main() -> i64 { let q: P = P { v: true }; q.v }",
        "fn br(x: i64) -> i64 { if x > 20 { 1 } else { 2 } }\nfn main() -> i64 { let s: secret i64 = 5; declassify(s) }",
        "fn main() -> i64 { let p: ?i64 = 5; if is_some(p) { 42 } else { 0 } }",
        "fn br(x: i64) -> i64 { if x > 20 { 1 } else { 2 } }\nfn sec() -> secret i64 { 42 }\nfn main() -> i64 { let s: secret i64 = sec(); br(s) }",
        "fn br(x: i64) -> i64 { if x > 20 { 1 } else { 2 } }\nfn sec() -> secret i64 { 42 }\nfn main() -> i64 { let s: secret i64 = sec(); let f: Fn<i64, i64> = br; apply(f, s) }",
        "fn br(x: i64) -> i64 { if x > 20 { 1 } else { 2 } }\nfn sec() -> secret i64 { 42 }\nfn main() -> i64 { let s: secret i64 = sec(); let f: Fn<i64, i64> = br; f(s) }",
        "fn br(x: i64) -> i64 { if x > 20 { 1 } else { 2 } }\nfn sec() -> secret i64 { 42 }\nfn main() -> i64 { let s: secret i64 = sec(); let p: i64 = s; br(p) }",
        "fn br(x: i64) -> i64 { if x > 20 { 1 } else { 2 } }\nfn sec() -> secret i64 { 42 }\nfn main() -> i64 { let s: secret i64 = sec(); let mut p: i64 = 0; p = s; br(p) }",
        "fn br(x: i64) -> i64 { if x > 20 { 1 } else { 2 } }\nfn sec() -> secret i64 { 42 }\nfn leak(s: secret i64) -> i64 { s }\nfn main() -> i64 { br(leak(sec())) }",
        "fn br(x: i64) -> i64 { if x > 20 { 1 } else { 2 } }\nfn sec() -> secret i64 { 42 }\nfn leak(s: secret i64) -> i64 { if true { return s } else { 0 } }\nfn main() -> i64 { br(leak(sec())) }",
        "fn br(x: i64) -> i64 { if x > 20 { 1 } else { 2 } }\nfn sec() -> secret i64 { 42 }\nstruct P { v: i64 }\nfn main() -> i64 { let s: secret i64 = sec(); let q: P = P { v: s }; br(q.v) }",
        "fn br(x: i64) -> i64 { if x > 20 { 1 } else { 2 } }\nfn sec() -> secret i64 { 42 }\nfn main() -> i64 { let s: secret i64 = sec(); let a: [i64] = [s, 1]; br(a[0]) }",
        "fn br(x: i64) -> i64 { if x > 20 { 1 } else { 2 } }\nfn sec() -> secret i64 { 42 }\neffect Ask { q() -> i64; }\nfn ask1() -> i64 ! { Ask } { perform Ask.q() }\nfn main() -> i64 { let s: secret i64 = sec(); let r: i64 = handle ask1() with { Ask.q(k) => k(s) }; br(r) }",
        "fn br(x: i64) -> i64 { if x > 20 { 1 } else { 2 } }\nfn sec() -> secret i64 { 42 }\neffect Log { put(x: i64) -> i64; }\nfn w(s: secret i64) -> i64 ! { Log } { perform Log.put(s) }\nfn main() -> i64 { handle w(sec()) with { Log.put(x, k) => k(br(x)) } }",
        "fn br(x: i64) -> i64 { if x > 20 { 1 } else { 2 } }\nfn sec() -> secret i64 { 42 }\nstruct P { v: i64 }\nfn main() -> i64 { let s: secret i64 = sec(); let mut q: P = P { v: 0 }; q.v = s; br(q.v) }",
        "fn br(x: i64) -> i64 { if x > 20 { 1 } else { 2 } }\nfn sec() -> secret i64 { 42 }\nfn main() -> i64 { let s: secret i64 = sec(); let mut a: [i64] = [0, 1]; a[0] = s; br(a[0]) }",
        "fn br(x: i64) -> i64 { if x > 20 { 1 } else { 2 } }\nfn sec() -> secret i64 { 42 }\nfn put(p: &mut i64, s: secret i64) -> i64 { *p = s; 0 }\nfn main() -> i64 { let mut x: i64 = 0; put(&mut x, sec()); br(x) }",
        "fn br(x: i64) -> i64 { if x > 20 { 1 } else { 2 } }\nfn sec() -> secret i64 { 42 }\nfn main() -> i64 { let s: secret i64 = sec(); let p: i64 = if true { s } else { 1 }; br(p) }",
        "fn br(x: i64) -> i64 { if x > 20 { 1 } else { 2 } }\nfn sec() -> secret i64 { 42 }\nclass C { let n: i64; pub init(n: i64) { self.n = n; 0 } pub fn get(self: &Self, x: i64) -> i64 { br(x) } }\nfn main() -> i64 { let c: C = C::init(1); c.get(sec()) }",
        "fn br(x: i64) -> i64 { if x > 20 { 1 } else { 2 } }\nfn sec() -> secret i64 { 42 }\nclass C { let n: i64; pub init(n: i64) { self.n = n; 0 } pub fn get(self: &Self, x: i64) -> i64 { br(x) } }\nfn main() -> i64 { let c: C = C::init(sec()); c.get(1) }",
        "fn br(x: i64) -> i64 { if x > 20 { 1 } else { 2 } }\nfn sec() -> secret i64 { 42 }\nenum E { A(i64), B }\nfn main() -> i64 { let e: E = E::A(sec()); match e { E::A(v) => br(v), E::B => 0 } }",
        "fn br(x: i64) -> i64 { if x > 20 { 1 } else { 2 } }\nfn sec() -> secret i64 { 42 }\nfn id<T>(x: T) -> T { x }\nfn main() -> i64 { let p: i64 = id(sec()); br(p) }",
        "fn br(x: i64) -> i64 { if x > 20 { 1 } else { 2 } }\nfn sec() -> secret i64 { 42 }\nfn main() -> i64 { let s: secret i64 = sec(); let p: ?i64 = s; if is_some(p) { 1 } else { 0 } }",
        "fn main() -> i64 { let o: ?i64 = 5; unwrap_or(o, true) }",
        "fn main() -> i64 { let mut v: Vec<i64> = vec_new(); push(&mut v, true); 0 }",
        "fn main() -> i64 { len(5) }",
        "fn main() -> i64 { let x: i64 = 5; push(x, 1); 0 }",
        "fn main() -> i64 { let n: i64 = 5; n(1) }",
        "fn main() -> i64 { apply(5, 1) }",
        "fn main() -> i64 { let s: secret i64 = 1; let mut v: Vec<i64> = vec_new(); push(&mut v, s); 0 }",
        "fn main() -> i64 { is_some(5) }",
        "enum E { A(i64), B }\nfn main() -> i64 { let e: E = E::A(1, 2); 0 }",
        "enum E { A(i64), B }\nfn main() -> i64 { let e: E = E::A(1); match e { E::A(x, y) => x, E::B => 0 } }",
        "enum E { A(i64, i64), B }\nfn main() -> i64 { let e: E = E::A(1, 2); match e { E::A(x) => x, E::B => 0 } }",
        "enum E { B, A(i64) }\nfn main() -> i64 { let e: E = E::A(1); match e { E::B => 0, E::A(x, y) => x } }",
        "effect Q { ask(x: i64) -> i64; }\nfn w() -> i64 ! { Q } { perform Q.ask(3) }\nfn main() -> i64 { if handle w() with { Q.ask(x, k) => k(x), return v => v > 2 } { 42 } else { 0 } }",
        "effect Q { ask(x: i64) -> secret i64; }\nfn w() -> i64 ! { Q } { let v: secret i64 = perform Q.ask(2); declassify(v) }\nfn main() -> i64 { handle w() with { Q.ask(x, k) => { let s: secret i64 = x; if k(s) > 1 { 42 } else { 0 } } } }",
        "effect Q { ask(x: i64) -> i64; }\nfn w() -> secret i64 ! { Q } { let v: i64 = perform Q.ask(2); v }\nfn main() -> i64 { if handle w() with { Q.ask(x, k) => k(x), return v => declassify(v) } > 1 { 42 } else { 0 } }",
        "effect Q { ask(x: i64) -> secret i64; }\nfn w() -> i64 ! { Q } { let v: secret i64 = perform Q.ask(2); declassify(v) }\nfn main() -> i64 { handle w() with { Q.ask(x, k) => { let s: secret i64 = x; let r: i64 = k(s); if r > 1 { 42 } else { 0 } } } }",
        "effect Q { ask(x: i64) -> i64; }\nfn w() -> secret i64 ! { Q } { let v: i64 = perform Q.ask(2); v }\nfn main() -> i64 { let r: secret i64 = handle w() with { Q.ask(x, k) => if k(x) > 1 { 42 } else { 0 } }; declassify(r) }",
        "effect Q { ask(x: i64) -> i64; }\nfn w() -> i64 ! { Q } { let v: i64 = perform Q.ask(2); v }\nfn main() -> i64 {\n  let r: secret i64 = handle w() with {\n    Q.ask(x, k) => if k(x) > 1 { 42 } else { 0 },\n    return v => { let s: secret i64 = v; s }\n  };\n  declassify(r)\n}",
        "fn f(s: secret bool) -> i64 { if !s { 1 } else { 0 } }\nfn main() -> i64 { 0 }",
        "fn f(s: secret i64) -> i64 { let mut n: i64 = 0; while !(s > n) { n = n + 1; } n }\nfn main() -> i64 { 0 }",
        "class K { let n: i64; pub init() { self.n = 1; 0 }\n  pub fn m(self: &Self, a: secret bool) -> i64 { if !a { 1 } else { 2 } } }\nfn main() -> i64 { 0 }",
        "fn f(a: secret bool, b: secret bool) -> secret bool { !a && b }\nfn main() -> i64 { 0 }",
        // A TCP socket builtin is typed as the oracle types it (ADR 0056), and a public byte
        // array widens to a secret one where the element widens (ADR 0051).
        "fn main() -> i64 { let b: [u8] = tcp_read(0, 4); len(b) }",
        "fn main() -> i64 { let a: [secret i32] = [1 as i32, 2 as i32]; len(a) }",
        "fn main() -> i64 { let b: i64 = tcp_read(0, 4); b }",
        // A `Vec<T>` returned by a generic call is substituted by the oracle's rule (register
        // D147's checking half): a `secret` element reaching a public `Vec` is refused, and a
        // struct element is accepted.
        "fn pass<T>(v: Vec<T>) -> Vec<T> { v }\nfn main() -> i64 { let sv: Vec<secret i64> = vec_new(); let p: Vec<i64> = pass(sv); 0 }",
        "fn pass<T>(v: Vec<T>) -> Vec<T> { v }\nfn w(sv: Vec<secret u8>) -> Vec<u8> { pass(sv) }\nfn main() -> i64 { 0 }",
        "fn pass<T>(v: Vec<T>) -> Vec<T> { v }\nfn main() -> i64 { let sv: Vec<secret i64> = vec_new(); let mut p: Vec<i64> = vec_new(); p = pass(sv); 0 }",
        "struct Q { a: i64 }\nfn pass<T>(v: Vec<T>) -> Vec<T> { v }\nfn w(v: Vec<Q>) -> Vec<Q> { pass(v) }\nfn main() -> i64 { 0 }",
        "fn pass<T>(v: Vec<T>) -> Vec<T> { v }\nfn main() -> i64 { let sv: Vec<secret i64> = vec_new(); let p: Vec<secret i64> = pass(sv); 0 }",
        // A `let`'s annotation has no type parameter in scope, as in the oracle: a `T` there is
        // an unknown type, unless a type of that name is declared (and `String` is the alias).
        "fn g<T>(x: T) -> i64 { let y: T = x; 0 }\nfn main() -> i64 { g(1) }",
        "fn g<T>(x: T) -> i64 { let v: Vec<T> = vec_new(); 0 }\nfn main() -> i64 { g(1) }",
        "struct B<T> { v: T }\nfn g<T>(x: T) -> i64 { let y: ?B<T> = null; 0 }\nfn main() -> i64 { g(1) }",
        "struct T { a: i64 }\nfn g<T>(x: T) -> i64 { let y: T = T { a: 1 }; 0 }\nfn main() -> i64 { 0 }",
        "fn g<String>(x: String) -> i64 { let y: String = vec_new(); 0 }\nfn main() -> i64 { 0 }",
        // A written `?X` over a type no nullable holds is `unknown type ?(nullable)`, wherever
        // it is written; a reference, a generic instance, `f64`, a channel and `?T` are held.
        "enum E { A, B }\nstruct Box<T> { v: T }\nclass K { let n: i64; pub init() { self.n = 1; 0 } }\nfn f(x: ?secret bool) -> i64 { 0 }\nfn main() -> i64 { 0 }",
        "enum E { A, B }\nstruct Box<T> { v: T }\nclass K { let n: i64; pub init() { self.n = 1; 0 } }\nfn f(x: ?[i64]) -> i64 { 0 }\nfn main() -> i64 { 0 }",
        "enum E { A, B }\nstruct Box<T> { v: T }\nclass K { let n: i64; pub init() { self.n = 1; 0 } }\nfn f(x: ?E) -> i64 { 0 }\nfn main() -> i64 { 0 }",
        "enum E { A, B }\nstruct Box<T> { v: T }\nclass K { let n: i64; pub init() { self.n = 1; 0 } }\nfn f(x: ?Vec<i64>) -> i64 { 0 }\nfn main() -> i64 { 0 }",
        "enum E { A, B }\nstruct Box<T> { v: T }\nclass K { let n: i64; pub init() { self.n = 1; 0 } }\nfn f(x: ?K) -> i64 { 0 }\nfn main() -> i64 { 0 }",
        "enum E { A, B }\nstruct Box<T> { v: T }\nclass K { let n: i64; pub init() { self.n = 1; 0 } }\nfn f(x: ?Fn<i64, i64>) -> i64 { 0 }\nfn main() -> i64 { 0 }",
        "enum E { A, B }\nstruct Box<T> { v: T }\nclass K { let n: i64; pub init() { self.n = 1; 0 } }\nfn f(x: ?Shared<i64>) -> i64 { 0 }\nfn main() -> i64 { 0 }",
        "enum E { A, B }\nstruct Box<T> { v: T }\nclass K { let n: i64; pub init() { self.n = 1; 0 } }\nfn f(x: ?String) -> i64 { 0 }\nfn main() -> i64 { 0 }",
        "enum E { A, B }\nstruct Box<T> { v: T }\nclass K { let n: i64; pub init() { self.n = 1; 0 } }\nfn f(x: ?&i64) -> i64 { 0 }\nfn main() -> i64 { 0 }",
        "enum E { A, B }\nstruct Box<T> { v: T }\nclass K { let n: i64; pub init() { self.n = 1; 0 } }\nfn f(x: ?Box<i64>) -> i64 { 0 }\nfn main() -> i64 { 0 }",
        "enum E { A, B }\nstruct Box<T> { v: T }\nclass K { let n: i64; pub init() { self.n = 1; 0 } }\nfn f(x: ?f64) -> i64 { 0 }\nfn main() -> i64 { 0 }",
        "enum E { A, B }\nstruct Box<T> { v: T }\nclass K { let n: i64; pub init() { self.n = 1; 0 } }\nfn f(x: ?Channel<i64>) -> i64 { 0 }\nfn main() -> i64 { 0 }",
        "fn f(s: secret bool) -> i64 { let o: ?secret bool = s; if o == null { 1 } else { 0 } }\nfn main() -> i64 { f(true) }",
        "fn f(a: [?secret i64]) -> i64 { 0 }\nfn main() -> i64 { 0 }",
        "enum E { A(?secret i64) }\nfn main() -> i64 { 0 }",
        "fn f<T>(x: ?T) -> i64 { 0 }\nfn main() -> i64 { 0 }",
        // An enum payload, a `perform`'s argument, a `resume`'s and an array literal's elements
        // are typed with the type expected there, as the oracle types them: a generic struct
        // literal takes its type arguments from it, and a `secret` field in it is refused.
        "struct B<T> { v: T }\nfn main() -> i64 { let a: [B<i64>] = [B { v: 3 }, B { v: 4 }]; 0 }",
        "struct B<T> { v: T }\nenum E { A(B<i64>) }\nfn main() -> i64 { let e = E::A(B { v: 3 }); 0 }",
        "struct B<T> { v: T }\neffect Q { q(b: B<i64>) -> i64; }\nfn a() -> i64 ! { Q } { perform Q.q(B { v: 3 }) }\nfn main() -> i64 { handle a() with { Q.q(b, k) => k(b.v) } }",
        "struct B<T> { v: T }\neffect Q { q(x: i64) -> B<i64>; }\nfn a() -> i64 ! { Q } { let b: B<i64> = perform Q.q(1); b.v }\nfn main() -> i64 { handle a() with { Q.q(x, k) => k(B { v: x }) } }",
        "struct B<T> { v: T }\nenum E { A(B<i64>) }\nfn main() -> i64 { let s: secret i64 = 5; let e = E::A(B { v: s }); 0 }",
        "struct B<T> { v: T }\neffect Q { q(b: B<i64>) -> i64; }\nfn a(s: secret i64) -> i64 ! { Q } { perform Q.q(B { v: s }) }\nfn main() -> i64 { 0 }",
        "struct B<T> { v: T }\nfn main() -> i64 { let s: secret i64 = 5; let a: [B<i64>] = [B { v: s }]; 0 }",
        "enum E { A(u8) }\nfn main() -> i64 { let e = E::A(5); 0 }",
        // A method's or an impl method's body, against its return type; an `if` or `match` with no
        // expected type, whose arms must agree; a `match` arm, a block, a `return` and a call's
        // argument coerced to the type expected there, so a mismatch inside them is reported where
        // the oracle reports it.
        "class K { let n: i64;\n pub init() { self.n = 1; 0 }\n pub fn get(self: &Self) -> bool { self.n } }\nfn main() -> i64 { 0 }",
        "class K { let n: i64;\n pub init() { self.n = 1; 0 }\n pub fn get(self: &Self, s: secret i64) -> i64 { s } }\nfn main() -> i64 { 0 }",
        "struct P { n: i64 }\ntrait T { fn eat(self: &Self, x: i64) -> i64; }\nimpl Direct as T for P { fn eat(self: &Self, x: i64) -> i64 { true } }\nfn main() -> i64 { 0 }",
        "fn main() -> i64 { let c: bool = true; let x = if c { 1 } else { true }; 0 }",
        "fn main() -> i64 { let c: bool = true; let s: secret i64 = 1; let x = if c { 1 } else { s }; 0 }",
        "enum E { A, B }\nfn main() -> i64 { let e = E::A; let x = match e { E::A => 1, E::B => true }; 0 }",
        "enum E { A, B }\nfn main() -> i64 { let e = E::A; let s: secret i64 = 1; let x: i64 = match e { E::A => s, E::B => 2 }; x }",
        "fn main() -> i64 { let c: bool = true; let x: secret i64 = { if c { true } else { 1 } }; 0 }",
        "fn main() -> i64 { let c: bool = true; let x: ?i64 = { if c { true } else { 1 } }; 0 }",
        "fn f(c: bool) -> secret i64 { if c { return if c { true } else { 1 } } else { 0 } }\nfn main() -> i64 { 0 }",
        "fn f(c: bool) -> ?i64 { if c { return if c { true } else { 1 } } else { null } }\nfn main() -> i64 { 0 }",
        "fn f(x: ?i64) -> i64 { 0 }\nfn main() -> i64 { f(true) }",
        "fn f(x: secret i64) -> i64 { 0 }\nfn main() -> i64 { f(true) }",
        // A value that diverges is not coerced, and is still compared where the oracle compares it: at
        // a `let`, an assignment, a field, a method's argument, a payload, a `resume`'s argument and
        // a later array element; a first array element that diverges is compared as the array.
        "fn f(s: secret i64) -> secret i64 { let x: i64 = return s; x }\nfn main() -> i64 { 0 }",
        "fn f(s: secret i64) -> secret i64 { let mut x: i64 = 0; x = return s; x }\nfn main() -> i64 { 0 }",
        "struct H { a: i64 }\nfn f(s: secret i64) -> secret i64 { let h = H { a: return s }; 0 }\nfn main() -> i64 { 0 }",
        "fn f(s: secret i64) -> secret i64 { let a: [i64] = [return s]; 0 }\nfn main() -> i64 { 0 }",
        "fn f(s: secret i64) -> secret i64 { let a: [i64] = [1, return s]; 0 }\nfn main() -> i64 { 0 }",
        "class K { let n: i64;\n pub init() { self.n = 1; 0 }\n pub fn t(self: &Self, a: i64) -> i64 { a } }\nfn f(s: secret i64) -> secret i64 { let k = K::init(); k.t(return s) }\nfn main() -> i64 { 0 }",
        "enum E { A(i64) }\nfn f(s: secret i64) -> secret i64 { let e = E::A(return s); 0 }\nfn main() -> i64 { 0 }",
        "effect Q { q(x: i64) -> i64; }\nfn w() -> i64 ! { Q } { perform Q.q(3) }\nfn f(s: secret i64) -> secret i64 { handle w() with { Q.q(x, k) => k(return s) } }\nfn main() -> i64 { 0 }",
        "fn f(s: secret i64) -> i64 { let a: [secret i64] = [return 5, s]; 0 }\nfn main() -> i64 { 0 }",
        // An arity: a `perform`'s, an `init`'s, a method's, a qualified call's, a builtin's, a
        // continuation's and a handler arm's; an undefined fn.
        "effect Q { q(x: i64) -> i64; }\nfn w() -> i64 ! { Q } { perform Q.q(1, 2) }\nfn main() -> i64 { handle w() with { Q.q(x, k) => k(x) } }",
        "class K { let n: i64; pub init(a: i64) { self.n = a; 0 } }\nfn main() -> i64 { let k = K::init(); 0 }",
        "class K { let n: i64;\n pub init() { self.n = 1; 0 }\n pub fn get(self: &Self) -> i64 { self.n } }\nfn main() -> i64 { let k = K::init(); k.get(1) }",
        "struct P { n: i64 }\ntrait T { fn eat(self: &Self, x: i64) -> i64; }\nimpl Direct as T for P { fn eat(self: &Self, x: i64) -> i64 { x + self.n } }\nfn main() -> i64 { let p = P { n: 1 }; Direct::eat(&p) }",
        "struct P { n: i64 }\ntrait T { fn eat(self: &Self, x: i64) -> i64; }\nimpl Direct as T for P { fn eat(self: &Self, x: i64) -> i64 { x + self.n } }\nfn main() -> i64 { let p = P { n: 1 }; Direct::eat(&p, 1, 2) }",
        "fn main() -> i64 { let a: [i64] = [1]; len(a, a) }",
        "fn main() -> i64 { let mut v: Vec<i64> = vec_new(); push(&mut v); 0 }",
        "fn main() -> i64 { nope(1) }",
        "effect Q { q(x: i64) -> i64; }\nfn w() -> i64 ! { Q } { perform Q.q(3) }\nfn main() -> i64 { handle w() with { Q.q(x, k) => k(x, 1) } }",
        "effect Q { q(x: i64) -> i64; }\nfn w() -> i64 ! { Q } { perform Q.q(3) }\nfn main() -> i64 { handle w() with { Q.q(x, k) => k() } }",
        "effect Q { q(x: i64) -> i64; }\nfn w() -> i64 ! { Q } { perform Q.q(3) }\nfn main() -> i64 { handle w() with { Q.q(k) => k(1) } }",
        "effect Q { q(x: i64) -> i64; }\nfn w() -> i64 ! { Q } { perform Q.q(3) }\nfn main() -> i64 { handle w() with { Q.q(x, y, k) => k(1) } }",
        // A handler arm's body against the `handle`'s type, and a `return` arm's typed with the type
        // expected of the `handle`.
        "effect Q { q(x: i64) -> i64; }\nfn w() -> i64 ! { Q } { perform Q.q(3) }\nfn main() -> i64 { handle w() with { Q.q(x, k) => true } }",
        "struct B<T> { v: T }\neffect Q { q(x: i64) -> i64; }\nfn w() -> i64 ! { Q } { perform Q.q(3) }\nfn main() -> i64 { let b: B<i64> = handle w() with { Q.q(x, k) => k(1), return r => B { v: r } }; b.v }",
        // An operator's operands: `!` and `-`, and `&&` / `||` over a `secret` and a public `bool`,
        // which do not join; an index into a non-array; a `send`'s value and a `recv`'s channel;
        // a type-argument count; an impl method called on a receiver it does not apply to.
        "fn main() -> i64 { let x: bool = !5; 0 }",
        "fn main() -> i64 { let x: i64 = -true; 0 }",
        "fn main() -> i64 { let x: i64 = 5; x[0] }",
        "fn main() -> i64 { let b: bool = true; b[0] }",
        "fn f(a: secret bool, b: bool) -> secret bool { a && b }\nfn main() -> i64 { 0 }",
        "fn f(a: bool, b: secret bool) -> secret bool { a || b }\nfn main() -> i64 { 0 }",
        "fn main() -> i64 { let x: bool = 1 && true; 0 }",
        "fn main() -> i64 { let ch: Channel<i64> = channel_new(); send(ch, true); 0 }",
        "fn main() -> i64 { let ch: Channel<i64> = channel_new(); let s: secret i64 = 1; send(ch, s); 0 }",
        "fn main() -> i64 { let x: ?i64 = recv(5); 0 }",
        "fn main() -> i64 { let v: Vec<i64, i64> = vec_new(); 0 }",
        "struct B<T> { v: T }\nfn f(b: B<i64, i64>) -> i64 { 0 }\nfn main() -> i64 { 0 }",
        "fn f(c: Channel<i64, i64>) -> i64 { 0 }\nfn main() -> i64 { 0 }",
        "struct P { n: i64 }\ntrait T { fn eat(self: &Self, x: i64) -> i64; }\nimpl Direct as T for P { fn eat(self: &Self, x: i64) -> i64 { x + self.n } }\nfn main() -> i64 { Direct::eat(5, 1) }",
        "struct P { n: i64 }\ntrait T { fn eat(self: &Self, x: i64) -> i64; }\nimpl Direct as T for P { fn eat(self: &Self, x: i64) -> i64 { x + self.n } }\nstruct R { m: i64 }\nfn main() -> i64 { let r = R { m: 1 }; Direct::eat(r, 1) }",
        // A public array into a `secret` one where the element widens (ADR 0051): at a `let` and at a
        // call's argument, whose parameter the oracle unifies with the coerced argument.
        "fn f(a: [u8]) -> i64 { let b: [secret u8] = a; len(b) }\nfn main() -> i64 { 0 }",
        "fn g(b: [secret i64]) -> i64 { len(b) }\nfn f(a: [i64]) -> i64 { g(a) }\nfn main() -> i64 { 0 }",
        "fn f(x: [secret u8]) -> i64 { len(x) }\nfn main() -> i64 { let b: [u8] = [1 as u8]; f(b) }",
        // A `handle` whose `return` arm holds a `handle` of its own, with a call through an `i64`
        // in an operation arm: the `return` arm's first typing leaves no continuation behind.
        "effect Q { ask(x: i64) -> i64; }\neffect R { op(y: i64) -> i64; }\nfn w() -> i64 ! { Q } { perform Q.ask(3) }\nfn w2() -> i64 ! { R } { perform R.op(4) }\nfn main() -> i64 {\n  handle w() with {\n    Q.ask(x, k) => { let z: i64 = 5; z(1) },\n    return v => handle w2() with { R.op(y, k2) => k2(y) }\n  }\n}",
        // An impl method's receiver kind and effect row against its trait's, both ways, the row
        // compared sorted and once each.
        "struct P { n: i64 }\ntrait T { fn eat(self: &Self, x: i64) -> i64; }\nimpl Direct as T for P { fn eat(self: &mut Self, x: i64) -> i64 { x } }\nfn main() -> i64 { let mut p: P = P { n: 1 }; Direct::eat(&mut p, 1) }",
        "struct P { n: i64 }\ntrait T { fn eat(self: &mut Self, x: i64) -> i64; }\nimpl Direct as T for P { fn eat(self: &Self, x: i64) -> i64 { x } }\nfn main() -> i64 { let p: P = P { n: 1 }; Direct::eat(&p, 1) }",
        "effect Q { ask(x: i64) -> i64; }\nstruct P { n: i64 }\ntrait T { fn eat(self: &Self, x: i64) -> i64; }\nimpl Direct as T for P { fn eat(self: &Self, x: i64) -> i64 ! { Q } { perform Q.ask(x) } }\nfn main() -> i64 { 0 }",
        "effect Q { ask(x: i64) -> i64; }\nstruct P { n: i64 }\ntrait T { fn eat(self: &Self, x: i64) -> i64 ! { Q }; }\nimpl Direct as T for P { fn eat(self: &Self, x: i64) -> i64 { x } }\nfn main() -> i64 { 0 }",
        "effect Q { ask(x: i64) -> i64; }\nstruct P { n: i64 }\ntrait T { fn eat(self: &Self, x: i64) -> i64 ! { Q }; }\nimpl Direct as T for P { fn eat(self: &Self, x: i64) -> i64 ! { Q } { perform Q.ask(x) } }\nfn main() -> i64 { let p: P = P { n: 1 }; Direct::eat(&p, 1) }",
        "effect A { a() -> i64; }\neffect B { b() -> i64; }\nstruct P { n: i64 }\ntrait T { fn eat(self: &Self) -> i64 ! { A, B }; }\nimpl Direct as T for P { fn eat(self: &Self) -> i64 ! { B, A } { perform A.a() + perform B.b() } }\nfn main() -> i64 { 0 }",
        "effect A { a() -> i64; }\neffect B { b() -> i64; }\nstruct P { n: i64 }\ntrait T { fn eat(self: &Self) -> i64 ! { A }; }\nimpl Direct as T for P { fn eat(self: &Self) -> i64 ! { A, A } { perform A.a() } }\nfn main() -> i64 { 0 }",
        "effect A { a() -> i64; }\neffect B { b() -> i64; }\nstruct P { n: i64 }\ntrait T { fn eat(self: &Self) -> i64 ! { A, B }; }\nimpl Direct as T for P { fn eat(self: &Self) -> i64 ! { A } { perform A.a() } }\nfn main() -> i64 { 0 }",
        // `unwrap_or` takes `T` from the expected type only where the oracle hands the call one (a
        // `let` annotation, an assignment), and then widens its first argument to `?T`; a `null`
        // first argument takes its type from the second. A generic fn under a `let` annotation.
        "fn main() -> i64 { let x: i64 = 3; let y: i64 = unwrap_or(x, 1); y }",
        "fn main() -> i64 { let x: i64 = 4; let mut y: i64 = 0; y = unwrap_or(x, 1); y }",
        "fn main() -> i64 { let x: ?i64 = 4; let y: secret i64 = unwrap_or(x, 1); 0 }",
        "fn main() -> i64 { let y: i64 = unwrap_or(null, 1); y }",
        "fn pick<T>(x: ?T, d: T) -> T { d }\nfn main() -> i64 { let x: i64 = 3; let y: i64 = pick(x, 1); y }",
        // A value coerced from `[S]` into `[secret S]` takes the expected type, as the oracle's
        // coercion gives it, at a join, in a nested array literal and through a generic fn's
        // `T` or `[T]` parameter.
        "fn main() -> i64 {\n    let c: bool = true;\n    let pubs: [i64] = [1, 2];\n    let sec: [secret i64] = [5];\n    let x: [secret i64] = if c { sec } else { pubs };\n    declassify(x[0])\n}",
        "fn f(c: bool) -> [secret u8] {\n    let fallback: [secret u8] = \"zz\";\n    if c { \"ab\" } else { fallback }\n}\nfn main() -> i64 { len(f(true)) }",
        "fn main() -> i64 {\n    let c: bool = true;\n    let s: [secret u8] = if c { \"ab\" } else { \"cd\" };\n    let t: [secret u8] = if c { s } else { \"cd\" };\n    0\n}",
        "fn main() -> i64 {\n    let a: [[secret u8]] = [\"ab\", \"cd\"];\n    declassify(a[0][0]) == 'a';\n    0\n}",
        "fn id<T>(x: T) -> T { x }\nfn main() -> i64 {\n    let a: [secret u8] = id(\"abc\");\n    declassify(a[0]) == 'a';\n    0\n}",
        "fn id<T>(x: T) -> T { x }\nfn main() -> i64 { let a: [secret i64] = id([1, 2]); declassify(a[0]) }",
        "fn put<T>(x: T, a: [T]) -> T { x }\nfn main() -> i64 { let s: secret i64 = 3; declassify(put(s, [1, 2])) }",
        "struct B<T> { v: T }\nfn wrap<T>(x: T) -> B<T> { B { v: x } }\nfn main() -> i64 {\n    let pubs: [i64] = [1, 2];\n    let b: B<[secret i64]> = wrap(pubs);\n    let c: B<secret i64> = wrap(5);\n    let d: B<?i64> = wrap(6);\n    declassify(b.v[0])\n}",
        // A generic call's `null` argument before the one that binds `T`, which the oracle types
        // after it; a `secret T` parameter, which no argument meets; a `?T` the oracle cannot form.
        "fn takes<T>(x: ?T, d: T) -> i64 {\n    if is_some(x) { 0 } else { 40 }\n}\nfn main() -> i64 {\n    takes(null, true) + 2\n}",
        "fn put<T>(x: ?T, y: T) -> T { y }\nfn main() -> i64 { put(null, 8) + 1 }",
        "struct B<T> { v: T }\nfn takes<T>(x: ?B<T>, d: T) -> i64 { 1 }\nfn main() -> i64 { takes(null, 3) }",
        "fn put<T>(x: T, a: secret T) -> T { x }\nfn main() -> i64 { put(4, 5) }",
        "fn pick<T>(x: ?T, d: T) -> T { d }\nfn main() -> i64 { let x: ?i64 = 4; let y: secret i64 = pick(x, 1); 0 }",
        // A type argument count is checked before the arguments (too few, too many with an unknown
        // one), and a field access on a type that is not a struct, an instance or a class.
        "struct P<A, B> { a: A, b: B }\nfn main() -> i64 { let p: P<i64> = P { a: 1, b: 2 }; 0 }",
        "fn main() -> i64 { let v: Vec<Bogus, i64> = vec_new(); 0 }",
        "struct P<A, B> { a: A, b: B }\nfn f(p: P<Bogus>) -> i64 { 0 }\nfn main() -> i64 { 0 }",
        "struct P { x: i64 }\nfn put<T>(t: &mut T, s: secret i64) -> i64 {\n    (*t).x = s;\n    0\n}\nfn main() -> i64 {\n    let mut p: P = P { x: 0 };\n    let s: secret i64 = 42;\n    put(&mut p, s);\n    if p.x > 3 { 1 } else { 0 }\n}",
        "struct P {\n    x: i64,\n}\n\nfn main() -> i64 {\n    let n: i64 = 5;\n    n.x\n}",
        // Two effect rows of one length that name different effects; `unwrap_or` over a `null` with
        // no expected type, and with a `secret` one its default must widen to; an array literal
        // passed to a plain `[i64]` parameter, which the oracle gives no expected type.
        "effect A { a() -> i64; }\neffect B { b() -> i64; }\nstruct P { n: i64 }\ntrait T { fn eat(self: &Self) -> i64 ! { A }; }\nimpl Direct as T for P { fn eat(self: &Self) -> i64 ! { B } { perform B.b() } }\nfn main() -> i64 { 0 }",
        "fn f() -> i64 { unwrap_or(null, 1) }\nfn main() -> i64 { f() }",
        "fn main() -> i64 { let y: secret i64 = unwrap_or(null, true); 0 }",
        "fn sum(a: [i64]) -> i64 { 0 }\nfn main() -> i64 { sum([true]) }",
        // (A17) `secret f64` wherever a type is written, and cast to; a cast of other than an
        // integer or a float, or to other than one; a `secret` handle; a deref of a `secret`
        // reference, of a non-reference, and of a reference to a `secret` (accepted).
        "struct P { a: secret f64 }\nfn main() -> i64 { 0 }",
        "enum E { A(secret f64) }\nfn main() -> i64 { 0 }",
        "fn f() -> secret f64 { 1.5 }\nfn main() -> i64 { 0 }",
        "fn main() -> i64 { let v: Vec<secret f64> = vec_new(); 0 }",
        "fn main() -> i64 { let x: ?secret f64 = null; 0 }",
        "class K { let x: secret f64; pub init() { self.x = 1.5; 0 } }\nfn main() -> i64 { 0 }",
        "trait T1 { fn m(self: &Self, x: secret f64) -> i64; }\nfn main() -> i64 { 0 }",
        "effect Q { q(x: secret f64) -> i64; }\nfn main() -> i64 { 0 }",
        "fn main() -> i64 { let s: secret u8 = 3 as u8; let f = s as f64; 0 }",
        "fn main() -> i64 { let s: secret i64 = 3; let t = s as i32; let f = declassify(s) as f64; 0 }",
        "fn main() -> i64 { let s: secret bool = true; let f = s as f64; 0 }",
        "fn main() -> i64 { let x: i64 = 3; let f = x as bool; 0 }",
        "struct P { a: i64 }\nfn main() -> i64 { let p = P { a: 1 }; let f = p as i64; 0 }",
        "fn g<T>(x: T) -> i64 { x as i64 }\nfn main() -> i64 { 0 }",
        "fn main() -> i64 { let x: f64 = 1.5; let n = x as i64; let u = n as u8; let w = u as i32; 0 }",
        "fn f(m: secret Mutex<i64>) -> i64 { 0 }\nfn main() -> i64 { 0 }",
        "fn f() -> secret Shared<i64> { shared_new(1) }\nfn main() -> i64 { 0 }",
        "fn f(r: secret &mut i64) -> i64 { *r = 5; 0 }\nfn main() -> i64 { 0 }",
        "fn f(x: secret i64) -> i64 { *x }\nfn main() -> i64 { 0 }",
        "fn f(a: [i64]) -> i64 { *a }\nfn main() -> i64 { 0 }",
        "fn f(r: & secret i64) -> secret i64 { *r }\nfn main() -> i64 { 0 }",
        // (A17) a `match` over other than an enum, an unknown variant in a pattern (nested too)
        // and in a construction, and a `match` over a plain enum (accepted).
        "enum E { A, B(i64) }\nfn f(e: [i64]) -> i64 { match e { E::A => 0, E::B(v) => v } }\nfn main() -> i64 { 0 }",
        "fn f(e: i64) -> i64 { match e { _ => 0 } }\nfn main() -> i64 { 0 }",
        "fn f(e: i64) -> i64 { match e { E::A => 0 } }\nfn main() -> i64 { 0 }",
        "enum F { A }\nfn f(e: F) -> i64 { match e { F::B => 0 } }\nfn main() -> i64 { 0 }",
        "enum F { A, B(i64) }\nfn f(e: F) -> i64 { match e { F::A => match e { F::Q => 1, _ => 2 }, _ => 0 } }\nfn main() -> i64 { 0 }",
        "enum F { A, B(i64) }\nfn main() -> i64 { let x: F = F::Z; 0 }",
        "enum F { A, B(i64) }\nfn f(e: F) -> i64 { match e { F::A => 0, F::B(v) => v } }\nfn main() -> i64 { f(F::B(4)) }",
        "enum F { A, B }\nenum G { X, Y }\nfn f(e: F, g: G) -> i64 { match e { F::A => match g { G::X => 1, G::Y => 2 }, F::B => 0 } }\nfn main() -> i64 { 0 }",
        // (A17) a `match` that names every variant or has a `_` arm, each `match` counted on its
        // own when one is nested in another's arm.
        "enum E { A, B(i64), C }\nfn f(e: E) -> i64 { match e { E::B(v) => v } }\nfn main() -> i64 { 0 }",
        "enum E { A, B(i64) }\nfn f(e: E) -> i64 { match e { E::A => 0, _ => 1 } }\nfn main() -> i64 { 0 }",
        "enum E { A, B(i64) }\nenum G { X, Y }\nfn f(e: E, g: G) -> i64 { match e { E::A => match g { G::X => 1, G::Y => 2 } } }\nfn main() -> i64 { 0 }",
        "enum E { A, B(i64) }\nenum G { X, Y }\nfn f(e: E, g: G) -> i64 { match e { E::A => match g { _ => 1 } } }\nfn main() -> i64 { 0 }",
        "enum E { A, B(i64) }\nenum G { X, Y }\nfn f(e: E, g: G) -> i64 { match e { _ => match g { G::X => 1 } } }\nfn main() -> i64 { 0 }",
        // (A17) the FFI fence at an `extern` declaration and an `export` fn; what it lets through.
        "struct P { a: i64 }\nextern \"C\" { fn g(x: P) -> i64; }\nfn main() -> i64 { 0 }",
        "extern \"C\" { fn g(x: i32) -> i64; }\nfn main() -> i64 { 0 }",
        "extern \"C\" { fn g(x: [u8]) -> i64; }\nfn main() -> i64 { 0 }",
        "extern \"C\" { fn g(x: ptr) -> f64; }\nfn main() -> i64 { 0 }",
        "export \"C\" fn f(x: i32) -> i64 { 0 }\nfn main() -> i64 { 0 }",
        "export \"C\" fn f(x: &[secret u8]) -> i64 { 0 }\nfn main() -> i64 { 0 }",
        "export \"C\" fn f(x: &mut [u8]) -> [u8] { let v: [u8] = \"ab\"; v }\nfn main() -> i64 { 0 }",
        "export \"C\" fn f(x: i64) -> i64 { if keep(true) { x } else { 0 } }\nfn keep(b: bool) -> bool { b }\nfn main() -> i64 { 0 }",
        // (A17) `secret T` passes through substitution unchanged, as the oracle's does: no
        // `secret f64` or `secret` handle is made by instantiating a generic. Since ADR 0016 A3
        // both typers refuse each of these at the use that would keep it (`uninstantiable_type`).
        "struct B<T> { v: secret T }\nfn main() -> i64 { let b: B<f64> = B { v: 1.5 }; 0 }",
        "fn g<T>(x: T) -> secret T { x }\nfn main() -> i64 { let y = g(1.5); let z = y * y; 0 }",
        "fn g<T>(x: T) -> secret T { x }\nfn main() -> i64 { let a: i64 = 4; let r = g(&a); let v = *r; 0 }",
        "enum E { A, B(i64) }\nfn g<T>(x: T) -> secret T { x }\nfn main() -> i64 { let e = g(E::B(3)); match e { E::A => 0, E::B(v) => v } }",
        "fn g<T>(x: T) -> secret T { x }\nfn main() -> i64 { let s: Shared<i64> = shared_new(1); let y = g(s); shared_get(y) }",
        // (A17) The same rule refuses `vec_to_array` over an instance whose field is `secret T`,
        // as the oracle does (register D146).
        "struct Box<T> { s: secret T }\nfn main() -> i64 { let mut v: Vec<Box<i32>> = vec_new(); let a: [Box<i32>] = vec_to_array(v); 0 }",
        // (A17) Nor does a `secret T` bind `T` in unification (the oracle's `unify_one` has no arm
        // for it). These programs were accepted, or refused at a comparison (ADR 0016 A2), as
        // `snc types` answered them; since ADR 0016 A3 each is refused at its first call of a
        // generic returning `secret T` (or the literal of a struct with a `secret T` field), as
        // `snc types` refuses it, and they stay here as shapes the two typers must answer alike.
        // (The code generator refuses every `secret` written over a type parameter, register D182.)
        "fn g<T>(x: T) -> secret T { x }\nfn main() -> i64 { let b = g(1) != g(2); 0 }",
        "fn g<T>(x: T) -> secret T { x }\nfn main() -> i64 { let a = [g(1), g(2)]; 0 }",
        "fn g<T>(x: T) -> secret T { x }\nfn main() -> i64 { let mut y = g(1); y = g(2); 0 }",
        "fn g<T>(x: T) -> secret T { x }\nfn main() -> i64 { let b = g(1) == g(1); if declassify(b) { 1 } else { 0 } }",
        "fn g<T>(x: T) -> secret T { x }\nfn f(c: bool) -> i64 { let y = if c { g(1) } else { g(2) }; 0 }\nfn main() -> i64 { 0 }",
        "fn h<T>(x: T) -> secret T { x }\nfn k<T>(x: T, y: secret T) -> i64 { 0 }\nfn main() -> i64 { k(1, h(2)) }",
        "fn k<T>(y: secret T) -> i64 { 0 }\nfn h<T>(x: T) -> secret T { x }\nfn main() -> i64 { k(h(2)) }",
        "fn r<T>(x: &secret T) -> i64 { 0 }\nfn h<T>(x: T) -> secret T { x }\nfn main() -> i64 { let s = h(3); r(&s) }",
        "fn r<T>(x: [secret T]) -> i64 { 0 }\nfn h<T>(x: T) -> secret T { x }\nfn main() -> i64 { let s = h(3); r([s]) }",
        "fn r<T>(x: &secret T, y: T) -> i64 { 0 }\nfn h<T>(x: T) -> secret T { x }\nfn main() -> i64 { let s = h(3); r(&s, 1) }",
        "struct B<T> { v: secret T, w: i64 }\nfn mk<T>(x: T, w: i64) -> B<T> { B { v: x, w: w } }\nfn main() -> i64 { let b = mk(true, 7); let c = mk(9, 8); let mut x = b.v; x = c.v; 0 }",
    ];
    let mut sources: Vec<String> = DA_BODIES.iter().map(|b| DA_CLASS.replace('@', b)).collect();
    sources.extend(OTHER.iter().map(|s| s.to_string()));

    let tmp =
        std::env::temp_dir().join(format!("snc_selfhost_types_shapes_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(&tmp).expect("create temp dir");
    let typer = build_sentinel_typer(&tmp);
    let work = tmp.join("work");
    std::fs::create_dir_all(&work).expect("create work dir");
    let input = work.join("input.sentinel");
    let mut refused = 0usize;
    let mut failures: Vec<String> = Vec::new();
    for src in &sources {
        std::fs::write(&input, src).expect("stage source");
        let oracle = Command::new(env!("CARGO_BIN_EXE_snc"))
            .arg("types")
            .arg(&input)
            .output()
            .expect("run snc types");
        let oracle_err = String::from_utf8_lossy(&oracle.stderr);
        let want = if oracle.status.success() {
            None
        } else {
            Some(oracle_err.lines().find_map(|l| l.strip_prefix("snc: ")).unwrap_or("").to_string())
        };
        let out = Command::new(&typer)
            .current_dir(&work)
            .output()
            .expect("run the Sentinel typer");
        let stdout = String::from_utf8_lossy(&out.stdout);
        // A refusal is exactly two lines: the code, then the message.
        let got = if out.status.success() {
            None
        } else if stdout.lines().count() != 2 {
            Some(format!("<{} lines>", stdout.lines().count()))
        } else {
            Some(stdout.lines().nth(1).and_then(|l| l.strip_prefix("scg: ")).unwrap_or("").to_string())
        };
        if want.is_some() {
            refused += 1;
        }
        if want != got {
            failures.push(format!("  {src:?}:\n    oracle {want:?}\n    typer  {got:?}"));
        }
    }
    assert!(refused >= 30, "expected at least thirty refused shapes, got {refused}");
    assert!(
        failures.is_empty(),
        "the Sentinel typer's refusals differ from the oracle's on {}/{} shape(s):\n{}",
        failures.len(),
        sources.len(),
        failures.join("\n")
    );
}
