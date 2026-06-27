//! Phase D self-host port (8g) / ADR 0045: **merge-to-source** — emit a
//! parsed [`Program`] back to *re-parseable* Sentinel source.
//!
//! This is the lighter of the two bootstrap-fixed-point paths (ADR 0045
//! D8(ii), owner-chosen): rather than port the D.6 module-merge machinery
//! (`discover_module_graph` + `merge_modules` + the `Renamer`) to Sentinel,
//! the Rust driver merges the multi-module compiler into one `Program`
//! (the existing `merge_modules`) and prints it back to a single
//! `.sentinel` file. The single-file Sentinel codegen (`scg`) then reads
//! that one file — no change to `scg` itself — so `snc llvm` and `scg`
//! lower the *same* merged source and must emit byte-identical `.ll`.
//!
//! **Fidelity by construction.** The merged `Program` is the AST from the
//! Rust parser *after* `merge_modules` qualified every top-level name by
//! module path (`util$add`, `parser$Expr` — `$` admitted as an identifier
//! char by the lexer, ADR 0045 (8g)). We re-emit it so that re-parsing
//! yields the *structurally identical* AST (spans differ, but spans do not
//! reach the `.ll`):
//!   - **Parens are AST-transparent** (no `Paren` node — ast/lib.rs:147),
//!     so every compound expression is wrapped in `( … )`. This preserves
//!     operator precedence/associativity *and* sidesteps every positional
//!     ambiguity (struct-literal-in-head, etc.) without re-deriving the
//!     parser's `allow_struct_lit` state.
//!   - **Decls are emitted per kind** (structs, then enums, then fns), each
//!     in `Program`-vector order. The parser buckets top-level items by
//!     kind, so intra-kind order — the only order that fixes StructId /
//!     EnumId / FnId — round-trips exactly; inter-kind order is irrelevant
//!     (resolve builds all symbol tables before any body).
//!   - **String/char bytes are re-encoded** from the AST's *decoded* bytes
//!     via `\xHH` (or a bare printable byte), which the parser decodes back
//!     to the same bytes (ADR 0033 D2).
//!
//! **Scope = Bar A.** The selfhost sources use no traits / impls / classes
//! / effects / handlers / concurrency / declassify (ADR 0045 D7). Those
//! kinds are rejected with a clear `Err` rather than emitted — the tool is
//! correct-or-loud over the bootstrap subset, never silently wrong.

use sentinel_ast::{
    BinOp, Block, ClassDecl, CmpOp, DelegateDecl, EffectDecl, EnumDecl, Expr, ExprKind, FnDef,
    HandlerArm, ImplDecl, InitDef, LogicOp, OpDecl, Param, Pattern, Program, SelfKind,
    Stmt, StmtKind, StructDecl, TraitDecl, TraitMethodSig, TypeExpr, TypeExprKind, UnaryOp,
    Visibility,
};

/// Emit `program` as re-parseable Sentinel source, or an error naming the
/// first construct outside the Bar-A subset this printer supports.
pub fn dump(program: &Program) -> Result<String, String> {
    let mut out = String::new();
    // Per-kind, in vector order (intra-kind order fixes the IDs; inter-kind
    // order is free — resolve builds all tables before any body). Traits/impls/
    // classes follow the fns: a delegate inside a class re-synthesizes its impl at
    // resolve time, so its ImplId lands right after the user impls — preserved by
    // emitting each kind's vector in order.
    for s in &program.structs {
        emit_struct(&mut out, s)?;
    }
    for e in &program.enums {
        emit_enum(&mut out, e)?;
    }
    for f in &program.fns {
        emit_fn(&mut out, f)?;
    }
    for t in &program.traits {
        emit_trait(&mut out, t)?;
    }
    for i in &program.impls {
        emit_impl(&mut out, i)?;
    }
    for c in &program.classes {
        emit_class(&mut out, c)?;
    }
    for ef in &program.effects {
        emit_effect(&mut out, ef)?;
    }
    Ok(out)
}

