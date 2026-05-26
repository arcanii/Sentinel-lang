//! sentinel-resolve
//!
//! Name resolution for Sentinel. Takes a parsed [`Program`] and
//! produces a [`ResolvedProgram`] where every name reference is a
//! stable identifier ([`VarId`] / [`FnId`]) — no more string lookups
//! downstream.
//!
//! Per ADR 0011 D4, this crate lifts the name-resolution logic that
//! `sentinel-codegen::CodegenCtx` has been carrying since C0.3.
//! Codegen at C1.1+ consumes a `ResolvedProgram` instead of a raw
//! `Program`; it never needs to do its own var/fn lookups, and the
//! `UndefinedVariable` / `UndefinedFunction` / `RedeclaredVariable`
//! / `RedefinedFunction` / `MissingMain` / `ArityMismatch`
//! diagnostics fire here.
//!
//! The representation is a parallel tree: [`ResolvedExpr`],
//! [`ResolvedStmt`], [`ResolvedBlock`], [`ResolvedFnDef`],
//! [`ResolvedParam`], [`ResolvedProgram`] mirror their AST
//! counterparts, with name-reference fields swapped for IDs.
//! Binding sites retain their source name (for diagnostics + LLVM
//! IR debug names); reference sites carry only IDs.
//!
//! `resolve(program) -> Result<ResolvedProgram, ResolveError>` is
//! the pure-function entry point. `resolve_query(db, file)` is the
//! `#[salsa::tracked]` wrapper for the driver's incremental
//! pipeline (per ADR 0011 D1, downstream stages of C1.0b's lex/
//! parse retrofit).

use std::collections::HashMap;

use salsa::Accumulator;
use sentinel_ast::{
    BinOp, Block, CmpOp, Expr, ExprKind, FnDef, LogicOp, Program, Span, Spanned, Stmt, StmtKind,
    TypeExpr, UnaryOp,
};
use sentinel_base::{Diagnostic, SentinelDb, Severity, SourceFile};

// =============================================================================
// Stable identifiers
// =============================================================================

/// Identifier for a value binding (parameter or `let`). Unique
/// per-program; assigned by [`resolve`] in source order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct VarId(pub u32);

/// Identifier for a function. Unique per-program; assigned by
/// [`resolve`] in source order, with `print` taking `FnId(0)`
/// because the runtime symbol is pre-registered before user fns.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct FnId(pub u32);

/// Sentinel's runtime `print` is always `FnId(0)`. Pre-registered
/// by [`resolve`]; user fns get higher IDs.
pub const PRINT_FN_ID: FnId = FnId(0);

/// `unwrap_or(x: ?T, default: T) -> T` per ADR 0014 D9. Generic
/// over T; the type checker special-cases the typing rule. Codegen
/// inlines per call site.
pub const UNWRAP_OR_FN_ID: FnId = FnId(1);

/// `is_some(x: ?T) -> bool` per ADR 0014 D9. Generic over T; same
/// machinery as `unwrap_or`.
pub const IS_SOME_FN_ID: FnId = FnId(2);

/// Identifier for a struct declaration. Added at C1.4 per ADR 0013
/// D4 / D5; unique per-program, assigned in source order starting
/// at 0.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct StructId(pub u32);

/// A function's signature as needed by call-site resolution and
/// arity-checking. Stored on the resolved program for codegen to
/// consult without re-walking source.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct FnSignature {
    pub id: FnId,
    pub name: String,
    /// Span of the function's name at its definition site (or
    /// `None` for the synthetic `print` runtime entry).
    pub name_span: Option<Span>,
    /// Number of formal parameters.
    pub arity: usize,
    /// `true` iff this is the user's `main`. Codegen uses this to
    /// emit the C-ABI i32 return.
    pub is_main: bool,
    /// `true` iff this is the runtime `print` symbol. Codegen uses
    /// this to map the call to `sentinel_print`.
    pub is_runtime: bool,
}

// =============================================================================
// Resolved AST — parallel tree
// =============================================================================

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ResolvedProgram {
    pub fns: Vec<ResolvedFnDef>,
    /// Lookup table — index by `FnId.0 as usize` for O(1) signature
    /// access from codegen / future passes.
    pub fn_signatures: Vec<FnSignature>,
    /// Struct declarations in source order, added at C1.4. Each
    /// carries its own [`StructId`] matching its index in this vec.
    /// The struct table is built before fn signatures so fn
    /// parameters can reference struct names per ADR 0013 D4.
    pub structs: Vec<ResolvedStructDecl>,
    pub span: Span,
}

/// A struct declaration after name resolution. The decl's
/// [`StructId`] matches its index in [`ResolvedProgram::structs`].
/// Field-type annotations are carried as [`TypeExpr`] (string-keyed)
/// — sentinel-types resolves those at C1.4.5.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ResolvedStructDecl {
    pub id: StructId,
    pub name: String,
    pub name_span: Span,
    pub fields: Vec<ResolvedStructField>,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ResolvedStructField {
    pub name: String,
    pub name_span: Span,
    pub ty: TypeExpr,
    pub span: Span,
}

impl ResolvedProgram {
    /// The user `main` function. Always present after a successful
    /// `resolve` (otherwise [`ResolveError::MissingMain`] would have
    /// fired).
    pub fn main(&self) -> &ResolvedFnDef {
        self.fns
            .iter()
            .find(|f| f.signature(self).is_main)
            .expect("MissingMain would have fired in resolve")
    }

