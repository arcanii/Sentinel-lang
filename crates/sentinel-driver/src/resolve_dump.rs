//! Phase D self-host port (3/N) / ADR 0040 D2: the resolve differential
//! oracle — a complete, regular S-expression dump of the `ResolvedProgram`
//! that the Sentinel-written resolve stage (`selfhost/resolve.sentinel`)
//! reproduces byte-for-byte. It is the `snc ast` form (see `ast_dump.rs`)
//! **extended with the resolved IDs** and the parser's syntactic ambiguity
//! resolved: a `Var` carries its `VarId` (`(var #N)`), a `Call` its `FnId`
//! (`(call #N …)`), a `StructLit` its `StructId`, and the parser's uniform
//! `qcall` / `class-init` is split into `(call …)` / `(resume-kont …)` /
//! `(enum-construct …)` / `(class-init …)` / `(qcall-impl …)`. A dev/validation
//! surface, NOT `abi-v1` — freely amendable; pinned by a golden test. The
//! integer IDs are the Rust assignment order (builtins 0–13, user fns 14+,
//! other decls in source order); the Sentinel side reproduces that exactly.
//!
//! Decls dump in source order (the `ResolvedProgram` is kind-bucketed, so
//! `dump` re-sorts by span start — the order the Sentinel stage emits as it
//! scans), matching `ast_dump`.

use crate::ast_dump::dump_type;
use sentinel_ast::SelfKind;
use sentinel_resolve::{ImplTarget, ResolvedBlock, ResolvedClassDecl, ResolvedEffectDecl,
    ResolvedEnumDecl, ResolvedExpr, ResolvedExprKind, ResolvedFnDef, ResolvedImplDecl,
    ResolvedParam, ResolvedPattern, ResolvedProgram, ResolvedStmt, ResolvedStmtKind,
    ResolvedStructDecl, ResolvedTraitDecl};

