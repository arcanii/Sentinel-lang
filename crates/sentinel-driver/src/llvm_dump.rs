//! Phase D self-host port (8/N) / ADR 0045 D2+D3: the codegen differential
//! oracle — a **canonical textual LLVM IR (`.ll`)** emitter that the
//! Sentinel-written codegen stage (`selfhost/codegen.sentinel`, via
//! `types::run` mode 4) will reproduce **byte-for-byte**, AND that
//! `clang`/`llc` lowers to a runnable object whose behaviour matches the
//! fixture (the behavioural half of the oracle).
//!
//! This is a SECOND backend alongside the inkwell `compile_to_object` — a
//! deliberately simple, deterministic `.ll` text we control (NOT inkwell's
//! `print_to_string()`, which is brittle to match by hand). The inkwell
//! backend stays `snc`'s production path; this textual one is the port's
//! byte-target + the readable IR.
//!
//! **The canonical `.ll` spec (8a = the straight-line subset):**
//! - A module preamble: `target triple = "<triple>"` (hardcoded for a
//!   reproducible byte-target).
//! - One `define` per top-level user fn, in **FnId order** (= source order —
//!   the Sentinel side assigns FnIds source-sequentially). `main` returns
//!   `i32` (the C-ABI entry; its `i64` body is truncated); other fns return
//!   their declared type.
//! - **No phi.** Mirroring the inkwell backend, every param + `let` binding
//!   is an `alloca` slot; reads `load` and writes `store`. So the only SSA
//!   numbering is for instruction-result temporaries — a single per-fn
//!   counter `%vN` (params arrive as `%argN`). Branch merges (8b) will go
//!   through memory cells, never phi.
//!
//! Emission is **partial + total-by-Err**: a construct not yet ported
//! returns `Err`, so `run_llvm` exits nonzero and the corpus differential
//! skips that fixture (exactly as it skips upstream parse/resolve/type
//! rejects). As the subset grows (8b..8l) fewer fixtures Err. The straight-
//! line subset (8a): const / bool / char / var / unary neg+not / binary
//! (add/sub/mul/sdiv/udiv/and/or/xor) / cmp (signed+unsigned icmp) / `let` /
//! assign-to-var / nested value-block / user-fn call (+ the `u8`↔`i64`
//! width builtins). Everything else → `Err` (deferred to a later slice).

use std::collections::{BTreeSet, HashMap};
use std::fmt::Write;

use sentinel_ast::{BinOp, CmpOp, LogicOp, UnaryOp};
use sentinel_borrow_check::DropPlan;
// Bar B / generics: reuse the inkwell backend's monomorphic-instance discovery so the
// oracle monomorphizes the same set, in the same order, as the production codegen.
use sentinel_codegen::collect_mono_instantiations;
use sentinel_resolve::{
    ClassId, EffectId, EnumId, FnId, StructId, VarId, ARG_COUNT_FN_ID, ARG_FN_ID, I64_TO_U8_FN_ID,
    IS_SOME_FN_ID, LEN_FN_ID,
    CHANNEL_CLOSE_FN_ID, CHANNEL_NEW_FN_ID, POP_FN_ID, PRINT_BYTES_FN_ID, PRINT_FN_ID,
    PROCESS_READ_FN_ID, PROCESS_RECV_FN_ID, PROCESS_SEND_FN_ID, PROCESS_SPAWN_FN_ID,
    PROCESS_WAIT_FN_ID, PROCESS_WRITE_FN_ID, PUSH_FN_ID, SEALED_CHANNEL_FN_ID, SEALED_PROCESS_FN_ID,
    STDIN_RECV_FN_ID, STDOUT_SEND_FN_ID,
    READ_FILE_FN_ID, RECV_FN_ID, SEND_FN_ID, STR_EQ_FN_ID, U8_TO_I64_FN_ID, UNWRAP_OR_FN_ID,
    VEC_NEW_FN_ID, VEC_TO_ARRAY_FN_ID, WRITE_FILE_FN_ID,
};
use sentinel_types::{
    NullableInner, Type, TypedBlock, TypedExpr, TypedExprKind, TypedFnDef, TypedFnSignature,
    TypedHandlerArm, TypedMatchArm, TypedParam, TypedPattern, TypedPatternBinding, TypedProgram,
    TypedReturnArm, TypedStmt, TypedStmtKind,
};

/// Hardcoded for a reproducible byte-target (not host inference). `clang`
/// emits a cosmetic `-Woverride-module` note if its host triple differs;
/// the object still links + runs (probe-validated, ADR 0045).
const TARGET_TRIPLE: &str = "arm64-apple-darwin";

/// The lowest user FnId — ids 0..=13 are runtime/builtins (ADR 0044 FnId map).
// ADR 0056 added the socket builtins (14..=20); ADR 0066 M1.2 added the
// channel builtins (21..=24); ADR 0066 M2.1 added subprocess spawn/wait
// (25..=26); M2.2 added subprocess write/read (27..=28); M2.3 added subprocess
// send/recv (29..=30); M2.4a added the SealedChannel bridge builtins (31..=32);
// M2.4b added the self-stdin/stdout framed builtins (33..=34); the M2.4 follow-on
// added arg_count/arg (35..=36); ADR 0070 added the `apply` indirect-call builtin
// (37), shifting the user-fn base to 38. A call to a builtin FnId not caught by a
// special lowering arm above returns Err (the fixture is skipped); the channel +
// subprocess + sealed-bridge + self-stdin/stdout + arg builtins ARE specially
// lowered — `apply` deliberately is NOT (ADR 0070 snc-only; it falls through to
// this same Err fallback, like an unported builtin).
const FIRST_USER_FN: u32 = 38;

/// Bar B / effects (ADR 0020): the reserved kont op_id for a PURE_RETURN wrap
/// (`u32::MAX`) — the handle dispatch + `k(v)` pure-check compare against it.
const PURE_RETURN_OP_ID: u32 = u32::MAX;

/// Bar B / effects: a unique 32-bit op id per `(EffectId, op_index)`, mirroring the
/// runtime/inkwell `encode_op_id` (`(eid << 16) | (op & 0xFFFF)`).
fn encode_op_id(effect_id: EffectId, op_index: usize) -> u32 {
    (effect_id.0 << 16) | ((op_index as u32) & 0xFFFF)
}

/// Which `sentinel_*` runtime symbols a module's bodies actually use. The module
/// emits a `declare` for each used symbol — in a FIXED order, before the fns — so a
/// program using none stays byte-identical to the slice that added the first
/// (8c-1). Each flag is set as the corresponding call is emitted.
#[derive(Default, Clone, Copy)]
struct RuntimeSyms {
    alloc: bool,
    realloc: bool,
    /// `sentinel_free(ptr) -> void` (C2.4) — scope-exit heap drops (8d-drops).
    /// Pairs with alloc/realloc, so declares right after them.
    free: bool,
    panic_oob: bool,
    str_eq: bool,
    read_file: bool,
    write_file: bool,
    /// `sentinel_print(i64) -> i64` (Bar B): the `print` builtin (FnId 0).
    print: bool,
    print_bytes: bool,
    /// The `llvm.memcpy` intrinsic (8d-Vec-2: `vec_to_array` copies the live
    /// prefix). An `@llvm.*` intrinsic, not a `sentinel_*` symbol, so it
    /// declares LAST — after the runtime-symbol group.
    memcpy: bool,
    /// Bar B / effects (ADR 0020): the handler-runtime kont symbols.
    /// `sentinel_perform_op(i32 op_id, i64 arg) -> ptr` (a fresh kont).
    perform_op: bool,
    /// `sentinel_kont_resume(ptr kont, i64 value) -> ptr` (a result kont).
    kont_resume: bool,
    /// `sentinel_kont_consume_pure(ptr kont) -> i64` (unwrap a PURE_RETURN kont).
    kont_consume_pure: bool,
    /// Bar B / effects (c35b): `sentinel_kont_pure(i64 value) -> ptr` — wrap a pure
    /// i64 as a PURE_RETURN-tagged kont (an effecting fn / handle body whose value
    /// never performed; the caller's dispatch then matches the PURE_RETURN case).
    kont_pure: bool,
    /// Bar B / effects (c35c): `sentinel_kont_push(ptr kont, ptr resumer, ptr captured)
    /// -> void` — push a captured evaluation frame (a let-body resumer + its captured
    /// state) onto a kont's chain, so `sentinel_kont_resume` replays the let's tail.
    kont_push: bool,
    /// Bar B / concurrency (ADR 0024): the structured-concurrency runtime.
    /// `sentinel_task_spawn(wrapper, args, args_size) -> *Task`.
    task_spawn: bool,
    /// `sentinel_task_await(task) -> i64`.
    task_await: bool,
    /// `sentinel_scope_enter() -> *ScopeCtx`.
    scope_enter: bool,
    /// `sentinel_scope_register(scope, task) -> void`.
    scope_register: bool,
    /// `sentinel_scope_exit(scope) -> void`.
    scope_exit: bool,
    /// ADR 0066 M1.2: the channel runtime symbols. `sentinel_channel_new() -> ptr`,
    /// `_send(ptr, i64) -> i64`, `_recv(ptr, ptr) -> i64`, `_close(ptr) -> i64`.
    channel_new: bool,
    channel_send: bool,
    channel_recv: bool,
    channel_close: bool,
    /// ADR 0066 M2.1: the subprocess runtime symbols.
    /// `sentinel_process_spawn(ptr, i64, ptr, i64) -> ptr`, `_wait(ptr) -> i64`.
    process_spawn: bool,
    process_wait: bool,
    /// ADR 0066 M2.2: the byte-pipe IPC runtime symbols.
    /// `sentinel_process_write(ptr, ptr, i64) -> i64`, `_read(ptr, ptr) -> ptr`.
    process_write: bool,
    process_read: bool,
    /// ADR 0066 M2.3: the typed framed-channel-over-pipe runtime symbols.
    /// `sentinel_process_send(ptr, i64) -> i64`, `_recv(ptr, ptr) -> i64`.
    process_send: bool,
    process_recv: bool,
    /// ADR 0066 M2.4b: the child-side self-stdin/stdout framed symbols.
    /// `sentinel_stdin_recv(ptr) -> i64`, `sentinel_stdout_send(i64) -> i64`.
    stdin_recv: bool,
    stdout_send: bool,
    /// ADR 0066 M2.4 follow-on: own command-line argument reflection.
    /// `sentinel_arg_count() -> i64`, `sentinel_arg(i64, ptr) -> ptr`.
    arg_count: bool,
    arg: bool,
}

impl RuntimeSyms {
    fn merge(&mut self, other: RuntimeSyms) {
        self.alloc |= other.alloc;
        self.realloc |= other.realloc;
        self.free |= other.free;
        self.panic_oob |= other.panic_oob;
        self.str_eq |= other.str_eq;
        self.read_file |= other.read_file;
        self.write_file |= other.write_file;
        self.print |= other.print;
        self.print_bytes |= other.print_bytes;
        self.memcpy |= other.memcpy;
        self.perform_op |= other.perform_op;
        self.kont_resume |= other.kont_resume;
        self.kont_consume_pure |= other.kont_consume_pure;
        self.kont_pure |= other.kont_pure;
        self.kont_push |= other.kont_push;
        self.task_spawn |= other.task_spawn;
        self.task_await |= other.task_await;
        self.scope_enter |= other.scope_enter;
        self.scope_register |= other.scope_register;
        self.scope_exit |= other.scope_exit;
        self.channel_new |= other.channel_new;
        self.channel_send |= other.channel_send;
        self.channel_recv |= other.channel_recv;
        self.channel_close |= other.channel_close;
        self.process_spawn |= other.process_spawn;
        self.process_wait |= other.process_wait;
        self.process_write |= other.process_write;
        self.process_read |= other.process_read;
        self.process_send |= other.process_send;
        self.process_recv |= other.process_recv;
        self.stdin_recv |= other.stdin_recv;
        self.stdout_send |= other.stdout_send;
        self.arg_count |= other.arg_count;
        self.arg |= other.arg;
    }

    /// Emit the `declare`s for the used symbols, in the fixed canonical order,
    /// returning whether any were emitted (the caller adds a trailing blank line).
    fn emit_declares(self, out: &mut String) -> bool {
        if self.alloc {
            writeln!(out, "declare ptr @sentinel_alloc(i64)").unwrap();
        }
        if self.realloc {
            writeln!(out, "declare ptr @sentinel_realloc(ptr, i64)").unwrap();
        }
        if self.free {
            writeln!(out, "declare void @sentinel_free(ptr)").unwrap();
        }
        if self.panic_oob {
            writeln!(out, "declare void @sentinel_panic_oob(i64, i64)").unwrap();
        }
        if self.str_eq {
            writeln!(out, "declare i1 @sentinel_str_eq(ptr, i64, ptr, i64)").unwrap();
        }
        if self.read_file {
            writeln!(out, "declare ptr @sentinel_read_file(ptr, i64, ptr)").unwrap();
        }
        if self.write_file {
            writeln!(out, "declare i64 @sentinel_write_file(ptr, i64, ptr, i64)").unwrap();
        }
        if self.print {
            writeln!(out, "declare i64 @sentinel_print(i64)").unwrap();
        }
        if self.print_bytes {
            writeln!(out, "declare i64 @sentinel_print_bytes(ptr, i64)").unwrap();
        }
        // Bar B / effects: the kont runtime symbols, in the `sentinel_*` group.
        if self.perform_op {
            writeln!(out, "declare ptr @sentinel_perform_op(i32, i64)").unwrap();
        }
        if self.kont_resume {
            writeln!(out, "declare ptr @sentinel_kont_resume(ptr, i64)").unwrap();
        }
        if self.kont_consume_pure {
            writeln!(out, "declare i64 @sentinel_kont_consume_pure(ptr)").unwrap();
        }
        if self.kont_pure {
            writeln!(out, "declare ptr @sentinel_kont_pure(i64)").unwrap();
        }
        if self.kont_push {
            writeln!(out, "declare void @sentinel_kont_push(ptr, ptr, ptr)").unwrap();
        }
        // Bar B / concurrency: the structured-concurrency runtime group.
        if self.task_spawn {
            writeln!(out, "declare ptr @sentinel_task_spawn(ptr, ptr, i64)").unwrap();
        }
        if self.task_await {
            writeln!(out, "declare i64 @sentinel_task_await(ptr)").unwrap();
        }
        if self.scope_enter {
            writeln!(out, "declare ptr @sentinel_scope_enter()").unwrap();
        }
        if self.scope_register {
            writeln!(out, "declare void @sentinel_scope_register(ptr, ptr)").unwrap();
        }
        if self.scope_exit {
            writeln!(out, "declare void @sentinel_scope_exit(ptr)").unwrap();
        }
        // ADR 0066 M1.2: the channel runtime group (recv returns the i64 status,
        // not i1 — codegen computes the valid bit via `icmp eq … 0`).
        if self.channel_new {
            writeln!(out, "declare ptr @sentinel_channel_new()").unwrap();
        }
        if self.channel_send {
            writeln!(out, "declare i64 @sentinel_channel_send(ptr, i64)").unwrap();
        }
        if self.channel_recv {
            writeln!(out, "declare i64 @sentinel_channel_recv(ptr, ptr)").unwrap();
        }
        if self.channel_close {
            writeln!(out, "declare i64 @sentinel_channel_close(ptr)").unwrap();
        }
        // ADR 0066 M2.1: the subprocess runtime group.
        if self.process_spawn {
            writeln!(out, "declare ptr @sentinel_process_spawn(ptr, i64, ptr, i64)").unwrap();
        }
        if self.process_wait {
            writeln!(out, "declare i64 @sentinel_process_wait(ptr)").unwrap();
        }
        // ADR 0066 M2.2: the byte-pipe IPC runtime group.
        if self.process_write {
            writeln!(out, "declare i64 @sentinel_process_write(ptr, ptr, i64)").unwrap();
        }
        if self.process_read {
            writeln!(out, "declare ptr @sentinel_process_read(ptr, ptr)").unwrap();
        }
        // ADR 0066 M2.3: the typed framed-channel-over-pipe runtime group.
        if self.process_send {
            writeln!(out, "declare i64 @sentinel_process_send(ptr, i64)").unwrap();
        }
        if self.process_recv {
            writeln!(out, "declare i64 @sentinel_process_recv(ptr, ptr)").unwrap();
        }
        // ADR 0066 M2.4b: the child-side self-stdin/stdout framed runtime group.
        if self.stdin_recv {
            writeln!(out, "declare i64 @sentinel_stdin_recv(ptr)").unwrap();
        }
        if self.stdout_send {
            writeln!(out, "declare i64 @sentinel_stdout_send(i64)").unwrap();
        }
        // ADR 0066 M2.4 follow-on: own command-line argument reflection.
        if self.arg_count {
            writeln!(out, "declare i64 @sentinel_arg_count()").unwrap();
        }
        if self.arg {
            writeln!(out, "declare ptr @sentinel_arg(i64, ptr)").unwrap();
        }
        if self.memcpy {
            writeln!(out, "declare void @llvm.memcpy.p0.p0.i64(ptr, ptr, i64, i1)").unwrap();
        }
        self.alloc
            || self.realloc
            || self.free
            || self.panic_oob
            || self.str_eq
            || self.read_file
            || self.write_file
            || self.print
            || self.print_bytes
            || self.perform_op
            || self.kont_resume
            || self.kont_consume_pure
            || self.kont_pure
            || self.kont_push
            || self.task_spawn
            || self.task_await
            || self.scope_enter
            || self.scope_register
            || self.scope_exit
            || self.channel_new
            || self.channel_send
            || self.channel_recv
            || self.channel_close
            || self.process_spawn
            || self.process_wait
            || self.process_write
            || self.process_read
            || self.process_send
            || self.process_recv
            || self.stdin_recv
            || self.stdout_send
            || self.arg_count
            || self.arg
            || self.memcpy
    }
}

/// Bar B / concurrency (ADR 0024) — collect every `spawn`-target FnId reachable from a
/// fn body (deduped + FnId-sorted by the caller). Each unique target gets one
/// `__spawn_wrapper_<id>` synthesized. Mirrors inkwell `collect_spawn_targets_*`.
fn collect_spawn_targets_block(block: &TypedBlock, acc: &mut Vec<FnId>) {
    for s in &block.stmts {
        collect_spawn_targets_stmt(&s.kind, acc);
    }
    collect_spawn_targets_expr(&block.tail, acc);
}

fn collect_spawn_targets_stmt(kind: &TypedStmtKind, acc: &mut Vec<FnId>) {
    match kind {
        TypedStmtKind::Let { value, .. } => collect_spawn_targets_expr(value, acc),
        TypedStmtKind::Assign { target, value } => {
            collect_spawn_targets_expr(target, acc);
            collect_spawn_targets_expr(value, acc);
        }
        TypedStmtKind::While { cond, body } => {
            collect_spawn_targets_expr(cond, acc);
            collect_spawn_targets_block(body, acc);
        }
        TypedStmtKind::Break | TypedStmtKind::Continue => {}
        TypedStmtKind::Expr(e) => collect_spawn_targets_expr(e, acc),
    }
}

fn collect_spawn_targets_expr(expr: &TypedExpr, acc: &mut Vec<FnId>) {
    match &expr.kind {
        TypedExprKind::Spawn { call, .. } => {
            if let TypedExprKind::Call { id, args, .. } = &call.kind {
                acc.push(*id);
                for a in args {
                    collect_spawn_targets_expr(a, acc);
                }
            }
        }
        TypedExprKind::Scope { body, .. } => collect_spawn_targets_block(body, acc),
        TypedExprKind::Await { task_expr, .. } => collect_spawn_targets_expr(task_expr, acc),
        TypedExprKind::Block(b) => collect_spawn_targets_block(b, acc),
        TypedExprKind::If { cond, then_branch, else_branch } => {
            collect_spawn_targets_expr(cond, acc);
            collect_spawn_targets_block(then_branch, acc);
            collect_spawn_targets_block(else_branch, acc);
        }
        TypedExprKind::Call { args, .. } => {
            for a in args {
                collect_spawn_targets_expr(a, acc);
            }
        }
        // Other forms carry no `spawn` in the emitted corpus (spawn only appears in
        // let-value / tail position inside a `scope`); a missed nesting would surface
        // loudly as a reference to an undefined wrapper.
        _ => {}
    }
}

/// Bar B / concurrency — synthesize `void @__spawn_wrapper_<id>(ptr %arg0, ptr %arg1)`
/// (`%arg0` = the Task, `%arg1` = the packed args): unpack `n` args (8-byte slots), call
/// the target, store the result into `task` (offset 0), set `task->done` (i32 @ 8), return
/// void. Mirrors inkwell's pre-walk wrapper synthesis. ADR 0066 M1.1: each arg is loaded
/// with its real type and the result is ENCODED into the i64 result slot (zext narrow int /
/// bitcast f64 / ptrtoint ptr); the `i64` case is a no-op, byte-identical to C4.4.
fn dump_spawn_wrapper(program: &TypedProgram, fn_id: FnId, out: &mut String) -> Result<(), String> {
    let target = program.fns.iter().find(|f| f.id == fn_id);
    let (name, n) = match target {
        Some(f) => (f.name.clone(), f.params.len()),
        None => return Ok(()),
    };
    let sig = program.signature(fn_id);
    writeln!(out, "define void @__spawn_wrapper_{}(ptr %arg0, ptr %arg1) {{", fn_id.0).unwrap();
    out.push_str("entry:\n");
    let mut next: u32 = 0;
    let mut call_args: Vec<String> = Vec::with_capacity(n);
    for (i, &pty) in sig.param_types.iter().enumerate() {
        let aty = llvm_ty(pty, program)?;
        let off = i * 8;
        let gp = next;
        next += 1;
        writeln!(out, "  %v{gp} = getelementptr i8, ptr %arg1, i64 {off}").unwrap();
        let ld = next;
        next += 1;
        writeln!(out, "  %v{ld} = load {aty}, ptr %v{gp}").unwrap();
        call_args.push(format!("{aty} %v{ld}"));
    }
    let rty = llvm_ty(sig.return_type, program)?;
    let call_reg = next;
    next += 1;
    writeln!(out, "  %v{call_reg} = call {rty} @{name}({})", call_args.join(", ")).unwrap();
    // ADR 0066 M1.1: encode the result into the Task's i64 result slot.
    let enc = match sig.return_type.strip_secret(&program.secrets).0 {
        Type::I64 => call_reg,
        Type::I32 | Type::U8 | Type::Bool => {
            let e = next;
            next += 1;
            writeln!(out, "  %v{e} = zext {rty} %v{call_reg} to i64").unwrap();
            e
        }
        Type::F64 => {
            let e = next;
            next += 1;
            writeln!(out, "  %v{e} = bitcast double %v{call_reg} to i64").unwrap();
            e
        }
        Type::Ptr | Type::Task(_) => {
            let e = next;
            next += 1;
            writeln!(out, "  %v{e} = ptrtoint ptr %v{call_reg} to i64").unwrap();
            e
        }
        other => {
            return Err(format!(
                "spawn result type not yet ported (ADR 0066 M1.1 word-scalar): {other:?}"
            ));
        }
    };
    writeln!(out, "  store i64 %v{enc}, ptr %arg0").unwrap();
    let done_gp = next;
    writeln!(out, "  %v{done_gp} = getelementptr i8, ptr %arg0, i64 8").unwrap();
    writeln!(out, "  store i32 1, ptr %v{done_gp}").unwrap();
    out.push_str("  ret void\n}\n");
    Ok(())
}