    /// Signature lookup by ID. Panics on out-of-range — IDs only
    /// come from [`resolve`], so this should never fail.
    pub fn signature(&self, id: FnId) -> &FnSignature {
        &self.fn_signatures[id.0 as usize]
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ResolvedFnDef {
    pub id: FnId,
    pub name: String,
    pub name_span: Span,
    pub params: Vec<ResolvedParam>,
    /// C1.2 (per ADR 0012 D1): mandatory return-type annotation.
    /// Carried through from the AST for the type checker to consume
    /// at C1.2's check() pass.
    pub return_type: TypeExpr,
    pub body: ResolvedBlock,
    pub span: Span,
}

impl ResolvedFnDef {
    pub fn signature<'a>(&self, program: &'a ResolvedProgram) -> &'a FnSignature {
        program.signature(self.id)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ResolvedParam {
    pub id: VarId,
    /// Source-level name retained for diagnostics and LLVM IR
    /// debug names.
    pub name: String,
    pub span: Span,
    /// C1.2 (per ADR 0012 D1): mandatory parameter-type annotation.
    pub ty: TypeExpr,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ResolvedBlock {
    pub stmts: Vec<ResolvedStmt>,
    pub tail: ResolvedExpr,
    pub span: Span,
}

pub type ResolvedStmt = Spanned<ResolvedStmtKind>;

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum ResolvedStmtKind {
    Let {
        id: VarId,
        /// Source-level name retained for diagnostics / IR debug.
        name: String,
        name_span: Span,
        /// C1.2 (per ADR 0012 D2): optional `let x: T = ...`
        /// annotation. None means "infer from RHS".
        ty_annot: Option<TypeExpr>,
        value: ResolvedExpr,
    },
    Expr(ResolvedExpr),
}

pub type ResolvedExpr = Spanned<ResolvedExprKind>;

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum ResolvedExprKind {
    IntLit(i64),
    /// Bool literal (`true` / `false`) per ADR 0012 D5. Added at C1.3.
    BoolLit(bool),
    /// Null literal per ADR 0014 D2. Added at C1.5. The type is
    /// resolved bidirectionally at the type-check stage.
    NullLit,
    /// Variable reference, resolved to a binding's [`VarId`].
    Var(VarId),
    Unary(UnaryOp, Box<ResolvedExpr>),
    Binary(BinOp, Box<ResolvedExpr>, Box<ResolvedExpr>),
    /// Comparison per ADR 0012 D6. Mirrors AST's [`ExprKind::Cmp`].
    Cmp(CmpOp, Box<ResolvedExpr>, Box<ResolvedExpr>),
    /// Logical `&&` / `||` per ADR 0012 D7. Mirrors AST's
    /// [`ExprKind::Logic`]. Short-circuit semantics belong to codegen.
    Logic(LogicOp, Box<ResolvedExpr>, Box<ResolvedExpr>),
    Block(Box<ResolvedBlock>),
    If {
        cond: Box<ResolvedExpr>,
        then_branch: Box<ResolvedBlock>,
        else_branch: Box<ResolvedBlock>,
    },
    Call {
        id: FnId,
        /// Span of the callee name (for diagnostics like arity
        /// mismatches, even though arity is checked here at resolve
        /// time).
        callee_span: Span,
        args: Vec<ResolvedExpr>,
    },
    /// Struct literal per ADR 0013 D3. The struct's [`StructId`] is
    /// resolved here; field names stay as strings (the type checker
    /// validates them at C1.4.5).
    StructLit {
        id: StructId,
        name: String,
        name_span: Span,
        fields: Vec<ResolvedFieldInit>,
    },
    /// Field access per ADR 0013 D2. The field stays as a string
    /// (the type checker validates it against the struct
    /// declaration at C1.4.5; we don't know the target's type yet).
    FieldAccess {
        target: Box<ResolvedExpr>,
        field: String,
        field_span: Span,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ResolvedFieldInit {
    pub name: String,
    pub name_span: Span,
    pub value: ResolvedExpr,
    pub span: Span,
}

// =============================================================================
// Errors
// =============================================================================

#[derive(Debug, Clone, thiserror::Error, miette::Diagnostic)]
pub enum ResolveError {
    #[error("undefined variable `{name}`")]
    #[diagnostic(
        code(sentinel::resolve::undefined_variable),
        help("declare it with `let {name} = ...;` before this reference")
    )]
    UndefinedVariable {
        name: String,
        #[label("not declared in this scope")]
        span: miette::SourceSpan,
    },

    #[error("variable `{name}` is already declared")]
    #[diagnostic(
        code(sentinel::resolve::redeclared_variable),
        help("C0 scoping is flat per function; pick a different name or remove the earlier binding")
    )]
    RedeclaredVariable {
        name: String,
        #[label("redeclaration here")]
        span: miette::SourceSpan,
    },

    #[error("undefined function `{name}`")]
    #[diagnostic(
        code(sentinel::resolve::undefined_function),
        help("define it with `fn {name}(…) {{ … }}` or use `print` for the runtime builtin")
    )]
    UndefinedFunction {
        name: String,
        #[label("no such function")]
        span: miette::SourceSpan,
    },

    #[error("`{name}` takes {expected} argument(s), got {got}")]
    #[diagnostic(code(sentinel::resolve::arity_mismatch))]
    ArityMismatch {
        name: String,
        expected: usize,
        got: usize,
        #[label("wrong number of arguments")]
        span: miette::SourceSpan,
    },

    #[error("function `{name}` is already declared")]
    #[diagnostic(
        code(sentinel::resolve::redefined_function),
        help("each fn name must be unique within a program; `print` is reserved by the runtime")
    )]
    RedefinedFunction {
        name: String,
        #[label("redefinition here")]
        span: miette::SourceSpan,
    },

    #[error("program has no `main` function")]
    #[diagnostic(
        code(sentinel::resolve::missing_main),
        help("add `fn main() {{ … }}` — it is the program entry point")
    )]
    MissingMain,

    /// C1.4 / ADR 0013 D1: struct names must be unique within a
    /// program.
    #[error("struct `{name}` is already declared")]
    #[diagnostic(
        code(sentinel::resolve::redefined_struct),
        help("each struct name must be unique within a program")
    )]
    RedefinedStruct {
        name: String,
        #[label("redefinition here")]
        span: miette::SourceSpan,
    },

    /// C1.4 / ADR 0013 D3: struct literal references an unknown
    /// struct name. (Type-position uses of unknown names surface as
    /// `TypeError::UnknownType` at the type-check stage, since the
    /// type-resolution lookup happens there. Expression-position
    /// uses are caught here at resolve time.)
    #[error("undefined struct `{name}`")]
    #[diagnostic(
        code(sentinel::resolve::undefined_struct),
        help("declare it with `struct {name} {{ … }}` at the top level before this reference")
    )]
    UndefinedStruct {
        name: String,
        #[label("no such struct")]
        span: miette::SourceSpan,
    },
}

