//! Phase D self-host port (8/N) / ADR 0045 D2+D3: tests for the `snc llvm`
//! codegen oracle — the canonical textual LLVM IR (`.ll`) that
//! `selfhost/codegen.sentinel` will reproduce byte-for-byte.
//!
//! Three layers (ADR 0045 D3):
//!   1. **Goldens** — pin the canonical `.ll` spec for straight-line seeds.
//!   2. **0-panics corpus sweep** — `snc llvm` over the whole corpus never
//!      crashes; it either emits (`exit 0`) or cleanly Errs (`exit 1`, an
//!      unsupported construct or an upstream reject → the differential skips
//!      it). The supported subset grows per sub-slice (8a..8l).
//!   3. **Behavioural parity** — every emitted `.ll`, compiled by `cc` and
//!      run, behaves identically (exit code + stdout) to the inkwell backend
//!      (`snc build`). Proves the textual backend is *correct*, not just a
//!      parser-pleaser. (8a = the straight-line subset.)
//!
//! Plus **layer 2b** between them: every `.ll` the oracle emits for a
//! `tests/pass` fixture parses AND verifies under `llvm-as` (register D10).
//! Layer 3 subsumes it in principle — but layer 3 needs `cc` and a
//! `libsentinel_runtime.a`, neither of which exists on Windows, where it fails on
//! its first assert and checks nothing. D10 was found and filed BY HAND, not by a
//! test: for as long as it was open no automated check in this repo rejected it,
//! on any platform. Layer 2b needs neither tool, so it is the one that runs here.

use std::path::{Path, PathBuf};
use std::process::Command;

fn temp_dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("snc_llvm_{}_{}", std::process::id(), name));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("create temp dir");
    dir
}

