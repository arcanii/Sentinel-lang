//! sentinel-ast
//!
//! Abstract syntax tree for Sentinel source. C0.1 populated the
//! expression layer (integer literals, arithmetic, parens); C0.3
//! added variable references, let-statements, expression-statements,
//! and a [`Program`] structure of `stmt* tail_expr` (the implicit
//! body of main). C0.4 added [`Block`], `ExprKind::Block`,
//! `ExprKind::If`, and `ExprKind::Call`. **C0.5** restructures the
//! top level: [`Program`] now contains [`FnDef`]s (with an explicit
//! `main` entry point), and the old implicit-main `stmt* tail`
//! form is gone (it lives inside fn bodies now per ADR 0010 D4/D5).
//!
//! `Span` and `Spanned<T>` live here because they straddle the
//! lexer/parser/AST boundary — the lexer produces tokens with spans
//! and the parser assembles them into AST nodes that carry the same
//! span type. Putting them in `sentinel-ast` keeps the pipeline
//! dependency direction clean: syntax depends on ast, not the
//! reverse.

use std::fmt;

/// Byte-range span into the original source text. Half-open: `start..end`.
pub type Span = std::ops::Range<usize>;

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Spanned<T> {
    pub kind: T,
    pub span: Span,
}

/// Binary arithmetic operator. C0.1 supports the four operators in
/// the lexer's punctuation set. Comparison + logical operators get
/// their own enums per ADR 0012 D6/D7 because their typing rules
/// differ (cmp: same → Bool; logic: Bool, Bool → Bool, short-circuit).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum BinOp {
    Add,
    Sub,
    Mul,
    Div,
}

impl BinOp {
    pub fn symbol(self) -> &'static str {
        match self {
            BinOp::Add => "+",
            BinOp::Sub => "-",
            BinOp::Mul => "*",
            BinOp::Div => "/",
        }
    }
}

/// Comparison operator (C1.3, per ADR 0012 D6). Six operators; both
/// operands must be the same numeric type, result is `bool`. Parsed
/// non-associatively (`1 < 2 < 3` is a parse error) per D6.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CmpOp {
    Eq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
}

impl CmpOp {
    pub fn symbol(self) -> &'static str {
        match self {
            CmpOp::Eq => "==",
            CmpOp::Ne => "!=",
            CmpOp::Lt => "<",
            CmpOp::Le => "<=",
            CmpOp::Gt => ">",
            CmpOp::Ge => ">=",
        }
    }
}

/// Logical operator (C1.3, per ADR 0012 D7). Binary, short-circuit;
/// both operands must be `bool`, result is `bool`. Unary `!` is a
/// new [`UnaryOp::Not`] variant alongside [`UnaryOp::Neg`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum LogicOp {
    And,
    Or,
}

impl LogicOp {
    pub fn symbol(self) -> &'static str {
        match self {
            LogicOp::And => "&&",
            LogicOp::Or => "||",
        }
    }
}

/// Unary operator. C0.1 ships only negation per ADR 0010 D8; C1.3
/// adds logical not (`!`) per ADR 0012 D7.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum UnaryOp {
    Neg,
    Not,
}

impl UnaryOp {
    pub fn symbol(self) -> &'static str {
        match self {
            UnaryOp::Neg => "-",
            UnaryOp::Not => "!",
        }
    }
}