// =============================================================================
// resolve() — pure-function entry point
// =============================================================================

/// Resolve a [`Program`] to a [`ResolvedProgram`]. Fails fast on
/// the first error encountered, matching the C0 codegen pass's
/// existing diagnostic shape. Multi-error accumulation is a future
/// concern.
///
/// Order: structs are registered first (so fn signatures can
/// reference struct types in their TypeExprs), then fns, then fn
/// bodies.
pub fn resolve(program: &Program) -> Result<ResolvedProgram, ResolveError> {
    let mut next_fn_id: u32 = 0;
    let mut next_var_id: u32 = 0;

    // Pass 0: collect struct declarations. Indexed by source order
    // (each struct's StructId matches its index in resolved_structs).
    // Names must be unique within a program; struct names and fn
    // names share a namespace at C1.4 (a struct named `print` is
    // illegal, same as a fn named `print`). Future ADRs may relax.
    let mut struct_table: HashMap<String, StructId> = HashMap::new();
    let mut resolved_structs: Vec<ResolvedStructDecl> =
        Vec::with_capacity(program.structs.len());
    for (idx, sd) in program.structs.iter().enumerate() {
        if struct_table.contains_key(&sd.name) {
            return Err(ResolveError::RedefinedStruct {
                name: sd.name.clone(),
                span: to_source_span(&sd.name_span),
            });
        }
        let id = StructId(idx as u32);
        struct_table.insert(sd.name.clone(), id);
        let fields = sd
            .fields
            .iter()
            .map(|f| ResolvedStructField {
                name: f.name.clone(),
                name_span: f.name_span.clone(),
                ty: f.ty.clone(),
                span: f.span.clone(),
            })
            .collect();
        resolved_structs.push(ResolvedStructDecl {
            id,
            name: sd.name.clone(),
            name_span: sd.name_span.clone(),
            fields,
            span: sd.span.clone(),
        });
    }

    // Pre-register the runtime builtins. The runtime (and codegen
    // for C1.5's generic builtins) supplies them; user code can't
    // redefine them (that path errors as RedefinedFunction below).
    let print_sig = FnSignature {
        id: FnId(next_fn_id),
        name: "print".to_string(),
        name_span: None,
        arity: 1,
        is_main: false,
        is_runtime: true,
    };
    next_fn_id += 1;
    let unwrap_or_sig = FnSignature {
        id: FnId(next_fn_id),
        name: "unwrap_or".to_string(),
        name_span: None,
        arity: 2,
        is_main: false,
        is_runtime: true,
    };
    next_fn_id += 1;
    let is_some_sig = FnSignature {
        id: FnId(next_fn_id),
        name: "is_some".to_string(),
        name_span: None,
        arity: 1,
        is_main: false,
        is_runtime: true,
    };
    next_fn_id += 1;

    let mut fn_table: HashMap<String, FnId> = HashMap::new();
    let mut signatures: Vec<FnSignature> = vec![print_sig, unwrap_or_sig, is_some_sig];
    fn_table.insert("print".to_string(), PRINT_FN_ID);
    fn_table.insert("unwrap_or".to_string(), UNWRAP_OR_FN_ID);
    fn_table.insert("is_some".to_string(), IS_SOME_FN_ID);

    // Pass 1: collect every fn into the table.
    for fn_def in &program.fns {
        if fn_table.contains_key(&fn_def.name) {
            return Err(ResolveError::RedefinedFunction {
                name: fn_def.name.clone(),
                span: to_source_span(&fn_def.name_span),
            });
        }
        let id = FnId(next_fn_id);
        next_fn_id += 1;
        let is_main = fn_def.name == "main";
        signatures.push(FnSignature {
            id,
            name: fn_def.name.clone(),
            name_span: Some(fn_def.name_span.clone()),
            arity: fn_def.params.len(),
            is_main,
            is_runtime: false,
        });
        fn_table.insert(fn_def.name.clone(), id);
    }

    if !fn_table.contains_key("main") {
        return Err(ResolveError::MissingMain);
    }

    // Pass 2: resolve each fn body.
    let mut resolved_fns = Vec::with_capacity(program.fns.len());
    for fn_def in &program.fns {
        resolved_fns.push(resolve_fn(
            fn_def,
            &fn_table,
            &signatures,
            &struct_table,
            &mut next_var_id,
        )?);
    }

    Ok(ResolvedProgram {
        fns: resolved_fns,
        fn_signatures: signatures,
        structs: resolved_structs,
        span: program.span.clone(),
    })
}

