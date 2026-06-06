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
    //  - `arr` is moved into `consume(arr)` (consuming call) → NOT freed in `main`; the
    //    callee owns + frees it. No double-free. consume(arr)=1 + tmp[0]=4 = 5.
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
            "  %v29 = call i64 @consume({ i64, ptr } %v28)\n",
            "  %v30 = load i64, ptr %v27\n",
            "  %v31 = add i64 %v29, %v30\n",
            "  %v32 = trunc i64 %v31 to i32\n",
            "  ret i32 %v32\n",
            "}\n",
            "\n",
        )
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
    // `snc llvm` is partial-by-Err: it either emits (0) or cleanly Errs (1) —
    // never a panic (101) or a signal. Emission grows per sub-slice; the
    // floor guards against a regression that stops emitting the straight-line
    // subset entirely.
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
