//! Phase D self-host port (4/N) / ADR 0041 D2: golden tests for the
//! `snc types` canonical typed-AST-dump oracle — the regular,
//! type-annotated S-expression form the Sentinel-written types stage
//! (`selfhost/types.sentinel`) must reproduce byte-for-byte. It is the
//! `snc resolve` form (see `tests/resolve.rs`) extended with each
//! expression node's inferred `Type` (a trailing ` :<type>`) and the
//! type-resolved disambiguations: a `let`'s inferred type replaces
//! resolve's `_` placeholder; implicit coercions appear as
//! `(widen-null …)` / `(widen-secret …)`; receiver-typed method dispatch
//! splits into `(method #classid <idx> …)` / `(impl-method …)`; field
//! access carries its declaration index; and generic calls carry their
//! inferred `(targs …)`.

use std::path::PathBuf;
use std::process::Command;

fn temp_file(name: &str, contents: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("snc_types_{}_{}", std::process::id(), name));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("create temp dir");
    let path = dir.join("input.sentinel");
    std::fs::write(&path, contents).expect("write source");
    path
}

fn types_dump(name: &str, contents: &str) -> String {
    let path = temp_file(name, contents);
    let out = Command::new(env!("CARGO_BIN_EXE_snc"))
        .arg("types")
        .arg(&path)
        .output()
        .expect("run snc types");
    assert!(
        out.status.success(),
        "snc types failed; stderr:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8(out.stdout).expect("utf-8 dump")
}

#[test]
fn types_dump_params_var_call() {
    // Every expression node carries its inferred type as a trailing ` :T`
    // (even the redundant `(int 6 :i64)` — full regularity is the
    // validation). Params + calls keep their resolve IDs.
    assert_eq!(
        types_dump(
            "fns",
            "fn add(a: i64, b: i64) -> i64 { a + b }\nfn main() -> i64 { add(1, 2) }\n"
        ),
        "(fn #31 add ((param #0 a i64) (param #1 b i64)) i64 \
         (block (binop + (var #0 :i64) (var #1 :i64) :i64) :i64))\n\
         (fn #32 main () i64 (block (call #31 (int 1 :i64) (int 2 :i64) :i64) :i64))\n\
         (effect #0 Async)\n\
         (effect #1 Subprocess)\n"
    );
}

#[test]
fn types_dump_let_inference() {
    // The `let` shows its *inferred* type (resolve dumped `_` for the
    // unannotated `y`); here both are `i64`.
    assert_eq!(
        types_dump("lets", "fn main() -> i64 { let x: i64 = 5; let y = x + 1; y }\n"),
        "(fn #31 main () i64 (block (let #0 i64 (int 5 :i64)) \
         (let #1 i64 (binop + (var #0 :i64) (int 1 :i64) :i64)) (var #1 :i64) :i64))\n\
         (effect #0 Async)\n\
         (effect #1 Subprocess)\n"
    );
}

#[test]
fn types_dump_struct_lit_and_field() {
    // Struct-lit fields are reordered to declaration order + stripped of
    // names (positional values); field access carries its declaration
    // index (`v` is field 0). The struct-lit node is typed `:Box`.
    assert_eq!(
        types_dump(
            "struct",
            "struct Box { v: i64 }\nfn main() -> i64 { let b = Box { v: 42 }; b.v }\n"
        ),
        "(struct #0 Box (field v i64))\n\
         (fn #31 main () i64 (block (let #0 Box (struct-lit #0 Box (int 42 :i64) :Box)) \
         (field (var #0 :Box) v 0 :i64) :i64))\n\
         (effect #0 Async)\n\
         (effect #1 Subprocess)\n"
    );
}

#[test]
fn types_dump_nullable_widen() {
    // `let x: ?i64 = 42` synthesizes a `(widen-null …)` coercion; the
    // builtin `unwrap_or` (#1) call carries its inferred `(targs i64)`.
    assert_eq!(
        types_dump("nullable", "fn main() -> i64 { let x: ?i64 = 42; unwrap_or(x, 0) }\n"),
        "(fn #31 main () i64 (block (let #0 ?i64 (widen-null (int 42 :i64) :?i64)) \
         (call #1 (targs i64) (var #0 :?i64) (int 0 :i64) :i64) :i64))\n\
         (effect #0 Async)\n\
         (effect #1 Subprocess)\n"
    );
}

#[test]
fn types_dump_method_dispatch() {
    // Receiver-typed dispatch (`c.get()`) resolves to the class's own
    // method: `(method #<classid> <method_index> <receiver :Class> get …)`.
    // The receiver `(var #0 :Counter)` is typed; `Counter::init` is a
    // `(class-init …)`.
    assert_eq!(
        types_dump(
            "method",
            "class Counter {\n    let n: i64;\n    init(start: i64) { self.n = start; 0 }\n    \
             fn get(self: &Self) -> i64 { self.n }\n}\n\
             fn main() -> i64 { let c = Counter::init(7); c.get() }\n"
        ),
        "(class #0 Counter (field n i64) \
         (init #1 ((param #2 start i64)) (block (assign (field (var #1 :Counter) n 0 :i64) \
         (var #2 :i64)) (int 0 :i64) :i64)) \
         (method #3 get shared () i64 (block (field (var #3 :Counter) n 0 :i64) :i64)))\n\
         (fn #31 main () i64 (block (let #0 Counter (class-init #0 Counter (int 7 :i64) :Counter)) \
         (method #0 0 (var #0 :Counter) get :i64) :i64))\n\
         (effect #0 Async)\n\
         (effect #1 Subprocess)\n"
    );
}
