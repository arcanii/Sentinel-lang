//! UI tests for the `snc` driver — full-diagnostic snapshots.
//!
//! Each `.sentinel` file under workspace-root `tests/ui/` that the
//! front-end is expected to *reject* is compiled via `snc build`, and
//! snc's complete rendered stderr is compared against an `insta`
//! blessed snapshot. This is the ADR 0025 D11 / HANDOVER §6.4
//! migration of the former ad-hoc `stderr.contains(code)` checks in
//! `pass.rs`: a snapshot pins the entire what/why/how diagnostic, so a
//! regression in the error code, message wording, source span, or help
//! text surfaces in the diff — not just the disappearance of a code.
//!
//! Snapshot stability: `snc` is run with the workspace root as its
//! working directory and a *relative* `tests/ui/<name>` source path,
//! so the diagnostic's path label is the same on every machine. snc
//! emits no ANSI color to a pipe (miette disables color for non-TTY
//! output) and the rejected fixtures fail before codegen, so the
//! output binary path never appears — the raw stderr needs no
//! normalization before snapshotting.
//!
//! The two pure-syntax fixtures (`lex_invalid_char`,
//! `parse_unbalanced_paren`) are snapshotted at the syntax layer in
//! `crates/sentinel-syntax/tests/ui.rs`; this harness covers the
//! resolve / types / effect-check / borrow-check rejections that only
//! the full `snc` pipeline surfaces.

use std::path::PathBuf;
use std::process::Command;

fn workspace_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("crate dir has a parent")
        .parent()
        .expect("crates/ has a parent")
        .to_path_buf()
}

fn snc_binary() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_snc"))
}

/// Compile a `tests/ui/` fixture that the front-end is expected to
/// reject, returning snc's rendered stderr. snc runs with the
/// workspace root as its cwd and a relative source + output path so
/// the captured diagnostic is byte-stable across machines.
fn reject_stderr(fixture: &str) -> String {
    let root = workspace_root();
    std::fs::create_dir_all(root.join("target/sentinel-ui")).expect("create build dir");
    let rel_src = format!("tests/ui/{fixture}");
    let rel_out = format!(
        "target/sentinel-ui/{}",
        PathBuf::from(fixture)
            .with_extension("")
            .display()
    );
    let out = Command::new(snc_binary())
        .current_dir(&root)
        .arg("build")
        .arg(&rel_src)
        .arg("-o")
        .arg(&rel_out)
        .output()
        .expect("snc invocation failed");
    assert!(
        !out.status.success(),
        "expected snc to reject {fixture}, but the build succeeded"
    );
    String::from_utf8(out.stderr).expect("snc stderr is not valid UTF-8")
}

/// Declare a UI snapshot test: compile the fixture, assert rejection,
/// and snapshot the full diagnostic. The snapshot file is
/// `tests/snapshots/ui__<test_name>.snap`.
macro_rules! ui_snapshot {
    ($test_name:ident, $fixture:literal) => {
        #[test]
        fn $test_name() {
            insta::assert_snapshot!(reject_stderr($fixture));
        }
    };
}

// ---- C3.4 handler-surface rejections (resolve + types) ----
ui_snapshot!(c34_perform_undefined_effect, "c34_perform_undefined_effect.sentinel");
ui_snapshot!(c34_handle_undefined_op, "c34_handle_undefined_op.sentinel");
ui_snapshot!(c34_handle_duplicate_arm, "c34_handle_duplicate_arm.sentinel");
ui_snapshot!(c34_kont_used_as_value, "c34_kont_used_as_value.sentinel");

// ---- C3.7 effect-check rejection ----
ui_snapshot!(c37_perform_outside_handle, "c37_perform_outside_handle.sentinel");

// ---- C4.1 class definite-assignment rejection ----
ui_snapshot!(c41_init_field_unassigned, "c41_init_field_unassigned.sentinel");

// ---- C4.2 trait / impl rejections (types + resolve) ----
ui_snapshot!(c42_impl_missing_method, "c42_impl_missing_method.sentinel");
ui_snapshot!(c42_impl_method_sig_mismatch, "c42_impl_method_sig_mismatch.sentinel");
ui_snapshot!(c42_duplicate_default_impl, "c42_duplicate_default_impl.sentinel");
ui_snapshot!(c42_duplicate_impl_name, "c42_duplicate_impl_name.sentinel");

// ---- C4.3 delegation rejections (resolve) ----
ui_snapshot!(c43_delegate_collides_with_impl, "c43_delegate_collides_with_impl.sentinel");
ui_snapshot!(c43_delegate_undefined_trait, "c43_delegate_undefined_trait.sentinel");

// ---- C4.4 structured-concurrency rejections (types) ----
ui_snapshot!(c44_spawn_non_fn_call, "c44_spawn_non_fn_call.sentinel");
ui_snapshot!(c44_await_on_non_task, "c44_await_on_non_task.sentinel");
ui_snapshot!(c44_spawn_result_must_be_i64, "c44_spawn_result_must_be_i64.sentinel");

// ---- C5.2 constant-time verification rejection (the MIR D5 pass) ----
ui_snapshot!(c52_secret_leak, "c52_secret_leak.sentinel");

// ---- D.1 (3/N) sum-type / match rejection (types exhaustiveness) ----
ui_snapshot!(c5d1_non_exhaustive_match, "c5d1_non_exhaustive_match.sentinel");

// ---- D.2 (3/N) u8 mixed-width rejection (types) ----
ui_snapshot!(c5d2_mixed_width, "c5d2_mixed_width.sentinel");

// ---- ADR 0046 partial-move-through-field: use-after-partial-move (borrow) ----
ui_snapshot!(c25_use_after_partial_move, "c25_use_after_partial_move.sentinel");
