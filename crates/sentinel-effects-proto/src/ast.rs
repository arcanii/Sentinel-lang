//! Abstract syntax tree for Sentinel-Mini (B0).
//!
//! Plain enums, owned `String` names, `Box`ed children. No arena
//! allocation: the B0 language is small enough that the resulting heap
//! traffic is irrelevant for a research artifact.

/// A Sentinel-Mini expression.
#[derive(Debug, Clone, PartialEq)]
pub enum Expr {
    /// Integer literal.
    Int(i64),
    /// Boolean literal.
    Bool(bool),
    /// Variable reference.
    Var(String),
    /// `let name = value in body`.
    Let {
        name: String,
        value: Box<Expr>,
        body: Box<Expr>,
    },
    /// `if cond then then_branch else else_branch`.
    If {
        cond: Box<Expr>,
        then_branch: Box<Expr>,
        else_branch: Box<Expr>,
    },
    /// `fn(param) => body`. B0 supports single-parameter lambdas only;
    /// multi-parameter functions are curried at the call site by the
    /// programmer for now.
    Lambda {
        param: String,
        body: Box<Expr>,
    },
    /// `callee(arg)`. Same single-arg restriction as [`Expr::Lambda`].
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

/// Binary operators recognised at B0.
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
