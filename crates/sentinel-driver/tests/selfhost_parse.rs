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
//! over calls/qcalls/arrays, prove the whole grammar. Struct literals,
//! `if`/`match`, perform/handle, and the statement plus decl grammar grow the
//! parser (and this corpus) in the later (2b)-(2d) slices toward the full
//! `tests/pass` plus `tests/ui` set, the way `tests/selfhost_lex.rs` covers the
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