// --- effects (Bar B / effects) -----------------------------------------------

fn emit_effect(out: &mut String, ef: &EffectDecl) -> Result<(), String> {
    if ef.visibility == Visibility::Public {
        out.push_str("pub ");
    }
    out.push_str("effect ");
    out.push_str(&ef.name);
    out.push_str(" { ");
    for op in &ef.ops {
        emit_op_decl(out, op);
        out.push(' ');
    }
    out.push_str("}\n");
    Ok(())
}

/// `op(params) -> ret;` — the return type is omitted when `None` (the parser accepts
/// a bodyless `op(params);`, ADR 0020).
fn emit_op_decl(out: &mut String, op: &OpDecl) {
    out.push_str(&op.name);
    out.push('(');
    for (i, p) in op.params.iter().enumerate() {
        if i > 0 {
            out.push_str(", ");
        }
        emit_param(out, p);
    }
    out.push(')');
    if let Some(rt) = &op.return_type {
        out.push_str(" -> ");
        emit_type(out, rt);
    }
    out.push(';');
}

// --- declarations -----------------------------------------------------------

fn emit_struct(out: &mut String, s: &StructDecl) -> Result<(), String> {
    if s.visibility == Visibility::Public {
        out.push_str("pub ");
    }
    out.push_str("struct ");
    out.push_str(&s.name);
    emit_type_params(out, &s.type_params);
    out.push_str(" { ");
    for (i, field) in s.fields.iter().enumerate() {
        if i > 0 {
            out.push_str(", ");
        }
        out.push_str(&field.name);
        out.push_str(": ");
        emit_type(out, &field.ty);
    }
    out.push_str(" }\n");
    Ok(())
}

fn emit_enum(out: &mut String, e: &EnumDecl) -> Result<(), String> {
    if e.visibility == Visibility::Public {
        out.push_str("pub ");
    }
    out.push_str("enum ");
    out.push_str(&e.name);
    out.push_str(" { ");
    for (i, v) in e.variants.iter().enumerate() {
        if i > 0 {
            out.push_str(", ");
        }
        out.push_str(&v.name);
        if !v.payloads.is_empty() {
            out.push('(');
            for (j, ty) in v.payloads.iter().enumerate() {
                if j > 0 {
                    out.push_str(", ");
                }
                emit_type(out, ty);
            }
            out.push(')');
        }
    }
    out.push_str(" }\n");
    Ok(())
}

fn emit_fn(out: &mut String, f: &FnDef) -> Result<(), String> {
    if f.visibility == Visibility::Public {
        out.push_str("pub ");
    }
    out.push_str("fn ");
    out.push_str(&f.name);
    emit_type_params(out, &f.type_params);
    out.push('(');
    for (i, p) in f.params.iter().enumerate() {
        if i > 0 {
            out.push_str(", ");
        }
        emit_param(out, p);
    }
    out.push_str(") -> ");
    emit_type(out, &f.return_type);
    if !f.effect_row.is_empty() {
        out.push_str(" ! { ");
        for (i, eff) in f.effect_row.iter().enumerate() {
            if i > 0 {
                out.push_str(", ");
            }
            out.push_str(&eff.kind);
        }
        out.push_str(" }");
    }
    out.push(' ');
    emit_block(out, &f.body)?;
    out.push('\n');
    Ok(())
}

fn emit_param(out: &mut String, p: &Param) {
    if p.mutable {
        out.push_str("mut ");
    }
    out.push_str(&p.name);
    out.push_str(": ");
    emit_type(out, &p.ty);
}

// --- traits / impls / classes (Bar B / classes) -----------------------------

fn emit_trait(out: &mut String, t: &TraitDecl) -> Result<(), String> {
    if t.visibility == Visibility::Public {
        out.push_str("pub ");
    }
    out.push_str("trait ");
    out.push_str(&t.name);
    out.push_str(" { ");
    for m in &t.methods {
        emit_trait_method_sig(out, m);
        out.push(' ');
    }
    out.push_str("}\n");
    Ok(())
}

