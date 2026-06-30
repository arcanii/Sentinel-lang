//! Phase D self-host port (6/N) / ADR 0043 D2: golden tests for the `snc borrow`
//! moved-sources-dump oracle — the per-fn moved-source set the Sentinel-written
//! borrow-check stage (`selfhost/borrow.sentinel`) must reproduce byte-for-byte.
//! One line per user fn in FnId order: `(fn #<id> <name> #<vid>…)` (the VarIds used
//! in a consuming/move position whose type is non-Copy — codegen's drop-skip set);
//! an empty set dumps `(fn #N name)`. `snc borrow` exits nonzero on any error (incl.
//! a borrow error), so the corpus differential skips rejected fixtures.

use std::path::PathBuf;
use std::process::Command;

fn temp_file(name: &str, contents: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("snc_borrow_{}_{}", std::process::id(), name));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("create temp dir");
    let path = dir.join("input.sentinel");
    std::fs::write(&path, contents).expect("write source");
    path
}

fn borrow_dump(name: &str, contents: &str) -> String {
    let path = temp_file(name, contents);
    let out = Command::new(env!("CARGO_BIN_EXE_snc"))
        .arg("borrow")
        .arg(&path)
        .output()
        .expect("run snc borrow");
    assert!(
        out.status.success(),
        "snc borrow failed; stderr:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8(out.stdout).expect("utf-8 dump")
}

#[test]
fn borrow_dump_copy_no_moves() {
    // All-scalar (Copy) program → every fn has an empty moved-source set.
    assert_eq!(
        borrow_dump(
            "copy",
            "fn add(a: i64, b: i64) -> i64 { a + b }\nfn main() -> i64 { add(1, 2) }\n"
        ),
        "(fn #33 add)\n(fn #34 main)\n"
    );
}

#[test]
fn borrow_dump_struct_move() {
    // `take` reads `p.x` (a field read is NON-consuming → no move); `main` passes
    // `a` (VarId #1, a Move-typed struct) by value into `take` → moved.
    assert_eq!(
        borrow_dump(
            "struct",
            "struct P { x: i64 }\nfn take(p: P) -> i64 { p.x }\nfn main() -> i64 { let a = P { x: 5 }; take(a) }\n"
        ),
        "(fn #33 take)\n(fn #34 main #1)\n"
    );
}

#[test]
fn borrow_dump_array_move() {
    // `take` reads `a[0]` (index read → no move); `main` moves the array `xs` (#1).
    assert_eq!(
        borrow_dump(
            "array",
            "fn take(a: [i64]) -> i64 { a[0] }\nfn main() -> i64 { let xs = [1, 2, 3]; take(xs) }\n"
        ),
        "(fn #33 take)\n(fn #34 main #1)\n"
    );
}

#[test]
fn borrow_dump_conservative_if_branch_merge() {
    // `pick` moves `p` (#1) in the then-branch and `q` (#2) in the else-branch — the
    // analysis is conservative (moved in EITHER branch → included), so both appear.
    // `main`'s struct-lit args are temporaries (not bindings), and `r.x` is a field
    // read → `main` has no moved sources.
    assert_eq!(
        borrow_dump(
            "branch",
            "struct P { x: i64 }\nfn pick(c: bool, p: P, q: P) -> P { if c { p } else { q } }\nfn main() -> i64 { let r = pick(true, P { x: 1 }, P { x: 2 }); r.x }\n"
        ),
        "(fn #33 pick #1 #2)\n(fn #34 main)\n"
    );
}
