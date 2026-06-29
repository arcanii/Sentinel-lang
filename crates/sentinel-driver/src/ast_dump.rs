//! Phase D self-host port (2/N) / ADR 0039 D2: the parser differential
//! oracle — a complete, regular S-expression dump of the AST that the
//! Sentinel-written parser (`selfhost/parser.sentinel`) reproduces
//! byte-for-byte. Distinct from the existing `snc parse` `Display` (which is
//! human-oriented + incomplete — `Program`'s `Display` omits
//! enums/traits/impls/classes). This dump is fully tagged + parenthesized so
//! the Sentinel side reproduces it from a uniform recursive walk; node
//! *names* (never numeric tags) are emitted. A dev/validation surface, NOT
//! `abi-v1` — freely amendable; pinned by a golden test.
//!
//! (2a)–(2c) scope: the function-level grammar — `fn` defs + every `Stmt` /
//! `Expr` / `TypeExpr` / `Pattern`. (2d) extends `dump` to the remaining
//! top-level decl kinds (use / struct / enum / effect / trait / impl / class),
//! landed one kind per increment. **Decls dump in source order** — the parsed
//! `Program` buckets decls by kind (separate `Vec`s), so `dump` re-collapses
//! them into a single list sorted by span start, which is exactly the order the
//! Sentinel parser emits them as it scans the token stream top-to-bottom.
//!
//! (2d-1, this increment): `use a::b::Item;` → `(use a b Item)`.

use sentinel_ast::{Block, ClassDecl, EffectDecl, EnumDecl, Expr, ExprKind, FnDef, ImplDecl,
    ModuleDecl, Param, PartDecl, Pattern, Program, SelfKind, Stmt, StmtKind, StructDecl, TraitDecl,
    TypeExpr, TypeExprKind, UseDecl};

