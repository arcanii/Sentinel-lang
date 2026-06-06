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
use sentinel_resolve::{FnId, VarId, I64_TO_U8_FN_ID, LEN_FN_ID, U8_TO_I64_FN_ID};
use sentinel_types::{
    Type, TypedBlock, TypedExpr, TypedExprKind, TypedFnDef, TypedProgram, TypedStmt, TypedStmtKind,
};

/// Hardcoded for a reproducible byte-target (not host inference). `clang`
/// emits a cosmetic `-Woverride-module` note if its host triple differs;
/// the object still links + runs (probe-validated, ADR 0045).
const TARGET_TRIPLE: &str = "arm64-apple-darwin";

/// The lowest user FnId — ids 0..=13 are runtime/builtins (ADR 0044 FnId map).
const FIRST_USER_FN: u32 = 14;

/// Emit the canonical `.ll` for `program`, or `Err(why)` if it uses a
/// construct not yet ported (the caller exits nonzero so the differential
/// skips the fixture).
pub fn dump(program: &TypedProgram) -> Result<String, String> {
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
    let mut used_alloc = false;
    let mut used_panic = false;
    let mut fns: Vec<&TypedFnDef> = program.fns.iter().collect();
    fns.sort_by_key(|f| f.id.0);
    for f in fns {
        dump_fn(f, program, &mut fns_buf, &mut used_alloc, &mut used_panic)?;
        fns_buf.push('\n');
    }
    // Runtime-symbol declarations (8c-2+): only the symbols actually used, in a
    // fixed order, so a program using no heap/bounds stays byte-identical to 8c-1.
    if used_alloc {
        writeln!(out, "declare ptr @sentinel_alloc(i64)").unwrap();
    }
    if used_panic {
        writeln!(out, "declare void @sentinel_panic_oob(i64, i64)").unwrap();
    }
    if used_alloc || used_panic {
        out.push('\n');
    }
    out.push_str(&fns_buf);
    Ok(out)
}

fn dump_fn(
    f: &TypedFnDef,
    program: &TypedProgram,
    out: &mut String,
    used_alloc: &mut bool,
    used_panic: &mut bool,
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
        allocas: String::new(),
        body: String::new(),
        loops: Vec::new(),
        used_alloc: false,
        used_panic: false,
    };
    // Allocas are HOISTED to the entry block: param slots, `let` slots, and the
    // if-result slot all land in `e.allocas` (emitted first), while stores/loads/
    // ops go to `e.body`. This (a) lets the if-result alloca carry a type known
    // only AFTER its then-branch is walked, and (b) keeps loop-body allocas out of
    // the loop (no per-iteration stack growth — ADR 0036). The param STORE stays
    // in the body.
    for (i, p) in f.params.iter().enumerate() {
        let ty = llvm_ty(p.ty)?;
        let slot = e.alloca(&ty);
        writeln!(e.body, "  store {ty} %arg{i}, ptr %v{slot}").unwrap();
        e.slots.insert(p.id, slot);
    }
    for stmt in &f.body.stmts {
        e.lower_stmt(stmt)?;
    }
    let tail = e.lower_expr(&f.body.tail)?;
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
    *used_alloc |= e.used_alloc;
    *used_panic |= e.used_panic;
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
    allocas: String,
    body: String,
    /// The enclosing loops' (cond-block, after-block) labels — `break` branches
    /// to the innermost after-block, `continue` to its cond-block.
    loops: Vec<(u32, u32)>,
    /// Set when this fn emits a `sentinel_alloc` / `sentinel_panic_oob` call —
    /// the module emits a `declare` for each symbol it actually uses (8c-2+).
    used_alloc: bool,
    used_panic: bool,
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
                Ok(())
            }
            TypedStmtKind::Assign { target, value } => {
                let v = self.lower_expr(value)?;
                match &target.kind {
                    TypedExprKind::Var(id) => {
                        let slot = *self
                            .slots
                            .get(id)
                            .ok_or("assign to an unbound var")?;
                        let llty = llvm_ty(target.ty)?;
                        writeln!(self.body, "  store {llty} {v}, ptr %v{slot}").unwrap();
                        Ok(())
                    }
                    _ => Err("assign to a non-Var lvalue (deferred to a later slice)".into()),
                }
            }
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
                self.loops.push((cond_b, after_b));
                let _ = self.lower_block_expr(body)?; // a while body's value is discarded
                self.loops.pop();
                writeln!(self.body, "  br label %bb{cond_b}").unwrap();
                writeln!(self.body, "bb{after_b}:").unwrap();
                Ok(())
            }
            TypedStmtKind::Break | TypedStmtKind::Continue => {
                let (cond_b, after_b) =
                    *self.loops.last().ok_or("break/continue outside a loop")?;
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
                if lhs.ty.is_nullable() || rhs.ty.is_nullable() {
                    return Err("nullable comparison (deferred)".into());
                }
                let l = self.lower_expr(lhs)?;
                let r = self.lower_expr(rhs)?;
                let llty = llvm_ty(lhs.ty)?;
                let is_unsigned = matches!(lhs.ty, Type::U8);
                let pred = match op {
                    CmpOp::Eq => "eq",
                    CmpOp::Ne => "ne",
                    CmpOp::Lt => if is_unsigned { "ult" } else { "slt" },
                    CmpOp::Le => if is_unsigned { "ule" } else { "sle" },
                    CmpOp::Gt => if is_unsigned { "ugt" } else { "sgt" },
                    CmpOp::Ge => if is_unsigned { "uge" } else { "sge" },
                };
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
                let len = self.fresh();
                writeln!(self.body, "  %v{len} = extractvalue {{ i64, ptr }} {arr}, 0").unwrap();
                let dat = self.fresh();
                writeln!(self.body, "  %v{dat} = extractvalue {{ i64, ptr }} {arr}, 1").unwrap();
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
                self.used_panic = true;
                writeln!(self.body, "  unreachable").unwrap();
                writeln!(self.body, "bb{ok_b}:").unwrap();
                let ep = self.fresh();
                writeln!(self.body, "  %v{ep} = getelementptr {ety}, ptr %v{dat}, i64 {idx}").unwrap();
                let v = self.fresh();
                writeln!(self.body, "  %v{v} = load {ety}, ptr %v{ep}").unwrap();
                Ok(format!("%v{v}"))
            }
            _ => Err("expression not yet ported (straight-line 8a only)".into()),
        }
    }

    fn lower_block_expr(&mut self, b: &TypedBlock) -> Result<String, String> {
        for stmt in &b.stmts {
            self.lower_stmt(stmt)?;
        }
        self.lower_expr(&b.tail)
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
        self.used_alloc = true;
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
            // `len(arr)` = the collection's length field (`extractvalue 0`); the
            // `{ i64 len, ptr data }` layout puts `len` first for `[T]`.
            let arr = self.lower_expr(&args[0])?;
            let v = self.fresh();
            writeln!(self.body, "  %v{v} = extractvalue {{ i64, ptr }} {arr}, 0").unwrap();
            return Ok(format!("%v{v}"));
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
        other => Err(format!("type not yet ported (8a scalars only): {other:?}")),
    }
}