/// The expression layer of the AST. Per ADR 0010 D8 the precedence
/// ladder is unary < mul < add; C1.3 (per ADR 0012 D6/D7) widens it
/// to `or < and < cmp < add < mul < unary`. The parser builds this
/// shape directly. Parens are syntactic only and are not represented
/// in the tree — they are recoverable from precedence.
///
/// C0.3 added [`ExprKind::Var`] for variable references. C0.4 adds
/// [`ExprKind::Block`] (brace-wrapped block as expression),
/// [`ExprKind::If`] (mandatory else per ADR 0010 D9, C-style truthy
/// condition; the C-style-truthy retires at C1.3 step 5), and
/// [`ExprKind::Call`] (direct call by identifier per ADR 0010 D10;
/// only `print` resolves to anything in C0.4 since `fn` defs wait
/// for C0.5).
///
/// C1.3 adds [`ExprKind::BoolLit`] (the `true` / `false` literals
/// per ADR 0012 D5), [`ExprKind::Cmp`] (six comparison operators per
/// D6, separate from [`ExprKind::Binary`] because the typing rule
/// differs — same → Bool), and [`ExprKind::Logic`] (`&&` / `||`,
/// short-circuit per D7).
///
/// C1.4 adds [`ExprKind::StructLit`] (`Name { field: expr, … }` per
/// ADR 0013 D3) and [`ExprKind::FieldAccess`] (postfix `expr.field`
/// per D2; binds as part of the atom so `-p.x` is `-(p.x)`).
///
/// C1.5 adds [`ExprKind::NullLit`] — the bare `null` keyword. Its
/// type is `?T` for some T, resolved bidirectionally at type-check
/// time per ADR 0014 D2.
///
/// C1.6 adds [`ExprKind::ArrayLit`] (`[e1, e2, …]` per ADR 0015 D2)
/// and [`ExprKind::Index`] (postfix `a[i]` per D3; binds as part of
/// the postfix chain alongside `.field`).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum ExprKind {
    IntLit(i64),
    BoolLit(bool),
    /// The `null` keyword literal per ADR 0014 D2. The type checker
    /// resolves its `?T` type from the surrounding context.
    NullLit,
    Var(String),
    Unary(UnaryOp, Box<Expr>),
    Binary(BinOp, Box<Expr>, Box<Expr>),
    /// Comparison expression per ADR 0012 D6. Non-associative — the
    /// parser rejects `1 < 2 < 3` at parse time.
    Cmp(CmpOp, Box<Expr>, Box<Expr>),
    /// Logical `&&` / `||` per ADR 0012 D7. Short-circuit semantics
    /// are codegen's concern; the AST shape is the same as any other
    /// binary node.
    Logic(LogicOp, Box<Expr>, Box<Expr>),
    Block(Box<Block>),
    If {
        cond: Box<Expr>,
        then_branch: Box<Block>,
        else_branch: Box<Block>,
    },
    Call {
        callee: String,
        callee_span: Span,
        args: Vec<Expr>,
    },
    /// Struct literal: `Name { field: expr, … }` per ADR 0013 D3.
    /// Field order may differ from the declaration order — the type
    /// checker validates the set, not the order.
    StructLit {
        name: String,
        name_span: Span,
        fields: Vec<FieldInit>,
    },
    /// Postfix field access: `target.field` per ADR 0013 D2. Binds
    /// as part of the atom (`-p.x` parses as `-(p.x)`).
    FieldAccess {
        target: Box<Expr>,
        field: String,
        field_span: Span,
    },
    /// Array literal `[e1, e2, …]` per ADR 0015 D2. Trailing comma
    /// allowed. All elements must type to the same T; the result
    /// is `[T]`.
    ArrayLit(Vec<Expr>),
    /// Postfix indexing `target[index]` per ADR 0015 D3. The target
    /// must type to `[T]` and the index to `i64`; result is `T`.
    /// Bounds-checked at runtime per D10.
    Index {
        target: Box<Expr>,
        index: Box<Expr>,
    },
}

/// A single `field: expr` initializer inside a struct literal. The
/// `name_span` is the span of just the field identifier (useful for
/// "unknown field" diagnostics).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct FieldInit {
    pub name: String,
    pub name_span: Span,
    pub value: Expr,
    /// Span covering the whole `field: expr` initializer.
    pub span: Span,
}

/// Spanned expression. Every AST node carries the byte range it was
/// parsed from for diagnostic purposes.
pub type Expr = Spanned<ExprKind>;

/// Statement kinds. Per ADR 0010 D7 there are exactly two statement
/// shapes in C0: `let x = expr;` and `expr;`. Both are `;`-terminated.
/// C1.2 (ADR 0012 D2) extends `Let` with an optional type annotation
/// (`let x: i64 = expr;`); the annotation is checked against the
/// RHS at type-check time when present, inferred otherwise.
///
/// `Let { name }` stores the bound name as an owned `String` rather
/// than an interned identifier — interning is C1's problem, when
/// name resolution moves into `sentinel-resolve`. `name_span` is
/// the span of just the identifier, useful for redeclaration and
/// undefined-name diagnostics that target the binding site.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum StmtKind {
    Let {
        name: String,
        name_span: Span,
        /// Optional type annotation per ADR 0012 D2. `Some` if the
        /// source had `let x: T = ...`; `None` if it was `let x = ...`
        /// (inference path).
        ty_annot: Option<TypeExpr>,
        value: Expr,
    },
    Expr(Expr),
}

pub type Stmt = Spanned<StmtKind>;

