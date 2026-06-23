//! sentinel-mir — a minimal SSA/CFG mid-level IR.
//!
//! Per ADR 0009 §6.1 the pipeline is `types → hir → mir → codegen`.
//! ADR 0026 D2 stands MIR up as the SSA substrate for the C5.2 D5
//! **constant-time verification**: taint-propagate `secret` through the
//! def-use graph and reject any secret value that reaches a conditional
//! branch, a memory index, or a variable-latency operation.
//!
//! Under the C5.1 escape hatch (ADR 0026 D1/D3 amendment) codegen stays
//! on the typed program; MIR is therefore **analysis-only** at 1.0 — it
//! is lowered from the typed program purely to host that verification
//! (and a future bounds-check elision), not to feed codegen.
//!
//! ## Shape — minimal SSA via block parameters
//!
//! A [`MirFunction`] is a list of basic blocks. Each [`MirBlock`] has
//! SSA *parameters* (the phi-equivalent: predecessors pass matching args
//! on their terminators), a list of [`MirInst`]s each defining exactly
//! one SSA [`MirValue`], and a [`MirTerminator`]. Every value carries its
//! [`Type`], so secrecy is read straight off the type
//! (`Type::Secret(_)`): the D5 taint pass seeds from that, and an op's
//! result is secret iff any operand is — except [`MirOp::Declassify`],
//! the one sink that clears it.
//!
//! Only the *secret-relevant* shapes are modelled precisely (the three
//! D5 sinks — a branch condition, a load index, a division operand —
//! plus `declassify`); everything else funnels through
//! [`MirOp::Opaque`] / [`MirTerminator::Unreachable`], which still carry
//! their operands so taint flows through. This keeps the IR minimal
//! while remaining *sound* for the analysis (no secret can vanish).
//!
//! ## Lowering — [`lower_to_mir`] (C5.1b 2/N)
//!
//! [`lower_to_mir`] turns each free function's type-checked body into a
//! [`MirFunction`]. Sentinel has only *structured* control flow and no
//! loops, so the CFG is a DAG and SSA falls out of a single structured
//! walk — no dominance-frontier phi placement is needed:
//!
//!   - `if` / `&&` / `||` lower to a [`MirTerminator::Branch`] into fresh
//!     blocks that reconcile at a *merge* block via SSA
//!     [`MirBlock::params`] (the phi-equivalent). `&&` / `||` are control
//!     flow because `secret bool && secret bool` type-checks (the
//!     `SecretBranch` source rejection only covers `if`) — a
//!     short-circuit branch on a secret is a leak the D5 pass must see.
//!   - A variable reassigned on one arm of an `if` is threaded through a
//!     merge parameter, in deterministic (`VarId`-sorted) order.
//!   - The three D5 sinks lower precisely — an `if`/short-circuit
//!     condition is a `Branch`, `a[i]` / `*p` are [`MirOp::Load`]s (so a
//!     secret index *or* address is visible), `a / b` is a
//!     [`MirOp::Binary`] — and `declassify(e)` is [`MirOp::Declassify`],
//!     the one taint sink. Everything else funnels through
//!     [`MirOp::Opaque`] carrying its operands so taint can't vanish.
//!
//! Codegen still consumes the typed program (the ADR 0026 D3 escape
//! hatch), so MIR stays *analysis-only*: nothing in the driver consumes
//! [`lower_to_mir`]'s output yet. The D5 verification (C5.2) is its first
//! consumer; the driver will call it as `lower_to_mir(hir.program())`
//! once D5 lands.

use sentinel_ast::{BinOp, CmpOp, LogicOp, Span, UnaryOp};
use sentinel_resolve::VarId;
use sentinel_types::{
    Type, TypedBlock, TypedExpr, TypedExprKind, TypedFnDef, TypedPattern, TypedProgram, TypedStmt,
    TypedStmtKind,
};
use std::collections::{BTreeMap, BTreeSet};

/// An SSA value, identified by its index into [`MirFunction::value_tys`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct MirValue(pub u32);

/// A basic block, identified by its index into [`MirFunction::blocks`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct MirBlockId(pub u32);

/// A lowered program: one [`MirFunction`] per (monomorphic) function.
#[derive(Debug, Clone, Default)]
pub struct MirProgram {
    pub functions: Vec<MirFunction>,
}

/// A function in SSA/CFG form.
#[derive(Debug, Clone)]
pub struct MirFunction {
    pub name: String,
    /// The type of every SSA value, indexed by [`MirValue`]. Block
    /// parameters and instruction results both allocate an entry here.
    pub value_tys: Vec<Type>,
    pub blocks: Vec<MirBlock>,
    /// The entry block; its parameters are the function parameters.
    pub entry: MirBlockId,
    pub ret_ty: Type,
}

impl MirFunction {
    /// The declared type of an SSA value.
    pub fn value_ty(&self, v: MirValue) -> Type {
        self.value_tys[v.0 as usize]
    }

    /// Whether a value is `secret`-typed — the seed for the D5 taint
    /// pass. (Taint then propagates through every op except
    /// [`MirOp::Declassify`].)
    pub fn is_secret(&self, v: MirValue) -> bool {
        matches!(self.value_ty(v), Type::Secret(_))
    }

    /// Borrow a block by id.
    pub fn block(&self, b: MirBlockId) -> &MirBlock {
        &self.blocks[b.0 as usize]
    }
}

/// A basic block: SSA parameters, straight-line instructions, terminator.
#[derive(Debug, Clone)]
pub struct MirBlock {
    /// SSA parameters (phi-equivalent): predecessors supply matching
    /// args on their [`MirTerminator`]. A parameter is `secret` iff any
    /// incoming arg is — the D5 taint pass resolves this by fixpoint.
    pub params: Vec<MirValue>,
    pub insts: Vec<MirInst>,
    pub term: MirTerminator,
}

/// An instruction: defines exactly one SSA value (`dest`) from `op`.
#[derive(Debug, Clone)]
pub struct MirInst {
    pub dest: MirValue,
    pub op: MirOp,
    /// The source span of the operation, so the D5 `secret_leak`
    /// diagnostic can point at a leaking `Load` / `Binary(Div)` sink.
    pub span: Span,
}

