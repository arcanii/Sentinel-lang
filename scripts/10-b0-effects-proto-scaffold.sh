#!/usr/bin/env bash
# 10-b0-effects-proto-scaffold.sh - Phase B0: scaffold sentinel-effects-proto
# with a minimal expression language (lex + parse + eval). Idempotent.
set -uo pipefail

REPO_ROOT="${REPO_ROOT:-$(pwd)}"
cd "$REPO_ROOT" || { echo "ERROR: cannot cd to $REPO_ROOT" >&2; return 1 2>/dev/null || exit 1; }

if [[ ! -f Cargo.toml ]]; then
  echo "ERROR: not at repo root (no Cargo.toml)" >&2
  return 1 2>/dev/null || exit 1
fi

echo "====== B0 SCAFFOLD START"
echo "Date: $(date -u +%Y-%m-%dT%H:%M:%SZ)"
echo

cat > /tmp/sentinel_b0_scaffold.py <<'PYEOF'
#!/usr/bin/env python3
"""B0 scaffold: create sentinel-effects-proto, add to workspace."""
import re
from pathlib import Path

ROOT = Path.cwd()
CRATE = ROOT / "crates" / "sentinel-effects-proto"

def write(p: Path, content: str):
    p.parent.mkdir(parents=True, exist_ok=True)
    if p.exists() and p.read_text() == content:
        print(f"  UNCHANGED {p.relative_to(ROOT)}")
        return
    action = "UPDATE" if p.exists() else "CREATE"
    p.write_text(content)
    print(f"  {action} {p.relative_to(ROOT)}")

# ---- Cargo.toml workspace member -------------------------------------------

def update_workspace_manifest():
    p = ROOT / "Cargo.toml"
    txt = p.read_text()
    if '"crates/sentinel-effects-proto"' in txt:
        print("  UNCHANGED Cargo.toml (member already present)")
        return
    # Insert after the sentinel-broker entry so it sits at the top of the
    # Phase B/research section.
    new = re.sub(
        r'("crates/sentinel-broker",\s*\n)',
        r'\1    "crates/sentinel-effects-proto",\n',
        txt,
        count=1,
    )
    if new == txt:
        print("  WARN Cargo.toml: could not find anchor; please add manually")
        return
    p.write_text(new)
    print("  UPDATE Cargo.toml (added sentinel-effects-proto member)")

# ---- crate manifest --------------------------------------------------------

CARGO_TOML = '''[package]
name        = "sentinel-effects-proto"
description = "Sentinel-Mini: research-grade tree-walking interpreter for validating the effect system in isolation (Phase B)."

edition.workspace      = true
rust-version.workspace = true
version.workspace      = true
authors.workspace      = true
license.workspace      = true
repository.workspace   = true

[dependencies]
logos      = { workspace = true }
thiserror  = { workspace = true }
tracing    = { workspace = true }

[dev-dependencies]
# none yet; insta/proptest deferred to later B phases

[lints.rust]
unsafe_code = "deny"
'''

# ---- lib.rs ----------------------------------------------------------------