/// Emit the canonical `.ll` for `program`, or `Err(why)` if it uses a
/// construct not yet ported (the caller exits nonzero so the differential
/// skips the fixture).
pub fn dump(program: &TypedProgram, drop_plan: &DropPlan) -> Result<String, String> {
    let mut out = String::new();
    writeln!(out, "target triple = \"{TARGET_TRIPLE}\"").unwrap();
    out.push('\n');
    // Pass 0: user struct type decls, in StructId order (= program.structs order),
    // as `%Struct.N = type { <field-ll-types> }`. The aggregate is a first-class
    // SSA value (struct-lit → insertvalue, field → extractvalue; no GEP/memory for
    // the struct itself). Generic struct *decls* (8h/Bar B) have no runtime layout
    // → skipped. A field of an as-yet-unported type makes `llvm_ty` Err → the whole
    // dump Errs → the fixture is skipped (partial-by-Err), lighting up once that
    // field type is ported (`[u8]`/strings 8c-rest, Vec 8d, enums 8e).
    let mut emitted_struct = false;
    for sd in &program.structs {
        if !sd.type_params.is_empty() {
            continue;
        }
        if sd.fields.is_empty() {
            writeln!(out, "%Struct.{} = type {{}}", sd.id.0).unwrap();
        } else {
            let mut field_lls = Vec::with_capacity(sd.fields.len());
            for f in &sd.fields {
                field_lls.push(llvm_ty(f.ty, program)?);
            }
            writeln!(out, "%Struct.{} = type {{ {} }}", sd.id.0, field_lls.join(", ")).unwrap();
        }
        emitted_struct = true;
    }
    // Generic-struct instance decls (Bar B): each CONCRETE instance `Decl<args>` gets
    // `%<mangled> = type { <substituted field types> }`. Abstract (TypeParam-bearing)
    // instances — interned for generic-fn signatures like `fst<A,B>(p: Pair<A,B>)` —
    // have no runtime layout, so they're skipped. The name is structural (order-
    // independent); the emission order here must match the Sentinel side (both iterate
    // the instance table; ≤1 concrete instance per corpus fixture today).
    for inst in &program.generic_instances {
        if !instance_is_concrete(&inst.args, program) {
            continue;
        }
        let name = mangle_instance(program, inst.struct_id, &inst.args);
        let decl = program.struct_decl(inst.struct_id);
        if decl.fields.is_empty() {
            writeln!(out, "%{name} = type {{}}").unwrap();
        } else {
            // Substitute each declared field type by the instance's type-args to get
            // the concrete LLVM field layout (`Box<i64>`'s `value: T` → `i64`).
            let mut field_lls = Vec::with_capacity(decl.fields.len());
            for f in &decl.fields {
                let mut insts = program.generic_instances.clone();
                let mut refs = program.refs.clone();
                let concrete = f.ty.substitute(&inst.args, &mut insts, &mut refs);
                field_lls.push(llvm_ty(concrete, program)?);
            }
            writeln!(out, "%{name} = type {{ {} }}", field_lls.join(", ")).unwrap();
        }
        emitted_struct = true;
    }
    // Class decls (Bar B / classes): each `class Name { let f: T; … }` gets a named
    // aggregate `%Class.N = type { <field types> }` (N = ClassId, source order), exactly
    // like `%Struct.N`. A class instance is held + passed BY VALUE as this aggregate; the
    // pointer/GEP machinery (init out_ptr, method self_ptr, `self.field`) layers on top of
    // it. Empty loop for class-free fixtures → byte-unchanged.
    for cd in &program.class_decls {
        if cd.fields.is_empty() {
            writeln!(out, "%Class.{} = type {{}}", cd.id.0).unwrap();
        } else {
            let mut field_lls = Vec::with_capacity(cd.fields.len());
            for f in &cd.fields {
                field_lls.push(llvm_ty(f.ty, program)?);
            }
            writeln!(out, "%Class.{} = type {{ {} }}", cd.id.0, field_lls.join(", ")).unwrap();
        }
        emitted_struct = true;
    }
    if emitted_struct {
        out.push('\n');
    }
    // Bar B / generics: discover every monomorphic fn instance `(FnId, type_args)`
    // reachable from non-generic bodies (reusing the inkwell backend's worklist, so
    // the set + order match). May extend `instances`/`refs` with nested generic
    // instances surfaced during substitution.
    let mut instances = program.generic_instances.clone();
    let mut refs = program.refs.clone();
    let mono_insts = collect_mono_instantiations(program, &mut instances, &mut refs);

    // FnId order = source order; deterministic + matches the Sentinel side. Build
    // the fns into a buffer so the runtime-symbol `declare`s — emitted only for the
    // symbols the bodies actually use — can be placed BEFORE them in the module.
    let mut fns_buf = String::new();
    let mut used = RuntimeSyms::default();
    // Non-generic fns first; generic fn DEFS are monomorphized below (not dumped raw).
    let mut fns: Vec<&TypedFnDef> =
        program.fns.iter().filter(|f| f.type_params.is_empty()).collect();
    fns.sort_by_key(|f| f.id.0);
    for f in fns {
        dump_fn(f, program, drop_plan, &mut fns_buf, &mut used)?;
        fns_buf.push('\n');
    }
    // Then one monomorphic `define` per instance — substitute the generic def to a
    // concrete `TypedFnDef`, emit under its mangled symbol (`id__i64`). Insertion
    // order from the worklist (mirrors inkwell; the Sentinel records the same order
    // during its walk).
    for (fn_id, args) in &mono_insts {
        let gdef = program
            .fns
            .iter()
            .find(|f| f.id == *fn_id)
            .ok_or("mono instance references an unknown fn")?;
        let mono_def = gdef.substitute(args, &mut instances, &mut refs);
        let sym = mangle_mono_name(&gdef.name, args, program);
        dump_fn_named(&mono_def, program, drop_plan, &mut fns_buf, &mut used, &sym)?;
        fns_buf.push('\n');
    }
    // Class init + method bodies, then impl method bodies (Bar B / classes). Each is a
    // pointer-ABI function — `self`/`out_ptr` as the first `ptr` param, declared params
    // following — emitted after the free fns (no FnId; they're not in `program.fns`).
    // ClassId / ImplId order = source order, matching the Sentinel side. Empty for
    // class-free fixtures → byte-unchanged.
    for cd in &program.class_decls {
        if let Some(init) = &cd.init {
            let sym = format!("{}__init", cd.name);
            dump_method(
                program, drop_plan, &mut fns_buf, &mut used, &sym, init.self_var_id, cd.id,
                &init.params, None, &init.body,
            )?;
            fns_buf.push('\n');
        }
        for m in &cd.methods {
            let sym = format!("{}__{}", cd.name, m.name);
            dump_method(
                program, drop_plan, &mut fns_buf, &mut used, &sym, m.self_var_id, cd.id,
                &m.params, Some(m.return_type), &m.body,
            )?;
            fns_buf.push('\n');
        }
    }
    for imp in &program.impl_decls {
        for m in &imp.methods {
            let sym = mangle_impl_method(imp, &m.name);
            // The impl method's `self` is a `ptr` to the implementing type's storage; we
            // bind it as `Type::Class(self_class)` — every corpus impl targets a class.
            let self_class = match imp.target {
                sentinel_resolve::ImplTarget::Class(cid) => cid,
                ref other => return Err(format!("impl on a non-class target: {other:?}")),
            };
            dump_method(
                program, drop_plan, &mut fns_buf, &mut used, &sym, m.self_var_id, self_class,
                &m.params, Some(m.return_type), &m.body,
            )?;
            fns_buf.push('\n');
        }
    }
    // Bar B / concurrency (ADR 0024 D8): one `__spawn_wrapper_<id>` per unique spawn
    // target, in FnId order (deduped) — emitted after all fn bodies. Empty for
    // concurrency-free fixtures → byte-unchanged.
    let mut spawn_targets: Vec<FnId> = Vec::new();
    for f in program.fns.iter().filter(|f| f.type_params.is_empty()) {
        collect_spawn_targets_block(&f.body, &mut spawn_targets);
    }
    spawn_targets.sort_by_key(|f| f.0);
    spawn_targets.dedup();
    for fn_id in spawn_targets {
        dump_spawn_wrapper(program, fn_id, &mut fns_buf)?;
        fns_buf.push('\n');
    }
    // Runtime-symbol declarations (8c-2+): only the symbols actually used, in a
    // fixed order, so a program using none stays byte-identical to 8c-1.
    if used.emit_declares(&mut out) {
        out.push('\n');
    }
    out.push_str(&fns_buf);
    Ok(out)
}

fn dump_fn(
    f: &TypedFnDef,
    program: &TypedProgram,
    drop_plan: &DropPlan,
    out: &mut String,
    used: &mut RuntimeSyms,
) -> Result<(), String> {
    dump_fn_named(f, program, drop_plan, out, used, &f.name)
}

/// Emit a fn `define` under symbol `sym` — `f.name` for a normal fn, or the mangled
/// `id__i64` for a monomorphic instance (whose `f` is a SUBSTITUTED, concrete
/// `TypedFnDef`, so no `TypeParam` reaches `llvm_ty`). Generic fn DEFS are never passed
/// here — the caller monomorphizes + filters them; only concrete defs reach this.
fn dump_fn_named(
    f: &TypedFnDef,
    program: &TypedProgram,
    drop_plan: &DropPlan,
    out: &mut String,
    used: &mut RuntimeSyms,
    sym: &str,
) -> Result<(), String> {
    let sig = program.signature(f.id);
    // Bar B / effects (c35b): an effecting fn (non-empty, non-`Async` effect row) uses
    // the Kont* ABI — it returns `ptr` (a continuation), not its declared type. Only
    // the c35b body shapes lower yet (a direct `perform`, a call to another effecting
    // fn, or a fully pure tail wrapped via `sentinel_kont_pure`); let-bound / embedded
    // / chained `perform` need per-eval-site frame reification (c35c+) and Err here.
    let is_effecting = uses_kont_abi(sig, program);
    if is_effecting {
        // c35c: a let-bound-perform body (`let v: i64 = perform …; <pure tail>`) reifies
        // a captured evaluation frame — emit a RESUMER fn + the parent (alloc the
        // captured-state struct, lower the RHS to a Kont*, `sentinel_kont_push` the
        // frame), returning early. Other performing bodies still defer (c35d+).
        if let Some(info) = detect_let_shape(f, program) {
            return dump_let_shape_fn(f, &info, program, drop_plan, out, used, sym);
        }
        // c35d: an embedded-perform body (a statement-free tail mixing exactly ONE
        // `perform` into pure context — `perform Op() + 1`, `f(perform Op())`) also
        // reifies a captured frame: the parent lowers JUST the perform; the resumer
        // lowers the tail with the perform substituted by the resumed value.
        if let Some(perform) = detect_embedded_shape(f, program) {
            return dump_embedded_shape_fn(f, perform, program, drop_plan, out, used, sym);
        }
        // c35e: chained effecting lets (2+ `let v: i64 = <produces-kont>` + a pure
        // tail) — N+1 defines: the parent pushes resumer-0 onto let-0's kont; each
        // resumer-i binds let-i + its captures, then performs let-(i+1) and pushes
        // resumer-(i+1) (the runtime bubbles the fresh kont); the last wraps the tail.
        if let Some(info) = detect_chained_lets_shape(f, program) {
            return dump_chained_lets_fn(f, &info, program, drop_plan, out, used, sym);
        }
        validate_effecting_fn_body(f, program)?;
    }
    // main is the C-ABI entry: i32 return (its i64 body is truncated). An effecting fn
    // returns `ptr`; an ordinary fn its declared type.
    let ret_ll = if sig.is_main {
        "i32".to_string()
    } else if is_effecting {
        "ptr".to_string()
    } else {
        llvm_ty(f.return_type, program)?
    };

    let mut e = Emit {
        program,
        next: 0,
        block: 0,
        slots: HashMap::new(),
        var_ty: HashMap::new(),
        scopes: Vec::new(),
        drop_plan,
        current_fn: f.id,
        allocas: String::new(),
        body: String::new(),
        loops: Vec::new(),
        used: RuntimeSyms::default(),
        self_var: None,
        handle_stack: Vec::new(),
        embed_ph: None,
        handle_depth: 0,
        current_scope: None,
    };
    // Allocas are HOISTED to the entry block: param slots, `let` slots, and the
    // if-result slot all land in `e.allocas` (emitted first), while stores/loads/
    // ops go to `e.body`. This (a) lets the if-result alloca carry a type known
    // only AFTER its then-branch is walked, and (b) keeps loop-body allocas out of
    // the loop (no per-iteration stack growth — ADR 0036). The param STORE stays
    // in the body.
    // 8d-drops: params live in the outermost scope frame (frame 0); the fn body is
    // a block that pushes frame 1. At fn exit the body-frame drops fire first, then
    // the param-frame drops (reverse declaration order overall) — matching the
    // production codegen's scope-0 (params) + scope-1 (body) structure.
    e.scopes.push(Vec::new());
    for (i, p) in f.params.iter().enumerate() {
        let ty = llvm_ty(p.ty, program)?;
        let slot = e.alloca(&ty);
        writeln!(e.body, "  store {ty} %arg{i}, ptr %v{slot}").unwrap();
        e.slots.insert(p.id, slot);
        e.var_ty.insert(p.id, p.ty);
        e.scopes.last_mut().unwrap().push(p.id);
    }
    // The body block (frame 1): its locals drop at the body's end, before the params.
    e.scopes.push(Vec::new());
    for stmt in &f.body.stmts {
        e.lower_stmt(stmt)?;
    }
    let tail = e.lower_expr(&f.body.tail)?;
    e.emit_scope_drops()?; // body-frame drops
    e.scopes.pop();
    e.emit_scope_drops()?; // param-frame drops
    e.scopes.pop();
    if sig.is_main {
        let t = e.fresh();
        writeln!(e.body, "  %v{t} = trunc i64 {tail} to i32").unwrap();
        writeln!(e.body, "  ret i32 %v{t}").unwrap();
    } else if is_effecting && !produces_kont(&f.body.tail, program) {
        // Effecting fn with a pure tail: wrap the i64 value as a PURE_RETURN kont so
        // the caller's `handle` sees a uniform Kont* (and dispatches PURE_RETURN).
        let kp = e.fresh();
        writeln!(e.body, "  %v{kp} = call ptr @sentinel_kont_pure(i64 {tail})").unwrap();
        e.used.kont_pure = true;
        writeln!(e.body, "  ret ptr %v{kp}").unwrap();
    } else {
        // Ordinary fn, or an effecting fn whose tail already produced a Kont*.
        writeln!(e.body, "  ret {ret_ll} {tail}").unwrap();
    }

    // Assemble: the `define` header, then the entry block (hoisted allocas first),
    // then the body.
    write!(out, "define {ret_ll} @{sym}(").unwrap();
    for (i, p) in f.params.iter().enumerate() {
        if i > 0 {
            out.push_str(", ");
        }
        write!(out, "{} %arg{i}", llvm_ty(p.ty, program)?).unwrap();
    }
    out.push_str(") {\nentry:\n");
    out.push_str(&e.allocas);
    out.push_str(&e.body);
    out.push_str("}\n");
    used.merge(e.used);
    Ok(())
}

/// Bar B / effects (c35c / ADR 0020 D7) — emit a let-bound-perform effecting fn as TWO
/// `define`s (mirroring inkwell `compile_effecting_fn_with_let`). The body shape is a
/// single `let v: i64 = <effecting RHS>` + a pure tail (`detect_let_shape`); the perform
/// sits in non-tail position, so it is reified into a runtime frame the kont's resume
/// replays:
///   1. the PARENT `@<sym>` (Kont* ABI, returns `ptr`): bind params, allocate + fill the
///      captured-state struct (i64[N], one field per tail-captured var, at byte offsets
///      0,8,…; `null` when empty), lower the let's effecting RHS to a Kont*,
///      `sentinel_kont_push` a frame (the resumer + captured ptr), return the Kont*.
///   2. the RESUMER `@__resume_<sym>(i64 %arg0, ptr %arg1)`: bind the let var to the
///      resumed value `%arg0` + each captured var to its struct field (loaded from
///      `%arg1`), lower the pure tail, wrap it via `sentinel_kont_pure`, return that
///      Kont*. Per the runtime ABI (`SentinelFrame`), the resumer is non-performing at
///      c35c (chains land at c35e).
///
/// The two share NO register counter (each is its own `Emit`, `next: 0`) — matching the
/// Sentinel side's `cg_reset` between the two defines. Emission order: parent, resumer.
#[allow(clippy::too_many_arguments)]
fn dump_let_shape_fn(
    f: &TypedFnDef,
    info: &LetShapeInfo<'_>,
    program: &TypedProgram,
    drop_plan: &DropPlan,
    out: &mut String,
    used: &mut RuntimeSyms,
    sym: &str,
) -> Result<(), String> {
    // The captured set: the tail's free vars minus the let-bound var (re-bound from the
    // resumed value), in first-reference order — the struct field layout.
    let captured = collect_captured_vars(info.tail, info.let_id);
    let resumer_sym = format!("__resume_{sym}");

    // --- the PARENT define: params, captured struct, RHS → kont, push, ret ptr ---
    {
        let mut e = Emit {
            program,
            next: 0,
            block: 0,
            slots: HashMap::new(),
            var_ty: HashMap::new(),
            scopes: Vec::new(),
            drop_plan,
            current_fn: f.id,
            allocas: String::new(),
            body: String::new(),
            loops: Vec::new(),
            used: RuntimeSyms::default(),
            self_var: None,
            handle_stack: Vec::new(),
            embed_ph: None,
        handle_depth: 0,
        current_scope: None,
        };
        e.scopes.push(Vec::new());
        for (i, p) in f.params.iter().enumerate() {
            let ty = llvm_ty(p.ty, program)?;
            let slot = e.alloca(&ty);
            writeln!(e.body, "  store {ty} %arg{i}, ptr %v{slot}").unwrap();
            e.slots.insert(p.id, slot);
            e.var_ty.insert(p.id, p.ty);
            e.scopes.last_mut().unwrap().push(p.id);
        }
        // The captured-state struct: i64[N] via `sentinel_alloc`, or a null ptr when the
        // tail captures nothing (the resumer ignores its `%arg1`).
        let captured_op = if captured.is_empty() {
            "null".to_string()
        } else {
            let size = captured.len() * 8;
            let a = e.fresh();
            writeln!(e.body, "  %v{a} = call ptr @sentinel_alloc(i64 {size})").unwrap();
            e.used.alloc = true;
            for (i, cap_id) in captured.iter().enumerate() {
                let off = i * 8;
                let gp = e.fresh();
                writeln!(e.body, "  %v{gp} = getelementptr i8, ptr %v{a}, i64 {off}").unwrap();
                let slot = *e
                    .slots
                    .get(cap_id)
                    .ok_or("c35c: captured var is not a bound fn param")?;
                let ld = e.fresh();
                writeln!(e.body, "  %v{ld} = load i64, ptr %v{slot}").unwrap();
                writeln!(e.body, "  store i64 %v{ld}, ptr %v{gp}").unwrap();
            }
            format!("%v{a}")
        };
        // Lower the let's effecting RHS (a `perform` / call-to-effecting) → a Kont*.
        let kont = e.lower_expr(info.rhs)?;
        writeln!(
            e.body,
            "  call void @sentinel_kont_push(ptr {kont}, ptr @{resumer_sym}, ptr {captured_op})"
        )
        .unwrap();
        e.used.kont_push = true;
        writeln!(e.body, "  ret ptr {kont}").unwrap();
        write!(out, "define ptr @{sym}(").unwrap();
        for (i, p) in f.params.iter().enumerate() {
            if i > 0 {
                out.push_str(", ");
            }
            write!(out, "{} %arg{i}", llvm_ty(p.ty, program)?).unwrap();
        }
        out.push_str(") {\nentry:\n");
        out.push_str(&e.allocas);
        out.push_str(&e.body);
        out.push_str("}\n");
        used.merge(e.used);
    }
    out.push('\n');

    // --- the RESUMER define: bind v + captures, lower tail, kont_pure wrap, ret ptr ---
    {
        let mut e = Emit {
            program,
            next: 0,
            block: 0,
            slots: HashMap::new(),
            var_ty: HashMap::new(),
            scopes: Vec::new(),
            drop_plan,
            current_fn: f.id,
            allocas: String::new(),
            body: String::new(),
            loops: Vec::new(),
            used: RuntimeSyms::default(),
            self_var: None,
            handle_stack: Vec::new(),
            embed_ph: None,
        handle_depth: 0,
        current_scope: None,
        };
        e.scopes.push(Vec::new());
        // Bind the let var to the resumed value (`%arg0`).
        let v_slot = e.alloca("i64");
        writeln!(e.body, "  store i64 %arg0, ptr %v{v_slot}").unwrap();
        e.slots.insert(info.let_id, v_slot);
        e.var_ty.insert(info.let_id, info.let_ty);
        // Bind each captured var to its struct field, loaded from the captured ptr (`%arg1`).
        for (i, cap_id) in captured.iter().enumerate() {
            let off = i * 8;
            let gp = e.fresh();
            writeln!(e.body, "  %v{gp} = getelementptr i8, ptr %arg1, i64 {off}").unwrap();
            let ld = e.fresh();
            writeln!(e.body, "  %v{ld} = load i64, ptr %v{gp}").unwrap();
            let slot = e.alloca("i64");
            writeln!(e.body, "  store i64 %v{ld}, ptr %v{slot}").unwrap();
            e.slots.insert(*cap_id, slot);
            e.var_ty.insert(*cap_id, Type::I64);
        }
        // Lower the pure tail, wrap its i64 as a PURE_RETURN kont.
        let tail = e.lower_expr(info.tail)?;
        let kp = e.fresh();
        writeln!(e.body, "  %v{kp} = call ptr @sentinel_kont_pure(i64 {tail})").unwrap();
        e.used.kont_pure = true;
        writeln!(e.body, "  ret ptr %v{kp}").unwrap();
        write!(out, "define ptr @{resumer_sym}(i64 %arg0, ptr %arg1) {{\nentry:\n").unwrap();
        out.push_str(&e.allocas);
        out.push_str(&e.body);
        out.push_str("}\n");
        used.merge(e.used);
    }
    Ok(())
}

/// Bar B / effects (c35d / ADR 0020 D7) — emit an embedded-perform effecting fn as TWO
/// `define`s (mirroring inkwell `compile_effecting_fn_with_embedded_perform`). The body
/// is a statement-free tail mixing exactly ONE `perform` into pure context
/// (`perform Op() + 1`, `f(perform Op())` — `detect_embedded_shape`); the perform is
/// reified into a runtime frame whose resume re-evaluates the surrounding context:
///   1. the PARENT `@<sym>` (Kont* ABI, returns `ptr`): bind params, allocate + fill the
///      captured-state struct (i64[N], one field per tail-captured var at byte offsets
///      0,8,…; `null` when empty), lower JUST the unique `perform` (its args reference
///      the parent-bound params) to a Kont*, `sentinel_kont_push` a frame (the resumer
///      + captured ptr), return the Kont*.
///   2. the RESUMER `@__resume_<sym>(i64 %arg0, ptr %arg1)`: bind the placeholder slot
///      to the resumed value `%arg0` + each captured var to its struct field (loaded
///      from `%arg1`), lower the FULL tail with `Emit::embed_ph` set — the unique
///      `Perform` lowers as a load from the placeholder slot (the substituted-Var
///      equivalent) — wrap the i64 result via `sentinel_kont_pure`, return that Kont*.
///
/// The captured set is the tail's free vars EXCLUDING the perform subtree
/// (`walk_collect_var_refs` skips `Perform`, matching inkwell's walk over the
/// placeholder-substituted tail). The two defines share NO register counter (each its
/// own `Emit`, `next: 0`), matching the Sentinel side's `cg_reset`.
fn dump_embedded_shape_fn(
    f: &TypedFnDef,
    perform: &TypedExpr,
    program: &TypedProgram,
    drop_plan: &DropPlan,
    out: &mut String,
    used: &mut RuntimeSyms,
    sym: &str,
) -> Result<(), String> {
    // The captured set: the tail's free vars minus the perform subtree (whose args the
    // PARENT lowers), in first-reference order — the struct field layout.
    let mut captured: Vec<VarId> = Vec::new();
    walk_collect_var_refs(&f.body.tail, &mut captured);
    let resumer_sym = format!("__resume_{sym}");

    // --- the PARENT define: params, captured struct, perform → kont, push, ret ptr ---
    {
        let mut e = Emit {
            program,
            next: 0,
            block: 0,
            slots: HashMap::new(),
            var_ty: HashMap::new(),
            scopes: Vec::new(),
            drop_plan,
            current_fn: f.id,
            allocas: String::new(),
            body: String::new(),
            loops: Vec::new(),
            used: RuntimeSyms::default(),
            self_var: None,
            handle_stack: Vec::new(),
            embed_ph: None,
        handle_depth: 0,
        current_scope: None,
        };
        e.scopes.push(Vec::new());
        for (i, p) in f.params.iter().enumerate() {
            let ty = llvm_ty(p.ty, program)?;
            let slot = e.alloca(&ty);
            writeln!(e.body, "  store {ty} %arg{i}, ptr %v{slot}").unwrap();
            e.slots.insert(p.id, slot);
            e.var_ty.insert(p.id, p.ty);
            e.scopes.last_mut().unwrap().push(p.id);
        }
        // The captured-state struct: i64[N] via `sentinel_alloc`, or a null ptr when the
        // tail captures nothing (the resumer ignores its `%arg1`).
        let captured_op = if captured.is_empty() {
            "null".to_string()
        } else {
            let size = captured.len() * 8;
            let a = e.fresh();
            writeln!(e.body, "  %v{a} = call ptr @sentinel_alloc(i64 {size})").unwrap();
            e.used.alloc = true;
            for (i, cap_id) in captured.iter().enumerate() {
                let off = i * 8;
                let gp = e.fresh();
                writeln!(e.body, "  %v{gp} = getelementptr i8, ptr %v{a}, i64 {off}").unwrap();
                let slot = *e
                    .slots
                    .get(cap_id)
                    .ok_or("c35d: captured var is not a bound fn param")?;
                let ld = e.fresh();
                writeln!(e.body, "  %v{ld} = load i64, ptr %v{slot}").unwrap();
                writeln!(e.body, "  store i64 %v{ld}, ptr %v{gp}").unwrap();
            }
            format!("%v{a}")
        };
        // Lower JUST the unique perform → a Kont* (the pure context around it is the
        // resumer's to re-evaluate).
        let kont = e.lower_expr(perform)?;
        writeln!(
            e.body,
            "  call void @sentinel_kont_push(ptr {kont}, ptr @{resumer_sym}, ptr {captured_op})"
        )
        .unwrap();
        e.used.kont_push = true;
        writeln!(e.body, "  ret ptr {kont}").unwrap();
        write!(out, "define ptr @{sym}(").unwrap();
        for (i, p) in f.params.iter().enumerate() {
            if i > 0 {
                out.push_str(", ");
            }
            write!(out, "{} %arg{i}", llvm_ty(p.ty, program)?).unwrap();
        }
        out.push_str(") {\nentry:\n");
        out.push_str(&e.allocas);
        out.push_str(&e.body);
        out.push_str("}\n");
        used.merge(e.used);
    }
    out.push('\n');

    // --- the RESUMER define: bind ph + captures, lower tail, kont_pure wrap, ret ptr ---
    {
        let mut e = Emit {
            program,
            next: 0,
            block: 0,
            slots: HashMap::new(),
            var_ty: HashMap::new(),
            scopes: Vec::new(),
            drop_plan,
            current_fn: f.id,
            allocas: String::new(),
            body: String::new(),
            loops: Vec::new(),
            used: RuntimeSyms::default(),
            self_var: None,
            handle_stack: Vec::new(),
            embed_ph: None,
        handle_depth: 0,
        current_scope: None,
        };
        e.scopes.push(Vec::new());
        // Bind the placeholder slot to the resumed value (`%arg0`).
        let ph_slot = e.alloca("i64");
        writeln!(e.body, "  store i64 %arg0, ptr %v{ph_slot}").unwrap();
        // Bind each captured var to its struct field, loaded from the captured ptr (`%arg1`).
        for (i, cap_id) in captured.iter().enumerate() {
            let off = i * 8;
            let gp = e.fresh();
            writeln!(e.body, "  %v{gp} = getelementptr i8, ptr %arg1, i64 {off}").unwrap();
            let ld = e.fresh();
            writeln!(e.body, "  %v{ld} = load i64, ptr %v{gp}").unwrap();
            let slot = e.alloca("i64");
            writeln!(e.body, "  store i64 %v{ld}, ptr %v{slot}").unwrap();
            e.slots.insert(*cap_id, slot);
            e.var_ty.insert(*cap_id, Type::I64);
        }
        // Lower the FULL tail with the placeholder hook armed — the unique Perform
        // lowers as a load from `ph_slot`; wrap the i64 result as a PURE_RETURN kont.
        e.embed_ph = Some(ph_slot);
        let tail = e.lower_expr(&f.body.tail)?;
        let kp = e.fresh();
        writeln!(e.body, "  %v{kp} = call ptr @sentinel_kont_pure(i64 {tail})").unwrap();
        e.used.kont_pure = true;
        writeln!(e.body, "  ret ptr %v{kp}").unwrap();
        write!(out, "define ptr @{resumer_sym}(i64 %arg0, ptr %arg1) {{\nentry:\n").unwrap();
        out.push_str(&e.allocas);
        out.push_str(&e.body);
        out.push_str("}\n");
        used.merge(e.used);
    }
    Ok(())
}

