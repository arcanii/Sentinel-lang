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
/// C0.3 added [`ExprKind::Var`] for variable references. C0.4 adds
/// [`ExprKind::Block`] (brace-wrapped block as expression),
/// [`ExprKind::If`] (mandatory else per ADR 0010 D9, C-style truthy
/// condition), and [`ExprKind::Call`] (direct call by identifier
/// per ADR 0010 D10; only `print` resolves to anything in C0.4
/// since `fn` defs wait for C0.5).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExprKind {
    IntLit(i64),
    Var(String),
    Unary(UnaryOp, Box<Expr>),
    Binary(BinOp, Box<Expr>, Box<Expr>),
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

/// A C0.5+ program: one or more function definitions. One of them
/// must be named `main` (no parameters) — the entry point. The
/// previous C0.3-0.4 shape (`stmt* tail_expr`) now lives inside fn
/// bodies as [`Block`]s.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Program {
    pub fns: Vec<FnDef>,
    pub span: Span,
}

/// A function definition: `fn name(p1, p2, …) { body }`. All
/// parameters and return value are i64 in C0.5 (ADR 0009 says
/// everything is i64); ADR 0010 D5 reserves the `->` token for
/// the C1 type-annotation grammar but C0.5 emits none.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FnDef {
    pub name: String,
    pub name_span: Span,
    pub params: Vec<Param>,
    pub body: Block,
    pub span: Span,
}

/// A function parameter. Just a name + span for C0.5 (no type
/// annotation); the annotation slot lands at C1.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Param {
    pub name: String,
    pub span: Span,
}

/// A brace-wrapped block expression `{ stmt* tail_expr }`. The
/// trailing expression is the block's value. Used by `if`/`else`
/// branches, standalone `{ ... }` expressions, and function bodies
/// (a function's body is its [`Block`] whose tail expression is
/// the return value).
#[derive(Debug, Clone, PartialEq, Eq)]
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
            ExprKind::Var(name) => write!(f, "{name}"),
            ExprKind::Unary(op, inner) => write!(f, "({} {})", op.symbol(), inner.kind),
            ExprKind::Binary(op, lhs, rhs) => {
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

/// Program prints each function definition on its own line.
impl fmt::Display for Program {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut first = true;
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

/// `(fn name (params) body)` — params is a space-separated list
/// of bare parameter names; body delegates to [`Block`].
impl fmt::Display for FnDef {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "(fn {} (", self.name)?;
        let mut first = true;
        for p in &self.params {
            if !first {
                write!(f, " ")?;
            }
            first = false;
            write!(f, "{}", p.name)?;
        }
        write!(f, ") {})", self.body)
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

    fn main_fn(tail: Expr) -> FnDef {
        let body_span = tail.span.clone();
        FnDef {
            name: "main".to_string(),
            name_span: 0..4,
            params: vec![],
            body: Block { stmts: vec![], tail, span: body_span.clone() },
            span: 0..body_span.end,
        }
    }

    #[test]
    fn display_program_one_main() {
        let p = Program {
            fns: vec![main_fn(lit(42, 0..2))],
            span: 0..2,
        };
        assert_eq!(p.to_string(), "(fn main () (block 42))");
    }

    #[test]
    fn display_program_two_fns() {
        let double = FnDef {
            name: "double".to_string(),
            name_span: 0..6,
            params: vec![Param { name: "x".to_string(), span: 0..1 }],
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
        let p = Program { fns: vec![double, main], span: 0..20 };
        assert_eq!(
            p.to_string(),
            "(fn double (x) (block (* x 2)))\n(fn main () (block (double 7)))"
        );
    }

    #[test]
    fn display_fn_def_zero_params() {
        let f = main_fn(lit(5, 0..1));
        assert_eq!(f.to_string(), "(fn main () (block 5))");
    }

    #[test]
    fn display_fn_def_multi_params() {
        let f = FnDef {
            name: "add".to_string(),
            name_span: 0..3,
            params: vec![
                Param { name: "a".to_string(), span: 0..1 },
                Param { name: "b".to_string(), span: 0..1 },
            ],
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
        assert_eq!(f.to_string(), "(fn add (a b) (block (+ a b)))");
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
}