/// Run `snc llvm` on a source string; assert success and return the `.ll`.
fn llvm_dump(name: &str, contents: &str) -> String {
    let path = temp_dir(name).join("input.sentinel");
    std::fs::write(&path, contents).expect("write source");
    let out = Command::new(env!("CARGO_BIN_EXE_snc"))
        .arg("llvm")
        .arg(&path)
        .output()
        .expect("run snc llvm");
    assert!(
        out.status.success(),
        "snc llvm failed; stderr:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8(out.stdout).expect("utf-8 dump")
}

// ---- Layer 1: goldens (the canonical .ll spec) --------------------------

#[test]
fn llvm_const_main_truncates_to_i32() {
    // `main` is the C-ABI entry: i32 return, the i64 body truncated.
    assert_eq!(
        llvm_dump("const_main", "fn main() -> i64 {\n    42\n}\n"),
        concat!(
            "target triple = \"arm64-apple-darwin\"\n",
            "\n",
            "define i32 @main() {\n",
            "entry:\n",
            "  %v0 = trunc i64 42 to i32\n",
            "  ret i32 %v0\n",
            "}\n",
            "\n",
        )
    );
}

#[test]
fn llvm_params_arith_and_call() {
    // Params are alloca'd + stored (no phi); a call names its mangled callee.
    assert_eq!(
        llvm_dump(
            "call",
            "fn add(a: i64, b: i64) -> i64 {\n    a + b\n}\nfn main() -> i64 {\n    add(20, 22)\n}\n"
        ),
        concat!(
            "target triple = \"arm64-apple-darwin\"\n",
            "\n",
            "define i64 @add(i64 %arg0, i64 %arg1) {\n",
            "entry:\n",
            "  %v0 = alloca i64\n",
            "  %v1 = alloca i64\n",
            "  store i64 %arg0, ptr %v0\n",
            "  store i64 %arg1, ptr %v1\n",
            "  %v2 = load i64, ptr %v0\n",
            "  %v3 = load i64, ptr %v1\n",
            "  %v4 = add i64 %v2, %v3\n",
            "  ret i64 %v4\n",
            "}\n",
            "\n",
            "define i32 @main() {\n",
            "entry:\n",
            "  %v0 = call i64 @add(i64 20, i64 22)\n",
            "  %v1 = trunc i64 %v0 to i32\n",
            "  ret i32 %v1\n",
            "}\n",
            "\n",
        )
    );
}

#[test]
fn llvm_cmp_unary_and_bool_let() {
    // `<` → `icmp slt` (signed; i1 result); `!c` → `xor i1, 1`; a bool `let`
    // is an `i1` alloca slot.
    assert_eq!(
        llvm_dump(
            "cmp",
            "fn f(a: i64, b: i64) -> bool {\n    let c: bool = a < b;\n    !c\n}\nfn main() -> i64 {\n    0\n}\n"
        ),
        concat!(
            "target triple = \"arm64-apple-darwin\"\n",
            "\n",
            "define i1 @f(i64 %arg0, i64 %arg1) {\n",
            "entry:\n",
            "  %v0 = alloca i64\n",
            "  %v1 = alloca i64\n",
            "  %v5 = alloca i1\n",
            "  store i64 %arg0, ptr %v0\n",
            "  store i64 %arg1, ptr %v1\n",
            "  %v2 = load i64, ptr %v0\n",
            "  %v3 = load i64, ptr %v1\n",
            "  %v4 = icmp slt i64 %v2, %v3\n",
            "  store i1 %v4, ptr %v5\n",
            "  %v6 = load i1, ptr %v5\n",
            "  %v7 = xor i1 %v6, 1\n",
            "  ret i1 %v7\n",
            "}\n",
            "\n",
            "define i32 @main() {\n",
            "entry:\n",
            "  %v0 = trunc i64 0 to i32\n",
            "  ret i32 %v0\n",
            "}\n",
            "\n",
        )
    );
}

#[test]
fn llvm_if_else_memory_cell_merge() {
    // `if c { a } else { b }` — no phi: a hoisted result slot, a conditional
    // branch, each arm stores into the slot, the merge loads it. Block labels
    // `bbN`; the result alloca (`%v5`) is hoisted to entry though reserved
    // mid-walk (after the then-branch, so its type is known).
    assert_eq!(
        llvm_dump(
            "ifelse",
            "fn pick(c: bool, a: i64, b: i64) -> i64 {\n    if c { a } else { b }\n}\nfn main() -> i64 {\n    pick(true, 7, 9)\n}\n"
        ),
        concat!(
            "target triple = \"arm64-apple-darwin\"\n",
            "\n",
            "define i64 @pick(i1 %arg0, i64 %arg1, i64 %arg2) {\n",
            "entry:\n",
            "  %v0 = alloca i1\n",
            "  %v1 = alloca i64\n",
            "  %v2 = alloca i64\n",
            "  %v5 = alloca i64\n",
            "  store i1 %arg0, ptr %v0\n",
            "  store i64 %arg1, ptr %v1\n",
            "  store i64 %arg2, ptr %v2\n",
            "  %v3 = load i1, ptr %v0\n",
            "  br i1 %v3, label %bb0, label %bb1\n",
            "bb0:\n",
            "  %v4 = load i64, ptr %v1\n",
            "  store i64 %v4, ptr %v5\n",
            "  br label %bb2\n",
            "bb1:\n",
            "  %v6 = load i64, ptr %v2\n",
            "  store i64 %v6, ptr %v5\n",
            "  br label %bb2\n",
            "bb2:\n",
            "  %v7 = load i64, ptr %v5\n",
            "  ret i64 %v7\n",
            "}\n",
            "\n",
            "define i32 @main() {\n",
            "entry:\n",
            "  %v0 = call i64 @pick(i1 1, i64 7, i64 9)\n",
            "  %v1 = trunc i64 %v0 to i32\n",
            "  ret i32 %v1\n",
            "}\n",
            "\n",
        )
    );
}

#[test]
fn llvm_while_loop_cfg() {
    // `while c { … }` — the loop CFG: enter the cond block (`bb0`), which branches to
    // the body (`bb1`) or after (`bb2`); the body branches back to the cond (the
    // back-edge). Body allocas are hoisted to entry (no per-iteration growth).
    assert_eq!(
        llvm_dump(
            "while",
            "fn count(n: i64) -> i64 {\n    let mut i: i64 = 0;\n    while i < n {\n        i = i + 1;\n    }\n    i\n}\nfn main() -> i64 {\n    count(3)\n}\n"
        ),
        concat!(
            "target triple = \"arm64-apple-darwin\"\n",
            "\n",
            "define i64 @count(i64 %arg0) {\n",
            "entry:\n",
            "  %v0 = alloca i64\n",
            "  %v1 = alloca i64\n",
            "  store i64 %arg0, ptr %v0\n",
            "  store i64 0, ptr %v1\n",
            "  br label %bb0\n",
            "bb0:\n",
            "  %v2 = load i64, ptr %v1\n",
            "  %v3 = load i64, ptr %v0\n",
            "  %v4 = icmp slt i64 %v2, %v3\n",
            "  br i1 %v4, label %bb1, label %bb2\n",
            "bb1:\n",
            "  %v5 = load i64, ptr %v1\n",
            "  %v6 = add i64 %v5, 1\n",
            "  store i64 %v6, ptr %v1\n",
            "  br label %bb0\n",
            "bb2:\n",
            "  %v7 = load i64, ptr %v1\n",
            "  ret i64 %v7\n",
            "}\n",
            "\n",
            "define i32 @main() {\n",
            "entry:\n",
            "  %v0 = call i64 @count(i64 3)\n",
            "  %v1 = trunc i64 %v0 to i32\n",
            "  ret i32 %v1\n",
            "}\n",
            "\n",
        )
    );
}

#[test]
fn llvm_struct_decl_lit_and_field() {
    // 8c-1 aggregates: a user struct is a Pass-0 named type
    // (`%Struct.N = type { … }`) and a first-class SSA value — the literal
    // builds it via an `insertvalue` chain from `undef`, a field reads via
    // `extractvalue`, and `let`/param/return/call carry it by value
    // (alloca/store/load of `%Struct.0`), no GEP. dist({30,12}) = 42.
    assert_eq!(
        llvm_dump(
            "struct",
            "struct Point { x: i64, y: i64 }\nfn dist(p: Point) -> i64 {\n    p.x + p.y\n}\nfn main() -> i64 {\n    let p = Point { x: 30, y: 12 };\n    dist(p)\n}\n"
        ),
        concat!(
            "target triple = \"arm64-apple-darwin\"\n",
            "\n",
            "%Struct.0 = type { i64, i64 }\n",
            "\n",
            "define i64 @dist(%Struct.0 %arg0) {\n",
            "entry:\n",
            "  %v0 = alloca %Struct.0\n",
            "  store %Struct.0 %arg0, ptr %v0\n",
            "  %v1 = load %Struct.0, ptr %v0\n",
            "  %v2 = extractvalue %Struct.0 %v1, 0\n",
            "  %v3 = load %Struct.0, ptr %v0\n",
            "  %v4 = extractvalue %Struct.0 %v3, 1\n",
            "  %v5 = add i64 %v2, %v4\n",
            "  ret i64 %v5\n",
            "}\n",
            "\n",
            "define i32 @main() {\n",
            "entry:\n",
            "  %v2 = alloca %Struct.0\n",
            "  %v0 = insertvalue %Struct.0 undef, i64 30, 0\n",
            "  %v1 = insertvalue %Struct.0 %v0, i64 12, 1\n",
            "  store %Struct.0 %v1, ptr %v2\n",
            "  %v3 = load %Struct.0, ptr %v2\n",
            "  %v4 = call i64 @dist(%Struct.0 %v3)\n",
            "  %v5 = trunc i64 %v4 to i32\n",
            "  ret i32 %v5\n",
            "}\n",
            "\n",
        )
    );
}

#[test]
fn llvm_array_lit_index_and_len() {
    // 8c-2 arrays: `[T]` is the abi-v1 `{ i64, ptr }`. A literal heap-allocates
    // (GEP-sizeof + `sentinel_alloc`), GEP-stores each element, builds `{len,ptr}`;
    // `a[i]` bounds-checks (0 <= i < len, else `sentinel_panic_oob` + unreachable)
    // then GEPs+loads; `len` is `extractvalue 0`. The module declares only the
    // runtime symbols actually used (both here). xs[1] + len = 20 + 3 = 23.
    assert_eq!(
        llvm_dump(
            "array",
            "fn main() -> i64 {\n    let xs = [10, 20, 30];\n    xs[1] + len(xs)\n}\n"
        ),
        concat!(
            "target triple = \"arm64-apple-darwin\"\n",
            "\n",
            "declare ptr @sentinel_alloc(i64)\n",
            "declare void @sentinel_free(ptr)\n",
            "declare void @sentinel_panic_oob(i64, i64)\n",
            "\n",
            "define i32 @main() {\n",
            "entry:\n",
            "  %v8 = alloca { i64, ptr }\n",
            "  %v0 = getelementptr i64, ptr null, i64 3\n",
            "  %v1 = ptrtoint ptr %v0 to i64\n",
            "  %v2 = call ptr @sentinel_alloc(i64 %v1)\n",
            "  %v3 = getelementptr i64, ptr %v2, i64 0\n",
            "  store i64 10, ptr %v3\n",
            "  %v4 = getelementptr i64, ptr %v2, i64 1\n",
            "  store i64 20, ptr %v4\n",
            "  %v5 = getelementptr i64, ptr %v2, i64 2\n",
            "  store i64 30, ptr %v5\n",
            "  %v6 = insertvalue { i64, ptr } undef, i64 3, 0\n",
            "  %v7 = insertvalue { i64, ptr } %v6, ptr %v2, 1\n",
            "  store { i64, ptr } %v7, ptr %v8\n",
            "  %v9 = load { i64, ptr }, ptr %v8\n",
            "  %v10 = extractvalue { i64, ptr } %v9, 0\n",
            "  %v11 = extractvalue { i64, ptr } %v9, 1\n",
            "  %v12 = icmp sge i64 1, 0\n",
            "  %v13 = icmp slt i64 1, %v10\n",
            "  %v14 = and i1 %v12, %v13\n",
            "  br i1 %v14, label %bb1, label %bb0\n",
            "bb0:\n",
            "  call void @sentinel_panic_oob(i64 1, i64 %v10)\n",
            "  unreachable\n",
            "bb1:\n",
            "  %v15 = getelementptr i64, ptr %v11, i64 1\n",
            "  %v16 = load i64, ptr %v15\n",
            "  %v17 = load { i64, ptr }, ptr %v8\n",
            "  %v18 = extractvalue { i64, ptr } %v17, 0\n",
            "  %v19 = add i64 %v16, %v18\n",
            "  %v20 = load { i64, ptr }, ptr %v8\n",
            "  %v21 = extractvalue { i64, ptr } %v20, 1\n",
            "  call void @sentinel_free(ptr %v21)\n",
            "  %v22 = trunc i64 %v19 to i32\n",
            "  ret i32 %v22\n",
            "}\n",
            "\n",
        )
    );
}

#[test]
fn llvm_index_assign_array() {
    // ADR 0050: `a[i] = v;` — index assignment. The lvalue address is the SAME
    // bounds-checked element GEP the read path computes (extract len(0)/data(1),
    // check `0 <= i < len` else `sentinel_panic_oob` + unreachable, GEP into the
    // in-bounds block) — but the caller STOREs through it instead of loading.
    // Here `a[1] = 99` stores into bb1, then `a[1]` reads it back → 99.
    assert_eq!(
        llvm_dump(
            "index_assign",
            "fn main() -> i64 {\n    let mut a: [i64] = [10, 20, 30];\n    a[1] = 99;\n    a[1]\n}\n"
        ),
        concat!(
            "target triple = \"arm64-apple-darwin\"\n",
            "\n",
            "declare ptr @sentinel_alloc(i64)\n",
            "declare void @sentinel_free(ptr)\n",
            "declare void @sentinel_panic_oob(i64, i64)\n",
            "\n",
            "define i32 @main() {\n",
            "entry:\n",
            "  %v8 = alloca { i64, ptr }\n",
            "  %v0 = getelementptr i64, ptr null, i64 3\n",
            "  %v1 = ptrtoint ptr %v0 to i64\n",
            "  %v2 = call ptr @sentinel_alloc(i64 %v1)\n",
            "  %v3 = getelementptr i64, ptr %v2, i64 0\n",
            "  store i64 10, ptr %v3\n",
            "  %v4 = getelementptr i64, ptr %v2, i64 1\n",
            "  store i64 20, ptr %v4\n",
            "  %v5 = getelementptr i64, ptr %v2, i64 2\n",
            "  store i64 30, ptr %v5\n",
            "  %v6 = insertvalue { i64, ptr } undef, i64 3, 0\n",
            "  %v7 = insertvalue { i64, ptr } %v6, ptr %v2, 1\n",
            "  store { i64, ptr } %v7, ptr %v8\n",
            // `a[1] = 99;` — target GEP (bounds-checked) FIRST, then the value, then store.
            "  %v9 = load { i64, ptr }, ptr %v8\n",
            "  %v10 = extractvalue { i64, ptr } %v9, 0\n",
            "  %v11 = extractvalue { i64, ptr } %v9, 1\n",
            "  %v12 = icmp sge i64 1, 0\n",
            "  %v13 = icmp slt i64 1, %v10\n",
            "  %v14 = and i1 %v12, %v13\n",
            "  br i1 %v14, label %bb1, label %bb0\n",
            "bb0:\n",
            "  call void @sentinel_panic_oob(i64 1, i64 %v10)\n",
            "  unreachable\n",
            "bb1:\n",
            "  %v15 = getelementptr i64, ptr %v11, i64 1\n",
            "  store i64 99, ptr %v15\n",
            // `a[1]` read-back — same bounds-checked GEP, then load.
            "  %v16 = load { i64, ptr }, ptr %v8\n",
            "  %v17 = extractvalue { i64, ptr } %v16, 0\n",
            "  %v18 = extractvalue { i64, ptr } %v16, 1\n",
            "  %v19 = icmp sge i64 1, 0\n",
            "  %v20 = icmp slt i64 1, %v17\n",
            "  %v21 = and i1 %v19, %v20\n",
            "  br i1 %v21, label %bb3, label %bb2\n",
            "bb2:\n",
            "  call void @sentinel_panic_oob(i64 1, i64 %v17)\n",
            "  unreachable\n",
            "bb3:\n",
            "  %v22 = getelementptr i64, ptr %v18, i64 1\n",
            "  %v23 = load i64, ptr %v22\n",
            "  %v24 = load { i64, ptr }, ptr %v8\n",
            "  %v25 = extractvalue { i64, ptr } %v24, 1\n",
            "  call void @sentinel_free(ptr %v25)\n",
            "  %v26 = trunc i64 %v23 to i32\n",
            "  ret i32 %v26\n",
            "}\n",
            "\n",
        )
    );
}

#[test]
fn llvm_string_literal_is_a_u8_array() {
    // 8c-3: a string literal is a `[u8]` (ADR 0033) — the decoded bytes heap-copied
    // (`sentinel_alloc` + N constant `i8` stores) into a `{ i64, ptr }`, exactly an
    // array literal of byte constants. "hi" = [104, 105]; len = 2.
    assert_eq!(
        llvm_dump(
            "string",
            "fn main() -> i64 {\n    let s: [u8] = \"hi\";\n    len(s)\n}\n"
        ),
        concat!(
            "target triple = \"arm64-apple-darwin\"\n",
            "\n",
            "declare ptr @sentinel_alloc(i64)\n",
            "declare void @sentinel_free(ptr)\n",
            "\n",
            "define i32 @main() {\n",
            "entry:\n",
            "  %v7 = alloca { i64, ptr }\n",
            "  %v0 = getelementptr i8, ptr null, i64 2\n",
            "  %v1 = ptrtoint ptr %v0 to i64\n",
            "  %v2 = call ptr @sentinel_alloc(i64 %v1)\n",
            "  %v3 = getelementptr i8, ptr %v2, i64 0\n",
            "  store i8 104, ptr %v3\n",
            "  %v4 = getelementptr i8, ptr %v2, i64 1\n",
            "  store i8 105, ptr %v4\n",
            "  %v5 = insertvalue { i64, ptr } undef, i64 2, 0\n",
            "  %v6 = insertvalue { i64, ptr } %v5, ptr %v2, 1\n",
            "  store { i64, ptr } %v6, ptr %v7\n",
            "  %v8 = load { i64, ptr }, ptr %v7\n",
            "  %v9 = extractvalue { i64, ptr } %v8, 0\n",
            "  %v10 = load { i64, ptr }, ptr %v7\n",
            "  %v11 = extractvalue { i64, ptr } %v10, 1\n",
            "  call void @sentinel_free(ptr %v11)\n",
            "  %v12 = trunc i64 %v9 to i32\n",
            "  ret i32 %v12\n",
            "}\n",
            "\n",
        )
    );
}

#[test]
fn llvm_str_eq_runtime_builtin() {
    // 8d runtime builtins: a byte-array builtin decomposes its `[u8]` arg(s) into
    // the (ptr, len) the C symbol wants, then calls it. `str_eq` extracts len(0)/
    // ptr(1) from each `{ i64, ptr }` and calls `sentinel_str_eq(ptr,i64,ptr,i64)`
    // → i1. The module declares only the symbols it uses.
    assert_eq!(
        llvm_dump(
            "streq",
            "fn eq(a: [u8], b: [u8]) -> bool {\n    str_eq(a, b)\n}\nfn main() -> i64 {\n    0\n}\n"
        ),
        concat!(
            "target triple = \"arm64-apple-darwin\"\n",
            "\n",
            "declare void @sentinel_free(ptr)\n",
            "declare i1 @sentinel_str_eq(ptr, i64, ptr, i64)\n",
            "\n",
            "define i1 @eq({ i64, ptr } %arg0, { i64, ptr } %arg1) {\n",
            "entry:\n",
            "  %v0 = alloca { i64, ptr }\n",
            "  %v1 = alloca { i64, ptr }\n",
            "  store { i64, ptr } %arg0, ptr %v0\n",
            "  store { i64, ptr } %arg1, ptr %v1\n",
            "  %v2 = load { i64, ptr }, ptr %v0\n",
            "  %v3 = load { i64, ptr }, ptr %v1\n",
            "  %v4 = extractvalue { i64, ptr } %v2, 0\n",
            "  %v5 = extractvalue { i64, ptr } %v2, 1\n",
            "  %v6 = extractvalue { i64, ptr } %v3, 0\n",
            "  %v7 = extractvalue { i64, ptr } %v3, 1\n",
            "  %v8 = call i1 @sentinel_str_eq(ptr %v5, i64 %v4, ptr %v7, i64 %v6)\n",
            "  %v9 = load { i64, ptr }, ptr %v1\n",
            "  %v10 = extractvalue { i64, ptr } %v9, 1\n",
            "  call void @sentinel_free(ptr %v10)\n",
            "  %v11 = load { i64, ptr }, ptr %v0\n",
            "  %v12 = extractvalue { i64, ptr } %v11, 1\n",
            "  call void @sentinel_free(ptr %v12)\n",
            "  ret i1 %v8\n",
            "}\n",
            "\n",
            "define i32 @main() {\n",
            "entry:\n",
            "  %v0 = trunc i64 0 to i32\n",
            "  ret i32 %v0\n",
            "}\n",
            "\n",
        )
    );
}

#[test]
fn llvm_refs_address_of_and_deref() {
    // 8d refs: a reference is an opaque `ptr`. `&x`/`&mut x` is x's alloca slot (no
    // instruction); `*r` loads the pointee through r's pointer value; a ref param
    // arrives as `ptr`. add(&10, &32) = *a + *b = 42.
    assert_eq!(
        llvm_dump(
            "refs",
            "fn add(a: &i64, b: &i64) -> i64 {\n    *a + *b\n}\nfn main() -> i64 {\n    let a: i64 = 10;\n    let b: i64 = 32;\n    add(&a, &b)\n}\n"
        ),
        concat!(
            "target triple = \"arm64-apple-darwin\"\n",
            "\n",
            "define i64 @add(ptr %arg0, ptr %arg1) {\n",
            "entry:\n",
            "  %v0 = alloca ptr\n",
            "  %v1 = alloca ptr\n",
            "  store ptr %arg0, ptr %v0\n",
            "  store ptr %arg1, ptr %v1\n",
            "  %v2 = load ptr, ptr %v0\n",
            "  %v3 = load i64, ptr %v2\n",
            "  %v4 = load ptr, ptr %v1\n",
            "  %v5 = load i64, ptr %v4\n",
            "  %v6 = add i64 %v3, %v5\n",
            "  ret i64 %v6\n",
            "}\n",
            "\n",
            "define i32 @main() {\n",
            "entry:\n",
            "  %v0 = alloca i64\n",
            "  %v1 = alloca i64\n",
            "  store i64 10, ptr %v0\n",
            "  store i64 32, ptr %v1\n",
            "  %v2 = call i64 @add(ptr %v0, ptr %v1)\n",
            "  %v3 = trunc i64 %v2 to i32\n",
            "  ret i32 %v3\n",
            "}\n",
            "\n",
        )
    );
}

#[test]
fn llvm_vec_new_push_and_len() {
    // 8d-Vec: `Vec<T>` = `{ i64 len, i64 cap, ptr data }` (ADR 0034). vec_new is the
    // constant `{0,0,null}`; push grows the buffer (len==cap → `sentinel_realloc` to
    // `max(1,cap*2)*sizeof`) through the `&mut Vec`'s field GEPs, then stores + bumps
    // len (a grow/cont CFG, no phi); len reads field 0 of `{i64,i64,ptr}`.
    assert_eq!(
        llvm_dump(
            "vec",
            "fn main() -> i64 {\n    let mut v: Vec<i64> = vec_new();\n    push(&mut v, 7);\n    len(v)\n}\n"
        ),
        concat!(
            "target triple = \"arm64-apple-darwin\"\n",
            "\n",
            "declare ptr @sentinel_realloc(ptr, i64)\n",
            "declare void @sentinel_free(ptr)\n",
            "\n",
            "define i32 @main() {\n",
            "entry:\n",
            "  %v0 = alloca { i64, i64, ptr }\n",
            "  store { i64, i64, ptr } { i64 0, i64 0, ptr null }, ptr %v0\n",
            "  %v1 = getelementptr { i64, i64, ptr }, ptr %v0, i32 0, i32 0\n",
            "  %v2 = getelementptr { i64, i64, ptr }, ptr %v0, i32 0, i32 2\n",
            "  %v3 = getelementptr { i64, i64, ptr }, ptr %v0, i32 0, i32 1\n",
            "  %v4 = load i64, ptr %v1\n",
            "  %v5 = load i64, ptr %v3\n",
            "  %v6 = icmp eq i64 %v4, %v5\n",
            "  br i1 %v6, label %bb0, label %bb1\n",
            "bb0:\n",
            "  %v7 = load ptr, ptr %v2\n",
            "  %v8 = mul i64 %v5, 2\n",
            "  %v9 = icmp eq i64 %v5, 0\n",
            "  %v10 = select i1 %v9, i64 1, i64 %v8\n",
            "  %v11 = getelementptr i64, ptr null, i64 1\n",
            "  %v12 = ptrtoint ptr %v11 to i64\n",
            "  %v13 = mul i64 %v10, %v12\n",
            "  %v14 = call ptr @sentinel_realloc(ptr %v7, i64 %v13)\n",
            "  store i64 %v10, ptr %v3\n",
            "  store ptr %v14, ptr %v2\n",
            "  br label %bb1\n",
            "bb1:\n",
            "  %v15 = load ptr, ptr %v2\n",
            "  %v16 = getelementptr i64, ptr %v15, i64 %v4\n",
            "  store i64 7, ptr %v16\n",
            "  %v17 = add i64 %v4, 1\n",
            "  store i64 %v17, ptr %v1\n",
            "  %v18 = load { i64, i64, ptr }, ptr %v0\n",
            "  %v19 = extractvalue { i64, i64, ptr } %v18, 0\n",
            "  %v20 = load { i64, i64, ptr }, ptr %v0\n",
            "  %v21 = extractvalue { i64, i64, ptr } %v20, 2\n",
            "  call void @sentinel_free(ptr %v21)\n",
            "  %v22 = trunc i64 %v19 to i32\n",
            "  ret i32 %v22\n",
            "}\n",
            "\n",
        )
    );
}

#[test]
fn llvm_vec_to_array_bridge() {
    // 8d-Vec-2: `vec_to_array(v: Vec<T>) -> [T]` — the `Vec` -> `[T]` bridge. The Vec
    // is loaded by value (`{i64,i64,ptr}`); extract len (field 0) + data (field 2),
    // size = len * sizeof(T) via the GEP-sizeof idiom, `sentinel_alloc` the dest,
    // `llvm.memcpy` the live prefix (align 1 implicit), build the owned `[T]`
    // `{ i64 len, ptr data }`. Non-consuming (an independent copy). The `llvm.memcpy`
    // intrinsic declares LAST — after the `sentinel_*` runtime-symbol group.
    assert_eq!(
        llvm_dump(
            "vta",
            "fn main() -> i64 {\n    let mut v: Vec<i64> = vec_new();\n    push(&mut v, 7);\n    let a: [i64] = vec_to_array(v);\n    len(a)\n}\n"
        ),
        concat!(
            "target triple = \"arm64-apple-darwin\"\n",
            "\n",
            "declare ptr @sentinel_alloc(i64)\n",
            "declare ptr @sentinel_realloc(ptr, i64)\n",
            "declare void @sentinel_free(ptr)\n",
            "declare void @llvm.memcpy.p0.p0.i64(ptr, ptr, i64, i1)\n",
            "\n",
            "define i32 @main() {\n",
            "entry:\n",
            "  %v0 = alloca { i64, i64, ptr }\n",
            "  %v26 = alloca { i64, ptr }\n",
            "  store { i64, i64, ptr } { i64 0, i64 0, ptr null }, ptr %v0\n",
            "  %v1 = getelementptr { i64, i64, ptr }, ptr %v0, i32 0, i32 0\n",
            "  %v2 = getelementptr { i64, i64, ptr }, ptr %v0, i32 0, i32 2\n",
            "  %v3 = getelementptr { i64, i64, ptr }, ptr %v0, i32 0, i32 1\n",
            "  %v4 = load i64, ptr %v1\n",
            "  %v5 = load i64, ptr %v3\n",
            "  %v6 = icmp eq i64 %v4, %v5\n",
            "  br i1 %v6, label %bb0, label %bb1\n",
            "bb0:\n",
            "  %v7 = load ptr, ptr %v2\n",
            "  %v8 = mul i64 %v5, 2\n",
            "  %v9 = icmp eq i64 %v5, 0\n",
            "  %v10 = select i1 %v9, i64 1, i64 %v8\n",
            "  %v11 = getelementptr i64, ptr null, i64 1\n",
            "  %v12 = ptrtoint ptr %v11 to i64\n",
            "  %v13 = mul i64 %v10, %v12\n",
            "  %v14 = call ptr @sentinel_realloc(ptr %v7, i64 %v13)\n",
            "  store i64 %v10, ptr %v3\n",
            "  store ptr %v14, ptr %v2\n",
            "  br label %bb1\n",
            "bb1:\n",
            "  %v15 = load ptr, ptr %v2\n",
            "  %v16 = getelementptr i64, ptr %v15, i64 %v4\n",
            "  store i64 7, ptr %v16\n",
            "  %v17 = add i64 %v4, 1\n",
            "  store i64 %v17, ptr %v1\n",
            "  %v18 = load { i64, i64, ptr }, ptr %v0\n",
            "  %v19 = extractvalue { i64, i64, ptr } %v18, 0\n",
            "  %v20 = extractvalue { i64, i64, ptr } %v18, 2\n",
            "  %v21 = getelementptr i64, ptr null, i64 %v19\n",
            "  %v22 = ptrtoint ptr %v21 to i64\n",
            "  %v23 = call ptr @sentinel_alloc(i64 %v22)\n",
            "  call void @llvm.memcpy.p0.p0.i64(ptr %v23, ptr %v20, i64 %v22, i1 false)\n",
            "  %v24 = insertvalue { i64, ptr } undef, i64 %v19, 0\n",
            "  %v25 = insertvalue { i64, ptr } %v24, ptr %v23, 1\n",
            "  store { i64, ptr } %v25, ptr %v26\n",
            "  %v27 = load { i64, ptr }, ptr %v26\n",
            "  %v28 = extractvalue { i64, ptr } %v27, 0\n",
            "  %v29 = load { i64, ptr }, ptr %v26\n",
            "  %v30 = extractvalue { i64, ptr } %v29, 1\n",
            "  call void @sentinel_free(ptr %v30)\n",
            "  %v31 = load { i64, i64, ptr }, ptr %v0\n",
            "  %v32 = extractvalue { i64, i64, ptr } %v31, 2\n",
            "  call void @sentinel_free(ptr %v32)\n",
            "  %v33 = trunc i64 %v28 to i32\n",
            "  ret i32 %v33\n",
            "}\n",
            "\n",
        )
    );
}

#[test]
fn llvm_scope_drops_moved_and_nested() {
    // 8d-drops: heap bindings are freed at scope exit (reverse declaration order),
    // EXCEPT those moved out (a consuming user-fn call records the move). Here:
    //  - `consume` frees its param `xs` at exit (param-frame drop, after the body).
    //  - `main`'s nested block frees `tmp` at the inner block's exit (before its value
    //    is stored), then drops nothing for the i64 `inner`.
    //  - `arr` is moved into `consume(arr)` (consuming call). Its scope-exit drop tests the
    //    moved flag `%mf0`, which the call's move sets first (ADR 0077 D3), so `main` never
    //    frees it; the callee owns + frees it. No double-free. consume(arr)=1 + tmp[0]=4 = 5.
    assert_eq!(
        llvm_dump(
            "drops",
            "fn consume(xs: [i64]) -> i64 {\n    xs[0]\n}\nfn main() -> i64 {\n    let arr: [i64] = [1, 2, 3];\n    let inner: i64 = {\n        let tmp: [i64] = [4, 5];\n        tmp[0]\n    };\n    consume(arr) + inner\n}\n"
        ),
        concat!(
            "target triple = \"arm64-apple-darwin\"\n",
            "\n",
            "declare ptr @sentinel_alloc(i64)\n",
            "declare void @sentinel_free(ptr)\n",
            "declare void @sentinel_panic_oob(i64, i64)\n",
            "\n",
            "define i64 @consume({ i64, ptr } %arg0) {\n",
            "entry:\n",
            "  %v0 = alloca { i64, ptr }\n",
            "  store { i64, ptr } %arg0, ptr %v0\n",
            "  %v1 = load { i64, ptr }, ptr %v0\n",
            "  %v2 = extractvalue { i64, ptr } %v1, 0\n",
            "  %v3 = extractvalue { i64, ptr } %v1, 1\n",
            "  %v4 = icmp sge i64 0, 0\n",
            "  %v5 = icmp slt i64 0, %v2\n",
            "  %v6 = and i1 %v4, %v5\n",
            "  br i1 %v6, label %bb1, label %bb0\n",
            "bb0:\n",
            "  call void @sentinel_panic_oob(i64 0, i64 %v2)\n",
            "  unreachable\n",
            "bb1:\n",
            "  %v7 = getelementptr i64, ptr %v3, i64 0\n",
            "  %v8 = load i64, ptr %v7\n",
            "  %v9 = load { i64, ptr }, ptr %v0\n",
            "  %v10 = extractvalue { i64, ptr } %v9, 1\n",
            "  call void @sentinel_free(ptr %v10)\n",
            "  ret i64 %v8\n",
            "}\n",
            "\n",
            "define i32 @main() {\n",
            "entry:\n",
            "  %v8 = alloca { i64, ptr }\n",
            "  %v16 = alloca { i64, ptr }\n",
            "  %v27 = alloca i64\n",
            "  %mf0 = alloca i1\n",
            "  store i1 false, ptr %mf0\n",
            "  %v0 = getelementptr i64, ptr null, i64 3\n",
            "  %v1 = ptrtoint ptr %v0 to i64\n",
            "  %v2 = call ptr @sentinel_alloc(i64 %v1)\n",
            "  %v3 = getelementptr i64, ptr %v2, i64 0\n",
            "  store i64 1, ptr %v3\n",
            "  %v4 = getelementptr i64, ptr %v2, i64 1\n",
            "  store i64 2, ptr %v4\n",
            "  %v5 = getelementptr i64, ptr %v2, i64 2\n",
            "  store i64 3, ptr %v5\n",
            "  %v6 = insertvalue { i64, ptr } undef, i64 3, 0\n",
            "  %v7 = insertvalue { i64, ptr } %v6, ptr %v2, 1\n",
            "  store { i64, ptr } %v7, ptr %v8\n",
            "  %v9 = getelementptr i64, ptr null, i64 2\n",
            "  %v10 = ptrtoint ptr %v9 to i64\n",
            "  %v11 = call ptr @sentinel_alloc(i64 %v10)\n",
            "  %v12 = getelementptr i64, ptr %v11, i64 0\n",
            "  store i64 4, ptr %v12\n",
            "  %v13 = getelementptr i64, ptr %v11, i64 1\n",
            "  store i64 5, ptr %v13\n",
            "  %v14 = insertvalue { i64, ptr } undef, i64 2, 0\n",
            "  %v15 = insertvalue { i64, ptr } %v14, ptr %v11, 1\n",
            "  store { i64, ptr } %v15, ptr %v16\n",
            "  %v17 = load { i64, ptr }, ptr %v16\n",
            "  %v18 = extractvalue { i64, ptr } %v17, 0\n",
            "  %v19 = extractvalue { i64, ptr } %v17, 1\n",
            "  %v20 = icmp sge i64 0, 0\n",
            "  %v21 = icmp slt i64 0, %v18\n",
            "  %v22 = and i1 %v20, %v21\n",
            "  br i1 %v22, label %bb1, label %bb0\n",
            "bb0:\n",
            "  call void @sentinel_panic_oob(i64 0, i64 %v18)\n",
            "  unreachable\n",
            "bb1:\n",
            "  %v23 = getelementptr i64, ptr %v19, i64 0\n",
            "  %v24 = load i64, ptr %v23\n",
            "  %v25 = load { i64, ptr }, ptr %v16\n",
            "  %v26 = extractvalue { i64, ptr } %v25, 1\n",
            "  call void @sentinel_free(ptr %v26)\n",
            "  store i64 %v24, ptr %v27\n",
            "  %v28 = load { i64, ptr }, ptr %v8\n",
            "  store i1 true, ptr %mf0\n",
            "  %v29 = call i64 @consume({ i64, ptr } %v28)\n",
            "  %v30 = load i64, ptr %v27\n",
            "  %v31 = add i64 %v29, %v30\n",
            "  %v32 = load i1, ptr %mf0\n",
            "  br i1 %v32, label %bb3, label %bb2\n",
            "bb2:\n",
            "  %v33 = load { i64, ptr }, ptr %v8\n",
            "  %v34 = extractvalue { i64, ptr } %v33, 1\n",
            "  call void @sentinel_free(ptr %v34)\n",
            "  br label %bb3\n",
            "bb3:\n",
            "  store i1 false, ptr %mf0\n",
            "  %v35 = trunc i64 %v31 to i32\n",
            "  ret i32 %v35\n",
            "}\n",
            "\n",
        )
    );
}

#[test]
fn llvm_struct_field_recursive_drop() {
    // 8d-drops-2: a struct owns its heap-backed fields. Dropping `b` GEPs into each
    // drop-needing field (declaration order) and frees it — here field 0 (`data:
    // [i64]`) frees its buffer; field 1 (`tag: i64`) is a scalar and is skipped (no
    // GEP). The GEP index is the field's position, not the drop-needing count.
    // b.data[0]=9 + b.tag=5 = 14.
    assert_eq!(
        llvm_dump(
            "sdrop",
            "struct Box { data: [i64], tag: i64 }\nfn main() -> i64 {\n    let b = Box { data: [9, 8], tag: 5 };\n    b.data[0] + b.tag\n}\n"
        ),
        concat!(
            "target triple = \"arm64-apple-darwin\"\n",
            "\n",
            "%Struct.0 = type { { i64, ptr }, i64 }\n",
            "\n",
            "declare ptr @sentinel_alloc(i64)\n",
            "declare void @sentinel_free(ptr)\n",
            "declare void @sentinel_panic_oob(i64, i64)\n",
            "\n",
            "define i32 @main() {\n",
            "entry:\n",
            "  %v9 = alloca %Struct.0\n",
            "  %v0 = getelementptr i64, ptr null, i64 2\n",
            "  %v1 = ptrtoint ptr %v0 to i64\n",
            "  %v2 = call ptr @sentinel_alloc(i64 %v1)\n",
            "  %v3 = getelementptr i64, ptr %v2, i64 0\n",
            "  store i64 9, ptr %v3\n",
            "  %v4 = getelementptr i64, ptr %v2, i64 1\n",
            "  store i64 8, ptr %v4\n",
            "  %v5 = insertvalue { i64, ptr } undef, i64 2, 0\n",
            "  %v6 = insertvalue { i64, ptr } %v5, ptr %v2, 1\n",
            "  %v7 = insertvalue %Struct.0 undef, { i64, ptr } %v6, 0\n",
            "  %v8 = insertvalue %Struct.0 %v7, i64 5, 1\n",
            "  store %Struct.0 %v8, ptr %v9\n",
            "  %v10 = load %Struct.0, ptr %v9\n",
            "  %v11 = extractvalue %Struct.0 %v10, 0\n",
            "  %v12 = extractvalue { i64, ptr } %v11, 0\n",
            "  %v13 = extractvalue { i64, ptr } %v11, 1\n",
            "  %v14 = icmp sge i64 0, 0\n",
            "  %v15 = icmp slt i64 0, %v12\n",
            "  %v16 = and i1 %v14, %v15\n",
            "  br i1 %v16, label %bb1, label %bb0\n",
            "bb0:\n",
            "  call void @sentinel_panic_oob(i64 0, i64 %v12)\n",
            "  unreachable\n",
            "bb1:\n",
            "  %v17 = getelementptr i64, ptr %v13, i64 0\n",
            "  %v18 = load i64, ptr %v17\n",
            "  %v19 = load %Struct.0, ptr %v9\n",
            "  %v20 = extractvalue %Struct.0 %v19, 1\n",
            "  %v21 = add i64 %v18, %v20\n",
            "  %v22 = getelementptr %Struct.0, ptr %v9, i32 0, i32 0\n",
            "  %v23 = load { i64, ptr }, ptr %v22\n",
            "  %v24 = extractvalue { i64, ptr } %v23, 1\n",
            "  call void @sentinel_free(ptr %v24)\n",
            "  %v25 = trunc i64 %v21 to i32\n",
            "  ret i32 %v25\n",
            "}\n",
            "\n",
        )
    );
}

#[test]
fn llvm_loop_exit_drops_on_break() {
    // 8d-drops-3: a heap binding allocated in a loop body is freed PER ITERATION. On a
    // `break` (bb3) the body frame is drained before branching to loop_after (bb2); on
    // the fall-through (bb5) the same `s` is freed before the back-edge to loop_cond
    // (bb0). Mutually exclusive blocks → each runtime path frees once. n = len("ab")=2
    // at i=0, +2 at i=1 then break = 4.
    assert_eq!(
        llvm_dump(
            "loopdrop",
            "fn main() -> i64 {\n    let mut i: i64 = 0;\n    let mut n: i64 = 0;\n    while i < 3 {\n        let s: [u8] = \"ab\";\n        n = n + len(s);\n        if i == 1 { break; 0 } else { 0 };\n        i = i + 1;\n    }\n    n\n}\n"
        ),
        concat!(
            "target triple = \"arm64-apple-darwin\"\n",
            "\n",
            "declare ptr @sentinel_alloc(i64)\n",
            "declare void @sentinel_free(ptr)\n",
            "\n",
            "define i32 @main() {\n",
            "entry:\n",
            "  %v0 = alloca i64\n",
            "  %v1 = alloca i64\n",
            "  %v11 = alloca { i64, ptr }\n",
            "  %v20 = alloca i64\n",
            "  store i64 0, ptr %v0\n",
            "  store i64 0, ptr %v1\n",
            "  br label %bb0\n",
            "bb0:\n",
            "  %v2 = load i64, ptr %v0\n",
            "  %v3 = icmp slt i64 %v2, 3\n",
            "  br i1 %v3, label %bb1, label %bb2\n",
            "bb1:\n",
            "  %v4 = getelementptr i8, ptr null, i64 2\n",
            "  %v5 = ptrtoint ptr %v4 to i64\n",
            "  %v6 = call ptr @sentinel_alloc(i64 %v5)\n",
            "  %v7 = getelementptr i8, ptr %v6, i64 0\n",
            "  store i8 97, ptr %v7\n",
            "  %v8 = getelementptr i8, ptr %v6, i64 1\n",
            "  store i8 98, ptr %v8\n",
            "  %v9 = insertvalue { i64, ptr } undef, i64 2, 0\n",
            "  %v10 = insertvalue { i64, ptr } %v9, ptr %v6, 1\n",
            "  store { i64, ptr } %v10, ptr %v11\n",
            "  %v12 = load i64, ptr %v1\n",
            "  %v13 = load { i64, ptr }, ptr %v11\n",
            "  %v14 = extractvalue { i64, ptr } %v13, 0\n",
            "  %v15 = add i64 %v12, %v14\n",
            "  store i64 %v15, ptr %v1\n",
            "  %v16 = load i64, ptr %v0\n",
            "  %v17 = icmp eq i64 %v16, 1\n",
            "  br i1 %v17, label %bb3, label %bb4\n",
            "bb3:\n",
            "  %v18 = load { i64, ptr }, ptr %v11\n",
            "  %v19 = extractvalue { i64, ptr } %v18, 1\n",
            "  call void @sentinel_free(ptr %v19)\n",
            "  br label %bb2\n",
            "bb6:\n",
            "  store i64 0, ptr %v20\n",
            "  br label %bb5\n",
            "bb4:\n",
            "  store i64 0, ptr %v20\n",
            "  br label %bb5\n",
            "bb5:\n",
            "  %v21 = load i64, ptr %v20\n",
            "  %v22 = load i64, ptr %v0\n",
            "  %v23 = add i64 %v22, 1\n",
            "  store i64 %v23, ptr %v0\n",
            "  %v24 = load { i64, ptr }, ptr %v11\n",
            "  %v25 = extractvalue { i64, ptr } %v24, 1\n",
            "  call void @sentinel_free(ptr %v25)\n",
            "  br label %bb0\n",
            "bb2:\n",
            "  %v26 = load i64, ptr %v1\n",
            "  %v27 = trunc i64 %v26 to i32\n",
            "  ret i32 %v27\n",
            "}\n",
            "\n",
        )
    );
}

#[test]
fn llvm_enum_construct_and_drop() {
    // 8e-1: an enum is `{ i32 tag, ptr payload }` (ADR 0032). A payload variant
    // (`B(7)`) builds a payload struct `{ i64 }` and heap-boxes it; a unit variant
    // (`A`) gets a null payload. At scope exit each enum is dropped: load `{i32,ptr}`,
    // null-check the payload, `sentinel_free` it (box-free-only). `match` (reading the
    // value back) is 8e-2 — here the program just constructs + drops, returning 0.
    assert_eq!(
        llvm_dump(
            "enumc",
            "enum E { A, B(i64) }\nfn main() -> i64 {\n    let x = E::B(7);\n    let y = E::A;\n    0\n}\n"
        ),
        concat!(
            "target triple = \"arm64-apple-darwin\"\n",
            "\n",
            "declare ptr @sentinel_alloc(i64)\n",
            "declare void @sentinel_free(ptr)\n",
            "\n",
            "define i32 @main() {\n",
            "entry:\n",
            "  %v6 = alloca { i32, ptr }\n",
            "  %v9 = alloca { i32, ptr }\n",
            "  %v0 = insertvalue { i64 } undef, i64 7, 0\n",
            "  %v1 = getelementptr { i64 }, ptr null, i64 1\n",
            "  %v2 = ptrtoint ptr %v1 to i64\n",
            "  %v3 = call ptr @sentinel_alloc(i64 %v2)\n",
            "  store { i64 } %v0, ptr %v3\n",
            "  %v4 = insertvalue { i32, ptr } undef, i32 1, 0\n",
            "  %v5 = insertvalue { i32, ptr } %v4, ptr %v3, 1\n",
            "  store { i32, ptr } %v5, ptr %v6\n",
            "  %v7 = insertvalue { i32, ptr } undef, i32 0, 0\n",
            "  %v8 = insertvalue { i32, ptr } %v7, ptr null, 1\n",
            "  store { i32, ptr } %v8, ptr %v9\n",
            "  %v10 = load { i32, ptr }, ptr %v9\n",
            "  %v11 = extractvalue { i32, ptr } %v10, 1\n",
            "  %v12 = icmp eq ptr %v11, null\n",
            "  br i1 %v12, label %bb1, label %bb0\n",
            "bb0:\n",
            "  call void @sentinel_free(ptr %v11)\n",
            "  br label %bb1\n",
            "bb1:\n",
            "  %v13 = load { i32, ptr }, ptr %v6\n",
            "  %v14 = extractvalue { i32, ptr } %v13, 1\n",
            "  %v15 = icmp eq ptr %v14, null\n",
            "  br i1 %v15, label %bb3, label %bb2\n",
            "bb2:\n",
            "  call void @sentinel_free(ptr %v14)\n",
            "  br label %bb3\n",
            "bb3:\n",
            "  %v16 = trunc i64 0 to i32\n",
            "  ret i32 %v16\n",
            "}\n",
            "\n",
        )
    );
}

#[test]
fn llvm_match_if_else_chain() {
    // 8e-2: `match` lowers to an if-else chain over the variant arms — per arm `icmp eq
    // tag, vidx` → branch to the arm block (bind payloads, lower body, store to the
    // result slot, branch to merge) or the next check; `unreachable` is the exhaustive
    // default; the merge loads the result (no phi). `f`'s param enum `e` is dropped at
    // exit (box freed). main moves `E::B(7)` into `f` (consuming), so only `f` frees it.
    // f(B(7)) = 7.
    assert_eq!(
        llvm_dump(
            "match",
            "enum E { A, B(i64) }\nfn f(e: E) -> i64 {\n    match e {\n        E::A => 0,\n        E::B(x) => x,\n    }\n}\nfn main() -> i64 {\n    f(E::B(7))\n}\n"
        ),
        concat!(
            "target triple = \"arm64-apple-darwin\"\n",
            "\n",
            "declare ptr @sentinel_alloc(i64)\n",
            "declare void @sentinel_free(ptr)\n",
            "\n",
            "define i64 @f({ i32, ptr } %arg0) {\n",
            "entry:\n",
            "  %v0 = alloca { i32, ptr }\n",
            "  %v4 = alloca i64\n",
            "  %v9 = alloca i64\n",
            "  store { i32, ptr } %arg0, ptr %v0\n",
            "  %v1 = load { i32, ptr }, ptr %v0\n",
            "  %v2 = extractvalue { i32, ptr } %v1, 0\n",
            "  %v3 = extractvalue { i32, ptr } %v1, 1\n",
            "  %v5 = icmp eq i32 %v2, 0\n",
            "  br i1 %v5, label %bb1, label %bb2\n",
            "bb1:\n",
            "  store i64 0, ptr %v4\n",
            "  br label %bb0\n",
            "bb2:\n",
            "  %v6 = icmp eq i32 %v2, 1\n",
            "  br i1 %v6, label %bb3, label %bb4\n",
            "bb3:\n",
            "  %v7 = getelementptr { i64 }, ptr %v3, i32 0, i32 0\n",
            "  %v8 = load i64, ptr %v7\n",
            "  store i64 %v8, ptr %v9\n",
            "  %v10 = load i64, ptr %v9\n",
            "  store i64 %v10, ptr %v4\n",
            "  br label %bb0\n",
            "bb4:\n",
            "  unreachable\n",
            "bb0:\n",
            "  %v11 = load i64, ptr %v4\n",
            "  %v12 = load { i32, ptr }, ptr %v0\n",
            "  %v13 = extractvalue { i32, ptr } %v12, 1\n",
            "  %v14 = icmp eq ptr %v13, null\n",
            "  br i1 %v14, label %bb6, label %bb5\n",
            "bb5:\n",
            "  call void @sentinel_free(ptr %v13)\n",
            "  br label %bb6\n",
            "bb6:\n",
            "  ret i64 %v11\n",
            "}\n",
            "\n",
            "define i32 @main() {\n",
            "entry:\n",
            "  %v0 = insertvalue { i64 } undef, i64 7, 0\n",
            "  %v1 = getelementptr { i64 }, ptr null, i64 1\n",
            "  %v2 = ptrtoint ptr %v1 to i64\n",
            "  %v3 = call ptr @sentinel_alloc(i64 %v2)\n",
            "  store { i64 } %v0, ptr %v3\n",
            "  %v4 = insertvalue { i32, ptr } undef, i32 1, 0\n",
            "  %v5 = insertvalue { i32, ptr } %v4, ptr %v3, 1\n",
            "  %v6 = call i64 @f({ i32, ptr } %v5)\n",
            "  %v7 = trunc i64 %v6 to i32\n",
            "  ret i32 %v7\n",
            "}\n",
            "\n",
        )
    );
}

#[test]
fn llvm_match_on_a_temporary_frees_its_box() {
    // ADR 0032 A5 (register D92): a scrutinee that is not a place — here a variant built in
    // place — is owned by no binding, so each arm frees its payload box once the bindings
    // have copied the payload out: the `B` arm after binding `x`, the `_` arm before its
    // body (a unit variant reaching it frees null, a no-op). Nothing frees it at the exit.
    assert_eq!(
        llvm_dump(
            "match_temp",
            "enum E { A, B(i64) }\nfn f(n: i64) -> i64 {\n    match E::B(n) {\n        E::B(x) => x,\n        _ => 0,\n    }\n}\nfn main() -> i64 {\n    f(7)\n}\n"
        ),
        concat!(
            "target triple = \"arm64-apple-darwin\"\n",
            "\n",
            "declare ptr @sentinel_alloc(i64)\n",
            "declare void @sentinel_free(ptr)\n",
            "\n",
            "define i64 @f(i64 %arg0) {\n",
            "entry:\n",
            "  %v0 = alloca i64\n",
            "  %v10 = alloca i64\n",
            "  %v14 = alloca i64\n",
            "  store i64 %arg0, ptr %v0\n",
            "  %v1 = load i64, ptr %v0\n",
            "  %v2 = insertvalue { i64 } undef, i64 %v1, 0\n",
            "  %v3 = getelementptr { i64 }, ptr null, i64 1\n",
            "  %v4 = ptrtoint ptr %v3 to i64\n",
            "  %v5 = call ptr @sentinel_alloc(i64 %v4)\n",
            "  store { i64 } %v2, ptr %v5\n",
            "  %v6 = insertvalue { i32, ptr } undef, i32 1, 0\n",
            "  %v7 = insertvalue { i32, ptr } %v6, ptr %v5, 1\n",
            "  %v8 = extractvalue { i32, ptr } %v7, 0\n",
            "  %v9 = extractvalue { i32, ptr } %v7, 1\n",
            "  %v11 = icmp eq i32 %v8, 1\n",
            "  br i1 %v11, label %bb1, label %bb2\n",
            "bb1:\n",
            "  %v12 = getelementptr { i64 }, ptr %v9, i32 0, i32 0\n",
            "  %v13 = load i64, ptr %v12\n",
            "  store i64 %v13, ptr %v14\n",
            "  call void @sentinel_free(ptr %v9)\n",
            "  %v15 = load i64, ptr %v14\n",
            "  store i64 %v15, ptr %v10\n",
            "  br label %bb0\n",
            "bb2:\n",
            "  call void @sentinel_free(ptr %v9)\n",
            "  store i64 0, ptr %v10\n",
            "  br label %bb0\n",
            "bb0:\n",
            "  %v16 = load i64, ptr %v10\n",
            "  ret i64 %v16\n",
            "}\n",
            "\n",
            "define i32 @main() {\n",
            "entry:\n",
            "  %v0 = call i64 @f(i64 7)\n",
            "  %v1 = trunc i64 %v0 to i32\n",
            "  ret i32 %v1\n",
            "}\n",
            "\n",
        )
    );
}

/// The body of `define … @name(…) { … }` in a `snc llvm` dump.
fn dump_fn_body<'a>(ir: &'a str, name: &str) -> &'a str {
    let head = ir
        .find(&format!(" @{name}("))
        .unwrap_or_else(|| panic!("no fn @{name} in:\n{ir}"));
    let start = head + ir[head..].find('{').expect("fn body opens");
    let end = start + ir[start..].find("\n}").expect("fn body closes");
    &ir[start..end]
}

