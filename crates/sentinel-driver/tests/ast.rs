//! Phase D self-host port (2/N) / ADR 0039 D2: golden tests for the
//! `snc ast` canonical AST-dump oracle — the regular S-expression form the
//! Sentinel-written parser (`selfhost/parser.sentinel`) must reproduce
//! byte-for-byte. Pinning it here means the format can't drift out from
//! under the port. (Distinct from `snc parse`, the human pretty-print.)

use std::path::PathBuf;
use std::process::Command;

fn temp_file(name: &str, contents: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("snc_ast_{}_{}", std::process::id(), name));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("create temp dir");
    let path = dir.join("input.sentinel");
    std::fs::write(&path, contents).expect("write source");
    path
}

fn ast_dump(name: &str, contents: &str) -> String {
    let path = temp_file(name, contents);
    let out = Command::new(env!("CARGO_BIN_EXE_snc"))
        .arg("ast")
        .arg(&path)
        .output()
        .expect("run snc ast");
    assert!(
        out.status.success(),
        "snc ast failed; stderr:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8(out.stdout).expect("utf-8 dump")
}

#[test]
fn ast_dump_arithmetic_precedence() {
    // `*` binds tighter than `+` — the tree, not the source order, is what
    // the dump pins.
    assert_eq!(
        ast_dump("arith", "fn main() -> i64 { 1 + 2 * 3 }\n"),
        "(fn main () i64 (block (binop + (int 1) (binop * (int 2) (int 3)))))\n"
    );
}

#[test]
fn ast_dump_params_let_call() {
    // Params, a `let` statement (with its type annotation), and a call.
    assert_eq!(
        ast_dump(
            "fns",
            "fn add(a: i64, b: i64) -> i64 { a + b }\nfn main() -> i64 { let x: i64 = 5; add(x, 2) }\n"
        ),
        "(fn add ((param a i64) (param b i64)) i64 (block (binop + (var a) (var b))))\n\
         (fn main () i64 (block (let x i64 (int 5)) (call add (var x) (int 2))))\n"
    );
}