/// A trait method signature: `fn name(self: &Self, …) -> R;` — head only, `;`
/// terminated (no body). Effect rows pass through like a free fn's.
fn emit_trait_method_sig(out: &mut String, m: &TraitMethodSig) {
    emit_method_head(out, &m.name, m.self_kind, &m.params, &m.return_type, &m.effect_row);
    out.push(';');
}

fn emit_impl(out: &mut String, i: &ImplDecl) -> Result<(), String> {
    out.push_str("impl ");
    if let Some(name) = &i.name {
        out.push_str(name);
        out.push(' ');
    }
    out.push_str("as ");
    out.push_str(&i.trait_name);
    out.push_str(" for ");
    out.push_str(&i.type_name);
    out.push_str(" { ");
    for m in &i.methods {
        emit_method_head(out, &m.name, m.self_kind, &m.params, &m.return_type, &m.effect_row);
        out.push(' ');
        emit_block(out, &m.body)?;
        out.push(' ');
    }
    out.push_str("}\n");
    Ok(())
}

fn emit_class(out: &mut String, c: &ClassDecl) -> Result<(), String> {
    out.push_str("class ");
    out.push_str(&c.name);
    out.push_str(" { ");
    // Fields, then delegates, then init, then methods. The parser buckets a class
    // body by item kind (not source order), so each KIND's intra-vector order is all
    // that fixes field indices / method indices / the delegate-synthesized ImplIds —
    // and that order round-trips here.
    for f in &c.fields {
        if f.visibility == Visibility::Public {
            out.push_str("pub ");
        }
        out.push_str("let ");
        out.push_str(&f.name);
        out.push_str(": ");
        emit_type(out, &f.ty);
        out.push_str("; ");
    }
    for d in &c.delegates {
        emit_delegate(out, d);
        out.push(' ');
    }
    if let Some(init) = &c.init {
        emit_init(out, init)?;
        out.push(' ');
    }
    for m in &c.methods {
        if m.visibility == Visibility::Public {
            out.push_str("pub ");
        }
        emit_method_head(out, &m.name, m.self_kind, &m.params, &m.return_type, &m.effect_row);
        out.push(' ');
        emit_block(out, &m.body)?;
        out.push(' ');
    }
    out.push_str("}\n");
    Ok(())
}

/// `('pub')? 'init' '(' params ')' '{' body '}'` — no return type (init implicitly
/// returns the constructed instance, ADR 0022 D4). The body's `0` placeholder tail
/// re-emits verbatim (round-trip exact).
fn emit_init(out: &mut String, init: &InitDef) -> Result<(), String> {
    if init.visibility == Visibility::Public {
        out.push_str("pub ");
    }
    out.push_str("init(");
    for (i, p) in init.params.iter().enumerate() {
        if i > 0 {
            out.push_str(", ");
        }
        emit_param(out, p);
    }
    out.push_str(") ");
    emit_block(out, &init.body)
}

fn emit_delegate(out: &mut String, d: &DelegateDecl) {
    if d.visibility == Visibility::Public {
        out.push_str("pub ");
    }
    out.push_str("delegate ");
    out.push_str(&d.field_name);
    out.push_str(": ");
    emit_type(out, &d.ty);
    out.push_str(" to ");
    out.push_str(&d.trait_name);
    out.push(';');
}

/// The shared `fn name(self: &[mut] Self, params) -> R [! { effects }]` head used by
/// trait sigs, impl methods, and class methods. The receiver `self` is implicit in
/// the AST (`self_kind`) + excluded from `params`, so we re-synthesize it first.
fn emit_method_head(
    out: &mut String,
    name: &str,
    self_kind: SelfKind,
    params: &[Param],
    return_type: &TypeExpr,
    effect_row: &[sentinel_ast::Spanned<String>],
) {
    out.push_str("fn ");
    out.push_str(name);
    out.push_str("(self: ");
    out.push_str(self_kind_str(self_kind));
    for p in params {
        out.push_str(", ");
        emit_param(out, p);
    }
    out.push_str(") -> ");
    emit_type(out, return_type);
    if !effect_row.is_empty() {
        out.push_str(" ! { ");
        for (i, eff) in effect_row.iter().enumerate() {
            if i > 0 {
                out.push_str(", ");
            }
            out.push_str(&eff.kind);
        }
        out.push_str(" }");
    }
}