/// A C0.5+ program: one or more function definitions plus zero or
/// more struct declarations (added at C1.4 per ADR 0013 D1). One fn
/// must be named `main` (no parameters) — the entry point. The
/// previous C0.3-0.4 shape (`stmt* tail_expr`) now lives inside fn
/// bodies as [`Block`]s.
///
/// Structs and fns are stored in separate vectors rather than a
/// shared `items` enum — the per-pass downstream code (resolve,
/// types, codegen) needs to walk them in a specific order anyway
/// (struct table built before fn signatures, which are built before
/// fn bodies). Source order within each category is preserved.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Program {
    pub fns: Vec<FnDef>,
    /// Top-level struct declarations per ADR 0013 D1. Always present
    /// (may be empty for C0/C1.3-compatible programs).
    pub structs: Vec<StructDecl>,
    pub span: Span,
}

/// A function definition: `fn name<T1, T2>(p1: T, p2: T, …) -> T { body }`.
/// C0.5 had no annotations and treated everything as `i64`; C1.2
/// (ADR 0012 D1) makes parameter types and the return type
/// mandatory; C1.7 (ADR 0016 D1) adds the optional `<T1, T2>`
/// generic-parameter clause between the name and the parameter
/// list.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct FnDef {
    pub name: String,
    pub name_span: Span,
    /// Generic type-parameter list per ADR 0016 D1. Empty for
    /// non-generic fns; never contains zero entries when the source
    /// wrote `fn f<>` (rejected at parse time as
    /// [`ParseError::EmptyTypeParams`](../sentinel_syntax/parser/enum.ParseError.html)).
    pub type_params: Vec<TypeParam>,
    pub params: Vec<Param>,
    /// Return-type annotation per ADR 0012 D1. Mandatory at C1.2 —
    /// every fn declares its return type explicitly at the boundary.
    pub return_type: TypeExpr,
    pub body: Block,
    pub span: Span,
}

/// A function parameter with a mandatory type annotation
/// (`name: type`) per ADR 0012 D1.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Param {
    pub name: String,
    pub span: Span,
    pub ty: TypeExpr,
}

/// A C1.4+ struct declaration: `struct Name<T1, T2> { field: Type, … }`
/// per ADR 0013 D1. Empty structs (zero fields) are allowed.
/// Trailing comma after the last field is permitted. C1.7 (ADR
/// 0016 D2) adds the optional `<T1, T2>` generic-parameter clause
/// between the name and the body.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct StructDecl {
    pub name: String,
    pub name_span: Span,
    /// Generic type-parameter list per ADR 0016 D2. Empty for
    /// non-generic structs; the parser rejects `struct Foo<>` via
    /// [`ParseError::EmptyTypeParams`](../sentinel_syntax/parser/enum.ParseError.html).
    pub type_params: Vec<TypeParam>,
    pub fields: Vec<StructField>,
    pub span: Span,
}

/// A single generic type parameter (`T`, `U`, …) per ADR 0016 D1
/// / D2. Carries just the name and its span — type-parameter
/// position inside the surrounding fn / struct is determined by
/// the index in the parent's `type_params` vector and assigned a
/// `TypeParamId` at the resolve stage.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct TypeParam {
    pub name: String,
    pub name_span: Span,
}

/// A single named field inside a struct declaration. `name_span`
/// is the span of just the field identifier; `span` covers the
/// whole `name: type` clause.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct StructField {
    pub name: String,
    pub name_span: Span,
    pub ty: TypeExpr,
    pub span: Span,
}

/// Surface-level type expression. C1.2 shipped only the `Ident` form
/// (recognised at type-check time as `i64`, later `i32`/`bool`/struct
/// names per ADR 0012 D3 and D4). C1.5 adds [`TypeExprKind::Nullable`]
/// for `?T` per ADR 0014 D1. The enum stays open-ended so later
/// sub-phases can add `&T` (C2), `secret T` (C3), and generics
/// (C1.7) without churning every annotation site.
pub type TypeExpr = Spanned<TypeExprKind>;

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum TypeExprKind {
    Ident(String),
    /// Postfix-`?` nullable type per ADR 0014 D1. The inner
    /// TypeExpr is the base type. Nested nullables (`??T`) are
    /// rejected at parse time per ADR 0014 D6.
    Nullable(Box<TypeExpr>),
    /// `[T]` array type per ADR 0015 D1. The inner TypeExpr is the
    /// element type. Nested arrays (`[[T]]`) are rejected at the
    /// type-resolution stage per D6 (the parser accepts them; the
    /// type checker rejects).
    Array(Box<TypeExpr>),
    /// `Name<TypeArg1, TypeArg2, ...>` generic instance per ADR
    /// 0016 D3. The parser accepts any non-zero arg list (empty
    /// `<>` is rejected as [`ParseError::EmptyTypeArgs`]); the type
    /// checker validates arity, that `Name` is a generic struct,
    /// etc.
    Generic {
        name: String,
        name_span: Span,
        args: Vec<TypeExpr>,
    },
}