#[test]
fn llvm_a_moved_generic_read_hands_its_unit_on() {
    // ADR 0071 D2 amendment A1: a generic body instantiated at `Shared` treats its type
    // parameter as Move, so the drop plan records the parameter (or a field of one) as moved
    // where it flows into the body value, a struct literal or a call, and skips its drop. That
    // read transfers the unit; a clone there would never be released. A `Shared` field read
    // of a binding moved only ELSEWHERE (`take(h.s)` before `eat(h)`) still clones. The
    // corpus cannot carry the generic shapes (register D36), so this reads the IR.
    let ir = llvm_dump(
        "a1_moved_reads",
        concat!(
            "struct Bx<U> { v: U, n: i64 }\n",
            "struct H { s: Shared<i64>, n: i64 }\n",
            "fn sink<U>(x: U) -> i64 { 1 }\n",
            "fn ident<U>(x: U) -> U { x }\n",
            "fn boxit<U>(x: U) -> Bx<U> { Bx { v: x, n: 1 } }\n",
            "fn unbox<U>(b: Bx<U>) -> U { b.v }\n",
            "fn relay<U>(x: U) -> i64 { sink({ x }) }\n",
            "fn eat(h: H) -> i64 { h.n }\n",
            "fn take(x: Shared<i64>) -> i64 { shared_get(x) }\n",
            "fn read_then_move(h: H) -> i64 { take(h.s) + eat(h) }\n",
            "fn main() -> i64 {\n",
            "    let s: Shared<i64> = shared_new(5);\n",
            "    let a: Shared<i64> = ident(s);\n",
            "    let b: Bx<Shared<i64>> = boxit(s);\n",
            "    let c: Shared<i64> = unbox(b);\n",
            "    relay(s) + shared_get(a) + shared_get(c) + read_then_move(H { s: s, n: 1 })\n",
            "}\n",
        ),
    );
    for f in ["ident__shared_i64", "boxit__shared_i64", "unbox__shared_i64", "relay__shared_i64"] {
        let body = dump_fn_body(&ir, f);
        assert_eq!(
            body.matches("@sentinel_shared_clone(").count(),
            0,
            "@{f} clones a parameter its drop plan moves, so nothing releases the clone:\n{body}"
        );
    }
    let body = dump_fn_body(&ir, "read_then_move");
    assert_eq!(
        body.matches("@sentinel_shared_clone(").count(),
        1,
        "@read_then_move: the field read before the move must clone:\n{body}"
    );
}

