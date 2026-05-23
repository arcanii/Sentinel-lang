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

/// Convenience constructor: wrap an [`ExprKind`] with a [`Span`].
#[inline]
pub fn expr(kind: ExprKind, span: Span) -> Expr {
    Spanned::new(kind, span)
}