/// A brace-wrapped block expression `{ stmt* tail_expr }`. The
/// trailing expression is the block's value. Used by `if`/`else`
/// branches, standalone `{ ... }` expressions, and function bodies
/// (a function's body is its [`Block`] whose tail expression is
/// the return value).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Block {
    pub stmts: Vec<Stmt>,
    pub tail: Expr,
    pub span: Span,
}

/// S-expression-style pretty-printer for inspection. Useful for
/// `snc parse` output and for debugging.
///
/// Examples:
///   `1`         → `1`
///   `1 + 2`     → `(+ 1 2)`
///   `1 + 2 * 3` → `(+ 1 (* 2 3))`
///   `-x`        → `(- x)` (when x lexes; currently atoms are int literals only)
impl fmt::Display for ExprKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ExprKind::IntLit(n) => write!(f, "{n}"),
            ExprKind::BoolLit(b) => write!(f, "{b}"),
            ExprKind::NullLit => write!(f, "null"),
            ExprKind::Var(name) => write!(f, "{name}"),
            ExprKind::Unary(op, inner) => write!(f, "({} {})", op.symbol(), inner.kind),
            ExprKind::Binary(op, lhs, rhs) => {
                write!(f, "({} {} {})", op.symbol(), lhs.kind, rhs.kind)
            }
            ExprKind::Cmp(op, lhs, rhs) => {
                write!(f, "({} {} {})", op.symbol(), lhs.kind, rhs.kind)
            }
            ExprKind::Logic(op, lhs, rhs) => {
                write!(f, "({} {} {})", op.symbol(), lhs.kind, rhs.kind)
            }
            ExprKind::Block(b) => b.fmt(f),
            ExprKind::If { cond, then_branch, else_branch } => {
                write!(f, "(if {} {} {})", cond.kind, then_branch, else_branch)
            }
            ExprKind::Call { callee, args, .. } => {
                write!(f, "({callee}")?;
                for arg in args {
                    write!(f, " {}", arg.kind)?;
                }
                write!(f, ")")
            }
            ExprKind::StructLit { name, fields, .. } => {
                write!(f, "(struct-lit {name}")?;
                for fi in fields {
                    write!(f, " ({} {})", fi.name, fi.value.kind)?;
                }
                write!(f, ")")
            }
            ExprKind::FieldAccess { target, field, .. } => {
                write!(f, "(. {} {})", target.kind, field)
            }
            ExprKind::ArrayLit(elems) => {
                write!(f, "(array")?;
                for e in elems {
                    write!(f, " {}", e.kind)?;
                }
                write!(f, ")")
            }
            ExprKind::Index { target, index } => {
                write!(f, "(index {} {})", target.kind, index.kind)
            }
        }
    }
}

impl fmt::Display for Block {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "(block")?;
        for stmt in &self.stmts {
            write!(f, " {}", stmt.kind)?;
        }
        write!(f, " {})", self.tail.kind)
    }
}

impl fmt::Display for Expr {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.kind.fmt(f)
    }
}

impl fmt::Display for StmtKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            StmtKind::Let { name, ty_annot, value, .. } => match ty_annot {
                Some(ty) => write!(f, "(let {name}: {} {})", ty.kind, value.kind),
                None => write!(f, "(let {name} {})", value.kind),
            },
            StmtKind::Expr(e) => write!(f, "{};", e.kind),
        }
    }
}

impl fmt::Display for TypeExprKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            TypeExprKind::Ident(name) => write!(f, "{name}"),
            TypeExprKind::Nullable(inner) => write!(f, "?{}", inner.kind),
            TypeExprKind::Array(inner) => write!(f, "[{}]", inner.kind),
            TypeExprKind::Generic { name, args, .. } => {
                write!(f, "{name}<")?;
                let mut first = true;
                for a in args {
                    if !first {
                        write!(f, ", ")?;
                    }
                    first = false;
                    write!(f, "{}", a.kind)?;
                }
                write!(f, ">")
            }
        }
    }
}

impl fmt::Display for TypeExpr {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.kind.fmt(f)
    }
}

impl fmt::Display for Stmt {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.kind.fmt(f)
    }
}