/// Bar B / effects (c35e) — emit a captured-state struct build: `i64[N]` via
/// `sentinel_alloc`, one field per captured var at byte offsets 0,8,… filled from the
/// var's CURRENT slot; returns the struct operand (`%vA`), or the literal `null` when
/// nothing is captured (the receiving resumer ignores its `%arg1`).
fn emit_chained_captures_build(e: &mut Emit<'_>, caps: &[VarId]) -> Result<String, String> {
    if caps.is_empty() {
        return Ok("null".to_string());
    }
    let size = caps.len() * 8;
    let a = e.fresh();
    writeln!(e.body, "  %v{a} = call ptr @sentinel_alloc(i64 {size})").unwrap();
    e.used.alloc = true;
    for (i, cap_id) in caps.iter().enumerate() {
        let off = i * 8;
        let gp = e.fresh();
        writeln!(e.body, "  %v{gp} = getelementptr i8, ptr %v{a}, i64 {off}").unwrap();
        let slot = *e
            .slots
            .get(cap_id)
            .ok_or("c35e: captured var has no bound slot in the emitting define")?;
        let ld = e.fresh();
        writeln!(e.body, "  %v{ld} = load i64, ptr %v{slot}").unwrap();
        writeln!(e.body, "  store i64 %v{ld}, ptr %v{gp}").unwrap();
    }
    Ok(format!("%v{a}"))
}

/// Bar B / effects (c35e / ADR 0020 D7) — emit a chained-effecting-lets fn as N+1
/// `define`s (mirroring inkwell `compile_effecting_fn_with_chained_lets`; the oracle
/// emits parent-first, the established c35c/c35d layout):
///   1. the PARENT `@<sym>` (Kont* ABI): bind params, build the captured struct for
///      resumer-0 (`compute_chained_captures(0)`), lower lets[0]'s RHS to a Kont*,
///      `sentinel_kont_push(kont, @__resume_<sym>_0, captured)`, return the Kont*.
///   2. resumer-`i` `@__resume_<sym>_<i>(i64 %arg0, ptr %arg1)` for i in 0..N-1: bind
///      let-i to the resumed value + each captures[i] var from `%arg1`, build the
///      struct for resumer-(i+1) from its OWN slots, lower lets[i+1]'s RHS (the
///      perform's args resolve against this resumer's bindings) to a Kont*, push
///      (resumer-(i+1), captured), return the Kont* — the runtime BUBBLES it.
///   3. the LAST resumer `@__resume_<sym>_<N-1>`: bind let-(N-1) + captures[N-1],
///      lower the pure tail, wrap via `sentinel_kont_pure`, return.
///
/// Each define is its own `Emit` (`next: 0` — no shared register counter).
#[allow(clippy::too_many_arguments)]
fn dump_chained_lets_fn(
    f: &TypedFnDef,
    info: &ChainedLetsInfo<'_>,
    program: &TypedProgram,
    drop_plan: &DropPlan,
    out: &mut String,
    used: &mut RuntimeSyms,
    sym: &str,
) -> Result<(), String> {
    let n = info.lets.len();
    let captures_per: Vec<Vec<VarId>> =
        (0..n).map(|i| compute_chained_captures(info, i)).collect();

    // --- the PARENT define: params, captures-0 struct, lets[0] RHS → kont, push ---
    {
        let mut e = Emit {
            program,
            next: 0,
            block: 0,
            slots: HashMap::new(),
            var_ty: HashMap::new(),
            scopes: Vec::new(),
            drop_plan,
            current_fn: f.id,
            allocas: String::new(),
            body: String::new(),
            loops: Vec::new(),
            used: RuntimeSyms::default(),
            self_var: None,
            handle_stack: Vec::new(),
            embed_ph: None,
        handle_depth: 0,
        current_scope: None,
        };
        e.scopes.push(Vec::new());
        for (i, p) in f.params.iter().enumerate() {
            let ty = llvm_ty(p.ty, program)?;
            let slot = e.alloca(&ty);
            writeln!(e.body, "  store {ty} %arg{i}, ptr %v{slot}").unwrap();
            e.slots.insert(p.id, slot);
            e.var_ty.insert(p.id, p.ty);
            e.scopes.last_mut().unwrap().push(p.id);
        }
        let captured_op = emit_chained_captures_build(&mut e, &captures_per[0])?;
        let kont = e.lower_expr(info.lets[0].2)?;
        writeln!(
            e.body,
            "  call void @sentinel_kont_push(ptr {kont}, ptr @__resume_{sym}_0, ptr {captured_op})"
        )
        .unwrap();
        e.used.kont_push = true;
        writeln!(e.body, "  ret ptr {kont}").unwrap();
        write!(out, "define ptr @{sym}(").unwrap();
        for (i, p) in f.params.iter().enumerate() {
            if i > 0 {
                out.push_str(", ");
            }
            write!(out, "{} %arg{i}", llvm_ty(p.ty, program)?).unwrap();
        }
        out.push_str(") {\nentry:\n");
        out.push_str(&e.allocas);
        out.push_str(&e.body);
        out.push_str("}\n");
        used.merge(e.used);
    }

    // --- the N resumer defines ---
    for level in 0..n {
        out.push('\n');
        let mut e = Emit {
            program,
            next: 0,
            block: 0,
            slots: HashMap::new(),
            var_ty: HashMap::new(),
            scopes: Vec::new(),
            drop_plan,
            current_fn: f.id,
            allocas: String::new(),
            body: String::new(),
            loops: Vec::new(),
            used: RuntimeSyms::default(),
            self_var: None,
            handle_stack: Vec::new(),
            embed_ph: None,
        handle_depth: 0,
        current_scope: None,
        };
        e.scopes.push(Vec::new());
        // Bind let-`level` to the resumed value (`%arg0`).
        let (let_id, let_ty, _rhs) = info.lets[level];
        let v_slot = e.alloca("i64");
        writeln!(e.body, "  store i64 %arg0, ptr %v{v_slot}").unwrap();
        e.slots.insert(let_id, v_slot);
        e.var_ty.insert(let_id, let_ty);
        // Bind each captures[level] var to its struct field, loaded from `%arg1`.
        for (i, cap_id) in captures_per[level].iter().enumerate() {
            let off = i * 8;
            let gp = e.fresh();
            writeln!(e.body, "  %v{gp} = getelementptr i8, ptr %arg1, i64 {off}").unwrap();
            let ld = e.fresh();
            writeln!(e.body, "  %v{ld} = load i64, ptr %v{gp}").unwrap();
            let slot = e.alloca("i64");
            writeln!(e.body, "  store i64 %v{ld}, ptr %v{slot}").unwrap();
            e.slots.insert(*cap_id, slot);
            e.var_ty.insert(*cap_id, Type::I64);
        }
        if level + 1 == n {
            // The LAST resumer: lower the pure tail, wrap as a PURE_RETURN kont.
            let tail = e.lower_expr(info.tail)?;
            let kp = e.fresh();
            writeln!(e.body, "  %v{kp} = call ptr @sentinel_kont_pure(i64 {tail})").unwrap();
            e.used.kont_pure = true;
            writeln!(e.body, "  ret ptr %v{kp}").unwrap();
        } else {
            // A chaining resumer: build the next struct from THIS resumer's slots,
            // lower lets[level+1]'s RHS → kont, push resumer-(level+1), return it.
            let captured_op =
                emit_chained_captures_build(&mut e, &captures_per[level + 1])?;
            let kont = e.lower_expr(info.lets[level + 1].2)?;
            let next = level + 1;
            writeln!(
                e.body,
                "  call void @sentinel_kont_push(ptr {kont}, ptr @__resume_{sym}_{next}, ptr {captured_op})"
            )
            .unwrap();
            e.used.kont_push = true;
            writeln!(e.body, "  ret ptr {kont}").unwrap();
        }
        write!(out, "define ptr @__resume_{sym}_{level}(i64 %arg0, ptr %arg1) {{\nentry:\n")
            .unwrap();
        out.push_str(&e.allocas);
        out.push_str(&e.body);
        out.push_str("}\n");
        used.merge(e.used);
    }
    Ok(())
}

/// The mangled symbol for an impl method (Bar B / classes), mirroring the inkwell
/// backend (lib.rs:732): `<prefix>__<Type>__<Trait>__<method>`, where `prefix` is the
/// impl's name (a named impl like `Doubling`) or `default` (an unnamed `impl as Trait
/// for Type`). The delegate-synthesized impls reach here as ordinary default impls.
fn mangle_impl_method(imp: &sentinel_types::ImplData, method: &str) -> String {
    let prefix = imp.name.as_deref().unwrap_or("default");
    format!("{prefix}__{}__{}__{}", imp.type_name, imp.trait_name, method)
}

/// Emit a class init / method / impl-method body under `sym` (Bar B / classes). The
/// pointer ABI (ADR 0022 D9): `self` is the FIRST param, a `ptr` (`%arg0`) into the
/// live class storage — bound via `Emit::self_var` (NOT an alloca, so writes persist
/// to the caller) — and the declared params follow as `%arg1..` (alloca/store like a
/// free fn's). `ret_ty = None` is an `init` (returns `void`; its body tail is the
/// `0` placeholder per ADR 0022's init shape, NOT lowered); `Some(t)` returns `t`.
#[allow(clippy::too_many_arguments)]
fn dump_method(
    program: &TypedProgram,
    drop_plan: &DropPlan,
    out: &mut String,
    used: &mut RuntimeSyms,
    sym: &str,
    self_var_id: VarId,
    self_class: ClassId,
    params: &[TypedParam],
    ret_ty: Option<Type>,
    body: &TypedBlock,
) -> Result<(), String> {
    let ret_ll = match ret_ty {
        Some(t) => llvm_ty(t, program)?,
        None => "void".to_string(),
    };
    let mut e = Emit {
        program,
        next: 0,
        block: 0,
        slots: HashMap::new(),
        var_ty: HashMap::new(),
        scopes: Vec::new(),
        drop_plan,
        // No FnId — method bodies aren't in `program.fns`; a placeholder keys an empty
        // moved-set (`moved_sources_for` returns EMPTY for an unknown fn), so scope-exit
        // drops fire normally (and the corpus methods have no heap locals anyway).
        current_fn: FnId(u32::MAX),
        allocas: String::new(),
        body: String::new(),
        loops: Vec::new(),
        used: RuntimeSyms::default(),
        self_var: Some(self_var_id),
        handle_stack: Vec::new(),
        embed_ph: None,
        handle_depth: 0,
        current_scope: None,
    };
    // `self` is `%arg0` — bound by `self_var`, with its body type recorded as the class
    // (so `Var(self)` loads `%Class.N` and `self.f` GEPs the class). It is BORROWED, not
    // owned, so it is NOT pushed onto a scope frame (never dropped here).
    e.var_ty.insert(self_var_id, Type::Class(self_class));
    // Param frame (frame 0): declared params are `%arg1..` (self shifts them by one).
    e.scopes.push(Vec::new());
    for (i, p) in params.iter().enumerate() {
        let ty = llvm_ty(p.ty, program)?;
        let slot = e.alloca(&ty);
        writeln!(e.body, "  store {ty} %arg{}, ptr %v{slot}", i + 1).unwrap();
        e.slots.insert(p.id, slot);
        e.var_ty.insert(p.id, p.ty);
        e.scopes.last_mut().unwrap().push(p.id);
    }
    // Body frame (frame 1): its locals drop at the body's end, before the params.
    e.scopes.push(Vec::new());
    for stmt in &body.stmts {
        e.lower_stmt(stmt)?;
    }
    match ret_ty {
        Some(_) => {
            let tail = e.lower_expr(&body.tail)?;
            e.emit_scope_drops()?;
            e.scopes.pop();
            e.emit_scope_drops()?;
            e.scopes.pop();
            writeln!(e.body, "  ret {ret_ll} {tail}").unwrap();
        }
        None => {
            // init: lower stmts only (the `0` tail is a placeholder, ADR 0022) → ret void.
            e.emit_scope_drops()?;
            e.scopes.pop();
            e.emit_scope_drops()?;
            e.scopes.pop();
            e.body.push_str("  ret void\n");
        }
    }
    // Assemble: `self` (`ptr %arg0`) then the declared params (`%arg1..`).
    write!(out, "define {ret_ll} @{sym}(ptr %arg0").unwrap();
    for (i, p) in params.iter().enumerate() {
        write!(out, ", {} %arg{}", llvm_ty(p.ty, program)?, i + 1).unwrap();
    }
    out.push_str(") {\nentry:\n");
    out.push_str(&e.allocas);
    out.push_str(&e.body);
    out.push_str("}\n");
    used.merge(e.used);
    Ok(())
}

/// Per-fn emission state: the SSA value counter, the block-label counter, the
/// `VarId → alloca-slot` map (the slot's `%vN` number), the hoisted-alloca buffer,
/// and the instruction buffer.
struct Emit<'a> {
    program: &'a TypedProgram,
    next: u32,
    block: u32,
    slots: HashMap<VarId, u32>,
    /// 8d-drops: the type of each bound var, for the scope-exit drop dispatch.
    var_ty: HashMap<VarId, Type>,
    /// 8d-drops: the per-scope declared VarIds (one inner `Vec` per lexical block,
    /// outermost = the fn's param frame). Drops fire in reverse declaration order at
    /// each block's exit. See [`Self::emit_scope_drops`].
    scopes: Vec<Vec<VarId>>,
    /// 8d-drops: the borrow-check move plan — a binding in `moved_sources_for` is
    /// owned + dropped by its consumer, so the current fn skips its scope-exit free.
    drop_plan: &'a DropPlan,
    /// 8d-drops: the fn being emitted (to key `moved_sources_for`).
    current_fn: FnId,
    allocas: String,
    body: String,
    /// The enclosing loops' (cond-block, after-block, scope_floor) — `break` branches
    /// to the innermost after-block, `continue` to its cond-block, and both drain the
    /// open scope frames down to `scope_floor` (the loop-body frame index) first, so
    /// per-iteration heap bindings are freed on the early-exit path (8d-drops-3).
    loops: Vec<(u32, u32, usize)>,
    /// The `sentinel_*` runtime symbols this fn's body uses (merged module-wide
    /// into the `declare`s — 8c-2+).
    used: RuntimeSyms,
    /// Bar B / classes: inside a class init / method / impl method, the synthetic
    /// `self` binding. `self` is NOT an alloca slot — it IS the first `ptr` param
    /// (`%arg0`), pointing at the live class storage so field writes persist to the
    /// caller. `Var(self)` LOADS the whole `%Class.N` from it; `&self`/the lvalue of
    /// `self.f` GEPs from it. `None` for a free fn.
    self_var: Option<VarId>,
    /// Bar B / effects (ADR 0020): the enclosing `handle`s' dispatch context —
    /// `(loop_block, current_kont_slot, return_arm)`. A `k(v)` (`ResumeKont`) inside an
    /// arm whose resume BUBBLES (a resumer performed) stores the bubble kont into the
    /// innermost handle's `current_kont_slot` and branches to its `loop_block` to
    /// re-dispatch. c36a: the optional `return v => body` arm is carried (owned clone) so
    /// `k(v)`'s pure-drain path applies it per Phase B's deep-handler re-wrap.
    handle_stack: Vec<(u32, u32, Option<TypedReturnArm>)>,
    /// Bar B / effects (c35d): inside an embedded-perform RESUMER, the placeholder
    /// slot holding the resumed value. When set, the unique `Perform` in the tail
    /// lowers as a `load` from this slot (the parent already lowered the real
    /// perform + its args); `None` everywhere else.
    embed_ph: Option<u32>,
    /// Bar B / effects (c36b): the dynamic `handle` nesting depth. Incremented on entry
    /// to `lower_handle`, decremented on exit; `> 1` means this handle is nested (its
    /// body is reached from an enclosing handle), so it lowers to a Kont*-typed result
    /// (arms wrap their i64 via `kont_pure`, the switch default PROPAGATES the un-caught
    /// kont to the merge) instead of an i64.
    handle_depth: u32,
    /// Bar B / concurrency (ADR 0024): the enclosing `scope concurrent`'s `*ScopeCtx`
    /// register, so a `spawn` inside the scope registers its Task with it (the scope
    /// owns + auto-awaits unawaited tasks at exit). `None` outside a scope.
    current_scope: Option<u32>,
}

