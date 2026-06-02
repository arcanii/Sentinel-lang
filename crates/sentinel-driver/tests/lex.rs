//! Phase D self-host port (1/N) / ADR 0038 D3+D4: golden tests for the
//! `snc lex` token-dump oracle — the canonical format the Sentinel-written
//! lexer (`selfhost/lexer.sentinel`) must reproduce byte-for-byte. Pinning
//! it here means the format can't drift silently out from under the port.
//!
//! Dump grammar (one line per token, then a trailing `EOF`):
//!   `<KIND> <start> <end> [<lexeme>]`
//! KIND = the `TokenKind` variant name; spans = byte offsets; `<lexeme>`
//! (the raw source slice) only for `Ident` / `IntLit` / `StringLit` / `CharLit`.

use std::path::PathBuf;
use std::process::Command;

fn temp_file(name: &str, contents: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("snc_lex_{}_{}", std::process::id(), name));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("create temp dir");
    let path = dir.join("input.sentinel");
    std::fs::write(&path, contents).expect("write source");
    path
}

/// Run `snc lex <file>` on `contents` and return its stdout dump.
fn lex_dump(name: &str, contents: &str) -> String {
    let path = temp_file(name, contents);
    let out = Command::new(env!("CARGO_BIN_EXE_snc"))
        .arg("lex")
        .arg(&path)
        .output()
        .expect("run snc lex");
    assert!(
        out.status.success(),
        "snc lex failed; stderr:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8(out.stdout).expect("utf-8 dump")
}

#[test]
fn lex_dump_let_int() {
    // Value-bearing Ident/IntLit carry their lexeme; keywords/operators don't.
    assert_eq!(
        lex_dump("let_int", "let x = 12;\n"),
        "Let 0 3\nIdent 4 5 x\nEq 6 7\nIntLit 8 10 12\nSemi 10 11\nEOF\n"
    );
}

#[test]
fn lex_dump_string_and_multichar_ops() {
    // StringLit lexeme includes its quotes; `==` lexes as one EqEq (longest
    // match), distinct from two `Eq`.
    assert_eq!(
        lex_dump("str_ops", "x == \"hi\"; foo(3)\n"),
        "Ident 0 1 x\nEqEq 2 4\nStringLit 5 9 \"hi\"\nSemi 9 10\n\
         Ident 11 14 foo\nLParen 14 15\nIntLit 15 16 3\nRParen 16 17\nEOF\n"
    );
}

#[test]
fn lex_dump_skips_comments_and_whitespace() {
    // A `//` line comment + surrounding whitespace are skipped; spans still
    // point at the real byte offsets of the surviving tokens.
    assert_eq!(
        lex_dump("comment", "// hi\nfn  main\n"),
        "Fn 6 8\nIdent 10 14 main\nEOF\n"
    );
}

#[test]
fn lex_dump_char_and_keyword() {
    // CharLit lexeme includes its quotes; `while` is a keyword, not an Ident.
    assert_eq!(
        lex_dump("char_kw", "while 'a'\n"),
        "While 0 5\nCharLit 6 9 'a'\nEOF\n"
    );
}
