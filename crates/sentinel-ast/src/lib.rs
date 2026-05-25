//! sentinel-ast
//!
//! Abstract syntax tree for Sentinel source. C0.1 populated the
//! expression layer (integer literals, arithmetic, parens); C0.3
//! adds variable references, let-statements, expression-statements,
//! and the program-level [`Program`] structure (a sequence of
//! statements followed by a trailing expression — the implicit body
//! of `main` until C0.5 introduces explicit `fn` syntax). Blocks
//! as nested expressions wait for C0.4 when `if`/`else` needs them.
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

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Spanned<T> {
    pub kind: T,
    pub span: Span,
}

/// Binary arithmetic operator. C0.1 supports the four operators in
/// the lexer's punctuation set.
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

/// Unary operator. C0.1 ships only negation per ADR 0010 D8.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum UnaryOp {
    Neg,
}

impl UnaryOp {
    pub fn symbol(self) -> &'static str {
        match self {
            UnaryOp::Neg => "-",
        }
    }
}

/// The expression layer of the AST. Per ADR 0010 D8 the precedence
/// ladder is unary < mul < add; the parser builds this shape
/// directly. Parens are syntactic only and are not represented in
/// the tree — they are recoverable from precedence.
///
/// C0.3 adds [`ExprKind::Var`] for variable references; the parser
/// recognises any bare identifier in atom position as a variable
/// reference. Name resolution (catching undefined names) currently
/// happens in `sentinel-codegen` because `sentinel-resolve` is
/// deferred to C1 per ADR 0009 D7.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExprKind {
    IntLit(i64),
    Var(String),
    Unary(UnaryOp, Box<Expr>),
    Binary(BinOp, Box<Expr>, Box<Expr>),
}

/// Spanned expression. Every AST node carries the byte range it was
/// parsed from for diagnostic purposes.
pub type Expr = Spanned<ExprKind>;

/// Statement kinds. Per ADR 0010 D7 there are exactly two statement
/// shapes in C0: `let x = expr;` and `expr;`. Both are `;`-terminated.
///
/// `Let { name }` stores the bound name as an owned `String` rather
/// than an interned identifier — interning is C1's problem, when
/// name resolution moves into `sentinel-resolve`. `name_span` is
/// the span of just the identifier, useful for redeclaration and
/// undefined-name diagnostics that target the binding site.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StmtKind {
    Let { name: String, name_span: Span, value: Expr },
    Expr(Expr),
}

pub type Stmt = Spanned<StmtKind>;

/// A C0.3+ program: zero or more statements followed by a trailing
/// expression. The trailing expression is the program's value
/// (returned as the exit code in C0.2-0.4, replaced by `print` at
/// C0.4 per ADR 0010 D11). At C0.5 this shape moves inside `fn
/// main() { ... }`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Program {
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
            ExprKind::Var(name) => write!(f, "{name}"),
            ExprKind::Unary(op, inner) => write!(f, "({} {})", op.symbol(), inner.kind),
            ExprKind::Binary(op, lhs, rhs) => {
                write!(f, "({} {} {})", op.symbol(), lhs.kind, rhs.kind)
            }
        }
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
            StmtKind::Let { name, value, .. } => write!(f, "(let {name} {})", value.kind),
            StmtKind::Expr(e) => write!(f, "{};", e.kind),
        }
    }
}

impl fmt::Display for Stmt {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.kind.fmt(f)
    }
}

/// Program prints statements one per line, then the trailing
/// expression. Pure expression programs (no statements) print as
/// just the expression — preserving the C0.1/C0.2 pretty-print
/// output verbatim for backward compatibility with their tests.
impl fmt::Display for Program {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for stmt in &self.stmts {
            writeln!(f, "{}", stmt.kind)?;
        }
        write!(f, "{}", self.tail.kind)
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
    fn unaryop_symbols() {
        assert_eq!(UnaryOp::Neg.symbol(), "-");
    }

    #[test]
    fn display_var() {
        let e = Spanned { kind: ExprKind::Var("x".to_string()), span: 0..1 };
        assert_eq!(e.to_string(), "x");
    }

    #[test]
    fn display_stmt_let() {
        let s = Spanned {
            kind: StmtKind::Let {
                name: "x".to_string(),
                name_span: 4..5,
                value: lit(5, 8..9),
            },
            span: 0..10,
        };
        assert_eq!(s.to_string(), "(let x 5)");
    }

    #[test]
    fn display_stmt_expr() {
        let s = Spanned {
            kind: StmtKind::Expr(lit(42, 0..2)),
            span: 0..3,
        };
        assert_eq!(s.to_string(), "42;");
    }

    #[test]
    fn display_program_empty_stmts_prints_just_tail() {
        let p = Program {
            stmts: vec![],
            tail: lit(42, 0..2),
            span: 0..2,
        };
        assert_eq!(p.to_string(), "42");
    }

    #[test]
    fn display_program_with_stmts() {
        let let_x = Spanned {
            kind: StmtKind::Let {
                name: "x".to_string(),
                name_span: 4..5,
                value: lit(1, 8..9),
            },
            span: 0..10,
        };
        let tail = Spanned {
            kind: ExprKind::Var("x".to_string()),
            span: 11..12,
        };
        let p = Program { stmts: vec![let_x], tail, span: 0..12 };
        assert_eq!(p.to_string(), "(let x 1)\nx");
    }
}