/// The value-producing operations.
#[derive(Debug, Clone)]
pub enum MirOp {
    ConstInt(i64),
    ConstBool(bool),
    Unary(UnaryOp, MirValue),
    /// Binary arithmetic. A `secret` operand to a variable-latency op
    /// (integer division / remainder) is a D5 leak.
    Binary(BinOp, MirValue, MirValue),
    Compare(CmpOp, MirValue, MirValue),
    /// `declassify(x)` — the only taint sink: the result is the
    /// non-secret view of `x`.
    Declassify(MirValue),
    /// A load from `base`, optionally at `index`. A `secret` `index` is
    /// a D5 leak (secret-dependent memory address).
    Load {
        base: MirValue,
        index: Option<MirValue>,
    },
    /// A call to a resolved (monomorphic) callee, named for the analysis.
    /// Operands carry taint in; the result's `Type` carries it out.
    Call {
        callee: String,
        args: Vec<MirValue>,
    },
    /// Any other value-producing construct (aggregates, field reads,
    /// widens, …) the minimal lowering doesn't model precisely. Its
    /// operands are listed so taint still flows through.
    Opaque(Vec<MirValue>),
}

/// A block terminator (the control-flow edges out of a block).
#[derive(Debug, Clone)]
pub enum MirTerminator {
    /// Unconditional jump, passing `args` to the target's block params.
    Jump {
        target: MirBlockId,
        args: Vec<MirValue>,
    },
    /// Conditional branch. A `secret` `cond` is a D5 leak (secret-
    /// dependent control flow). `span` points at the condition's source
    /// (an `if` cond, or the short-circuited operand of `&&`/`||`).
    Branch {
        cond: MirValue,
        then_blk: MirBlockId,
        then_args: Vec<MirValue>,
        else_blk: MirBlockId,
        else_args: Vec<MirValue>,
        span: Span,
    },
    /// Return an optional value.
    Return(Option<MirValue>),
    /// A terminator the minimal lowering doesn't model (an unlowered
    /// construct). A sink-free dead end for the analysis.
    Unreachable,
}

// =============================================================================
// Lowering — typed function bodies → MIR SSA (C5.1b 2/N, ADR 0026 D2)
// =============================================================================

/// Lower a type-checked program into analysis MIR: one [`MirFunction`]
/// per free function in `program.fns`, each function's body walked into
/// SSA/CFG form (see the module docs for the shape).
///
/// **Scope at this increment.** Only top-level functions are lowered.
/// Class / impl / init method bodies live in `program.class_decls` /
/// `program.impl_decls` (not `fns`); lowering them is a mechanical
/// follow-on (seed `self` + params, lower the body the same way) deferred
/// until the D5 verification needs it. Generic function *definitions* are
/// lowered as-is, with `Type::TypeParam` flowing through unchanged — they
/// are never `secret`, so they are inert for the taint analysis, and no
/// monomorphisation happens here (consistent with the D3 escape hatch:
/// MIR is analysis-only, not a codegen input).
pub fn lower_to_mir(program: &TypedProgram) -> MirProgram {
    MirProgram {
        functions: program.fns.iter().map(|f| lower_fn(program, f)).collect(),
    }
}

/// A basic block under construction: like [`MirBlock`] but with a
/// not-yet-assigned terminator, so [`FnBuilder::finish`] can assert every
/// block was terminated rather than silently leaving a placeholder.
struct BlockBuilder {
    params: Vec<MirValue>,
    insts: Vec<MirInst>,
    term: Option<MirTerminator>,
}

/// The mutable state threaded through lowering one function. SSA values
/// and blocks are append-only `Vec`s; `var_defs` maps each live source
/// variable to its current SSA value (rebound on `let` / assignment and
/// reconciled at control-flow merges).
struct FnBuilder {
    value_tys: Vec<Type>,
    blocks: Vec<BlockBuilder>,
    /// The block instructions are currently appended to.
    current: MirBlockId,
    /// `VarId → current SSA value`. A `BTreeMap` (not `HashMap`) so the
    /// merge-parameter order is deterministic across runs.
    var_defs: BTreeMap<VarId, MirValue>,
    /// ADR 0026 D5 / review F4: the VarIds that may legitimately be
    /// *unbound* in `var_defs` at use — `match`-arm pattern bindings, which
    /// are arm-scoped and not modeled by this minimal lowering (their use
    /// resolves to a fresh `Opaque`). Used ONLY by `lookup_var`'s
    /// debug-build assertion to distinguish that benign case from a genuine
    /// resolver/lowering bug; it never affects emitted IR.
    expected_unbound: BTreeSet<VarId>,
}

impl FnBuilder {
    fn new() -> Self {
        FnBuilder {
            value_tys: Vec::new(),
            blocks: Vec::new(),
            current: MirBlockId(0),
            var_defs: BTreeMap::new(),
            expected_unbound: BTreeSet::new(),
        }
    }

    /// Allocate a fresh SSA value of type `ty`.
    fn new_value(&mut self, ty: Type) -> MirValue {
        let v = MirValue(self.value_tys.len() as u32);
        self.value_tys.push(ty);
        v
    }

    /// Allocate a fresh, empty, not-yet-terminated block.
    fn new_block(&mut self) -> MirBlockId {
        let id = MirBlockId(self.blocks.len() as u32);
        self.blocks.push(BlockBuilder {
            params: Vec::new(),
            insts: Vec::new(),
            term: None,
        });
        id
    }

    /// Append an instruction (with its source span) to the current
    /// block; returns its `dest`.
    fn emit(&mut self, op: MirOp, ty: Type, span: Span) -> MirValue {
        let dest = self.new_value(ty);
        let cur = self.current.0 as usize;
        self.blocks[cur].insts.push(MirInst { dest, op, span });
        dest
    }

    /// Add an SSA parameter of type `ty` to `blk`; returns it.
    fn add_param(&mut self, blk: MirBlockId, ty: Type) -> MirValue {
        let v = self.new_value(ty);
        self.blocks[blk.0 as usize].params.push(v);
        v
    }

