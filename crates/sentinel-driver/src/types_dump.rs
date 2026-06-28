//! Phase D self-host port (4/N) / ADR 0041 D2: the types differential
//! oracle — a complete, regular S-expression dump of the `TypedProgram`
//! that the Sentinel-written types stage (`selfhost/types.sentinel`) will
//! reproduce byte-for-byte. It is the `snc resolve` form (see
//! `resolve_dump.rs`) **extended with each expression node's inferred
//! `Type`** (a trailing ` :<type>` rendered by `sentinel_types::type_display`)
//! **+ the type-resolved disambiguations**: the parser/resolver's uniform
//! `(method …)` is split by receiver type into `(method …)` (a class's own
//! method) / `(impl-method …)` (a default-impl method); the synthesized
//! implicit coercions appear as `(widen-null …)` / `(widen-secret …)`; and
//! computed indices (`field_index`, `variant_index`, generic `type_args`)
//! are shown. A dev/validation surface, NOT `abi-v1` — freely amendable,
//! pinned by a golden test.
//!
//! Unlike the resolve VarIds, the structural type rendering carries **no
//! interner-ID order obligation** — `type_display` renders a `Type`
//! structurally (`&mut i64`, `secret [u8]`, `Vec<i64>`, `Point<i64>`) and
//! never shows a `RefId`/`SecretId`/`GenericInstanceId`, so the Sentinel
//! side need only reproduce the structure (ADR 0041 D-context fact 3).
//!
//! Decls dump in source order (the same span-sort as `resolve_dump`); the
//! built-in `Async` effect (synthetic `0..0` span) dumps deterministically
//! last.

use sentinel_ast::SelfKind;
use sentinel_resolve::ImplTarget;
use sentinel_types::{
    type_display, ClassData, EnumData, ImplData, TraitData, Type, TypedBlock, TypedEffectDecl,
    TypedExpr, TypedExprKind, TypedFnDef, TypedParam, TypedPattern, TypedProgram, TypedStmt,
    TypedStmtKind, TypedStructDecl,
};

