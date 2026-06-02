//! Phase D self-host port (2a) / ADR 0039 D8: the parser differential test.
//! Compile `selfhost/parser.sentinel` with the Rust `snc`, then assert its
//! canonical AST dump is byte-identical to `snc ast` for a seed set of
//! programs. The seeds are within (2a) scope — paramless fns whose body is an
//! integer arithmetic expression (`+ - * /`, precedence, parens, left-assoc,
//! multi-fn). Vars / params / statements / the other expr+decl kinds grow the
//! parser (and this corpus) in (2b)–(2d) toward the full `tests/pass` +
//! `tests/ui` set, the way `tests/selfhost_lex.rs` covers the whole corpus.

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

/// The (2a) seed programs: arithmetic expression bodies in paramless fns.
const SEEDS: &[&str] = &[
    "fn main() -> i64 { 1 + 2 * 3 }\n",
    "fn f() -> i64 { (1 + 2) * 3 }\n",
    "fn g() -> i64 { 7 - 3 - 1 }\n",
    "fn h() -> i64 { 2 * 3 + 4 * 5 }\n",
    "fn a() -> i64 { 1 }\nfn b() -> i64 { 2 + 3 }\n",
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
