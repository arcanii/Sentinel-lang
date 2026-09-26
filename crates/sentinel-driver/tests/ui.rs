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

// ---- C3.5 / ADR 0072: the effecting-fn continuation boundary, fail-closed ----
// One fixture per way a body can miss the four lowerable shapes. Each of these used
// to be ACCEPTED and mis-lowered rather than refused — two silently (a raw pointer
// returned as the answer; an effect dropped so a handler never ran), one as an
// out-of-bounds stack read, one as an inkwell panic. The diagnostics are the fix.
ui_snapshot!(c35_effecting_call_in_statement, "c35_effecting_call_in_statement.sentinel");
ui_snapshot!(c35_effecting_call_in_operand, "c35_effecting_call_in_operand.sentinel");
ui_snapshot!(c35_effecting_let_nullable, "c35_effecting_let_nullable.sentinel");
ui_snapshot!(c35_effecting_narrow_capture, "c35_effecting_narrow_capture.sentinel");

// ---- C3.7 effect-check rejection ----
ui_snapshot!(c37_perform_outside_handle, "c37_perform_outside_handle.sentinel");

// ---- C1.4 struct literal rejection ----
// Register D96 (ADR 0013 A1): a field named twice is refused (DuplicateField) rather than
// panicking the type checker.
ui_snapshot!(c14_struct_literal_duplicate_field, "c14_struct_literal_duplicate_field.sentinel");

// ---- C4.1 class definite-assignment rejection ----
ui_snapshot!(c41_init_field_unassigned, "c41_init_field_unassigned.sentinel");
// ADR 0022 A3 (register D129): definite assignment is path by path — a field assigned on
// one path only, a field read before it is assigned, `self` used before every field is.
ui_snapshot!(c41_init_field_assigned_on_one_path, "c41_init_field_assigned_on_one_path.sentinel");
ui_snapshot!(c41_init_field_read_before_assign, "c41_init_field_read_before_assign.sentinel");
ui_snapshot!(c41_init_self_used_before_assigned, "c41_init_self_used_before_assigned.sentinel");
// Register D61: method bodies are borrow-checked. Before D61 none of these was rejected.
ui_snapshot!(c41_method_double_move, "c41_method_double_move.sentinel");
ui_snapshot!(c41_method_move_out_of_self, "c41_method_move_out_of_self.sentinel");
ui_snapshot!(c41_method_move_self_whole, "c41_method_move_self_whole.sentinel");
ui_snapshot!(c42_impl_move_out_of_self_struct, "c42_impl_move_out_of_self_struct.sentinel");

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
// ADR 0066: a `spawn` target must be a user/extern fn, not a runtime builtin
// (`print`) — the builtin passes the word-scalar arg/result gates but has no
// spawnable wrapper (its name is not its runtime symbol).
ui_snapshot!(c66_spawn_builtin, "c66_spawn_builtin.sentinel");
// ADR 0066 M1.2b-cont: a `Channel<T>` carries any word-scalar element now; a
// NON-word-scalar element (here `u128`, which doesn't fit the i64 channel slot) is
// still rejected with ChannelElementNotSupported.
ui_snapshot!(c66_channel_element_unsupported, "c66_channel_element_unsupported.sentinel");

