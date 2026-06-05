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
