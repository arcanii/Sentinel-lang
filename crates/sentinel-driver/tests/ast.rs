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

#[test]
fn ast_dump_use_decls_source_order() {
    // (2d-1) `use a::b::Item;` → `(use a b Item)`, each segment space-separated.
    // Decls dump in source order (uses before the fn here); the `Program` is
    // kind-bucketed, so `dump` re-sorts by span start to recover it.
    assert_eq!(
        ast_dump(
            "uses",
            "use a::b::Item;\nuse std::io::File;\nfn main() -> i64 { 0 }\n"
        ),
        "(use a b Item)\n(use std io File)\n(fn main () i64 (block (int 0)))\n"
    );
}

#[test]
fn ast_dump_struct_decls() {
    // (2d-2) `struct Name { f: T, … }` → `(struct Name (field f <type>) …)`;
    // empty → `(struct Name)`; field types route through `dump_type`.
    assert_eq!(
        ast_dump(
            "structs",
            "struct Point { x: i64, y: i64 }\nstruct Empty {}\nstruct Bag { items: Vec<i64>, tag: [u8] }\nfn main() -> i64 { 0 }\n"
        ),
        "(struct Point (field x i64) (field y i64))\n\
         (struct Empty)\n\
         (struct Bag (field items (generic Vec i64)) (field tag (arr u8)))\n\
         (fn main () i64 (block (int 0)))\n"
    );
}

#[test]
fn ast_dump_enum_decls() {
    // (2d-3) unit variants → `(variant V)`; payload variants list positional
    // types; empty → `(enum Name)`.
    assert_eq!(
        ast_dump(
            "enums",
            "enum Color { Red, Green, Blue }\nenum Mix { A, B(i64, [u8]), C(Vec<i64>) }\nenum E {}\nfn main() -> i64 { 0 }\n"
        ),
        "(enum Color (variant Red) (variant Green) (variant Blue))\n\
         (enum Mix (variant A) (variant B i64 (arr u8)) (variant C (generic Vec i64)))\n\
         (enum E)\n\
         (fn main () i64 (block (int 0)))\n"
    );
}

#[test]
fn ast_dump_effect_decls() {
    // (2d-4) ops dump like fn decls inside the effect; a missing op return type
    // dumps `_`; empty → `(effect Name)`.
    assert_eq!(
        ast_dump(
            "effects",
            "effect Net { recv() -> i64; send(x: i64, y: [u8]) -> i64; }\neffect Log { write(msg: [u8]); }\neffect Empty {}\nfn main() -> i64 { 0 }\n"
        ),
        "(effect Net (op recv () i64) (op send ((param x i64) (param y (arr u8))) i64))\n\
         (effect Log (op write ((param msg (arr u8))) _))\n\
         (effect Empty)\n\
         (fn main () i64 (block (int 0)))\n"
    );
}

#[test]
fn ast_dump_trait_decls() {
    // (2d-5) trait method sigs (no body): `self` dumps as shared/exclusive, the
    // non-self params dump like fn params, the effect row is omitted; empty →
    // `(trait Name)`.
    assert_eq!(
        ast_dump(
            "traits",
            "trait Writer { fn write(self: &mut Self, n: i64) -> i64; fn cap(self: &Self) -> i64; }\ntrait Marker {}\nfn main() -> i64 { 0 }\n"
        ),
        "(trait Writer (method write exclusive ((param n i64)) i64) (method cap shared () i64))\n\
         (trait Marker)\n\
         (fn main () i64 (block (int 0)))\n"
    );
}

#[test]
fn ast_dump_impl_decls() {
    // (2d-6) a default impl dumps `_` in the name slot, a named impl its name;
    // impl methods carry a body block (vs trait sigs).
    assert_eq!(
        ast_dump(
            "impls",
            "impl as Writer for FileSink { fn write(self: &mut Self, n: i64) -> i64 { n } }\nimpl Doubling as Writer for FileSink { fn write(self: &mut Self, n: i64) -> i64 { n + n } }\nfn main() -> i64 { 0 }\n"
        ),
        "(impl _ Writer FileSink (method write exclusive ((param n i64)) i64 (block (var n))))\n\
         (impl Doubling Writer FileSink (method write exclusive ((param n i64)) i64 (block (binop + (var n) (var n)))))\n\
         (fn main () i64 (block (int 0)))\n"
    );
}