impl Emit<'_> {
    /// Allocate the next `%vN` number (for an alloca or an instruction dest).
    fn fresh(&mut self) -> u32 {
        let n = self.next;
        self.next += 1;
        n
    }

    /// A fresh basic-block label number (`bbN`); `entry` is the implicit first.
    fn fresh_block(&mut self) -> u32 {
        let b = self.block;
        self.block += 1;
        b
    }

    /// Reserve an alloca slot, emitting its `alloca` line into the HOISTED entry
    /// buffer; the slot `%vN` is referenced by stores/loads in the body.
    fn alloca(&mut self, ty: &str) -> u32 {
        let s = self.fresh();
        writeln!(self.allocas, "  %v{s} = alloca {ty}").unwrap();
        s
    }

    /// The LLVM type for `ty` in the body walk — forwards to the program-aware
    /// `llvm_ty` (which strips a top-level `secret T` and renders `GenericInstance`
    /// structurally). Used for every expression/binding/operand type, any of which
    /// may carry a `secret` qualifier or be a generic instance.
    fn lty(&self, ty: Type) -> Result<String, String> {
        llvm_ty(ty, self.program)
    }

    fn lower_stmt(&mut self, stmt: &TypedStmt) -> Result<(), String> {
        match &stmt.kind {
            TypedStmtKind::Let { id, ty, value, .. } => {
                let v = self.lower_expr(value)?;
                let llty = self.lty(*ty)?;
                let slot = self.alloca(&llty);
                writeln!(self.body, "  store {llty} {v}, ptr %v{slot}").unwrap();
                self.slots.insert(*id, slot);
                // 8d-drops: record the binding in the current scope frame so it is
                // freed at the block's exit (unless moved out — see emit_scope_drops).
                self.var_ty.insert(*id, *ty);
                self.scopes.last_mut().expect("a scope frame is open").push(*id);
                Ok(())
            }
            TypedStmtKind::Assign { target, value } => match &target.kind {
                TypedExprKind::Var(id) => {
                    // The Var target is just a slot (no emission), so lowering the
                    // value then storing matches the Sentinel's target-then-value
                    // order (its suppressed target walk emits nothing).
                    let slot = *self.slots.get(id).ok_or("assign to an unbound var")?;
                    let v = self.lower_expr(value)?;
                    let llty = self.lty(target.ty)?;
                    writeln!(self.body, "  store {llty} {v}, ptr %v{slot}").unwrap();
                    Ok(())
                }
                // `*r = x` / `(*c).f = x` / `a[i] = x` (ADR 0050) — the target pointer
                // (r's value, the field GEP, or the bounds-checked element GEP) is
                // emitted FIRST, then the value, then the store (the Sentinel walks
                // target-then-value, and the deref/field/index target DOES emit).
                TypedExprKind::Unary(UnaryOp::Deref, _)
                | TypedExprKind::FieldAccess { .. }
                | TypedExprKind::Index { .. } => {
                    let ptr = self.lower_lvalue_ptr(target)?;
                    let v = self.lower_expr(value)?;
                    let llty = self.lty(target.ty)?;
                    writeln!(self.body, "  store {llty} {v}, ptr {ptr}").unwrap();
                    Ok(())
                }
                _ => Err("assign to a non-Var/deref lvalue (deferred to a later slice)".into()),
            },
            TypedStmtKind::Expr(e) => {
                // Statement-position expr: lower for effect, discard the value.
                self.lower_expr(e)?;
                Ok(())
            }
            TypedStmtKind::While { cond, body } => {
                // The loop CFG: enter the cond; cond branches to body or after; the
                // body (per-iteration) branches back to cond (the back-edge). Body
                // allocas are hoisted to entry (no per-iteration stack growth).
                let cond_b = self.fresh_block();
                let body_b = self.fresh_block();
                let after_b = self.fresh_block();
                writeln!(self.body, "  br label %bb{cond_b}").unwrap();
                writeln!(self.body, "bb{cond_b}:").unwrap();
                let c = self.lower_expr(cond)?;
                writeln!(self.body, "  br i1 {c}, label %bb{body_b}, label %bb{after_b}").unwrap();
                writeln!(self.body, "bb{body_b}:").unwrap();
                // 8d-drops-3: scope_floor = the body frame's index, captured NOW (before
                // lower_block_expr pushes it) so break/continue drain frames >= it.
                self.loops.push((cond_b, after_b, self.scopes.len()));
                let _ = self.lower_block_expr(body)?; // a while body's value is discarded
                self.loops.pop();
                writeln!(self.body, "  br label %bb{cond_b}").unwrap();
                writeln!(self.body, "bb{after_b}:").unwrap();
                Ok(())
            }
            TypedStmtKind::Break | TypedStmtKind::Continue => {
                let (cond_b, after_b, scope_floor) =
                    *self.loops.last().ok_or("break/continue outside a loop")?;
                // 8d-drops-3: the per-iteration drops for every open frame from the top
                // down to the loop body — branching to loop_after/loop_cond skips
                // lower_block_expr's end-of-iteration drops, so a body-scope heap binding
                // live here would leak. (Each runtime path frees once: this early-exit
                // drop, or the fall-through body-end drop — mutually exclusive blocks.)
                self.emit_loop_exit_drops(scope_floor)?;
                let dest = if matches!(stmt.kind, TypedStmtKind::Break) {
                    after_b
                } else {
                    cond_b
                };
                writeln!(self.body, "  br label %bb{dest}").unwrap();
                // The branch terminates this block; park on a fresh (unreachable) block
                // so any trailing code in this source block has somewhere to go.
                let dead = self.fresh_block();
                writeln!(self.body, "bb{dead}:").unwrap();
                Ok(())
            }
        }
    }

    /// Lower an expression, emitting instructions into `self.body`, and
    /// return its **operand** — either a literal (`42`, `0`/`1`) or a
    /// register (`%vN`).
    fn lower_expr(&mut self, expr: &TypedExpr) -> Result<String, String> {
        match &expr.kind {
            TypedExprKind::IntLit(n) => Ok(n.to_string()),
            // ADR 0058: `f64` is not ported to the textual `snc llvm` oracle
            // (snc-side-only this increment). It Errs cleanly, so the selfhost
            // corpus codegen differential SKIPS any f64 fixture (the skip is
            // keyed on the oracle erroring) — scg stays untouched.
            TypedExprKind::FloatLit(_) => Err("float literal not ported (ADR 0058 snc-only)".into()),
            // ADR 0070: a Fn value is snc-only at v1 (the demonstrator lives in
            // `examples/`, never `tests/pass`) — Err cleanly so the selfhost
            // differential SKIPS any fixture using it, mirroring the `f64` /
            // `FloatLit` deferral above (NOT a stub — a deliberate scope cut).
            TypedExprKind::FnRef(_) => Err("Fn value not ported to the snc llvm oracle (ADR 0070 snc-only)".into()),
            TypedExprKind::BoolLit(b) => Ok(if *b { "1".into() } else { "0".into() }),
            TypedExprKind::CharLit(b) => Ok(b.to_string()),
            // `secret T` lowers identically to `T` (ADR 0019 D12); declassify
            // is value-level identity. Both just flow the inner operand.
            TypedExprKind::WidenToSecret(inner) | TypedExprKind::Declassify(inner) => {
                self.lower_expr(inner)
            }
            // ADR 0065 stage 4: `return e` — evaluate `e`, drop EVERY live scope
            // frame down to the FUNCTION floor (the ADR 0036 break/continue
            // machinery with floor 0 = "break all the way out"), then `ret` with the
            // SAME ABI as the epilogue (`main` truncates i64→i32; an effecting fn
            // wraps a pure value via `sentinel_kont_pure`; an ordinary fn rets its
            // value). The current block is now terminated, so park the builder on a
            // fresh dead block — the unreachable remainder of the enclosing block /
            // if-arm (its store-to-result + merge branch) lands there, never on the
            // terminated block. Drops rely on the moved set alone (a returned binding
            // is recorded moved, so skipped), the same one-free invariant the
            // loop-exit drops use. CONSTANT-TIME unchanged: `return` is unconditional
            // control flow, not a branch on a value, so it is no new `secret_leak`
            // sink. This is byte-identical to the self-hosted `scg` (the `cg` mode of
            // `dump_texpr` in `selfhost/types.sentinel`).
            TypedExprKind::Return(inner) => {
                let val = self.lower_expr(inner)?;
                // Floor 0: drain ALL open scope frames (params + body + any nested).
                self.emit_loop_exit_drops(0)?;
                let sig = self.program.signature(self.current_fn);
                let is_main = sig.is_main;
                let is_eff = uses_kont_abi(sig, self.program);
                if is_main {
                    let t = self.fresh();
                    writeln!(self.body, "  %v{t} = trunc i64 {val} to i32").unwrap();
                    writeln!(self.body, "  ret i32 %v{t}").unwrap();
                } else if is_eff {
                    // Effecting ABI: wrap the pure value as a PURE_RETURN kont so the
                    // caller's `handle` sees a uniform Kont* (the effect-free path; a
                    // `return` crossing a `handle` is ADR 0065 stage 3).
                    let kp = self.fresh();
                    writeln!(self.body, "  %v{kp} = call ptr @sentinel_kont_pure(i64 {val})").unwrap();
                    self.used.kont_pure = true;
                    writeln!(self.body, "  ret ptr %v{kp}").unwrap();
                } else {
                    // `inner.ty` is the fn's return type (the typer checked it), so
                    // its LLVM type is the epilogue's `ret` type (secret stripped).
                    let rty = self.lty(inner.ty)?;
                    writeln!(self.body, "  ret {rty} {val}").unwrap();
                }
                let dead = self.fresh_block();
                writeln!(self.body, "bb{dead}:").unwrap();
                // `return` is divergent: hand back a typed-zero placeholder operand
                // (only ever stored into the now-dead block).
                Ok("0".to_string())
            }
            TypedExprKind::Cast(inner) => {
                // ADR 0049: integer width conversion — trunc / sext / zext by
                // width (u8 is the only unsigned source → zext when widening);
                // same width is a no-op. (No cast fixture is in the differential
                // corpus until the selfhost mirror lands, so this is unexercised
                // there; it keeps `snc llvm` correct on cast programs.)
                let x = self.lower_expr(inner)?;
                let src = inner.ty.strip_secret(&self.program.secrets).0;
                let dst = expr.ty.strip_secret(&self.program.secrets).0;
                let bits = |t: Type| -> u32 {
                    match t {
                        Type::U8 => 8,
                        Type::I32 => 32,
                        _ => 64,
                    }
                };
                let (sw, dw) = (bits(src), bits(dst));
                if sw == dw {
                    return Ok(x);
                }
                let srcty = self.lty(src)?;
                let dstty = self.lty(dst)?;
                let op = if dw < sw {
                    "trunc"
                } else if matches!(src, Type::U8) {
                    "zext"
                } else {
                    "sext"
                };
                let v = self.fresh();
                writeln!(self.body, "  %v{v} = {op} {srcty} {x} to {dstty}").unwrap();
                Ok(format!("%v{v}"))
            }
            TypedExprKind::Var(id) => {
                let llty = self.lty(expr.ty)?;
                let v = self.fresh();
                // Bar B / classes: `Var(self)` loads the whole `%Class.N` aggregate from
                // the self pointer (`%arg0`); a normal var loads from its alloca slot.
                if self.self_var == Some(*id) {
                    writeln!(self.body, "  %v{v} = load {llty}, ptr %arg0").unwrap();
                } else {
                    let slot = *self.slots.get(id).ok_or("read of an unbound var")?;
                    writeln!(self.body, "  %v{v} = load {llty}, ptr %v{slot}").unwrap();
                }
                Ok(format!("%v{v}"))
            }
            TypedExprKind::Unary(UnaryOp::Neg, inner) => {
                let x = self.lower_expr(inner)?;
                let llty = self.lty(expr.ty)?;
                let v = self.fresh();
                // LLVM has no int `neg`; inkwell's build_int_neg = `sub 0, x`.
                writeln!(self.body, "  %v{v} = sub {llty} 0, {x}").unwrap();
                Ok(format!("%v{v}"))
            }
            TypedExprKind::Unary(UnaryOp::Not, inner) => {
                // `!b` ≡ `b xor 1` on i1 (type-check guarantees bool).
                let x = self.lower_expr(inner)?;
                let v = self.fresh();
                writeln!(self.body, "  %v{v} = xor i1 {x}, 1").unwrap();
                Ok(format!("%v{v}"))
            }
            // ADR 0058: `sqrt` (and `f64`) are not ported to the textual oracle
            // (snc-only). Erring keeps the selfhost corpus differential skipping
            // any f64 fixture.
            TypedExprKind::Unary(UnaryOp::Sqrt, _) => {
                Err("sqrt not ported (ADR 0058 snc-only)".into())
            }
            // ADR 0057 Phase 1b: `ptr_of` / `ptr_of_mut` (and `ptr` / `extern`)
            // are not ported to the textual oracle (snc-only). Erring keeps the
            // selfhost corpus differential skipping any FFI fixture.
            TypedExprKind::Unary(UnaryOp::PtrOf | UnaryOp::PtrOfMut | UnaryOp::IsNull, _) => {
                Err("ptr_of / is_null not ported (ADR 0057 snc-only)".into())
            }
            // `&x` / `&mut x` → a pointer to x's storage (its lvalue): for a `Var`,
            // its alloca slot — no instruction. LLVM ignores mutability.
            TypedExprKind::Unary(UnaryOp::Ref, inner)
            | TypedExprKind::Unary(UnaryOp::RefMut, inner) => self.lower_lvalue_ptr(inner),
            // `*r` (rvalue) → load the pointee through r's pointer value.
            TypedExprKind::Unary(UnaryOp::Deref, inner) => {
                let ptr = self.lower_expr(inner)?;
                let pointee = match inner.ty {
                    Type::Ref(id) => self.program.refs[id.0 as usize].inner,
                    _ => return Err("deref of a non-ref (deferred)".into()),
                };
                let llty = self.lty(pointee)?;
                let v = self.fresh();
                writeln!(self.body, "  %v{v} = load {llty}, ptr {ptr}").unwrap();
                Ok(format!("%v{v}"))
            }
            TypedExprKind::Binary(op, lhs, rhs) => {
                let l = self.lower_expr(lhs)?;
                let r = self.lower_expr(rhs)?;
                let llty = self.lty(expr.ty)?;
                // `u8` is unsigned → `/` is `udiv` (ADR 0033 D6); the rest are
                // sign-agnostic in two's complement.
                let is_unsigned = matches!(lhs.ty, Type::U8);
                let opcode = match op {
                    BinOp::Add => "add",
                    BinOp::Sub => "sub",
                    BinOp::Mul => "mul",
                    BinOp::Div if is_unsigned => "udiv",
                    BinOp::Div => "sdiv",
                    BinOp::BitAnd => "and",
                    BinOp::BitOr => "or",
                    BinOp::BitXor => "xor",
                    // ADR 0048: `>>` is LOGICAL (zero-fill) for all int types.
                    // (Phase 2 / full oracle: a mismatched-width shift amount
                    // would need a trunc/zext here to match the inkwell backend
                    // byte-for-byte; no shift fixture is in the differential
                    // corpus yet, so the corpus emission is unaffected.)
                    BinOp::Shl => "shl",
                    BinOp::Shr => "lshr",
                };
                let v = self.fresh();
                writeln!(self.body, "  %v{v} = {opcode} {llty} {l}, {r}").unwrap();
                Ok(format!("%v{v}"))
            }
            TypedExprKind::Cmp(op, lhs, rhs) => {
                let l = self.lower_expr(lhs)?;
                let r = self.lower_expr(rhs)?;
                let is_unsigned = matches!(lhs.ty, Type::U8);
                let pred = match op {
                    CmpOp::Eq => "eq",
                    CmpOp::Ne => "ne",
                    CmpOp::Lt => if is_unsigned { "ult" } else { "slt" },
                    CmpOp::Le => if is_unsigned { "ule" } else { "sle" },
                    CmpOp::Gt => if is_unsigned { "ugt" } else { "sgt" },
                    CmpOp::Ge => if is_unsigned { "uge" } else { "sge" },
                };
                // ADR 0014 D7: `x == null` / `x != null` (the only type-legal nullable
                // comparisons) compare the i1 discriminators, not the payloads. Extract
                // the valid bit (field 0) from each side and `icmp` those.
                if lhs.ty.is_nullable() || rhs.ty.is_nullable() {
                    let lty = self.lty(lhs.ty)?;
                    let rty = self.lty(rhs.ty)?;
                    let lv = self.fresh();
                    writeln!(self.body, "  %v{lv} = extractvalue {lty} {l}, 0").unwrap();
                    let rv = self.fresh();
                    writeln!(self.body, "  %v{rv} = extractvalue {rty} {r}, 0").unwrap();
                    let v = self.fresh();
                    writeln!(self.body, "  %v{v} = icmp {pred} i1 %v{lv}, %v{rv}").unwrap();
                    return Ok(format!("%v{v}"));
                }
                let llty = self.lty(lhs.ty)?;
                let v = self.fresh();
                writeln!(self.body, "  %v{v} = icmp {pred} {llty} {l}, {r}").unwrap();
                Ok(format!("%v{v}"))
            }
            // A value-block `{ stmts; tail }` needs no new basic block (only
            // if/while do) — lower its stmts, return its tail operand.
            TypedExprKind::Block(b) => self.lower_block_expr(b),
            // `if c { t } else { e }` — the no-phi memory-cell merge: a hoisted
            // result slot, a conditional branch, each arm storing its value into
            // the slot, and a load at the merge. The result type is the (precomputed)
            // then-branch type; the slot is reserved AFTER the then walk so the
            // Sentinel side (which learns the type only then) numbers it identically.
            TypedExprKind::If { cond, then_branch, else_branch } => {
                let c = self.lower_expr(cond)?;
                let then_b = self.fresh_block();
                let else_b = self.fresh_block();
                let merge_b = self.fresh_block();
                writeln!(self.body, "  br i1 {c}, label %bb{then_b}, label %bb{else_b}").unwrap();
                writeln!(self.body, "bb{then_b}:").unwrap();
                let tv = self.lower_block_expr(then_branch)?;
                let rty = self.lty(then_branch.ty)?;
                let slot = self.alloca(&rty);
                writeln!(self.body, "  store {rty} {tv}, ptr %v{slot}").unwrap();
                writeln!(self.body, "  br label %bb{merge_b}").unwrap();
                writeln!(self.body, "bb{else_b}:").unwrap();
                let ev = self.lower_block_expr(else_branch)?;
                writeln!(self.body, "  store {rty} {ev}, ptr %v{slot}").unwrap();
                writeln!(self.body, "  br label %bb{merge_b}").unwrap();
                writeln!(self.body, "bb{merge_b}:").unwrap();
                let d = self.fresh();
                writeln!(self.body, "  %v{d} = load {rty}, ptr %v{slot}").unwrap();
                Ok(format!("%v{d}"))
            }
            // `a && b` / `a || b` — short-circuit, no phi: branch on `a`; the rhs block
            // evaluates `b` and stores it, the short block stores the constant (false for
            // `&&`, true for `||`); the merge loads the i1 result.
            TypedExprKind::Logic(op, lhs, rhs) => {
                let l = self.lower_expr(lhs)?;
                let rhs_b = self.fresh_block();
                let short_b = self.fresh_block();
                let merge_b = self.fresh_block();
                let is_and = matches!(op, LogicOp::And);
                if is_and {
                    writeln!(self.body, "  br i1 {l}, label %bb{rhs_b}, label %bb{short_b}").unwrap();
                } else {
                    writeln!(self.body, "  br i1 {l}, label %bb{short_b}, label %bb{rhs_b}").unwrap();
                }
                writeln!(self.body, "bb{rhs_b}:").unwrap();
                let r = self.lower_expr(rhs)?;
                let slot = self.alloca("i1");
                writeln!(self.body, "  store i1 {r}, ptr %v{slot}").unwrap();
                writeln!(self.body, "  br label %bb{merge_b}").unwrap();
                writeln!(self.body, "bb{short_b}:").unwrap();
                let sc = if is_and { 0 } else { 1 };
                writeln!(self.body, "  store i1 {sc}, ptr %v{slot}").unwrap();
                writeln!(self.body, "  br label %bb{merge_b}").unwrap();
                writeln!(self.body, "bb{merge_b}:").unwrap();
                let d = self.fresh();
                writeln!(self.body, "  %v{d} = load i1, ptr %v{slot}").unwrap();
                Ok(format!("%v{d}"))
            }
            TypedExprKind::Call { id, args, type_args, .. } => {
                self.lower_call(*id, args, type_args, expr.ty)
            }
            // A struct literal builds its aggregate value by an `insertvalue` chain
            // from `undef` (declaration field order; the typed `fields` are already
            // reordered to it). All field operands are lowered FIRST, then the chain
            // is emitted — a collect-then-emit shape (like a call's args) so a
            // side-effecting field value emits before the chain on BOTH backends (the
            // Sentinel side reuses its cg-collect arg stacks). The result is a single
            // SSA aggregate operand, so `let`/`Var`/param/return handle a struct
            // generically (alloca/store/load of `%Struct.N`) — no GEP needed.
            TypedExprKind::StructLit { fields, .. } => {
                let sty = self.lty(expr.ty)?;
                let mut field_ops = Vec::with_capacity(fields.len());
                for fv in fields {
                    let fty = self.lty(fv.ty)?;
                    let op = self.lower_expr(fv)?;
                    field_ops.push((fty, op));
                }
                let mut agg = "undef".to_string();
                for (i, (fty, op)) in field_ops.iter().enumerate() {
                    let v = self.fresh();
                    writeln!(
                        self.body,
                        "  %v{v} = insertvalue {sty} {agg}, {fty} {op}, {i}"
                    )
                    .unwrap();
                    agg = format!("%v{v}");
                }
                Ok(agg)
            }
            // Field read = `extractvalue` on the target aggregate (chained accesses
            // nest naturally — the inner access's result is the outer's operand).
            TypedExprKind::FieldAccess { target, field_index, .. } => {
                let agg = self.lower_expr(target)?;
                let sty = self.lty(target.ty)?;
                let v = self.fresh();
                writeln!(self.body, "  %v{v} = extractvalue {sty} {agg}, {field_index}").unwrap();
                Ok(format!("%v{v}"))
            }
            // An array literal `[e1, …]` heap-allocates `n * sizeof(elem)` bytes
            // (`sentinel_alloc`), GEP-stores each element, and builds the abi-v1
            // `{ i64 len, ptr data }`. Element operands are lowered FIRST (collect-
            // then-emit, like a struct lit). Size = the GEP-sizeof constant idiom
            // (`getelementptr T, null, n` then `ptrtoint`), correct for any element
            // type incl. structs, without replicating layout/padding.
            TypedExprKind::ArrayLit { elem_ty, elements } => {
                let ety = self.lty(self.program.array_elem_type(*elem_ty))?;
                let mut ops = Vec::with_capacity(elements.len());
                for el in elements {
                    ops.push(self.lower_expr(el)?);
                }
                Ok(self.emit_array_buffer(&ety, &ops))
            }
            // A string literal is a `[u8]` (ADR 0033): the decoded bytes heap-copied
            // into a fresh `{ i64, ptr }` — an array literal whose elements are byte
            // constants (the `lower_string_lit` shape; sizeof(u8) = 1 so the buffer is
            // exactly N bytes, but the GEP-sizeof idiom reuses `emit_array_buffer`).
            TypedExprKind::StringLit(bytes) => {
                let ops: Vec<String> = bytes.iter().map(|b| b.to_string()).collect();
                Ok(self.emit_array_buffer("i8", &ops))
            }
            // `a[i]` — extract len(0)/data(1), bounds-check (0 <= i < len), then
            // GEP+load. OOB → `sentinel_panic_oob` + `unreachable`; the OK block
            // continues (subsequent code emits into it).
            TypedExprKind::Index { target, index, elem_ty } => {
                let arr = self.lower_expr(target)?;
                let idx = self.lower_expr(index)?;
                let ety = self.lty(self.program.array_elem_type(*elem_ty))?;
                // `[T]` is `{i64,ptr}` (data field 1); `Vec<T>` is `{i64,i64,ptr}`
                // (data field 2 — capacity sits between). `len` is field 0 for both.
                let aggty = self.lty(target.ty)?;
                let data_field = if matches!(target.ty, Type::Vec(_)) { 2 } else { 1 };
                let len = self.fresh();
                writeln!(self.body, "  %v{len} = extractvalue {aggty} {arr}, 0").unwrap();
                let dat = self.fresh();
                writeln!(self.body, "  %v{dat} = extractvalue {aggty} {arr}, {data_field}").unwrap();
                let ge = self.fresh();
                writeln!(self.body, "  %v{ge} = icmp sge i64 {idx}, 0").unwrap();
                let lt = self.fresh();
                writeln!(self.body, "  %v{lt} = icmp slt i64 {idx}, %v{len}").unwrap();
                let inb = self.fresh();
                writeln!(self.body, "  %v{inb} = and i1 %v{ge}, %v{lt}").unwrap();
                let oob_b = self.fresh_block();
                let ok_b = self.fresh_block();
                writeln!(self.body, "  br i1 %v{inb}, label %bb{ok_b}, label %bb{oob_b}").unwrap();
                writeln!(self.body, "bb{oob_b}:").unwrap();
                writeln!(self.body, "  call void @sentinel_panic_oob(i64 {idx}, i64 %v{len})").unwrap();
                self.used.panic_oob = true;
                writeln!(self.body, "  unreachable").unwrap();
                writeln!(self.body, "bb{ok_b}:").unwrap();
                let ep = self.fresh();
                writeln!(self.body, "  %v{ep} = getelementptr {ety}, ptr %v{dat}, i64 {idx}").unwrap();
                let v = self.fresh();
                writeln!(self.body, "  %v{v} = load {ety}, ptr %v{ep}").unwrap();
                Ok(format!("%v{v}"))
            }
            // 8e-1: enum construction — `{ i32 tag, ptr payload }`. The payload is null
            // for a unit variant, else a heap-boxed struct of the variant's payload
            // fields (built from the lowered args).
            TypedExprKind::EnumConstruct { enum_id, variant_index, args, .. } => {
                self.lower_enum_construct(*enum_id, *variant_index, args)
            }
            // 8e-2: `match` lowers to an if-else chain over the variant arms (binding
            // each arm's payload fields), a memory-cell result merge (no phi), and the
            // `_` wildcard (or `unreachable`) as the final else.
            TypedExprKind::Match { scrutinee, enum_id, arms } => {
                self.lower_match(scrutinee, *enum_id, arms, expr.ty)
            }
            // `null` (ADR 0014 D2) is the constant `{ i1 0, <zero payload> }` — a
            // constant aggregate operand (no instruction), mirroring inkwell's
            // `const_named_struct`. `?primitive` zeroes its payload (`iN 0`);
            // `?Struct`/`?GI` (heap-indirected) use a null pointer.
            TypedExprKind::NullLit => {
                let inner = match expr.ty {
                    Type::Nullable(ni) => ni,
                    _ => return Err("NullLit type is not nullable".into()),
                };
                let payload = match inner {
                    NullableInner::Struct(_) | NullableInner::GenericInstance(_) => {
                        "ptr null".to_string()
                    }
                    _ => format!("{} 0", self.lty(inner.to_type())?),
                };
                Ok(format!("{{ i1 0, {payload} }}"))
            }
            // `T → ?T` widening (ADR 0014 D3): wrap the inner value as `{ i1 1, T }`
            // via an `insertvalue` chain from `undef` (mirroring inkwell
            // `build_insert_value`). The `?Struct`/`?GI` heap-box path (alloc + store +
            // pointer payload) is unexercised by the corpus → deferred with an Err.
            TypedExprKind::WidenToNullable(inner) => {
                let ni = match expr.ty {
                    Type::Nullable(ni) => ni,
                    _ => return Err("WidenToNullable type is not nullable".into()),
                };
                if matches!(ni, NullableInner::Struct(_) | NullableInner::GenericInstance(_)) {
                    return Err("widen-to-nullable of a struct/generic payload (heap-box) deferred".into());
                }
                let sty = self.lty(expr.ty)?;
                let pty = self.lty(inner.ty)?;
                let payload = self.lower_expr(inner)?;
                let a0 = self.fresh();
                writeln!(self.body, "  %v{a0} = insertvalue {sty} undef, i1 1, 0").unwrap();
                let a1 = self.fresh();
                writeln!(self.body, "  %v{a1} = insertvalue {sty} %v{a0}, {pty} {payload}, 1").unwrap();
                Ok(format!("%v{a1}"))
            }
            // Bar B / classes — `Name::init(args)` (ADR 0022 D5). Alloca the class
            // storage, call `Name__init(out_ptr, args)` (it writes through `out_ptr`),
            // then LOAD the constructed value (so a `let`/param/return handles a class
            // generically as the `%Class.N` aggregate). Args lowered after the alloca,
            // before the call (matching the inkwell order).
            TypedExprKind::ClassInit { id, name, args, .. } => {
                let cty = self.lty(expr.ty)?;
                let slot = self.alloca(&cty);
                let arg_ops = self.lower_args(args)?;
                write!(self.body, "  call void @{name}__init(ptr %v{slot}").unwrap();
                for (t, op) in &arg_ops {
                    write!(self.body, ", {t} {op}").unwrap();
                }
                self.body.push_str(")\n");
                let _ = id;
                let v = self.fresh();
                writeln!(self.body, "  %v{v} = load {cty}, ptr %v{slot}").unwrap();
                Ok(format!("%v{v}"))
            }
            // Bar B / classes — postfix method call `recv.m(args)` (ADR 0022 D7). The
            // receiver is an lvalue; pass its pointer as `self`, then the args. Symbol
            // `Class__method` (class NAME, like the inkwell backend).
            TypedExprKind::MethodCall { target, class_id, method, args, .. } => {
                let self_ptr = self.lower_lvalue_ptr(target)?;
                let arg_ops = self.lower_args(args)?;
                let ret = self.lty(expr.ty)?;
                let cls = self.program.class_decl(*class_id).name.clone();
                let v = self.fresh();
                write!(self.body, "  %v{v} = call {ret} @{cls}__{method}(ptr {self_ptr}").unwrap();
                for (t, op) in &arg_ops {
                    write!(self.body, ", {t} {op}").unwrap();
                }
                self.body.push_str(")\n");
                Ok(format!("%v{v}"))
            }
            // Bar B / classes — receiver-typed dispatch to an impl method (ADR 0023 D5
            // Path 1). Same self-ptr ABI as a class MethodCall, but the symbol is the
            // impl-method mangle. Also the shape the delegate-synthesized impl bodies use.
            TypedExprKind::ImplMethodCall { target, impl_id, method, args, .. } => {
                let self_ptr = self.lower_lvalue_ptr(target)?;
                let arg_ops = self.lower_args(args)?;
                let ret = self.lty(expr.ty)?;
                let sym = mangle_impl_method(self.program.impl_decl(*impl_id), method);
                let v = self.fresh();
                write!(self.body, "  %v{v} = call {ret} @{sym}(ptr {self_ptr}").unwrap();
                for (t, op) in &arg_ops {
                    write!(self.body, ", {t} {op}").unwrap();
                }
                self.body.push_str(")\n");
                Ok(format!("%v{v}"))
            }
            // Bar B / classes — qualified-named dispatch `Impl::m(&mut recv, args)` (ADR
            // 0023 D5 Path 2). `args[0]` IS the receiver as a ref-typed expr (lowered to a
            // `ptr` value), passed as `self`; `args[1..]` are the declared params.
            TypedExprKind::QualifiedCall { impl_id, method, args, .. } => {
                let self_val = self.lower_expr(&args[0])?;
                let arg_ops = self.lower_args(&args[1..])?;
                let ret = self.lty(expr.ty)?;
                let sym = mangle_impl_method(self.program.impl_decl(*impl_id), method);
                let v = self.fresh();
                write!(self.body, "  %v{v} = call {ret} @{sym}(ptr {self_val}").unwrap();
                for (t, op) in &arg_ops {
                    write!(self.body, ", {t} {op}").unwrap();
                }
                self.body.push_str(")\n");
                Ok(format!("%v{v}"))
            }
            // Bar B / effects (ADR 0020) — `perform Eff.op(args)` raises a continuation:
            // `call ptr @sentinel_perform_op(i32 <op_id>, i64 <arg|0>)`. The result is a
            // kont* that flows up to the enclosing `handle`'s dispatch. (C3.5(a) single-arg.)
            TypedExprKind::Perform { effect_id, op_index, args, .. } => {
                // Bar B / effects (c35d): inside an embedded-shape RESUMER the unique
                // perform was reified by the parent — its result arrives as the resumed
                // value at the placeholder slot. Emit a load (the substituted-placeholder
                // equivalent), NOT a perform call; the args were the PARENT's to lower.
                if let Some(slot) = self.embed_ph {
                    let v = self.fresh();
                    writeln!(self.body, "  %v{v} = load i64, ptr %v{slot}").unwrap();
                    return Ok(format!("%v{v}"));
                }
                let op_id = encode_op_id(*effect_id, *op_index);
                let arg = if args.is_empty() {
                    "0".to_string()
                } else {
                    self.lower_expr(&args[0])?
                };
                let v = self.fresh();
                writeln!(self.body, "  %v{v} = call ptr @sentinel_perform_op(i32 {op_id}, i64 {arg})").unwrap();
                self.used.perform_op = true;
                Ok(format!("%v{v}"))
            }
            // Bar B / effects — `handle <body> with { arms }`: the dispatch loop.
            TypedExprKind::Handle { body, arms, return_arm, .. } => {
                self.lower_handle(body, arms, return_arm.as_deref())
            }
            // Bar B / effects — `k(v)` resumes a continuation (inside a handler arm).
            TypedExprKind::ResumeKont { kont, args, .. } => {
                self.lower_resume_kont(*kont, args)
            }
            // Bar B / concurrency (ADR 0024) — `scope concurrent { body }`: enter a scope
            // ctx (so `spawn`s inside register with it), lower the body, exit the scope
            // (which joins/awaits any unawaited tasks). The block's value is the result.
            TypedExprKind::Scope { body, .. } => {
                let sc = self.fresh();
                writeln!(self.body, "  %v{sc} = call ptr @sentinel_scope_enter()").unwrap();
                self.used.scope_enter = true;
                let prev = self.current_scope;
                self.current_scope = Some(sc);
                let bv = self.lower_block_expr(body)?;
                self.current_scope = prev;
                writeln!(self.body, "  call void @sentinel_scope_exit(ptr %v{sc})").unwrap();
                self.used.scope_exit = true;
                Ok(bv)
            }
            // Bar B / concurrency — `spawn fn(args)`: pack the args into a heap buffer
            // (one i64 slot each), spawn a Task via the per-target `__spawn_wrapper_<id>`,
            // and (inside a scope) register the Task so the scope owns it.
            TypedExprKind::Spawn { call, .. } => {
                let (callee_id, args) = match &call.kind {
                    TypedExprKind::Call { id, args, .. } => (*id, args),
                    _ => return Err("spawn target must be a direct fn call".into()),
                };
                let n = args.len();
                let size = n.max(1) * 8;
                // ADR 0066 M1.2b: evaluate ALL args BEFORE allocating the packed-
                // args struct, then GEP+store — matching the selfhost cg's
                // collect-then-store order (and inkwell's). A non-constant arg
                // (e.g. a copy-var `Channel` endpoint, whose eval emits a `load`)
                // must precede the alloc to stay byte-identical with `scg`; a
                // constant arg emits no instruction, so this is unchanged there.
                let mut lowered: Vec<(String, String)> = Vec::with_capacity(n);
                for arg in args.iter() {
                    let v = self.lower_expr(arg)?;
                    // ADR 0066 M1.1: store the arg with its real type (was i64).
                    let aty = self.lty(arg.ty)?;
                    lowered.push((aty, v));
                }
                let st = self.fresh();
                writeln!(self.body, "  %v{st} = call ptr @sentinel_alloc(i64 {size})").unwrap();
                self.used.alloc = true;
                for (i, (aty, v)) in lowered.into_iter().enumerate() {
                    let off = i * 8;
                    let gp = self.fresh();
                    writeln!(self.body, "  %v{gp} = getelementptr i8, ptr %v{st}, i64 {off}").unwrap();
                    writeln!(self.body, "  store {aty} {v}, ptr %v{gp}").unwrap();
                }
                let task = self.fresh();
                writeln!(
                    self.body,
                    "  %v{task} = call ptr @sentinel_task_spawn(ptr @__spawn_wrapper_{}, ptr %v{st}, i64 {size})",
                    callee_id.0
                )
                .unwrap();
                self.used.task_spawn = true;
                if let Some(sc) = self.current_scope {
                    writeln!(self.body, "  call void @sentinel_scope_register(ptr %v{sc}, ptr %v{task})").unwrap();
                    self.used.scope_register = true;
                }
                Ok(format!("%v{task}"))
            }
            // Bar B / concurrency — `task.await`: join the task + read its i64 result,
            // then DECODE it back to the Task's result type (ADR 0066 M1.1 — the inverse
            // of the wrapper's encode; i64 is a no-op, byte-identical to C4.4).
            TypedExprKind::Await { task_expr, .. } => {
                let t = self.lower_expr(task_expr)?;
                let r = self.fresh();
                writeln!(self.body, "  %v{r} = call i64 @sentinel_task_await(ptr {t})").unwrap();
                self.used.task_await = true;
                let conv = match expr.ty {
                    Type::I64 => None,
                    Type::I32 => Some(("trunc", "i32")),
                    Type::U8 => Some(("trunc", "i8")),
                    Type::Bool => Some(("trunc", "i1")),
                    Type::F64 => Some(("bitcast", "double")),
                    Type::Ptr | Type::Task(_) => Some(("inttoptr", "ptr")),
                    other => {
                        return Err(format!(
                            "await result type not yet ported (ADR 0066 M1.1 word-scalar): {other:?}"
                        ));
                    }
                };
                match conv {
                    None => Ok(format!("%v{r}")),
                    Some((op, to)) => {
                        let d = self.fresh();
                        writeln!(self.body, "  %v{d} = {op} i64 %v{r} to {to}").unwrap();
                        Ok(format!("%v{d}"))
                    }
                }
            }
        }
    }

    /// 8e-2: lower `match scrutinee { arms }` (ADR 0032 D5). Extract the scrutinee's
    /// tag + payload ptr, then an IF-ELSE CHAIN over the variant arms: per arm, `icmp
    /// eq tag, vidx` → branch to the arm block (bind the payload fields, lower the body,
    /// store the result, branch to merge) or the next check. The final else is the `_`
    /// wildcard body, or `unreachable` (exhaustiveness is a type-check guarantee). The
    /// merge loads the result (the if-merge memory cell, no phi).
    ///
    /// An if-else chain rather than the production's `switch` (behaviourally identical
    /// for disjoint variants — `cc`-run == inkwell) because it lowers in a SINGLE pass:
    /// the Sentinel walks the arm cons-list consuming, with no slice to pre-scan for the
    /// switch's up-front case-block table. The two stay byte-identical.
    fn lower_match(
        &mut self,
        scrutinee: &TypedExpr,
        enum_id: EnumId,
        arms: &[TypedMatchArm],
        result_ty: Type,
    ) -> Result<String, String> {
        let scrut = self.lower_expr(scrutinee)?;
        let tag = self.fresh();
        writeln!(self.body, "  %v{tag} = extractvalue {{ i32, ptr }} {scrut}, 0").unwrap();
        let payload = self.fresh();
        writeln!(self.body, "  %v{payload} = extractvalue {{ i32, ptr }} {scrut}, 1").unwrap();
        let rty = self.lty(result_ty)?;
        let result = self.alloca(&rty); // hoisted result slot (the if-merge memory cell)
        let merge_b = self.fresh_block();

        // The variant arms become the chain; a `_` wildcard (if any) is the final else.
        let mut wildcard: Option<&TypedMatchArm> = None;
        for arm in arms {
            match &arm.pattern {
                TypedPattern::Variant { variant_index, bindings, .. } => {
                    let cmp = self.fresh();
                    writeln!(self.body, "  %v{cmp} = icmp eq i32 %v{tag}, {variant_index}").unwrap();
                    let arm_b = self.fresh_block();
                    let next_b = self.fresh_block();
                    writeln!(self.body, "  br i1 %v{cmp}, label %bb{arm_b}, label %bb{next_b}").unwrap();
                    writeln!(self.body, "bb{arm_b}:").unwrap();
                    self.bind_pattern_payloads(payload, enum_id, *variant_index, bindings)?;
                    let v = self.lower_expr(&arm.body)?;
                    writeln!(self.body, "  store {rty} {v}, ptr %v{result}").unwrap();
                    writeln!(self.body, "  br label %bb{merge_b}").unwrap();
                    writeln!(self.body, "bb{next_b}:").unwrap();
                }
                TypedPattern::Wildcard(_) => wildcard = Some(arm),
            }
        }
        // The final else (the last `next_b` block): the wildcard body, or `unreachable`.
        if let Some(arm) = wildcard {
            let v = self.lower_expr(&arm.body)?;
            writeln!(self.body, "  store {rty} {v}, ptr %v{result}").unwrap();
            writeln!(self.body, "  br label %bb{merge_b}").unwrap();
        } else {
            writeln!(self.body, "  unreachable").unwrap();
        }

        writeln!(self.body, "bb{merge_b}:").unwrap();
        let loaded = self.fresh();
        writeln!(self.body, "  %v{loaded} = load {rty}, ptr %v{result}").unwrap();
        Ok(format!("%v{loaded}"))
    }

    /// 8e-2: bind a variant pattern's payload fields into the arm's locals — GEP each
    /// field of the heap-boxed payload struct, load it into a fresh (hoisted) alloca
    /// slot keyed by the binding's VarId, so the arm body's `Var(binding)` reads it. A
    /// `_` binding still gets a slot (positional; never read). NOT added to a scope
    /// drop frame: a heap-typed binding aliases the box's buffer (owned + freed by the
    /// scrutinee's enum drop), so dropping it would double-free (matches production).
    fn bind_pattern_payloads(
        &mut self,
        payload: u32,
        enum_id: EnumId,
        variant_index: usize,
        bindings: &[TypedPatternBinding],
    ) -> Result<(), String> {
        if bindings.is_empty() {
            return Ok(());
        }
        let prog = self.program;
        let payload_tys: Vec<Type> =
            prog.enum_data(enum_id).variants[variant_index].payloads.clone();
        let mut field_lls = Vec::with_capacity(payload_tys.len());
        for t in &payload_tys {
            field_lls.push(self.lty(*t)?);
        }
        let pstruct = format!("{{ {} }}", field_lls.join(", "));
        for (i, b) in bindings.iter().enumerate() {
            let fty = self.lty(b.ty)?;
            let fp = self.fresh();
            writeln!(self.body, "  %v{fp} = getelementptr {pstruct}, ptr %v{payload}, i32 0, i32 {i}").unwrap();
            let ld = self.fresh();
            writeln!(self.body, "  %v{ld} = load {fty}, ptr %v{fp}").unwrap();
            let slot = self.alloca(&fty);
            writeln!(self.body, "  store {fty} %v{ld}, ptr %v{slot}").unwrap();
            self.slots.insert(b.var_id, slot);
            self.var_ty.insert(b.var_id, b.ty);
        }
        Ok(())
    }

    /// 8e-1: lower an enum construction to `{ i32 tag, ptr payload }` (ADR 0032 D4). A
    /// unit variant gets a null payload; a payload variant builds the payload struct
    /// `{ <field tys> }` from the args (`insertvalue` chain), heap-boxes it
    /// (GEP-sizeof + `sentinel_alloc` + store), and points the enum at it. The args are
    /// lowered FIRST (matching the Sentinel's collect-then-emit), so a side-effecting
    /// arg's instructions land before the payload chain on both backends.
    fn lower_enum_construct(
        &mut self,
        enum_id: EnumId,
        variant_index: usize,
        args: &[TypedExpr],
    ) -> Result<String, String> {
        let payload: String = if args.is_empty() {
            "null".to_string()
        } else {
            let prog = self.program;
            let payload_tys: Vec<Type> =
                prog.enum_data(enum_id).variants[variant_index].payloads.clone();
            let mut field_lls = Vec::with_capacity(payload_tys.len());
            for t in &payload_tys {
                field_lls.push(self.lty(*t)?);
            }
            let pstruct = format!("{{ {} }}", field_lls.join(", "));
            // Lower the args, then build the payload aggregate (declaration order).
            let mut ops = Vec::with_capacity(args.len());
            for a in args {
                ops.push((self.lty(a.ty)?, self.lower_expr(a)?));
            }
            let mut agg = "undef".to_string();
            for (i, (fty, op)) in ops.iter().enumerate() {
                let v = self.fresh();
                writeln!(self.body, "  %v{v} = insertvalue {pstruct} {agg}, {fty} {op}, {i}").unwrap();
                agg = format!("%v{v}");
            }
            // Heap-box the payload struct (sizeof via the GEP-sizeof idiom).
            let sz = self.fresh();
            writeln!(self.body, "  %v{sz} = getelementptr {pstruct}, ptr null, i64 1").unwrap();
            let szi = self.fresh();
            writeln!(self.body, "  %v{szi} = ptrtoint ptr %v{sz} to i64").unwrap();
            let boxp = self.fresh();
            writeln!(self.body, "  %v{boxp} = call ptr @sentinel_alloc(i64 %v{szi})").unwrap();
            self.used.alloc = true;
            writeln!(self.body, "  store {pstruct} {agg}, ptr %v{boxp}").unwrap();
            format!("%v{boxp}")
        };
        let e0 = self.fresh();
        writeln!(self.body, "  %v{e0} = insertvalue {{ i32, ptr }} undef, i32 {variant_index}, 0").unwrap();
        let e1 = self.fresh();
        writeln!(self.body, "  %v{e1} = insertvalue {{ i32, ptr }} %v{e0}, ptr {payload}, 1").unwrap();
        Ok(format!("%v{e1}"))
    }

    /// The pointer operand for an lvalue (a place): a `Var`'s alloca slot, the value of
    /// a deref's inner ref (`&*r` / the `*r = …` target), or a struct field's address
    /// (`&mut (*c).f` — the selfhost sources' pervasive `&mut`-into-a-`ctx`-field form).
    /// Element-ref lvalues (`&a[i]`) are a later slice (the selfhost sources don't use them).
    fn lower_lvalue_ptr(&mut self, expr: &TypedExpr) -> Result<String, String> {
        match &expr.kind {
            TypedExprKind::Var(id) => {
                // Bar B / classes: the lvalue of `self` IS the self pointer (`%arg0`) —
                // no alloca. So `self.f = x` GEPs from `%arg0` and a method call on
                // `self` (`self.manhattan()`) passes `%arg0` straight through.
                if self.self_var == Some(*id) {
                    return Ok("%arg0".to_string());
                }
                let slot = *self.slots.get(id).ok_or("address-of an unbound var")?;
                Ok(format!("%v{slot}"))
            }
            TypedExprKind::Unary(UnaryOp::Deref, inner) => self.lower_expr(inner),
            // 8f-3: `&(target).f` — GEP into the target's lvalue pointer. For `&mut
            // (*c).f` the target is `*c` (a Deref → c's value, a struct pointer). The
            // GEP type is the target struct (`%Struct.N`), field `field_index`.
            TypedExprKind::FieldAccess { target, field_index, .. } => {
                let target_ptr = self.lower_lvalue_ptr(target)?;
                let struct_ll = self.lty(target.ty)?;
                let fp = self.fresh();
                writeln!(
                    self.body,
                    "  %v{fp} = getelementptr {struct_ll}, ptr {target_ptr}, i32 0, i32 {field_index}"
                )
                .unwrap();
                Ok(format!("%v{fp}"))
            }
            // ADR 0050: `a[i] = v;` — the lvalue address is the bounds-checked element
            // GEP: the read-index sequence (extract len(0)/data(1 for `[T]`, 2 for
            // `Vec<T>`), check `0 <= i < len`, `sentinel_panic_oob` on miss) MINUS the
            // load. Returns the element pointer; the `Assign` caller stores through it.
            TypedExprKind::Index { target, index, elem_ty } => {
                let arr = self.lower_expr(target)?;
                let idx = self.lower_expr(index)?;
                let ety = self.lty(self.program.array_elem_type(*elem_ty))?;
                let aggty = self.lty(target.ty)?;
                let data_field = if matches!(target.ty, Type::Vec(_)) { 2 } else { 1 };
                let len = self.fresh();
                writeln!(self.body, "  %v{len} = extractvalue {aggty} {arr}, 0").unwrap();
                let dat = self.fresh();
                writeln!(self.body, "  %v{dat} = extractvalue {aggty} {arr}, {data_field}").unwrap();
                let ge = self.fresh();
                writeln!(self.body, "  %v{ge} = icmp sge i64 {idx}, 0").unwrap();
                let lt = self.fresh();
                writeln!(self.body, "  %v{lt} = icmp slt i64 {idx}, %v{len}").unwrap();
                let inb = self.fresh();
                writeln!(self.body, "  %v{inb} = and i1 %v{ge}, %v{lt}").unwrap();
                let oob_b = self.fresh_block();
                let ok_b = self.fresh_block();
                writeln!(self.body, "  br i1 %v{inb}, label %bb{ok_b}, label %bb{oob_b}").unwrap();
                writeln!(self.body, "bb{oob_b}:").unwrap();
                writeln!(self.body, "  call void @sentinel_panic_oob(i64 {idx}, i64 %v{len})").unwrap();
                self.used.panic_oob = true;
                writeln!(self.body, "  unreachable").unwrap();
                writeln!(self.body, "bb{ok_b}:").unwrap();
                let ep = self.fresh();
                writeln!(self.body, "  %v{ep} = getelementptr {ety}, ptr %v{dat}, i64 {idx}").unwrap();
                Ok(format!("%v{ep}"))
            }
            _ => Err("address-of a non-place (deferred to a later slice)".into()),
        }
    }

    fn lower_block_expr(&mut self, b: &TypedBlock) -> Result<String, String> {
        // 8d-drops: a nested `{ … }` block opens a scope frame whose locals are freed
        // at its exit (after the tail value is computed, before the block's value is
        // used by the parent). Moved-out / tail-returned bindings are in
        // `moved_sources` and skipped (the body tail is walked consuming).
        self.scopes.push(Vec::new());
        for stmt in &b.stmts {
            self.lower_stmt(stmt)?;
        }
        let val = self.lower_expr(&b.tail)?;
        self.emit_scope_drops()?;
        self.scopes.pop();
        Ok(val)
    }

    /// 8d-drops: free the un-moved heap-backed bindings of the top scope frame, in
    /// reverse declaration order. A binding in the fn's `moved_sources` is owned +
    /// dropped by its consumer — and that set already contains tail-returned
    /// bindings (the body tail is walked consuming, so returning a `Var` records it
    /// as a move), so skipping `moved` alone avoids every double-free without a
    /// separate tail-returned guard.
    fn emit_scope_drops(&mut self) -> Result<(), String> {
        if self.scopes.is_empty() {
            return Ok(());
        }
        let top = self.scopes.len() - 1;
        self.emit_frame_drops(top)
    }

    /// 8d-drops: free the un-moved heap-backed bindings of scope frame `idx` in reverse
    /// declaration order. Factored out of `emit_scope_drops` (8d-drops-3) so
    /// `emit_loop_exit_drops` can drain several frames at once.
    fn emit_frame_drops(&mut self, idx: usize) -> Result<(), String> {
        let dp = self.drop_plan;
        let moved = dp.moved_sources_for(self.current_fn);
        // ADR 0046: the partial-move set (Move-typed fields consumed by value); the drop
        // of a partially-moved binding elides these fields (the consumer freed them).
        let moved_fields = dp.moved_fields_for(self.current_fn);
        let scope = self.scopes[idx].clone();
        for &id in scope.iter().rev() {
            if moved.contains(&id) {
                continue;
            }
            let ty = match self.var_ty.get(&id) {
                Some(&t) => t,
                None => continue,
            };
            let slot = match self.slots.get(&id) {
                Some(&s) => s,
                None => continue,
            };
            // ADR 0046: this binding's partially-moved field indices (empty for the
            // common case) — `emit_drop_for_binding` skips them in the struct field walk.
            let id_moved_fields: BTreeSet<u32> = moved_fields
                .iter()
                .filter_map(|(b, f)| (*b == id).then_some(*f))
                .collect();
            self.emit_drop_for_binding(slot, ty, &id_moved_fields)?;
        }
        Ok(())
    }

    /// 8d-drops-3: drain every open scope frame from the current top down to (and
    /// including) the loop-body frame at `scope_floor`, innermost first — the
    /// per-iteration drops on a `break`/`continue` early-exit path. The frames are NOT
    /// popped (the now-dead remainder of each block still pops them via its own
    /// `lower_block_expr`, emitting a second, unreachable drop set), so each runtime
    /// path frees a given binding exactly once.
    fn emit_loop_exit_drops(&mut self, scope_floor: usize) -> Result<(), String> {
        for i in (scope_floor..self.scopes.len()).rev() {
            self.emit_frame_drops(i)?;
        }
        Ok(())
    }

    /// 8d-drops: emit a `sentinel_free` for a value of type `ty` living AT `ptr_reg`
    /// (a `%vN` — an alloca slot, or a struct-field GEP register when recursing). An
    /// array `[T]` (`{ i64, ptr }`) frees its data pointer (field 1); a `Vec<T>`
    /// (`{ i64, i64, ptr }`) frees field 2 (`sentinel_free(null)` is a safe no-op, so
    /// an empty `vec_new()` drops cleanly with no guard); a struct recurses into its
    /// heap-backed fields (8d-drops-2). Primitives / refs have no heap → nothing. The
    /// Bar-B shapes (nullable/enum/class) are later slices; their fixtures don't emit.
    /// ADR 0046: `moved_fields` = field indices of THIS binding that were partially moved
    /// (a Move-typed field consumed by value → owned + freed by the consumer), so they are
    /// elided from the struct field walk below. Empty for a fully-live binding and for
    /// every NESTED (recursive) field drop (deep paths deferred — D5).
    fn emit_drop_for_binding(
        &mut self,
        ptr_reg: u32,
        ty: Type,
        moved_fields: &BTreeSet<u32>,
    ) -> Result<(), String> {
        match ty {
            Type::Array(_) => {
                let v = self.fresh();
                writeln!(self.body, "  %v{v} = load {{ i64, ptr }}, ptr %v{ptr_reg}").unwrap();
                let d = self.fresh();
                writeln!(self.body, "  %v{d} = extractvalue {{ i64, ptr }} %v{v}, 1").unwrap();
                writeln!(self.body, "  call void @sentinel_free(ptr %v{d})").unwrap();
                self.used.free = true;
            }
            Type::Vec(_) => {
                let v = self.fresh();
                writeln!(self.body, "  %v{v} = load {{ i64, i64, ptr }}, ptr %v{ptr_reg}").unwrap();
                let d = self.fresh();
                writeln!(self.body, "  %v{d} = extractvalue {{ i64, i64, ptr }} %v{v}, 2").unwrap();
                writeln!(self.body, "  call void @sentinel_free(ptr %v{d})").unwrap();
                self.used.free = true;
            }
            // 8d-drops-2: a struct owns its heap-backed fields — GEP into each
            // drop-needing field (in declaration order) and recurse. The `ptr_reg`
            // is where the struct lives; field N lives at `gep %Struct.K, ptr, 0, N`.
            Type::Struct(id) => {
                let prog = self.program;
                let field_tys: Vec<Type> = prog.struct_decl(id).fields.iter().map(|f| f.ty).collect();
                for (idx, &fty) in field_tys.iter().enumerate() {
                    if !needs_drop(fty, prog) {
                        continue;
                    }
                    // ADR 0046: a field consumed by value (partial move) is owned + freed
                    // by the consumer — skip it here so we don't double-free.
                    if moved_fields.contains(&(idx as u32)) {
                        continue;
                    }
                    let fp = self.fresh();
                    writeln!(
                        self.body,
                        "  %v{fp} = getelementptr %Struct.{}, ptr %v{ptr_reg}, i32 0, i32 {idx}",
                        id.0
                    )
                    .unwrap();
                    // ADR 0046: nested struct fields carry no tracked moves at the MVP
                    // (deep paths deferred — D5), so the recursive drop gets an empty set.
                    self.emit_drop_for_binding(fp, fty, &BTreeSet::new())?;
                }
            }
            // A generic-struct instance drops like a struct, but its LLVM type is the
            // structural `%<mangled>` name and its field types are the declared fields
            // SUBSTITUTED by the instance args (`Holder<[i64]>` → field 0 is `[i64]`).
            Type::GenericInstance(id) => {
                let prog = self.program;
                let inst = prog.generic_instance(id);
                let struct_id = inst.struct_id;
                let inst_args = inst.args.clone();
                let name = mangle_instance(prog, struct_id, &inst_args);
                let field_tys: Vec<Type> = prog
                    .struct_decl(struct_id)
                    .fields
                    .iter()
                    .map(|f| {
                        let mut insts = prog.generic_instances.clone();
                        let mut refs = prog.refs.clone();
                        f.ty.substitute(&inst_args, &mut insts, &mut refs)
                    })
                    .collect();
                for (idx, &fty) in field_tys.iter().enumerate() {
                    if !needs_drop(fty, prog) {
                        continue;
                    }
                    // ADR 0046: same partial-move skip as the plain-struct arm (the oracle
                    // drops `Struct | GenericInstance` through one field walk).
                    if moved_fields.contains(&(idx as u32)) {
                        continue;
                    }
                    let fp = self.fresh();
                    writeln!(
                        self.body,
                        "  %v{fp} = getelementptr %{name}, ptr %v{ptr_reg}, i32 0, i32 {idx}"
                    )
                    .unwrap();
                    self.emit_drop_for_binding(fp, fty, &BTreeSet::new())?;
                }
            }
            // 8e-1: an enum owns its heap-boxed payload — load `{ i32, ptr }`, and if
            // the payload is non-null, free it. (Box-free-only: a recursive enum's
            // nested boxes are NOT walked — the production's measured D.1b limit.)
            Type::Enum(_) => {
                let v = self.fresh();
                writeln!(self.body, "  %v{v} = load {{ i32, ptr }}, ptr %v{ptr_reg}").unwrap();
                let pl = self.fresh();
                writeln!(self.body, "  %v{pl} = extractvalue {{ i32, ptr }} %v{v}, 1").unwrap();
                let isnull = self.fresh();
                writeln!(self.body, "  %v{isnull} = icmp eq ptr %v{pl}, null").unwrap();
                let free_b = self.fresh_block();
                let after_b = self.fresh_block();
                writeln!(self.body, "  br i1 %v{isnull}, label %bb{after_b}, label %bb{free_b}").unwrap();
                writeln!(self.body, "bb{free_b}:").unwrap();
                writeln!(self.body, "  call void @sentinel_free(ptr %v{pl})").unwrap();
                self.used.free = true;
                writeln!(self.body, "  br label %bb{after_b}").unwrap();
                writeln!(self.body, "bb{after_b}:").unwrap();
            }
            _ => {}
        }
        Ok(())
    }

    /// Emit a heap `[T]` buffer from already-rendered element operands: the
    /// GEP-sizeof size, `sentinel_alloc`, a per-element GEP-store, and the
    /// `{ i64 len, ptr data }` `insertvalue`. Shared by array literals (lowered
    /// element operands) and string literals (constant `i8` byte operands) so the
    /// two cannot drift.
    fn emit_array_buffer(&mut self, ety: &str, ops: &[String]) -> String {
        let n = ops.len();
        let sz = self.fresh();
        writeln!(self.body, "  %v{sz} = getelementptr {ety}, ptr null, i64 {n}").unwrap();
        let szi = self.fresh();
        writeln!(self.body, "  %v{szi} = ptrtoint ptr %v{sz} to i64").unwrap();
        let data = self.fresh();
        writeln!(self.body, "  %v{data} = call ptr @sentinel_alloc(i64 %v{szi})").unwrap();
        self.used.alloc = true;
        for (i, op) in ops.iter().enumerate() {
            let ep = self.fresh();
            writeln!(self.body, "  %v{ep} = getelementptr {ety}, ptr %v{data}, i64 {i}").unwrap();
            writeln!(self.body, "  store {ety} {op}, ptr %v{ep}").unwrap();
        }
        let a0 = self.fresh();
        writeln!(self.body, "  %v{a0} = insertvalue {{ i64, ptr }} undef, i64 {n}, 0").unwrap();
        let a1 = self.fresh();
        writeln!(self.body, "  %v{a1} = insertvalue {{ i64, ptr }} %v{a0}, ptr %v{data}, 1").unwrap();
        format!("%v{a1}")
    }

    /// Bar B / effects (ADR 0020) — `handle <body> with { arms }` as a dispatch loop.
    /// The body produces a kont* (a direct `perform` at c35a); the loop reads the kont's
    /// `op_id` (offset 0) + branches via an **if-else chain** (NOT a `switch` — the
    /// Sentinel side's arm cons-list is single-consumption, so both backends chain for
    /// byte-parity): an arm whose `(EffectId, op_index)` matches binds its op-param
    /// (kont.arg @8) + the continuation + runs its body; a final PURE_RETURN check unwraps
    /// the value. A `k(v)` bubble re-enters the loop. The merge is via a result memory
    /// cell (NO phi). Top-level + no return arm at c35a (nesting / return arm later).
    fn lower_handle(
        &mut self,
        body: &TypedExpr,
        arms: &[TypedHandlerArm],
        return_arm: Option<&TypedReturnArm>,
    ) -> Result<String, String> {
        // c36b: track dynamic nesting depth so a handle whose body is reached from an
        // enclosing handle (`depth > 1`) lowers to a Kont*-typed result. Decrement on
        // every exit (incl. the `?` error paths) via the inner-fn wrapper.
        self.handle_depth += 1;
        let is_nested = self.handle_depth > 1;
        let result = self.lower_handle_inner(body, arms, return_arm, is_nested);
        self.handle_depth -= 1;
        result
    }

    fn lower_handle_inner(
        &mut self,
        body: &TypedExpr,
        arms: &[TypedHandlerArm],
        return_arm: Option<&TypedReturnArm>,
        is_nested: bool,
    ) -> Result<String, String> {
        // Lower the body to an initial kont: a kont-producing body (perform / call-to-
        // effecting / a NESTED `handle` — which itself yields a Kont*) is used directly;
        // a PURE body (`handle 42`) is wrapped via `sentinel_kont_pure` so the dispatch
        // loop sees a uniform Kont* (PURE_RETURN-tagged → the pure block fires).
        let body_is_kont = produces_kont(body, self.program)
            || matches!(body.kind, TypedExprKind::Handle { .. });
        // ADR 0020 D9 / ADR 0065 stage 3a: reject a body that `perform`s through
        // CONTROL FLOW (an `if`/`match` branch, or a non-tail `let`) — not a clean
        // kont, yet not pure either, so it would store the perform's `Kont*` into
        // the `i64` merge slot and miscompile. Mirrors inkwell's `lower_handle_inner`
        // (`!handle_body_produces_kont && expr_performs`). A pure body / an early
        // `return` is fine. Erroring here makes `snc llvm` skip the fixture in the
        // differential exactly as `snc build` rejects it.
        if !body_is_kont && expr_performs(body) {
            return Err(
                "a `handle` body that performs must do so directly (a `perform`, a \
                 call to an effecting fn, or a nested `handle`); a perform through \
                 control flow is not yet supported"
                    .to_string(),
            );
        }
        let kptr = if body_is_kont {
            self.lower_expr(body)?
        } else {
            let bv = self.lower_expr(body)?;
            let w = self.fresh();
            writeln!(self.body, "  %v{w} = call ptr @sentinel_kont_pure(i64 {bv})").unwrap();
            self.used.kont_pure = true;
            format!("%v{w}")
        };
        let cks = self.alloca("ptr");
        writeln!(self.body, "  store ptr {kptr}, ptr %v{cks}").unwrap();
        // When nested, the handle's RESULT is a Kont* (the un-caught/wrapped continuation
        // the enclosing handle dispatches on); top-level it's the i64 value.
        let rty = if is_nested { "ptr" } else { "i64" };
        let rslot = self.alloca(rty);
        let loop_b = self.fresh_block();
        let merge_b = self.fresh_block();
        writeln!(self.body, "  br label %bb{loop_b}").unwrap();
        // Loop: re-load the current kont, read its op_id.
        writeln!(self.body, "bb{loop_b}:").unwrap();
        let ck = self.fresh();
        writeln!(self.body, "  %v{ck} = load ptr, ptr %v{cks}").unwrap();
        let opid = self.fresh();
        writeln!(self.body, "  %v{opid} = load i32, ptr %v{ck}").unwrap();
        // If-else chain over the arms: each compares op_id, branches to the arm or the
        // next check. The current check block starts as the loop block; each arm opens a
        // fresh next-check block to continue in.
        for arm in arms {
            let oid = encode_op_id(arm.effect_id, arm.op_index);
            let cmp = self.fresh();
            writeln!(self.body, "  %v{cmp} = icmp eq i32 %v{opid}, {oid}").unwrap();
            let arm_b = self.fresh_block();
            let next_b = self.fresh_block();
            writeln!(self.body, "  br i1 %v{cmp}, label %bb{arm_b}, label %bb{next_b}").unwrap();
            writeln!(self.body, "bb{arm_b}:").unwrap();
            self.bind_handler_arm_params(arm, ck)?;
            self.handle_stack.push((loop_b, cks, return_arm.cloned()));
            let av = self.lower_expr(&arm.body)?;
            self.handle_stack.pop();
            // Nested: the merge type is Kont*, so wrap the arm's i64 via `kont_pure`.
            self.store_handle_result(is_nested, &av, rslot);
            writeln!(self.body, "  br label %bb{merge_b}").unwrap();
            writeln!(self.body, "bb{next_b}:").unwrap();
        }
        // The PURE_RETURN check (the chain's tail): the body / a `k(v)` drained pure.
        let cmpp = self.fresh();
        writeln!(self.body, "  %v{cmpp} = icmp eq i32 %v{opid}, {PURE_RETURN_OP_ID}").unwrap();
        let pure_b = self.fresh_block();
        let default_b = self.fresh_block();
        writeln!(self.body, "  br i1 %v{cmpp}, label %bb{pure_b}, label %bb{default_b}").unwrap();
        writeln!(self.body, "bb{pure_b}:").unwrap();
        if is_nested && return_arm.is_none() {
            // Nested + no return arm: pass the PURE_RETURN kont THROUGH unchanged (the
            // enclosing handle's pure path consumes it). No `consume_pure` here.
            writeln!(self.body, "  store ptr %v{ck}, ptr %v{rslot}").unwrap();
        } else {
            let pv = self.fresh();
            writeln!(self.body, "  %v{pv} = call i64 @sentinel_kont_consume_pure(ptr %v{ck})").unwrap();
            self.used.kont_consume_pure = true;
            // c36a: a non-identity `return v => body` arm transforms the pure value.
            let pure_result = self.apply_return_arm(return_arm, &format!("%v{pv}"))?;
            // Nested: re-wrap the (transformed) i64 in a PURE_RETURN kont for the outer.
            self.store_handle_result(is_nested, &pure_result, rslot);
        }
        writeln!(self.body, "  br label %bb{merge_b}").unwrap();
        writeln!(self.body, "bb{default_b}:").unwrap();
        if is_nested {
            // Nested default = PROPAGATE: the op_id matched no arm + isn't PURE_RETURN, so
            // it's an effect for an ENCLOSING handle — contribute the kont to the merge.
            writeln!(self.body, "  store ptr %v{ck}, ptr %v{rslot}").unwrap();
            writeln!(self.body, "  br label %bb{merge_b}").unwrap();
        } else {
            // Top-level: type-check guarantees full op coverage.
            writeln!(self.body, "  unreachable").unwrap();
        }
        writeln!(self.body, "bb{merge_b}:").unwrap();
        let rv = self.fresh();
        writeln!(self.body, "  %v{rv} = load {rty}, ptr %v{rslot}").unwrap();
        Ok(format!("%v{rv}"))
    }

    /// Bar B / effects (c36b) — store a handler-arm / pure-return i64 `val` into the
    /// handle's result slot. Top-level: a plain `store i64`. Nested: the slot is Kont*,
    /// so wrap `val` via `sentinel_kont_pure` first (the merged result is the kont the
    /// enclosing handle dispatches on).
    fn store_handle_result(&mut self, is_nested: bool, val: &str, rslot: u32) {
        if is_nested {
            let w = self.fresh();
            writeln!(self.body, "  %v{w} = call ptr @sentinel_kont_pure(i64 {val})").unwrap();
            self.used.kont_pure = true;
            writeln!(self.body, "  store ptr %v{w}, ptr %v{rslot}").unwrap();
        } else {
            writeln!(self.body, "  store i64 {val}, ptr %v{rslot}").unwrap();
        }
    }

    /// Bar B / effects (c36a / ADR 0020 D4) — apply an optional non-identity `return v =>
    /// body` arm to a pure i64 `value`: bind `v` (the arm's `value_var_id`) to an i64 slot
    /// holding `value`, then lower the arm body (which reads `v`) and return its operand.
    /// With no return arm this is the identity (returns `value`). The binding is a flat
    /// per-fn slot (like a handler-arm op-param), so no scope teardown is needed.
    fn apply_return_arm(
        &mut self,
        return_arm: Option<&TypedReturnArm>,
        value: &str,
    ) -> Result<String, String> {
        match return_arm {
            Some(ra) => {
                let slot = self.alloca("i64");
                writeln!(self.body, "  store i64 {value}, ptr %v{slot}").unwrap();
                self.slots.insert(ra.value_var_id, slot);
                self.var_ty.insert(ra.value_var_id, Type::I64);
                self.lower_expr(&ra.body)
            }
            None => Ok(value.to_string()),
        }
    }

    /// Bar B / effects — bind a handler arm's op-param(s) + the continuation. The single
    /// op-param (C3.5(a)) reads `kont.arg` (i8-GEP offset 8) into an alloca slot; the
    /// continuation (the last `param_var_id`) gets a slot holding the kont pointer
    /// (`ResumeKont` loads `ptr` from it). `kont_reg` is the loaded current-kont `%vN`.
    fn bind_handler_arm_params(
        &mut self,
        arm: &TypedHandlerArm,
        kont_reg: u32,
    ) -> Result<(), String> {
        let n = arm.param_var_ids.len();
        let n_op = n - 1; // the last param is the continuation
        if n_op >= 1 {
            let ap = self.fresh();
            writeln!(self.body, "  %v{ap} = getelementptr i8, ptr %v{kont_reg}, i64 8").unwrap();
            let av = self.fresh();
            writeln!(self.body, "  %v{av} = load i64, ptr %v{ap}").unwrap();
            let slot = self.alloca("i64");
            writeln!(self.body, "  store i64 %v{av}, ptr %v{slot}").unwrap();
            self.slots.insert(arm.param_var_ids[0], slot);
            self.var_ty.insert(arm.param_var_ids[0], Type::I64);
        }
        let kont_vid = arm.param_var_ids[n - 1];
        let kslot = self.alloca("ptr");
        writeln!(self.body, "  store ptr %v{kont_reg}, ptr %v{kslot}").unwrap();
        self.slots.insert(kont_vid, kslot);
        Ok(())
    }

    /// Bar B / effects — `k(v)` resumes the continuation `kont` with `v`:
    /// `call ptr @sentinel_kont_resume(kont, v)` → a result kont; if its `op_id` is
    /// PURE_RETURN the chain drained → `consume_pure` to the i64 value; otherwise a
    /// resumer bubbled → store it to the enclosing handle's slot + re-enter its loop.
    /// Returns the i64 value (the builder ends on the pure path).
    fn lower_resume_kont(&mut self, kont: VarId, args: &[TypedExpr]) -> Result<String, String> {
        let kslot = *self.slots.get(&kont).ok_or("resume of an unbound kont")?;
        let kreg = self.fresh();
        writeln!(self.body, "  %v{kreg} = load ptr, ptr %v{kslot}").unwrap();
        let arg = self.lower_expr(&args[0])?;
        let kr = self.fresh();
        writeln!(self.body, "  %v{kr} = call ptr @sentinel_kont_resume(ptr %v{kreg}, i64 {arg})").unwrap();
        self.used.kont_resume = true;
        let kropid = self.fresh();
        writeln!(self.body, "  %v{kropid} = load i32, ptr %v{kr}").unwrap();
        let isp = self.fresh();
        writeln!(self.body, "  %v{isp} = icmp eq i32 %v{kropid}, {PURE_RETURN_OP_ID}").unwrap();
        let pure_b = self.fresh_block();
        let bubble_b = self.fresh_block();
        writeln!(self.body, "  br i1 %v{isp}, label %bb{pure_b}, label %bb{bubble_b}").unwrap();
        // Bubble: hand the new kont to the enclosing handle's dispatch loop. Clone the
        // frame (its `return_arm` is non-Copy) so the pure path can apply the arm without
        // re-borrowing `self.handle_stack` across the `lower_expr` it triggers.
        let frame = self
            .handle_stack
            .last()
            .ok_or("ResumeKont must be lowered inside a handle arm")?
            .clone();
        let (loop_b, cks, ret_arm) = frame;
        writeln!(self.body, "bb{bubble_b}:").unwrap();
        writeln!(self.body, "  store ptr %v{kr}, ptr %v{cks}").unwrap();
        writeln!(self.body, "  br label %bb{loop_b}").unwrap();
        // Pure: unwrap. c36a — `k := \v. handle (kont.resume v) with H` (Phase B), so a
        // non-identity `return v => body` arm transforms k(v)'s pure result just as it
        // transforms the body's pure return.
        writeln!(self.body, "bb{pure_b}:").unwrap();
        let pv = self.fresh();
        writeln!(self.body, "  %v{pv} = call i64 @sentinel_kont_consume_pure(ptr %v{kr})").unwrap();
        self.used.kont_consume_pure = true;
        self.apply_return_arm(ret_arm.as_ref(), &format!("%v{pv}"))
    }

    /// Lower a slice of argument expressions to `(ll-type, operand)` pairs, in order —
    /// the collect-then-emit shape shared by every call form (Bar B / classes reuses it
    /// for the class-call args after the leading `self`/`out_ptr`).
    fn lower_args(&mut self, args: &[TypedExpr]) -> Result<Vec<(String, String)>, String> {
        let mut ops = Vec::with_capacity(args.len());
        for a in args {
            let op = self.lower_expr(a)?;
            ops.push((self.lty(a.ty)?, op));
        }
        Ok(ops)
    }

    fn lower_call(&mut self, id: FnId, args: &[TypedExpr], type_args: &[Type], ret_ty: Type) -> Result<String, String> {
        // The two trivial width-conversion builtins (used by the selfhost
        // sources): zext (u8→i64) / trunc (i64→u8).
        // print(x: i64) -> i64 (Bar B): the simplest runtime builtin — call the C symbol.
        if id == PRINT_FN_ID {
            let x = self.lower_expr(&args[0])?;
            let v = self.fresh();
            writeln!(self.body, "  %v{v} = call i64 @sentinel_print(i64 {x})").unwrap();
            self.used.print = true;
            return Ok(format!("%v{v}"));
        }
        if id == U8_TO_I64_FN_ID {
            let x = self.lower_expr(&args[0])?;
            let v = self.fresh();
            writeln!(self.body, "  %v{v} = zext i8 {x} to i64").unwrap();
            return Ok(format!("%v{v}"));
        }
        if id == I64_TO_U8_FN_ID {
            let x = self.lower_expr(&args[0])?;
            let v = self.fresh();
            writeln!(self.body, "  %v{v} = trunc i64 {x} to i8").unwrap();
            return Ok(format!("%v{v}"));
        }
        if id == LEN_FN_ID {
            // `len` works on `[T]` (`{i64,ptr}`) and `Vec<T>` (`{i64,i64,ptr}`) — both
            // put `len` at field 0; use the arg's actual aggregate type.
            let arr = self.lower_expr(&args[0])?;
            let aggty = self.lty(args[0].ty)?;
            let v = self.fresh();
            writeln!(self.body, "  %v{v} = extractvalue {aggty} {arr}, 0").unwrap();
            return Ok(format!("%v{v}"));
        }
        // is_some(x: ?T) -> bool (Bar B / ADR 0014 D9): extract the i1 valid bit
        // (field 0) of the `{ i1, T }` nullable struct. Inline, no runtime symbol.
        if id == IS_SOME_FN_ID {
            let x = self.lower_expr(&args[0])?;
            let nty = self.lty(args[0].ty)?;
            let v = self.fresh();
            writeln!(self.body, "  %v{v} = extractvalue {nty} {x}, 0").unwrap();
            return Ok(format!("%v{v}"));
        }
        // unwrap_or(x: ?T, default: T) -> T (Bar B / ADR 0014 D9): extract the valid
        // bit + payload, then `select` between the payload (valid) and the default
        // (null). `select` (not control flow) since both operands are already eval'd.
        if id == UNWRAP_OR_FN_ID {
            let x = self.lower_expr(&args[0])?;
            let d = self.lower_expr(&args[1])?;
            let nty = self.lty(args[0].ty)?;
            let pty = self.lty(ret_ty)?;
            let valid = self.fresh();
            writeln!(self.body, "  %v{valid} = extractvalue {nty} {x}, 0").unwrap();
            let payload = self.fresh();
            writeln!(self.body, "  %v{payload} = extractvalue {nty} {x}, 1").unwrap();
            let v = self.fresh();
            writeln!(self.body, "  %v{v} = select i1 %v{valid}, {pty} %v{payload}, {pty} {d}").unwrap();
            return Ok(format!("%v{v}"));
        }
        // The byte-array runtime builtins (ADR 0033/0035): each decomposes its
        // `[u8]` (`{ i64 len, ptr data }`) arg(s) into the (ptr, len) the C symbol
        // wants and calls it. str_eq → i1; print_bytes/write_file → i64; read_file
        // reassembles an owned `[u8]` from the returned data ptr + the out-len slot.
        if id == STR_EQ_FN_ID {
            let a = self.lower_expr(&args[0])?;
            let b = self.lower_expr(&args[1])?;
            let al = self.fresh();
            writeln!(self.body, "  %v{al} = extractvalue {{ i64, ptr }} {a}, 0").unwrap();
            let ap = self.fresh();
            writeln!(self.body, "  %v{ap} = extractvalue {{ i64, ptr }} {a}, 1").unwrap();
            let bl = self.fresh();
            writeln!(self.body, "  %v{bl} = extractvalue {{ i64, ptr }} {b}, 0").unwrap();
            let bp = self.fresh();
            writeln!(self.body, "  %v{bp} = extractvalue {{ i64, ptr }} {b}, 1").unwrap();
            let v = self.fresh();
            writeln!(
                self.body,
                "  %v{v} = call i1 @sentinel_str_eq(ptr %v{ap}, i64 %v{al}, ptr %v{bp}, i64 %v{bl})"
            )
            .unwrap();
            self.used.str_eq = true;
            return Ok(format!("%v{v}"));
        }
        if id == PRINT_BYTES_FN_ID {
            let d = self.lower_expr(&args[0])?;
            let dl = self.fresh();
            writeln!(self.body, "  %v{dl} = extractvalue {{ i64, ptr }} {d}, 0").unwrap();
            let dp = self.fresh();
            writeln!(self.body, "  %v{dp} = extractvalue {{ i64, ptr }} {d}, 1").unwrap();
            let v = self.fresh();
            writeln!(
                self.body,
                "  %v{v} = call i64 @sentinel_print_bytes(ptr %v{dp}, i64 %v{dl})"
            )
            .unwrap();
            self.used.print_bytes = true;
            return Ok(format!("%v{v}"));
        }
        if id == WRITE_FILE_FN_ID {
            let p = self.lower_expr(&args[0])?;
            let d = self.lower_expr(&args[1])?;
            let pl = self.fresh();
            writeln!(self.body, "  %v{pl} = extractvalue {{ i64, ptr }} {p}, 0").unwrap();
            let pp = self.fresh();
            writeln!(self.body, "  %v{pp} = extractvalue {{ i64, ptr }} {p}, 1").unwrap();
            let dl = self.fresh();
            writeln!(self.body, "  %v{dl} = extractvalue {{ i64, ptr }} {d}, 0").unwrap();
            let dp = self.fresh();
            writeln!(self.body, "  %v{dp} = extractvalue {{ i64, ptr }} {d}, 1").unwrap();
            let v = self.fresh();
            writeln!(
                self.body,
                "  %v{v} = call i64 @sentinel_write_file(ptr %v{pp}, i64 %v{pl}, ptr %v{dp}, i64 %v{dl})"
            )
            .unwrap();
            self.used.write_file = true;
            return Ok(format!("%v{v}"));
        }
        if id == READ_FILE_FN_ID {
            let p = self.lower_expr(&args[0])?;
            let pl = self.fresh();
            writeln!(self.body, "  %v{pl} = extractvalue {{ i64, ptr }} {p}, 0").unwrap();
            let pp = self.fresh();
            writeln!(self.body, "  %v{pp} = extractvalue {{ i64, ptr }} {p}, 1").unwrap();
            // The out-len slot is a hoisted i64 alloca; the call writes the byte
            // count there and returns the data ptr → reassemble the owned `[u8]`.
            let slot = self.alloca("i64");
            let data = self.fresh();
            writeln!(
                self.body,
                "  %v{data} = call ptr @sentinel_read_file(ptr %v{pp}, i64 %v{pl}, ptr %v{slot})"
            )
            .unwrap();
            self.used.read_file = true;
            let rlen = self.fresh();
            writeln!(self.body, "  %v{rlen} = load i64, ptr %v{slot}").unwrap();
            let a0 = self.fresh();
            writeln!(self.body, "  %v{a0} = insertvalue {{ i64, ptr }} undef, i64 %v{rlen}, 0").unwrap();
            let a1 = self.fresh();
            writeln!(self.body, "  %v{a1} = insertvalue {{ i64, ptr }} %v{a0}, ptr %v{data}, 1").unwrap();
            return Ok(format!("%v{a1}"));
        }
        // The growable-collection builtins (ADR 0034). A `Vec<T>` is
        // `{ i64 len, i64 cap, ptr data }`; push/pop mutate it in place through the
        // `&mut Vec` pointer (its first arg).
        if id == VEC_NEW_FN_ID {
            // An empty vector value — the same constant for every element type.
            return Ok("{ i64 0, i64 0, ptr null }".to_string());
        }
        if id == PUSH_FN_ID || id == POP_FN_ID {
            // The element type, recovered from the `&mut Vec<T>` first arg.
            let vec_ty = match args[0].ty {
                Type::Ref(rid) => self.program.refs[rid.0 as usize].inner,
                _ => return Err("push/pop arg is not a &mut Vec".into()),
            };
            let elem = match vec_ty {
                Type::Vec(ve) => ve.to_type(),
                _ => return Err("push/pop is not on a Vec".into()),
            };
            let ety = self.lty(elem)?;
            // Lower the args FIRST (push's element next), matching the Sentinel — which
            // collects both call args before emitting — so a side-effecting arg's
            // instructions land before the field GEPs on both backends.
            let vec_ptr = self.lower_expr(&args[0])?;
            let elem_val = if id == PUSH_FN_ID {
                self.lower_expr(&args[1])?
            } else {
                String::new()
            };
            let lenp = self.fresh();
            writeln!(self.body, "  %v{lenp} = getelementptr {{ i64, i64, ptr }}, ptr {vec_ptr}, i32 0, i32 0").unwrap();
            let datp = self.fresh();
            writeln!(self.body, "  %v{datp} = getelementptr {{ i64, i64, ptr }}, ptr {vec_ptr}, i32 0, i32 2").unwrap();
            if id == PUSH_FN_ID {
                let capp = self.fresh();
                writeln!(self.body, "  %v{capp} = getelementptr {{ i64, i64, ptr }}, ptr {vec_ptr}, i32 0, i32 1").unwrap();
                let lenv = self.fresh();
                writeln!(self.body, "  %v{lenv} = load i64, ptr %v{lenp}").unwrap();
                let capv = self.fresh();
                writeln!(self.body, "  %v{capv} = load i64, ptr %v{capp}").unwrap();
                let ng = self.fresh();
                writeln!(self.body, "  %v{ng} = icmp eq i64 %v{lenv}, %v{capv}").unwrap();
                let gb = self.fresh_block();
                let cb = self.fresh_block();
                writeln!(self.body, "  br i1 %v{ng}, label %bb{gb}, label %bb{cb}").unwrap();
                writeln!(self.body, "bb{gb}:").unwrap();
                let od = self.fresh();
                writeln!(self.body, "  %v{od} = load ptr, ptr %v{datp}").unwrap();
                let cx = self.fresh();
                writeln!(self.body, "  %v{cx} = mul i64 %v{capv}, 2").unwrap();
                let cz = self.fresh();
                writeln!(self.body, "  %v{cz} = icmp eq i64 %v{capv}, 0").unwrap();
                let nc = self.fresh();
                writeln!(self.body, "  %v{nc} = select i1 %v{cz}, i64 1, i64 %v{cx}").unwrap();
                let sz = self.fresh();
                writeln!(self.body, "  %v{sz} = getelementptr {ety}, ptr null, i64 1").unwrap();
                let szi = self.fresh();
                writeln!(self.body, "  %v{szi} = ptrtoint ptr %v{sz} to i64").unwrap();
                let ns = self.fresh();
                writeln!(self.body, "  %v{ns} = mul i64 %v{nc}, %v{szi}").unwrap();
                let nd = self.fresh();
                writeln!(self.body, "  %v{nd} = call ptr @sentinel_realloc(ptr %v{od}, i64 %v{ns})").unwrap();
                self.used.realloc = true;
                writeln!(self.body, "  store i64 %v{nc}, ptr %v{capp}").unwrap();
                writeln!(self.body, "  store ptr %v{nd}, ptr %v{datp}").unwrap();
                writeln!(self.body, "  br label %bb{cb}").unwrap();
                writeln!(self.body, "bb{cb}:").unwrap();
                let dd = self.fresh();
                writeln!(self.body, "  %v{dd} = load ptr, ptr %v{datp}").unwrap();
                let sl = self.fresh();
                writeln!(self.body, "  %v{sl} = getelementptr {ety}, ptr %v{dd}, i64 %v{lenv}").unwrap();
                writeln!(self.body, "  store {ety} {elem_val}, ptr %v{sl}").unwrap();
                let nl = self.fresh();
                writeln!(self.body, "  %v{nl} = add i64 %v{lenv}, 1").unwrap();
                writeln!(self.body, "  store i64 %v{nl}, ptr %v{lenp}").unwrap();
                return Ok("0".into());
            }
            // pop: empty-check (reuse the OOB trap), decrement len, return data[len-1].
            let lenv = self.fresh();
            writeln!(self.body, "  %v{lenv} = load i64, ptr %v{lenp}").unwrap();
            let nl = self.fresh();
            writeln!(self.body, "  %v{nl} = sub i64 %v{lenv}, 1").unwrap();
            let ne = self.fresh();
            writeln!(self.body, "  %v{ne} = icmp sge i64 %v{nl}, 0").unwrap();
            let ok = self.fresh_block();
            let empty = self.fresh_block();
            writeln!(self.body, "  br i1 %v{ne}, label %bb{ok}, label %bb{empty}").unwrap();
            writeln!(self.body, "bb{empty}:").unwrap();
            writeln!(self.body, "  call void @sentinel_panic_oob(i64 %v{nl}, i64 %v{lenv})").unwrap();
            self.used.panic_oob = true;
            writeln!(self.body, "  unreachable").unwrap();
            writeln!(self.body, "bb{ok}:").unwrap();
            let dd = self.fresh();
            writeln!(self.body, "  %v{dd} = load ptr, ptr %v{datp}").unwrap();
            let ep = self.fresh();
            writeln!(self.body, "  %v{ep} = getelementptr {ety}, ptr %v{dd}, i64 %v{nl}").unwrap();
            let ev = self.fresh();
            writeln!(self.body, "  %v{ev} = load {ety}, ptr %v{ep}").unwrap();
            writeln!(self.body, "  store i64 %v{nl}, ptr %v{lenp}").unwrap();
            return Ok(format!("%v{ev}"));
        }
        if id == VEC_TO_ARRAY_FN_ID {
            // vec_to_array(v: Vec<T>) -> [T]: copy the live `len` elements into a
            // fresh heap buffer and build an owned `[T]` `{ i64 len, ptr data }`.
            // Non-consuming — the array is an independent copy (`v` keeps its own
            // buffer), so both drop their buffers at scope exit (leak-free). The Vec
            // is passed by VALUE (its aggregate `{i64,i64,ptr}`), so `data` is field 2.
            let elem = match args[0].ty {
                Type::Vec(ve) => ve.to_type(),
                _ => return Err("vec_to_array arg is not a Vec".into()),
            };
            let ety = self.lty(elem)?;
            let vec_val = self.lower_expr(&args[0])?;
            let len = self.fresh();
            writeln!(self.body, "  %v{len} = extractvalue {{ i64, i64, ptr }} {vec_val}, 0").unwrap();
            let src = self.fresh();
            writeln!(self.body, "  %v{src} = extractvalue {{ i64, i64, ptr }} {vec_val}, 2").unwrap();
            // Size = len * sizeof(T) via the GEP-sizeof idiom (correct for any element
            // type, incl. padded structs); `sentinel_alloc` the dest; `llvm.memcpy`
            // the live prefix (align 1 implicit; a 0-length copy is a no-op).
            let sz = self.fresh();
            writeln!(self.body, "  %v{sz} = getelementptr {ety}, ptr null, i64 %v{len}").unwrap();
            let szi = self.fresh();
            writeln!(self.body, "  %v{szi} = ptrtoint ptr %v{sz} to i64").unwrap();
            let dest = self.fresh();
            writeln!(self.body, "  %v{dest} = call ptr @sentinel_alloc(i64 %v{szi})").unwrap();
            self.used.alloc = true;
            writeln!(self.body, "  call void @llvm.memcpy.p0.p0.i64(ptr %v{dest}, ptr %v{src}, i64 %v{szi}, i1 false)").unwrap();
            self.used.memcpy = true;
            let a0 = self.fresh();
            writeln!(self.body, "  %v{a0} = insertvalue {{ i64, ptr }} undef, i64 %v{len}, 0").unwrap();
            let a1 = self.fresh();
            writeln!(self.body, "  %v{a1} = insertvalue {{ i64, ptr }} %v{a0}, ptr %v{dest}, 1").unwrap();
            return Ok(format!("%v{a1}"));
        }
        // ADR 0066 M1.2: the channel builtins. channel_new -> a ptr; send/close ->
        // i64 status; recv -> ?i64 built from the runtime's (status, *out) pair.
        if id == CHANNEL_NEW_FN_ID {
            let v = self.fresh();
            writeln!(self.body, "  %v{v} = call ptr @sentinel_channel_new()").unwrap();
            self.used.channel_new = true;
            return Ok(format!("%v{v}"));
        }
        if id == SEND_FN_ID {
            // ADR 0066 M1.2b-cont: ENCODE the word-scalar element into the i64 slot
            // (the M1.1 encode); the `i64` case is byte-identical to M1.2.
            let ch = self.lower_expr(&args[0])?;
            let val = self.lower_expr(&args[1])?;
            let enc = match args[1].ty {
                Type::I64 => val,
                Type::I32 | Type::U8 | Type::Bool => {
                    let ety = self.lty(args[1].ty)?;
                    let e = self.fresh();
                    writeln!(self.body, "  %v{e} = zext {ety} {val} to i64").unwrap();
                    format!("%v{e}")
                }
                Type::F64 => {
                    let e = self.fresh();
                    writeln!(self.body, "  %v{e} = bitcast double {val} to i64").unwrap();
                    format!("%v{e}")
                }
                Type::Ptr => {
                    let e = self.fresh();
                    writeln!(self.body, "  %v{e} = ptrtoint ptr {val} to i64").unwrap();
                    format!("%v{e}")
                }
                other => return Err(format!("channel send element not ported (word-scalar): {other:?}")),
            };
            let v = self.fresh();
            writeln!(self.body, "  %v{v} = call i64 @sentinel_channel_send(ptr {ch}, i64 {enc})").unwrap();
            self.used.channel_send = true;
            return Ok(format!("%v{v}"));
        }
        if id == CHANNEL_CLOSE_FN_ID {
            let ch = self.lower_expr(&args[0])?;
            let v = self.fresh();
            writeln!(self.body, "  %v{v} = call i64 @sentinel_channel_close(ptr {ch})").unwrap();
            self.used.channel_close = true;
            return Ok(format!("%v{v}"));
        }
        if id == RECV_FN_ID {
            // recv(ch) -> ?T: write the i64 slot, DECODE it into the element T (M1.2b-
            // cont; `i64` byte-identical to M1.2), then build `{ i1 valid, T value }`
            // (valid = status == 0). Element from `type_args[0]` (`i64` if absent).
            let elem = type_args.first().copied().unwrap_or(Type::I64);
            let ety = self.lty(elem)?;
            let ch = self.lower_expr(&args[0])?;
            let out = self.alloca("i64");
            let status = self.fresh();
            writeln!(self.body, "  %v{status} = call i64 @sentinel_channel_recv(ptr {ch}, ptr %v{out})").unwrap();
            self.used.channel_recv = true;
            let valid = self.fresh();
            writeln!(self.body, "  %v{valid} = icmp eq i64 %v{status}, 0").unwrap();
            let raw = self.fresh();
            writeln!(self.body, "  %v{raw} = load i64, ptr %v{out}").unwrap();
            let value = match elem {
                Type::I64 => format!("%v{raw}"),
                Type::I32 | Type::U8 | Type::Bool => {
                    let d = self.fresh();
                    writeln!(self.body, "  %v{d} = trunc i64 %v{raw} to {ety}").unwrap();
                    format!("%v{d}")
                }
                Type::F64 => {
                    let d = self.fresh();
                    writeln!(self.body, "  %v{d} = bitcast i64 %v{raw} to double").unwrap();
                    format!("%v{d}")
                }
                Type::Ptr => {
                    let d = self.fresh();
                    writeln!(self.body, "  %v{d} = inttoptr i64 %v{raw} to ptr").unwrap();
                    format!("%v{d}")
                }
                other => return Err(format!("channel recv element not ported (word-scalar): {other:?}")),
            };
            let a0 = self.fresh();
            writeln!(self.body, "  %v{a0} = insertvalue {{ i1, {ety} }} undef, i1 %v{valid}, 0").unwrap();
            let a1 = self.fresh();
            writeln!(self.body, "  %v{a1} = insertvalue {{ i1, {ety} }} %v{a0}, {ety} {value}, 1").unwrap();
            return Ok(format!("%v{a1}"));
        }
        // ADR 0066 M2.1: the subprocess builtins. process_spawn decomposes the
        // `[u8]` path + `[[u8]]` args into (ptr, len) pairs (the args ptr is the
        // element buffer = argv, the args len = argc) and returns a `Process` ptr;
        // process_wait returns the i64 exit code.
        if id == PROCESS_SPAWN_FN_ID {
            let path = self.lower_expr(&args[0])?;
            let argsv = self.lower_expr(&args[1])?;
            let pl = self.fresh();
            writeln!(self.body, "  %v{pl} = extractvalue {{ i64, ptr }} {path}, 0").unwrap();
            let pp = self.fresh();
            writeln!(self.body, "  %v{pp} = extractvalue {{ i64, ptr }} {path}, 1").unwrap();
            let ac = self.fresh();
            writeln!(self.body, "  %v{ac} = extractvalue {{ i64, ptr }} {argsv}, 0").unwrap();
            let av = self.fresh();
            writeln!(self.body, "  %v{av} = extractvalue {{ i64, ptr }} {argsv}, 1").unwrap();
            let v = self.fresh();
            writeln!(
                self.body,
                "  %v{v} = call ptr @sentinel_process_spawn(ptr %v{pp}, i64 %v{pl}, ptr %v{av}, i64 %v{ac})"
            )
            .unwrap();
            self.used.process_spawn = true;
            return Ok(format!("%v{v}"));
        }
        if id == PROCESS_WAIT_FN_ID {
            let p = self.lower_expr(&args[0])?;
            let v = self.fresh();
            writeln!(self.body, "  %v{v} = call i64 @sentinel_process_wait(ptr {p})").unwrap();
            self.used.process_wait = true;
            return Ok(format!("%v{v}"));
        }
        // ADR 0066 M2.2: the byte-pipe IPC builtins. process_write decomposes the
        // `[u8]` data into (ptr, len) and returns the i64 status; process_read
        // calls with an out-len alloca and reassembles the owned `[u8]` result
        // (the read_file shape).
        if id == PROCESS_WRITE_FN_ID {
            let p = self.lower_expr(&args[0])?;
            let data = self.lower_expr(&args[1])?;
            let dl = self.fresh();
            writeln!(self.body, "  %v{dl} = extractvalue {{ i64, ptr }} {data}, 0").unwrap();
            let dp = self.fresh();
            writeln!(self.body, "  %v{dp} = extractvalue {{ i64, ptr }} {data}, 1").unwrap();
            let v = self.fresh();
            writeln!(
                self.body,
                "  %v{v} = call i64 @sentinel_process_write(ptr {p}, ptr %v{dp}, i64 %v{dl})"
            )
            .unwrap();
            self.used.process_write = true;
            return Ok(format!("%v{v}"));
        }
        if id == PROCESS_READ_FN_ID {
            let p = self.lower_expr(&args[0])?;
            // The out-len slot is a hoisted i64 alloca; the call writes the byte
            // count there and returns the data ptr → reassemble the owned `[u8]`.
            let slot = self.alloca("i64");
            let data = self.fresh();
            writeln!(
                self.body,
                "  %v{data} = call ptr @sentinel_process_read(ptr {p}, ptr %v{slot})"
            )
            .unwrap();
            self.used.process_read = true;
            let rlen = self.fresh();
            writeln!(self.body, "  %v{rlen} = load i64, ptr %v{slot}").unwrap();
            let a0 = self.fresh();
            writeln!(self.body, "  %v{a0} = insertvalue {{ i64, ptr }} undef, i64 %v{rlen}, 0").unwrap();
            let a1 = self.fresh();
            writeln!(self.body, "  %v{a1} = insertvalue {{ i64, ptr }} %v{a0}, ptr %v{data}, 1").unwrap();
            return Ok(format!("%v{a1}"));
        }
        // ADR 0066 M2.3 / M2.3b: the typed framed-channel-over-pipe builtins.
        // process_send ENCODES the word-scalar element into the i64 frame (the M1.1
        // encode); process_recv reads one i64 frame, DECODES it into the element T,
        // and builds the `?T`. The `i64` case has no encode/decode → byte-identical
        // to M2.3.
        if id == PROCESS_SEND_FN_ID {
            let p = self.lower_expr(&args[0])?;
            let v = self.lower_expr(&args[1])?;
            let enc = match args[1].ty {
                Type::I64 => v,
                Type::I32 | Type::U8 | Type::Bool => {
                    let ety = self.lty(args[1].ty)?;
                    let e = self.fresh();
                    writeln!(self.body, "  %v{e} = zext {ety} {v} to i64").unwrap();
                    format!("%v{e}")
                }
                Type::F64 => {
                    let e = self.fresh();
                    writeln!(self.body, "  %v{e} = bitcast double {v} to i64").unwrap();
                    format!("%v{e}")
                }
                Type::Ptr => {
                    let e = self.fresh();
                    writeln!(self.body, "  %v{e} = ptrtoint ptr {v} to i64").unwrap();
                    format!("%v{e}")
                }
                other => {
                    return Err(format!(
                        "process_send element not ported (M2.3b word-scalar): {other:?}"
                    ))
                }
            };
            let r = self.fresh();
            writeln!(self.body, "  %v{r} = call i64 @sentinel_process_send(ptr {p}, i64 {enc})").unwrap();
            self.used.process_send = true;
            return Ok(format!("%v{r}"));
        }
        if id == PROCESS_RECV_FN_ID {
            // process_recv(p) -> ?T: write the i64 frame to a stack out-slot, decode
            // it into T, then build `{ i1 valid, T value }` (valid = status == 0).
            let elem = type_args.first().copied().unwrap_or(Type::I64);
            let ety = self.lty(elem)?;
            let p = self.lower_expr(&args[0])?;
            let out = self.alloca("i64");
            let status = self.fresh();
            writeln!(self.body, "  %v{status} = call i64 @sentinel_process_recv(ptr {p}, ptr %v{out})").unwrap();
            self.used.process_recv = true;
            let valid = self.fresh();
            writeln!(self.body, "  %v{valid} = icmp eq i64 %v{status}, 0").unwrap();
            let raw = self.fresh();
            writeln!(self.body, "  %v{raw} = load i64, ptr %v{out}").unwrap();
            let value = match elem {
                Type::I64 => format!("%v{raw}"),
                Type::I32 | Type::U8 | Type::Bool => {
                    let d = self.fresh();
                    writeln!(self.body, "  %v{d} = trunc i64 %v{raw} to {ety}").unwrap();
                    format!("%v{d}")
                }
                Type::F64 => {
                    let d = self.fresh();
                    writeln!(self.body, "  %v{d} = bitcast i64 %v{raw} to double").unwrap();
                    format!("%v{d}")
                }
                Type::Ptr => {
                    let d = self.fresh();
                    writeln!(self.body, "  %v{d} = inttoptr i64 %v{raw} to ptr").unwrap();
                    format!("%v{d}")
                }
                other => {
                    return Err(format!(
                        "process_recv element not ported (M2.3b word-scalar): {other:?}"
                    ))
                }
            };
            let a0 = self.fresh();
            writeln!(self.body, "  %v{a0} = insertvalue {{ i1, {ety} }} undef, i1 %v{valid}, 0").unwrap();
            let a1 = self.fresh();
            writeln!(self.body, "  %v{a1} = insertvalue {{ i1, {ety} }} %v{a0}, {ety} {value}, 1").unwrap();
            return Ok(format!("%v{a1}"));
        }
        // ADR 0066 M2.4a / ADR 0069: the SealedChannel bridge builtins are
        // identity-ptr passthroughs — both `Process` and `SealedChannel` lower to
        // the same opaque ptr (bridge (iii)), so re-typing is a value-level no-op.
        if id == SEALED_CHANNEL_FN_ID || id == SEALED_PROCESS_FN_ID {
            return self.lower_expr(&args[0]);
        }
        // ADR 0066 M2.4b: the child-side self-stdin/stdout framed builtins (i64-only
        // twins of process_send/recv — no `Process` arg, no element encode/decode).
        if id == STDOUT_SEND_FN_ID {
            let v = self.lower_expr(&args[0])?;
            let r = self.fresh();
            writeln!(self.body, "  %v{r} = call i64 @sentinel_stdout_send(i64 {v})").unwrap();
            self.used.stdout_send = true;
            return Ok(format!("%v{r}"));
        }
        if id == STDIN_RECV_FN_ID {
            let out = self.alloca("i64");
            let status = self.fresh();
            writeln!(self.body, "  %v{status} = call i64 @sentinel_stdin_recv(ptr %v{out})").unwrap();
            self.used.stdin_recv = true;
            let valid = self.fresh();
            writeln!(self.body, "  %v{valid} = icmp eq i64 %v{status}, 0").unwrap();
            let raw = self.fresh();
            writeln!(self.body, "  %v{raw} = load i64, ptr %v{out}").unwrap();
            let a0 = self.fresh();
            writeln!(self.body, "  %v{a0} = insertvalue {{ i1, i64 }} undef, i1 %v{valid}, 0").unwrap();
            let a1 = self.fresh();
            writeln!(self.body, "  %v{a1} = insertvalue {{ i1, i64 }} %v{a0}, i64 %v{raw}, 1").unwrap();
            return Ok(format!("%v{a1}"));
        }
        // ADR 0066 M2.4 follow-on: own command-line argument reflection.
        if id == ARG_COUNT_FN_ID {
            let r = self.fresh();
            writeln!(self.body, "  %v{r} = call i64 @sentinel_arg_count()").unwrap();
            self.used.arg_count = true;
            return Ok(format!("%v{r}"));
        }
        if id == ARG_FN_ID {
            let i = self.lower_expr(&args[0])?;
            let slot = self.alloca("i64");
            let data = self.fresh();
            writeln!(self.body, "  %v{data} = call ptr @sentinel_arg(i64 {i}, ptr %v{slot})").unwrap();
            self.used.arg = true;
            let rlen = self.fresh();
            writeln!(self.body, "  %v{rlen} = load i64, ptr %v{slot}").unwrap();
            let a0 = self.fresh();
            writeln!(self.body, "  %v{a0} = insertvalue {{ i64, ptr }} undef, i64 %v{rlen}, 0").unwrap();
            let a1 = self.fresh();
            writeln!(self.body, "  %v{a1} = insertvalue {{ i64, ptr }} %v{a0}, ptr %v{data}, 1").unwrap();
            return Ok(format!("%v{a1}"));
        }
        if id.0 < FIRST_USER_FN {
            return Err(format!("builtin call #{} (deferred to a later slice)", id.0));
        }
        let sig = self.program.signature(id);
        // Bar B / effects (c35b): a call to an effecting fn yields a Kont* (`ptr`), not
        // its declared return type — the enclosing `handle` dispatches on the kont.
        let ret = if uses_kont_abi(sig, self.program) {
            "ptr".to_string()
        } else {
            self.lty(ret_ty)?
        };
        // A generic callee resolves to its monomorphic instance's mangled symbol
        // (`id__i64`); `type_args` are concrete here (inferred at a non-generic call
        // site, or substituted in a mono body). A normal fn uses its plain name.
        let sym = if sig.type_params.is_empty() {
            sig.name.clone()
        } else {
            mangle_mono_name(&sig.name, type_args, self.program)
        };
        // Lower args to operands first, then emit the call.
        let mut arg_ops: Vec<(String, String)> = Vec::with_capacity(args.len());
        for a in args {
            let op = self.lower_expr(a)?;
            arg_ops.push((self.lty(a.ty)?, op));
        }
        let v = self.fresh();
        write!(self.body, "  %v{v} = call {ret} @{sym}(").unwrap();
        for (i, (t, op)) in arg_ops.iter().enumerate() {
            if i > 0 {
                self.body.push_str(", ");
            }
            write!(self.body, "{t} {op}").unwrap();
        }
        self.body.push_str(")\n");
        Ok(format!("%v{v}"))
    }
}

