//! Phase D self-host port (2b) / ADR 0039 D8: the parser differential test.
//! Compile `selfhost/parser.sentinel` with the Rust `snc`, then assert its
//! canonical AST dump is byte-identical to `snc ast` for a seed set of
//! programs. The (2b) increment-1 seeds are paramless fns whose body is an
//! expression over the COMPLETE operator-precedence ladder — `|| && | ^ & ==
//! != < <= > >= + - * /`, prefix unary `- !`, parens — plus the scalar atom
//! leaves (integer / `true` / `false` / `null` literals + variable refs). The
//! tree shape (not source order) is what the dump pins, so a single seed
//! mixing every level proves the whole ladder. Calls, field/index/method,
//! `if`/`match`, struct/array literals, perform/handle, and the statement +
//! decl grammar grow the parser (and this corpus) in the later (2b)–(2d)
//! slices toward the full `tests/pass` + `tests/ui` set, the way
//! `tests/selfhost_lex.rs` covers the whole corpus for `snc lex`.

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