#[test]
fn llvm_a_class_field_store_counts_and_a_class_drop_releases() {
    // ADR 0071 D2 amendment A4 (register D156): a `Shared` / `Mutex` stored into a field of
    // a class instance is cloned like any assignment of a place read, and a class binding's
    // drop releases every handle its fields hold, at any depth through struct,
    // generic-instance and class fields, a field the binding was partially moved out of
    // behind its moved flag (ADR 0077 D5), and frees nothing else (register D137). The
    // oracle, unlike inkwell (A2),
    // releases a method's and an init's handle parameters, a class parameter's fields
    // included. A missing release is a leak, not an exit code, so this reads the IR; the
    // codegen differential holds `scg` to the same IR on `c71_class_field_handles`.
    let ir = llvm_dump(
        "a4_class_fields",
        concat!(
            "struct H { s: Shared<i64>, a: [i64] }\n",
            "struct B<T> { v: T, n: i64 }\n",
            "class K {\n",
            "    let s: Shared<i64>;\n",
            "    let m: Mutex<i64>;\n",
            "    pub init(s: Shared<i64>, m: Mutex<i64>) { self.s = s; self.m = m; 0 }\n",
            "    pub fn get(self: &Self) -> i64 { shared_get(self.s) }\n",
            "    pub fn set(self: &mut Self, t: Shared<i64>) -> i64 { self.s = t; 0 }\n",
            "}\n",
            "class N {\n",
            "    let h: H;\n",
            "    let k: K;\n",
            "    let b: B<Shared<i64>>;\n",
            "    let a: [i64];\n",
            "    pub init(h: H, k: K) { self.h = h; self.k = k; self.b = B { v: shared_new(4), n: 1 }; self.a = [1, 2]; 0 }\n",
            "}\n",
            "class A { let a: [i64]; pub init() { self.a = [1]; 0 } }\n",
            "class U {\n",
            "    let n: i64;\n",
            "    pub init() { self.n = 0; 0 }\n",
            "    pub fn eat(self: &Self, k: K) -> i64 { k.get() }\n",
            "}\n",
            "class SK { let k: secret K; pub init(k: secret K) { self.k = k; 0 } }\n",
            "struct W { k: K, n: i64 }\n",
            "fn eat_h(h: H) -> i64 { h.a[0] }\n",
            "fn store(k: &mut K, s: Shared<i64>) -> i64 { (*k).s = s; 0 }\n",
            "fn direct() -> i64 { let k: K = K::init(shared_new(1), mutex_new(2)); k.get() }\n",
            "fn nested() -> i64 {\n",
            "    let n: N = N::init(H { s: shared_new(1), a: [3] }, K::init(shared_new(2), mutex_new(3)));\n",
            "    1\n",
            "}\n",
            "fn partial() -> i64 {\n",
            "    let n: N = N::init(H { s: shared_new(1), a: [3] }, K::init(shared_new(2), mutex_new(3)));\n",
            "    eat_h(n.h)\n",
            "}\n",
            "fn arrays_only() -> i64 { let a: A = A::init(); 1 }\n",
            "fn held() -> i64 { let w: W = W { k: K::init(shared_new(1), mutex_new(2)), n: 1 }; w.n }\n",
            "fn secret_field() -> i64 { let o: SK = SK::init(K::init(shared_new(1), mutex_new(2))); 1 }\n",
            "fn main() -> i64 {\n",
            "    let mut k: K = K::init(shared_new(5), mutex_new(6));\n",
            "    let u: U = U::init();\n",
            "    store(&mut k, shared_new(7)) + k.set(shared_new(8)) + u.eat(K::init(shared_new(9), mutex_new(1)))\n",
            "        + direct() + nested() + partial() + arrays_only() + held() + secret_field()\n",
            "}\n",
        ),
    );
    // (fn, shared clones, mutex clones, shared releases, mutex releases, frees)
    for (f, sc, mc, sr, mr, fr) in [
        // each store into a class field clones; the frame releases the handle parameters
        ("K__init", 1, 1, 1, 1, 0),
        ("K__set", 1, 0, 1, 0, 0),
        // the class parameter's drop releases both of its fields
        ("U__eat", 0, 0, 1, 1, 0),
        // a struct and a class moved into fields, and a value no place holds: the moved
        // parameters' drop is emitted behind the flags their moves set (ADR 0077), so none
        // of it runs (checked below)
        ("N__init", 0, 0, 2, 1, 1),
        ("store", 1, 0, 1, 0, 0),
        // `k`'s drop releases both of its fields
        ("direct", 0, 0, 1, 1, 0),
        // `n`'s drop: `h.s`, `k.s`, `k.m`, `b.v`, and none of the arrays it holds
        ("nested", 0, 0, 3, 1, 0),
        // `n.h` was moved out: its release sits behind its field's flag (ADR 0077 D5)
        ("partial", 0, 0, 3, 1, 0),
        ("arrays_only", 0, 0, 0, 0, 0),
        // a struct's drop reaches the class in its field
        ("held", 0, 0, 1, 1, 0),
        // a `secret`-qualified class field holds what the class holds
        ("secret_field", 0, 0, 1, 1, 0),
    ] {
        // The `define`, not a call site: the oracle emits class methods after the free fns
        // that call them.
        let head = format!("@{f}(");
        let body: String = ir
            .lines()
            .skip_while(|l| !(l.starts_with("define ") && l.contains(&head)))
            .take_while(|l| *l != "}")
            .collect::<Vec<_>>()
            .join("\n");
        assert!(!body.is_empty(), "no define for @{f} in:\n{ir}");
        for (sym, want) in [
            ("@sentinel_shared_clone(", sc),
            ("@sentinel_mutex_clone(", mc),
            ("@sentinel_shared_release(", sr),
            ("@sentinel_mutex_release(", mr),
            ("@sentinel_free(", fr),
        ] {
            assert_eq!(body.matches(sym).count(), want, "@{f}: {sym} count:\n{body}");
        }
        // ADR 0077: the drops of what a move took sit behind the flag the move set -- all of
        // `N__init`'s, and `partial`'s release of `n.h.s` -- and no other drop does.
        let lines: Vec<&str> = body.lines().collect();
        let guarded = lines
            .iter()
            .enumerate()
            .filter(|(_, l)| l.contains("_release(") || l.contains("@sentinel_free("))
            .filter(|(i, _)| drop_is_behind_a_moved_flag(&lines, *i))
            .count();
        let want = match f {
            "N__init" => 4,
            "partial" => 1,
            _ => 0,
        };
        assert_eq!(guarded, want, "@{f}: drops behind a set moved flag:\n{body}");
    }
}