    /// Set a block's terminator (once).
    fn terminate(&mut self, blk: MirBlockId, term: MirTerminator) {
        self.blocks[blk.0 as usize].term = Some(term);
    }

    fn value_ty(&self, v: MirValue) -> Type {
        self.value_tys[v.0 as usize]
    }

    /// Finalise into a [`MirFunction`], asserting every block was
    /// terminated (the structured walk terminates each block by
    /// construction; a failure here is a lowering bug).
    fn finish(self, name: String, entry: MirBlockId, ret_ty: Type) -> MirFunction {
        let blocks = self
            .blocks
            .into_iter()
            .map(|b| MirBlock {
                params: b.params,
                insts: b.insts,
                term: b.term.expect("every MIR block must be terminated by lowering"),
            })
            .collect();
        MirFunction {
            name,
            value_tys: self.value_tys,
            blocks,
            entry,
            ret_ty,
        }
    }

    /// Look up a variable's current SSA value. A well-typed body binds every
    /// regular VarId before use; the one legitimate unbound case is a
    /// `match`-arm pattern binding (arm-scoped, not modeled here — recorded
    /// in [`expected_unbound`]), which resolves to a fresh `Opaque`.
    ///
    /// ADR 0026 D5 / review F4: any OTHER unbound VarId is a resolver/lowering
    /// bug. Its taint-free empty `Opaque` could mask a `secret` and
    /// *false-negative* the constant-time check, so make it LOUD in debug
    /// builds (where the test suite runs). Release builds keep the total
    /// fallback so a production compile never aborts on this edge.
    fn lookup_var(&mut self, id: VarId, ty: Type, span: Span) -> MirValue {
        match self.var_defs.get(&id) {
            Some(&v) => v,
            None => {
                debug_assert!(
                    self.expected_unbound.contains(&id),
                    "lower_to_mir: unbound VarId {id:?} is not a match-arm pattern binding — a \
                     resolver/lowering bug; its taint-free Opaque could mask a secret leak \
                     (ADR 0026 D5 / review F4)"
                );
                self.emit(MirOp::Opaque(Vec::new()), ty, span)
            }
        }
    }
}

fn lower_fn(program: &TypedProgram, f: &TypedFnDef) -> MirFunction {
    let mut b = FnBuilder::new();
    let entry = b.new_block();
    b.current = entry;
    // The function parameters are the entry block's SSA parameters.
    for p in &f.params {
        let v = b.add_param(entry, p.ty);
        b.var_defs.insert(p.id, v);
    }
    let ret = b.lower_block(program, &f.body);
    let exit = b.current;
    b.terminate(exit, MirTerminator::Return(Some(ret)));
    b.finish(f.name.clone(), entry, f.return_type)
}

impl FnBuilder {
    /// Lower a block: its statements for effect, then its tail for value.
    fn lower_block(&mut self, program: &TypedProgram, block: &TypedBlock) -> MirValue {
        for stmt in &block.stmts {
            self.lower_stmt(program, stmt);
        }
        self.lower_expr(program, &block.tail)
    }

    fn lower_stmt(&mut self, program: &TypedProgram, stmt: &TypedStmt) {
        match &stmt.kind {
            TypedStmtKind::Let { id, value, .. } => {
                let v = self.lower_expr(program, value);
                self.var_defs.insert(*id, v);
            }
            TypedStmtKind::Assign { target, value } => {
                let v = self.lower_expr(program, value);
                match &target.kind {
                    // `x = v;` rebinds the variable's current SSA value.
                    TypedExprKind::Var(id) => {
                        self.var_defs.insert(*id, v);
                    }
                    // A store to a field / deref lvalue. The minimal IR
                    // has no `Store` op; record the value as consumed into
                    // opaque memory so taint does not vanish (a store
                    // target is not itself a D5 sink — mutable indexing is
                    // out of scope per ADR 0017 D12, so no secret address
                    // is hidden here).
                    _ => {
                        self.emit(MirOp::Opaque(vec![v]), target.ty, target.span.clone());
                    }
                }
            }
            TypedStmtKind::While { cond, body } => {
                // Phase D.5 / ADR 0036: the flat taint IR lowers the
                // condition + the body once. A `secret`-typed `while`
                // condition is already rejected at TYPE-CHECK by the
                // SecretBranch rule (ADR 0019 D7, extended to `while` in
                // D.5 — `sentinel::types::secret_branch`), exactly as for
                // `if`, so this pass never sees a secret loop condition;
                // it still lowers `cond` so any secret operand reaching a
                // sink *inside* the condition expression is caught. The
                // body's stmts + tail flow taint to any sinks they contain.
                // The loop structure adds no new taint path a single pass
                // misses.
                self.lower_expr(program, cond);
                self.lower_block(program, body);
            }
            // Phase D.5 (2/N) / ADR 0036 D9: `break` / `continue` carry no
            // expression and reach no sink — the flat taint IR has nothing
            // to lower (the loop structure they redirect adds no taint path
            // a single body pass misses, as for `While` above).
            TypedStmtKind::Break | TypedStmtKind::Continue => {}
            TypedStmtKind::Expr(e) => {
                self.lower_expr(program, e);
            }
        }
    }