/// One top-level declaration, tagged for the source-order re-collation in
/// [`dump`]. Grows a variant per (2d) increment as each decl kind lands.
enum Item<'a> {
    /// ADR 0067: `module a::b;` / `part name;` discovery directives.
    Module(&'a ModuleDecl),
    Part(&'a PartDecl),
    Use(&'a UseDecl),
    Fn(&'a FnDef),
    Struct(&'a StructDecl),
    Enum(&'a EnumDecl),
    Effect(&'a EffectDecl),
    Trait(&'a TraitDecl),
    Impl(&'a ImplDecl),
    Class(&'a ClassDecl),
}

/// The `self` receiver kind as a bare dump word.
fn self_kind_word(k: SelfKind) -> &'static str {
    match k {
        SelfKind::Shared => "shared",
        SelfKind::Exclusive => "exclusive",
    }
}

/// The shared method head — `(method name <shared|exclusive> (<params>) <ret>`
/// (no leading space, no closing paren). Callers prepend ` ` and append `)` (a
/// trait sig) or ` <block>)` (an impl / class method body).
fn dump_method_sig(name: &str, self_kind: SelfKind, params: &[Param], return_type: &TypeExpr,
    out: &mut String) {
    out.push_str("(method ");
    out.push_str(name);
    out.push(' ');
    out.push_str(self_kind_word(self_kind));
    out.push_str(" (");
    for (i, p) in params.iter().enumerate() {
        if i > 0 {
            out.push(' ');
        }
        dump_param(p, out);
    }
    out.push_str(") ");
    dump_type(return_type, out);
}

/// Canonical S-expression dump of `program` — every top-level decl in source
/// order (newline-separated). The parser buckets decls by kind, so we re-sort
/// by span start to recover the source order the Sentinel parser produces.
pub fn dump(program: &Program) -> String {
    let mut items: Vec<(usize, Item)> = Vec::new();
    // ADR 0067: the `module` decl + `part` manifest sort first (smallest span).
    if let Some(m) = &program.module {
        items.push((m.span.start, Item::Module(m)));
    }
    for p in &program.parts {
        items.push((p.span.start, Item::Part(p)));
    }
    for u in &program.uses {
        items.push((u.span.start, Item::Use(u)));
    }
    for f in &program.fns {
        items.push((f.span.start, Item::Fn(f)));
    }
    for s in &program.structs {
        items.push((s.span.start, Item::Struct(s)));
    }
    for en in &program.enums {
        items.push((en.span.start, Item::Enum(en)));
    }
    for ef in &program.effects {
        items.push((ef.span.start, Item::Effect(ef)));
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
            Item::Module(m) => dump_module(m, &mut out),
            Item::Part(p) => dump_part(p, &mut out),
            Item::Use(u) => dump_use(u, &mut out),
            Item::Fn(f) => dump_fn(f, &mut out),
            Item::Struct(s) => dump_struct(s, &mut out),
            Item::Enum(en) => dump_enum(en, &mut out),
            Item::Effect(ef) => dump_effect(ef, &mut out),
            Item::Trait(tr) => dump_trait(tr, &mut out),
            Item::Impl(im) => dump_impl(im, &mut out),
            Item::Class(c) => dump_class(c, &mut out),
        }
    }
    out.push('\n');
    out
}

/// ADR 0067: `module a::b;` → `(module a b)` (segments space-separated, like
/// `use`); `part name;` → `(part name)`.
fn dump_module(m: &ModuleDecl, out: &mut String) {
    out.push_str("(module");
    for seg in &m.path {
        out.push(' ');
        out.push_str(seg);
    }
    out.push(')');
}

fn dump_part(p: &PartDecl, out: &mut String) {
    out.push_str("(part ");
    out.push_str(&p.name);
    out.push(')');
}

/// `use a::b::Item;` → `(use a b Item)` (each path segment space-separated).
fn dump_use(u: &UseDecl, out: &mut String) {
    out.push_str("(use");
    for seg in &u.path {
        out.push(' ');
        out.push_str(seg);
    }
    out.push(')');
}

/// `struct Name { f: T, … }` → `(struct Name (field f <type>) …)` — generic
/// type-params + visibility omitted (as `dump_fn` omits them). Empty → `(struct
/// Name)`.
fn dump_struct(s: &StructDecl, out: &mut String) {
    out.push_str("(struct ");
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

/// `enum Name { V1, V2(T), … }` → `(enum Name (variant V1) (variant V2 <type>)
/// …)` — a unit variant has no payload types, a payload variant lists them
/// positionally. Empty → `(enum Name)`.
fn dump_enum(e: &EnumDecl, out: &mut String) {
    out.push_str("(enum ");
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

/// `effect Name { op(p: T) -> R; … }` → `(effect Name (op op (<params>) <ret>)
/// …)`. An op's params dump like fn params; a **missing** return type dumps `_`
/// (matching the `let`/`TyOpt` convention). Empty → `(effect Name)`.
fn dump_effect(e: &EffectDecl, out: &mut String) {
    out.push_str("(effect ");
    out.push_str(&e.name);
    for op in &e.ops {
        out.push_str(" (op ");
        out.push_str(&op.name);
        out.push_str(" (");
        for (i, p) in op.params.iter().enumerate() {
            if i > 0 {
                out.push(' ');
            }
            dump_param(p, out);
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

/// `trait Name { fn m(self: &Self, p: T) -> R; … }` → `(trait Name (method m
/// <shared|exclusive> (<params>) <ret>) …)`. Trait method sigs have no body; the
/// non-self params dump like fn params; `self` dumps as its kind word, the
/// effect row is omitted. Empty → `(trait Name)`.
fn dump_trait(t: &TraitDecl, out: &mut String) {
    out.push_str("(trait ");
    out.push_str(&t.name);
    for m in &t.methods {
        out.push(' ');
        dump_method_sig(&m.name, m.self_kind, &m.params, &m.return_type, out);
        out.push(')');
    }
    out.push(')');
}

/// `impl Name? as Trait for Type { fn m(self: &Self, …) -> R { … } … }` →
/// `(impl <name-or-_> Trait Type (method m <self> (<params>) <ret> <block>) …)`.
/// A default impl (no name) dumps `_` in the name slot; impl methods carry a
/// body (vs trait sigs).
fn dump_impl(i: &ImplDecl, out: &mut String) {
    out.push_str("(impl ");
    match &i.name {
        Some(n) => out.push_str(n),
        None => out.push('_'),
    }
    out.push(' ');
    out.push_str(&i.trait_name);
    out.push(' ');
    out.push_str(&i.type_name);
    for m in &i.methods {
        out.push(' ');
        dump_method_sig(&m.name, m.self_kind, &m.params, &m.return_type, out);
        out.push(' ');
        dump_block(&m.body, out);
        out.push(')');
    }
    out.push(')');
}

/// `class Name { let f: T; init(p: T) { … } fn m(self: &Self) -> R { … }
/// delegate g: T to Tr; }` → `(class Name (field f <type>) … (init (<params>)
/// <block>)? (method m <self> (<params>) <ret> <block>) … (delegate g <type>
/// Tr) …)`. Class items are **bucketed** in the AST (fields / init / methods /
/// delegates), so they dump in that fixed order regardless of source order;
/// visibility is omitted. Empty → `(class Name)`.
fn dump_class(c: &ClassDecl, out: &mut String) {
    out.push_str("(class ");
    out.push_str(&c.name);
    for f in &c.fields {
        out.push_str(" (field ");
        out.push_str(&f.name);
        out.push(' ');
        dump_type(&f.ty, out);
        out.push(')');
    }
    if let Some(init) = &c.init {
        out.push_str(" (init (");
        for (i, p) in init.params.iter().enumerate() {
            if i > 0 {
                out.push(' ');
            }
            dump_param(p, out);
        }
        out.push_str(") ");
        dump_block(&init.body, out);
        out.push(')');
    }
    for m in &c.methods {
        out.push(' ');
        dump_method_sig(&m.name, m.self_kind, &m.params, &m.return_type, out);
        out.push(' ');
        dump_block(&m.body, out);
        out.push(')');
    }
    for d in &c.delegates {
        out.push_str(" (delegate ");
        out.push_str(&d.field_name);
        out.push(' ');
        dump_type(&d.ty, out);
        out.push(' ');
        out.push_str(&d.trait_name);
        out.push(')');
    }
    out.push(')');
}

fn dump_fn(f: &FnDef, out: &mut String) {
    out.push_str("(fn ");
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

fn dump_param(p: &Param, out: &mut String) {
    out.push_str("(param ");
    if p.mutable {
        out.push_str("mut ");
    }
    out.push_str(&p.name);
    out.push(' ');
    dump_type(&p.ty, out);
    out.push(')');
}

pub(crate) fn dump_type(t: &TypeExpr, out: &mut String) {
    match &t.kind {
        TypeExprKind::Ident(name) => out.push_str(name),
        TypeExprKind::Nullable(inner) => {
            out.push_str("(opt ");
            dump_type(inner, out);
            out.push(')');
        }
        TypeExprKind::Array(inner) => {
            out.push_str("(arr ");
            dump_type(inner, out);
            out.push(')');
        }
        TypeExprKind::Ref { mutable, inner } => {
            out.push_str(if *mutable { "(refmut " } else { "(ref " });
            dump_type(inner, out);
            out.push(')');
        }
        TypeExprKind::Generic { name, args, .. } => {
            out.push_str("(generic ");
            out.push_str(name);
            for a in args {
                out.push(' ');
                dump_type(a, out);
            }
            out.push(')');
        }
        TypeExprKind::Secret(inner) => {
            out.push_str("(secret ");
            dump_type(inner, out);
            out.push(')');
        }
    }
}

fn dump_block(b: &Block, out: &mut String) {
    out.push_str("(block");
    for s in &b.stmts {
        out.push(' ');
        dump_stmt(s, out);
    }
    out.push(' ');
    dump_expr(&b.tail, out);
    out.push(')');
}

fn dump_stmt(s: &Stmt, out: &mut String) {
    match &s.kind {
        StmtKind::Let { mutable, name, ty_annot, value, .. } => {
            out.push_str("(let ");
            if *mutable {
                out.push_str("mut ");
            }
            out.push_str(name);
            out.push(' ');
            match ty_annot {
                Some(t) => dump_type(t, out),
                None => out.push('_'),
            }
            out.push(' ');
            dump_expr(value, out);
            out.push(')');
        }
        StmtKind::Assign { target, value } => {
            out.push_str("(assign ");
            dump_expr(target, out);
            out.push(' ');
            dump_expr(value, out);
            out.push(')');
        }
        StmtKind::While { cond, body } => {
            out.push_str("(while ");
            dump_expr(cond, out);
            out.push(' ');
            dump_block(body, out);
            out.push(')');
        }
        StmtKind::Break => out.push_str("(break)"),
        StmtKind::Continue => out.push_str("(continue)"),
        StmtKind::Expr(e) => {
            out.push_str("(expr ");
            dump_expr(e, out);
            out.push(')');
        }
    }
}

fn dump_args(args: &[Expr], out: &mut String) {
    for a in args {
        out.push(' ');
        dump_expr(a, out);
    }
}

fn dump_expr(e: &Expr, out: &mut String) {
    match &e.kind {
        ExprKind::IntLit(n) => {
            out.push_str("(int ");
            out.push_str(&n.to_string());
            out.push(')');
        }
        // ADR 0058: render a float literal by its value (`{:?}` keeps a
        // decimal point, so `1.0` not `1`).
        ExprKind::FloatLit(bits) => {
            out.push_str("(float ");
            out.push_str(&format!("{:?}", f64::from_bits(*bits)));
            out.push(')');
        }
        ExprKind::BoolLit(b) => {
            out.push_str(if *b { "(bool true)" } else { "(bool false)" });
        }
        ExprKind::NullLit => out.push_str("(null)"),
        ExprKind::CharLit(byte) => {
            out.push_str("(char ");
            out.push_str(&byte.to_string());
            out.push(')');
        }
        ExprKind::StringLit(bytes) => {
            out.push_str("(str");
            for b in bytes {
                out.push(' ');
                out.push_str(&b.to_string());
            }
            out.push(')');
        }
        ExprKind::Var(name) => {
            out.push_str("(var ");
            out.push_str(name);
            out.push(')');
        }
        ExprKind::Unary(op, inner) => {
            out.push_str("(unary ");
            out.push_str(op.symbol());
            out.push(' ');
            dump_expr(inner, out);
            out.push(')');
        }
        ExprKind::Binary(op, l, r) => {
            out.push_str("(binop ");
            out.push_str(op.symbol());
            out.push(' ');
            dump_expr(l, out);
            out.push(' ');
            dump_expr(r, out);
            out.push(')');
        }
        ExprKind::Cmp(op, l, r) => {
            out.push_str("(cmp ");
            out.push_str(op.symbol());
            out.push(' ');
            dump_expr(l, out);
            out.push(' ');
            dump_expr(r, out);
            out.push(')');
        }
        ExprKind::Logic(op, l, r) => {
            out.push_str("(logic ");
            out.push_str(op.symbol());
            out.push(' ');
            dump_expr(l, out);
            out.push(' ');
            dump_expr(r, out);
            out.push(')');
        }
        ExprKind::Block(b) => dump_block(b, out),
        ExprKind::If { cond, then_branch, else_branch } => {
            out.push_str("(if ");
            dump_expr(cond, out);
            out.push(' ');
            dump_block(then_branch, out);
            out.push(' ');
            dump_block(else_branch, out);
            out.push(')');
        }
        ExprKind::Call { callee, args, .. } => {
            out.push_str("(call ");
            out.push_str(callee);
            dump_args(args, out);
            out.push(')');
        }
        ExprKind::StructLit { name, fields, .. } => {
            out.push_str("(struct-lit ");
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
        ExprKind::FieldAccess { target, field, .. } => {
            out.push_str("(field ");
            dump_expr(target, out);
            out.push(' ');
            out.push_str(field);
            out.push(')');
        }
        ExprKind::ArrayLit(elems) => {
            out.push_str("(array");
            dump_args(elems, out);
            out.push(')');
        }
        ExprKind::Index { target, index } => {
            out.push_str("(index ");
            dump_expr(target, out);
            out.push(' ');
            dump_expr(index, out);
            out.push(')');
        }
        ExprKind::Declassify(inner) => {
            out.push_str("(declassify ");
            dump_expr(inner, out);
            out.push(')');
        }
        ExprKind::Return(inner) => {
            out.push_str("(return ");
            dump_expr(inner, out);
            out.push(')');
        }
        ExprKind::Cast(inner, te) => {
            out.push_str("(cast ");
            dump_expr(inner, out);
            out.push(' ');
            out.push_str(&te.kind.to_string());
            out.push(')');
        }
        ExprKind::Perform { effect, op, args } => {
            out.push_str("(perform ");
            out.push_str(&effect.kind);
            out.push(' ');
            out.push_str(&op.kind);
            dump_args(args, out);
            out.push(')');
        }
        ExprKind::Handle { body, arms, return_arm } => {
            out.push_str("(handle ");
            dump_expr(body, out);
            for arm in arms {
                out.push_str(" (arm ");
                out.push_str(&arm.effect.kind);
                out.push(' ');
                out.push_str(&arm.op.kind);
                out.push(' ');
                dump_expr(&arm.body, out);
                out.push(')');
            }
            if let Some(ra) = return_arm {
                out.push_str(" (return ");
                out.push_str(&ra.value_name.kind);
                out.push(' ');
                dump_expr(&ra.body, out);
                out.push(')');
            }
            out.push(')');
        }
        ExprKind::MethodCall { target, method, args, .. } => {
            out.push_str("(method ");
            dump_expr(target, out);
            out.push(' ');
            out.push_str(method);
            dump_args(args, out);
            out.push(')');
        }
        ExprKind::ClassInit { class_name, args, .. } => {
            out.push_str("(class-init ");
            out.push_str(class_name);
            dump_args(args, out);
            out.push(')');
        }
        ExprKind::QualifiedCall { impl_name, method, args, .. } => {
            out.push_str("(qcall ");
            out.push_str(impl_name);
            out.push(' ');
            out.push_str(method);
            dump_args(args, out);
            out.push(')');
        }
        ExprKind::Scope { body, .. } => {
            out.push_str("(scope ");
            dump_block(body, out);
            out.push(')');
        }
        ExprKind::Spawn { call_expr } => {
            out.push_str("(spawn ");
            dump_expr(call_expr, out);
            out.push(')');
        }
        ExprKind::Await { task_expr } => {
            out.push_str("(await ");
            dump_expr(task_expr, out);
            out.push(')');
        }
        ExprKind::Match { scrutinee, arms } => {
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

fn dump_pattern(p: &Pattern, out: &mut String) {
    match p {
        Pattern::Variant { enum_name, variant, bindings, .. } => {
            out.push_str("(pat ");
            out.push_str(enum_name);
            out.push(' ');
            out.push_str(variant);
            for b in bindings {
                out.push(' ');
                out.push_str(&b.kind);
            }
            out.push(')');
        }
        Pattern::Wildcard(_) => out.push_str("(pat _)"),
    }
}
