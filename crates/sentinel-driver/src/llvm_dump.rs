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

use std::collections::HashMap;
use std::fmt::Write;

use sentinel_ast::{BinOp, CmpOp, LogicOp, UnaryOp};
use sentinel_borrow_check::DropPlan;
use sentinel_resolve::{
    EnumId, FnId, VarId, I64_TO_U8_FN_ID, IS_SOME_FN_ID, LEN_FN_ID, POP_FN_ID, PRINT_BYTES_FN_ID,
    PRINT_FN_ID, PUSH_FN_ID, READ_FILE_FN_ID, STR_EQ_FN_ID, U8_TO_I64_FN_ID, UNWRAP_OR_FN_ID,
    VEC_NEW_FN_ID, VEC_TO_ARRAY_FN_ID, WRITE_FILE_FN_ID,
};
use sentinel_types::{
    NullableInner, Type, TypedBlock, TypedExpr, TypedExprKind, TypedFnDef, TypedMatchArm,
    TypedPattern, TypedPatternBinding, TypedProgram, TypedStmt, TypedStmtKind,
};

/// Hardcoded for a reproducible byte-target (not host inference). `clang`
/// emits a cosmetic `-Woverride-module` note if its host triple differs;
/// the object still links + runs (probe-validated, ADR 0045).
const TARGET_TRIPLE: &str = "arm64-apple-darwin";

