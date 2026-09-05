//! Phase D self-host port (2b) / ADR 0039 D8: the parser differential test.
//! Compile `selfhost/parser.sentinel` with the Rust `snc`, then assert its
//! canonical AST dump is byte-identical to `snc ast` for a seed set of
//! programs. The (2b) seeds are paramless fns whose body is an expression over
//! the surface landed so far. Increment-1 covers the COMPLETE
//! operator-precedence ladder (logical, comparison, bitwise, additive,
//! multiplicative, prefix unary, parens) plus the scalar atom leaves (integer,
//! `true`, `false`, `null` literals, variable refs). Increment-2 adds function
//! calls `f(args)` and the postfix chain: field `t.field`, index `t[i]`, and
//! method `t.m(args)`. Increment-3 adds the `::` paths after an identifier
//! (qualified call `A::b(args)`, class init `Name::init(args)`, and the
//! paren-less unit form `Enum::Variant`) plus array literals `[e1, e2, ...]`,
//! all reusing the argument-list cons-list. The tree shape (not source order)
//! is what the dump pins, so seeds mixing every level, and chaining postfix
//! over calls/qcalls/arrays, prove the whole grammar. Increment-4 adds `if`
//! expressions (with mandatory `else` and `else if` chains) and brace blocks
//! `{ <expr> }`, which are statement-free for now (just a tail). Increment-5
//! adds `match <scrutinee> { pat => body, ... }` with `_` and qualified-variant
//! patterns (positional bindings, themselves possibly `_`). Increment-6 adds
//! struct literals `Name { f: v, ... }`, disambiguated from an if/match head's
//! block by a context-free `{ Ident :` lookahead (so the head seeds below must
//! keep parsing the head as a condition, not a struct literal). Increment-7 adds
//! the effect / concurrency leaf forms: `declassify(e)`, `perform Eff.op(args)`,
//! `scope concurrent { block }`, `spawn <call>`, and the `.await` postfix.
//! Increment-8 adds `handle <body> with { ... }` — which CLOSES the expression
//! grammar (every `ExprKind` the oracle emits). (2c-1) then turns a block into a
//! real `{ <stmt>* <tail> }`: `let`, assignment, `while`, `break`, `continue`,
//! and expr-statements (a statement-only `while` body keeps the synthesized
//! `(int 0)` tail). (2c-2) adds the optional `let` type annotation via a
//! `parse_type` covering `Ident` / `Ident<args>` / `[T]` / `?T` / `&T` / `&mut T`
//! / `secret T` (and nesting). (2c-3) adds full `fn` definitions: a param list
//! `( [mut] name: T, … )` → `(param [mut] name <type>)`, a `-> TYPE` return type
//! (routed through `parse_type`), and — parsed but NOT dumped, matching the
//! oracle — generic type-params `<…>` and the postfix effect row `! { … }`.
//! (2d) adds the top-level decl grammar — `use` / `struct` / `enum` / `effect`
//! / `trait` / `impl` / `class` (dumped in source order; the oracle re-sorts its
//! kind-bucketed `Program` by span, the Sentinel parser emits decls as it
//! scans). (2d-8) then closes the gaps to the **full corpus**: `//` line
//! comments, the prefix `*` (deref) / `&` / `&mut` (address-of) unary operators,
//! and char / string literals (`'c'` → `(char N)`, `"s"` → `(str b…)`, escapes
//! decoded per ADR 0033 D2). Beyond the curated `SEEDS` above,
//! [`sentinel_parser_matches_oracle_on_corpus`] is the parser-stage phase-go: it
//! runs the Sentinel parser against `snc ast` over every clean-parsing fixture
//! in `tests/pass` + `tests/ui`, the way `tests/selfhost_lex.rs` covers the
//! corpus for `snc lex`.

use std::path::{Path, PathBuf};
use std::process::Command;

fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .canonicalize()
        .expect("canonicalize workspace root")
}

