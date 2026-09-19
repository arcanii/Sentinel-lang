//! ADR 0074 (register D79): a handler arm owns its continuation until it resumes it.
//!
//! **Every exit releases (D2).** The five programs under
//! `tests/fixtures/handler_arm_exits/` leave an arm by each route — the
//! fall-through after declining to resume, a `return`, a `break` / `continue` to a
//! loop around the `handle`, an exit taken inside a `k(v)` argument (D1), and an
//! exit from an inner arm that leaves an outer one too. The exit codes here are the
//! cheap half: the release is visible only in the IR, which is pinned twice —
//! inkwell's by the `adr0074_*` unit tests in `sentinel-codegen`, and the text
//! oracle's by `tests/llvm.rs`, with `scg` held to it byte-for-byte because the same
//! five files are seeds of the codegen differential (`tests/selfhost_codegen.rs`,
//! which also builds and runs the oracle's IR of each). They are not in `tests/pass`:
//! an `if` inside a handler arm makes the two MIR lowerers diverge (register D83,
//! pre-existing), and every `tests/pass` file is swept by the MIR differential.
//!
//! **A second resume aborts (D3).** ADR 0020 D2's one-shot check is made on the
//! arm's continuation slot, which a `k(v)` clears before it resumes, so the second
//! resume reaches `sentinel_kont_resume` with `null` and the runtime refuses it.
//! ADR 0020 D2's `consumed` flag lives inside the kont, which the first resume frees,
//! so it cannot be what decides. Three shapes, one per way a second resume can be
//! written: in the same expression (`k(1) + k(2)`), nested in the first one's
//! argument (`k(k(1))` — D1 evaluates the argument before reading the slot, so the
//! INNER call resumes and the outer one is refused), and on a later iteration of a
//! loop. None of the three can be a `tests/pass` fixture, because each aborts.
//!
//! ⚠ These three pin the observable abort, NOT the slot clear that produces it.
//! Measured by mutation (2026-09-19, `snc build` on Windows): with the clear removed
//! they still passed, so they cannot be what pins it. The clear is pinned by
//! `adr0074_resume_clears_the_arm_slot_after_its_argument` in `sentinel-codegen` and
//! by the exit-value tests below, all of which failed under that mutation. What the
//! three do pin is the runtime's `null` refusal (D3): with it removed, the second
//! resume aborts without the diagnostic.
//!
//! Like `examples.rs` and `ref_receiver.rs`, the builds link (needs the host link
//! toolchain).

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

fn temp_dir() -> PathBuf {
    let dir = std::env::temp_dir().join(format!("snc_arm_exits_{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("create temp dir");
    dir
}

/// Build the Sentinel source at `entry` into `dir` and run it.
fn build_and_run(entry: &Path, dir: &Path, stem: &str) -> Output {
    let exe = dir.join(format!("{stem}{}", std::env::consts::EXE_SUFFIX));
    let res = Command::new(env!("CARGO_BIN_EXE_snc"))
        .arg("build")
        .arg(entry)
        .arg("-o")
        .arg(&exe)
        .output()
        .expect("run snc build");
    assert!(
        res.status.success(),
        "build of {stem} failed; stderr:\n{}",
        String::from_utf8_lossy(&res.stderr),
    );
    Command::new(&exe).output().expect("run the built program")
}

/// Build and run `tests/fixtures/handler_arm_exits/<stem>.sentinel`; its exit code.
fn run_fixture(stem: &str) -> i32 {
    let entry = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/handler_arm_exits")
        .join(format!("{stem}.sentinel"));
    let dir = temp_dir().join(stem);
    std::fs::create_dir_all(&dir).expect("create case dir");
    let run = build_and_run(&entry, &dir, stem);
    run.status.code().expect("program exited with a code")
}

#[test]
fn an_arm_that_declines_to_resume() {
    // A direct `perform`, a conditional resume, and an effecting fn whose pending
    // frame the arm drops by declining: 5 + 7 + 30 + 1 + 100 = 143.
    assert_eq!(run_fixture("c74_arm_declines_to_resume"), 143);
}

#[test]
fn a_return_before_and_after_the_resume() {
    // Before the arm resumes, the slot still owns the kont; after, it is clear and
    // the return releases nothing: 1 + 7 + 5 + 21 = 34.
    assert_eq!(run_fixture("c74_arm_return_leaves_the_arm"), 34);
}

#[test]
fn a_break_or_continue_out_of_an_arm() {
    // Both leave the arm for a loop around the `handle`; a loop INSIDE the arm is
    // the control, whose `break` releases nothing: 6 + 5 = 11.
    assert_eq!(run_fixture("c74_arm_break_continue"), 11);
}

#[test]
fn an_exit_that_leaves_two_arms() {
    // A `handle` inside another handle's arm, its value discarded: a `return` from
    // the inner arm, or a `break` / `continue` to the loop around both, leaves both:
    // 1 + 9 + 3 + 3 = 16. The exit code cannot see the releases; they are pinned in
    // the IR (`adr0074_a_return_from_an_inner_arm_releases_both_open_arms` and
    // `adr0074_a_break_or_continue_out_of_two_arms_releases_both` for inkwell,
    // `tests/llvm.rs` for the oracle).
    assert_eq!(run_fixture("c74_two_open_arms"), 16);
}

#[test]
fn an_exit_taken_inside_a_resume_argument() {
    // `k(<argument that returns or breaks>)`: the argument runs before the slot is
    // read, so the exit it takes still finds the kont owned: 1 + 9 + 3 = 13.
    assert_eq!(run_fixture("c74_resume_arg_leaves_the_arm"), 13);
}

const DIAGNOSTIC: &str = "continuation already resumed (one-shot per ADR 0020 D2)";

fn assert_one_shot_abort(src: &str, stem: &str) {
    let dir = temp_dir().join(stem);
    std::fs::create_dir_all(&dir).expect("create case dir");
    let entry = dir.join(format!("{stem}.sentinel"));
    std::fs::write(&entry, src).expect("write source");
    let run = build_and_run(&entry, &dir, stem);
    let stderr = String::from_utf8_lossy(&run.stderr);
    assert!(!run.status.success(), "{stem}: a second resume must not complete; stderr:\n{stderr}");
    assert!(
        stderr.contains(DIAGNOSTIC),
        "{stem}: expected the one-shot diagnostic, got status {:?} and stderr:\n{stderr}",
        run.status
    );
}

#[test]
fn a_second_resume_in_the_same_expression_aborts() {
    assert_one_shot_abort(
        "effect Io { read() -> i64; }\n\
         fn main() -> i64 { handle perform Io.read() with { Io.read(k) => k(1) + k(2) } }\n",
        "same_expr",
    );
}

#[test]
fn a_resume_nested_in_its_own_argument_aborts() {
    assert_one_shot_abort(
        "effect Io { read() -> i64; }\n\
         fn main() -> i64 { handle perform Io.read() with { Io.read(k) => k(k(1)) } }\n",
        "nested_arg",
    );
}

#[test]
fn a_resume_on_a_later_loop_iteration_aborts() {
    assert_one_shot_abort(
        "effect Io { read() -> i64; }\n\
         fn main() -> i64 {\n\
             handle perform Io.read() with {\n\
                 Io.read(k) => {\n\
                     let mut i: i64 = 0;\n\
                     let mut s: i64 = 0;\n\
                     while i < 2 { s = s + k(i); i = i + 1; }\n\
                     s\n\
                 }\n\
             }\n\
         }\n",
        "loop",
    );
}