fn resolve_fn(
    fn_def: &FnDef,
    fn_table: &HashMap<String, FnId>,
    signatures: &[FnSignature],
    struct_table: &HashMap<String, StructId>,
    next_var_id: &mut u32,
) -> Result<ResolvedFnDef, ResolveError> {
    let id = *fn_table
        .get(&fn_def.name)
        .expect("registered in pass 1");

    let mut vars: HashMap<String, VarId> = HashMap::new();
    let mut params = Vec::with_capacity(fn_def.params.len());
    for param in &fn_def.params {
        if vars.contains_key(&param.name) {
            return Err(ResolveError::RedeclaredVariable {
                name: param.name.clone(),
                span: to_source_span(&param.span),
            });
        }
        let var_id = VarId(*next_var_id);
        *next_var_id += 1;
        vars.insert(param.name.clone(), var_id);
        params.push(ResolvedParam {
            id: var_id,
            name: param.name.clone(),
            span: param.span.clone(),
            ty: param.ty.clone(),
        });
    }

    let body = resolve_block(
        &fn_def.body,
        fn_table,
        signatures,
        struct_table,
        &mut vars,
        next_var_id,
    )?;

    Ok(ResolvedFnDef {
        id,
        name: fn_def.name.clone(),
        name_span: fn_def.name_span.clone(),
        params,
        return_type: fn_def.return_type.clone(),
        body,
        span: fn_def.span.clone(),
    })
}

fn resolve_block(
    block: &Block,
    fn_table: &HashMap<String, FnId>,
    signatures: &[FnSignature],
    struct_table: &HashMap<String, StructId>,
    vars: &mut HashMap<String, VarId>,
    next_var_id: &mut u32,
) -> Result<ResolvedBlock, ResolveError> {
    let mut stmts = Vec::with_capacity(block.stmts.len());
    for stmt in &block.stmts {
        stmts.push(resolve_stmt(
            stmt,
            fn_table,
            signatures,
            struct_table,
            vars,
            next_var_id,
        )?);
    }
    let tail = resolve_expr(
        &block.tail,
        fn_table,
        signatures,
        struct_table,
        vars,
        next_var_id,
    )?;
    Ok(ResolvedBlock {
        stmts,
        tail,
        span: block.span.clone(),
    })
}

fn resolve_stmt(
    stmt: &Stmt,
    fn_table: &HashMap<String, FnId>,
    signatures: &[FnSignature],
    struct_table: &HashMap<String, StructId>,
    vars: &mut HashMap<String, VarId>,
    next_var_id: &mut u32,
) -> Result<ResolvedStmt, ResolveError> {
    let kind = match &stmt.kind {
        StmtKind::Let { name, name_span, ty_annot, value } => {
            // Resolve the RHS BEFORE binding the name — `let x = x` with
            // `x` undefined in the outer scope is an error, not a self-
            // reference. (Matches C0's existing behaviour: lower_expr on
            // the RHS happens before vars.insert(name).)
            let value = resolve_expr(
                value,
                fn_table,
                signatures,
                struct_table,
                vars,
                next_var_id,
            )?;
            if vars.contains_key(name) {
                return Err(ResolveError::RedeclaredVariable {
                    name: name.clone(),
                    span: to_source_span(name_span),
                });
            }
            let id = VarId(*next_var_id);
            *next_var_id += 1;
            vars.insert(name.clone(), id);
            ResolvedStmtKind::Let {
                id,
                name: name.clone(),
                name_span: name_span.clone(),
                ty_annot: ty_annot.clone(),
                value,
            }
        }
        StmtKind::Expr(e) => ResolvedStmtKind::Expr(resolve_expr(
            e,
            fn_table,
            signatures,
            struct_table,
            vars,
            next_var_id,
        )?),
    };
    Ok(Spanned { kind, span: stmt.span.clone() })
}