/// The LLVM type for a Sentinel `Type`. Takes `program` for the two type-table-
/// dependent cases: a top-level `secret T` strips to its inner (ADR 0019 D5/D12),
/// and a `GenericInstance` renders its structural mangled name (mirroring inkwell's
/// `llvm_basic_type`, which receives all interner tables). Un-ported types → `Err`.
fn llvm_ty(ty: Type, program: &TypedProgram) -> Result<String, String> {
    // `secret T` lowers identically to its inner — strip at entry (ADR 0019 D5/D12).
    let ty = ty.strip_secret(&program.secrets).0;
    match ty {
        Type::I64 => Ok("i64".into()),
        Type::I32 => Ok("i32".into()),
        Type::U8 => Ok("i8".into()),
        Type::Bool => Ok("i1".into()),
        // A user struct is the named aggregate declared in Pass 0.
        Type::Struct(id) => Ok(format!("%Struct.{}", id.0)),
        // A class is the named aggregate declared in Pass 0 (Bar B / classes). Unlike a
        // struct (an SSA aggregate value), a class instance lives in memory: `init` writes
        // through an `out_ptr`, methods receive `self` as a `ptr`, and `self.field` GEPs.
        // But as a VALUE type (an init param like `Logger.init(w: FileSink)`, a `let`
        // binding, a `Var(self)` load) it's still the named aggregate `%Class.N`.
        Type::Class(id) => Ok(format!("%Class.{}", id.0)),
        // `[T]` is the abi-v1 `{ i64 len, ptr data }` (ADR 0029 §2) — an inline
        // literal struct type, the same for every element type (the data is an
        // opaque heap pointer), so it needs no Pass-0 name. The element type only
        // matters for the GEP stride in ArrayLit/Index (carried by those nodes).
        Type::Array(_) => Ok("{ i64, ptr }".into()),
        // A reference `&T`/`&mut T` is an opaque pointer (LLVM doesn't distinguish
        // mutability; the pointee type is recovered from `program.refs` at the deref).
        Type::Ref(_) => Ok("ptr".into()),
        // A `Vec<T>` is `{ i64 len, i64 cap, ptr data }` (ADR 0034) — data is FIELD 2
        // (the capacity sits between len and data, vs `[T]`'s field 1).
        Type::Vec(_) => Ok("{ i64, i64, ptr }".into()),
        // An enum is the abi-v1 `{ i32 tag, ptr payload }` (ADR 0032 D4) — a 4-byte
        // discriminant (variant index, source order) + an opaque pointer to a
        // heap-boxed payload struct (`null` for a unit variant). One inline literal
        // for every enum, like `[T]`/`Vec` (the payload type is recovered per variant).
        Type::Enum(_) => Ok("{ i32, ptr }".into()),
        // A nullable `?T` (ADR 0014 D2 / ADR 0015 D11) is `{ i1 valid, <payload> }`.
        // The layout is inner-dependent (mirroring inkwell `llvm_basic_type`,
        // lib.rs:1623): `?primitive` stays inline (`{ i1, T }`), while `?Struct` /
        // `?GenericInstance` heap-indirect (`{ i1, ptr }`) — which is what breaks a
        // recursive `struct Node { next: ?Node }` cycle (the cycle goes through a
        // pointer-sized field). `?Ref` recurses to `ptr` too (a ref IS a pointer).
        Type::Nullable(inner) => {
            let payload = match inner {
                NullableInner::Struct(_) | NullableInner::GenericInstance(_) => "ptr".to_string(),
                _ => llvm_ty(inner.to_type(), program)?,
            };
            Ok(format!("{{ i1, {payload} }}"))
        }
        // A generic-struct instance `Decl<args>` lowers like a plain struct, but its
        // LLVM type is named STRUCTURALLY (`%Decl_arg1_arg2`) not by interner id — the
        // two type-checkers may intern instances in different orders, so a structural
        // name is order-independent (mirrors inkwell `mangle_generic_struct_name`).
        // Declared in Pass 0 with its substituted field layout.
        Type::GenericInstance(id) => {
            let inst = &program.generic_instances[id.0 as usize];
            Ok(format!("%{}", mangle_instance(program, inst.struct_id, &inst.args)))
        }
        // Bar B / concurrency (ADR 0024): a `Task<i64>` is an opaque `*Task` (the runtime
        // SentinelTask struct) — codegen only ever holds/passes the pointer.
        Type::Task(_) => Ok("ptr".to_string()),
        // ADR 0066 M1.2: a `Channel<i64>` is an opaque `*SentinelChannel`.
        Type::Channel(_) => Ok("ptr".to_string()),
        // ADR 0066 M2.1: a Process handle lowers to an opaque ptr.
        Type::Process => Ok("ptr".to_string()),
        // ADR 0066 M2.4a: a SealedChannel lowers to the same opaque ptr as Process.
        Type::SealedChannel => Ok("ptr".to_string()),
        other => Err(format!("type not yet ported (8a scalars only): {other:?}")),
    }
}