// ---- ADR 0015 D11 / ADR 0016 D6b, register D47: the heap-indirected `?T` payload ----
// `unwrap_or` has no load through the box in ANY back end, so the shape used to
// type-check and then die in codegen — invalid IR from both text emitters, and from
// inkwell either a `verify()` failure or an outright PANIC when the result was used
// unbound. These refuse it at the type layer instead.
// ⚠ THREE FILES ON PURPOSE, PINNING TWO INDEPENDENT AXES.
//   * PAYLOAD axis — the gate is one match arm, `Struct(_) | GenericInstance(_)`, so a
//     mutation dropping either disjunct must turn exactly one file red. Hence the
//     `?Struct` and `?GenericInstance` pair, both in the BOUND position.
//   * POSITION axis — before the gate, the two positions failed DIFFERENTLY: bound
//     reached inkwell's `verify()` and produced a diagnostic, while UNBOUND
//     (`unwrap_or(o, d).v`) PANICKED at `enums.rs:333` before verify. The panic is the
//     rule violation this change exists to remove, and the bound fixtures do not reach
//     it. Hence the third file.
// Do not fold them: each covers a mutation the others leave green.
ui_snapshot!(c15_unwrap_or_heap_payload, "c15_unwrap_or_heap_payload.sentinel");
ui_snapshot!(
    c15_unwrap_or_heap_payload_unbound,
    "c15_unwrap_or_heap_payload_unbound.sentinel"
);
ui_snapshot!(
    c17_unwrap_or_heap_payload_generic,
    "c17_unwrap_or_heap_payload_generic.sentinel"
);
// ADR 0066 M2.2 / D8: the cross-process secret fence — a `[secret u8]` payload
// cannot cross the public `[u8]` byte-pipe boundary (`process_write`), so the
// program is rejected as a type mismatch (the type system IS the fence).
ui_snapshot!(c66_process_secret_fence, "c66_process_secret_fence.sentinel");
// ADR 0066 M2.3 / D8: the typed framed-channel fence — a `secret i64` cannot cross
// the public `i64` framed-channel boundary (`process_send`), rejected as a type
// mismatch (the type system IS the fence, exactly as M2.2's `[u8]` byte-pipe).
ui_snapshot!(c66_process_channel_secret_fence, "c66_process_channel_secret_fence.sentinel");
// ADR 0066 M2.3b / D8: the other half of the cross-process element fence — a
// process-local HANDLE may not cross a pipe either. Pins `is_process_channel_elem`
// as an explicit list; M1.2c showed it could otherwise be widened from a distance.
ui_snapshot!(c66_process_channel_handle_fence, "c66_process_channel_handle_fence.sentinel");
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
// ADR 0071 D2 amendment A1: a `Shared` / `Mutex` handle cannot be `secret`-qualified —
// the secret belongs inside the container (`Shared<secret T>`, D6). SecretHandle, at a
// `let` annotation and at a parameter's.
ui_snapshot!(c71_secret_handle, "c71_secret_handle.sentinel");
ui_snapshot!(c71_secret_mutex_handle, "c71_secret_mutex_handle.sentinel");
// ADR 0071 M1.4c (D6): a secret read out of a `Mutex<secret T>` is STILL secret for
// every downstream check — branching on `*g` is rejected exactly as any other secret
// branch is. This is the fixture that proves the secret container did not become a
// laundering hole: if it ever compiles, constant-time is broken.
ui_snapshot!(c71_secret_mutex_branch, "c71_secret_mutex_branch.sentinel");
// ADR 0071 M1.4c / register D17: the `Shared<T>` element-domain fence — the twin of
// `c66_channel_element_unsupported`. The element rides the C-ABI as one i64 slot, so
// the domain is the word scalars {i64,i32,u8,bool,f64,ptr} plus the four `secret` forms
// that exist (`secret ptr` hits this same fence, `secret f64` is refused outright); a
// nested container is in neither list. ⚠ This pins only the NON-WORD-SCALAR half of the
// boundary — see the fixture's own header, which refutes at length the claim that used
// to stand here, that the fence makes the width mirror's fall-through arm unreachable.
// It does not: `shared_id_for` admits f64 and ptr, and `shared_new(x as f64)` reaches
// that arm and diverges (register D30). Read the fixture before extending this.
ui_snapshot!(
    c71_shared_element_unsupported,
    "c71_shared_element_unsupported.sentinel"
);

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
// Register D60: `return` is refused inside a class `init`.
ui_snapshot!(c65_return_in_init, "c65_return_in_init.sentinel");
// Register D59: secrets reaching `&&` through the joins D59 changed — a node with a guarded
// `return` inside it, and matches typed by their first live arm. The constant-time pass
// refuses each; the ctverify differential holds the self-hosted verifier to the same
// verdicts, some of which it used to miss.
ui_snapshot!(c65_secret_join_guarded, "c65_secret_join_guarded.sentinel");
// Register D69: an effecting fn whose tail evaluates a `return` before its `perform` is
// refused, not built to perform anyway. The rule's other refusals are in
// `tests/embedded_perform.rs`.
ui_snapshot!(c65_return_before_perform, "c65_return_before_perform.sentinel");

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

