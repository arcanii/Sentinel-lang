//! Phase D self-host port (3/N) / ADR 0040 D2: golden tests for the
//! `snc resolve` canonical resolved-AST-dump oracle — the regular,
//! ID-bearing S-expression form the Sentinel-written resolve stage
//! (`selfhost/resolve.sentinel`) must reproduce byte-for-byte. It is the
//! `snc ast` form (see `tests/ast.rs`) extended with the resolved IDs:
//! `(var #N)` (VarId), `(call #N …)` (FnId), `(struct-lit #N …)` (StructId),
//! and the parser's uniform `qcall` / `class-init` disambiguated. Builtins
//! occupy `FnId 0..=37`, so user fns start at `#38`; the auto-registered
//! built-in `Async` then `Subprocess` effects are emitted last in every program.

use std::path::PathBuf;
use std::process::Command;

fn temp_file(name: &str, contents: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("snc_resolve_{}_{}", std::process::id(), name));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("create temp dir");
    let path = dir.join("input.sentinel");
    std::fs::write(&path, contents).expect("write source");
    path
}

fn resolve_dump(name: &str, contents: &str) -> String {
    let path = temp_file(name, contents);
    let out = Command::new(env!("CARGO_BIN_EXE_snc"))
        .arg("resolve")
        .arg(&path)
        .output()
        .expect("run snc resolve");
    assert!(
        out.status.success(),
        "snc resolve failed; stderr:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8(out.stdout).expect("utf-8 dump")
}

#[test]
fn resolve_dump_params_var_call() {
    // Params bind VarIds (#0, #1); a free call resolves to the callee's FnId
    // (user fns start at #38, after the 38 builtins — 0..13 + sockets 14..20 +
    // channels 21..24 + subprocess spawn/wait 25..26 + write/read 27..28 +
    // send/recv 29..30 + sealed bridge 31..32 + stdin/stdout 33..34 + arg 35..36 +
    // apply 37 (ADR 0070)); `main` is #39. The built-in `Async` then `Subprocess`
    // (ADR 0066 M2.1) effects are emitted last.
    assert_eq!(
        resolve_dump(
            "fns",
            "fn add(a: i64, b: i64) -> i64 { a + b }\nfn main() -> i64 { add(1, 2) }\n"
        ),
        "(fn #38 add ((param #0 a i64) (param #1 b i64)) i64 (block (binop + (var #0) (var #1))))\n\
         (fn #39 main () i64 (block (call #38 (int 1) (int 2))))\n\
         (effect #0 Async)\n\
         (effect #1 Subprocess)\n"
    );
}

#[test]
fn resolve_dump_let_bindings() {
    // `let` bindings take the next VarId; a later reference resolves to it.
    assert_eq!(
        resolve_dump("lets", "fn main() -> i64 { let x: i64 = 5; let y = x + 1; y }\n"),
        "(fn #38 main () i64 (block (let #0 i64 (int 5)) (let #1 _ (binop + (var #0) (int 1))) (var #1)))\n\
         (effect #0 Async)\n\
         (effect #1 Subprocess)\n"
    );
}

#[test]
fn resolve_dump_builtin_call() {
    // A call to a runtime builtin resolves to its reserved FnId (`print` = #0).
    assert_eq!(
        resolve_dump("builtin", "fn main() -> i64 { print(42) }\n"),
        "(fn #38 main () i64 (block (call #0 (int 42))))\n(effect #0 Async)\n(effect #1 Subprocess)\n"
    );
}

#[test]
fn resolve_dump_struct_lit_and_field() {
    // A struct decl gets a StructId; a struct literal + field access carry it /
    // the field name. Source-order dump (struct before fn).
    assert_eq!(
        resolve_dump(
            "struct",
            "struct Box { v: i64 }\nfn main() -> i64 { let b = Box { v: 42 }; b.v }\n"
        ),
        "(struct #0 Box (field v i64))\n\
         (fn #38 main () i64 (block (let #0 _ (struct-lit #0 Box (field v (int 42)))) (field (var #0) v)))\n\
         (effect #0 Async)\n\
         (effect #1 Subprocess)\n"
    );
}