    fn lower_expr(&mut self, program: &TypedProgram, e: &TypedExpr) -> MirValue {
        let ty = e.ty;
        // Each instruction records its expression's span so the D5
        // `secret_leak` diagnostic can point at the sink. (Control-flow
        // arms use the condition's span instead — see `lower_if` /
        // `lower_logic` — so `span` is unused there.)
        let span = e.span.clone();
        match &e.kind {
            // ---- constants + variables -------------------------------
            TypedExprKind::IntLit(n) => self.emit(MirOp::ConstInt(*n), ty, span),
            TypedExprKind::BoolLit(b) => self.emit(MirOp::ConstBool(*b), ty, span),
            // D.2 / ADR 0033: char/string literals are public constants
            // (never secret) — model as Opaque with no operands so taint
            // can't flow in (MIR is the constant-time analysis substrate).
            TypedExprKind::CharLit(_) | TypedExprKind::StringLit(_) => {
                self.emit(MirOp::Opaque(Vec::new()), ty, span)
            }
            TypedExprKind::Var(id) => self.lookup_var(*id, ty, span),

            // ---- secret-relevant arithmetic / comparison -------------
            TypedExprKind::Unary(op, inner) => {
                let v = self.lower_expr(program, inner);
                match op {
                    // `*p` is a memory load; model it as a `Load` so the
                    // D5 pass can flag a secret pointer (the
                    // `SecretInRefDeref` class of leak).
                    UnaryOp::Deref => self.emit(MirOp::Load { base: v, index: None }, ty, span),
                    _ => self.emit(MirOp::Unary(*op, v), ty, span),
                }
            }
            TypedExprKind::Binary(op, l, r) => {
                let vl = self.lower_expr(program, l);
                let vr = self.lower_expr(program, r);
                // `Div` is the variable-latency op; a secret divisor `vr`
                // is the D5 leak the pass reads off this instruction.
                self.emit(MirOp::Binary(*op, vl, vr), ty, span)
            }
            TypedExprKind::Cmp(op, l, r) => {
                let vl = self.lower_expr(program, l);
                let vr = self.lower_expr(program, r);
                self.emit(MirOp::Compare(*op, vl, vr), ty, span)
            }
            TypedExprKind::Logic(op, l, r) => self.lower_logic(program, *op, l, r, ty),

            // ---- the secret qualifier itself -------------------------
            TypedExprKind::Declassify(inner) => {
                let v = self.lower_expr(program, inner);
                self.emit(MirOp::Declassify(v), ty, span)
            }
            // `T → secret T` / `T → ?T` widening is identity at runtime;
            // carry the operand and let the result type speak (the
            // `secret`/`?` bit lives on `ty`).
            TypedExprKind::WidenToSecret(inner) | TypedExprKind::WidenToNullable(inner) => {
                let v = self.lower_expr(program, inner);
                self.emit(MirOp::Opaque(vec![v]), ty, span)
            }
            TypedExprKind::NullLit => self.emit(MirOp::Opaque(Vec::new()), ty, span),

            // ---- control flow ----------------------------------------
            TypedExprKind::Block(b) => self.lower_block(program, b),
            TypedExprKind::If {
                cond,
                then_branch,
                else_branch,
            } => self.lower_if(program, cond, then_branch, else_branch, ty),

            // ---- calls + memory --------------------------------------
            TypedExprKind::Call { id, args, .. } => {
                let arg_vals: Vec<MirValue> =
                    args.iter().map(|a| self.lower_expr(program, a)).collect();
                let callee = program.signature(*id).name.clone();
                self.emit(MirOp::Call { callee, args: arg_vals }, ty, span)
            }
            TypedExprKind::Index { target, index, .. } => {
                let base = self.lower_expr(program, target);
                let idx = self.lower_expr(program, index);
                // A secret `index` is the D5 leak (secret-dependent
                // address) the pass reads off this load.
                self.emit(MirOp::Load { base, index: Some(idx) }, ty, span)
            }

            // ---- everything else → Opaque (operands carried) ---------
            TypedExprKind::StructLit { fields, .. } => {
                let vals: Vec<MirValue> =
                    fields.iter().map(|f| self.lower_expr(program, f)).collect();
                self.emit(MirOp::Opaque(vals), ty, span)
            }
            TypedExprKind::FieldAccess { target, .. } => {
                let v = self.lower_expr(program, target);
                self.emit(MirOp::Opaque(vec![v]), ty, span)
            }
            TypedExprKind::ArrayLit { elements, .. } => {
                let vals: Vec<MirValue> =
                    elements.iter().map(|el| self.lower_expr(program, el)).collect();
                self.emit(MirOp::Opaque(vals), ty, span)
            }
            // Receiver + args: a method/impl-method call.
            TypedExprKind::MethodCall { target, args, .. }
            | TypedExprKind::ImplMethodCall { target, args, .. } => {
                let mut vals = vec![self.lower_expr(program, target)];
                vals.extend(args.iter().map(|a| self.lower_expr(program, a)));
                self.emit(MirOp::Opaque(vals), ty, span)
            }
            // Args-only forms (the receiver, if any, is already in `args`).
            TypedExprKind::QualifiedCall { args, .. }
            | TypedExprKind::ClassInit { args, .. }
            | TypedExprKind::Perform { args, .. }
            | TypedExprKind::ResumeKont { args, .. } => {
                let vals: Vec<MirValue> =
                    args.iter().map(|a| self.lower_expr(program, a)).collect();
                self.emit(MirOp::Opaque(vals), ty, span)
            }
            // A handler's arms bind their own (handler-scoped) `VarId`s
            // that this walk does not model, so lower only the body — its
            // straight-line secret-relevant ops still surface. (A
            // minimal-lowering boundary; handler arms are not on the 1.0
            // constant-time path.)
            TypedExprKind::Handle { body, .. } => {
                let v = self.lower_expr(program, body);
                self.emit(MirOp::Opaque(vec![v]), ty, span)
            }
            // `scope concurrent { .. }` is value-transparent (its value is
            // the body's tail); lower the block inline so any nested
            // control flow inside stays precise.
            TypedExprKind::Scope { body, .. } => self.lower_block(program, body),
            TypedExprKind::Spawn { call, .. } => {
                let v = self.lower_expr(program, call);
                self.emit(MirOp::Opaque(vec![v]), ty, span)
            }
            TypedExprKind::Await { task_expr, .. } => {
                let v = self.lower_expr(program, task_expr);
                self.emit(MirOp::Opaque(vec![v]), ty, span)
            }
            // Phase D.1 / ADR 0032 (3/N): enum construction carries its
            // payload args; `match` carries the scrutinee + every arm
            // body. Both funnel through `Opaque` so any secret operand
            // stays in the taint graph. There is no `secret enum` at the
            // MVP, so a `match` tag is never itself secret (the D7
            // secret-tag-branch sink is a future guard); operands are
            // carried for soundness. Pattern bindings are arm-scoped
            // `VarId`s this walk doesn't model — `lookup_var` resolves
            // them to `Opaque`, so arm bodies lower without panicking.
            // Real `{tag,ptr}` + `switch` lowering is codegen's at (4/N).
            TypedExprKind::EnumConstruct { args, .. } => {
                let vals: Vec<MirValue> =
                    args.iter().map(|a| self.lower_expr(program, a)).collect();
                self.emit(MirOp::Opaque(vals), ty, span)
            }
            TypedExprKind::Match { scrutinee, arms, .. } => {
                let mut vals = vec![self.lower_expr(program, scrutinee)];
                for arm in arms {
                    // ADR 0026 D5 / review F4: an arm's pattern bindings are
                    // arm-scoped VarIds this lowering doesn't model (their use
                    // resolves to a fresh `Opaque`). Record them so
                    // `lookup_var`'s unbound assert treats them as benign and
                    // not a resolver bug.
                    if let TypedPattern::Variant { bindings, .. } = &arm.pattern {
                        for b in bindings {
                            self.expected_unbound.insert(b.var_id);
                        }
                    }
                    vals.push(self.lower_expr(program, &arm.body));
                }
                self.emit(MirOp::Opaque(vals), ty, span)
            }
        }
    }