fn self_kind_str(k: SelfKind) -> &'static str {
    match k {
        SelfKind::Shared => "&Self",
        SelfKind::Exclusive => "&mut Self",
    }
}

fn emit_type_params(out: &mut String, params: &[sentinel_ast::TypeParam]) {
    if params.is_empty() {
        return;
    }
    out.push('<');
    for (i, tp) in params.iter().enumerate() {
        if i > 0 {
            out.push_str(", ");
        }
        out.push_str(&tp.name);
    }
    out.push('>');
}

// --- types ------------------------------------------------------------------

fn emit_type(out: &mut String, ty: &TypeExpr) {
    match &ty.kind {
        TypeExprKind::Ident(n) => out.push_str(n),
        TypeExprKind::Nullable(inner) => {
            out.push('?');
            emit_type(out, inner);
        }
        TypeExprKind::Array(inner) => {
            out.push('[');
            emit_type(out, inner);
            out.push(']');
        }
        TypeExprKind::Ref { mutable, inner } => {
            out.push('&');
            if *mutable {
                out.push_str("mut ");
            }
            emit_type(out, inner);
        }
        TypeExprKind::Generic { name, args, .. } => {
            out.push_str(name);
            out.push('<');
            for (i, a) in args.iter().enumerate() {
                if i > 0 {
                    out.push_str(", ");
                }
                emit_type(out, a);
            }
            out.push('>');
        }
        TypeExprKind::Secret(inner) => {
            out.push_str("secret ");
            emit_type(out, inner);
        }
    }
}

// --- statements + blocks ----------------------------------------------------

fn emit_block(out: &mut String, b: &Block) -> Result<(), String> {
    out.push_str("{ ");
    for s in &b.stmts {
        emit_stmt(out, s)?;
        out.push(' ');
    }
    // Every block has a tail expr (a stmt-only block synthesises `0` at
    // parse time — parser.rs:3429 — which we print explicitly, round-trip
    // exact). No trailing `;` on the tail.
    emit_expr(out, &b.tail)?;
    out.push_str(" }");
    Ok(())
}

fn emit_stmt(out: &mut String, s: &Stmt) -> Result<(), String> {
    match &s.kind {
        StmtKind::Let { mutable, name, ty_annot, value, .. } => {
            out.push_str("let ");
            if *mutable {
                out.push_str("mut ");
            }
            out.push_str(name);
            if let Some(ty) = ty_annot {
                out.push_str(": ");
                emit_type(out, ty);
            }
            out.push_str(" = ");
            emit_expr(out, value)?;
            out.push(';');
        }
        StmtKind::Assign { target, value } => {
            emit_expr(out, target)?;
            out.push_str(" = ");
            emit_expr(out, value)?;
            out.push(';');
        }
        StmtKind::While { cond, body } => {
            out.push_str("while ");
            emit_expr(out, cond)?;
            out.push(' ');
            emit_block(out, body)?;
        }
        StmtKind::Break => out.push_str("break;"),
        StmtKind::Continue => out.push_str("continue;"),
        StmtKind::Expr(e) => {
            emit_expr(out, e)?;
            out.push(';');
        }
    }
    Ok(())
}

// --- expressions ------------------------------------------------------------

fn is_atom(k: &ExprKind) -> bool {
    matches!(
        k,
        ExprKind::IntLit(_)
            | ExprKind::BoolLit(_)
            | ExprKind::NullLit
            | ExprKind::CharLit(_)
            | ExprKind::StringLit(_)
            | ExprKind::Var(_)
    )
}