/// One top-level resolved declaration, tagged for the source-order
/// re-collation in [`dump`].
enum Item<'a> {
    Fn(&'a ResolvedFnDef),
    Struct(&'a ResolvedStructDecl),
    Enum(&'a ResolvedEnumDecl),
    Effect(&'a ResolvedEffectDecl),
    Trait(&'a ResolvedTraitDecl),
    Impl(&'a ResolvedImplDecl),
    Class(&'a ResolvedClassDecl),
}

/// Canonical S-expression dump of `program` — every resolved decl in source
/// order (newline-separated).
pub fn dump(program: &ResolvedProgram) -> String {
    let mut items: Vec<(usize, Item)> = Vec::new();
    for f in &program.fns {
        items.push((f.span.start, Item::Fn(f)));
    }
    for s in &program.structs {
        items.push((s.span.start, Item::Struct(s)));
    }
    for e in &program.enums {
        items.push((e.span.start, Item::Enum(e)));
    }
    // The built-in `Async` effect is auto-registered by resolve with a synthetic
    // `0..0` span (id = user-effect count; ADR 0024 D5), so it has no source
    // position to sort by — emit it **deterministically last** instead, separate
    // from the source-order user decls. The Sentinel stage appends it likewise
    // (it never appears in the source). A user effect literally named `Async`
    // is rejected by resolve, so the name uniquely tags the built-in.
    let mut async_effect: Option<&ResolvedEffectDecl> = None;
    for ef in &program.effects {
        if ef.name == "Async" {
            async_effect = Some(ef);
        } else {
            items.push((ef.span.start, Item::Effect(ef)));
        }
    }
    for tr in &program.traits {
        items.push((tr.span.start, Item::Trait(tr)));
    }
    for im in &program.impls {
        items.push((im.span.start, Item::Impl(im)));
    }
    for c in &program.classes {
        items.push((c.span.start, Item::Class(c)));
    }
    items.sort_by_key(|(start, _)| *start);

    let mut out = String::new();
    let mut first = true;
    for (_, item) in items {
        if !first {
            out.push('\n');
        }
        first = false;
        match item {
            Item::Fn(f) => dump_fn(f, &mut out),
            Item::Struct(s) => dump_struct(s, &mut out),
            Item::Enum(e) => dump_enum(e, &mut out),
            Item::Effect(ef) => dump_effect(ef, &mut out),
            Item::Trait(tr) => dump_trait(tr, &mut out),
            Item::Impl(im) => dump_impl(im, &mut out),
            Item::Class(c) => dump_class(c, &mut out),
        }
    }
    if let Some(ef) = async_effect {
        if !first {
            out.push('\n');
        }
        dump_effect(ef, &mut out);
    }
    out.push('\n');
    out
}

/// Push `#<n>`.
fn push_id(out: &mut String, n: u32) {
    out.push('#');
    out.push_str(&n.to_string());
}

fn self_kind_word(k: SelfKind) -> &'static str {
    match k {
        SelfKind::Shared => "shared",
        SelfKind::Exclusive => "exclusive",
    }
}

fn dump_fn(f: &ResolvedFnDef, out: &mut String) {
    out.push_str("(fn ");
    push_id(out, f.id.0);
    out.push(' ');
    out.push_str(&f.name);
    out.push_str(" (");
    for (i, p) in f.params.iter().enumerate() {
        if i > 0 {
            out.push(' ');
        }
        dump_param(p, out);
    }
    out.push_str(") ");
    dump_type(&f.return_type, out);
    out.push(' ');
    dump_block(&f.body, out);
    out.push(')');
}

/// A body-bound parameter: `(param #<varid> [mut] <name> <type>)`.
fn dump_param(p: &ResolvedParam, out: &mut String) {
    out.push_str("(param ");
    push_id(out, p.id.0);
    out.push(' ');
    if p.mutable {
        out.push_str("mut ");
    }
    out.push_str(&p.name);
    out.push(' ');
    dump_type(&p.ty, out);
    out.push(')');
}

fn dump_block(b: &ResolvedBlock, out: &mut String) {
    out.push_str("(block");
    for s in &b.stmts {
        out.push(' ');
        dump_stmt(s, out);
    }
    out.push(' ');
    dump_expr(&b.tail, out);
    out.push(')');
}

fn dump_stmt(s: &ResolvedStmt, out: &mut String) {
    match &s.kind {
        ResolvedStmtKind::Let { id, mutable, ty_annot, value, .. } => {
            out.push_str("(let ");
            push_id(out, id.0);
            out.push(' ');
            if *mutable {
                out.push_str("mut ");
            }
            match ty_annot {
                Some(t) => dump_type(t, out),
                None => out.push('_'),
            }
            out.push(' ');
            dump_expr(value, out);
            out.push(')');
        }
        ResolvedStmtKind::Assign { target, value } => {
            out.push_str("(assign ");
            dump_expr(target, out);
            out.push(' ');
            dump_expr(value, out);
            out.push(')');
        }
        ResolvedStmtKind::While { cond, body } => {
            out.push_str("(while ");
            dump_expr(cond, out);
            out.push(' ');
            dump_block(body, out);
            out.push(')');
        }
        ResolvedStmtKind::Break => out.push_str("(break)"),
        ResolvedStmtKind::Continue => out.push_str("(continue)"),
        ResolvedStmtKind::Expr(e) => {
            out.push_str("(expr ");
            dump_expr(e, out);
            out.push(')');
        }
    }
}

fn dump_args(args: &[ResolvedExpr], out: &mut String) {
    for a in args {
        out.push(' ');
        dump_expr(a, out);
    }
}

fn dump_expr(e: &ResolvedExpr, out: &mut String) {
    match &e.kind {
        ResolvedExprKind::IntLit(n) => {
            out.push_str("(int ");
            out.push_str(&n.to_string());
            out.push(')');
        }
        // ADR 0058: a float literal renders by value (`{:?}` keeps the dot).
        ResolvedExprKind::FloatLit(bits) => {
            out.push_str("(float ");
            out.push_str(&format!("{:?}", f64::from_bits(*bits)));
            out.push(')');
        }
        ResolvedExprKind::BoolLit(b) => {
            out.push_str(if *b { "(bool true)" } else { "(bool false)" });
        }
        ResolvedExprKind::NullLit => out.push_str("(null)"),
        ResolvedExprKind::CharLit(byte) => {
            out.push_str("(char ");
            out.push_str(&byte.to_string());
            out.push(')');
        }
        ResolvedExprKind::StringLit(bytes) => {
            out.push_str("(str");
            for b in bytes {
                out.push(' ');
                out.push_str(&b.to_string());
            }
            out.push(')');
        }
        ResolvedExprKind::Var(id) => {
            out.push_str("(var ");
            push_id(out, id.0);
            out.push(')');
        }
        ResolvedExprKind::Unary(op, inner) => {
            out.push_str("(unary ");
            out.push_str(op.symbol());
            out.push(' ');
            dump_expr(inner, out);
            out.push(')');
        }
        ResolvedExprKind::Binary(op, l, r) => {
            out.push_str("(binop ");
            out.push_str(op.symbol());
            out.push(' ');
            dump_expr(l, out);
            out.push(' ');
            dump_expr(r, out);
            out.push(')');
        }
        ResolvedExprKind::Cmp(op, l, r) => {
            out.push_str("(cmp ");
            out.push_str(op.symbol());
            out.push(' ');
            dump_expr(l, out);
            out.push(' ');
            dump_expr(r, out);
            out.push(')');
        }
        ResolvedExprKind::Logic(op, l, r) => {
            out.push_str("(logic ");
            out.push_str(op.symbol());
            out.push(' ');
            dump_expr(l, out);
            out.push(' ');
            dump_expr(r, out);
            out.push(')');
        }
        ResolvedExprKind::Block(b) => dump_block(b, out),
        ResolvedExprKind::If { cond, then_branch, else_branch } => {
            out.push_str("(if ");
            dump_expr(cond, out);
            out.push(' ');
            dump_block(then_branch, out);
            out.push(' ');
            dump_block(else_branch, out);
            out.push(')');
        }
        ResolvedExprKind::Call { id, args, .. } => {
            out.push_str("(call ");
            push_id(out, id.0);
            dump_args(args, out);
            out.push(')');
        }
        ResolvedExprKind::ResumeKont { kont, args, .. } => {
            out.push_str("(resume-kont ");
            push_id(out, kont.0);
            dump_args(args, out);
            out.push(')');
        }
        ResolvedExprKind::StructLit { id, name, fields, .. } => {
            out.push_str("(struct-lit ");
            push_id(out, id.0);
            out.push(' ');
            out.push_str(name);
            for f in fields {
                out.push_str(" (field ");
                out.push_str(&f.name);
                out.push(' ');
                dump_expr(&f.value, out);
                out.push(')');
            }
            out.push(')');
        }
        ResolvedExprKind::FieldAccess { target, field, .. } => {
            out.push_str("(field ");
            dump_expr(target, out);
            out.push(' ');
            out.push_str(field);
            out.push(')');
        }
        ResolvedExprKind::ArrayLit(elems) => {
            out.push_str("(array");
            dump_args(elems, out);
            out.push(')');
        }
        ResolvedExprKind::Index { target, index } => {
            out.push_str("(index ");
            dump_expr(target, out);
            out.push(' ');
            dump_expr(index, out);
            out.push(')');
        }
        ResolvedExprKind::Declassify(inner) => {
            out.push_str("(declassify ");
            dump_expr(inner, out);
            out.push(')');
        }
        ResolvedExprKind::Cast(inner, te) => {
            out.push_str("(cast ");
            dump_expr(inner, out);
            out.push(' ');
            out.push_str(&te.kind.to_string());
            out.push(')');
        }
        ResolvedExprKind::Perform { effect_id, op_index, effect_name, op_name, args, .. } => {
            out.push_str("(perform ");
            push_id(out, effect_id.0);
            out.push(' ');
            out.push_str(&op_index.to_string());
            out.push(' ');
            out.push_str(effect_name);
            out.push(' ');
            out.push_str(op_name);
            dump_args(args, out);
            out.push(')');
        }
        ResolvedExprKind::Handle { body, arms, return_arm } => {
            out.push_str("(handle ");
            dump_expr(body, out);
            for arm in arms {
                out.push_str(" (arm ");
                push_id(out, arm.effect_id.0);
                out.push(' ');
                out.push_str(&arm.op_index.to_string());
                out.push(' ');
                out.push_str(&arm.effect_name);
                out.push(' ');
                out.push_str(&arm.op_name);
                out.push_str(" (");
                for (i, v) in arm.param_var_ids.iter().enumerate() {
                    if i > 0 {
                        out.push(' ');
                    }
                    push_id(out, v.0);
                }
                out.push_str(") ");
                dump_expr(&arm.body, out);
                out.push(')');
            }
            if let Some(ra) = return_arm {
                out.push_str(" (return ");
                push_id(out, ra.value_var_id.0);
                out.push(' ');
                dump_expr(&ra.body, out);
                out.push(')');
            }
            out.push(')');
        }
        ResolvedExprKind::MethodCall { target, method, args, .. } => {
            out.push_str("(method ");
            dump_expr(target, out);
            out.push(' ');
            out.push_str(method);
            dump_args(args, out);
            out.push(')');
        }
        ResolvedExprKind::ClassInit { id, name, args, .. } => {
            out.push_str("(class-init ");
            push_id(out, id.0);
            out.push(' ');
            out.push_str(name);
            dump_args(args, out);
            out.push(')');
        }
        ResolvedExprKind::QualifiedCall { impl_id, method_index, impl_name, method, args, .. } => {
            out.push_str("(qcall-impl ");
            push_id(out, impl_id.0);
            out.push(' ');
            out.push_str(&method_index.to_string());
            out.push(' ');
            out.push_str(impl_name);
            out.push(' ');
            out.push_str(method);
            dump_args(args, out);
            out.push(')');
        }
        ResolvedExprKind::EnumConstruct { enum_id, enum_name, variant_name, args, .. } => {
            out.push_str("(enum-construct ");
            push_id(out, enum_id.0);
            out.push(' ');
            out.push_str(enum_name);
            out.push(' ');
            out.push_str(variant_name);
            dump_args(args, out);
            out.push(')');
        }
        ResolvedExprKind::Scope { body, .. } => {
            out.push_str("(scope ");
            dump_block(body, out);
            out.push(')');
        }
        ResolvedExprKind::Spawn { call_expr } => {
            out.push_str("(spawn ");
            dump_expr(call_expr, out);
            out.push(')');
        }
        ResolvedExprKind::Await { task_expr } => {
            out.push_str("(await ");
            dump_expr(task_expr, out);
            out.push(')');
        }
        ResolvedExprKind::Match { scrutinee, arms } => {
            out.push_str("(match ");
            dump_expr(scrutinee, out);
            for arm in arms {
                out.push_str(" (arm ");
                dump_pattern(&arm.pattern, out);
                out.push(' ');
                dump_expr(&arm.body, out);
                out.push(')');
            }
            out.push(')');
        }
    }
}

fn dump_pattern(p: &ResolvedPattern, out: &mut String) {
    match p {
        ResolvedPattern::Variant { enum_name, variant_name, bindings, .. } => {
            out.push_str("(pat ");
            out.push_str(enum_name);
            out.push(' ');
            out.push_str(variant_name);
            for b in bindings {
                out.push(' ');
                push_id(out, b.var_id.0);
            }
            out.push(')');
        }
        ResolvedPattern::Wildcard(_) => out.push_str("(pat _)"),
    }
}

fn dump_struct(s: &ResolvedStructDecl, out: &mut String) {
    out.push_str("(struct ");
    push_id(out, s.id.0);
    out.push(' ');
    out.push_str(&s.name);
    for f in &s.fields {
        out.push_str(" (field ");
        out.push_str(&f.name);
        out.push(' ');
        dump_type(&f.ty, out);
        out.push(')');
    }
    out.push(')');
}

fn dump_enum(e: &ResolvedEnumDecl, out: &mut String) {
    out.push_str("(enum ");
    push_id(out, e.id.0);
    out.push(' ');
    out.push_str(&e.name);
    for v in &e.variants {
        out.push_str(" (variant ");
        out.push_str(&v.name);
        for p in &v.payloads {
            out.push(' ');
            dump_type(p, out);
        }
        out.push(')');
    }
    out.push(')');
}

fn dump_effect(e: &ResolvedEffectDecl, out: &mut String) {
    out.push_str("(effect ");
    push_id(out, e.id.0);
    out.push(' ');
    out.push_str(&e.name);
    for op in &e.ops {
        out.push_str(" (op ");
        out.push_str(&op.name);
        out.push_str(" (");
        // Op params are not body-bound; dump without VarIds (like trait params).
        for (i, p) in op.params.iter().enumerate() {
            if i > 0 {
                out.push(' ');
            }
            out.push_str("(param ");
            if p.mutable {
                out.push_str("mut ");
            }
            out.push_str(&p.name);
            out.push(' ');
            dump_type(&p.ty, out);
            out.push(')');
        }
        out.push_str(") ");
        match &op.return_type {
            Some(t) => dump_type(t, out),
            None => out.push('_'),
        }
        out.push(')');
    }
    out.push(')');
}

fn dump_trait(t: &ResolvedTraitDecl, out: &mut String) {
    out.push_str("(trait ");
    push_id(out, t.id.0);
    out.push(' ');
    out.push_str(&t.name);
    for m in &t.methods {
        out.push_str(" (method ");
        out.push_str(&m.name);
        out.push(' ');
        out.push_str(self_kind_word(m.self_kind));
        out.push_str(" (");
        for (i, p) in m.params.iter().enumerate() {
            if i > 0 {
                out.push(' ');
            }
            out.push_str("(param ");
            if p.mutable {
                out.push_str("mut ");
            }
            out.push_str(&p.name);
            out.push(' ');
            dump_type(&p.ty, out);
            out.push(')');
        }
        out.push_str(") ");
        dump_type(&m.return_type, out);
        out.push(')');
    }
    out.push(')');
}

fn dump_impl(i: &ResolvedImplDecl, out: &mut String) {
    out.push_str("(impl ");
    push_id(out, i.id.0);
    out.push(' ');
    match &i.name {
        Some(n) => out.push_str(n),
        None => out.push('_'),
    }
    out.push(' ');
    push_id(out, i.trait_id.0);
    out.push(' ');
    out.push_str(&i.trait_name);
    out.push(' ');
    match i.target {
        ImplTarget::Class(id) => {
            out.push_str("class");
            push_id(out, id.0);
        }
        ImplTarget::Struct(id) => {
            out.push_str("struct");
            push_id(out, id.0);
        }
    }
    out.push(' ');
    out.push_str(&i.type_name);
    for m in &i.methods {
        out.push_str(" (method ");
        push_id(out, m.self_var_id.0);
        out.push(' ');
        out.push_str(&m.name);
        out.push(' ');
        out.push_str(self_kind_word(m.self_kind));
        out.push_str(" (");
        for (j, p) in m.params.iter().enumerate() {
            if j > 0 {
                out.push(' ');
            }
            dump_param(p, out);
        }
        out.push_str(") ");
        dump_type(&m.return_type, out);
        out.push(' ');
        dump_block(&m.body, out);
        out.push(')');
    }
    out.push(')');
}

fn dump_class(c: &ResolvedClassDecl, out: &mut String) {
    out.push_str("(class ");
    push_id(out, c.id.0);
    out.push(' ');
    out.push_str(&c.name);
    for f in &c.fields {
        out.push_str(" (field ");
        out.push_str(&f.name);
        out.push(' ');
        dump_type(&f.ty, out);
        out.push(')');
    }
    if let Some(init) = &c.init {
        out.push_str(" (init ");
        push_id(out, init.self_var_id.0);
        out.push_str(" (");
        for (j, p) in init.params.iter().enumerate() {
            if j > 0 {
                out.push(' ');
            }
            dump_param(p, out);
        }
        out.push_str(") ");
        dump_block(&init.body, out);
        out.push(')');
    }
    for m in &c.methods {
        out.push_str(" (method ");
        push_id(out, m.self_var_id.0);
        out.push(' ');
        out.push_str(&m.name);
        out.push(' ');
        out.push_str(self_kind_word(m.self_kind));
        out.push_str(" (");
        for (j, p) in m.params.iter().enumerate() {
            if j > 0 {
                out.push(' ');
            }
            dump_param(p, out);
        }
        out.push_str(") ");
        dump_type(&m.return_type, out);
        out.push(' ');
        dump_block(&m.body, out);
        out.push(')');
    }
    out.push(')');
}