LIB_RS = '''//! sentinel-effects-proto (Sentinel-Mini)
//!
//! A research-grade tree-walking interpreter built to validate Sentinel's
//! effect-system design before committing to the production compiler in
//! Phase C. See HANDOVER.md §5 ("The Effects Prototype") for the strategic
//! framing.
//!
//! # Scope (B0)
//!
//! This is the **B0** milestone: the smallest possible end-to-end
//! pipeline. The language at B0 is a pure expression calculus with:
//!
//! - integer literals (`42`, `-3`)
//! - boolean literals (`true`, `false`)
//! - identifiers
//! - `let x = e1 in e2`
//! - `if cond then e1 else e2`
//! - lambdas: `fn(x) => body`
//! - application: `f(x)`
//! - arithmetic: `+ - * /`
//! - comparison: `== < >`
//! - parenthesised grouping
//!
//! Everything is an expression; there are no statements. There are no
//! types, no effects, no `secret` qualifier, no broker integration.
//! Those land in B1 through B4 (see BACKLOG / future plan).
//!
//! # Non-goals
//!
//! Performance is irrelevant. Error recovery is minimal (first error
//! wins). The AST is plain `enum`s; no arena allocation. This is a
//! research artifact, not a production compiler.

pub mod ast;
pub mod eval;
pub mod lexer;
pub mod parser;

pub use ast::Expr;
pub use eval::{eval, EvalError, Value};
pub use lexer::{lex, LexError, Token};
pub use parser::{parse, ParseError};

/// Convenience: lex + parse + eval a source string in a fresh environment.
///
/// Returns the resulting [`Value`] or the first error encountered.
pub fn run(source: &str) -> Result<Value, MiniError> {
    let tokens = lex(source).map_err(MiniError::Lex)?;
    let expr = parse(&tokens).map_err(MiniError::Parse)?;
    eval(&expr, &eval::Env::empty()).map_err(MiniError::Eval)
}

/// Top-level error type spanning every pipeline stage.
#[derive(Debug, thiserror::Error)]
pub enum MiniError {
    #[error("lex error: {0}")]
    Lex(#[from] LexError),
    #[error("parse error: {0}")]
    Parse(#[from] ParseError),
    #[error("eval error: {0}")]
    Eval(#[from] EvalError),
}
'''

# ---- lexer.rs --------------------------------------------------------------

LEXER_RS = r'''//! Lexer for Sentinel-Mini (B0).
//!
//! Uses [`logos`] for token recognition. Returns a flat `Vec<Token>` so
//! the hand-written parser can index into it freely (recovery and span
//! tracking are deliberately minimal at B0; both will get attention in
//! later phases).

use logos::Logos;
use thiserror::Error;

/// All tokens recognised by the B0 lexer.
#[derive(Logos, Debug, Clone, PartialEq)]
#[logos(skip r"[ \t\n\r\f]+")]
#[logos(skip r"//[^\n]*")]
pub enum Token {
    // Literals
    #[regex(r"-?[0-9]+", |lex| lex.slice().parse::<i64>().ok())]
    Int(i64),

    #[token("true", |_| true)]
    #[token("false", |_| false)]
    Bool(bool),

    // Keywords
    #[token("let")]
    Let,
    #[token("in")]
    In,
    #[token("if")]
    If,
    #[token("then")]
    Then,
    #[token("else")]
    Else,
    #[token("fn")]
    Fn,

    // Identifiers (after keywords so keywords match first)
    #[regex(r"[a-zA-Z_][a-zA-Z0-9_]*", |lex| lex.slice().to_owned())]
    Ident(String),

    // Punctuation
    #[token("=")]
    Eq,
    #[token("==")]
    EqEq,
    #[token("<")]
    Lt,
    #[token(">")]
    Gt,
    #[token("+")]
    Plus,
    #[token("-")]
    Minus,
    #[token("*")]
    Star,
    #[token("/")]
    Slash,
    #[token("(")]
    LParen,
    #[token(")")]
    RParen,
    #[token(",")]
    Comma,
    #[token("=>")]
    FatArrow,
}

/// Errors produced by [`lex`].
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum LexError {
    #[error("unrecognised token at byte offset {offset}: {snippet:?}")]
    Unrecognised { offset: usize, snippet: String },
}

/// Tokenise `source` into a flat vector of tokens.
///
/// Whitespace and `// line comments` are skipped. The first unrecognised
/// character produces [`LexError::Unrecognised`] and aborts lexing.
pub fn lex(source: &str) -> Result<Vec<Token>, LexError> {
    let mut out = Vec::new();
    let mut lx = Token::lexer(source);
    while let Some(res) = lx.next() {
        match res {
            Ok(tok) => out.push(tok),
            Err(()) => {
                let span = lx.span();
                let snippet: String = source
                    .get(span.start..span.end)
                    .unwrap_or("")
                    .chars()
                    .take(16)
                    .collect();
                return Err(LexError::Unrecognised { offset: span.start, snippet });
            }
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lex_integers_and_arith() {
        let toks = lex("1 + 2 * 3").unwrap();
        assert_eq!(
            toks,
            vec![
                Token::Int(1),
                Token::Plus,
                Token::Int(2),
                Token::Star,
                Token::Int(3),
            ]
        );
    }

    #[test]
    fn lex_keywords_vs_idents() {
        let toks = lex("let lettuce = true in lettuce").unwrap();
        assert_eq!(
            toks,
            vec![
                Token::Let,
                Token::Ident("lettuce".into()),
                Token::Eq,
                Token::Bool(true),
                Token::In,
                Token::Ident("lettuce".into()),
            ]
        );
    }

    #[test]
    fn lex_lambda_syntax() {
        let toks = lex("fn(x) => x + 1").unwrap();
        assert_eq!(
            toks,
            vec![
                Token::Fn,
                Token::LParen,
                Token::Ident("x".into()),
                Token::RParen,
                Token::FatArrow,
                Token::Ident("x".into()),
                Token::Plus,
                Token::Int(1),
            ]
        );
    }

    #[test]
    fn lex_skips_line_comments() {
        let toks = lex("// hello\n1 + 2 // trailing").unwrap();
        assert_eq!(toks, vec![Token::Int(1), Token::Plus, Token::Int(2)]);
    }

    #[test]
    fn lex_unrecognised_char() {
        let err = lex("1 + @").unwrap_err();
        match err {
            LexError::Unrecognised { offset, .. } => assert_eq!(offset, 4),
        }
    }
}
'''