fn resolve_expr(
    expr: &Expr,
    fn_table: &HashMap<String, FnId>,
    signatures: &[FnSignature],
    struct_table: &HashMap<String, StructId>,
    vars: &mut HashMap<String, VarId>,
    next_var_id: &mut u32,
) -> Result<ResolvedExpr, ResolveError> {
    let kind = match &expr.kind {
        ExprKind::IntLit(n) => ResolvedExprKind::IntLit(*n),
        ExprKind::BoolLit(b) => ResolvedExprKind::BoolLit(*b),
        ExprKind::NullLit => ResolvedExprKind::NullLit,
        ExprKind::Var(name) => {
            let id =
                *vars
                    .get(name)
                    .ok_or_else(|| ResolveError::UndefinedVariable {
                        name: name.clone(),
                        span: to_source_span(&expr.span),
                    })?;
            ResolvedExprKind::Var(id)
        }
        ExprKind::Unary(op, inner) => ResolvedExprKind::Unary(
            *op,
            Box::new(resolve_expr(
                inner,
                fn_table,
                signatures,
                struct_table,
                vars,
                next_var_id,
            )?),
        ),
        ExprKind::Binary(op, lhs, rhs) => {
            let l = resolve_expr(lhs, fn_table, signatures, struct_table, vars, next_var_id)?;
            let r = resolve_expr(rhs, fn_table, signatures, struct_table, vars, next_var_id)?;
            ResolvedExprKind::Binary(*op, Box::new(l), Box::new(r))
        }
        ExprKind::Cmp(op, lhs, rhs) => {
            let l = resolve_expr(lhs, fn_table, signatures, struct_table, vars, next_var_id)?;
            let r = resolve_expr(rhs, fn_table, signatures, struct_table, vars, next_var_id)?;
            ResolvedExprKind::Cmp(*op, Box::new(l), Box::new(r))
        }
        ExprKind::Logic(op, lhs, rhs) => {
            let l = resolve_expr(lhs, fn_table, signatures, struct_table, vars, next_var_id)?;
            let r = resolve_expr(rhs, fn_table, signatures, struct_table, vars, next_var_id)?;
            ResolvedExprKind::Logic(*op, Box::new(l), Box::new(r))
        }
        ExprKind::Block(b) => ResolvedExprKind::Block(Box::new(resolve_block(
            b,
            fn_table,
            signatures,
            struct_table,
            vars,
            next_var_id,
        )?)),
        ExprKind::If { cond, then_branch, else_branch } => {
            let cond = resolve_expr(cond, fn_table, signatures, struct_table, vars, next_var_id)?;
            let then_b = resolve_block(
                then_branch,
                fn_table,
                signatures,
                struct_table,
                vars,
                next_var_id,
            )?;
            let else_b = resolve_block(
                else_branch,
                fn_table,
                signatures,
                struct_table,
                vars,
                next_var_id,
            )?;
            ResolvedExprKind::If {
                cond: Box::new(cond),
                then_branch: Box::new(then_b),
                else_branch: Box::new(else_b),
            }
        }
        ExprKind::Call { callee, callee_span, args } => {
            let id =
                *fn_table
                    .get(callee)
                    .ok_or_else(|| ResolveError::UndefinedFunction {
                        name: callee.clone(),
                        span: to_source_span(callee_span),
                    })?;
            let signature = &signatures[id.0 as usize];
            if args.len() != signature.arity {
                return Err(ResolveError::ArityMismatch {
                    name: callee.clone(),
                    expected: signature.arity,
                    got: args.len(),
                    span: to_source_span(callee_span),
                });
            }
            let mut resolved_args = Vec::with_capacity(args.len());
            for arg in args {
                resolved_args.push(resolve_expr(
                    arg,
                    fn_table,
                    signatures,
                    struct_table,
                    vars,
                    next_var_id,
                )?);
            }
            ResolvedExprKind::Call {
                id,
                callee_span: callee_span.clone(),
                args: resolved_args,
            }
        }
        ExprKind::StructLit { name, name_span, fields } => {
            let id =
                *struct_table
                    .get(name)
                    .ok_or_else(|| ResolveError::UndefinedStruct {
                        name: name.clone(),
                        span: to_source_span(name_span),
                    })?;
            let mut resolved_fields = Vec::with_capacity(fields.len());
            for fi in fields {
                let value = resolve_expr(
                    &fi.value,
                    fn_table,
                    signatures,
                    struct_table,
                    vars,
                    next_var_id,
                )?;
                resolved_fields.push(ResolvedFieldInit {
                    name: fi.name.clone(),
                    name_span: fi.name_span.clone(),
                    value,
                    span: fi.span.clone(),
                });
            }
            ResolvedExprKind::StructLit {
                id,
                name: name.clone(),
                name_span: name_span.clone(),
                fields: resolved_fields,
            }
        }
        ExprKind::FieldAccess { target, field, field_span } => {
            let target = resolve_expr(
                target,
                fn_table,
                signatures,
                struct_table,
                vars,
                next_var_id,
            )?;
            ResolvedExprKind::FieldAccess {
                target: Box::new(target),
                field: field.clone(),
                field_span: field_span.clone(),
            }
        }
    };
    Ok(Spanned { kind, span: expr.span.clone() })
}

/// Visible to crates that consume `ResolvedProgram` and need a
/// struct-name → [`StructId`] lookup (e.g., sentinel-types resolving
/// type-position identifiers per ADR 0013 D4). Built on demand from
/// `program.structs`.
pub fn struct_name_table(program: &ResolvedProgram) -> HashMap<String, StructId> {
    program
        .structs
        .iter()
        .map(|s| (s.name.clone(), s.id))
        .collect()
}

fn to_source_span(span: &Span) -> miette::SourceSpan {
    (span.start, span.len()).into()
}

// =============================================================================
// Salsa-tracked query
// =============================================================================

/// Salsa-tracked resolve query. Chains on
/// [`sentinel_syntax::parse_query`]: pulls the parsed Program out
/// of the parse query's cached result and resolves it. Returns
/// `None` if either parsing or resolving fails. Diagnostics flow
/// through the salsa accumulator (parse diagnostics from
/// parse_query, resolve diagnostics from here); the driver
/// collects everything via
/// `resolve_query::accumulated::<Diagnostic>(db, file)` and gets
/// the transitively-accumulated set.
#[salsa::tracked(return_ref)]
pub fn resolve_query(db: &dyn SentinelDb, file: SourceFile) -> Option<ResolvedProgram> {
    let program = sentinel_syntax::parse_query(db, file).as_ref()?;
    match resolve(program) {
        Ok(resolved) => Some(resolved),
        Err(err) => {
            resolve_error_to_diagnostic(&err).accumulate(db);
            None
        }
    }
}

