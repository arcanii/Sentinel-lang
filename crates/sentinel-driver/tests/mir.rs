//! Phase D self-host port (7/N) / ADR 0044 D2: golden tests for the `snc mir`
//! lowered-form-dump oracle — the SSA/CFG MIR the Sentinel-written MIR stage
//! (`selfhost/mir.sentinel`, via `types::run` mode 2) must reproduce
//! byte-for-byte. One `(fn …)` per top-level user fn in FnId order; each fn its
//! return type then its blocks (in id order); each block its SSA params,
//! instructions, terminator. A value def is `v<N>:<ty>` (type via
//! `type_display`), a use is the bare `v<N>`; operators use `.symbol()`; no
//! spans. `snc mir` lowers TOTALLY (never rejects) and does NOT run the
//! constant-time verifier, so the dump is the lowered form regardless of any
//! leak; it exits nonzero only on an upstream parse/resolve/type error, so the
//! corpus differential skips those.

use std::path::PathBuf;
use std::process::Command;

fn temp_file(name: &str, contents: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("snc_mir_{}_{}", std::process::id(), name));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("create temp dir");
    let path = dir.join("input.sentinel");
    std::fs::write(&path, contents).expect("write source");
    path
}

fn mir_dump(name: &str, contents: &str) -> String {
    let path = temp_file(name, contents);
    let out = Command::new(env!("CARGO_BIN_EXE_snc"))
        .arg("mir")
        .arg(&path)
        .output()
        .expect("run snc mir");
    assert!(
        out.status.success(),
        "snc mir failed; stderr:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8(out.stdout).expect("utf-8 dump")
}

#[test]
fn mir_straight_line_and_call() {
    // No control flow ⇒ one block; the entry params are the fn params; the
    // tail is the return value. A free call names its callee.
    assert_eq!(
        mir_dump(
            "straight",
            "fn dbl(x: i64) -> i64 { x * 2 }\nfn main() -> i64 { dbl(21) }\n"
        ),
        "(fn dbl i64 (block b0 (params v0:i64) (v1:i64 const_int 2) (v2:i64 binop * v0 v1) (term return v2)))\n\
         (fn main i64 (block b0 (params) (v0:i64 const_int 21) (v1:i64 call dbl v0) (term return v1)))\n"
    );
}

#[test]
fn mir_if_lowers_to_branch_and_merge_param() {
    // `if c { 1 } else { 2 }` ⇒ entry branches into b1/b2, each jumps to the
    // merge b3 whose single param is the if-result.
    assert_eq!(
        mir_dump(
            "if",
            "fn pick(c: bool) -> i64 { if c { 1 } else { 2 } }\nfn main() -> i64 { 0 }\n"
        ),
        "(fn pick i64 (block b0 (params v0:bool) (term branch v0 (b1) (b2))) \
         (block b1 (params) (v1:i64 const_int 1) (term jump b3 v1)) \
         (block b2 (params) (v2:i64 const_int 2) (term jump b3 v2)) \
         (block b3 (params v3:i64) (term return v3)))\n\
         (fn main i64 (block b0 (params) (v0:i64 const_int 0) (term return v0)))\n"
    );
}

#[test]
fn mir_logical_and_is_a_short_circuit_branch() {
    // `a && b` is control flow (so the D5 verifier can see a secret-dependent
    // short-circuit): branch on `a`; the true edge evaluates `b`, the false
    // edge short-circuits to `false`; merge carries the result.
    assert_eq!(
        mir_dump(
            "and",
            "fn both(a: bool, b: bool) -> bool { a && b }\nfn main() -> i64 { 0 }\n"
        ),
        "(fn both bool (block b0 (params v0:bool v1:bool) (term branch v0 (b1) (b2))) \
         (block b1 (params) (term jump b3 v1)) \
         (block b2 (params) (v2:bool const_bool false) (term jump b3 v2)) \
         (block b3 (params v3:bool) (term return v3)))\n\
         (fn main i64 (block b0 (params) (v0:i64 const_int 0) (term return v0)))\n"
    );
}

#[test]
fn mir_secret_type_renders_and_secret_short_circuit_branches() {
    // `secret bool && secret bool` type-checks (SecretBranch only rejects `if`)
    // and lowers to a branch on a `secret bool` cond — the leak the verifier
    // (D6) flags. The `secret bool` type renders structurally.
    assert_eq!(
        mir_dump(
            "secret",
            "fn f(s: secret bool, t: secret bool) -> secret bool { s && t }\nfn main() -> i64 { 0 }\n"
        ),
        "(fn f secret bool (block b0 (params v0:secret bool v1:secret bool) (term branch v0 (b1) (b2))) \
         (block b1 (params) (term jump b3 v1)) \
         (block b2 (params) (v2:secret bool const_bool false) (term jump b3 v2)) \
         (block b3 (params v3:secret bool) (term return v3)))\n\
         (fn main i64 (block b0 (params) (v0:i64 const_int 0) (term return v0)))\n"
    );
}

#[test]
fn mir_var_reassigned_in_one_arm_threads_a_merge_param() {
    // `x` is reassigned only on the then-arm ⇒ the merge carries TWO params
    // (the if-result v5, then the diverged var v6 in VarId order); the arms
    // jump with matching args; the body returns the merged `x` (v6).
    assert_eq!(
        mir_dump(
            "merge",
            "fn g(c: bool) -> i64 { let mut x = 1; if c { x = 2; 0 } else { 0 }; x }\nfn main() -> i64 { 0 }\n"
        ),
        "(fn g i64 (block b0 (params v0:bool) (v1:i64 const_int 1) (term branch v0 (b1) (b2))) \
         (block b1 (params) (v2:i64 const_int 2) (v3:i64 const_int 0) (term jump b3 v3 v2)) \
         (block b2 (params) (v4:i64 const_int 0) (term jump b3 v4 v1)) \
         (block b3 (params v5:i64 v6:i64) (term return v6)))\n\
         (fn main i64 (block b0 (params) (v0:i64 const_int 0) (term return v0)))\n"
    );
}

#[test]
fn mir_index_is_a_load_and_div_is_a_binary() {
    // `a[i]` ⇒ `load <base> <index>` (a secret index would be the D5 leak);
    // `_ / 2` ⇒ `binop /` (a secret divisor would be the D5 leak).
    assert_eq!(
        mir_dump(
            "loaddiv",
            "fn at(a: [i64], i: i64) -> i64 { a[i] / 2 }\nfn main() -> i64 { 0 }\n"
        ),
        "(fn at i64 (block b0 (params v0:[i64] v1:i64) (v2:i64 load v0 v1) (v3:i64 const_int 2) (v4:i64 binop / v2 v3) (term return v4)))\n\
         (fn main i64 (block b0 (params) (v0:i64 const_int 0) (term return v0)))\n"
    );
}

#[test]
fn mir_declassify_and_unary() {
    // `declassify(s)` ⇒ the one taint sink; `-n` ⇒ `unary -`.
    assert_eq!(
        mir_dump(
            "declassify",
            "fn h(s: secret i64) -> i64 { let n = declassify(s); -n }\nfn main() -> i64 { 0 }\n"
        ),
        "(fn h i64 (block b0 (params v0:secret i64) (v1:i64 declassify v0) (v2:i64 unary - v1) (term return v2)))\n\
         (fn main i64 (block b0 (params) (v0:i64 const_int 0) (term return v0)))\n"
    );
}

/// `snc ctverify <file>` — the const-time verifier's leak set (ADR 0044 D6).
fn ctverify_dump(name: &str, contents: &str) -> String {
    let path = temp_file(name, contents);
    let out = Command::new(env!("CARGO_BIN_EXE_snc"))
        .arg("ctverify")
        .arg(&path)
        .output()
        .expect("run snc ctverify");
    assert!(
        out.status.success(),
        "snc ctverify failed; stderr:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8(out.stdout).expect("utf-8 dump")
}

#[test]
fn ctverify_secret_short_circuit_is_a_branch_leak() {
    // `secret bool && secret bool` type-checks (SecretBranch only rejects `if`) but
    // lowers to a branch on a secret cond — the one type-clean MIR leak.
    assert_eq!(
        ctverify_dump(
            "leak",
            "fn f(s: secret bool, t: secret bool) -> secret bool { s && t }\nfn main() -> i64 { 0 }\n"
        ),
        "(leak Branch)\n"
    );
}

#[test]
fn ctverify_constant_time_secret_has_no_leak() {
    // A secret flowing through arithmetic / compare / declassify is constant-time —
    // the verifier reports nothing (no false positive).
    assert_eq!(
        ctverify_dump(
            "clean",
            "fn f(a: secret i64, b: secret i64) -> i64 { declassify(a + b) }\nfn main() -> i64 { 0 }\n"
        ),
        ""
    );
}

#[test]
fn mir_struct_lit_and_field_are_opaque() {
    // Aggregates the minimal IR doesn't model precisely funnel through
    // `opaque` carrying their operands (so taint can't vanish).
    assert_eq!(
        mir_dump(
            "opaque",
            "struct P { x: i64 }\nfn mk() -> i64 { let p = P { x: 7 }; p.x }\nfn main() -> i64 { 0 }\n"
        ),
        "(fn mk i64 (block b0 (params) (v0:i64 const_int 7) (v1:P opaque v0) (v2:i64 opaque v1) (term return v2)))\n\
         (fn main i64 (block b0 (params) (v0:i64 const_int 0) (term return v0)))\n"
    );
}