# ---- ast.rs ----------------------------------------------------------------

AST_RS = '''//! Abstract syntax tree for Sentinel-Mini (B0).
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
'''

# ---- parser.rs -------------------------------------------------------------

PARSER_RS = r'''//! Hand-written recursive-descent parser for Sentinel-Mini (B0).
//!
//! Precedence climbing handles the binary operators. Single-pass; the
//! first error aborts parsing. Spans are not tracked yet (B0 keeps
//! diagnostics minimal); they will be added in B1 alongside the type
//! checker so error highlighting can be meaningful.

use crate::ast::{BinOp, Expr};
use crate::lexer::Token;
use thiserror::Error;

/// Errors produced by [`parse`].
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum ParseError {
    #[error("unexpected end of input; expected {expected}")]
    UnexpectedEof { expected: &'static str },
    #[error("unexpected token {found:?}; expected {expected}")]
    Unexpected { found: Token, expected: &'static str },
    #[error("trailing tokens after expression: {found:?}")]
    Trailing { found: Token },
}

/// Parse a flat token stream into a single [`Expr`].
pub fn parse(tokens: &[Token]) -> Result<Expr, ParseError> {
    let mut p = Parser { toks: tokens, pos: 0 };
    let e = p.parse_expr()?;
    if let Some(t) = p.peek().cloned() {
        return Err(ParseError::Trailing { found: t });
    }
    Ok(e)
}

struct Parser<'a> {
    toks: &'a [Token],
    pos: usize,
}

impl<'a> Parser<'a> {
    fn peek(&self) -> Option<&Token> {
        self.toks.get(self.pos)
    }

    fn bump(&mut self) -> Option<Token> {
        let t = self.toks.get(self.pos).cloned();
        if t.is_some() {
            self.pos += 1;
        }
        t
    }

    fn expect(&mut self, expected: &'static str, pred: impl Fn(&Token) -> bool) -> Result<Token, ParseError> {
        match self.peek() {
            None => Err(ParseError::UnexpectedEof { expected }),
            Some(t) if pred(t) => Ok(self.bump().unwrap()),
            Some(t) => Err(ParseError::Unexpected { found: t.clone(), expected }),
        }
    }

    // --- expression grammar ------------------------------------------------
    //
    //   expr      := let | if | lambda | binop
    //   let       := "let" IDENT "=" expr "in" expr
    //   if        := "if" expr "then" expr "else" expr
    //   lambda    := "fn" "(" IDENT ")" "=>" expr
    //   binop     := compare
    //   compare   := add ( ("==" | "<" | ">") add )?
    //   add       := mul ( ("+" | "-") mul )*
    //   mul       := app ( ("*" | "/") app )*
    //   app       := atom ( "(" expr ")" )*
    //   atom      := INT | BOOL | IDENT | "(" expr ")"

    fn parse_expr(&mut self) -> Result<Expr, ParseError> {
        match self.peek() {
            Some(Token::Let) => self.parse_let(),
            Some(Token::If) => self.parse_if(),
            Some(Token::Fn) => self.parse_lambda(),
            _ => self.parse_compare(),
        }
    }

    fn parse_let(&mut self) -> Result<Expr, ParseError> {
        self.bump(); // 'let'
        let name = match self.bump() {
            Some(Token::Ident(s)) => s,
            Some(t) => return Err(ParseError::Unexpected { found: t, expected: "identifier" }),
            None => return Err(ParseError::UnexpectedEof { expected: "identifier" }),
        };
        self.expect("'='", |t| matches!(t, Token::Eq))?;
        let value = Box::new(self.parse_expr()?);
        self.expect("'in'", |t| matches!(t, Token::In))?;
        let body = Box::new(self.parse_expr()?);
        Ok(Expr::Let { name, value, body })
    }

    fn parse_if(&mut self) -> Result<Expr, ParseError> {
        self.bump(); // 'if'
        let cond = Box::new(self.parse_expr()?);
        self.expect("'then'", |t| matches!(t, Token::Then))?;
        let then_branch = Box::new(self.parse_expr()?);
        self.expect("'else'", |t| matches!(t, Token::Else))?;
        let else_branch = Box::new(self.parse_expr()?);
        Ok(Expr::If { cond, then_branch, else_branch })
    }

    fn parse_lambda(&mut self) -> Result<Expr, ParseError> {
        self.bump(); // 'fn'
        self.expect("'('", |t| matches!(t, Token::LParen))?;
        let param = match self.bump() {
            Some(Token::Ident(s)) => s,
            Some(t) => return Err(ParseError::Unexpected { found: t, expected: "identifier" }),
            None => return Err(ParseError::UnexpectedEof { expected: "identifier" }),
        };
        self.expect("')'", |t| matches!(t, Token::RParen))?;
        self.expect("'=>'", |t| matches!(t, Token::FatArrow))?;
        let body = Box::new(self.parse_expr()?);
        Ok(Expr::Lambda { param, body })
    }

    fn parse_compare(&mut self) -> Result<Expr, ParseError> {
        let lhs = self.parse_add()?;
        let op = match self.peek() {
            Some(Token::EqEq) => BinOp::Eq,
            Some(Token::Lt) => BinOp::Lt,
            Some(Token::Gt) => BinOp::Gt,
            _ => return Ok(lhs),
        };
        self.bump();
        let rhs = self.parse_add()?;
        Ok(Expr::BinOp { op, lhs: Box::new(lhs), rhs: Box::new(rhs) })
    }

    fn parse_add(&mut self) -> Result<Expr, ParseError> {
        let mut lhs = self.parse_mul()?;
        loop {
            let op = match self.peek() {
                Some(Token::Plus) => BinOp::Add,
                Some(Token::Minus) => BinOp::Sub,
                _ => break,
            };
            self.bump();
            let rhs = self.parse_mul()?;
            lhs = Expr::BinOp { op, lhs: Box::new(lhs), rhs: Box::new(rhs) };
        }
        Ok(lhs)
    }

    fn parse_mul(&mut self) -> Result<Expr, ParseError> {
        let mut lhs = self.parse_app()?;
        loop {
            let op = match self.peek() {
                Some(Token::Star) => BinOp::Mul,
                Some(Token::Slash) => BinOp::Div,
                _ => break,
            };
            self.bump();
            let rhs = self.parse_app()?;
            lhs = Expr::BinOp { op, lhs: Box::new(lhs), rhs: Box::new(rhs) };
        }
        Ok(lhs)
    }

    fn parse_app(&mut self) -> Result<Expr, ParseError> {
        let mut callee = self.parse_atom()?;
        while matches!(self.peek(), Some(Token::LParen)) {
            self.bump(); // '('
            let arg = self.parse_expr()?;
            self.expect("')'", |t| matches!(t, Token::RParen))?;
            callee = Expr::App { callee: Box::new(callee), arg: Box::new(arg) };
        }
        Ok(callee)
    }

    fn parse_atom(&mut self) -> Result<Expr, ParseError> {
        match self.bump() {
            Some(Token::Int(n)) => Ok(Expr::Int(n)),
            Some(Token::Bool(b)) => Ok(Expr::Bool(b)),
            Some(Token::Ident(s)) => Ok(Expr::Var(s)),
            Some(Token::LParen) => {
                let e = self.parse_expr()?;
                self.expect("')'", |t| matches!(t, Token::RParen))?;
                Ok(e)
            }
            Some(t) => Err(ParseError::Unexpected { found: t, expected: "atom" }),
            None => Err(ParseError::UnexpectedEof { expected: "atom" }),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lexer::lex;

    fn p(src: &str) -> Expr {
        parse(&lex(src).unwrap()).unwrap()
    }

    #[test]
    fn parses_integer_literal() {
        assert_eq!(p("42"), Expr::Int(42));
    }

    #[test]
    fn arithmetic_precedence() {
        // 1 + 2 * 3  parses as  1 + (2 * 3)
        let e = p("1 + 2 * 3");
        match e {
            Expr::BinOp { op: BinOp::Add, lhs, rhs } => {
                assert_eq!(*lhs, Expr::Int(1));
                match *rhs {
                    Expr::BinOp { op: BinOp::Mul, .. } => {}
                    other => panic!("rhs not mul: {other:?}"),
                }
            }
            other => panic!("not an add: {other:?}"),
        }
    }

    #[test]
    fn parses_let_in() {
        let e = p("let x = 1 in x + 2");
        match e {
            Expr::Let { name, .. } => assert_eq!(name, "x"),
            other => panic!("not a let: {other:?}"),
        }
    }

    #[test]
    fn parses_lambda_and_application() {
        let e = p("fn(x) => x + 1");
        assert!(matches!(e, Expr::Lambda { .. }));
        let e = p("(fn(x) => x + 1)(41)");
        assert!(matches!(e, Expr::App { .. }));
    }

    #[test]
    fn parse_error_on_missing_then() {
        let err = parse(&lex("if true 1 else 2").unwrap()).unwrap_err();
        assert!(matches!(err, ParseError::Unexpected { expected: "'then'", .. }));
    }
}
'''