/// A structural mangling tag for `ty` (`i64` → `"i64"`, `[i64]` → `"arr_i64"`,
/// `Pair<i64, bool>` → `"Pair_i64_bool"`). Order-independent (no interner ids), so
/// both the oracle and the Sentinel side derive the same generic-instance type names
/// and monomorphic fn symbols. Mirrors inkwell `mangle_type` (lib.rs:2063).
fn mangle_type(ty: Type, program: &TypedProgram) -> String {
    match ty {
        Type::I64 => "i64".into(),
        Type::I32 => "i32".into(),
        Type::Bool => "bool".into(),
        Type::U8 => "u8".into(),
        Type::Struct(id) => mangle_struct_name(program, id),
        Type::Nullable(inner) => format!("opt_{}", mangle_type(inner.to_type(), program)),
        Type::Array(elem) => format!("arr_{}", mangle_type(program.array_elem_type(elem), program)),
        Type::Vec(elem) => format!("vec_{}", mangle_type(elem.to_type(), program)),
        Type::Ref(id) => {
            let d = &program.refs[id.0 as usize];
            let prefix = if d.mutable { "refmut" } else { "ref" };
            format!("{prefix}_{}", mangle_type(d.inner, program))
        }
        Type::Secret(id) => format!("sec_{}", mangle_type(program.secrets[id.0 as usize].inner, program)),
        Type::GenericInstance(id) => {
            let inst = &program.generic_instances[id.0 as usize];
            mangle_instance(program, inst.struct_id, &inst.args)
        }
        Type::TypeParam(id) => format!("T{}", id.0),
        // Enum/Class/Kont/Task/TraitSelf never appear in a corpus mono key or instance
        // arg — a defensive label only (keeps the oracle self-contained).
        other => format!("ty_{other:?}"),
    }
}