/// The lowest user FnId — ids 0..=13 are runtime/builtins (ADR 0044 FnId map).
const FIRST_USER_FN: u32 = 14;

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
            || self.memcpy
    }
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
                field_lls.push(llvm_ty(f.ty)?);
            }
            writeln!(out, "%Struct.{} = type {{ {} }}", sd.id.0, field_lls.join(", ")).unwrap();
        }
        emitted_struct = true;
    }
    if emitted_struct {
        out.push('\n');
    }
    // FnId order = source order; deterministic + matches the Sentinel side. Build
    // the fns into a buffer so the runtime-symbol `declare`s — emitted only for the
    // symbols the bodies actually use — can be placed BEFORE them in the module.
    let mut fns_buf = String::new();
    let mut used = RuntimeSyms::default();
    let mut fns: Vec<&TypedFnDef> = program.fns.iter().collect();
    fns.sort_by_key(|f| f.id.0);
    for f in fns {
        dump_fn(f, program, drop_plan, &mut fns_buf, &mut used)?;
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
    let sig = program.signature(f.id);
    if !sig.type_params.is_empty() {
        return Err(format!("generic fn `{}` (deferred to Bar B)", f.name));
    }
    if !sig.effect_row.is_empty() {
        return Err(format!("effecting fn `{}` (deferred to Bar B)", f.name));
    }
    // main is the C-ABI entry: i32 return (its i64 body is truncated).
    let ret_ll = if sig.is_main {
        "i32".to_string()
    } else {
        llvm_ty(f.return_type)?
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
        let ty = llvm_ty(p.ty)?;
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
    } else {
        writeln!(e.body, "  ret {ret_ll} {tail}").unwrap();
    }

    // Assemble: the `define` header, then the entry block (hoisted allocas first),
    // then the body.
    write!(out, "define {ret_ll} @{}(", f.name).unwrap();
    for (i, p) in f.params.iter().enumerate() {
        if i > 0 {
            out.push_str(", ");
        }
        write!(out, "{} %arg{i}", llvm_ty(p.ty)?).unwrap();
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

    fn lower_stmt(&mut self, stmt: &TypedStmt) -> Result<(), String> {
        match &stmt.kind {
            TypedStmtKind::Let { id, ty, value, .. } => {
                let v = self.lower_expr(value)?;
                let llty = llvm_ty(*ty)?;
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
                    let llty = llvm_ty(target.ty)?;
                    writeln!(self.body, "  store {llty} {v}, ptr %v{slot}").unwrap();
                    Ok(())
                }
                // `*r = x` / `(*c).f = x` — the target pointer (r's value, or the field
                // GEP) is emitted FIRST, then the value, then the store (the Sentinel
                // walks target-then-value, and the deref/field target DOES emit).
                TypedExprKind::Unary(UnaryOp::Deref, _) | TypedExprKind::FieldAccess { .. } => {
                    let ptr = self.lower_lvalue_ptr(target)?;
                    let v = self.lower_expr(value)?;
                    let llty = llvm_ty(target.ty)?;
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
            TypedExprKind::BoolLit(b) => Ok(if *b { "1".into() } else { "0".into() }),
            TypedExprKind::CharLit(b) => Ok(b.to_string()),
            // `secret T` lowers identically to `T` (ADR 0019 D12); declassify
            // is value-level identity. Both just flow the inner operand.
            TypedExprKind::WidenToSecret(inner) | TypedExprKind::Declassify(inner) => {
                self.lower_expr(inner)
            }
            TypedExprKind::Var(id) => {
                let slot = *self.slots.get(id).ok_or("read of an unbound var")?;
                let llty = llvm_ty(expr.ty)?;
                let v = self.fresh();
                writeln!(self.body, "  %v{v} = load {llty}, ptr %v{slot}").unwrap();
                Ok(format!("%v{v}"))
            }
            TypedExprKind::Unary(UnaryOp::Neg, inner) => {
                let x = self.lower_expr(inner)?;
                let llty = llvm_ty(expr.ty)?;
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
                let llty = llvm_ty(pointee)?;
                let v = self.fresh();
                writeln!(self.body, "  %v{v} = load {llty}, ptr {ptr}").unwrap();
                Ok(format!("%v{v}"))
            }
            TypedExprKind::Binary(op, lhs, rhs) => {
                let l = self.lower_expr(lhs)?;
                let r = self.lower_expr(rhs)?;
                let llty = llvm_ty(expr.ty)?;
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
                    let lty = llvm_ty(lhs.ty)?;
                    let rty = llvm_ty(rhs.ty)?;
                    let lv = self.fresh();
                    writeln!(self.body, "  %v{lv} = extractvalue {lty} {l}, 0").unwrap();
                    let rv = self.fresh();
                    writeln!(self.body, "  %v{rv} = extractvalue {rty} {r}, 0").unwrap();
                    let v = self.fresh();
                    writeln!(self.body, "  %v{v} = icmp {pred} i1 %v{lv}, %v{rv}").unwrap();
                    return Ok(format!("%v{v}"));
                }
                let llty = llvm_ty(lhs.ty)?;
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
                let rty = llvm_ty(then_branch.ty)?;
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
            TypedExprKind::Call { id, args, .. } => self.lower_call(*id, args, expr.ty),
            // A struct literal builds its aggregate value by an `insertvalue` chain
            // from `undef` (declaration field order; the typed `fields` are already
            // reordered to it). All field operands are lowered FIRST, then the chain
            // is emitted — a collect-then-emit shape (like a call's args) so a
            // side-effecting field value emits before the chain on BOTH backends (the
            // Sentinel side reuses its cg-collect arg stacks). The result is a single
            // SSA aggregate operand, so `let`/`Var`/param/return handle a struct
            // generically (alloca/store/load of `%Struct.N`) — no GEP needed.
            TypedExprKind::StructLit { fields, .. } => {
                let sty = llvm_ty(expr.ty)?;
                let mut field_ops = Vec::with_capacity(fields.len());
                for fv in fields {
                    let fty = llvm_ty(fv.ty)?;
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
                let sty = llvm_ty(target.ty)?;
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
                let ety = llvm_ty(elem_ty.to_type())?;
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
                let ety = llvm_ty(elem_ty.to_type())?;
                // `[T]` is `{i64,ptr}` (data field 1); `Vec<T>` is `{i64,i64,ptr}`
                // (data field 2 — capacity sits between). `len` is field 0 for both.
                let aggty = llvm_ty(target.ty)?;
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
                    _ => format!("{} 0", llvm_ty(inner.to_type())?),
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
                let sty = llvm_ty(expr.ty)?;
                let pty = llvm_ty(inner.ty)?;
                let payload = self.lower_expr(inner)?;
                let a0 = self.fresh();
                writeln!(self.body, "  %v{a0} = insertvalue {sty} undef, i1 1, 0").unwrap();
                let a1 = self.fresh();
                writeln!(self.body, "  %v{a1} = insertvalue {sty} %v{a0}, {pty} {payload}, 1").unwrap();
                Ok(format!("%v{a1}"))
            }
            _ => Err("expression not yet ported (straight-line 8a only)".into()),
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
        let rty = llvm_ty(result_ty)?;
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
            field_lls.push(llvm_ty(*t)?);
        }
        let pstruct = format!("{{ {} }}", field_lls.join(", "));
        for (i, b) in bindings.iter().enumerate() {
            let fty = llvm_ty(b.ty)?;
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
                field_lls.push(llvm_ty(*t)?);
            }
            let pstruct = format!("{{ {} }}", field_lls.join(", "));
            // Lower the args, then build the payload aggregate (declaration order).
            let mut ops = Vec::with_capacity(args.len());
            for a in args {
                ops.push((llvm_ty(a.ty)?, self.lower_expr(a)?));
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
                let slot = *self.slots.get(id).ok_or("address-of an unbound var")?;
                Ok(format!("%v{slot}"))
            }
            TypedExprKind::Unary(UnaryOp::Deref, inner) => self.lower_expr(inner),
            // 8f-3: `&(target).f` — GEP into the target's lvalue pointer. For `&mut
            // (*c).f` the target is `*c` (a Deref → c's value, a struct pointer). The
            // GEP type is the target struct (`%Struct.N`), field `field_index`.
            TypedExprKind::FieldAccess { target, field_index, .. } => {
                let target_ptr = self.lower_lvalue_ptr(target)?;
                let struct_ll = llvm_ty(target.ty)?;
                let fp = self.fresh();
                writeln!(
                    self.body,
                    "  %v{fp} = getelementptr {struct_ll}, ptr {target_ptr}, i32 0, i32 {field_index}"
                )
                .unwrap();
                Ok(format!("%v{fp}"))
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
            self.emit_drop_for_binding(slot, ty)?;
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
    fn emit_drop_for_binding(&mut self, ptr_reg: u32, ty: Type) -> Result<(), String> {
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
                    let fp = self.fresh();
                    writeln!(
                        self.body,
                        "  %v{fp} = getelementptr %Struct.{}, ptr %v{ptr_reg}, i32 0, i32 {idx}",
                        id.0
                    )
                    .unwrap();
                    self.emit_drop_for_binding(fp, fty)?;
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

    fn lower_call(&mut self, id: FnId, args: &[TypedExpr], ret_ty: Type) -> Result<String, String> {
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
            let aggty = llvm_ty(args[0].ty)?;
            let v = self.fresh();
            writeln!(self.body, "  %v{v} = extractvalue {aggty} {arr}, 0").unwrap();
            return Ok(format!("%v{v}"));
        }
        // is_some(x: ?T) -> bool (Bar B / ADR 0014 D9): extract the i1 valid bit
        // (field 0) of the `{ i1, T }` nullable struct. Inline, no runtime symbol.
        if id == IS_SOME_FN_ID {
            let x = self.lower_expr(&args[0])?;
            let nty = llvm_ty(args[0].ty)?;
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
            let nty = llvm_ty(args[0].ty)?;
            let pty = llvm_ty(ret_ty)?;
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
            let ety = llvm_ty(elem)?;
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
            let ety = llvm_ty(elem)?;
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
        if id.0 < FIRST_USER_FN {
            return Err(format!("builtin call #{} (deferred to a later slice)", id.0));
        }
        let sig = self.program.signature(id);
        if !sig.type_params.is_empty() {
            return Err("generic call (deferred to Bar B)".into());
        }
        let ret = llvm_ty(ret_ty)?;
        // Lower args to operands first, then emit the call.
        let mut arg_ops: Vec<(String, String)> = Vec::with_capacity(args.len());
        for a in args {
            let op = self.lower_expr(a)?;
            arg_ops.push((llvm_ty(a.ty)?, op));
        }
        let v = self.fresh();
        write!(self.body, "  %v{v} = call {ret} @{}(", sig.name).unwrap();
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

/// The LLVM type for a Sentinel scalar `Type` (8a subset). Anything else
/// (structs, arrays, Vec, refs, nullable, secret, …) → `Err`.
fn llvm_ty(ty: Type) -> Result<String, String> {
    match ty {
        Type::I64 => Ok("i64".into()),
        Type::I32 => Ok("i32".into()),
        Type::U8 => Ok("i8".into()),
        Type::Bool => Ok("i1".into()),
        // A user struct is the named aggregate declared in Pass 0. (Generic
        // instances — `Type::GenericInstance` — are Bar B → still Err below.)
        Type::Struct(id) => Ok(format!("%Struct.{}", id.0)),
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
                _ => llvm_ty(inner.to_type())?,
            };
            Ok(format!("{{ i1, {payload} }}"))
        }
        other => Err(format!("type not yet ported (8a scalars only): {other:?}")),
    }
}

/// 8d-drops-2: does a value of type `ty` own heap that scope-exit must `sentinel_free`?
/// An array / `Vec` always does; a struct does iff some field does (recursive — so a
/// struct of only scalars drops nothing). The Bar-B shapes (nullable / enum / generic /
/// secret) don't appear in the emitting subset, so a plain `false` suffices.
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
        _ => false,
    }
}