fn resolve_error_to_diagnostic(err: &ResolveError) -> Diagnostic {
    let (code, message, span): (&'static str, String, std::ops::Range<usize>) = match err {
        ResolveError::UndefinedVariable { name, span } => (
            "sentinel::resolve::undefined_variable",
            format!("undefined variable `{name}`"),
            span.offset()..(span.offset() + span.len()),
        ),
        ResolveError::RedeclaredVariable { name, span } => (
            "sentinel::resolve::redeclared_variable",
            format!("variable `{name}` is already declared"),
            span.offset()..(span.offset() + span.len()),
        ),
        ResolveError::UndefinedFunction { name, span } => (
            "sentinel::resolve::undefined_function",
            format!("undefined function `{name}`"),
            span.offset()..(span.offset() + span.len()),
        ),
        ResolveError::ArityMismatch { name, expected, got, span } => (
            "sentinel::resolve::arity_mismatch",
            format!("`{name}` takes {expected} argument(s), got {got}"),
            span.offset()..(span.offset() + span.len()),
        ),
        ResolveError::RedefinedFunction { name, span } => (
            "sentinel::resolve::redefined_function",
            format!("function `{name}` is already declared"),
            span.offset()..(span.offset() + span.len()),
        ),
        ResolveError::MissingMain => (
            "sentinel::resolve::missing_main",
            "program has no `main` function".to_string(),
            0..0,
        ),
        ResolveError::RedefinedStruct { name, span } => (
            "sentinel::resolve::redefined_struct",
            format!("struct `{name}` is already declared"),
            span.offset()..(span.offset() + span.len()),
        ),
        ResolveError::UndefinedStruct { name, span } => (
            "sentinel::resolve::undefined_struct",
            format!("undefined struct `{name}`"),
            span.offset()..(span.offset() + span.len()),
        ),
    };
    Diagnostic {
        stage: "resolve",
        severity: Severity::Error,
        code,
        message,
        span,
    }
}

/// Returns the crate name as a sanity-check that the build is wired up.
pub fn crate_name() -> &'static str {
    "sentinel-resolve"
}

#[cfg(test)]
mod tests {
    use super::*;
    use sentinel_syntax::parse;

    fn resolve_ok(src: &str) -> ResolvedProgram {
        let prog = parse(src).expect("parse");
        resolve(&prog).expect("resolve")
    }

    fn resolve_err(src: &str) -> ResolveError {
        let prog = parse(src).expect("parse");
        resolve(&prog).expect_err("expected resolve error")
    }

    #[test]
    fn smoke() {
        assert_eq!(crate_name(), "sentinel-resolve");
    }

    // ----- positive paths -----

    #[test]
    fn resolves_minimal_main() {
        let p = resolve_ok("fn main() -> i64 { 0 }");
        assert_eq!(p.fns.len(), 1);
        assert_eq!(p.main().name, "main");
        assert!(p.main().signature(&p).is_main);
        // FnId(0) = print, FnId(1) = unwrap_or, FnId(2) = is_some,
        // FnId(3) = main (the first user fn).
        assert_eq!(p.main().id, FnId(3));
        assert_eq!(p.fn_signatures[0].name, "print");
        assert!(p.fn_signatures[0].is_runtime);
    }

    #[test]
    fn resolves_let_and_var() {
        let p = resolve_ok("fn main() -> i64 { let x = 5; x }");
        let body = &p.main().body;
        assert_eq!(body.stmts.len(), 1);
        // Let binds VarId(0) (first var in the program).
        match &body.stmts[0].kind {
            ResolvedStmtKind::Let { id, name, .. } => {
                assert_eq!(*id, VarId(0));
                assert_eq!(name, "x");
            }
            other => panic!("expected Let, got {other:?}"),
        }
        match &body.tail.kind {
            ResolvedExprKind::Var(id) => assert_eq!(*id, VarId(0)),
            other => panic!("expected Var, got {other:?}"),
        }
    }

    #[test]
    fn resolves_param_and_use() {
        let p = resolve_ok("fn double(x: i64) -> i64 { x * 2 }\nfn main() -> i64 { double(7) }");
        // double has 1 param → VarId(0)
        let double = &p.fns[0];
        assert_eq!(double.name, "double");
        assert_eq!(double.params.len(), 1);
        assert_eq!(double.params[0].id, VarId(0));
        // Body references VarId(0)
        match &double.body.tail.kind {
            ResolvedExprKind::Binary(_, lhs, _) => match &lhs.kind {
                ResolvedExprKind::Var(id) => assert_eq!(*id, VarId(0)),
                other => panic!("expected Var lhs, got {other:?}"),
            },
            other => panic!("expected Binary, got {other:?}"),
        }
        // FnId(0) = print, FnId(1) = unwrap_or, FnId(2) = is_some,
        // FnId(3) = double (first user fn), FnId(4) = main.
        let main = p.main();
        match &main.body.tail.kind {
            ResolvedExprKind::Call { id, .. } => assert_eq!(*id, FnId(3)),
            other => panic!("expected Call, got {other:?}"),
        }
    }

    #[test]
    fn print_resolves_to_print_fn_id() {
        let p = resolve_ok("fn main() -> i64 { print(42) }");
        match &p.main().body.tail.kind {
            ResolvedExprKind::Call { id, .. } => assert_eq!(*id, PRINT_FN_ID),
            other => panic!("expected Call to print, got {other:?}"),
        }
    }

    #[test]
    fn fn_ids_are_unique_per_program() {
        let p = resolve_ok("fn a() -> i64 { 1 }\nfn b() -> i64 { 2 }\nfn c() -> i64 { 3 }\nfn main() -> i64 { 0 }");
        let mut ids: Vec<FnId> = p.fns.iter().map(|f| f.id).collect();
        ids.sort_by_key(|f| f.0);
        ids.dedup();
        assert_eq!(ids.len(), 4); // all distinct
    }