// ADR 0017 D7 (ref-escape): references are second-class, so they cannot be
// stored in an aggregate, and a reference to a reference is not a type. These
// pin the positions where the rules keyed on `is_ref()` and so saw nothing.
ui_snapshot!(c21_nested_nullable_ref, "c21_nested_nullable_ref.sentinel");
ui_snapshot!(c21_nested_ref_inferred, "c21_nested_ref_inferred.sentinel");
ui_snapshot!(c21_nullable_ref_struct_field, "c21_nullable_ref_struct_field.sentinel");
ui_snapshot!(c21_ref_in_class_field, "c21_ref_in_class_field.sentinel");
ui_snapshot!(c21_ref_in_class_field_delegate, "c21_ref_in_class_field_delegate.sentinel");
ui_snapshot!(c21_ref_in_enum_payload, "c21_ref_in_enum_payload.sentinel");
ui_snapshot!(c21_ref_in_generic_field_call, "c21_ref_in_generic_field_call.sentinel");
ui_snapshot!(c21_ref_in_generic_field_literal, "c21_ref_in_generic_field_literal.sentinel");

// ---- ADR 0017 D7 (ref-escape), borrow layer. A reference may not outlive the
// storage it points at. Each family below pins one POSITION where a dead source
// can be caught, and each position has its own diagnostic code so the `help`
// line can name the fix that applies there. ----

// Exit positions (-> returns_local_ref): a reference to a fn-local escaping the
// function, from every exit shape the language has.
ui_snapshot!(c21_return_local_ref_stmt, "c21_return_local_ref_stmt.sentinel");
ui_snapshot!(c21_match_tail_local_ref, "c21_match_tail_local_ref.sentinel");
ui_snapshot!(c21_scope_tail_local_ref, "c21_scope_tail_local_ref.sentinel");
ui_snapshot!(c21_method_call_tail_local_recv, "c21_method_call_tail_local_recv.sentinel");
ui_snapshot!(c21_qualified_call_tail, "c21_qualified_call_tail.sentinel");
ui_snapshot!(c21_nullable_ref_return_local, "c21_nullable_ref_return_local.sentinel");
ui_snapshot!(c21_secret_ref_return_local, "c21_secret_ref_return_local.sentinel");

// Binding / assignment positions (-> ref_outlives_binding, ref_outlives_assignment):
// a binding may not start out, or be re-pointed at, storage that dies before it.
ui_snapshot!(c21_match_launder_outlives, "c21_match_launder_outlives.sentinel");
ui_snapshot!(c21_if_merge_dead_branch, "c21_if_merge_dead_branch.sentinel");
ui_snapshot!(c21_assign_widens_ref, "c21_assign_widens_ref.sentinel");
ui_snapshot!(c21_nullable_ref_local_outlives, "c21_nullable_ref_local_outlives.sentinel");
ui_snapshot!(c21_borrow_of_temporary, "c21_borrow_of_temporary.sentinel");

// Operand position (-> ref_operand_dead): a ref-carrying value consumed as a call
// argument or a deref operand is bound to nothing, so no other check sees it.
ui_snapshot!(c21_call_arg_computed_ref, "c21_call_arg_computed_ref.sentinel");
ui_snapshot!(c21_method_arg_computed_ref, "c21_method_arg_computed_ref.sentinel");
ui_snapshot!(c21_deref_computed_ref, "c21_deref_computed_ref.sentinel");

// Moving out from under a live reference (-> move_while_borrowed).
ui_snapshot!(c23_move_while_borrowed, "c23_move_while_borrowed.sentinel");
ui_snapshot!(c23_move_while_borrowed_merge, "c23_move_while_borrowed_merge.sentinel");

// Borrows that outlive an inner block now keep their place borrowed for the
// binding's life, so the shared-XOR-mutable rule sees them.
ui_snapshot!(c22_borrow_survives_inner_block, "c22_borrow_survives_inner_block.sentinel");
ui_snapshot!(c22_ref_out_of_block_then_push, "c22_ref_out_of_block_then_push.sentinel");
ui_snapshot!(c22_method_ref_then_mut_method, "c22_method_ref_then_mut_method.sentinel");

// The FFI fence from the reference side: an `extern "C"` body is never
// borrow-checked by any run, and what keeps the interprocedural summary sound
// at that boundary is that no reference can cross it.
ui_snapshot!(c21_extern_ref_param, "c21_extern_ref_param.sentinel");