/// Program prints each top-level item on its own line. Structs
/// come first (declaration order), then fns. Each is rendered by
/// its own Display impl.
impl fmt::Display for Program {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut first = true;
        for s in &self.structs {
            if !first {
                writeln!(f)?;
            }
            first = false;
            write!(f, "{s}")?;
        }
        for fn_def in &self.fns {
            if !first {
                writeln!(f)?;
            }
            first = false;
            write!(f, "{fn_def}")?;
        }
        Ok(())
    }
}

/// `(struct Name (field_name: type) (field_name: type) ...)`.
/// Empty struct renders as `(struct Name)`.
impl fmt::Display for StructDecl {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "(struct {}", self.name)?;
        for field in &self.fields {
            write!(f, " ({}: {})", field.name, field.ty.kind)?;
        }
        write!(f, ")")
    }
}

/// `(fn name (params) -> return_type body)` — params is a space-
/// separated list of `name: type` pairs (or just names for C0
/// backwards-compatible rendering, but C1.2 onward always carries
/// the annotation). Body delegates to [`Block`].
impl fmt::Display for FnDef {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "(fn {}", self.name)?;
        if !self.type_params.is_empty() {
            write!(f, "<")?;
            let mut first = true;
            for tp in &self.type_params {
                if !first {
                    write!(f, ", ")?;
                }
                first = false;
                write!(f, "{}", tp.name)?;
            }
            write!(f, ">")?;
        }
        write!(f, " (")?;
        let mut first = true;
        for p in &self.params {
            if !first {
                write!(f, " ")?;
            }
            first = false;
            write!(f, "{}: {}", p.name, p.ty.kind)?;
        }
        write!(f, ") -> {} {})", self.return_type.kind, self.body)
    }
}