fn mangle_struct_name(program: &TypedProgram, id: StructId) -> String {
    program
        .structs
        .get(id.0 as usize)
        .map(|s| s.name.clone())
        .unwrap_or_else(|| format!("struct{}", id.0))
}

/// The mangled name for a generic-struct instance: `{StructName}_{arg1}_{arg2}…`
/// (mirrors inkwell `mangle_generic_struct_name`, lib.rs:2198).
fn mangle_instance(program: &TypedProgram, struct_id: StructId, args: &[Type]) -> String {
    let mut s = mangle_struct_name(program, struct_id);
    for a in args {
        s.push('_');
        s.push_str(&mangle_type(*a, program));
    }
    s
}

/// The mangled symbol for a monomorphic fn instance: `{name}__{arg1}__{arg2}…`
/// (mirrors inkwell `mangle_mono_name`, lib.rs:2045 — note the DOUBLE underscore,
/// vs the single-underscore generic-struct mangle). `id<i64>` → `id__i64`.
fn mangle_mono_name(base: &str, type_args: &[Type], program: &TypedProgram) -> String {
    let mut s = base.to_string();
    for t in type_args {
        s.push_str("__");
        s.push_str(&mangle_type(*t, program));
    }
    s
}

/// `true` iff none of `args` mentions a `Type::TypeParam` (transitively). Abstract
/// instances (TypeParam-bearing — interned for generic-fn signatures like
/// `fst<A,B>(p: Pair<A,B>)`) have no runtime layout and are skipped in Pass 0.
fn instance_is_concrete(args: &[Type], program: &TypedProgram) -> bool {
    args.iter().all(|a| !type_has_typeparam(*a, program))
}