#[test]
fn llvm_refuses_a_non_word_param_the_embedded_shape_would_capture() {
    // ADR 0072 D4 and register D69: the embedded shape's parent copies each param its resumer
    // reads into the frame with an 8-byte load, and the resumer rebuilds the param from that
    // one word. A struct or class rebuilt that way releases words that were never handles when
    // the replay hands it to a by-value callee, whose drop releases its handle fields (ADR 0071
    // A4). The oracle now refuses such a param, as its let and chained shapes already did and
    // inkwell does (the let and chained cases pin those older refusals for a class, the last
    // one read only by a later `let`'s `perform` argument); a word-typed param in the same
    // shape still lowers.
    let header = concat!(
        "class K { let n: i64; let s: Shared<i64>; pub init(s: Shared<i64>) { self.n = 5; self.s = s; 0 } }\n",
        "struct H { n: i64, s: Shared<i64> }\n",
        "effect Io { read() -> i64; echo(x: i64) -> i64; }\n",
        "fn eat(k: K) -> i64 { 1 }\n",
        "fn eat_h(h: H) -> i64 { 1 }\n",
    );
    for (name, body, refused) in [
        (
            "class_param",
            "fn eff(k: K) -> i64 ! { Io } { eat(k) + perform Io.read() }\nfn main() -> i64 { let s: Shared<i64> = shared_new(42); handle eff(K::init(s)) with { Io.read(kk) => kk(41) } }\n",
            Some("`k` is captured across the continuation, so it must be `i64` or `secret i64`, and it is `K`"),
        ),
        (
            "struct_param",
            "fn eff(h: H) -> i64 ! { Io } { eat_h(h) + perform Io.read() }\nfn main() -> i64 { let s: Shared<i64> = shared_new(42); handle eff(H { n: 1, s: s }) with { Io.read(kk) => kk(41) } }\n",
            Some("`h` is captured across the continuation, so it must be `i64` or `secret i64`, and it is `H`"),
        ),
        (
            "let_class_param",
            "fn eff(k: K) -> i64 ! { Io } { let v: i64 = perform Io.read(); v + eat(k) }\nfn main() -> i64 { let s: Shared<i64> = shared_new(42); handle eff(K::init(s)) with { Io.read(kk) => kk(41) } }\n",
            Some("`k` is captured across the continuation, so it must be `i64` or `secret i64`; `K` would be read out of bounds"),
        ),
        (
            "chained_class_param",
            "fn eff(k: K) -> i64 ! { Io } { let a: i64 = perform Io.read(); let b: i64 = perform Io.read(); a + b + eat(k) }\nfn main() -> i64 { let s: Shared<i64> = shared_new(42); handle eff(K::init(s)) with { Io.read(kk) => kk(41) } }\n",
            Some("effecting fn `eff` cannot be lowered"),
        ),
        (
            "chained_later_let_class_param",
            "fn eff(k: K) -> i64 ! { Io } { let a: i64 = perform Io.read(); let b: i64 = perform Io.echo(eat(k)); a + b }\nfn main() -> i64 { let s: Shared<i64> = shared_new(42); handle eff(K::init(s)) with { Io.read(kk) => kk(41), Io.echo(x, kk) => kk(x) } }\n",
            Some("effecting fn `eff` cannot be lowered"),
        ),
        (
            "word_param",
            "fn eff(n: i64) -> i64 ! { Io } { n + perform Io.read() }\nfn main() -> i64 { handle eff(1) with { Io.read(kk) => kk(41) } }\n",
            None,
        ),
    ] {
        let path = temp_dir(&format!("a4_embedded_capture_{name}")).join("input.sentinel");
        std::fs::write(&path, format!("{header}{body}")).expect("write source");
        let out = Command::new(env!("CARGO_BIN_EXE_snc"))
            .arg("llvm")
            .arg(&path)
            .output()
            .expect("run snc llvm");
        let stderr = String::from_utf8_lossy(&out.stderr);
        match refused {
            Some(reason) => assert!(
                !out.status.success() && stderr.contains(reason),
                "{name}: snc llvm must refuse, because {reason}; got {:?}:\n{stderr}",
                out.status
            ),
            None => assert!(out.status.success(), "{name}: snc llvm must lower it:\n{stderr}"),
        }
    }
}

#[test]
fn llvm_survives_a_by_value_cycle_through_a_class_field() {
    // Register D157: the type checker refuses a struct that holds itself by value, but
    // follows only struct-to-struct edges, so a struct and a class that hold each other are
    // accepted though they have no finite layout. A class's drop now walks its fields for the
    // handles they hold (ADR 0071 A4), and this walk stops at a type already on its path;
    // without that stop it recursed until `snc` overflowed its stack. The IR emitted for such
    // a type does not assemble (`llc`: "Cannot allocate unsized type"), so this checks only
    // that `snc llvm` ends with a status of its own: 0 now, 1 once D157's refusal exists.
    let path = temp_dir("a4_class_cycle").join("input.sentinel");
    std::fs::write(
        &path,
        concat!(
            "struct S { k: K, n: i64 }\n",
            "class K { let s: S; let h: Shared<i64>; pub init(s: S) { self.s = s; self.h = shared_new(1); 0 } }\n",
            "fn f(s: S) -> i64 { s.n }\n",
            "fn g(k: K) -> i64 { 1 }\n",
            "fn main() -> i64 { 7 }\n",
        ),
    )
    .expect("write source");
    let out = Command::new(env!("CARGO_BIN_EXE_snc"))
        .arg("llvm")
        .arg(&path)
        .output()
        .expect("run snc llvm");
    assert!(
        matches!(out.status.code(), Some(0) | Some(1)),
        "snc llvm did not end with a status of its own on a class/struct cycle: {:?}\n{}",
        out.status,
        String::from_utf8_lossy(&out.stderr)
    );
}

// ---- Layer 2: the 0-panics corpus sweep ---------------------------------

fn corpus_fixtures() -> Vec<PathBuf> {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .to_path_buf();
    let mut fixtures = Vec::new();
    for sub in ["tests/pass", "tests/ui"] {
        let dir = root.join(sub);
        if let Ok(entries) = std::fs::read_dir(&dir) {
            for e in entries.flatten() {
                let p = e.path();
                if p.extension().is_some_and(|x| x == "sentinel") {
                    fixtures.push(p);
                }
            }
        }
    }
    fixtures.sort();
    fixtures
}

#[test]
fn llvm_never_panics_over_corpus() {
    // Over THIS CORPUS `snc llvm` is partial-by-Err: it either emits (0) or cleanly
    // Errs (1) — never a panic (101) or a signal. Emission grows per sub-slice; the
    // floor guards against a regression that stops emitting the straight-line
    // subset entirely.
    //
    // ⚠ That is a property of the corpus, not of `snc llvm`. It used to be written as
    // the latter, and it was false: until register D60, a `return` inside a CLASS METHOD
    // panicked the oracle outright (`dump_method` never bound a `FnId`, so the Return arm
    // indexed the signature table at `u32::MAX`). No fixture had that shape, which is
    // exactly why the claim survived. Layer 2b leans on the corpus-scoped reading —
    // it treats a non-zero exit as "did not emit" — so keep this test sweeping the same
    // `corpus_fixtures()` layer 2b does.
    let mut emitted = 0;
    let mut total = 0;
    for f in corpus_fixtures() {
        total += 1;
        let out = Command::new(env!("CARGO_BIN_EXE_snc"))
            .arg("llvm")
            .arg(&f)
            .output()
            .expect("run snc llvm");
        let code = out.status.code();
        assert!(
            code == Some(0) || code == Some(1),
            "snc llvm crashed on {} (exit {:?})\nstderr:\n{}",
            f.display(),
            code,
            String::from_utf8_lossy(&out.stderr)
        );
        if code == Some(0) {
            emitted += 1;
        }
    }
    assert!(total > 100, "corpus should be large, got {total}");
    assert!(
        emitted >= 15,
        "expected the straight-line subset (~16) to emit, got {emitted}"
    );
}

// ---- Layer 2b: the emitted IR assembles (register D10) ------------------

/// `llvm-as` from the same LLVM 18 the workspace already hard-requires to build.
/// Looked for under `LLVM_SYS_180_PREFIX/bin` (the variable `llvm-sys` needs), then
/// `llvm-config --bindir`, then `PATH`.
///
/// Fails closed twice over. A missing assembler is an environment defect, not a
/// reason to skip — a gate that quietly checks nothing is the failure mode this
/// layer exists to close, and `llvm_rejects` in `selfhost_codegen.rs` is an in-tree
/// instance of it (it returns `None`, checking nothing, when `LLVM_SYS_180_PREFIX`
/// is unset). And only the first of the three locations below is tied to LLVM 18 by
/// construction, so every candidate is version-gated as well: an `llvm-as` from
/// another toolchain accepts and rejects different IR, and one silently answering
/// for a different LLVM is no better than none.
fn llvm_as() -> PathBuf {
    let exe = if cfg!(windows) { "llvm-as.exe" } else { "llvm-as" };
    // `llvm-as --version` prints "  LLVM version 18.1.8" on stdout's second line.
    let is_llvm_18 = |p: &Path| -> bool {
        Command::new(p).arg("--version").output().is_ok_and(|o| {
            o.status.success() && String::from_utf8_lossy(&o.stdout).contains("LLVM version 18.")
        })
    };
    let mut looked: Vec<String> = Vec::new();

    if let Ok(prefix) = std::env::var("LLVM_SYS_180_PREFIX") {
        let p = PathBuf::from(&prefix).join("bin").join(exe);
        if p.is_file() && is_llvm_18(&p) {
            return p;
        }
        looked.push(format!("{} (absent or not LLVM 18)", p.display()));
    }
    if let Ok(out) = Command::new("llvm-config").arg("--bindir").output() {
        if out.status.success() {
            let dir = String::from_utf8_lossy(&out.stdout).trim().to_string();
            let p = PathBuf::from(&dir).join(exe);
            if p.is_file() && is_llvm_18(&p) {
                return p;
            }
            looked.push(format!("{} (absent or not LLVM 18)", p.display()));
        }
    }
    let bare = PathBuf::from(exe);
    if is_llvm_18(&bare) {
        return bare;
    }
    looked.push(format!("{exe} on PATH (absent or not LLVM 18)"));
    panic!(
        "no LLVM 18 llvm-as found — looked at: {}.\n\
         It ships with the LLVM 18 this workspace already requires to build; set \
         LLVM_SYS_180_PREFIX or put llvm-config/llvm-as on PATH.",
        looked.join(", ")
    );
}