// ADR 0073 D1: an effect operation's parameters and result cross the `Kont*`
// seam, which carries one `i64`. Before this rule the four reference spellings
// reached codegen and aborted inkwell on a perfectly live reference.
ui_snapshot!(c21_ref_in_effect_op_param, "c21_ref_in_effect_op_param.sentinel");
ui_snapshot!(c21_ref_in_effect_op_return, "c21_ref_in_effect_op_return.sentinel");

// ADR 0075 D3 (register D87): a handler arm is a loop body — the dispatch loop
// re-enters it per performed operation — so ADR 0036 D8's loop-carried move rule
// applies to it. This is what makes ADR 0075 D1's per-entry drain of the arm's
// scopes sound.
ui_snapshot!(c75_move_into_handler_arm, "c75_move_into_handler_arm.sentinel");

// ADR 0075 A1 (register D99): refused by `snc build`, and its real job is the corpus
// differential -- the text back ends number an effecting fn's arm-remainder resumers in
// one sequence across its frames and emit them after its last frame, and `scg` is held
// to the oracle's bytes on this file.
ui_snapshot!(
    c75_effecting_frames_share_resumers,
    "c75_effecting_frames_share_resumers.sentinel"
);

// ADR 0036 D8 / A5: the loop-carried move rule, for a whole outer binding (the UI fixture
// D10 asked for) and for a FIELD moved out of one, which the rule flags by its root.
ui_snapshot!(c5d5_move_outer_in_loop, "c5d5_move_outer_in_loop.sentinel");
ui_snapshot!(c5d5_move_outer_field_in_loop, "c5d5_move_outer_field_in_loop.sentinel");

// ADR 0034 C1 (register D125): `vec_to_array` copies without moving, so it is refused for an
// element that owns memory (VecToArrayElementNotPlain).
ui_snapshot!(c5d3_vec_to_array_element_not_plain, "c5d3_vec_to_array_element_not_plain.sentinel");

// ADR 0050 A6: an element store needs a collection that still owns its buffer.
ui_snapshot!(c55_index_assign_after_move, "c55_index_assign_after_move.sentinel");

// ADR 0075 A2: a `return` arm's moves reach an op arm's code after a resume.
ui_snapshot!(
    c75_return_arm_move_read_after_resume,
    "c75_return_arm_move_read_after_resume.sentinel"
);

// ADR 0046 A4: the moves the partial-move state cannot represent are refused, and a
// payload moved out of a `match` moves out of its scrutinee.
ui_snapshot!(c25_move_out_of_borrow, "c25_move_out_of_borrow.sentinel");
ui_snapshot!(c25_move_out_of_nested_field, "c25_move_out_of_nested_field.sentinel");
ui_snapshot!(c25_move_out_of_element, "c25_move_out_of_element.sentinel");
ui_snapshot!(c25_match_payload_moved_twice, "c25_match_payload_moved_twice.sentinel");
// ... including under a deref of a computed value in a comparison or a discarded statement;
// after the scrutinee was consumed; after a reassignment on some paths; after a merge with a
// path that consumed the scrutinee; in a method's own arguments; and while a payload binding
// is borrowed.
ui_snapshot!(c25_move_under_a_computed_deref, "c25_move_under_a_computed_deref.sentinel");
ui_snapshot!(
    c25_payload_used_after_scrutinee_moved,
    "c25_payload_used_after_scrutinee_moved.sentinel"
);
ui_snapshot!(
    c25_payload_after_conditional_reassign,
    "c25_payload_after_conditional_reassign.sentinel"
);
ui_snapshot!(c25_payload_moved_after_a_merge, "c25_payload_moved_after_a_merge.sentinel");
ui_snapshot!(
    c25_method_arg_moves_receiver_field,
    "c25_method_arg_moves_receiver_field.sentinel"
);
ui_snapshot!(
    c25_borrowed_payload_scrutinee_moved,
    "c25_borrowed_payload_scrutinee_moved.sentinel"
);

// Register D111: a move inside nested loop-like constructs is reported once.
ui_snapshot!(
    c75_return_arm_in_loop_reports_once,
    "c75_return_arm_in_loop_reports_once.sentinel"
);