fn type_has_typeparam(ty: Type, program: &TypedProgram) -> bool {
    match ty {
        Type::TypeParam(_) => true,
        Type::Nullable(ni) => type_has_typeparam(ni.to_type(), program),
        Type::Array(ae) => type_has_typeparam(ae.to_type(), program),
        Type::Vec(ve) => type_has_typeparam(ve.to_type(), program),
        Type::Ref(id) => type_has_typeparam(program.refs[id.0 as usize].inner, program),
        Type::GenericInstance(id) => program.generic_instances[id.0 as usize]
            .args
            .iter()
            .any(|a| type_has_typeparam(*a, program)),
        _ => false,
    }
}

/// 8d-drops-2: does a value of type `ty` own heap that scope-exit must `sentinel_free`?
/// An array / `Vec` always does; a struct does iff some field does (recursive — so a
/// struct of only scalars drops nothing). The Bar-B shapes (nullable / enum / generic /
/// secret) don't appear in the emitting subset, so a plain `false` suffices.
/// Bar B / effects — `true` iff `expr` lowers to a kont* (so a `handle` body uses it
/// directly as the initial kont, and an effecting-fn tail returns it as-is). c35a: a
/// direct `perform`. c35b: also a call to an effecting fn, or a block whose tail does
/// (with no performing statement). c36b would add a nested `handle`. A pure body (i64)
/// is wrapped via `sentinel_kont_pure` by the caller (the fn-ABI / handle-body paths).
fn produces_kont(expr: &TypedExpr, program: &TypedProgram) -> bool {
    match &expr.kind {
        TypedExprKind::Perform { .. } => true,
        TypedExprKind::Call { id, .. } => uses_kont_abi(program.signature(*id), program),
        TypedExprKind::Block(b) => {
            !b.stmts.iter().any(|s| stmt_performs(&s.kind)) && produces_kont(&b.tail, program)
        }
        _ => false,
    }
}

/// Bar B / effects (c35b) — does this fn use the effecting-fn `Kont*` ABI? Any fn with
/// a non-empty effect row EXCEPT one whose only effect is the built-in `Async` (a
/// direct-runtime marker — spawn/await lower to runtime calls, never `handle`, so an
/// Async-only fn keeps the plain value-returning ABI). Mirrors inkwell `uses_kont_abi`.
fn uses_kont_abi(sig: &TypedFnSignature, program: &TypedProgram) -> bool {
    sig.effect_row.iter().any(|eid| {
        program
            .effect_decls
            .get(eid.0 as usize)
            // ADR 0024 / 0066 M2.1: `Async` + `Subprocess` are built-in capability
            // effects (task / process runtime), NOT perform-based — no Kont* ABI.
            .map(|d| d.name != "Async" && d.name != "Subprocess")
            .unwrap_or(true)
    })
}

/// Bar B / effects (c35b) — validate that an effecting fn's body is a shape codegen can
/// lower at this sub-phase: no top-level (or nested-block) statement may itself
/// `perform`, and the tail must either produce a kont (direct `perform` / call-to-
/// effecting) or be fully pure (no `perform` anywhere — wrapped via `sentinel_kont_pure`
/// by the fn-ABI return path). A tail that mixes a `perform` into surrounding pure
/// context (`perform Op(..) + 1`, `f(perform Op(..))`) or a let-bound / chained perform
/// needs per-eval-site frame reification (c35c+) and Errs here so the fixture defers.
/// Mirrors inkwell `validate_effecting_fn_body`.
fn validate_effecting_fn_body(f: &TypedFnDef, program: &TypedProgram) -> Result<(), String> {
    for stmt in &f.body.stmts {
        if stmt_performs(&stmt.kind) {
            return Err(format!(
                "effecting fn `{}` body has a performing statement (deferred to c35c+)",
                f.name
            ));
        }
    }
    let tail = &f.body.tail;
    if produces_kont(tail, program) || !expr_performs(tail) {
        Ok(())
    } else {
        Err(format!(
            "effecting fn `{}` tail mixes perform with pure context (deferred to c35c+)",
            f.name
        ))
    }
}

/// Does this statement contain a `perform` (transitively)? (c35b validation helper.)
fn stmt_performs(kind: &TypedStmtKind) -> bool {
    match kind {
        TypedStmtKind::Let { value, .. } => expr_performs(value),
        TypedStmtKind::Assign { target, value } => expr_performs(target) || expr_performs(value),
        TypedStmtKind::While { cond, body } => {
            expr_performs(cond)
                || body.stmts.iter().any(|s| stmt_performs(&s.kind))
                || expr_performs(&body.tail)
        }
        TypedStmtKind::Break | TypedStmtKind::Continue => false,
        TypedStmtKind::Expr(e) => expr_performs(e),
    }
}

/// Does this expression contain a `perform` / `k(v)` (transitively)? (c35b validation
/// helper — mirrors inkwell `expr_performs`; total over every `TypedExprKind`.)
fn expr_performs(expr: &TypedExpr) -> bool {
    match &expr.kind {
        TypedExprKind::Perform { .. } | TypedExprKind::ResumeKont { .. } => true,
        TypedExprKind::Handle { body, arms, return_arm, .. } => {
            expr_performs(body)
                || arms.iter().any(|a| expr_performs(&a.body))
                || return_arm.as_deref().is_some_and(|ra| expr_performs(&ra.body))
        }
        TypedExprKind::IntLit(_)
        | TypedExprKind::FloatLit(_)
        | TypedExprKind::BoolLit(_)
        | TypedExprKind::NullLit
        | TypedExprKind::CharLit(_)
        | TypedExprKind::StringLit(_)
        | TypedExprKind::Var(_)
        | TypedExprKind::FnRef(_) => false,
        TypedExprKind::Unary(_, inner)
        | TypedExprKind::WidenToNullable(inner)
        | TypedExprKind::WidenToSecret(inner)
        | TypedExprKind::Cast(inner)
        | TypedExprKind::Return(inner)
        | TypedExprKind::Declassify(inner) => expr_performs(inner),
        TypedExprKind::Binary(_, l, r)
        | TypedExprKind::Cmp(_, l, r)
        | TypedExprKind::Logic(_, l, r) => expr_performs(l) || expr_performs(r),
        TypedExprKind::Block(b) => {
            b.stmts.iter().any(|s| stmt_performs(&s.kind)) || expr_performs(&b.tail)
        }
        TypedExprKind::If { cond, then_branch, else_branch } => {
            expr_performs(cond)
                || then_branch.stmts.iter().any(|s| stmt_performs(&s.kind))
                || expr_performs(&then_branch.tail)
                || else_branch.stmts.iter().any(|s| stmt_performs(&s.kind))
                || expr_performs(&else_branch.tail)
        }
        TypedExprKind::Call { args, .. } => args.iter().any(expr_performs),
        TypedExprKind::StructLit { fields, .. } => fields.iter().any(expr_performs),
        TypedExprKind::FieldAccess { target, .. } => expr_performs(target),
        TypedExprKind::ArrayLit { elements, .. } => elements.iter().any(expr_performs),
        TypedExprKind::Index { target, index, .. } => expr_performs(target) || expr_performs(index),
        TypedExprKind::MethodCall { target, args, .. } => {
            expr_performs(target) || args.iter().any(expr_performs)
        }
        TypedExprKind::ClassInit { args, .. } => args.iter().any(expr_performs),
        TypedExprKind::ImplMethodCall { target, args, .. } => {
            expr_performs(target) || args.iter().any(expr_performs)
        }
        TypedExprKind::QualifiedCall { args, .. } => args.iter().any(expr_performs),
        TypedExprKind::Scope { body, .. } => {
            body.stmts.iter().any(|s| stmt_performs(&s.kind)) || expr_performs(&body.tail)
        }
        TypedExprKind::Spawn { call, .. } => expr_performs(call),
        TypedExprKind::Await { task_expr, .. } => expr_performs(task_expr),
        TypedExprKind::EnumConstruct { args, .. } => args.iter().any(expr_performs),
        TypedExprKind::Match { scrutinee, arms, .. } => {
            expr_performs(scrutinee) || arms.iter().any(|a| expr_performs(&a.body))
        }
    }
}

/// Bar B / effects (c35c) — the let-bound-perform shape: an effecting (non-main) fn whose
/// body is a SINGLE `let v: i64 = <produces-kont>` with a pure tail. The perform sits in
/// non-tail position → it is reified into a captured frame (`dump_let_shape_fn`). Mirrors
/// inkwell `detect_let_shape`. (Embedded / chained / non-i64 lets defer to c35d+.)
struct LetShapeInfo<'a> {
    let_id: VarId,
    let_ty: Type,
    rhs: &'a TypedExpr,
    tail: &'a TypedExpr,
}

fn detect_let_shape<'a>(f: &'a TypedFnDef, program: &TypedProgram) -> Option<LetShapeInfo<'a>> {
    let sig = program.signature(f.id);
    if !uses_kont_abi(sig, program) || sig.is_main {
        return None;
    }
    if f.body.stmts.len() != 1 {
        return None;
    }
    let (let_id, value, ty) = match &f.body.stmts[0].kind {
        TypedStmtKind::Let { id, value, ty, .. } => (*id, value, *ty),
        _ => return None,
    };
    // The RHS must produce a Kont (a direct `perform` / call-to-effecting) — the C3.5(b)
    // `produces_kont` predicate; the tail must be pure; MVP-restricted to an i64 let (the
    // single SentinelKont `arg` slot carries the resumed value).
    if !produces_kont(value, program) {
        return None;
    }
    if expr_performs(&f.body.tail) {
        return None;
    }
    if ty != Type::I64 {
        return None;
    }
    Some(LetShapeInfo { let_id, let_ty: ty, rhs: value, tail: &f.body.tail })
}

/// Bar B / effects (c35d) — the embedded-perform shape: an effecting (non-main) fn whose
/// body is a STATEMENT-FREE tail mixing exactly ONE `perform` into pure surrounding
/// context (`perform Op() + 1`, `f(perform Op())`, nested binops, …). The perform is
/// reified into a captured frame (`dump_embedded_shape_fn`); the resumer re-evaluates
/// the surrounding context with the resumed value in the perform's place. Excludes the
/// shapes already lowered straight-line at c35a/c35b (a tail that IS a direct `perform`
/// or a direct call to an effecting fn — both produce a Kont* without reification).
/// MVP-restricted to an i64-valued perform (the SentinelKont `arg` slot + placeholder
/// are i64). Returns the unique `Perform` node. Mirrors inkwell
/// `detect_embedded_perform_shape`.
fn detect_embedded_shape<'a>(
    f: &'a TypedFnDef,
    program: &TypedProgram,
) -> Option<&'a TypedExpr> {
    let sig = program.signature(f.id);
    if !uses_kont_abi(sig, program) || sig.is_main {
        return None;
    }
    if !f.body.stmts.is_empty() {
        return None;
    }
    let tail = &f.body.tail;
    if matches!(tail.kind, TypedExprKind::Perform { .. }) {
        return None;
    }
    if let TypedExprKind::Call { id, .. } = &tail.kind {
        if uses_kont_abi(program.signature(*id), program) {
            return None;
        }
    }
    let mut performs: Vec<&TypedExpr> = Vec::new();
    collect_performs(tail, &mut performs);
    if performs.len() != 1 {
        return None;
    }
    let perform = performs[0];
    if perform.ty != Type::I64 {
        return None;
    }
    Some(perform)
}

/// Bar B / effects (c35d) — collect every `Perform` node in pre-order, recursing into
/// perform args so nested performs are all seen (the embedded shape requires EXACTLY
/// one). Mirrors inkwell `count_performs` + `find_unique_perform` in a single walk.
fn collect_performs<'a>(expr: &'a TypedExpr, acc: &mut Vec<&'a TypedExpr>) {
    if let TypedExprKind::Perform { args, .. } = &expr.kind {
        acc.push(expr);
        for a in args {
            collect_performs(a, acc);
        }
        return;
    }
    match &expr.kind {
        TypedExprKind::Perform { .. } => unreachable!("handled above"),
        TypedExprKind::IntLit(_)
        | TypedExprKind::FloatLit(_)
        | TypedExprKind::BoolLit(_)
        | TypedExprKind::NullLit
        | TypedExprKind::CharLit(_)
        | TypedExprKind::StringLit(_)
        | TypedExprKind::Var(_)
        | TypedExprKind::FnRef(_) => {}
        TypedExprKind::Unary(_, inner)
        | TypedExprKind::WidenToNullable(inner)
        | TypedExprKind::WidenToSecret(inner)
        | TypedExprKind::Cast(inner)
        | TypedExprKind::Return(inner)
        | TypedExprKind::Declassify(inner) => collect_performs(inner, acc),
        TypedExprKind::Binary(_, l, r)
        | TypedExprKind::Cmp(_, l, r)
        | TypedExprKind::Logic(_, l, r) => {
            collect_performs(l, acc);
            collect_performs(r, acc);
        }
        TypedExprKind::Block(b) => {
            for s in &b.stmts {
                collect_performs_stmt(&s.kind, acc);
            }
            collect_performs(&b.tail, acc);
        }
        TypedExprKind::If { cond, then_branch, else_branch } => {
            collect_performs(cond, acc);
            for s in &then_branch.stmts {
                collect_performs_stmt(&s.kind, acc);
            }
            collect_performs(&then_branch.tail, acc);
            for s in &else_branch.stmts {
                collect_performs_stmt(&s.kind, acc);
            }
            collect_performs(&else_branch.tail, acc);
        }
        TypedExprKind::Call { args, .. }
        | TypedExprKind::EnumConstruct { args, .. }
        | TypedExprKind::ClassInit { args, .. }
        | TypedExprKind::QualifiedCall { args, .. }
        | TypedExprKind::ResumeKont { args, .. } => {
            for a in args {
                collect_performs(a, acc);
            }
        }
        TypedExprKind::MethodCall { target, args, .. }
        | TypedExprKind::ImplMethodCall { target, args, .. } => {
            collect_performs(target, acc);
            for a in args {
                collect_performs(a, acc);
            }
        }
        TypedExprKind::StructLit { fields, .. } => {
            for f in fields {
                collect_performs(f, acc);
            }
        }
        TypedExprKind::ArrayLit { elements, .. } => {
            for el in elements {
                collect_performs(el, acc);
            }
        }
        TypedExprKind::FieldAccess { target, .. } => collect_performs(target, acc),
        TypedExprKind::Index { target, index, .. } => {
            collect_performs(target, acc);
            collect_performs(index, acc);
        }
        TypedExprKind::Handle { body, arms, return_arm, .. } => {
            collect_performs(body, acc);
            for a in arms {
                collect_performs(&a.body, acc);
            }
            if let Some(ra) = return_arm.as_deref() {
                collect_performs(&ra.body, acc);
            }
        }
        TypedExprKind::Scope { body, .. } => {
            for s in &body.stmts {
                collect_performs_stmt(&s.kind, acc);
            }
            collect_performs(&body.tail, acc);
        }
        TypedExprKind::Spawn { call, .. } => collect_performs(call, acc),
        TypedExprKind::Await { task_expr, .. } => collect_performs(task_expr, acc),
        TypedExprKind::Match { scrutinee, arms, .. } => {
            collect_performs(scrutinee, acc);
            for a in arms {
                collect_performs(&a.body, acc);
            }
        }
    }
}

fn collect_performs_stmt<'a>(kind: &'a TypedStmtKind, acc: &mut Vec<&'a TypedExpr>) {
    match kind {
        TypedStmtKind::Let { value, .. } => collect_performs(value, acc),
        TypedStmtKind::Assign { target, value } => {
            collect_performs(target, acc);
            collect_performs(value, acc);
        }
        TypedStmtKind::While { cond, body } => {
            collect_performs(cond, acc);
            for s in &body.stmts {
                collect_performs_stmt(&s.kind, acc);
            }
            collect_performs(&body.tail, acc);
        }
        TypedStmtKind::Break | TypedStmtKind::Continue => {}
        TypedStmtKind::Expr(e) => collect_performs(e, acc),
    }
}

/// Bar B / effects (c35e) — the chained-effecting-lets shape: an effecting (non-main)
/// fn whose body is 2+ `let v: i64 = <produces-kont>` statements followed by a pure
/// tail. Each let's perform is reified into its own frame; resumer-i itself performs
/// (lowering let-(i+1)'s RHS) and pushes resumer-(i+1) — the runtime's
/// `sentinel_kont_resume` BUBBLES the fresh kont so the handle's dispatch loop
/// re-dispatches it (ADR 0020 D3 deep-handler semantics). Mirrors inkwell
/// `detect_chained_effecting_lets_shape`. (A single let is the c35c shape.)
struct ChainedLetsInfo<'a> {
    lets: Vec<(VarId, Type, &'a TypedExpr)>,
    tail: &'a TypedExpr,
}

fn detect_chained_lets_shape<'a>(
    f: &'a TypedFnDef,
    program: &TypedProgram,
) -> Option<ChainedLetsInfo<'a>> {
    let sig = program.signature(f.id);
    if !uses_kont_abi(sig, program) || sig.is_main {
        return None;
    }
    if f.body.stmts.len() < 2 {
        return None;
    }
    let mut lets = Vec::with_capacity(f.body.stmts.len());
    for stmt in &f.body.stmts {
        let (id, value, ty) = match &stmt.kind {
            TypedStmtKind::Let { id, value, ty, .. } => (*id, value, *ty),
            _ => return None,
        };
        // Each RHS must produce a Kont (direct `perform` / call-to-effecting) so the
        // next resumer can be pushed onto it; MVP-restricted to i64 lets (the kont's
        // single `arg` slot carries the resumed value) — same as c35c/c35d.
        if !produces_kont(value, program) {
            return None;
        }
        if ty != Type::I64 {
            return None;
        }
        lets.push((id, ty, value));
    }
    if expr_performs(&f.body.tail) {
        return None;
    }
    Some(ChainedLetsInfo { lets, tail: &f.body.tail })
}

/// Bar B / effects (c35e) — the captured set for chained-lets resumer-`level`: the
/// vars referenced anywhere in the body resumer-`level` runs (the suffix RHSes
/// `lets[level+1..]` + the tail), in first-reference order, minus the resumed-value
/// let (its `value` param) and the deeper lets (bound in deeper resumers, not
/// captured). Mirrors inkwell `compute_chained_lets_captures`. The math closes:
/// everything captures[level+1] needs is either resumer-`level`'s value param or in
/// captures[level], so each resumer can build the next struct from its own slots.
fn compute_chained_captures(info: &ChainedLetsInfo<'_>, level: usize) -> Vec<VarId> {
    let mut refs: Vec<VarId> = Vec::new();
    for j in (level + 1)..info.lets.len() {
        walk_collect_rhs_var_refs(info.lets[j].2, &mut refs);
    }
    walk_collect_var_refs(info.tail, &mut refs);
    let mut excluded: Vec<VarId> =
        info.lets.iter().skip(level + 1).map(|l| l.0).collect();
    excluded.push(info.lets[level].0);
    refs.into_iter().filter(|id| !excluded.contains(id)).collect()
}

/// Bar B / effects (c35e) — collect var refs from a chained let's RHS. Unlike the
/// c35d captured walk (where the perform subtree is the PARENT's to lower, so
/// `walk_collect_var_refs` skips it), a chained RHS's perform is lowered by the
/// EMITTING resumer — its ARG vars must be captured, so descend into a top-level
/// `Perform`'s args (inkwell's walker descends into performs everywhere; for the
/// shapes `detect_chained_lets_shape` admits, this wrapper is equivalent — a
/// block-wrapped kont RHS with an arg'd nested perform would diverge, but no such
/// corpus shape exists and the differential would catch it loudly).
fn walk_collect_rhs_var_refs(rhs: &TypedExpr, acc: &mut Vec<VarId>) {
    if let TypedExprKind::Perform { args, .. } = &rhs.kind {
        for a in args {
            walk_collect_var_refs(a, acc);
        }
        return;
    }
    walk_collect_var_refs(rhs, acc);
}

/// Bar B / effects (c35c) — the captured set for a let-shape resumer: the tail's free
/// VarIds (first-reference order = the captured-struct field layout) minus the let-bound
/// var (re-bound from the resumed value). Mirrors inkwell `collect_captured_vars`.
fn collect_captured_vars(tail: &TypedExpr, let_id: VarId) -> Vec<VarId> {
    let mut acc: Vec<VarId> = Vec::new();
    walk_collect_var_refs(tail, &mut acc);
    acc.into_iter().filter(|id| *id != let_id).collect()
}

fn walk_collect_var_refs(expr: &TypedExpr, acc: &mut Vec<VarId>) {
    match &expr.kind {
        TypedExprKind::Var(id) if !acc.contains(id) => acc.push(*id),
        TypedExprKind::Unary(_, inner)
        | TypedExprKind::WidenToNullable(inner)
        | TypedExprKind::WidenToSecret(inner)
        | TypedExprKind::Cast(inner)
        | TypedExprKind::Return(inner)
        | TypedExprKind::Declassify(inner) => walk_collect_var_refs(inner, acc),
        TypedExprKind::Binary(_, l, r)
        | TypedExprKind::Cmp(_, l, r)
        | TypedExprKind::Logic(_, l, r) => {
            walk_collect_var_refs(l, acc);
            walk_collect_var_refs(r, acc);
        }
        TypedExprKind::Block(b) => {
            for s in &b.stmts {
                walk_collect_var_refs_stmt(&s.kind, acc);
            }
            walk_collect_var_refs(&b.tail, acc);
        }
        TypedExprKind::If { cond, then_branch, else_branch } => {
            walk_collect_var_refs(cond, acc);
            for s in &then_branch.stmts {
                walk_collect_var_refs_stmt(&s.kind, acc);
            }
            walk_collect_var_refs(&then_branch.tail, acc);
            for s in &else_branch.stmts {
                walk_collect_var_refs_stmt(&s.kind, acc);
            }
            walk_collect_var_refs(&else_branch.tail, acc);
        }
        TypedExprKind::Call { args, .. }
        | TypedExprKind::EnumConstruct { args, .. }
        | TypedExprKind::ClassInit { args, .. }
        | TypedExprKind::QualifiedCall { args, .. } => {
            for a in args {
                walk_collect_var_refs(a, acc);
            }
        }
        TypedExprKind::MethodCall { target, args, .. }
        | TypedExprKind::ImplMethodCall { target, args, .. } => {
            walk_collect_var_refs(target, acc);
            for a in args {
                walk_collect_var_refs(a, acc);
            }
        }
        TypedExprKind::StructLit { fields, .. } => {
            for f in fields {
                walk_collect_var_refs(f, acc);
            }
        }
        TypedExprKind::ArrayLit { elements, .. } => {
            for el in elements {
                walk_collect_var_refs(el, acc);
            }
        }
        TypedExprKind::FieldAccess { target, .. } => walk_collect_var_refs(target, acc),
        TypedExprKind::Index { target, index, .. } => {
            walk_collect_var_refs(target, acc);
            walk_collect_var_refs(index, acc);
        }
        // Literals reference no vars; spawn/await/handle/match can't appear in a c35c
        // pure tail (`expr_performs` would have rejected them in `detect_let_shape`).
        // A `Perform` (the c35d embedded tail) is INTENTIONALLY skipped — its subtree
        // is the parent's to lower, so its arg vars are not part of the captured set
        // (matching inkwell's walk over the placeholder-SUBSTITUTED tail).
        _ => {}
    }
}

fn walk_collect_var_refs_stmt(kind: &TypedStmtKind, acc: &mut Vec<VarId>) {
    match kind {
        TypedStmtKind::Let { value, .. } => walk_collect_var_refs(value, acc),
        TypedStmtKind::Assign { target, value } => {
            walk_collect_var_refs(target, acc);
            walk_collect_var_refs(value, acc);
        }
        TypedStmtKind::Expr(e) => walk_collect_var_refs(e, acc),
        TypedStmtKind::While { cond, body } => {
            walk_collect_var_refs(cond, acc);
            for s in &body.stmts {
                walk_collect_var_refs_stmt(&s.kind, acc);
            }
            walk_collect_var_refs(&body.tail, acc);
        }
        TypedStmtKind::Break | TypedStmtKind::Continue => {}
    }
}

fn needs_drop(ty: Type, program: &TypedProgram) -> bool {
    match ty {
        Type::Array(_) | Type::Vec(_) => true,
        Type::Struct(id) => program
            .struct_decl(id)
            .fields
            .iter()
            .any(|f| needs_drop(f.ty, program)),
        // 8e-1: an enum needs a drop iff some variant carries a payload (a pure-unit
        // C-style enum boxes nothing). No recursion — the box-free-only drop matches
        // the production (recursive payload-field drop needs synthesized per-enum fns).
        Type::Enum(id) => program
            .enum_data(id)
            .variants
            .iter()
            .any(|v| !v.payloads.is_empty()),
        // A generic-struct instance owns drop iff some SUBSTITUTED field does
        // (`Holder<[i64]>`'s `value: T` → `[i64]` → needs drop). Mirrors the Struct
        // arm with the instance type-args applied to each declared field type.
        Type::GenericInstance(id) => {
            let inst = program.generic_instance(id);
            let struct_id = inst.struct_id;
            let inst_args = inst.args.clone();
            program.struct_decl(struct_id).fields.iter().any(|f| {
                let mut insts = program.generic_instances.clone();
                let mut refs = program.refs.clone();
                needs_drop(f.ty.substitute(&inst_args, &mut insts, &mut refs), program)
            })
        }
        _ => false,
    }
}