/// The `tests/ui` fixtures that `snc llvm` emits IR for, where that IR does NOT assemble.
///
/// An explicit fail-closed list, NOT a filter. `run_llvm` (`main.rs`) never runs
/// effect-check at all, and runs borrow-check only to get its drop plan — discarding
/// its errors — so a program that either of those two rejects still reaches the
/// dump. `c37` performs an effect outside
/// any `handle`; the unhandled `perform` lowers to a `Kont*` where the body wants an
/// `i64`. Nothing consumes that IR — the fixture's whole job is to be REJECTED, and
/// `snc build` does reject it — so it is a byproduct of dumping past a rejection,
/// not an oracle defect. Pinned rather than skipped so that a SECOND such program
/// has to be looked at by a human instead of joining a silent exemption.
const UI_EMITS_UNASSEMBLABLE: &[&str] = &["c37_perform_outside_handle.sentinel"];

#[test]
fn llvm_emitted_ir_assembles_over_corpus() {
    // `llvm-as` runs the VERIFIER unless `-disable-verify`, so this is parse +
    // verify, not parse alone (checked: a dominance violation is rejected here).
    let asm = llvm_as();
    let dir = temp_dir("assemble");
    let ll = dir.join("m.ll");
    let bc = dir.join("m.bc");
    let mut checked = 0;
    let mut ui_unassemblable: Vec<String> = Vec::new();

    for f in corpus_fixtures() {
        // Match on the parent DIRECTORY rather than on a substring of the path. The
        // substring form layer 3 uses is NOT broken on Windows, as one might assume:
        // `corpus_fixtures()` builds the directory with `root.join("tests/pass")` and
        // `Path::join` appends that literal, so the forward slash survives (measured —
        // a "tests/pass" substring filter matches all 182). This form simply does not
        // depend on that.
        let corpus = f
            .parent()
            .and_then(|p| p.file_name())
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_default();
        let dump = Command::new(env!("CARGO_BIN_EXE_snc"))
            .arg("llvm")
            .arg(&f)
            .output()
            .expect("run snc llvm");
        if !dump.status.success() {
            continue; // did not emit — layer 2 already pinned that it Err'd cleanly
        }
        // llvm-as exits 0 on an empty file, and on one holding only the `target
        // triple` preamble. Without this, a regression that stopped emitting bodies
        // but kept exit 0 would score every fixture as "checked" and clear the floor.
        assert!(
            dump.stdout.windows(7).any(|w| w == b"define "),
            "snc llvm exited 0 for {} but emitted no function definition",
            f.display()
        );
        std::fs::write(&ll, &dump.stdout).expect("write .ll");
        let out = Command::new(&asm)
            .arg(&ll)
            .arg("-o")
            .arg(&bc)
            .output()
            .expect("run llvm-as");
        let name = f.file_name().unwrap_or_default().to_string_lossy().to_string();

        if corpus == "ui" {
            if !out.status.success() {
                ui_unassemblable.push(name);
            }
            continue;
        }

        assert!(
            out.status.success(),
            "snc llvm emitted IR for {} that does not assemble:\n{}\n\
             (the .ll is at {})",
            f.display(),
            String::from_utf8_lossy(&out.stderr),
            ll.display()
        );
        checked += 1;
    }

    // A floor, so a change that stops the oracle emitting cannot turn this green by
    // checking nothing. 179 of the 182 `tests/pass` fixtures emitted on 2026-09-08
    // (counted AFTER c65_return_aggregate_shapes was added by this same change — the
    // first count taken was 178/181 and was stale by the time it was written down).
    assert!(
        checked >= 170,
        "expected the emitting tests/pass subset (~179) to be checked, got {checked}"
    );

    ui_unassemblable.sort();
    let mut expected: Vec<String> =
        UI_EMITS_UNASSEMBLABLE.iter().map(|s| (*s).to_string()).collect();
    expected.sort();
    assert_eq!(
        ui_unassemblable, expected,
        "the set of tests/ui fixtures whose emitted IR does not assemble changed; \
         see UI_EMITS_UNASSEMBLABLE — a new one is not automatically benign"
    );
}

/// Register D61: `tests/pass/c41_method_moves_tracked` has five method bodies that freed
/// memory they had given away — a local moved into a call in a class method
/// (`Counter::go`), an impl method (`Job::run`) and an init's inner block (`Seeded::init`);
/// a returned local (`Counter::fresh`); and a param stored into a field (`Holder::init`).
/// The pass fixture's exit code sees the first three but not the last two (inkwell was
/// right about those), and `llvm-as` accepts every wrong free. So assert the oracle's IR
/// directly. Since ADR 0077 a moved binding keeps a drop at scope exit behind its moved
/// flag, so a body may call `sentinel_free` for it; what must hold is that the call never
/// runs: every `sentinel_free` in these bodies is the false branch of a test of a moved
/// flag that the same basic block set `true` before loading it.
#[test]
fn llvm_method_moves_are_not_freed() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .to_path_buf();
    let src = root.join("tests/pass/c41_method_moves_tracked.sentinel");
    let out = Command::new(env!("CARGO_BIN_EXE_snc"))
        .arg("llvm")
        .arg(&src)
        .output()
        .expect("run snc llvm");
    assert!(
        out.status.success(),
        "snc llvm failed on the D61 fixture:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let ll = String::from_utf8(out.stdout).expect("utf-8 dump");
    for sym in [
        "Counter__go",
        "Counter__fresh",
        "Holder__init",
        "default__Job__Runner__run",
        "Seeded__init",
    ] {
        let head = format!("@{sym}(");
        let lines: Vec<&str> = ll.lines().collect();
        let def = lines
            .iter()
            .position(|l| l.starts_with("define ") && l.contains(&head))
            .unwrap_or_else(|| panic!("no `define` for {sym} in:\n{ll}"));
        let body: Vec<&str> = lines[def..].iter().take_while(|l| **l != "}").copied().collect();
        for (i, l) in body.iter().enumerate() {
            if l.contains("@sentinel_free") {
                assert!(
                    free_is_behind_a_set_flag(&body, i),
                    "{sym} frees memory it no longer owns:\n{}",
                    body.join("\n")
                );
            }
        }
    }
}

/// ADR 0077 D5: a moved field gets a flag only if its binding's drop does something with
/// it. A class's drop releases the handles its fields hold and nothing else, so in
/// `secret_field` the moved `c.h` -- a `secret`-qualified struct holding a `Shared` -- is
/// released only behind its flag, on the path that did not move it, and in `array_field` the
/// moved `c.a`, which a class's drop never frees, gets no flag. A missing release is a leak,
/// not an exit code, so this reads the IR; the codegen differential holds `scg` to it.
#[test]
fn llvm_a_moved_field_gets_a_flag_only_if_its_binding_drop_touches_it() {
    let ir = llvm_dump(
        "a77_class_fields",
        concat!(
            "struct H { s: Shared<i64>, n: i64 }\n",
            "fn eat(h: secret H) -> i64 { 1 }\n",
            "fn consume(v: [i64]) -> i64 { v[0] }\n",
            "class C {\n",
            "    let h: secret H;\n",
            "    let a: [i64];\n",
            "    pub init(n: i64) { self.h = H { s: shared_new(n), n: n }; self.a = [n]; 0 }\n",
            "}\n",
            "fn secret_field(n: i64) -> i64 {\n",
            "    let c: C = C::init(40);\n",
            "    if n > 5 { eat(c.h) } else { 0 }\n",
            "}\n",
            "fn array_field(n: i64) -> i64 {\n",
            "    let c: C = C::init(40);\n",
            "    if n > 5 { consume(c.a) } else { 0 }\n",
            "}\n",
            "fn main() -> i64 { secret_field(1) + array_field(1) + 42 }\n",
        ),
    );
    let body = dump_fn_body(&ir, "secret_field");
    let lines: Vec<&str> = body.lines().collect();
    let releases: Vec<usize> = lines
        .iter()
        .enumerate()
        .filter(|(_, l)| l.contains("@sentinel_shared_release("))
        .map(|(i, _)| i)
        .collect();
    assert_eq!(releases.len(), 1, "@secret_field: one release, of `c.h.s`:\n{body}");
    assert!(
        drop_is_behind_a_moved_flag(&lines, releases[0]),
        "@secret_field: the moved field's release is behind its flag:\n{body}"
    );
    let body = dump_fn_body(&ir, "array_field");
    assert!(
        !body.contains("%mf"),
        "@array_field: a field a class's drop never touches gets no flag:\n{body}"
    );
}

/// ADR 0077: does the drop at `body[at]` sit behind a moved flag? Its block must be the false
/// target of `br i1 %c, ...` where `%c = load i1, ptr %mfN`, and `store i1 true, ptr %mfN`
/// must come earlier in the function. Weaker than [`free_is_behind_a_set_flag`], which also
/// asks for the store in the guard's own block: a second guard follows the first's join.
fn drop_is_behind_a_moved_flag(body: &[&str], at: usize) -> bool {
    let is_label = |l: &str| !l.starts_with(' ') && l.ends_with(':');
    let Some(label) = body[..at].iter().rev().find(|l| is_label(l)) else {
        return false;
    };
    let false_target = format!(", label %{}", label.trim_end_matches(':'));
    body.iter().enumerate().any(|(b, br)| {
        let Some(rest) = br.strip_prefix("  br i1 ") else {
            return false;
        };
        if !br.ends_with(&false_target) {
            return false;
        }
        let cond = rest.split(',').next().unwrap_or("");
        let load = format!("  {cond} = load i1, ptr %mf");
        body[..b].iter().any(|l| {
            l.starts_with(&load) && {
                let flag = &l[l.rfind("ptr ").map_or(0, |p| p + 4)..];
                body[..b].iter().any(|s| *s == format!("  store i1 true, ptr {flag}"))
            }
        })
    })
}

/// ADR 0077: is the `sentinel_free` at `body[at]` dead because a moved flag guards it? Its
/// block must be the false target of `br i1 %c, ...` where `%c = load i1, ptr %mfN`, and a
/// `store i1 true, ptr %mfN` must come before that load in the same basic block.
fn free_is_behind_a_set_flag(body: &[&str], at: usize) -> bool {
    let is_label = |l: &str| !l.starts_with(' ') && l.ends_with(':');
    let Some(label) = body[..at].iter().rev().find(|l| is_label(l)) else {
        return false;
    };
    let false_target = format!(", label %{}", label.trim_end_matches(':'));
    for (b, br) in body.iter().enumerate() {
        let Some(rest) = br.strip_prefix("  br i1 ") else {
            continue;
        };
        if !br.ends_with(&false_target) {
            continue;
        }
        let cond = rest.split(',').next().unwrap_or("");
        let load = format!("  {cond} = load i1, ptr ");
        // Walk back through the branch's own block to the flag's load.
        let mut j = b;
        while j > 0 && !is_label(body[j - 1]) {
            j -= 1;
            if let Some(flag) = body[j].strip_prefix(&load) {
                if !flag.starts_with("%mf") {
                    return false;
                }
                let set = format!("  store i1 true, ptr {flag}");
                return body[..j].iter().rev().take_while(|l| !is_label(l)).any(|l| *l == set);
            }
        }
    }
    false
}

/// ADR 0074 (register D79): a handler arm owns its continuation until it resumes it.
/// The golden pins both halves on one program: `k(10)` loads the kont from the arm's
/// slot (`%v7`), CLEARS the slot, then resumes (D1 — the argument is a constant, so the
/// golden cannot show that it is evaluated first; `llvm_every_handler_arm_exit_releases_the_kont`
/// pins that order on `early`); and the arm's fall-through (`bb6`) tests what the slot
/// holds and frees it only when it is not `null` (`bb9`) — the kont on the `else` path,
/// which declines to resume, and nothing after `k(10)` (D2). The test is in the emitted
/// code, so the program never depends on the runtime's own `null` check (D4).
#[test]
fn llvm_handler_arm_owns_its_continuation() {
    assert_eq!(
        llvm_dump(
            "armkont",
            "effect Io { read() -> i64; }\n\
             fn one(i: i64) -> i64 {\n\
             \x20   handle perform Io.read() with { Io.read(k) => if i > 2 { k(10) } else { 5 } }\n\
             }\n\
             fn main() -> i64 { one(1) + one(3) }\n"
        ),
        concat!(
            "target triple = \"arm64-apple-darwin\"\n",
            "\n",
            "declare ptr @sentinel_perform_op(i32, i64)\n",
            "declare ptr @sentinel_kont_resume(ptr, i64)\n",
            "declare i64 @sentinel_kont_consume_pure(ptr)\n",
            "declare void @sentinel_kont_free(ptr)\n",
            "\n",
            "define i64 @one(i64 %arg0) {\n",
            "entry:\n",
            "  %v0 = alloca i64\n",
            "  %v2 = alloca ptr\n",
            "  %v3 = alloca i64\n",
            "  %v7 = alloca ptr\n",
            "  %v15 = alloca i64\n",
            "  store i64 %arg0, ptr %v0\n",
            "  %v1 = call ptr @sentinel_perform_op(i32 0, i64 0)\n",
            "  store ptr %v1, ptr %v2\n",
            "  br label %bb0\n",
            "bb0:\n",
            "  %v4 = load ptr, ptr %v2\n",
            "  %v5 = load i32, ptr %v4\n",
            "  %v6 = icmp eq i32 %v5, 0\n",
            "  br i1 %v6, label %bb2, label %bb3\n",
            "bb2:\n",
            "  store ptr %v4, ptr %v7\n",
            "  %v8 = load i64, ptr %v0\n",
            "  %v9 = icmp sgt i64 %v8, 2\n",
            "  br i1 %v9, label %bb4, label %bb5\n",
            "bb4:\n",
            "  %v10 = load ptr, ptr %v7\n",
            "  store ptr null, ptr %v7\n",
            "  %v11 = call ptr @sentinel_kont_resume(ptr %v10, i64 10)\n",
            "  %v12 = load i32, ptr %v11\n",
            "  %v13 = icmp eq i32 %v12, 4294967295\n",
            "  br i1 %v13, label %bb7, label %bb8\n",
            "bb8:\n",
            "  store ptr %v11, ptr %v2\n",
            "  br label %bb0\n",
            "bb7:\n",
            "  %v14 = call i64 @sentinel_kont_consume_pure(ptr %v11)\n",
            "  store i64 %v14, ptr %v15\n",
            "  br label %bb6\n",
            "bb5:\n",
            "  store i64 5, ptr %v15\n",
            "  br label %bb6\n",
            "bb6:\n",
            "  %v16 = load i64, ptr %v15\n",
            "  %v17 = load ptr, ptr %v7\n",
            "  %v18 = icmp ne ptr %v17, null\n",
            "  br i1 %v18, label %bb9, label %bb10\n",
            "bb9:\n",
            "  call void @sentinel_kont_free(ptr %v17)\n",
            "  br label %bb10\n",
            "bb10:\n",
            "  store i64 %v16, ptr %v3\n",
            "  br label %bb1\n",
            "bb3:\n",
            "  %v19 = icmp eq i32 %v5, 4294967295\n",
            "  br i1 %v19, label %bb11, label %bb12\n",
            "bb11:\n",
            "  %v20 = call i64 @sentinel_kont_consume_pure(ptr %v4)\n",
            "  store i64 %v20, ptr %v3\n",
            "  br label %bb1\n",
            "bb12:\n",
            "  unreachable\n",
            "bb1:\n",
            "  %v21 = load i64, ptr %v3\n",
            "  ret i64 %v21\n",
            "}\n",
            "\n",
            "define i32 @main() {\n",
            "entry:\n",
            "  %v0 = call i64 @one(i64 1)\n",
            "  %v1 = call i64 @one(i64 3)\n",
            "  %v2 = add i64 %v0, %v1\n",
            "  %v3 = trunc i64 %v2 to i32\n",
            "  ret i32 %v3\n",
            "}\n",
            "\n",
        )
    );
}