/// One top-level typed declaration, tagged for the source-order
/// re-collation in [`dump`].
enum Item<'a> {
    Fn(&'a TypedFnDef),
    Struct(&'a TypedStructDecl),
    Enum(&'a EnumData),
    Effect(&'a TypedEffectDecl),
    Trait(&'a TraitData),
    Impl(&'a ImplData),
    Class(&'a ClassData),
}

/// Canonical S-expression dump of `program` — every typed decl in source
/// order (newline-separated).
pub fn dump(program: &TypedProgram) -> String {
    let mut items: Vec<(usize, Item)> = Vec::new();
    for f in &program.fns {
        items.push((f.span.start, Item::Fn(f)));
    }
    for s in &program.structs {
        items.push((s.span.start, Item::Struct(s)));
    }
    // EnumData carries no full `span` (only `name_span`); sort by the name
    // position. Every enum's name sits a constant `len("enum ")` after its
    // keyword, and decls are newline-separated, so this preserves the
    // keyword source order the Sentinel stage emits (ADR 0041 D2).
    for e in &program.enums {
        items.push((e.name_span.start, Item::Enum(e)));
    }
    // The built-in `Async` effect (synthetic `0..0` span, id = user-effect
    // count) has no source position — emit it deterministically last, as
    // `resolve_dump` does. A user effect named `Async` is rejected upstream.
    let mut async_effect: Option<&TypedEffectDecl> = None;
    for ef in &program.effect_decls {
        if ef.name == "Async" {
            async_effect = Some(ef);
        } else {
            items.push((ef.span.start, Item::Effect(ef)));
        }
    }
    for tr in &program.trait_decls {
        items.push((tr.span.start, Item::Trait(tr)));
    }
    for im in &program.impl_decls {
        items.push((im.span.start, Item::Impl(im)));
    }
    for c in &program.class_decls {
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
            Item::Fn(f) => dump_fn(f, program, &mut out),
            Item::Struct(s) => dump_struct(s, program, &mut out),
            Item::Enum(e) => dump_enum(e, program, &mut out),
            Item::Effect(ef) => dump_effect(ef, program, &mut out),
            Item::Trait(tr) => dump_trait(tr, program, &mut out),
            Item::Impl(im) => dump_impl(im, program, &mut out),
            Item::Class(c) => dump_class(c, program, &mut out),
        }
    }
    if let Some(ef) = async_effect {
        if !first {
            out.push('\n');
        }
        dump_effect(ef, program, &mut out);
    }
    out.push('\n');
    out
}

/// Push `#<n>`.
fn push_id(out: &mut String, n: u32) {
    out.push('#');
    out.push_str(&n.to_string());
}

/// Push a `Type` rendered structurally (the same form `type_display` prints).
fn push_ty(out: &mut String, ty: Type, program: &TypedProgram) {
    out.push_str(&type_display(ty, Some(program)));
}

/// Close a typed expression node: ` :<type>)`. Every expression node ends
/// with its inferred type (the whole point of the types dump — pinning that
/// the Sentinel side synthesized the identical type at every node).
fn close_ty(out: &mut String, ty: Type, program: &TypedProgram) {
    out.push_str(" :");
    push_ty(out, ty, program);
    out.push(')');
}

fn self_kind_word(k: SelfKind) -> &'static str {
    match k {
        SelfKind::Shared => "shared",
        SelfKind::Exclusive => "exclusive",
    }
}

fn dump_fn(f: &TypedFnDef, program: &TypedProgram, out: &mut String) {
    out.push_str("(fn ");
    push_id(out, f.id.0);
    out.push(' ');
    out.push_str(&f.name);
    out.push_str(" (");
    for (i, p) in f.params.iter().enumerate() {
        if i > 0 {
            out.push(' ');
        }
        dump_param(p, program, out);
    }
    out.push_str(") ");
    push_ty(out, f.return_type, program);
    out.push(' ');
    dump_block(&f.body, program, out);
    out.push(')');
}

/// A body-bound parameter: `(param #<varid> [mut] <name> <type>)`. The type
/// is the resolved `Type` (structural), not the surface annotation.
fn dump_param(p: &TypedParam, program: &TypedProgram, out: &mut String) {
    out.push_str("(param ");
    push_id(out, p.id.0);
    out.push(' ');
    if p.mutable {
        out.push_str("mut ");
    }
    out.push_str(&p.name);
    out.push(' ');
    push_ty(out, p.ty, program);
    out.push(')');
}

/// A block: `(block <stmt>… <tail> :<type>)`. The block self-annotates with
/// its `ty` (= the tail's type), so the `Block` expr arm adds no extra wrap.
fn dump_block(b: &TypedBlock, program: &TypedProgram, out: &mut String) {
    out.push_str("(block");
    for s in &b.stmts {
        out.push(' ');
        dump_stmt(s, program, out);
    }
    out.push(' ');
    dump_expr(&b.tail, program, out);
    close_ty(out, b.ty, program);
}

fn dump_stmt(s: &TypedStmt, program: &TypedProgram, out: &mut String) {
    match &s.kind {
        // The let's type is now always known (inferred if unannotated) — the
        // resolve dump's `_` placeholder becomes the resolved `Type`.
        TypedStmtKind::Let { id, mutable, ty, value, .. } => {
            out.push_str("(let ");
            push_id(out, id.0);
            out.push(' ');
            if *mutable {
                out.push_str("mut ");
            }
            push_ty(out, *ty, program);
            out.push(' ');
            dump_expr(value, program, out);
            out.push(')');
        }
        TypedStmtKind::Assign { target, value } => {
            out.push_str("(assign ");
            dump_expr(target, program, out);
            out.push(' ');
            dump_expr(value, program, out);
            out.push(')');
        }
        TypedStmtKind::While { cond, body } => {
            out.push_str("(while ");
            dump_expr(cond, program, out);
            out.push(' ');
            dump_block(body, program, out);
            out.push(')');
        }
        TypedStmtKind::Break => out.push_str("(break)"),
        TypedStmtKind::Continue => out.push_str("(continue)"),
        TypedStmtKind::Expr(e) => {
            out.push_str("(expr ");
            dump_expr(e, program, out);
            out.push(')');
        }
    }
}

fn dump_args(args: &[TypedExpr], program: &TypedProgram, out: &mut String) {
    for a in args {
        out.push(' ');
        dump_expr(a, program, out);
    }
}

fn dump_expr(e: &TypedExpr, program: &TypedProgram, out: &mut String) {
    match &e.kind {
        TypedExprKind::IntLit(n) => {
            out.push_str("(int ");
            out.push_str(&n.to_string());
            close_ty(out, e.ty, program);
        }
        // ADR 0058: a float literal renders by value (`{:?}` keeps the dot).
        TypedExprKind::FloatLit(bits) => {
            out.push_str("(float ");
            out.push_str(&format!("{:?}", f64::from_bits(*bits)));
            close_ty(out, e.ty, program);
        }
        TypedExprKind::BoolLit(b) => {
            out.push_str(if *b { "(bool true" } else { "(bool false" });
            close_ty(out, e.ty, program);
        }
        TypedExprKind::NullLit => {
            out.push_str("(null");
            close_ty(out, e.ty, program);
        }
        TypedExprKind::CharLit(byte) => {
            out.push_str("(char ");
            out.push_str(&byte.to_string());
            close_ty(out, e.ty, program);
        }
        TypedExprKind::StringLit(bytes) => {
            out.push_str("(str");
            for b in bytes {
                out.push(' ');
                out.push_str(&b.to_string());
            }
            close_ty(out, e.ty, program);
        }
        // Synthesized implicit coercions — not present in the resolve dump.
        TypedExprKind::WidenToNullable(inner) => {
            out.push_str("(widen-null ");
            dump_expr(inner, program, out);
            close_ty(out, e.ty, program);
        }
        TypedExprKind::WidenToSecret(inner) => {
            out.push_str("(widen-secret ");
            dump_expr(inner, program, out);
            close_ty(out, e.ty, program);
        }
        TypedExprKind::Declassify(inner) => {
            out.push_str("(declassify ");
            dump_expr(inner, program, out);
            close_ty(out, e.ty, program);
        }
        TypedExprKind::Return(inner) => {
            out.push_str("(return ");
            dump_expr(inner, program, out);
            close_ty(out, e.ty, program);
        }
        TypedExprKind::Cast(inner) => {
            out.push_str("(cast ");
            dump_expr(inner, program, out);
            close_ty(out, e.ty, program);
        }
        TypedExprKind::Var(id) => {
            out.push_str("(var ");
            push_id(out, id.0);
            close_ty(out, e.ty, program);
        }
        TypedExprKind::Unary(op, inner) => {
            out.push_str("(unary ");
            out.push_str(op.symbol());
            out.push(' ');
            dump_expr(inner, program, out);
            close_ty(out, e.ty, program);
        }
        TypedExprKind::Binary(op, l, r) => {
            out.push_str("(binop ");
            out.push_str(op.symbol());
            out.push(' ');
            dump_expr(l, program, out);
            out.push(' ');
            dump_expr(r, program, out);
            close_ty(out, e.ty, program);
        }
        TypedExprKind::Cmp(op, l, r) => {
            out.push_str("(cmp ");
            out.push_str(op.symbol());
            out.push(' ');
            dump_expr(l, program, out);
            out.push(' ');
            dump_expr(r, program, out);
            close_ty(out, e.ty, program);
        }
        TypedExprKind::Logic(op, l, r) => {
            out.push_str("(logic ");
            out.push_str(op.symbol());
            out.push(' ');
            dump_expr(l, program, out);
            out.push(' ');
            dump_expr(r, program, out);
            close_ty(out, e.ty, program);
        }
        // A Block expr's dump IS the block (self-annotated) — no extra wrap.
        TypedExprKind::Block(b) => dump_block(b, program, out),
        TypedExprKind::If { cond, then_branch, else_branch } => {
            out.push_str("(if ");
            dump_expr(cond, program, out);
            out.push(' ');
            dump_block(then_branch, program, out);
            out.push(' ');
            dump_block(else_branch, program, out);
            close_ty(out, e.ty, program);
        }
        TypedExprKind::Call { id, args, type_args, .. } => {
            out.push_str("(call ");
            push_id(out, id.0);
            // Concrete type-args inferred for a generic-fn call (empty for
            // non-generic calls) — a types-stage product (ADR 0016 D7c).
            if !type_args.is_empty() {
                out.push_str(" (targs");
                for ta in type_args {
                    out.push(' ');
                    push_ty(out, *ta, program);
                }
                out.push(')');
            }
            dump_args(args, program, out);
            close_ty(out, e.ty, program);
        }
        TypedExprKind::ResumeKont { kont, args, .. } => {
            out.push_str("(resume-kont ");
            push_id(out, kont.0);
            dump_args(args, program, out);
            close_ty(out, e.ty, program);
        }
        // The typed StructLit's fields are reordered to declaration order
        // and stripped of names (positional values), so codegen lowers by
        // index — dump them positionally.
        TypedExprKind::StructLit { id, name, fields, .. } => {
            out.push_str("(struct-lit ");
            push_id(out, id.0);
            out.push(' ');
            out.push_str(name);
            dump_args(fields, program, out);
            close_ty(out, e.ty, program);
        }
        // Types computes the field's declaration index for codegen's GEP.
        TypedExprKind::FieldAccess { target, field, field_index, .. } => {
            out.push_str("(field ");
            dump_expr(target, program, out);
            out.push(' ');
            out.push_str(field);
            out.push(' ');
            out.push_str(&field_index.to_string());
            close_ty(out, e.ty, program);
        }
        TypedExprKind::ArrayLit { elements, .. } => {
            out.push_str("(array");
            dump_args(elements, program, out);
            close_ty(out, e.ty, program);
        }
        TypedExprKind::Index { target, index, .. } => {
            out.push_str("(index ");
            dump_expr(target, program, out);
            out.push(' ');
            dump_expr(index, program, out);
            close_ty(out, e.ty, program);
        }
        TypedExprKind::Handle { body, arms, return_arm, .. } => {
            out.push_str("(handle ");
            dump_expr(body, program, out);
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
                dump_expr(&arm.body, program, out);
                out.push(')');
            }
            if let Some(ra) = return_arm {
                out.push_str(" (return ");
                push_id(out, ra.value_var_id.0);
                out.push(' ');
                dump_expr(&ra.body, program, out);
                out.push(')');
            }
            close_ty(out, e.ty, program);
        }
        TypedExprKind::Perform { effect_id, op_index, effect_name, op_name, args, .. } => {
            out.push_str("(perform ");
            push_id(out, effect_id.0);
            out.push(' ');
            out.push_str(&op_index.to_string());
            out.push(' ');
            out.push_str(effect_name);
            out.push(' ');
            out.push_str(op_name);
            dump_args(args, program, out);
            close_ty(out, e.ty, program);
        }
        // Receiver-typed dispatch resolved to a class's own method (the
        // resolver's uniform `(method …)`, now type-resolved).
        TypedExprKind::MethodCall { target, class_id, method_index, method, args, .. } => {
            out.push_str("(method ");
            push_id(out, class_id.0);
            out.push(' ');
            out.push_str(&method_index.to_string());
            out.push(' ');
            dump_expr(target, program, out);
            out.push(' ');
            out.push_str(method);
            dump_args(args, program, out);
            close_ty(out, e.ty, program);
        }
        // Receiver-typed dispatch resolved to a default-impl method (ADR
        // 0023 D5 Path 1) — the class itself had no such method.
        TypedExprKind::ImplMethodCall { target, impl_id, method_index, method, args, .. } => {
            out.push_str("(impl-method ");
            push_id(out, impl_id.0);
            out.push(' ');
            out.push_str(&method_index.to_string());
            out.push(' ');
            dump_expr(target, program, out);
            out.push(' ');
            out.push_str(method);
            dump_args(args, program, out);
            close_ty(out, e.ty, program);
        }
        TypedExprKind::QualifiedCall { impl_id, method_index, impl_name, method, args, .. } => {
            out.push_str("(qcall-impl ");
            push_id(out, impl_id.0);
            out.push(' ');
            out.push_str(&method_index.to_string());
            out.push(' ');
            out.push_str(impl_name);
            out.push(' ');
            out.push_str(method);
            dump_args(args, program, out);
            close_ty(out, e.ty, program);
        }
        TypedExprKind::ClassInit { id, name, args, .. } => {
            out.push_str("(class-init ");
            push_id(out, id.0);
            out.push(' ');
            out.push_str(name);
            dump_args(args, program, out);
            close_ty(out, e.ty, program);
        }
        TypedExprKind::Scope { body, .. } => {
            out.push_str("(scope ");
            dump_block(body, program, out);
            close_ty(out, e.ty, program);
        }
        TypedExprKind::Spawn { call, .. } => {
            out.push_str("(spawn ");
            dump_expr(call, program, out);
            close_ty(out, e.ty, program);
        }
        TypedExprKind::Await { task_expr, .. } => {
            out.push_str("(await ");
            dump_expr(task_expr, program, out);
            close_ty(out, e.ty, program);
        }
        // Types computes the variant discriminant (variant_index).
        TypedExprKind::EnumConstruct { enum_id, variant_index, enum_name, variant_name, args } => {
            out.push_str("(enum-construct ");
            push_id(out, enum_id.0);
            out.push(' ');
            out.push_str(&variant_index.to_string());
            out.push(' ');
            out.push_str(enum_name);
            out.push(' ');
            out.push_str(variant_name);
            dump_args(args, program, out);
            close_ty(out, e.ty, program);
        }
        // Types resolves the scrutinee's enum + each pattern's variant index
        // + each binding's payload type.
        TypedExprKind::Match { scrutinee, enum_id, arms } => {
            out.push_str("(match ");
            dump_expr(scrutinee, program, out);
            out.push(' ');
            push_id(out, enum_id.0);
            for arm in arms {
                out.push_str(" (arm ");
                dump_pattern(&arm.pattern, program, out);
                out.push(' ');
                dump_expr(&arm.body, program, out);
                out.push(')');
            }
            close_ty(out, e.ty, program);
        }
    }
}

fn dump_pattern(p: &TypedPattern, program: &TypedProgram, out: &mut String) {
    match p {
        TypedPattern::Variant { variant_index, variant_name, bindings, .. } => {
            out.push_str("(pat ");
            out.push_str(&variant_index.to_string());
            out.push(' ');
            out.push_str(variant_name);
            for b in bindings {
                out.push_str(" (bind ");
                push_id(out, b.var_id.0);
                out.push(' ');
                out.push_str(&b.name);
                out.push(' ');
                push_ty(out, b.ty, program);
                out.push(')');
            }
            out.push(')');
        }
        TypedPattern::Wildcard(_) => out.push_str("(pat _)"),
    }
}

fn dump_struct(s: &TypedStructDecl, program: &TypedProgram, out: &mut String) {
    out.push_str("(struct ");
    push_id(out, s.id.0);
    out.push(' ');
    out.push_str(&s.name);
    for f in &s.fields {
        out.push_str(" (field ");
        out.push_str(&f.name);
        out.push(' ');
        push_ty(out, f.ty, program);
        out.push(')');
    }
    out.push(')');
}

fn dump_enum(e: &EnumData, program: &TypedProgram, out: &mut String) {
    out.push_str("(enum ");
    push_id(out, e.id.0);
    out.push(' ');
    out.push_str(&e.name);
    for v in &e.variants {
        out.push_str(" (variant ");
        out.push_str(&v.name);
        for p in &v.payloads {
            out.push(' ');
            push_ty(out, *p, program);
        }
        out.push(')');
    }
    out.push(')');
}

fn dump_effect(e: &TypedEffectDecl, program: &TypedProgram, out: &mut String) {
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
            push_ty(out, p.ty, program);
            out.push(')');
        }
        out.push_str(") ");
        // The typed op return type is concrete (defaults to i64 upstream).
        push_ty(out, op.return_type, program);
        out.push(')');
    }
    out.push(')');
}

fn dump_trait(t: &TraitData, program: &TypedProgram, out: &mut String) {
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
            push_ty(out, p.ty, program);
            out.push(')');
        }
        out.push_str(") ");
        push_ty(out, m.return_type, program);
        out.push(')');
    }
    out.push(')');
}

fn dump_impl(i: &ImplData, program: &TypedProgram, out: &mut String) {
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
            dump_param(p, program, out);
        }
        out.push_str(") ");
        push_ty(out, m.return_type, program);
        out.push(' ');
        dump_block(&m.body, program, out);
        out.push(')');
    }
    out.push(')');
}

fn dump_class(c: &ClassData, program: &TypedProgram, out: &mut String) {
    out.push_str("(class ");
    push_id(out, c.id.0);
    out.push(' ');
    out.push_str(&c.name);
    for f in &c.fields {
        out.push_str(" (field ");
        out.push_str(&f.name);
        out.push(' ');
        push_ty(out, f.ty, program);
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
            dump_param(p, program, out);
        }
        out.push_str(") ");
        dump_block(&init.body, program, out);
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
            dump_param(p, program, out);
        }
        out.push_str(") ");
        push_ty(out, m.return_type, program);
        out.push(' ');
        dump_block(&m.body, program, out);
        out.push(')');
    }
    out.push(')');
}