    /// Lower `if cond { then } else { else }` to a `Branch` into two
    /// blocks that reconcile at a merge block. The merge takes the `if`'s
    /// result as its first SSA parameter, plus one parameter per variable
    /// the two arms left holding different values.
    fn lower_if(
        &mut self,
        program: &TypedProgram,
        cond: &TypedExpr,
        then_b: &TypedBlock,
        else_b: &TypedBlock,
        result_ty: Type,
    ) -> MirValue {
        let cond_val = self.lower_expr(program, cond);
        let then_blk = self.new_block();
        let else_blk = self.new_block();
        let pred = self.current;
        let defs_at_branch = self.var_defs.clone();
        self.terminate(
            pred,
            MirTerminator::Branch {
                cond: cond_val,
                then_blk,
                then_args: Vec::new(),
                else_blk,
                else_args: Vec::new(),
                span: cond.span.clone(),
            },
        );

        // Each arm starts from the variable environment at the branch.
        self.current = then_blk;
        self.var_defs = defs_at_branch.clone();
        let then_val = self.lower_block(program, then_b);
        let then_exit = self.current;
        let then_defs = self.var_defs.clone();

        self.current = else_blk;
        self.var_defs = defs_at_branch.clone();
        let else_val = self.lower_block(program, else_b);
        let else_exit = self.current;
        let else_defs = self.var_defs.clone();

        // Merge: the result first, then any variable that diverged.
        let merge = self.new_block();
        let result = self.add_param(merge, result_ty);
        let mut then_args = vec![then_val];
        let mut else_args = vec![else_val];
        let mut merged = defs_at_branch.clone();
        for &var in defs_at_branch.keys() {
            let tv = then_defs[&var];
            let ev = else_defs[&var];
            if tv != ev {
                let pty = self.value_ty(tv);
                let p = self.add_param(merge, pty);
                then_args.push(tv);
                else_args.push(ev);
                merged.insert(var, p);
            }
        }
        self.terminate(
            then_exit,
            MirTerminator::Jump {
                target: merge,
                args: then_args,
            },
        );
        self.terminate(
            else_exit,
            MirTerminator::Jump {
                target: merge,
                args: else_args,
            },
        );
        self.current = merge;
        self.var_defs = merged;
        result
    }

    /// Lower a short-circuit `&&` / `||` as control flow: branch on the
    /// left operand to either evaluate the right operand or short-circuit
    /// to a constant, merging the two. Modelling it as a `Branch` (rather
    /// than a binary op) is what lets the D5 pass see a secret-dependent
    /// short-circuit — `secret bool && _` type-checks (only `if` is
    /// rejected by `SecretBranch`).
    fn lower_logic(
        &mut self,
        program: &TypedProgram,
        op: LogicOp,
        l: &TypedExpr,
        r: &TypedExpr,
        ty: Type,
    ) -> MirValue {
        let lv = self.lower_expr(program, l);
        let rhs_blk = self.new_block();
        let short_blk = self.new_block();
        let pred = self.current;
        // `&&`: left-true evaluates the right; left-false short-circuits
        // to `false`.  `||`: left-true short-circuits to `true`.
        let (then_blk, else_blk) = match op {
            LogicOp::And => (rhs_blk, short_blk),
            LogicOp::Or => (short_blk, rhs_blk),
        };
        self.terminate(
            pred,
            MirTerminator::Branch {
                cond: lv,
                then_blk,
                then_args: Vec::new(),
                else_blk,
                else_args: Vec::new(),
                span: l.span.clone(),
            },
        );

        // The right operand is itself an expression (no assignments), so
        // its lowering rebinds no pre-existing variable — only the result
        // needs reconciling at the merge.
        self.current = rhs_blk;
        let rv = self.lower_expr(program, r);
        let rhs_exit = self.current;

        self.current = short_blk;
        let short_val = self.emit(MirOp::ConstBool(matches!(op, LogicOp::Or)), ty, l.span.clone());

        let merge = self.new_block();
        let result = self.add_param(merge, ty);
        self.terminate(
            rhs_exit,
            MirTerminator::Jump {
                target: merge,
                args: vec![rv],
            },
        );
        self.terminate(
            short_blk,
            MirTerminator::Jump {
                target: merge,
                args: vec![short_val],
            },
        );
        self.current = merge;
        result
    }
}

// =============================================================================
// D5 — the constant-time verification pass (C5.2, ADR 0026 D5)
// =============================================================================

/// Which timing-observable sink a `secret` value reached — the three
/// constant-time sinks of ADR 0026 D5 (`SinkKind::MemoryIndex` and
/// `MemoryAddress` are the two ways a [`MirOp::Load`] can leak).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SinkKind {
    /// The condition of a conditional branch — secret-dependent control
    /// flow (an `if`, or a `&&` / `||` short-circuit).
    Branch,
    /// The index of a memory load (`a[i]`) — a secret-dependent address.
    MemoryIndex,
    /// The base pointer of a load (`*p`) — a secret pointer.
    MemoryAddress,
    /// The divisor of an integer division — a variable-latency operation.
    Division,
}