# ---- eval.rs ---------------------------------------------------------------

EVAL_RS = r'''//! Tree-walking evaluator for Sentinel-Mini (B0).
//!
//! Closures capture by sharing a persistent environment (a cons-list of
//! `(name, value)` cells behind an `Arc`). This is the textbook
//! Crafting-Interpreters-style approach; we'll revisit it if/when we
//! integrate the broker as a value heap in a later B milestone.

use crate::ast::{BinOp, Expr};
use std::sync::Arc;
use thiserror::Error;

/// Runtime values.
#[derive(Debug, Clone)]
pub enum Value {
    Int(i64),
    Bool(bool),
    /// A closure: parameter, body, and the environment captured at
    /// definition time.
    Closure {
        param: String,
        body: Arc<Expr>,
        env: Env,
    },
}

impl PartialEq for Value {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (Value::Int(a), Value::Int(b)) => a == b,
            (Value::Bool(a), Value::Bool(b)) => a == b,
            // Closures compare by identity-of-source, which we don't
            // track at B0; treat them as never equal.
            _ => false,
        }
    }
}

/// Errors produced by [`eval`].
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum EvalError {
    #[error("unbound variable: {0}")]
    Unbound(String),
    #[error("type error: expected {expected}, got {got}")]
    Type { expected: &'static str, got: &'static str },
    #[error("division by zero")]
    DivByZero,
    #[error("cannot apply non-function value")]
    NotAFunction,
}

/// A persistent environment. Cheaply cloneable.
#[derive(Debug, Clone, Default)]
pub struct Env(Option<Arc<EnvCell>>);

#[derive(Debug)]
struct EnvCell {
    name: String,
    value: Value,
    rest: Env,
}

impl Env {
    /// The empty environment.
    pub fn empty() -> Self {
        Self::default()
    }

    /// Extend with a new binding. Returns a new environment; the original
    /// is unchanged (shared structurally).
    pub fn extend(&self, name: String, value: Value) -> Self {
        Env(Some(Arc::new(EnvCell { name, value, rest: self.clone() })))
    }

    fn lookup(&self, name: &str) -> Option<Value> {
        let mut cur = self.0.as_deref();
        while let Some(cell) = cur {
            if cell.name == name {
                return Some(cell.value.clone());
            }
            cur = cell.rest.0.as_deref();
        }
        None
    }
}

fn type_name(v: &Value) -> &'static str {
    match v {
        Value::Int(_) => "int",
        Value::Bool(_) => "bool",
        Value::Closure { .. } => "function",
    }
}

/// Evaluate `expr` in the given environment.
pub fn eval(expr: &Expr, env: &Env) -> Result<Value, EvalError> {
    match expr {
        Expr::Int(n) => Ok(Value::Int(*n)),
        Expr::Bool(b) => Ok(Value::Bool(*b)),
        Expr::Var(name) => env
            .lookup(name)
            .ok_or_else(|| EvalError::Unbound(name.clone())),
        Expr::Let { name, value, body } => {
            let v = eval(value, env)?;
            eval(body, &env.extend(name.clone(), v))
        }
        Expr::If { cond, then_branch, else_branch } => {
            let c = eval(cond, env)?;
            match c {
                Value::Bool(true) => eval(then_branch, env),
                Value::Bool(false) => eval(else_branch, env),
                other => Err(EvalError::Type { expected: "bool", got: type_name(&other) }),
            }
        }
        Expr::Lambda { param, body } => Ok(Value::Closure {
            param: param.clone(),
            body: Arc::new((**body).clone()),
            env: env.clone(),
        }),
        Expr::App { callee, arg } => {
            let f = eval(callee, env)?;
            let a = eval(arg, env)?;
            match f {
                Value::Closure { param, body, env: captured } => {
                    eval(&body, &captured.extend(param, a))
                }
                _ => Err(EvalError::NotAFunction),
            }
        }
        Expr::BinOp { op, lhs, rhs } => {
            let l = eval(lhs, env)?;
            let r = eval(rhs, env)?;
            eval_binop(*op, l, r)
        }
    }
}

fn eval_binop(op: BinOp, l: Value, r: Value) -> Result<Value, EvalError> {
    use BinOp::*;
    match (op, &l, &r) {
        (Add, Value::Int(a), Value::Int(b)) => Ok(Value::Int(a.wrapping_add(*b))),
        (Sub, Value::Int(a), Value::Int(b)) => Ok(Value::Int(a.wrapping_sub(*b))),
        (Mul, Value::Int(a), Value::Int(b)) => Ok(Value::Int(a.wrapping_mul(*b))),
        (Div, Value::Int(_), Value::Int(0)) => Err(EvalError::DivByZero),
        (Div, Value::Int(a), Value::Int(b)) => Ok(Value::Int(a.wrapping_div(*b))),
        (Eq, Value::Int(a), Value::Int(b)) => Ok(Value::Bool(a == b)),
        (Eq, Value::Bool(a), Value::Bool(b)) => Ok(Value::Bool(a == b)),
        (Lt, Value::Int(a), Value::Int(b)) => Ok(Value::Bool(a < b)),
        (Gt, Value::Int(a), Value::Int(b)) => Ok(Value::Bool(a > b)),
        _ => Err(EvalError::Type {
            expected: "matching numeric/boolean operands",
            got: type_name(&l),
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{lex, parse};

    fn run(src: &str) -> Result<Value, super::super::MiniError> {
        let toks = lex(src)?;
        let expr = parse(&toks)?;
        Ok(eval(&expr, &Env::empty())?)
    }

    #[test]
    fn eval_arithmetic() {
        assert_eq!(run("1 + 2 * 3").unwrap(), Value::Int(7));
    }

    #[test]
    fn eval_let_in() {
        assert_eq!(run("let x = 10 in x + x").unwrap(), Value::Int(20));
    }

    #[test]
    fn eval_if_then_else() {
        assert_eq!(run("if 1 < 2 then 100 else 200").unwrap(), Value::Int(100));
    }

    #[test]
    fn eval_lambda_application() {
        assert_eq!(run("(fn(x) => x + 1)(41)").unwrap(), Value::Int(42));
    }

    #[test]
    fn eval_closure_captures_env() {
        let v = run("let y = 10 in (fn(x) => x + y)(5)").unwrap();
        assert_eq!(v, Value::Int(15));
    }

    #[test]
    fn eval_unbound_variable() {
        let err = run("oops").unwrap_err();
        assert!(matches!(err, super::super::MiniError::Eval(EvalError::Unbound(_))));
    }

    #[test]
    fn eval_div_by_zero() {
        let err = run("10 / 0").unwrap_err();
        assert!(matches!(err, super::super::MiniError::Eval(EvalError::DivByZero)));
    }

    #[test]
    fn eval_type_error_in_if() {
        let err = run("if 1 then 2 else 3").unwrap_err();
        assert!(matches!(err, super::super::MiniError::Eval(EvalError::Type { .. })));
    }
}
'''