/// Returns the crate name as a sanity-check that the build is wired up.
pub fn crate_name() -> &'static str {
    "sentinel-ast"
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lit(n: i64, span: Span) -> Expr {
        Spanned { kind: ExprKind::IntLit(n), span }
    }

    /// Test helper: an `i64` type annotation at the given span.
    fn ty_i64(span: Span) -> TypeExpr {
        Spanned { kind: TypeExprKind::Ident("i64".to_string()), span }
    }

    #[test]
    fn smoke() {
        assert_eq!(crate_name(), "sentinel-ast");
    }

    #[test]
    fn display_int_lit() {
        assert_eq!(lit(42, 0..2).to_string(), "42");
    }

    #[test]
    fn display_binary() {
        let e = Spanned {
            kind: ExprKind::Binary(BinOp::Add, Box::new(lit(1, 0..1)), Box::new(lit(2, 4..5))),
            span: 0..5,
        };
        assert_eq!(e.to_string(), "(+ 1 2)");
    }

    #[test]
    fn display_nested_precedence() {
        // (+ 1 (* 2 3))
        let two_times_three = Spanned {
            kind: ExprKind::Binary(BinOp::Mul, Box::new(lit(2, 4..5)), Box::new(lit(3, 8..9))),
            span: 4..9,
        };
        let e = Spanned {
            kind: ExprKind::Binary(BinOp::Add, Box::new(lit(1, 0..1)), Box::new(two_times_three)),
            span: 0..9,
        };
        assert_eq!(e.to_string(), "(+ 1 (* 2 3))");
    }

    #[test]
    fn display_unary() {
        let e = Spanned {
            kind: ExprKind::Unary(UnaryOp::Neg, Box::new(lit(5, 1..2))),
            span: 0..2,
        };
        assert_eq!(e.to_string(), "(- 5)");
    }

    #[test]
    fn binop_symbols() {
        assert_eq!(BinOp::Add.symbol(), "+");
        assert_eq!(BinOp::Sub.symbol(), "-");
        assert_eq!(BinOp::Mul.symbol(), "*");
        assert_eq!(BinOp::Div.symbol(), "/");
    }

    #[test]
    fn cmpop_symbols() {
        assert_eq!(CmpOp::Eq.symbol(), "==");
        assert_eq!(CmpOp::Ne.symbol(), "!=");
        assert_eq!(CmpOp::Lt.symbol(), "<");
        assert_eq!(CmpOp::Le.symbol(), "<=");
        assert_eq!(CmpOp::Gt.symbol(), ">");
        assert_eq!(CmpOp::Ge.symbol(), ">=");
    }

    #[test]
    fn logicop_symbols() {
        assert_eq!(LogicOp::And.symbol(), "&&");
        assert_eq!(LogicOp::Or.symbol(), "||");
    }

    #[test]
    fn unaryop_symbols() {
        assert_eq!(UnaryOp::Neg.symbol(), "-");
        assert_eq!(UnaryOp::Not.symbol(), "!");
    }

    #[test]
    fn display_bool_lit() {
        let t = Spanned { kind: ExprKind::BoolLit(true), span: 0..4 };
        let f = Spanned { kind: ExprKind::BoolLit(false), span: 0..5 };
        assert_eq!(t.to_string(), "true");
        assert_eq!(f.to_string(), "false");
    }

    #[test]
    fn display_cmp_eq() {
        let e = Spanned {
            kind: ExprKind::Cmp(CmpOp::Eq, Box::new(lit(1, 0..1)), Box::new(lit(2, 5..6))),
            span: 0..6,
        };
        assert_eq!(e.to_string(), "(== 1 2)");
    }

    #[test]
    fn display_logic_and() {
        let lhs = Spanned { kind: ExprKind::BoolLit(true), span: 0..4 };
        let rhs = Spanned { kind: ExprKind::BoolLit(false), span: 8..13 };
        let e = Spanned {
            kind: ExprKind::Logic(LogicOp::And, Box::new(lhs), Box::new(rhs)),
            span: 0..13,
        };
        assert_eq!(e.to_string(), "(&& true false)");
    }

    #[test]
    fn display_unary_not() {
        let inner = Spanned { kind: ExprKind::BoolLit(true), span: 1..5 };
        let e = Spanned {
            kind: ExprKind::Unary(UnaryOp::Not, Box::new(inner)),
            span: 0..5,
        };
        assert_eq!(e.to_string(), "(! true)");
    }

    #[test]
    fn display_var() {
        let e = Spanned { kind: ExprKind::Var("x".to_string()), span: 0..1 };
        assert_eq!(e.to_string(), "x");
    }

    #[test]
    fn display_stmt_let_no_annotation() {
        let s = Spanned {
            kind: StmtKind::Let {
                name: "x".to_string(),
                name_span: 4..5,
                ty_annot: None,
                value: lit(5, 8..9),
            },
            span: 0..10,
        };
        assert_eq!(s.to_string(), "(let x 5)");
    }

    #[test]
    fn display_stmt_let_with_annotation() {
        let s = Spanned {
            kind: StmtKind::Let {
                name: "x".to_string(),
                name_span: 4..5,
                ty_annot: Some(ty_i64(7..10)),
                value: lit(5, 13..14),
            },
            span: 0..15,
        };
        assert_eq!(s.to_string(), "(let x: i64 5)");
    }

    #[test]
    fn display_type_expr_ident() {
        assert_eq!(ty_i64(0..3).to_string(), "i64");
    }

    #[test]
    fn display_stmt_expr() {
        let s = Spanned {
            kind: StmtKind::Expr(lit(42, 0..2)),
            span: 0..3,
        };
        assert_eq!(s.to_string(), "42;");
    }

    fn main_fn(tail: Expr) -> FnDef {
        let body_span = tail.span.clone();
        FnDef {
            name: "main".to_string(),
            name_span: 0..4,
            type_params: vec![],
            params: vec![],
            return_type: ty_i64(0..3),
            body: Block { stmts: vec![], tail, span: body_span.clone() },
            span: 0..body_span.end,
        }
    }

    #[test]
    fn display_program_one_main() {
        let p = Program {
            fns: vec![main_fn(lit(42, 0..2))],
            structs: vec![],
            span: 0..2,
        };
        assert_eq!(p.to_string(), "(fn main () -> i64 (block 42))");
    }

    #[test]
    fn display_program_two_fns() {
        let double = FnDef {
            name: "double".to_string(),
            name_span: 0..6,
            type_params: vec![],
            params: vec![Param {
                name: "x".to_string(),
                span: 0..1,
                ty: ty_i64(0..3),
            }],
            return_type: ty_i64(0..3),
            body: Block {
                stmts: vec![],
                tail: Spanned {
                    kind: ExprKind::Binary(
                        BinOp::Mul,
                        Box::new(Spanned { kind: ExprKind::Var("x".to_string()), span: 0..1 }),
                        Box::new(lit(2, 0..1)),
                    ),
                    span: 0..5,
                },
                span: 0..5,
            },
            span: 0..10,
        };
        let main = main_fn(Spanned {
            kind: ExprKind::Call {
                callee: "double".to_string(),
                callee_span: 0..6,
                args: vec![lit(7, 0..1)],
            },
            span: 0..9,
        });
        let p = Program { fns: vec![double, main], structs: vec![], span: 0..20 };
        assert_eq!(
            p.to_string(),
            "(fn double (x: i64) -> i64 (block (* x 2)))\n(fn main () -> i64 (block (double 7)))"
        );
    }

    #[test]
    fn display_fn_def_zero_params() {
        let f = main_fn(lit(5, 0..1));
        assert_eq!(f.to_string(), "(fn main () -> i64 (block 5))");
    }

    #[test]
    fn display_fn_def_multi_params() {
        let f = FnDef {
            name: "add".to_string(),
            name_span: 0..3,
            type_params: vec![],
            params: vec![
                Param { name: "a".to_string(), span: 0..1, ty: ty_i64(0..3) },
                Param { name: "b".to_string(), span: 0..1, ty: ty_i64(0..3) },
            ],
            return_type: ty_i64(0..3),
            body: Block {
                stmts: vec![],
                tail: Spanned {
                    kind: ExprKind::Binary(
                        BinOp::Add,
                        Box::new(Spanned { kind: ExprKind::Var("a".to_string()), span: 0..1 }),
                        Box::new(Spanned { kind: ExprKind::Var("b".to_string()), span: 0..1 }),
                    ),
                    span: 0..5,
                },
                span: 0..5,
            },
            span: 0..10,
        };
        assert_eq!(f.to_string(), "(fn add (a: i64 b: i64) -> i64 (block (+ a b)))");
    }

    fn block_lit(n: i64, span: Span) -> Block {
        Block { stmts: vec![], tail: lit(n, span.clone()), span }
    }

    #[test]
    fn display_block_simple() {
        let b = block_lit(7, 0..1);
        assert_eq!(b.to_string(), "(block 7)");
    }

    #[test]
    fn display_block_with_stmt() {
        let let_x = Spanned {
            kind: StmtKind::Let {
                name: "x".to_string(),
                name_span: 6..7,
                ty_annot: None,
                value: lit(1, 10..11),
            },
            span: 2..12,
        };
        let tail = Spanned { kind: ExprKind::Var("x".to_string()), span: 13..14 };
        let b = Block { stmts: vec![let_x], tail, span: 0..16 };
        assert_eq!(b.to_string(), "(block (let x 1) x)");
    }

    #[test]
    fn display_expr_block() {
        let inner = Box::new(block_lit(42, 1..3));
        let e = Spanned { kind: ExprKind::Block(inner), span: 0..4 };
        assert_eq!(e.to_string(), "(block 42)");
    }

    #[test]
    fn display_if() {
        let cond = Box::new(lit(1, 3..4));
        let then_b = Box::new(block_lit(2, 5..8));
        let else_b = Box::new(block_lit(3, 14..17));
        let e = Spanned {
            kind: ExprKind::If { cond, then_branch: then_b, else_branch: else_b },
            span: 0..18,
        };
        assert_eq!(e.to_string(), "(if 1 (block 2) (block 3))");
    }

    #[test]
    fn display_call_zero_args() {
        let e = Spanned {
            kind: ExprKind::Call {
                callee: "print".to_string(),
                callee_span: 0..5,
                args: vec![],
            },
            span: 0..7,
        };
        assert_eq!(e.to_string(), "(print)");
    }

    #[test]
    fn display_call_one_arg() {
        let e = Spanned {
            kind: ExprKind::Call {
                callee: "print".to_string(),
                callee_span: 0..5,
                args: vec![lit(42, 6..8)],
            },
            span: 0..9,
        };
        assert_eq!(e.to_string(), "(print 42)");
    }

    #[test]
    fn display_call_two_args() {
        let e = Spanned {
            kind: ExprKind::Call {
                callee: "f".to_string(),
                callee_span: 0..1,
                args: vec![lit(1, 2..3), lit(2, 5..6)],
            },
            span: 0..7,
        };
        assert_eq!(e.to_string(), "(f 1 2)");
    }

    // ----- C1.4: struct decl, struct literal, field access -----

    #[test]
    fn display_struct_decl_with_two_fields() {
        let s = StructDecl {
            name: "Point".to_string(),
            name_span: 7..12,
            type_params: vec![],
            fields: vec![
                StructField {
                    name: "x".to_string(),
                    name_span: 15..16,
                    ty: ty_i64(18..21),
                    span: 15..21,
                },
                StructField {
                    name: "y".to_string(),
                    name_span: 23..24,
                    ty: ty_i64(26..29),
                    span: 23..29,
                },
            ],
            span: 0..31,
        };
        assert_eq!(s.to_string(), "(struct Point (x: i64) (y: i64))");
    }

    #[test]
    fn display_struct_decl_empty() {
        let s = StructDecl {
            name: "Empty".to_string(),
            name_span: 7..12,
            type_params: vec![],
            fields: vec![],
            span: 0..14,
        };
        assert_eq!(s.to_string(), "(struct Empty)");
    }

    #[test]
    fn display_struct_literal() {
        let e = Spanned {
            kind: ExprKind::StructLit {
                name: "Point".to_string(),
                name_span: 0..5,
                fields: vec![
                    FieldInit {
                        name: "x".to_string(),
                        name_span: 8..9,
                        value: lit(3, 11..12),
                        span: 8..12,
                    },
                    FieldInit {
                        name: "y".to_string(),
                        name_span: 14..15,
                        value: lit(4, 17..18),
                        span: 14..18,
                    },
                ],
            },
            span: 0..20,
        };
        assert_eq!(e.to_string(), "(struct-lit Point (x 3) (y 4))");
    }

    #[test]
    fn display_field_access() {
        let target =
            Box::new(Spanned { kind: ExprKind::Var("p".to_string()), span: 0..1 });
        let e = Spanned {
            kind: ExprKind::FieldAccess {
                target,
                field: "x".to_string(),
                field_span: 2..3,
            },
            span: 0..3,
        };
        assert_eq!(e.to_string(), "(. p x)");
    }

    #[test]
    fn display_field_access_chained() {
        // a.b.c -> (. (. a b) c)
        let a = Spanned { kind: ExprKind::Var("a".to_string()), span: 0..1 };
        let a_dot_b = Spanned {
            kind: ExprKind::FieldAccess {
                target: Box::new(a),
                field: "b".to_string(),
                field_span: 2..3,
            },
            span: 0..3,
        };
        let e = Spanned {
            kind: ExprKind::FieldAccess {
                target: Box::new(a_dot_b),
                field: "c".to_string(),
                field_span: 4..5,
            },
            span: 0..5,
        };
        assert_eq!(e.to_string(), "(. (. a b) c)");
    }

    // ----- C1.5: NullLit + Nullable TypeExpr -----

    #[test]
    fn display_null_lit() {
        let e = Spanned { kind: ExprKind::NullLit, span: 0..4 };
        assert_eq!(e.to_string(), "null");
    }

    #[test]
    fn display_nullable_type() {
        let inner = ty_i64(0..3);
        let ne = Spanned {
            kind: TypeExprKind::Nullable(Box::new(inner)),
            span: 0..4,
        };
        assert_eq!(ne.kind.to_string(), "?i64");
    }

    // ----- C1.6: array literal + indexing + array type -----

    #[test]
    fn display_array_literal() {
        let e = Spanned {
            kind: ExprKind::ArrayLit(vec![
                lit(1, 1..2),
                lit(2, 4..5),
                lit(3, 7..8),
            ]),
            span: 0..9,
        };
        assert_eq!(e.to_string(), "(array 1 2 3)");
    }

    #[test]
    fn display_array_literal_empty() {
        let e = Spanned { kind: ExprKind::ArrayLit(vec![]), span: 0..2 };
        assert_eq!(e.to_string(), "(array)");
    }

    #[test]
    fn display_index() {
        let target = Box::new(Spanned { kind: ExprKind::Var("a".to_string()), span: 0..1 });
        let index = Box::new(lit(0, 2..3));
        let e = Spanned {
            kind: ExprKind::Index { target, index },
            span: 0..4,
        };
        assert_eq!(e.to_string(), "(index a 0)");
    }

    #[test]
    fn display_array_type() {
        let inner = ty_i64(1..4);
        let arr = Spanned {
            kind: TypeExprKind::Array(Box::new(inner)),
            span: 0..5,
        };
        assert_eq!(arr.kind.to_string(), "[i64]");
    }

    #[test]
    fn display_nullable_struct_type() {
        let inner = Spanned {
            kind: TypeExprKind::Ident("Point".to_string()),
            span: 0..5,
        };
        let ne = Spanned {
            kind: TypeExprKind::Nullable(Box::new(inner)),
            span: 0..6,
        };
        assert_eq!(ne.kind.to_string(), "?Point");
    }

    #[test]
    fn display_program_with_struct_and_fn() {
        let s = StructDecl {
            name: "P".to_string(),
            name_span: 7..8,
            type_params: vec![],
            fields: vec![StructField {
                name: "x".to_string(),
                name_span: 11..12,
                ty: ty_i64(14..17),
                span: 11..17,
            }],
            span: 0..19,
        };
        let p = Program {
            fns: vec![main_fn(lit(7, 0..1))],
            structs: vec![s],
            span: 0..30,
        };
        // Structs first, then fns.
        assert_eq!(
            p.to_string(),
            "(struct P (x: i64))\n(fn main () -> i64 (block 7))"
        );
    }
}
