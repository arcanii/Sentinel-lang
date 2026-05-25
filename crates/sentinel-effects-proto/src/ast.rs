//! Abstract syntax tree for Sentinel-Mini.
//!
//! Every expression node carries a [`Span`] via the [`Spanned`] wrapper.
//! The split between [`ExprKind`] (the variants) and [`Expr`] (the
//! span-wrapped node) keeps spans in exactly one place.

use crate::span::{Span, Spanned};

/// A Sentinel-Mini expression: the kind plus its source span.
pub type Expr = Spanned<ExprKind>;

/// The shape of an expression, without the span.
#[derive(Debug, Clone, PartialEq)]
pub enum ExprKind {
    /// Integer literal.
    Int(i64),
    /// Boolean literal.
    Bool(bool),
    /// Variable reference.
    Var(String),
    /// `let name = value in body`. Non-recursive.
    Let {
        name: String,
        value: Box<Expr>,
        body: Box<Expr>,
    },
    /// `let rec name = fn(param) => ... in body`.
    ///
    /// The right-hand side is required at parse time to be a [`Lambda`]
    /// expression. This is the standard ML restriction; it sidesteps the
    /// question of "what does `let rec x = x + 1` mean" without losing
    /// expressiveness, since the recursion we care about is function
    /// recursion.
    ///
    /// [`Lambda`]: ExprKind::Lambda
    LetRec {
        name: String,
        /// The lambda being bound. Always an [`ExprKind::Lambda`] at the
        /// node level; the parser enforces this.
        value: Box<Expr>,
        body: Box<Expr>,
    },
    /// `if cond then then_branch else else_branch`.
    If {
        cond: Box<Expr>,
        then_branch: Box<Expr>,
        else_branch: Box<Expr>,
    },
    /// `fn(param) => body`.
    Lambda {
        param: String,
        body: Box<Expr>,
    },
    /// `callee(arg)`.
    App {
        callee: Box<Expr>,
        arg: Box<Expr>,
    },
    /// Binary arithmetic / comparison.
    BinOp {
        op: BinOp,
        lhs: Box<Expr>,
        rhs: Box<Expr>,
    },
    /// `do <Label>(arg)` -- perform an effect operation.
    ///
    /// B2.2a: parser/AST only. Inference and eval reject this with a
    /// dedicated `EffectNotYetSupported` error in B2.2b; handlers and
    /// real semantics arrive in B2.3 / B3.
    Perform {
        label: String,
        label_span: Span,
        arg: Box<Expr>,
    },
    /// `handle body with { L(x, k) => arm_body, ..., return v => ret_body }`.
    ///
    /// B3.1: typed per ADR 0007 D3. Runtime is still a placeholder
    /// (`EvalError::HandlersNotYetSupported`) until B3.2 lands the
    /// operation-reification model (D5).
    Handle {
        body: Box<Expr>,
        arms: Vec<HandlerArm>,
        ret_arm: Option<ReturnArm>,
    },
    /// `declassify(e)` (ADR 0008 D5). Special form: lowers a
    /// `secret T`-typed expression to `T` at the type level;
    /// eval is identity on the value. The audit-point property
    /// is preserved by keeping this a syntactic form rather than
    /// a function, so every declassification is grep-able in source.
    Declassify {
        inner: Box<Expr>,
        span: Span,
    },
}

/// Binary operators.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BinOp {
    Add,
    Sub,
    Mul,
    Div,
    Eq,
    Lt,
    Gt,
}

/// One arm of a handler: `L(x, k) => body`.
///
/// `label` is the effect operation handled. `arg` and `kont` are the
/// names bound for the operation argument and the resumption
/// continuation respectively. `body` is checked under the outer row
/// (see ADR 0007 D3).
#[derive(Debug, Clone, PartialEq)]
pub struct HandlerArm {
    pub label: String,
    pub label_span: Span,
    pub arg: String,
    pub kont: String,
    pub body: Box<Expr>,
    pub span: Span,
}

/// Optional `return v => body` arm of a handler.
///
/// When omitted at the source level, defaults semantically to
/// `return v => v` (see ADR 0007 D1).
#[derive(Debug, Clone, PartialEq)]
pub struct ReturnArm {
    pub var: String,
    pub body: Box<Expr>,
    pub span: Span,
}

/// Convenience constructor: wrap an [`ExprKind`] with a [`Span`].
#[inline]
pub fn expr(kind: ExprKind, span: Span) -> Expr {
    Spanned::new(kind, span)
}


// ---- B2.2a: program-level AST ----

/// A surface-level type expression, as written in `effect` declarations.
///
/// Kept distinct from the inference-time [`crate::Ty`] so the parser does
/// not depend on the type system and so future surface extensions (rows,
/// qualifiers) live here without churning `types.rs`.
#[derive(Debug, Clone, PartialEq)]
pub enum TyExpr {
    Int(Span),
    Bool(Span),
    Arrow(Box<TyExpr>, Box<TyExpr>, Span),
    /// `secret T` qualifier (ADR 0008 D1/D6). Parser produces this
    /// from the prefix `secret` keyword in TyExpr position;
    /// `to_ty` lowers to [`Ty::secret`].
    Secret(Box<TyExpr>, Span),
}

impl TyExpr {
    pub fn span(&self) -> Span {
        match self {
            TyExpr::Int(s) | TyExpr::Bool(s) => *s,
            TyExpr::Arrow(_, _, s) => *s,
            TyExpr::Secret(_, s) => *s,
        }
    }

    /// B2.3b2: convert a surface `TyExpr` (from an `effect` decl) into
    /// a core `Ty`. Arrows use `Row::Empty` since effect-decl arrow
    /// annotations describe pure handler shapes at B2 scope; B3 will
    /// revisit when handlers can declare residual effects on their
    /// return arrows.
    pub fn to_ty(&self) -> crate::types::Ty {
        use crate::types::{Row, Ty};
        match self {
            TyExpr::Int(_) => Ty::Int,
            TyExpr::Bool(_) => Ty::Bool,
            TyExpr::Arrow(a, b, _) => Ty::arrow_with(a.to_ty(), Row::Empty, b.to_ty()),
            // ADR 0008 D1/D6: prefix `secret T` lowers to
            // Ty::secret(inner). Idempotent smart constructor
            // collapses any accidental double-wrap (the parser
            // separately rejects literal `secret secret T` via
            // ParseError::DoubleSecret).
            TyExpr::Secret(inner, _) => Ty::secret(inner.to_ty()),
        }
    }
}

/// A single `effect Label : ArgTy -> RetTy ;` declaration.
#[derive(Debug, Clone, PartialEq)]
pub struct EffectDecl {
    pub label: String,
    pub label_span: Span,
    pub arg: TyExpr,
    pub ret: TyExpr,
    pub span: Span,
}

/// A whole Sentinel-Mini program: zero or more effect declarations
/// followed by a single body expression.
#[derive(Debug, Clone, PartialEq)]
pub struct Program {
    pub effects: Vec<EffectDecl>,
    pub body: Expr,
}