# ---- tests/integration.rs --------------------------------------------------

INTEGRATION_RS = r'''//! End-to-end tests for sentinel-effects-proto B0.
//!
//! These exercise the full lex+parse+eval pipeline via the top-level
//! [`run`] convenience function, ensuring the public surface composes.

use sentinel_effects_proto::{run, Value};

#[test]
fn pipeline_arithmetic() {
    assert_eq!(run("1 + 2 * 3 - 4").unwrap(), Value::Int(3));
}

#[test]
fn pipeline_nested_let() {
    let src = "let x = 1 in let y = x + 1 in let z = y + 1 in x + y + z";
    assert_eq!(run(src).unwrap(), Value::Int(1 + 2 + 3));
}

#[test]
fn pipeline_higher_order_function() {
    // (fn(f) => f(10))(fn(x) => x * x)  =  100
    assert_eq!(
        run("(fn(f) => f(10))(fn(x) => x * x)").unwrap(),
        Value::Int(100),
    );
}

#[test]
fn pipeline_recursion_via_y_combinator_would_need_letrec() {
    // B0 has no `letrec`; this just documents that recursion is a B1+
    // problem. We test that the missing-feature failure mode is sensible:
    // `f` is unbound inside its own definition.
    let src = "let f = fn(n) => if n == 0 then 0 else f(n - 1) in f(3)";
    let err = run(src).unwrap_err();
    let msg = format!("{err}");
    assert!(msg.contains("unbound variable: f"), "got: {msg}");
}

#[test]
fn pipeline_line_comments_are_ignored() {
    let src = "// header comment\n1 + 2 // tail comment\n";
    assert_eq!(run(src).unwrap(), Value::Int(3));
}
'''

