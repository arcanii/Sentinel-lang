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
ui_snapshot!(c43_delegate_same_method_ambiguous, "c43_delegate_same_method_ambiguous.sentinel");

// ---- C4.4 structured-concurrency rejections (types) ----
ui_snapshot!(c44_spawn_non_fn_call, "c44_spawn_non_fn_call.sentinel");
ui_snapshot!(c44_await_on_non_task, "c44_await_on_non_task.sentinel");
// ADR 0066 M1.1: word-sized scalar spawn results are now accepted; an
// aggregate (here `u128`) is the deferred case that's still rejected.
ui_snapshot!(c66_spawn_aggregate_unsupported, "c66_spawn_aggregate_unsupported.sentinel");
// ADR 0066 M1.2b-cont: a `Channel<T>` carries any word-scalar element now; a
// NON-word-scalar element (here `u128`, which doesn't fit the i64 channel slot) is
// still rejected with ChannelElementNotSupported.
ui_snapshot!(c66_channel_element_unsupported, "c66_channel_element_unsupported.sentinel");
// ADR 0066 M2.2 / D8: the cross-process secret fence — a `[secret u8]` payload
// cannot cross the public `[u8]` byte-pipe boundary (`process_write`), so the
// program is rejected as a type mismatch (the type system IS the fence).
ui_snapshot!(c66_process_secret_fence, "c66_process_secret_fence.sentinel");
// ADR 0066 M2.3 / D8: the typed framed-channel fence — a `secret i64` cannot cross
// the public `i64` framed-channel boundary (`process_send`), rejected as a type
// mismatch (the type system IS the fence, exactly as M2.2's `[u8]` byte-pipe).
ui_snapshot!(c66_process_channel_secret_fence, "c66_process_channel_secret_fence.sentinel");
// ADR 0066 M2.4a / ADR 0069 D1: a `SealedChannel<T>` with a NON-secret element is a
// type error (the fence-as-type — a SealedChannel carries an encrypted secret, so a
// public element is pointless). Surfaces SealedChannelElementNotSupported.
ui_snapshot!(c66_sealed_channel_public_element, "c66_sealed_channel_public_element.sentinel");
// ADR 0071 M1.4a slice 3: returning a NAMED `Shared<T>` binding (a bare Var in tail
// or `return` position) transfers a refcount unit — guarded until the slice-3b
// transfer exemption is mirrored into the oracle + scg. Surfaces
// SharedReturnNotSupported (return `shared_new(...)` directly instead).
ui_snapshot!(c71_shared_return_named, "c71_shared_return_named.sentinel");
// ADR 0071 M1.4b slice 3a: returning a NAMED `Mutex<T>` binding — guarded like the
// Shared case. Surfaces MutexReturnNotSupported (return `mutex_new(...)` directly).
ui_snapshot!(c71_mutex_return_named, "c71_mutex_return_named.sentinel");
// ADR 0071 M1.4b slice 3b: the guard no-escape conservative pin — a `lock()` must be
// the direct RHS of an immutable `let`. A `lock()` in an argument position (here
// `is_some(lock(m))`) is rejected with GuardNotLetBound so the unlock-on-drop guard
// cannot outlive its mutex.
ui_snapshot!(c71_guard_not_let_bound, "c71_guard_not_let_bound.sentinel");
// ADR 0071 M1.4b slice 3c: taking a reference through a lock guard (`& *g`) is
// rejected (GuardBorrowNotAllowed) — the ref would alias the mutex slot and could
// escape the guard's unlock + the cell's release into a use-after-free.
ui_snapshot!(c71_guard_no_borrow, "c71_guard_no_borrow.sentinel");
// ADR 0071 M1.4b slice 3c: a computed guard deref (`*{ g }`) is rejected
// (GuardDerefNotVar) — only the directly `let`-bound guard Var may be `*g`-derefed
// (a computed operand would consume the guard, skipping its unlock-on-drop).
ui_snapshot!(c71_guard_deref_computed, "c71_guard_deref_computed.sentinel");
// ADR 0071 M1.4c (D6): a secret read out of a `Mutex<secret T>` is STILL secret for
// every downstream check — branching on `*g` is rejected exactly as any other secret
// branch is. This is the fixture that proves the secret container did not become a
// laundering hole: if it ever compiles, constant-time is broken.
ui_snapshot!(c71_secret_mutex_branch, "c71_secret_mutex_branch.sentinel");