/// Emit `e`, wrapping every compound node in `( … )` so the surface form
/// round-trips to the identical AST regardless of precedence or position
/// (parens leave no AST trace). Atoms emit bare.
fn emit_expr(out: &mut String, e: &Expr) -> Result<(), String> {
    if is_atom(&e.kind) {
        emit_node(out, e)
    } else {
        out.push('(');
        emit_node(out, e)?;
        out.push(')');
        Ok(())
    }
}

fn emit_node(out: &mut String, e: &Expr) -> Result<(), String> {
    match &e.kind {
        ExprKind::IntLit(n) => out.push_str(&n.to_string()),
        ExprKind::BoolLit(b) => out.push_str(if *b { "true" } else { "false" }),
        ExprKind::NullLit => out.push_str("null"),
        ExprKind::CharLit(b) => emit_char_lit(out, *b),
        ExprKind::StringLit(bytes) => emit_string_lit(out, bytes),
        ExprKind::Var(name) => out.push_str(name),
        ExprKind::Unary(UnaryOp::Sqrt, _) => {
            // ADR 0058: `sqrt` operates on `f64`, which is out of the selfhost
            // Bar-A scope (like `cast` / a float literal) — error cleanly.
            return Err("merge-to-source: `sqrt` is out of Bar-A scope".to_string());
        }
        ExprKind::Unary(UnaryOp::PtrOf | UnaryOp::PtrOfMut | UnaryOp::IsNull, _) => {
            // ADR 0057 Phase 1b: `ptr_of` / `ptr_of_mut` / `is_null` are FFI
            // call-form intrinsics, out of the selfhost Bar-A scope (like `sqrt`).
            return Err("merge-to-source: `ptr_of` / `is_null` is out of Bar-A scope".to_string());
        }
        ExprKind::Unary(op, inner) => {
            out.push_str(unary_symbol(*op));
            out.push(' ');
            emit_expr(out, inner)?;
        }
        ExprKind::Binary(op, l, r) => emit_binary(out, bin_symbol(*op), l, r)?,
        ExprKind::Cmp(op, l, r) => emit_binary(out, cmp_symbol(*op), l, r)?,
        ExprKind::Logic(op, l, r) => emit_binary(out, logic_symbol(*op), l, r)?,
        ExprKind::Block(b) => emit_block(out, b)?,
        ExprKind::If { cond, then_branch, else_branch } => {
            out.push_str("if ");
            emit_expr(out, cond)?;
            out.push(' ');
            emit_block(out, then_branch)?;
            out.push_str(" else ");
            emit_block(out, else_branch)?;
        }
        ExprKind::Call { callee, args, .. } => {
            out.push_str(callee);
            emit_args(out, args)?;
        }
        ExprKind::StructLit { name, fields, .. } => {
            out.push_str(name);
            out.push_str(" { ");
            for (i, field) in fields.iter().enumerate() {
                if i > 0 {
                    out.push_str(", ");
                }
                out.push_str(&field.name);
                out.push_str(": ");
                emit_expr(out, &field.value)?;
            }
            out.push_str(" }");
        }
        ExprKind::FieldAccess { target, field, .. } => {
            emit_expr(out, target)?;
            out.push('.');
            out.push_str(field);
        }
        ExprKind::ArrayLit(elems) => {
            out.push('[');
            for (i, el) in elems.iter().enumerate() {
                if i > 0 {
                    out.push_str(", ");
                }
                emit_expr(out, el)?;
            }
            out.push(']');
        }
        ExprKind::Index { target, index } => {
            emit_expr(out, target)?;
            out.push('[');
            emit_expr(out, index)?;
            out.push(']');
        }
        ExprKind::MethodCall { target, method, args, .. } => {
            emit_expr(out, target)?;
            out.push('.');
            out.push_str(method);
            emit_args(out, args)?;
        }
        ExprKind::QualifiedCall { impl_name, method, args, .. } => {
            // The parser's uniform `Name::tail` form — enum construction
            // (`Enum::Variant` / `Enum::Variant(payload)`), or a named-impl
            // call (Bar B). Empty args + no parens round-trips to args=[]
            // (the bare unit-variant form).
            out.push_str(impl_name);
            out.push_str("::");
            out.push_str(method);
            if !args.is_empty() {
                emit_args(out, args)?;
            }
        }
        ExprKind::ClassInit { class_name, args, .. } => {
            out.push_str(class_name);
            out.push_str("::init");
            emit_args(out, args)?;
        }
        ExprKind::Match { scrutinee, arms } => {
            out.push_str("match ");
            emit_expr(out, scrutinee)?;
            out.push_str(" { ");
            for (i, arm) in arms.iter().enumerate() {
                if i > 0 {
                    out.push_str(", ");
                }
                emit_pattern(out, &arm.pattern);
                out.push_str(" => ");
                emit_expr(out, &arm.body)?;
            }
            out.push_str(" }");
        }
        // Bar B / effects (ADR 0020): `perform Eff.op(args)` + `handle body with { … }`.
        ExprKind::Perform { effect, op, args } => {
            out.push_str("perform ");
            out.push_str(&effect.kind);
            out.push('.');
            out.push_str(&op.kind);
            emit_args(out, args)?;
        }
        ExprKind::Handle { body, arms, return_arm } => {
            out.push_str("handle ");
            emit_expr(out, body)?;
            out.push_str(" with { ");
            let mut first = true;
            for arm in arms {
                if !first {
                    out.push_str(", ");
                }
                first = false;
                emit_handler_arm(out, arm)?;
            }
            if let Some(ra) = return_arm {
                if !first {
                    out.push_str(", ");
                }
                out.push_str("return ");
                out.push_str(&ra.value_name.kind);
                out.push_str(" => ");
                emit_expr(out, &ra.body)?;
            }
            out.push_str(" }");
        }
        // ADR 0058: a float literal is out of the selfhost Bar-A scope (no
        // selfhost source uses `f64`), like `cast` / `declassify` — the
        // merge-to-source path errors cleanly rather than rendering it.
        ExprKind::FloatLit(_)
        | ExprKind::Declassify(_)
        | ExprKind::Cast(_, _)
        | ExprKind::Scope { .. }
        | ExprKind::Spawn { .. }
        | ExprKind::Await { .. } => {
            return Err(format!(
                "merge-to-source: expression `{}` is out of Bar-A scope",
                expr_kind_name(&e.kind)
            ));
        }
    }
    Ok(())
}