/// The labels of the blocks in `body` (a `define`'s lines) reachable from `entry:`.
/// A block's successors are the `label %…` operands of its terminator — `br` is the
/// only instruction the oracle emits that names a block.
fn reachable_labels<'a>(body: &[&'a str]) -> Vec<&'a str> {
    let mut succ: Vec<(&str, Vec<&str>)> = Vec::new();
    for l in body.iter().skip(1) {
        if !l.starts_with(' ') {
            succ.push((l.trim_end_matches(':'), Vec::new()));
        } else if let Some((_, to)) = succ.last_mut() {
            for target in l.split("label %").skip(1) {
                to.push(target.split([',', ' ']).next().expect("a label name"));
            }
        }
    }
    let mut seen: Vec<&str> = Vec::new();
    let mut stack = vec!["entry"];
    while let Some(b) = stack.pop() {
        if !seen.contains(&b) {
            seen.push(b);
            let (_, to) = succ.iter().find(|(l, _)| *l == b).expect("a branch target is a block");
            stack.extend(to.iter().copied());
        }
    }
    seen
}

/// ADR 0075 D1 (register D87) in the TEXT oracle: a `k(v)` whose resume bubbles drains
/// the arm's scopes before it stores the new kont and branches back to the dispatch
/// loop. The codegen differential holds `scg` to this output byte-for-byte, so it
/// catches the two text back ends DIVERGING — not both of them regressing together,
/// which is what this pins. Memory only: every fn below exits the same either way.
#[test]
fn llvm_a_bubbling_resume_drains_the_arms_scopes() {
    // (fn, frees on its bubble path, frees everywhere else) over
    // `tests/pass/c75_bubble_drains_the_arm.sentinel`. `simple` holds one array in the
    // arm, `nested` two frames' worth, and `enclosing` one in the arm plus a SECOND array
    // below the arm floor, in the fn around the `handle`, which must not be drained here
    // — so a floor at the function makes its bubble count 2. (A `while` written INSIDE
    // the arm, around the `k(v)`, would open a body frame at the bubble as well; ADR 0075
    // D6 classifies such a `k(v)` (A), so its bubble aborts and that drain is
    // unreachable.)
    //
    // The second number is the half a bubble-only check cannot see: the drain is an
    // ADDITION, so the pure path must keep freeing what it freed before. An
    // implementation that MOVED the drops onto the bubble passes a bubble-only pin,
    // the fixture, the corpus differential and both bootstrap fixed points, and leaks
    // on the pure path at exactly the unfixed rate.
    let cases: &[(&str, usize, usize)] =
        &[("simple", 1, 1), ("nested", 2, 2), ("enclosing", 1, 2)];
    let src = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/pass/c75_bubble_drains_the_arm.sentinel");
    let out = Command::new(env!("CARGO_BIN_EXE_snc"))
        .arg("llvm")
        .arg(&src)
        .output()
        .expect("run snc llvm");
    assert!(
        out.status.success(),
        "snc llvm failed:
{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let ll = String::from_utf8(out.stdout).expect("utf-8 dump");
    let lines: Vec<&str> = ll.lines().collect();
    for (name, want, want_elsewhere) in cases {
        let head = format!("@{name}(");
        let def = lines
            .iter()
            .position(|l| l.starts_with("define ") && l.contains(&head))
            .unwrap_or_else(|| panic!("no `define` for {name} in:
{ll}"));
        let body: Vec<&str> = lines[def..].iter().take_while(|l| **l != "}").copied().collect();
        let live = reachable_labels(&body);
        // The dispatch loop reads its slot and then that kont's op id; a BUBBLE block is
        // any later block that ends by storing a kont into that slot and branching back.
        // `entry:` ends the same way — the body's kont is stored into the dispatch slot
        // before the branch into the loop — so it is skipped explicitly. (It is not
        // excluded "by construction": `blocks` below begins AT `entry:`, and a first
        // draft of this pin found two bubbles in every fn for that reason.)
        let dispatch_slots: Vec<&str> = body
            .windows(2)
            .filter_map(|w| {
                let (kont, slot) = w[0].trim().split_once(" = load ptr, ptr ")?;
                w[1].trim().ends_with(&format!(" = load i32, ptr {kont}")).then_some(slot)
            })
            .collect();
        assert!(!dispatch_slots.is_empty(), "@{name} has no dispatch loop:
{}", body.join("
"));
        let mut blocks: Vec<(&str, Vec<&str>)> = Vec::new();
        for l in body.iter().skip(1) {
            if !l.starts_with(' ') {
                blocks.push((l.trim_end_matches(':'), Vec::new()));
            } else if let Some((_, ls)) = blocks.last_mut() {
                ls.push(l.trim());
            }
        }
        let bubbles: Vec<&(&str, Vec<&str>)> = blocks
            .iter()
            .skip(1)
            .filter(|(label, ls)| {
                live.contains(label)
                    && ls.len() >= 2
                    && ls[ls.len() - 1].starts_with("br label %")
                    && dispatch_slots.iter().any(|slot| {
                        ls[ls.len() - 2].starts_with("store ptr ")
                            && ls[ls.len() - 2].ends_with(&format!(", ptr {slot}"))
                    })
            })
            .collect();
        assert_eq!(
            bubbles.len(),
            1,
            "@{name}: expected one reachable bubble block, found {} — this pin proves              nothing about a fn whose `k(v)` site it cannot find:
{}",
            bubbles.len(),
            body.join("
")
        );
        let (label, ls) = bubbles[0];
        let frees = ls.iter().filter(|l| l.contains("@sentinel_free(")).count();
        assert_eq!(
            frees, *want,
            "@{name}: the bubble at {label} frees {frees} of the arm's bindings, not              {want} — a floor below the arm reaches the function's own, and one above              misses the arm's outermost frame:
{}",
            ls.join("
")
        );
        let elsewhere: usize = blocks
            .iter()
            .filter(|(l, _)| *l != *label && live.contains(l))
            .map(|(_, ls)| ls.iter().filter(|l| l.contains("@sentinel_free(")).count())
            .sum();
        assert_eq!(
            elsewhere, *want_elsewhere,
            "@{name}: {elsewhere} frees outside the bubble, not {want_elsewhere} — the              drops were moved onto the bubble rather than added to it:
{}",
            body.join("
")
        );
    }
}

/// ADR 0074 D2 over `tests/fixtures/handler_arm_exits/`: each fn's count of
/// `sentinel_kont_free` calls that can run — in a block reachable from `entry:` — is
/// its number of (exit, open arm) pairs: the fall-through, a `return`, a `break` /
/// `continue` to a loop around the `handle`, including one taken inside a `k(v)`
/// argument (D1), and TWO for a `return`, `break` or `continue` that leaves an inner
/// arm and the outer arm around it (`c74_two_open_arms`; the inner arm's fall-through
/// counts one). A loop INSIDE the arm (`inner_loop`) adds none, and `after`'s arm ends
/// in a `return`, so the fall-through release emitted after it is dead and does not
/// count — nor would a `return`, `break` or `continue` release moved past its exit's
/// terminator into the dead block the oracle opens there. (A fall-through release moved
/// past its branch into the next dispatch check would still count, as that block is
/// live; the golden above pins where that release goes.) Every release is guarded — the
/// call sits in a block reached only when `icmp ne ptr %vK, null` holds (D4) — and reads
/// its kont from an arm's continuation slot, never from a dispatch slot. (The check
/// recognises an arm slot as any slot a dispatched kont is stored into, so in
/// `c74_two_open_arms` it would also accept the inner `handle`'s result slot, which that
/// handle's pure and propagate paths write; no release reads it.) Every
/// `sentinel_kont_resume` is preceded by the load of the arm's slot and the store that
/// clears it (D1). `scg` matches these byte-for-byte because the same files are seeds
/// of the codegen differential; this is what makes the oracle's side of that
/// comparison mean something. (Not `tests/pass` files: see `tests/handler_arm_exits.rs`
/// for why.)
#[test]
fn llvm_every_handler_arm_exit_releases_the_kont() {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/handler_arm_exits");
    let cases: &[(&str, &[(&str, usize)])] = &[
        (
            "c74_arm_declines_to_resume",
            &[("fixed", 1), ("maybe", 1), ("skip_frame", 1), ("framed", 0), ("main", 0)],
        ),
        ("c74_arm_return_leaves_the_arm", &[("before", 2), ("after", 1)]),
        ("c74_arm_break_continue", &[("sum", 3), ("inner_loop", 1)]),
        ("c74_resume_arg_leaves_the_arm", &[("early", 2), ("in_loop", 2)]),
        ("c74_two_open_arms", &[("ret_inner", 4), ("brk_both", 4), ("cont_both", 4)]),
    ];
    for (fixture, fns) in cases {
        let src = dir.join(format!("{fixture}.sentinel"));
        let out = Command::new(env!("CARGO_BIN_EXE_snc"))
            .arg("llvm")
            .arg(&src)
            .output()
            .expect("run snc llvm");
        assert!(
            out.status.success(),
            "snc llvm failed on {fixture}:\n{}",
            String::from_utf8_lossy(&out.stderr)
        );
        let ll = String::from_utf8(out.stdout).expect("utf-8 dump");
        let lines: Vec<&str> = ll.lines().collect();
        for (name, want) in *fns {
            let head = format!("@{name}(");
            let def = lines
                .iter()
                .position(|l| l.starts_with("define ") && l.contains(&head))
                .unwrap_or_else(|| panic!("{fixture}: no `define` for {name} in:\n{ll}"));
            let body: Vec<&str> = lines[def..].iter().take_while(|l| **l != "}").copied().collect();
            let live = reachable_labels(&body);
            // A dispatch loop reads its slot and then the kont's op id; an arm head
            // stores that dispatched kont into the arm's own continuation slot.
            let dispatch: Vec<(&str, &str)> = body
                .windows(2)
                .filter_map(|w| {
                    let (kont, slot) = w[0].trim().split_once(" = load ptr, ptr ")?;
                    w[1].ends_with(&format!(" = load i32, ptr {kont}")).then_some((kont, slot))
                })
                .collect();
            let arm_slots: Vec<&str> = body
                .iter()
                .filter_map(|l| {
                    let (v, slot) = l.trim().strip_prefix("store ptr ")?.split_once(", ptr ")?;
                    dispatch.iter().any(|(kont, _)| *kont == v).then_some(slot)
                })
                .collect();
            let got = body
                .iter()
                .enumerate()
                .filter(|(i, l)| {
                    l.contains("call void @sentinel_kont_free(")
                        && live.contains(&body[i - 1].trim_end_matches(':'))
                })
                .count();
            assert_eq!(got, *want, "{fixture}: @{name} releases on {got} exits:\n{}", body.join("\n"));
            // D4: each release is `%vK = load ptr, ptr %vS` / `%vN = icmp ne ptr %vK,
            // null` / `br i1 %vN, label %bbF, …` / `bbF:` / the call, and `%vS` is an arm
            // slot, not a dispatch slot (ADR 0065 D6's old record).
            for (i, l) in body.iter().enumerate() {
                if let Some(rest) = l.strip_prefix("  call void @sentinel_kont_free(ptr ") {
                    let kont = rest.trim_end_matches(')');
                    let label = body[i - 1].trim_end_matches(':');
                    let test = body[i - 3];
                    let owned = test.split(" = ").next().unwrap().trim();
                    assert_eq!(
                        test,
                        format!("  {owned} = icmp ne ptr {kont}, null"),
                        "{fixture}: @{name}: an unguarded release"
                    );
                    assert!(
                        body[i - 2].starts_with(&format!("  br i1 {owned}, label %{label},")),
                        "{fixture}: @{name}: the release is not the guard's taken branch"
                    );
                    let slot = body[i - 4]
                        .strip_prefix(&format!("  {kont} = load ptr, ptr "))
                        .unwrap_or_else(|| panic!("{fixture}: @{name}: the released kont is not a slot load"));
                    assert!(
                        arm_slots.contains(&slot) && !dispatch.iter().any(|(_, s)| *s == slot),
                        "{fixture}: @{name}: a release reads {slot}, which is not an arm's continuation slot"
                    );
                }
            }
            if *name == "early" {
                // D1's ORDER: the argument `if i > 2 { return 9 } else { i }` is
                // evaluated before the slot is read and cleared, so the `return` inside
                // it still finds the kont owned. Reading first would put the clear
                // above the argument's compare, and the release on that `return` would
                // free `null`.
                let cmp = body.iter().position(|l| l.contains("icmp sgt")).expect("the argument's compare");
                let clear = body.iter().position(|l| l.starts_with("  store ptr null")).expect("the slot clear");
                assert!(cmp < clear, "{fixture}: @early clears the slot before its argument:\n{}", body.join("\n"));
            }
            for (i, l) in body.iter().enumerate() {
                if let Some(rest) = l.split("@sentinel_kont_resume(ptr ").nth(1) {
                    let kont = rest.split(',').next().unwrap();
                    let slot = body[i - 1]
                        .strip_prefix("  store ptr null, ptr ")
                        .unwrap_or_else(|| panic!("{fixture}: @{name}: no slot clear before `{l}`"));
                    assert_eq!(
                        body[i - 2],
                        format!("  {kont} = load ptr, ptr {slot}"),
                        "{fixture}: @{name}: the resumed kont is not the one loaded from the cleared slot"
                    );
                }
            }
        }
    }
}

// ---- Layer 3: behavioural parity (textual .ll == inkwell) ---------------

fn runtime_lib() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_snc"))
        .parent()
        .unwrap()
        .join("libsentinel_runtime.a")
}

fn run_capture(bin: &Path) -> (Option<i32>, Vec<u8>) {
    let out = Command::new(bin).output().expect("run compiled binary");
    (out.status.code(), out.stdout)
}

#[test]
fn llvm_behaviour_matches_inkwell_over_emitted_subset() {
    let runtime = runtime_lib();
    assert!(
        runtime.exists(),
        "libsentinel_runtime.a not found at {} (build the workspace first)",
        runtime.display()
    );
    let dir = temp_dir("behaviour");
    let mut checked = 0;

    for f in corpus_fixtures() {
        if !f.to_string_lossy().contains("tests/pass") {
            continue; // pass fixtures run cleanly; ui fixtures are reject cases
        }
        // Only the emitted subset.
        let dump = Command::new(env!("CARGO_BIN_EXE_snc"))
            .arg("llvm")
            .arg(&f)
            .output()
            .expect("run snc llvm");
        if !dump.status.success() {
            continue;
        }

        // Ground truth: the inkwell backend.
        let gt_bin = dir.join("gt");
        let built = Command::new(env!("CARGO_BIN_EXE_snc"))
            .arg("build")
            .arg(&f)
            .arg("-o")
            .arg(&gt_bin)
            .output()
            .expect("run snc build");
        if !built.status.success() {
            continue; // not behaviourally comparable here
        }
        let (gt_code, gt_out) = run_capture(&gt_bin);

        // Textual path: write the .ll, compile with cc, run.
        let ll_path = dir.join("out.ll");
        std::fs::write(&ll_path, &dump.stdout).expect("write .ll");
        let tx_bin = dir.join("tx");
        let cc = Command::new("cc")
            .arg(&ll_path)
            .arg(&runtime)
            .arg("-o")
            .arg(&tx_bin)
            .output()
            .expect("run cc on the .ll");
        assert!(
            cc.status.success(),
            "cc failed to compile the canonical .ll for {}:\n{}",
            f.display(),
            String::from_utf8_lossy(&cc.stderr)
        );
        let (tx_code, tx_out) = run_capture(&tx_bin);

        assert_eq!(
            (gt_code, &gt_out),
            (tx_code, &tx_out),
            "behavioural mismatch on {} (inkwell vs textual .ll)",
            f.display()
        );
        checked += 1;
    }

    assert!(
        checked >= 15,
        "expected to behaviourally check the straight-line subset (~16), got {checked}"
    );
}

/// Register D59: every `load T, ptr %vN` from a hoisted `%vN = alloca U` must have `T == U`. A
/// load wider than its slot reads past it. This does NOT catch D59 as it shipped: the oracle
/// sized the `if` slot AND its load from the divergent THEN arm, so the two agreed, and the
/// overrun was the other arm's store — a full revert fails `llvm-as` and the pass exit codes
/// instead. What it catches is a half-revert that sizes the slot from the then arm again but
/// loads at the join's type, in the oracle AND scg together. That is byte-identical, so the
/// differential stays green; opaque pointers mean `llvm-as` never relates a load or a store to
/// its alloca; and on Windows the one test that EXECUTES the oracle's IR
/// (`llvm_behaviour_matches_inkwell_over_emitted_subset`) cannot run — so it would pass every
/// other test here (both measured by the D59/D60 reviews). Stores are deliberately NOT
/// checked: a divergent arm stores at its own type, into a dead block, by design.
#[test]
fn llvm_loads_match_their_slot_over_corpus() {
    let mut checked = 0;
    let mut bad: Vec<String> = Vec::new();
    for f in corpus_fixtures() {
        let dump = Command::new(env!("CARGO_BIN_EXE_snc"))
            .arg("llvm")
            .arg(&f)
            .output()
            .expect("run snc llvm");
        if !dump.status.success() {
            continue;
        }
        checked += 1;
        let ll = String::from_utf8_lossy(&dump.stdout);
        let mut func = String::new();
        let mut slots: std::collections::HashMap<String, String> = std::collections::HashMap::new();
        for line in ll.lines() {
            if let Some(rest) = line.strip_prefix("define ") {
                func = rest.split('@').nth(1).unwrap_or("").split('(').next().unwrap_or("").to_string();
                slots.clear();
                continue;
            }
            let t = line.trim();
            if let Some((lhs, ty)) = t.split_once(" = alloca ") {
                slots.insert(lhs.trim_start_matches('%').to_string(), ty.trim().to_string());
                continue;
            }
            if let Some((_, rhs)) = t.split_once("= load ") {
                if let Some((ty, ptr)) = rhs.rsplit_once(", ptr %") {
                    if let Some(slot_ty) = slots.get(ptr.trim()) {
                        if slot_ty != ty.trim() {
                            bad.push(format!(
                                "  {}: @{func}: `{t}` from a slot allocated `{slot_ty}`",
                                f.file_name().unwrap_or_default().to_string_lossy()
                            ));
                        }
                    }
                }
            }
        }
    }
    assert!(checked >= 170, "expected the emitting corpus to be checked, got {checked}");
    assert!(bad.is_empty(), "loads wider or narrower than their slot:\n{}", bad.join("\n"));
}

/// ADR 0077: a moved binding's drop is decided per exit. In `early`, the `return` comes
/// before any move of `v` in the emitted code, so its drain frees `v` unconditionally (D2);
/// the scope's end comes after the move in `bb4`, so it frees `v` only when `%mf0`, set at
/// the move, is `false`, and stores it back `false` (D3). In `field`, only the moved field
/// `a` is guarded, by its own flag, and `b` is freed unconditionally (D5). Each flag is
/// numbered in the order the walk first needs it, and its `alloca` and `false` store follow
/// the hoisted allocas.
#[test]
fn llvm_a_moved_binding_drop_is_decided_per_exit() {
    assert_eq!(
        llvm_dump(
            "dropflags",
            "struct S { a: [i64], b: [i64] }\n\
             fn consume(v: [i64]) -> i64 { v[0] }\n\
             fn early(n: i64) -> i64 {\n\
             \x20   let v: [i64] = [1, 2];\n\
             \x20   if n < 0 { return 0 } else { 0 };\n\
             \x20   if n > 5 { consume(v) } else { 0 }\n\
             }\n\
             fn field(n: i64) -> i64 {\n\
             \x20   let s: S = S { a: [1], b: [2] };\n\
             \x20   if n > 5 { consume(s.a) } else { 0 }\n\
             }\n\
             fn main() -> i64 { early(1) + field(1) }\n"
        ),
        concat!(
            "target triple = \"arm64-apple-darwin\"\n",
            "\n",
            "%Struct.0 = type { { i64, ptr }, { i64, ptr } }\n",
            "\n",
            "declare ptr @sentinel_alloc(i64)\n",
            "declare void @sentinel_free(ptr)\n",
            "declare void @sentinel_panic_oob(i64, i64)\n",
            "\n",
            "define i64 @consume({ i64, ptr } %arg0) {\n",
            "entry:\n",
            "  %v0 = alloca { i64, ptr }\n",
            "  store { i64, ptr } %arg0, ptr %v0\n",
            "  %v1 = load { i64, ptr }, ptr %v0\n",
            "  %v2 = extractvalue { i64, ptr } %v1, 0\n",
            "  %v3 = extractvalue { i64, ptr } %v1, 1\n",
            "  %v4 = icmp sge i64 0, 0\n",
            "  %v5 = icmp slt i64 0, %v2\n",
            "  %v6 = and i1 %v4, %v5\n",
            "  br i1 %v6, label %bb1, label %bb0\n",
            "bb0:\n",
            "  call void @sentinel_panic_oob(i64 0, i64 %v2)\n",
            "  unreachable\n",
            "bb1:\n",
            "  %v7 = getelementptr i64, ptr %v3, i64 0\n",
            "  %v8 = load i64, ptr %v7\n",
            "  %v9 = load { i64, ptr }, ptr %v0\n",
            "  %v10 = extractvalue { i64, ptr } %v9, 1\n",
            "  call void @sentinel_free(ptr %v10)\n",
            "  ret i64 %v8\n",
            "}\n",
            "\n",
            "define i64 @early(i64 %arg0) {\n",
            "entry:\n",
            "  %v0 = alloca i64\n",
            "  %v8 = alloca { i64, ptr }\n",
            "  %v13 = alloca i64\n",
            "  %v19 = alloca i64\n",
            "  %mf0 = alloca i1\n",
            "  store i1 false, ptr %mf0\n",
            "  store i64 %arg0, ptr %v0\n",
            "  %v1 = getelementptr i64, ptr null, i64 2\n",
            "  %v2 = ptrtoint ptr %v1 to i64\n",
            "  %v3 = call ptr @sentinel_alloc(i64 %v2)\n",
            "  %v4 = getelementptr i64, ptr %v3, i64 0\n",
            "  store i64 1, ptr %v4\n",
            "  %v5 = getelementptr i64, ptr %v3, i64 1\n",
            "  store i64 2, ptr %v5\n",
            "  %v6 = insertvalue { i64, ptr } undef, i64 2, 0\n",
            "  %v7 = insertvalue { i64, ptr } %v6, ptr %v3, 1\n",
            "  store { i64, ptr } %v7, ptr %v8\n",
            "  %v9 = load i64, ptr %v0\n",
            "  %v10 = icmp slt i64 %v9, 0\n",
            "  br i1 %v10, label %bb0, label %bb1\n",
            "bb0:\n",
            "  %v11 = load { i64, ptr }, ptr %v8\n",
            "  %v12 = extractvalue { i64, ptr } %v11, 1\n",
            "  call void @sentinel_free(ptr %v12)\n",
            "  ret i64 0\n",
            "bb3:\n",
            "  store i64 zeroinitializer, ptr %v13\n",
            "  br label %bb2\n",
            "bb1:\n",
            "  store i64 0, ptr %v13\n",
            "  br label %bb2\n",
            "bb2:\n",
            "  %v14 = load i64, ptr %v13\n",
            "  %v15 = load i64, ptr %v0\n",
            "  %v16 = icmp sgt i64 %v15, 5\n",
            "  br i1 %v16, label %bb4, label %bb5\n",
            "bb4:\n",
            "  %v17 = load { i64, ptr }, ptr %v8\n",
            "  store i1 true, ptr %mf0\n",
            "  %v18 = call i64 @consume({ i64, ptr } %v17)\n",
            "  store i64 %v18, ptr %v19\n",
            "  br label %bb6\n",
            "bb5:\n",
            "  store i64 0, ptr %v19\n",
            "  br label %bb6\n",
            "bb6:\n",
            "  %v20 = load i64, ptr %v19\n",
            "  %v21 = load i1, ptr %mf0\n",
            "  br i1 %v21, label %bb8, label %bb7\n",
            "bb7:\n",
            "  %v22 = load { i64, ptr }, ptr %v8\n",
            "  %v23 = extractvalue { i64, ptr } %v22, 1\n",
            "  call void @sentinel_free(ptr %v23)\n",
            "  br label %bb8\n",
            "bb8:\n",
            "  store i1 false, ptr %mf0\n",
            "  ret i64 %v20\n",
            "}\n",
            "\n",
            "define i64 @field(i64 %arg0) {\n",
            "entry:\n",
            "  %v0 = alloca i64\n",
            "  %v15 = alloca %Struct.0\n",
            "  %v21 = alloca i64\n",
            "  %mf0 = alloca i1\n",
            "  store i1 false, ptr %mf0\n",
            "  store i64 %arg0, ptr %v0\n",
            "  %v1 = getelementptr i64, ptr null, i64 1\n",
            "  %v2 = ptrtoint ptr %v1 to i64\n",
            "  %v3 = call ptr @sentinel_alloc(i64 %v2)\n",
            "  %v4 = getelementptr i64, ptr %v3, i64 0\n",
            "  store i64 1, ptr %v4\n",
            "  %v5 = insertvalue { i64, ptr } undef, i64 1, 0\n",
            "  %v6 = insertvalue { i64, ptr } %v5, ptr %v3, 1\n",
            "  %v7 = getelementptr i64, ptr null, i64 1\n",
            "  %v8 = ptrtoint ptr %v7 to i64\n",
            "  %v9 = call ptr @sentinel_alloc(i64 %v8)\n",
            "  %v10 = getelementptr i64, ptr %v9, i64 0\n",
            "  store i64 2, ptr %v10\n",
            "  %v11 = insertvalue { i64, ptr } undef, i64 1, 0\n",
            "  %v12 = insertvalue { i64, ptr } %v11, ptr %v9, 1\n",
            "  %v13 = insertvalue %Struct.0 undef, { i64, ptr } %v6, 0\n",
            "  %v14 = insertvalue %Struct.0 %v13, { i64, ptr } %v12, 1\n",
            "  store %Struct.0 %v14, ptr %v15\n",
            "  %v16 = load i64, ptr %v0\n",
            "  %v17 = icmp sgt i64 %v16, 5\n",
            "  br i1 %v17, label %bb0, label %bb1\n",
            "bb0:\n",
            "  %v18 = load %Struct.0, ptr %v15\n",
            "  %v19 = extractvalue %Struct.0 %v18, 0\n",
            "  store i1 true, ptr %mf0\n",
            "  %v20 = call i64 @consume({ i64, ptr } %v19)\n",
            "  store i64 %v20, ptr %v21\n",
            "  br label %bb2\n",
            "bb1:\n",
            "  store i64 0, ptr %v21\n",
            "  br label %bb2\n",
            "bb2:\n",
            "  %v22 = load i64, ptr %v21\n",
            "  %v23 = load i1, ptr %mf0\n",
            "  br i1 %v23, label %bb4, label %bb3\n",
            "bb3:\n",
            "  %v24 = getelementptr %Struct.0, ptr %v15, i32 0, i32 0\n",
            "  %v25 = load { i64, ptr }, ptr %v24\n",
            "  %v26 = extractvalue { i64, ptr } %v25, 1\n",
            "  call void @sentinel_free(ptr %v26)\n",
            "  br label %bb4\n",
            "bb4:\n",
            "  store i1 false, ptr %mf0\n",
            "  %v27 = getelementptr %Struct.0, ptr %v15, i32 0, i32 1\n",
            "  %v28 = load { i64, ptr }, ptr %v27\n",
            "  %v29 = extractvalue { i64, ptr } %v28, 1\n",
            "  call void @sentinel_free(ptr %v29)\n",
            "  ret i64 %v22\n",
            "}\n",
            "\n",
            "define i32 @main() {\n",
            "entry:\n",
            "  %v0 = call i64 @early(i64 1)\n",
            "  %v1 = call i64 @field(i64 1)\n",
            "  %v2 = add i64 %v0, %v1\n",
            "  %v3 = trunc i64 %v2 to i32\n",
            "  ret i32 %v3\n",
            "}\n",
            "\n",
        )
    );
}