impl std::fmt::Display for SinkKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            SinkKind::Branch => "a conditional branch",
            SinkKind::MemoryIndex => "a memory index",
            SinkKind::MemoryAddress => "a memory address",
            SinkKind::Division => "a variable-latency division",
        })
    }
}

/// A `secret` value reached a timing-observable sink: the
/// `sentinel::mir::secret_leak` constant-time violation (ADR 0026 D5).
/// The driver renders this as a what/why/how diagnostic.
#[derive(Debug, Clone, thiserror::Error, miette::Diagnostic)]
#[error("a `secret` value reaches {sink}, which would leak it via timing")]
#[diagnostic(
    code(sentinel::mir::secret_leak),
    help(
        "a secret must not influence control flow, a memory address, or a \
         variable-latency instruction; restructure to be branch-free (e.g. a \
         masked select) or `declassify` the value first if the leak is acceptable"
    )
)]
pub struct SecretLeak {
    /// Which sink the secret reached (interpolated into the message).
    pub sink: SinkKind,
    #[label("secret value reaches the sink here")]
    pub span: miette::SourceSpan,
}

fn secret_leak(sink: SinkKind, span: &Span) -> SecretLeak {
    SecretLeak {
        sink,
        span: (span.start, span.len()).into(),
    }
}

/// Verify the constant-time discipline over a lowered program
/// (ADR 0026 D5): no `secret` value may reach the condition of a
/// conditional branch, the index or base address of a memory load, or
/// the divisor of an integer division. Returns one [`SecretLeak`] per
/// violation — an empty result means the program is constant-time at the
/// MIR level. This is the machine-checkable expression of ADR 0008's
/// guarantee, and the first consumer of [`lower_to_mir`].
///
/// **Taint oracle.** Each SSA value carries its [`Type`], and the type
/// checker's operator-secret-preserving rules already computed the taint
/// fixpoint — a value's type is `secret` iff a source-reachable operand
/// was, with `declassify` clearing it and function-signature boundaries
/// respected. So the pass reads taint straight off the type
/// ([`MirFunction::is_secret`]) and inspects each sink; no separate
/// def-use propagation is needed here. (That standalone forward
/// propagation only becomes necessary once MIR is lowered from
/// *post-optimisation* code, where the optimiser may have produced a
/// secret-derived value the type no longer marks — a **post-1.0** form,
/// recorded as an amendment to ADR 0026 D5. At 1.0 the additional leak
/// this pass catches over the C3.1 source rejections is the
/// `secret bool && secret bool` short-circuit, which type-checks because
/// `SecretBranch` only rejects `if`.)
pub fn verify_constant_time(program: &MirProgram) -> Vec<SecretLeak> {
    let mut leaks = Vec::new();
    for f in &program.functions {
        for block in &f.blocks {
            for inst in &block.insts {
                match &inst.op {
                    MirOp::Load { base, index } => {
                        if let Some(idx) = index {
                            if f.is_secret(*idx) {
                                leaks.push(secret_leak(SinkKind::MemoryIndex, &inst.span));
                            }
                        }
                        if f.is_secret(*base) {
                            leaks.push(secret_leak(SinkKind::MemoryAddress, &inst.span));
                        }
                    }
                    MirOp::Binary(BinOp::Div, _, divisor) if f.is_secret(*divisor) => {
                        leaks.push(secret_leak(SinkKind::Division, &inst.span));
                    }
                    _ => {}
                }
            }
            if let MirTerminator::Branch { cond, span, .. } = &block.term {
                if f.is_secret(*cond) {
                    leaks.push(secret_leak(SinkKind::Branch, span));
                }
            }
        }
    }
    leaks
}

#[cfg(test)]
mod tests {
    use super::*;
    use sentinel_types::SecretId;

    /// Hand-build the SSA for `fn f(s: secret bool) -> i64 { if s { 1 }
    /// else { 0 } }` — shapes only — and confirm the data model
    /// round-trips and the secret seed the D5 pass keys off is readable.
    #[test]
    fn builds_a_function_with_a_secret_branch() {
        let secret = Type::Secret(SecretId(0)); // a secret bool, for the analysis
        let f = MirFunction {
            name: "f".to_string(),
            // v0 = secret param, v1 = const 1, v2 = const 0
            value_tys: vec![secret, Type::I64, Type::I64],
            entry: MirBlockId(0),
            ret_ty: Type::I64,
            blocks: vec![
                // entry(s): branch s ? blk1 : blk2
                MirBlock {
                    params: vec![MirValue(0)],
                    insts: vec![],
                    term: MirTerminator::Branch {
                        cond: MirValue(0),
                        then_blk: MirBlockId(1),
                        then_args: vec![],
                        else_blk: MirBlockId(2),
                        else_args: vec![],
                        span: 0..0,
                    },
                },
                // blk1: v1 = 1; return v1
                MirBlock {
                    params: vec![],
                    insts: vec![MirInst {
                        dest: MirValue(1),
                        op: MirOp::ConstInt(1),
                        span: 0..0,
                    }],
                    term: MirTerminator::Return(Some(MirValue(1))),
                },
                // blk2: v2 = 0; return v2
                MirBlock {
                    params: vec![],
                    insts: vec![MirInst {
                        dest: MirValue(2),
                        op: MirOp::ConstInt(0),
                        span: 0..0,
                    }],
                    term: MirTerminator::Return(Some(MirValue(2))),
                },
            ],
        };

        assert_eq!(f.blocks.len(), 3);
        assert!(f.is_secret(MirValue(0)), "the secret param is tainted");
        assert!(!f.is_secret(MirValue(1)), "an i64 constant is not secret");

        // The branch condition is the value the D5 pass will flag when
        // secret — here it is secret, so this function would be rejected.
        match &f.block(f.entry).term {
            MirTerminator::Branch { cond, .. } => {
                assert!(f.is_secret(*cond), "secret-conditional branch = a D5 leak");
            }
            _ => panic!("entry block should end in a branch"),
        }

        // And the D5 pass flags exactly this one secret-conditional branch.
        let leaks = verify_constant_time(&MirProgram { functions: vec![f] });
        assert_eq!(leaks.len(), 1);
        assert_eq!(leaks[0].sink, SinkKind::Branch);
    }