// ---- ADR 0070: non-capturing function values (types) ----
// D4: a wrong-arity fn is not eligible as a Fn<T,R> value (any word-scalar shape).
ui_snapshot!(c70_fn_value_ineligible, "c70_fn_value_ineligible.sentinel");
// D1 (generalized): `Fn<T, R>` requires word-scalar T/R — u128 doesn't fit.
ui_snapshot!(c70_fn_type_args_unsupported, "c70_fn_type_args_unsupported.sentinel");
// D3-revisit: direct `f(x)` call syntax on a bound local var, generalized
// beyond the `apply` builtin — a var that's neither Kont nor Fn is rejected
// (CalleeNotCallable), and a Fn value called with the wrong arity/arg type
// is rejected (FnValueArityMismatch / FnValueArgMismatch).
ui_snapshot!(c70_callee_not_callable, "c70_callee_not_callable.sentinel");
ui_snapshot!(c70_fn_value_arity_mismatch, "c70_fn_value_arity_mismatch.sentinel");
ui_snapshot!(c70_fn_value_arg_mismatch, "c70_fn_value_arg_mismatch.sentinel");

// ---- C5.2 constant-time verification rejection (the MIR D5 pass) ----
ui_snapshot!(c52_secret_leak, "c52_secret_leak.sentinel");

// ---- D.1 (3/N) sum-type / match rejection (types exhaustiveness) ----
ui_snapshot!(c5d1_non_exhaustive_match, "c5d1_non_exhaustive_match.sentinel");

// ---- D.2 (3/N) u8 mixed-width rejection (types) ----
ui_snapshot!(c5d2_mixed_width, "c5d2_mixed_width.sentinel");

// ---- ADR 0065 early-return type mismatch (types) ----
ui_snapshot!(c65_return_type_mismatch, "c65_return_type_mismatch.sentinel");

// ---- ADR 0020 D9 / ADR 0065 stage 3a: a handle body that performs through
// control flow (an if/match branch) is rejected, not silently miscompiled (codegen) ----
ui_snapshot!(
    c65_handle_perform_in_control_flow,
    "c65_handle_perform_in_control_flow.sentinel"
);

// ---- ADR 0046 partial-move-through-field: use-after-partial-move (borrow) ----
ui_snapshot!(c25_use_after_partial_move, "c25_use_after_partial_move.sentinel");

// ---- Secret-flow conformance suite (review F3 / P2.2): a secret routed
// through each construct into each sink must still be rejected. The first four
// prove taint SURVIVES the conservative `Opaque`/`Call` funnels (caught by the
// MIR D5 pass); the rest are the type checker's source-level rejections of the
// direct sinks — `if` and (since D.5 loops) `while` conditions, secret array
// index, and secret divisor. The accept side is
// tests/pass/c52_secret_through_constructs_ok.
ui_snapshot!(c52_secret_via_call, "c52_secret_via_call.sentinel");
ui_snapshot!(c52_secret_via_field, "c52_secret_via_field.sentinel");
ui_snapshot!(c52_secret_via_match, "c52_secret_via_match.sentinel");
ui_snapshot!(c52_secret_or_leak, "c52_secret_or_leak.sentinel");
ui_snapshot!(c52_secret_in_if, "c52_secret_in_if.sentinel");
ui_snapshot!(c52_secret_in_while, "c52_secret_in_while.sentinel");
ui_snapshot!(c52_secret_array_index, "c52_secret_array_index.sentinel");
ui_snapshot!(c52_secret_divisor, "c52_secret_divisor.sentinel");

// ---- Fuzzer-found parse robustness (review F11 / P2.5): a trailing `pub`
// with no item used to panic (`unreachable!`); now a clean unexpected_eof. ----
ui_snapshot!(parse_trailing_pub, "parse_trailing_pub.sentinel");