fn build_sentinel_parser(tmp: &Path) -> PathBuf {
    let src = workspace_root().join("selfhost/parser.sentinel");
    let bin = tmp.join("sparser");
    let out = Command::new(env!("CARGO_BIN_EXE_snc"))
        .arg("build")
        .arg(&src)
        .arg("-o")
        .arg(&bin)
        .output()
        .expect("run snc build");
    assert!(
        out.status.success(),
        "compiling selfhost/parser.sentinel failed:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    bin
}

/// The (2b) seed programs: full operator-precedence expression bodies in
/// paramless fns. Each line below targets a distinct part of the ladder; the
/// `mx`/`mx2` seeds interleave every level at once so the whole precedence
/// tree is pinned, not just adjacent pairs.
const SEEDS: &[&str] = &[
    // (2a) arithmetic carried forward.
    "fn main() -> i64 { 1 + 2 * 3 }\n",
    "fn f() -> i64 { (1 + 2) * 3 }\n",
    "fn g() -> i64 { 7 - 3 - 1 }\n",
    "fn h() -> i64 { 2 * 3 + 4 * 5 }\n",
    "fn a() -> i64 { 1 }\nfn b() -> i64 { 2 + 3 }\n",
    // Scalar atom leaves.
    "fn t() -> bool { true }\n",
    "fn fa() -> bool { false }\n",
    "fn nu() -> i64 { null }\n",
    "fn v() -> i64 { x }\n",
    "fn vv() -> i64 { foo + bar - baz }\n",
    // Prefix unary (and unary vs. infix `-`).
    "fn ng() -> i64 { -5 }\n",
    "fn nt() -> bool { !x }\n",
    "fn nn() -> i64 { 1 - -2 }\n",
    "fn nnt() -> bool { !!x }\n",
    // Comparisons (non-associative) and their precedence vs. arithmetic.
    "fn c1() -> bool { 1 < 2 }\n",
    "fn c2() -> bool { x == y }\n",
    "fn c3() -> bool { a + b >= c * d }\n",
    "fn c4() -> bool { x != y }\n",
    // Logical with short-circuit precedence (`&&` binds tighter than `||`).
    "fn l1() -> bool { a && b }\n",
    "fn l2() -> bool { a || b && c }\n",
    "fn l3() -> bool { !a || b }\n",
    // Bitwise ladder (`&` > `^` > `|`).
    "fn bw() -> i64 { 5 & 6 ^ 3 | 8 }\n",
    "fn bw2() -> i64 { 1 | 2 & 3 }\n",
    // Parenthesised regrouping across levels.
    "fn pg() -> i64 { (a | b) & c }\n",
    // Every precedence level interleaved in one expression.
    "fn mx() -> bool { 1 + 2 * 3 < 4 | 5 && 6 == 7 }\n",
    "fn mx2() -> bool { a && b || c == d & e + f * g }\n",
    // (increment-2) function calls — zero/one/many args, expr args, nesting.
    "fn ca() -> i64 { g() }\n",
    "fn cb() -> i64 { g(1) }\n",
    "fn cc() -> i64 { g(1, 2) }\n",
    "fn cd() -> i64 { g(a + 1, b * 2) }\n",
    "fn ce() -> i64 { g(h(1)) }\n",
    "fn cf() -> i64 { p(q(r(s(1)))) }\n",
    // (increment-2) postfix: field, index, method.
    "fn pa() -> i64 { x.y }\n",
    "fn pb() -> i64 { a.b.c }\n",
    "fn ix() -> i64 { a[0] }\n",
    "fn iy() -> i64 { a[i + 1] }\n",
    "fn me() -> i64 { x.foo() }\n",
    "fn mg() -> i64 { x.foo(1, 2) }\n",
    // (increment-2) chained postfix + interaction with operators/unary.
    "fn ch() -> i64 { a.b(c)[d].e }\n",
    "fn ci() -> i64 { g(1).h(2) }\n",
    "fn cj() -> i64 { f(1) + g(2) * 3 }\n",
    "fn ck() -> i64 { a[0] + b.c }\n",
    "fn cl() -> i64 { -a.b + !c[0] }\n",
    "fn cm() -> i64 { x.foo(1).bar[k].baz(y, z) }\n",
    // (increment-3) `::` paths: qualified call, class init, unit construction.
    "fn qa() -> i64 { A::b() }\n",
    "fn qb() -> i64 { A::b(1, 2) }\n",
    "fn qc() -> i64 { Color::Red }\n",
    "fn qd() -> i64 { Point::init(1, 2) }\n",
    "fn qe() -> i64 { Empty::init() }\n",
    "fn qf() -> i64 { Foo::init }\n",
    // (increment-3) array literals (incl. empty, expr elems, then postfix index).
    "fn ar() -> i64 { [1, 2, 3] }\n",
    "fn ae() -> i64 { [] }\n",
    "fn ax() -> i64 { [a + 1, b * 2] }\n",
    "fn ai() -> i64 { [1, 2][0] }\n",
    "fn ao() -> i64 { [1][0] + 2 }\n",
    // (increment-3) interaction with postfix / calls / each other.
    "fn qg() -> i64 { A::b().c }\n",
    "fn qh() -> i64 { f(A::b(), [1, 2]) }\n",
    "fn qi() -> i64 { g(A::b(x), [1, h(3)], Point::init(y, z)) }\n",
    "fn qj() -> i64 { [A::b(), c.d][0].e }\n",
    // (increment-4) if-expressions: basic, with cond exprs, else-if chains.
    "fn ia() -> i64 { if c { 1 } else { 2 } }\n",
    "fn ib() -> i64 { if a < b { a } else { b } }\n",
    "fn ic() -> i64 { if x { 1 } else if y { 2 } else { 3 } }\n",
    "fn id() -> i64 { if a && b { 1 + 2 } else { 3 * 4 } }\n",
    "fn ie() -> i64 { if c { if d { 1 } else { 2 } } else { 3 } }\n",
    // (increment-4) brace blocks + if interacting with calls/arrays.
    "fn ba() -> i64 { { 5 } }\n",
    "fn bb() -> i64 { g(if c { 1 } else { 2 }) }\n",
    "fn bc() -> i64 { [if c { 1 } else { 2 }, 3] }\n",
    // (increment-5) match: variants, bindings, wildcard, nesting, expr bodies.
    "fn ma() -> i64 { match x { A::B => 1, A::C => 2, _ => 0 } }\n",
    "fn mb() -> i64 { match x { E::Some(v) => v, E::None => 0 } }\n",
    "fn mc() -> i64 { match p { Pair::Of(a, b) => a + b, _ => 0 } }\n",
    "fn md() -> i64 { match g(x) { R::Ok(v) => v, _ => 0 } }\n",
    "fn me2() -> i64 { match x { E::P(_, b) => b, _ => 0 } }\n",
    "fn mf2() -> i64 { match x { A::B => if c { 1 } else { 2 }, _ => g(3) } }\n",
    "fn mh() -> i64 { match x { A::B => match y { C::D => 1, _ => 2 }, _ => 0 } }\n",
    "fn mi() -> i64 { g(match x { A::B => 1, _ => 0 }) }\n",
    "fn mj() -> i64 { match x { Color::Red => 1, _ => 0, } }\n",
    "fn mk() -> i64 { match parse(t) { Node::Bin(op, l, r) => eval(l) + eval(r), Node::Leaf(v) => v, _ => 0 } }\n",
    // (increment-6) struct literals: single/multi field, expr values, nested.
    "fn sa() -> i64 { Point { x: 1, y: 2 } }\n",
    "fn sb() -> i64 { Wrapper { v: 42 } }\n",
    "fn sc() -> i64 { P { x: a + 1, y: g(2) } }\n",
    "fn sd() -> i64 { Outer { inner: Inner { y: 2 } } }\n",
    "fn se2() -> i64 { g(Point { x: 1 }) }\n",
    "fn sf2() -> i64 { [P { a: 1 }, Q { b: 2 }] }\n",
    "fn sg() -> i64 { P { x: 1, } }\n",
    // (increment-6) struct-lit-vs-block disambiguation (heads stay conditions).
    "fn sh() -> i64 { if x { P { a: 1 } } else { Q { b: 2 } } }\n",
    "fn si() -> i64 { (P { x: 1 }).x }\n",
    "fn sj() -> i64 { match s { St::A => P { v: 1 }, _ => Q { v: 2 } } }\n",
    // (increment-7) declassify / perform / scope / spawn / .await.
    "fn da() -> i64 { declassify(s) }\n",
    "fn db() -> i64 { declassify(a + b) }\n",
    "fn pe() -> i64 { perform Net.recv() }\n",
    "fn pf() -> i64 { perform Net.send(x, y) }\n",
    "fn sk() -> i64 { scope concurrent { 42 } }\n",
    "fn sl() -> i64 { scope concurrent { h.await } }\n",
    "fn sp() -> i64 { spawn worker(x) }\n",
    "fn aw() -> i64 { t.await }\n",
    "fn aw2() -> i64 { spawn f(x).await }\n",
    // (increment-7) composed with the rest of the expression grammar.
    "fn cp() -> i64 { g(perform E.op(1)) }\n",
    "fn cq() -> i64 { declassify(x) + perform E.op() }\n",
    "fn cr() -> bool { match s { St::Done => declassify(perform Tls.verify(mac)), _ => false } }\n",
    "fn cs() -> i64 { if ready { spawn w(x) } else { compute().await } }\n",
    "fn ct() -> i64 { scope concurrent { declassify(perform Net.recv()) } }\n",
    // (increment-8) handle — closes the expression grammar.
    "fn ha() -> i64 { handle compute() with { Net.recv(k) => 5 } }\n",
    "fn hb() -> i64 { handle b with { Net.recv(k) => 1, Net.send(v, k) => 2 } }\n",
    "fn hc() -> i64 { handle b with { E.op(k) => 1, return v => v } }\n",
    "fn hd() -> i64 { handle b with { return v => v } }\n",
    "fn he2() -> i64 { handle b with { E.op() => 1 } }\n",
    // return NOT last in source still dumps last (the &mut Ret separation).
    "fn hf() -> i64 { handle b with { return v => v, E.op(k) => 1 } }\n",
    // composed with the rest of the grammar.
    "fn hg() -> i64 { g(handle b with { E.op(k) => declassify(s) }) }\n",
    "fn hh() -> i64 { handle perform Net.recv() with { Net.recv(k) => 5, return v => v + 1 } }\n",
    // (2c-1) statements: let (un-annotated), assign, while, break, continue, expr-stmt.
    "fn la() -> i64 { let x = 5; x }\n",
    "fn lb() -> i64 { let mut y = 0; y }\n",
    "fn lc() -> i64 { let a = 1; let b = 2; a + b }\n",
    "fn as2() -> i64 { x = 5; x }\n",
    "fn es() -> i64 { g(1); h(2); 0 }\n",
    "fn wa() -> i64 { while c { x = x + 1; } 0 }\n",
    "fn wb2() -> i64 { while i < n { break; } 0 }\n",
    "fn wc2() -> i64 { while c { continue; } 0 }\n",
    // statement-only while body keeps the synthesized (int 0) tail.
    "fn wd() -> i64 { let mut s = 0; while i < n { s = s + i; } s }\n",
    // statements inside an if-branch / nested.
    "fn ni() -> i64 { if c { let x = 1; x } else { 2 } }\n",
    "fn nx() -> i64 { let mut s = 0; while i < n { s = s + g(i); spawn w(i); } declassify(s) }\n",
    // (2c-2) let type annotations + parse_type (ident, array, generic, nullable,
    // ref/refmut, secret, and nesting).
    "fn ta() -> i64 { let x: i64 = 5; x }\n",
    "fn tb() -> i64 { let mut y: bool = true; 0 }\n",
    "fn tc() -> i64 { let a: [u8] = s; 0 }\n",
    "fn td() -> i64 { let v: Vec<i64> = w; 0 }\n",
    "fn te() -> i64 { let p: ?Point = null; 0 }\n",
    "fn tf() -> i64 { let r: &i64 = x; 0 }\n",
    "fn tg() -> i64 { let r: &mut i64 = x; 0 }\n",
    "fn th() -> i64 { let sx: secret i64 = x; 0 }\n",
    "fn ti() -> i64 { let m: Vec<[u8]> = x; 0 }\n",
    "fn tj() -> i64 { let b: Box<Vec<i64>> = x; 0 }\n",
    "fn tk() -> i64 { let mp: Map<i64, [u8]> = x; 0 }\n",
    "fn tl() -> i64 { let sa: secret [u8] = x; 0 }\n",
    "fn tm() -> i64 { let x = 5; let y: i64 = x; y }\n",
    // (2c-3) fn definitions: params, mut params, complex return types.
    "fn add(a: i64, b: i64) -> i64 { a + b }\n",
    "fn fm(mut x: i64) -> i64 { x = x + 1; x }\n",
    "fn gr() -> [u8] { s }\n",
    "fn hv(v: Vec<i64>) -> i64 { len(v) }\n",
    "fn wp(p: &mut Point) -> i64 { 0 }\n",
    "fn ctf(s: secret i64) -> secret i64 { s }\n",
    "fn nf() -> ?Point { null }\n",
    // (2c-3) generic type-params + effect row are parsed but NOT dumped.
    "fn idf<T>(x: T) -> T { x }\n",
    "fn prf<K, V>(k: K, v: V) -> i64 { 0 }\n",
    "fn rf() -> i64 ! { Net } { perform Net.recv() }\n",
    // (2c-3) multi-fn programs with params.
    "fn add2(a: i64, b: i64) -> i64 { a + b }\nfn main() -> i64 { add2(1, 2) }\n",
    "fn hdl<T>(s: secret [u8], n: i64) -> Vec<i64> ! { Net, Log } { let mut acc: i64 = 0; while i < n { acc = acc + g(i); } w }\n",
    // (2d-1) `use` imports — dumped `(use seg…)` in source order, before the fns.
    "use a::b::Item;\nfn main() -> i64 { 0 }\n",
    "use std::io::File;\nfn main() -> i64 { 1 }\n",
    "use a::b::c::d::Thing;\nfn f() -> i64 { 2 }\n",
    "use a::b::C;\nuse d::e::F;\nfn main() -> i64 { add(1, 2) }\n",
    // (2d-2) `struct` decls — fields (name: type via parse_type), empty, trailing
    // comma, generics skipped, and source-order interleaving with fns.
    "struct Point { x: i64, y: i64 }\nfn main() -> i64 { 0 }\n",
    "struct Empty {}\nfn main() -> i64 { 0 }\n",
    "struct One { v: i64 }\nfn main() -> i64 { 0 }\n",
    "struct Bag { items: Vec<i64>, tag: [u8] }\nfn main() -> i64 { 0 }\n",
    "struct TrailC { a: i64, b: bool, }\nfn main() -> i64 { 0 }\n",
    "struct S { p: ?Point, r: &i64, m: secret [u8], n: Map<i64, Vec<u8>> }\nfn main() -> i64 { 0 }\n",
    "struct A { x: i64 }\nfn f() -> i64 { 1 }\nstruct B { y: bool }\nfn main() -> i64 { 0 }\n",
    // (2d-3) `enum` decls — unit + payload variants, empty, trailing comma,
    // recursive payloads, and interleaving with struct/fn.
    "enum Color { Red, Green, Blue }\nfn main() -> i64 { 0 }\n",
    "enum Opt { None, Some(i64) }\nfn main() -> i64 { 0 }\n",
    "enum E {}\nfn main() -> i64 { 0 }\n",
    "enum Mix { A, B(i64, [u8]), C(Vec<i64>) }\nfn main() -> i64 { 0 }\n",
    "enum Trail { A, B, }\nfn main() -> i64 { 0 }\n",
    "enum Node { Leaf(i64), Bin(Node, Node) }\nfn main() -> i64 { 0 }\n",
    "struct S { x: i64 }\nenum E { A(S), B }\nfn f() -> i64 { 1 }\nfn main() -> i64 { 0 }\n",
    // (2d-4) `effect` decls — ops with/without params + return type (missing
    // return dumps `_`), empty, trailing `;`, and interleaving with struct/fn.
    "effect Net { recv() -> i64; send(x: i64, y: [u8]) -> i64; }\nfn main() -> i64 { 0 }\n",
    "effect Log { write(msg: [u8]); }\nfn main() -> i64 { 0 }\n",
    "effect Empty {}\nfn main() -> i64 { 0 }\n",
    "effect E { a(); b() -> bool; c(x: secret i64); }\nfn main() -> i64 { 0 }\n",
    "effect Tls { verify(mac: [u8]) -> bool }\nfn main() -> i64 { 0 }\n",
    "effect Net { recv() -> i64; }\nstruct S { x: i64 }\nfn handler() -> i64 ! { Net } { perform Net.recv() }\nfn main() -> i64 { 0 }\n",
    // (2d-5) `trait` decls — &Self / &mut Self receivers (shared/exclusive), extra
    // params, complex return types, effect-row methods, empty, and interleaving.
    "trait Writer { fn write(self: &mut Self, n: i64) -> i64; fn cap(self: &Self) -> i64; }\nfn main() -> i64 { 0 }\n",
    "trait Marker {}\nfn main() -> i64 { 0 }\n",
    "trait Reader { fn read(self: &Self) -> [u8]; }\nfn main() -> i64 { 0 }\n",
    "trait Multi { fn f(self: &Self, a: i64, b: secret [u8]) -> Vec<i64>; }\nfn main() -> i64 { 0 }\n",
    "trait Eff { fn go(self: &mut Self) -> i64 ! { Net, Log }; }\nfn main() -> i64 { 0 }\n",
    "enum E { A }\ntrait T { fn m(self: &Self) -> i64; }\nstruct S { x: i64 }\nfn main() -> i64 { 0 }\n",
    // (2d-6) `impl` decls — default (`_` name) + named, empty, multi-method with
    // bodies, a `pub` method, and trait+impl+struct interleaving.
    "impl as Writer for FileSink { fn write(self: &mut Self, n: i64) -> i64 { n } }\nfn main() -> i64 { 0 }\n",
    "impl Doubling as Writer for FileSink { fn write(self: &mut Self, n: i64) -> i64 { n + n } }\nfn main() -> i64 { 0 }\n",
    "impl as Marker for Thing {}\nfn main() -> i64 { 0 }\n",
    "impl as W for S { fn a(self: &Self) -> i64 { 1 } fn b(self: &mut Self, x: i64) -> i64 { let y = x + 1; y } }\nfn main() -> i64 { 0 }\n",
    "impl as W for S { pub fn a(self: &Self) -> i64 { 1 } }\nfn main() -> i64 { 0 }\n",
    "trait Writer { fn write(self: &mut Self, n: i64) -> i64; }\nimpl as Writer for FileSink { fn write(self: &mut Self, n: i64) -> i64 { n } }\nstruct FileSink { fd: i64 }\nfn main() -> i64 { 0 }\n",
    // (2d-7) `class` decls — fields/init/methods/delegates BUCKET in the AST, so
    // they dump grouped in that fixed order (NOT source order). The `Weird` seed
    // (method before field in source) proves the bucketing; `pub` items + empty
    // + interleaving covered too.
    "class Point { let x: i64; let y: i64; init(a: i64, b: i64) { let p = a; p } fn sum(self: &Self) -> i64 { 0 } }\nfn main() -> i64 { 0 }\n",
    "class Empty {}\nfn main() -> i64 { 0 }\n",
    "class Wrap { let inner: FileSink; delegate w: FileSink to Writer; }\nfn main() -> i64 { 0 }\n",
    "class C { let v: i64; init(n: i64) { n } }\nfn main() -> i64 { 0 }\n",
    "class M { fn a(self: &Self) -> i64 { 1 } fn b(self: &mut Self, x: i64) -> i64 { x } }\nfn main() -> i64 { 0 }\n",
    "class Pub { pub let x: i64; pub fn get(self: &Self) -> i64 { 0 } }\nfn main() -> i64 { 0 }\n",
    "class Weird { fn m(self: &Self) -> i64 { 0 } let x: i64; }\nfn main() -> i64 { 0 }\n",
    "struct S { a: i64 }\nclass C { let x: i64; init(n: i64) { n } }\ntrait T { fn f(self: &Self) -> i64; }\nfn main() -> i64 { 0 }\n",
];

#[test]
fn sentinel_parser_matches_oracle_on_seeds() {
    let tmp = std::env::temp_dir().join(format!("snc_selfhost_parse_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(&tmp).expect("create temp dir");
    let parser = build_sentinel_parser(&tmp);

    let work = tmp.join("work");
    std::fs::create_dir_all(&work).expect("create work dir");
    let input = work.join("input.sentinel");

    let mut mismatches: Vec<String> = Vec::new();
    for seed in SEEDS {
        std::fs::write(&input, seed).expect("stage seed");

        let oracle = Command::new(env!("CARGO_BIN_EXE_snc"))
            .arg("ast")
            .arg(&input)
            .output()
            .expect("run snc ast");
        let sentinel = Command::new(&parser)
            .current_dir(&work)
            .output()
            .expect("run the Sentinel parser");

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
        "the Sentinel parser diverged from `snc ast` on {}/{} seed(s):\n{}",
        mismatches.len(),
        SEEDS.len(),
        mismatches.join("\n")
    );
}

/// Every `*.sentinel` fixture in `tests/pass` + `tests/ui` (sorted).
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

/// (2d-8) The corpus-wide differential — the parser-stage phase-go (ADR 0039
/// D8), mirroring `tests/selfhost_lex.rs`. For every fixture where the Rust
/// `snc ast` oracle succeeds (a clean-parsing fixture), assert the Sentinel
/// parser's dump is byte-identical. Fixtures the oracle rejects — the two
/// deliberate negative fixtures `lex_invalid_char.sentinel` (a lex error) and
/// `parse_unbalanced_paren.sentinel` (a parse error) — are skipped: parser
/// ERROR parity is out of scope (happy-path AST production first, ADR 0039 D7),
/// so the Sentinel parser is only run where the oracle produced a full dump.
#[test]
fn sentinel_parser_matches_oracle_on_corpus() {
    let tmp = std::env::temp_dir().join(format!("snc_selfhost_parse_corpus_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(&tmp).expect("create temp dir");
    let parser = build_sentinel_parser(&tmp);

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
            .arg("ast")
            .arg(&input)
            .output()
            .expect("run snc ast");
        // Skip fixtures the oracle rejects (negative / parse-error fixtures) —
        // the Sentinel parser only mirrors happy-path AST production.
        if !oracle.status.success() {
            continue;
        }
        clean += 1;

        let sentinel = Command::new(&parser)
            .current_dir(&work)
            .output()
            .expect("run the Sentinel parser");

        if oracle.stdout != sentinel.stdout {
            mismatches.push(format!(
                "  {} (oracle {} bytes vs sentinel {} bytes)",
                fixture.file_name().unwrap().to_string_lossy(),
                oracle.stdout.len(),
                sentinel.stdout.len()
            ));
        }
    }

    assert!(clean > 100, "expected >100 clean-parsing fixtures, got {clean}");
    assert!(
        mismatches.is_empty(),
        "the Sentinel parser diverged from `snc ast` on {}/{} clean-parsing fixture(s):\n{}",
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
// of its causes at once; `fn_value_generic.sentinel` is the live one, narrowed
// by that same slice from float+ADR-0070 down to ADR 0070 alone.)

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
/// EMPTY, and the whole list emptied within two slices. It held eight ADR 0058
/// float-literal entries (closed by the float mirror) and five ADR 0057
/// `ptr_of` / `ptr_of_mut` / `is_null` entries (closed by this one) — the two
/// halves of the RESERVED-NAME family plus the float literal, which is every
/// front-end surface the real-program sweep found when it was first run. Each
/// left the list the only way an entry may: by being FIXED, and each is now
/// pinned in the FIXTURE corpus too (`tests/pass/c58_float_math.sentinel`,
/// `tests/pass/c57_ptr_of.sentinel`), so the blind spot that hid them — no
/// fixture used any of them — is closed as well.
const DEFERRED_PROGRAMS: &[(&str, &str)] = &[];

/// Programs whose divergence is a REAL BUG in the self-hosted parser, not a
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
///
/// EMPTY. The three `examples/export/*` programs used to head this list — the
/// ADR 0059 `export "C"` item-dispatch hole, where `dump_item` had an `extern`
/// arm but no `export` one, so `export` fell through to `dump_fn_decl`, which
/// read the ABI string as the fn NAME. It is DELETED, not re-labelled, which is
/// the only way an entry may leave: `selfhost/parser/dump.sentinel` grew the
/// arm, and `tests/pass/c59_export_call.sentinel` now pins it in the FIXTURE
/// corpus so the gap that hid it (no fixture used `export`) is closed too.
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
fn sentinel_parser_matches_oracle_on_real_programs() {
    let tmp = std::env::temp_dir().join(format!("snc_selfhost_parse_prog_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(&tmp).expect("create temp dir");
    let parser = build_sentinel_parser(&tmp);
    let work = tmp.join("work");
    std::fs::create_dir_all(&work).expect("create work dir");
    // 121 -> 122 (register D54, `examples/export/mut_buffer_lib.sentinel`) -> 123
    // (request R4, `examples/security/chacha20poly1305_open.sentinel`). The count is a TRIPWIRE, not bookkeeping —
    // it is what catches a program silently dropping out of the sweep — so bumping
    // it is only correct when you know why it moved. Here: one file added.
    real_program_differential("ast", &parser, &work, 123, 23);
    let _ = std::fs::remove_dir_all(&tmp);
}