# ---- write files -----------------------------------------------------------

def main():
    print("---- writing crate files ----")
    write(CRATE / "Cargo.toml", CARGO_TOML)
    write(CRATE / "src" / "lib.rs", LIB_RS)
    write(CRATE / "src" / "lexer.rs", LEXER_RS)
    write(CRATE / "src" / "ast.rs", AST_RS)
    write(CRATE / "src" / "parser.rs", PARSER_RS)
    write(CRATE / "src" / "eval.rs", EVAL_RS)
    write(CRATE / "tests" / "integration.rs", INTEGRATION_RS)
    print("---- updating workspace manifest ----")
    update_workspace_manifest()

if __name__ == "__main__":
    main()
PYEOF

python3 /tmp/sentinel_b0_scaffold.py
PATCH_RC=$?

echo
echo "====== B0 SCAFFOLD DONE (rc=$PATCH_RC)"
echo
echo "====== BUILD"
cargo build -p sentinel-effects-proto 2>&1 | tail -30
echo
echo "====== CLIPPY"
cargo clippy -p sentinel-effects-proto --all-targets -- -D warnings 2>&1 | tail -40
echo
echo "====== TESTS"
if command -v cargo-nextest >/dev/null 2>&1; then
  cargo nextest run -p sentinel-effects-proto 2>&1 | tail -30
else
  cargo test -p sentinel-effects-proto 2>&1 | tail -30
fi
echo
echo "====== DOC TESTS"
cargo test -p sentinel-effects-proto --doc 2>&1 | tail -15
echo
echo "====== B0 SCRIPT END"