    #[test]
    fn var_ids_increment_across_functions() {
        // Two fns each binding one param — should get distinct VarIds.
        let p = resolve_ok("fn f(x: i64) -> i64 { x }\nfn g(y: i64) -> i64 { y }\nfn main() -> i64 { f(1) + g(2) }");
        let f = &p.fns[0];
        let g = &p.fns[1];
        assert_ne!(f.params[0].id, g.params[0].id);
    }

    // ----- error paths -----

    #[test]
    fn undefined_variable_errors() {
        let err = resolve_err("fn main() -> i64 { y }");
        assert!(matches!(err, ResolveError::UndefinedVariable { ref name, .. } if name == "y"));
    }

    #[test]
    fn redeclared_variable_errors() {
        let err = resolve_err("fn main() -> i64 { let x = 1; let x = 2; x }");
        assert!(matches!(err, ResolveError::RedeclaredVariable { ref name, .. } if name == "x"));
    }

    #[test]
    fn redeclared_param_errors() {
        let err = resolve_err("fn dup(x: i64, x: i64) -> i64 { x }\nfn main() -> i64 { dup(1, 2) }");
        assert!(matches!(err, ResolveError::RedeclaredVariable { ref name, .. } if name == "x"));
    }

    #[test]
    fn undefined_function_errors() {
        let err = resolve_err("fn main() -> i64 { frobnicate(5) }");
        assert!(matches!(err, ResolveError::UndefinedFunction { ref name, .. } if name == "frobnicate"));
    }

    #[test]
    fn arity_mismatch_errors() {
        let err = resolve_err("fn main() -> i64 { print(1, 2) }");
        assert!(matches!(
            err,
            ResolveError::ArityMismatch { ref name, expected: 1, got: 2, .. } if name == "print"
        ));
    }

    #[test]
    fn arity_mismatch_for_user_fn() {
        let err = resolve_err("fn one(x: i64) -> i64 { x }\nfn main() -> i64 { one(1, 2) }");
        assert!(matches!(
            err,
            ResolveError::ArityMismatch { ref name, expected: 1, got: 2, .. } if name == "one"
        ));
    }

    #[test]
    fn redefined_function_errors() {
        let err = resolve_err("fn dup() -> i64 { 1 }\nfn dup() -> i64 { 2 }\nfn main() -> i64 { 0 }");
        assert!(matches!(err, ResolveError::RedefinedFunction { ref name, .. } if name == "dup"));
    }

    #[test]
    fn user_redefining_print_errors() {
        let err = resolve_err("fn print(x: i64) -> i64 { x }\nfn main() -> i64 { 0 }");
        assert!(matches!(err, ResolveError::RedefinedFunction { ref name, .. } if name == "print"));
    }

    #[test]
    fn missing_main_errors() {
        let err = resolve_err("fn foo() -> i64 { 0 }");
        assert!(matches!(err, ResolveError::MissingMain));
    }

    #[test]
    fn let_x_equals_x_errors_when_outer_x_undefined() {
        // The RHS resolves BEFORE the binding takes effect, so this
        // is "x undefined" not "self-reference."
        let err = resolve_err("fn main() -> i64 { let x = x; x }");
        assert!(matches!(err, ResolveError::UndefinedVariable { ref name, .. } if name == "x"));
    }

    // ----- Salsa query smoke -----

    #[salsa::db]
    #[derive(Default, Clone)]
    struct TestDb {
        storage: salsa::Storage<Self>,
    }

    #[salsa::db]
    impl salsa::Database for TestDb {
        fn salsa_event(&self, _event: &dyn Fn() -> salsa::Event) {}
    }

    #[salsa::db]
    impl SentinelDb for TestDb {}

    #[test]
    fn resolve_query_returns_some_for_valid_source() {
        let db = TestDb::default();
        let file =
            SourceFile::new(&db, "test.sentinel".to_string(), "fn main() -> i64 { 42 }".to_string());
        let result = resolve_query(&db, file);
        assert!(result.is_some());
        let diags = resolve_query::accumulated::<Diagnostic>(&db, file);
        assert!(diags.is_empty(), "expected no diagnostics, got {diags:?}");
    }

    #[test]
    fn resolve_query_emits_diagnostic_on_error() {
        let db = TestDb::default();
        let file = SourceFile::new(
            &db,
            "test.sentinel".to_string(),
            "fn main() -> i64 { undeclared }".to_string(),
        );
        let result = resolve_query(&db, file);
        assert!(result.is_none());
        let diags = resolve_query::accumulated::<Diagnostic>(&db, file);
        assert_eq!(diags.len(), 1);
        assert_eq!(diags[0].stage, "resolve");
        assert_eq!(diags[0].code, "sentinel::resolve::undefined_variable");
    }

    #[test]
    fn resolve_query_propagates_parse_diagnostic_with_parse_stage() {
        // Lex/parse failure → resolve_query short-circuits to None;
        // parse_query's accumulated diagnostic flows transitively.
        let db = TestDb::default();
        let file = SourceFile::new(
            &db,
            "test.sentinel".to_string(),
            "fn main() { (1 + 2 }".to_string(),
        );
        let result = resolve_query(&db, file);
        assert!(result.is_none());
        let diags = resolve_query::accumulated::<Diagnostic>(&db, file);
        assert_eq!(diags.len(), 1);
        assert_eq!(diags[0].stage, "parse");
    }