/// `Eff.op(p1, p2, …) => body` — the params are the op-params followed by the
/// continuation binding (all plain idents; the typing layer splits them).
fn emit_handler_arm(out: &mut String, arm: &HandlerArm) -> Result<(), String> {
    out.push_str(&arm.effect.kind);
    out.push('.');
    out.push_str(&arm.op.kind);
    out.push('(');
    for (i, p) in arm.param_names.iter().enumerate() {
        if i > 0 {
            out.push_str(", ");
        }
        out.push_str(&p.kind);
    }
    out.push(')');
    out.push_str(" => ");
    emit_expr(out, &arm.body)
}

fn emit_binary(out: &mut String, sym: &str, l: &Expr, r: &Expr) -> Result<(), String> {
    emit_expr(out, l)?;
    out.push(' ');
    out.push_str(sym);
    out.push(' ');
    emit_expr(out, r)?;
    Ok(())
}

fn emit_args(out: &mut String, args: &[Expr]) -> Result<(), String> {
    out.push('(');
    for (i, a) in args.iter().enumerate() {
        if i > 0 {
            out.push_str(", ");
        }
        emit_expr(out, a)?;
    }
    out.push(')');
    Ok(())
}

fn emit_pattern(out: &mut String, p: &Pattern) {
    match p {
        Pattern::Wildcard(_) => out.push('_'),
        Pattern::Variant { enum_name, variant, bindings, .. } => {
            out.push_str(enum_name);
            out.push_str("::");
            out.push_str(variant);
            if !bindings.is_empty() {
                out.push('(');
                for (i, b) in bindings.iter().enumerate() {
                    if i > 0 {
                        out.push_str(", ");
                    }
                    out.push_str(&b.kind);
                }
                out.push(')');
            }
        }
    }
}