    // ---- lower_to_mir (C5.1b 2/N) -------------------------------------

    /// Drive the real front-end (parse → resolve → check) and lower the
    /// result — the way `mir_query` will once it is wired at C5.2.
    fn lower(src: &str) -> MirProgram {
        let prog = sentinel_syntax::parse(src).expect("parse");
        let resolved = sentinel_resolve::resolve(&prog).expect("resolve");
        let typed = sentinel_types::check(&resolved).expect("check");
        lower_to_mir(&typed)
    }

    /// The lowered function named `name`.
    fn func<'a>(p: &'a MirProgram, name: &str) -> &'a MirFunction {
        p.functions
            .iter()
            .find(|f| f.name == name)
            .expect("the named function was lowered")
    }

    /// Every instruction in a function, flattened across its blocks.
    fn insts(f: &MirFunction) -> Vec<&MirInst> {
        f.blocks.iter().flat_map(|b| b.insts.iter()).collect()
    }

    #[test]
    fn lowers_straight_line_arithmetic_to_one_block() {
        let p = lower("fn dbl(x: i64) -> i64 { x * 2 }\nfn main() -> i64 { dbl(21) }");
        let f = func(&p, "dbl");
        assert_eq!(f.blocks.len(), 1, "no control flow ⇒ a single block");
        assert_eq!(f.blocks[0].params.len(), 1, "the entry param is `x`");
        assert!(!f.is_secret(f.blocks[0].params[0]), "a plain i64 param is not secret");
        assert!(
            matches!(f.blocks[0].term, MirTerminator::Return(Some(_))),
            "the block returns its tail value"
        );
        assert!(
            insts(f).iter().any(|i| matches!(i.op, MirOp::Binary(BinOp::Mul, ..))),
            "`x * 2` lowers to a Binary(Mul)"
        );
    }

    #[test]
    fn match_arm_payload_binding_lowers_without_tripping_the_unbound_assert() {
        // ADR 0026 D5 / review F4: a `match`-arm pattern binding (`r`, `w`,
        // `h`) is arm-scoped and unbound in `var_defs`; its use must resolve
        // to a fresh `Opaque` (taint preserved by type), NOT trip
        // `lookup_var`'s unbound `debug_assert!`. `expected_unbound` records
        // it as the benign case. In a debug build a regression panics — so
        // that this lowers at all is the guard's positive test.
        let p = lower(
            "enum Shape { Unit, Circle(i64), Rect(i64, i64) }\n\
             fn area(s: Shape) -> i64 {\n\
                 match s {\n\
                     Shape::Unit => 0,\n\
                     Shape::Circle(r) => r * r * 3,\n\
                     Shape::Rect(w, h) => w * h,\n\
                 }\n\
             }\n\
             fn main() -> i64 { area(Shape::Unit) }",
        );
        let f = func(&p, "area");
        assert!(
            insts(f).iter().any(|i| matches!(i.op, MirOp::Opaque(_))),
            "the match (and its arm-scoped bindings) funnel through Opaque"
        );
    }

    #[test]
    fn lowers_if_to_branch_and_a_merge_param() {
        let p = lower("fn pick(c: bool) -> i64 { if c { 1 } else { 2 } }\nfn main() -> i64 { 0 }");
        let f = func(&p, "pick");
        assert_eq!(f.blocks.len(), 4, "entry + then + else + merge");
        assert!(
            matches!(f.blocks[0].term, MirTerminator::Branch { .. }),
            "the entry branches on the condition"
        );
        // The merge block is the one the function returns from; it carries
        // exactly the `if`'s result (no variable diverged across the arms).
        let merge = f
            .blocks
            .iter()
            .find(|b| matches!(b.term, MirTerminator::Return(Some(_))))
            .expect("a returning block");
        assert_eq!(merge.params.len(), 1, "the merge carries just the if-result");
        // Both arms jump to that same merge block.
        let targets: Vec<MirBlockId> = f
            .blocks
            .iter()
            .filter_map(|b| match &b.term {
                MirTerminator::Jump { target, .. } => Some(*target),
                _ => None,
            })
            .collect();
        assert_eq!(targets.len(), 2, "both arms jump");
        assert_eq!(targets[0], targets[1], "...to the same merge block");
    }

    #[test]
    fn lowers_logical_and_to_a_short_circuit_branch() {
        // `&&` is control flow, not a binary op — this is what lets the D5
        // pass flag a secret-dependent short-circuit (which type-checks,
        // since `SecretBranch` only rejects `if`).
        let p = lower("fn both(a: bool, b: bool) -> bool { a && b }\nfn main() -> i64 { 0 }");
        let f = func(&p, "both");
        assert!(
            matches!(f.blocks[0].term, MirTerminator::Branch { .. }),
            "short-circuit && lowers to a Branch"
        );
        assert!(
            insts(f).iter().any(|i| matches!(i.op, MirOp::ConstBool(false))),
            "&& short-circuits to the constant false"
        );
    }

    #[test]
    fn secret_param_seeds_taint_and_declassify_clears_it() {
        let p = lower(
            "fn f(a: secret i64, b: i64) -> i64 { declassify(a) / b }\nfn main() -> i64 { 0 }",
        );
        let f = func(&p, "f");
        assert!(f.is_secret(f.blocks[0].params[0]), "the `secret i64` param is the taint seed");
        assert!(!f.is_secret(f.blocks[0].params[1]), "the plain i64 param is not secret");
        assert!(
            insts(f).iter().any(|i| matches!(i.op, MirOp::Declassify(_))),
            "declassify lowers to the one taint-clearing op"
        );
        // The `/` is a Binary(Div) — the variable-latency sink — and here
        // its divisor is the declassified (public) value, so it is no leak.
        let all = insts(f);
        let div = all
            .iter()
            .find(|i| matches!(i.op, MirOp::Binary(BinOp::Div, ..)))
            .expect("a `/`");
        assert!(!f.is_secret(div.dest), "declassify cleared the secret before the `/`");
        if let MirOp::Binary(BinOp::Div, _, divisor) = &div.op {
            assert!(!f.is_secret(*divisor), "the divisor operand is the public `b`");
        }
    }

    #[test]
    fn lowers_index_to_a_load_carrying_its_index() {
        let p = lower("fn get(a: [i64], i: i64) -> i64 { a[i] }\nfn main() -> i64 { 0 }");
        let f = func(&p, "get");
        assert!(
            insts(f)
                .iter()
                .any(|i| matches!(i.op, MirOp::Load { index: Some(_), .. })),
            "`a[i]` lowers to a Load carrying its index (the D5 index sink)"
        );
    }

    #[test]
    fn lowers_call_naming_the_concrete_callee() {
        let p = lower("fn helper(x: i64) -> i64 { x }\nfn main() -> i64 { helper(7) }");
        let f = func(&p, "main");
        let all = insts(f);
        let call = all
            .iter()
            .find(|i| matches!(i.op, MirOp::Call { .. }))
            .expect("a call");
        if let MirOp::Call { callee, .. } = &call.op {
            assert_eq!(callee.as_str(), "helper", "the call records its concrete callee");
        }
    }

    #[test]
    fn reassignment_in_a_branch_threads_a_merge_param() {
        // `x` is reassigned on both arms, so after the `if` it must come
        // from a merge parameter, and the tail `x` reads that parameter.
        let src = concat!(
            "fn f(c: bool) -> i64 { let mut x = 10; if c { x = 1; 0 } else { x = 2; 0 }; x }\n",
            "fn main() -> i64 { 0 }"
        );
        let f_prog = lower(src);
        let f = func(&f_prog, "f");
        assert_eq!(f.blocks.len(), 4, "entry + then + else + merge");
        let merge = f
            .blocks
            .iter()
            .find(|b| matches!(b.term, MirTerminator::Return(Some(_))))
            .expect("a returning block");
        assert_eq!(
            merge.params.len(),
            2,
            "the merge carries the (discarded) if-result and the reconciled `x`"
        );
        if let MirTerminator::Return(Some(ret)) = &merge.term {
            assert!(merge.params.contains(ret), "the tail `x` reads a merge param");
            assert_eq!(*ret, merge.params[1], "...the reconciled-variable param");
        }
    }

    // ---- verify_constant_time (C5.2 / D5) -----------------------------

    #[test]
    fn verify_flags_a_secret_short_circuit() {
        // `secret bool && secret bool` type-checks — `SecretBranch` only
        // rejects `if` — and lowers to a secret-conditional Branch, so D5
        // catches the leak the source rejections miss.
        let p = lower(concat!(
            "fn leaky(a: secret bool, b: secret bool) -> secret bool { a && b }\n",
            "fn main() -> i64 { 0 }"
        ));
        let leaks = verify_constant_time(&p);
        assert!(
            leaks.iter().any(|l| l.sink == SinkKind::Branch),
            "a secret && short-circuit is a branch leak"
        );
    }

    #[test]
    fn verify_passes_branch_free_secret_arithmetic() {
        // A branch-free masked select over secrets (`c*a + (1-c)*b`) has no
        // sink at all, so it is constant-time and D5 accepts it.
        let p = lower(concat!(
            "fn ct_select(c: secret i64, a: secret i64, b: secret i64) -> secret i64 {\n",
            "  let one: secret i64 = 1;\n",
            "  c * a + (one - c) * b\n",
            "}\n",
            "fn main() -> i64 { 0 }"
        ));
        assert!(
            verify_constant_time(&p).is_empty(),
            "branch-free secret arithmetic is constant-time"
        );
    }

    #[test]
    fn verify_passes_after_declassify() {
        // `declassify` clears the secret, so the following `/` is public.
        let p = lower(
            "fn f(a: secret i64, b: i64) -> i64 { declassify(a) / b }\nfn main() -> i64 { 0 }",
        );
        assert!(
            verify_constant_time(&p).is_empty(),
            "a declassified value is no longer a secret at the `/`"
        );
    }

    #[test]
    fn verify_flags_hand_built_secret_divisor_and_index() {
        // A secret divisor / index is *type-rejected* at the source
        // (SecretDivisor / IndexNotInt), so build the MIR by hand to prove
        // the pass inspects those two sinks (the ones an optimiser could
        // reintroduce below the type level).
        let secret = Type::Secret(SecretId(0));
        // v0: secret (the divisor + the index); v1: const; v2: div result;
        // v3: opaque base; v4: load result.
        let f = MirFunction {
            name: "h".to_string(),
            value_tys: vec![secret, Type::I64, Type::I64, Type::I64, Type::I64],
            entry: MirBlockId(0),
            ret_ty: Type::I64,
            blocks: vec![MirBlock {
                params: vec![MirValue(0)],
                insts: vec![
                    MirInst { dest: MirValue(1), op: MirOp::ConstInt(10), span: 0..0 },
                    // v2 = 10 / secret  → a secret divisor.
                    MirInst {
                        dest: MirValue(2),
                        op: MirOp::Binary(BinOp::Div, MirValue(1), MirValue(0)),
                        span: 0..0,
                    },
                    // v3 = (opaque array base), v4 = base[secret] → secret index.
                    MirInst { dest: MirValue(3), op: MirOp::Opaque(vec![]), span: 0..0 },
                    MirInst {
                        dest: MirValue(4),
                        op: MirOp::Load { base: MirValue(3), index: Some(MirValue(0)) },
                        span: 0..0,
                    },
                ],
                term: MirTerminator::Return(Some(MirValue(4))),
            }],
        };
        let leaks = verify_constant_time(&MirProgram { functions: vec![f] });
        assert!(leaks.iter().any(|l| l.sink == SinkKind::Division), "secret divisor");
        assert!(leaks.iter().any(|l| l.sink == SinkKind::MemoryIndex), "secret index");
        // The opaque base is not secret, so no MemoryAddress leak.
        assert!(!leaks.iter().any(|l| l.sink == SinkKind::MemoryAddress));
    }
}