    // ----- C1.4: struct decls + struct literal + field access -----

    #[test]
    fn resolves_struct_decl() {
        let p = resolve_ok(
            "struct Point { x: i64, y: i64 }\nfn main() -> i64 { 0 }",
        );
        assert_eq!(p.structs.len(), 1);
        assert_eq!(p.structs[0].name, "Point");
        assert_eq!(p.structs[0].id, StructId(0));
        assert_eq!(p.structs[0].fields.len(), 2);
    }

    #[test]
    fn resolves_multiple_struct_decls_in_order() {
        let p = resolve_ok(
            "struct A { x: i64 }\nstruct B { y: i64 }\nfn main() -> i64 { 0 }",
        );
        assert_eq!(p.structs[0].id, StructId(0));
        assert_eq!(p.structs[0].name, "A");
        assert_eq!(p.structs[1].id, StructId(1));
        assert_eq!(p.structs[1].name, "B");
    }

    #[test]
    fn resolves_struct_literal_to_struct_id() {
        let p = resolve_ok(
            "struct Point { x: i64, y: i64 }\nfn main() -> i64 { let p = Point { x: 3, y: 4 }; 0 }",
        );
        let main = p.main();
        match &main.body.stmts[0].kind {
            ResolvedStmtKind::Let { value, .. } => match &value.kind {
                ResolvedExprKind::StructLit { id, name, fields, .. } => {
                    assert_eq!(*id, StructId(0));
                    assert_eq!(name, "Point");
                    assert_eq!(fields.len(), 2);
                }
                other => panic!("expected StructLit, got {other:?}"),
            },
            other => panic!("expected Let, got {other:?}"),
        }
    }

    #[test]
    fn resolves_field_access() {
        let p = resolve_ok(
            "struct P { x: i64 }\nfn main() -> i64 { let p = P { x: 3 }; p.x }",
        );
        let tail = &p.main().body.tail;
        match &tail.kind {
            ResolvedExprKind::FieldAccess { field, target, .. } => {
                assert_eq!(field, "x");
                match &target.kind {
                    ResolvedExprKind::Var(_) => {}
                    other => panic!("expected Var target, got {other:?}"),
                }
            }
            other => panic!("expected FieldAccess, got {other:?}"),
        }
    }

    #[test]
    fn redefined_struct_errors() {
        let err = resolve_err(
            "struct Foo { x: i64 }\nstruct Foo { y: i64 }\nfn main() -> i64 { 0 }",
        );
        assert!(matches!(err, ResolveError::RedefinedStruct { ref name, .. } if name == "Foo"));
    }

    #[test]
    fn undefined_struct_in_literal_errors() {
        let err = resolve_err(
            "fn main() -> i64 { let p = Bogus { x: 1 }; 0 }",
        );
        assert!(matches!(err, ResolveError::UndefinedStruct { ref name, .. } if name == "Bogus"));
    }

    #[test]
    fn struct_name_table_helper_works() {
        let p = resolve_ok(
            "struct A { x: i64 }\nstruct B { y: i64 }\nfn main() -> i64 { 0 }",
        );
        let t = struct_name_table(&p);
        assert_eq!(t["A"], StructId(0));
        assert_eq!(t["B"], StructId(1));
        assert_eq!(t.len(), 2);
    }

    // ----- C1.5: null literal + builtin registration -----

    #[test]
    fn null_literal_resolves() {
        let p = resolve_ok("fn main() -> i64 { let _x = null; 0 }");
        // The Let RHS is a NullLit.
        match &p.main().body.stmts[0].kind {
            ResolvedStmtKind::Let { value, .. } => match &value.kind {
                ResolvedExprKind::NullLit => {}
                other => panic!("expected NullLit, got {other:?}"),
            },
            other => panic!("expected Let, got {other:?}"),
        }
    }

    #[test]
    fn unwrap_or_builtin_pre_registered() {
        let p = resolve_ok("fn main() -> i64 { 0 }");
        // FnId(0) is print, FnId(1) is unwrap_or, FnId(2) is is_some.
        assert_eq!(p.fn_signatures[0].name, "print");
        assert_eq!(p.fn_signatures[1].name, "unwrap_or");
        assert_eq!(p.fn_signatures[1].arity, 2);
        assert!(p.fn_signatures[1].is_runtime);
        assert_eq!(p.fn_signatures[2].name, "is_some");
        assert_eq!(p.fn_signatures[2].arity, 1);
        assert!(p.fn_signatures[2].is_runtime);
    }

    #[test]
    fn user_redefining_unwrap_or_errors() {
        let err = resolve_err(
            "fn unwrap_or(x: i64, y: i64) -> i64 { x }\nfn main() -> i64 { 0 }",
        );
        assert!(matches!(err, ResolveError::RedefinedFunction { ref name, .. } if name == "unwrap_or"));
    }

    #[test]
    fn user_redefining_is_some_errors() {
        let err = resolve_err(
            "fn is_some(x: i64) -> bool { true }\nfn main() -> i64 { 0 }",
        );
        assert!(matches!(err, ResolveError::RedefinedFunction { ref name, .. } if name == "is_some"));
    }

    #[test]
    fn resolve_query_caches_across_reruns() {
        let db = TestDb::default();
        let file = SourceFile::new(
            &db,
            "test.sentinel".to_string(),
            "fn main() -> i64 { 1 + 2 }".to_string(),
        );
        let r1 = resolve_query(&db, file).clone();
        let r2 = resolve_query(&db, file).clone();
        assert_eq!(r1, r2);
        assert!(r1.is_some());
    }
}