// --- literals ---------------------------------------------------------------

fn emit_string_lit(out: &mut String, bytes: &[u8]) {
    out.push('"');
    for &b in bytes {
        push_byte_escaped(out, b, b'"');
    }
    out.push('"');
}

fn emit_char_lit(out: &mut String, b: u8) {
    out.push('\'');
    push_byte_escaped(out, b, b'\'');
    out.push('\'');
}

/// Re-encode one decoded byte into a literal body the parser decodes back
/// to the same byte: `\\` / the delimiter escaped, a bare printable ASCII
/// byte verbatim, anything else as `\xHH` (ADR 0033 D2). Never emits a raw
/// newline or an unescaped delimiter (the lexer's string/char regex
/// forbids both).
fn push_byte_escaped(out: &mut String, b: u8, delim: u8) {
    if b == b'\\' {
        out.push_str("\\\\");
    } else if b == delim {
        out.push('\\');
        out.push(delim as char);
    } else if (0x20..=0x7E).contains(&b) {
        out.push(b as char);
    } else {
        out.push_str("\\x");
        const HEX: &[u8; 16] = b"0123456789ABCDEF";
        out.push(HEX[(b >> 4) as usize] as char);
        out.push(HEX[(b & 0xF) as usize] as char);
    }
}

// --- operator symbols (match the lexer's tokens exactly) --------------------

fn unary_symbol(op: UnaryOp) -> &'static str {
    match op {
        UnaryOp::Neg => "-",
        UnaryOp::Not => "!",
        UnaryOp::Ref => "&",
        UnaryOp::RefMut => "&mut",
        UnaryOp::Deref => "*",
        // ADR 0058: `sqrt` is call-form, not a prefix operator — the emitter
        // errors before reaching this (out of Bar-A scope); kept exhaustive.
        UnaryOp::Sqrt => "sqrt",
        // ADR 0057 Phase 1b: `ptr_of` / `ptr_of_mut` are call-form FFI
        // intrinsics — the emitter errors before reaching this; kept exhaustive.
        UnaryOp::PtrOf => "ptr_of",
        UnaryOp::PtrOfMut => "ptr_of_mut",
        UnaryOp::IsNull => "is_null",
    }
}

fn bin_symbol(op: BinOp) -> &'static str {
    match op {
        BinOp::Add => "+",
        BinOp::Sub => "-",
        BinOp::Mul => "*",
        BinOp::Div => "/",
        BinOp::Shl => "<<",
        BinOp::Shr => ">>",
        BinOp::BitAnd => "&",
        BinOp::BitOr => "|",
        BinOp::BitXor => "^",
    }
}

fn cmp_symbol(op: CmpOp) -> &'static str {
    match op {
        CmpOp::Eq => "==",
        CmpOp::Ne => "!=",
        CmpOp::Lt => "<",
        CmpOp::Le => "<=",
        CmpOp::Gt => ">",
        CmpOp::Ge => ">=",
    }
}

fn logic_symbol(op: LogicOp) -> &'static str {
    match op {
        LogicOp::And => "&&",
        LogicOp::Or => "||",
    }
}

fn expr_kind_name(k: &ExprKind) -> &'static str {
    match k {
        ExprKind::FloatLit(_) => "float literal",
        ExprKind::Declassify(_) => "declassify",
        ExprKind::Cast(_, _) => "cast",
        ExprKind::Perform { .. } => "perform",
        ExprKind::Handle { .. } => "handle",
        ExprKind::Scope { .. } => "scope",
        ExprKind::Spawn { .. } => "spawn",
        ExprKind::Await { .. } => "await",
        _ => "expression",
    }
}
