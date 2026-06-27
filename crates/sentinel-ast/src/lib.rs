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
    /// C5.3 / ADR 0027: bitwise AND `&` (infix; the borrow prefix reuses
    /// the same token, disambiguated by parser position). Data-independent
    /// latency — a constant-time operation on `secret` values.
    BitAnd,
    /// C5.3 / ADR 0027: bitwise OR `|`. Constant-time.
    BitOr,
    /// C5.3 / ADR 0027: bitwise XOR `^`. Constant-time.
    BitXor,
    /// ADR 0048: shift left `<<`. Asymmetric (value `<<` amount): the result
    /// takes the LEFT (value) operand's type/secrecy; a SECRET amount is a
    /// variable-time shift rejected by the constant-time check (like a secret
    /// divisor). Lexed in the parser from two span-adjacent `<` tokens.
    Shl,
    /// ADR 0048: LOGICAL (zero-fill) shift right `>>` for all integer types —
    /// so a rotate composes as `(x << n) | (x >> (W - n))`. Lexed from two
    /// span-adjacent `>` tokens (NOT a lexer token, to keep nested generics
    /// like `Vec<Box<i64>>` closing one `>` at a time).
    Shr,
}

impl BinOp {
    pub fn symbol(self) -> &'static str {
        match self {
            BinOp::Add => "+",
            BinOp::Sub => "-",
            BinOp::Mul => "*",
            BinOp::Div => "/",
            BinOp::BitAnd => "&",
            BinOp::BitOr => "|",
            BinOp::BitXor => "^",
            BinOp::Shl => "<<",
            BinOp::Shr => ">>",
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
/// adds logical not (`!`) per ADR 0012 D7; C2.0.2 (per ADR 0017 D3 /
/// D4 / D10) adds `&` shared borrow, `&mut` exclusive borrow, and
/// `*` dereference. The `*` and `&` tokens are reused from existing
/// lexer entries — the parser disambiguates positionally (prefix
/// unary vs. infix multiply for `*`; prefix borrow-take is unambiguous
/// since `&&` lexes longest-first as logical-and).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum UnaryOp {
    Neg,
    Not,
    /// `&expr` — shared borrow per ADR 0017 D3. Operand must be an
    /// lvalue (the type checker enforces this with
    /// [`TypeError::BorrowOfRvalue`](../sentinel_types/enum.TypeError.html)).
    Ref,
    /// `&mut expr` — exclusive borrow per ADR 0017 D3. Operand must
    /// be a *mutable* lvalue.
    RefMut,
    /// `*expr` — dereference per ADR 0017 D4. Operand must be a
    /// reference type (`&T` or `&mut T`).
    Deref,
    /// ADR 0058: `sqrt(expr)` — the IEEE-754 square root of an `f64`,
    /// lowering to the `llvm.sqrt.f64` intrinsic (a hardware instruction).
    /// Written in CALL form (`sqrt(x)`), not as a prefix operator — the
    /// parser recognises the reserved name `sqrt` with exactly one argument
    /// and produces this node. Modelled as a unary op (rather than a builtin
    /// fn) so it needs no `FnId` and so causes no `FnId` shift / scg
    /// re-mirror. Operand and result are `f64`.
    Sqrt,
    /// ADR 0057 Phase 1b: `ptr_of(expr)` — the raw C data pointer of a
    /// borrowed byte array `&[u8]` (the `data` field of the `{ len, data }`
    /// slice). Written in call form, recognised by the parser's reserved-name
    /// pass (like `sqrt`), so it needs no `FnId`. Operand is `&[u8]` (or
    /// `&mut [u8]`); result is the opaque `ptr` for passing to an `extern`.
    PtrOf,
    /// ADR 0057 Phase 1b: `ptr_of_mut(expr)` — like `ptr_of` but requires a
    /// `&mut [u8]` (an exclusive borrow, so C may write through the pointer).
    PtrOfMut,
    /// ADR 0057 Phase 1b: `is_null(expr)` — `true` iff a `ptr` is NULL.
    /// Recognised by reserved name (like `ptr_of`); operand is `ptr`, result is
    /// `bool`. Lets a Sentinel wrapper null-check an FFI return (e.g. `getenv`)
    /// before reading through it. No `FnId`.
    IsNull,
}

impl UnaryOp {
    pub fn symbol(self) -> &'static str {
        match self {
            UnaryOp::Neg => "-",
            UnaryOp::Not => "!",
            UnaryOp::Ref => "&",
            UnaryOp::RefMut => "&mut",
            UnaryOp::Deref => "*",
            // Not a true prefix symbol — `sqrt` is written in call form. Used
            // only for debug rendering (it never round-trips through a
            // selfhost / Bar-A path, which errors on `f64`).
            UnaryOp::Sqrt => "sqrt",
            // ADR 0057 Phase 1b — call-form intrinsics, debug rendering only
            // (they never round-trip through a selfhost / Bar-A path).
            UnaryOp::PtrOf => "ptr_of",
            UnaryOp::PtrOfMut => "ptr_of_mut",
            UnaryOp::IsNull => "is_null",
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
///
/// D.2 adds [`ExprKind::CharLit`] (`'a'` → a `u8` byte) and
/// [`ExprKind::StringLit`] (`"abc"` → a `[u8]` byte array) per ADR
/// 0033 D2. Both carry the *decoded* bytes (escapes already
/// processed at parse time, like [`ExprKind::IntLit`] decodes its
/// span); the type layer (`Type::U8`, char → `u8`, string → `[u8]`)
/// lands at D.2 (3/N).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum ExprKind {
    IntLit(i64),
    /// ADR 0058: a 64-bit float literal — `3.14`, `1e9`. The payload is
    /// the IEEE-754 bit pattern (`f64::to_bits`), NOT the `f64` itself, so
    /// `ExprKind` can keep deriving `Eq`/`Hash` (which `f64` does not
    /// implement). The parser decodes the literal text once; codegen
    /// reconstructs the value via `f64::from_bits`. Types to `Type::F64`.
    FloatLit(u64),
    BoolLit(bool),
    /// The `null` keyword literal per ADR 0014 D2. The type checker
    /// resolves its `?T` type from the surrounding context.
    NullLit,
    /// D.2 / ADR 0033 D2: a char/byte literal `'a'`. The payload is
    /// the single decoded byte (escapes `\n \t \r \0 \\ \' \" \xHH`
    /// resolved by the parser; a char literal is exactly one byte —
    /// the parser rejects `''` / `'ab'` / multi-byte source chars).
    /// Types to `u8` at D.2 (3/N).
    CharLit(u8),
    /// D.2 / ADR 0033 D2: a string literal `"abc"`. The payload is
    /// the decoded byte sequence (the UTF-8 source bytes with the
    /// same escapes plus `\"`). A string IS a `[u8]` (ADR 0033 D3),
    /// so this types to `[u8]` and reuses the array machinery at
    /// D.2 (3/N)+(4/N).
    StringLit(Vec<u8>),
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
    /// `declassify(e)` — the C3 / ADR 0019 D6 escape hatch from
    /// `secret T`. Inner must type to `Type::Secret(_)`; result is
    /// the unwrapped type. Mandatory parens. Parser produces this
    /// variant at C3.0; type-check rejects with
    /// `TypeError::DeclassifyNotYet` until C3.1 lands.
    Declassify(Box<Expr>),
    /// ADR 0049: integer cast `expr as T` (T ∈ {i64, i32, u8}). A width
    /// conversion: zero-extend (u8 source) / sign-extend (signed source) /
    /// truncate / no-op (same width). Preserves secrecy (`(secret T) as U` →
    /// `secret U`); data-independent, so NOT a constant-time sink. The target
    /// stays a `TypeExpr` (resolved at the types stage, like a `let`
    /// annotation). Closes the i32-construction gap.
    Cast(Box<Expr>, TypeExpr),
    /// `handle expr with { Effect.op(p1, ..., k) => body, return v => body }`
    /// per ADR 0020 D4 + D5 (C3.4). The body's effect row is
    /// reduced by the set of effects handled here per D6. Arms
    /// share the outer handle expression's type — return arm's
    /// body type IS the outer type, all op arms' bodies match it.
    /// The last `param_names` entry inside each arm is the kont
    /// binding (used to resume the captured continuation).
    Handle {
        body: Box<Expr>,
        arms: Vec<HandlerArm>,
        return_arm: Option<Box<ReturnArm>>,
    },
    /// `perform Effect.op(args)` per ADR 0020 D4 + D5 (C3.4). The
    /// effect bubbles into the enclosing fn's row; the inferred
    /// row only discharges when the perform is wrapped in a
    /// matching `handle` per D6.
    Perform {
        effect: Spanned<String>,
        op: Spanned<String>,
        args: Vec<Expr>,
    },
    /// Postfix method call `target.method(args)` per ADR 0022 D3 +
    /// D7 (C4.1). The auto-ref rule at type-check time inserts the
    /// matching `&target` / `&mut target` based on the method's
    /// `self_kind`. The method name stays as a string here — the
    /// type checker looks it up against the receiver's class type
    /// in pass 2 (after fn / class signatures are populated).
    MethodCall {
        target: Box<Expr>,
        method: String,
        method_span: Span,
        args: Vec<Expr>,
    },
    /// `Name::init(args)` class instantiation per ADR 0022 D5
    /// (C4.1). The struct-literal syntax `Name { ... }` is
    /// rejected for classes — every class flows through `init`
    /// to enforce D4's no-half-constructed invariant. Resolve
    /// validates the class name; type-check validates arg types
    /// against the init signature.
    ClassInit {
        class_name: String,
        class_name_span: Span,
        args: Vec<Expr>,
    },
    /// `ImplName::method(args)` qualified call per ADR 0023 D5
    /// Path 2 (C4.2). Picks a specific named impl by name; the
    /// first arg is the receiver (no auto-ref — the user passes
    /// `&obj` / `&mut obj` explicitly). Disambiguated from
    /// `Name::init` at parse time: if the second Ident is
    /// `init`, produces `ClassInit`; otherwise produces this
    /// variant. Resolve at C4.2 (1/N) leaves this untouched —
    /// (2/N) brings up the impl table + dispatch.
    QualifiedCall {
        impl_name: String,
        impl_name_span: Span,
        method: String,
        method_span: Span,
        args: Vec<Expr>,
    },
    /// C4.4 / ADR 0024 D1: `scope concurrent { ... }` — structured-
    /// concurrency block. The `concurrent` keyword is a positional
    /// Ident (the C4.0 lexer keeps it as a plain Ident per the
    /// smallest-surface principle); other scope modes are reserved
    /// for future ADRs. The block's value is its tail expression;
    /// the scope's concurrency contract (auto-await on exit) is a
    /// runtime concern. Resolve at C4.4 (1/N) surfaces ScopeNotYet
    /// until C4.4 (2/N) brings up the runtime + codegen.
    Scope {
        mode: ScopeMode,
        body: Box<Block>,
    },
    /// C4.4 / ADR 0024 D2: `spawn fn_name(args)` — creates a Task
    /// running the given fn call. At C4.4 minimum the inner
    /// expression must be a function-call (validated at resolve);
    /// arbitrary spawnable expressions are deferred per ADR 0024
    /// D10. The outer expression's type is `Task<T>` where T is
    /// the call's return type.
    Spawn {
        call_expr: Box<Expr>,
    },
    /// C4.4 / ADR 0024 D3: `task.await` — postfix form over a
    /// Task<T>-typed expression. Result type is T. Blocks until
    /// the spawned task produces a value.
    Await {
        task_expr: Box<Expr>,
    },
    /// Phase D.1 / ADR 0032: `match scrutinee { pat => body, … }` —
    /// exhaustive pattern matching over a sum type. An expression (all
    /// arms share a result type, like `if`). Resolve + types check it at
    /// D.1 (3/N) (`ResolvedExprKind::Match` → `TypedExprKind::Match` with
    /// exhaustiveness); codegen lowers it to a `switch` at D.1 (4/N).
    Match {
        scrutinee: Box<Expr>,
        arms: Vec<MatchArm>,
    },
}

/// C4.4 / ADR 0024 D1: scope-mode tag. Only `Concurrent` ships
/// at C4.4 minimum; `Sequential` / `Race` / other modes are
/// reserved for future ADRs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ScopeMode {
    Concurrent,
}

/// A single operation arm inside a `handle ... with { ... }`
/// expression per ADR 0020 D4 + D5 (C3.4). The `param_names`
/// vector lists the bindings for the op's parameters in order,
/// PLUS a final continuation binding (the kont). For an op
/// declared as `op(p1: T1, p2: T2) -> Ret`, an arm's
/// `param_names` has exactly three entries: two op-param
/// bindings and the kont. Resolve gives each its own VarId.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct HandlerArm {
    pub effect: Spanned<String>,
    pub op: Spanned<String>,
    pub param_names: Vec<Spanned<String>>,
    pub body: Expr,
    pub span: Span,
}

/// The optional `return v => body` arm of a `handle ... with`
/// expression per ADR 0020 D4 (C3.4). When present, the
/// handle's outer type equals `body`'s type with `value_name`
/// bound to the handled expression's type. When absent the
/// default `return v => v` is assumed.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ReturnArm {
    pub value_name: Spanned<String>,
    pub body: Expr,
    pub span: Span,
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
        /// C2 / ADR 0017 D2: `let mut x = ...` produces a mutable
        /// binding (re-assignable + exclusive-borrowable). Bare
        /// `let x = ...` is immutable.
        mutable: bool,
        name: String,
        name_span: Span,
        /// Optional type annotation per ADR 0012 D2. `Some` if the
        /// source had `let x: T = ...`; `None` if it was `let x = ...`
        /// (inference path).
        ty_annot: Option<TypeExpr>,
        value: Expr,
    },
    /// Assignment statement per ADR 0017 D2: `lhs = expr;`. LHS must
    /// be an lvalue (bare ident, `*ref`, or `expr.field`); the type
    /// checker enforces this and the mutability constraints.
    /// Mutable indexing (`a[i] = v;`) is out of scope at C2 per ADR
    /// 0017 D12.
    Assign {
        target: Expr,
        value: Expr,
    },
    /// Phase D.5 / ADR 0036 D3: `while <cond> { <body> }` — a loop
    /// statement (NOT an expression; a loop has no value). `cond` is a
    /// `bool` expression; `body` is run repeatedly while it holds, its
    /// tail value discarded each iteration.
    While {
        cond: Expr,
        body: Box<Block>,
    },
    /// Phase D.5 (2/N) / ADR 0036 D9: `break;` — exit the innermost
    /// enclosing `while` loop (codegen branches to its `loop_after`). A
    /// payload-free statement: no labels, no break-with-value (both
    /// deferred, D8). Rejected outside any loop at type-check (D7).
    Break,
    /// Phase D.5 (2/N) / ADR 0036 D9: `continue;` — skip to the next
    /// iteration of the innermost enclosing `while` loop (codegen
    /// branches to its `loop_cond`). Payload-free; rejected outside any
    /// loop at type-check (D7).
    Continue,
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
/// Phase D.6 (1/N) / ADR 0037 D2: a top-level `use a::b::Item;` import.
/// `path` is the full `::`-separated segment list as written, INCLUDING
/// the trailing item (e.g. `["lex", "token", "Token"]`); resolve splits
/// it into the module path + the imported item (the last segment is the
/// item — ADR 0037 open point 4). File-as-module: the leading segments
/// name a file relative to the source root (`lex/token.sentinel`).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct UseDecl {
    pub path: Vec<String>,
    pub span: Span,
}

impl fmt::Display for UseDecl {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "(use {})", self.path.join("::"))
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Program {
    /// Phase D.6 (1/N) / ADR 0037: top-level `use` imports. Always
    /// present (empty for single-file programs). Resolve rejects a
    /// non-empty `uses` with `UseDeclNotYet` until the D.6 (1/N) resolve
    /// increment wires the module graph; single-file programs are
    /// unaffected.
    pub uses: Vec<UseDecl>,
    pub fns: Vec<FnDef>,
    /// Top-level struct declarations per ADR 0013 D1. Always present
    /// (may be empty for C0/C1.3-compatible programs).
    pub structs: Vec<StructDecl>,
    /// Top-level effect declarations per ADR 0019 D4 (C3.0). Always
    /// present (may be empty for pre-C3 programs). Resolve rejects
    /// any non-empty `effects` at C3.0 with
    /// `ResolveError::EffectDeclNotYet` until C3.2 ships the
    /// effect-check query.
    pub effects: Vec<EffectDecl>,
    /// Top-level class declarations per ADR 0021 D1 + ADR 0022 D1
    /// (C4.1). Always present (may be empty for pre-C4 programs).
    /// Resolve / types / codegen at C4.1's first iteration leave
    /// classes untouched — the AST + parser layer ships first; the
    /// downstream passes light up incrementally in follow-on
    /// commits.
    pub classes: Vec<ClassDecl>,
    /// Top-level trait declarations per ADR 0021 D4 + ADR 0023 D1
    /// (C4.2). Always present (may be empty for pre-C4.2 programs).
    /// Resolve / types / codegen at C4.2's first iteration leave
    /// traits untouched — the AST + parser layer ships first.
    pub traits: Vec<TraitDecl>,
    /// Top-level impl declarations per ADR 0021 D5 + ADR 0023 D3+D4
    /// (C4.2). Both default impls (no name) and named impls
    /// (`impl Name as Trait for Type`) live in this vec; the
    /// optional `name` field distinguishes.
    pub impls: Vec<ImplDecl>,
    /// Phase D.1 / ADR 0032: top-level sum-type (`enum`) declarations.
    /// Always present (may be empty for pre-D.1 programs). Resolve builds
    /// the enum table + types interns `Type::Enum` at D.1 (3/N); codegen
    /// lowers construction + `match` at D.1 (4/N).
    pub enums: Vec<EnumDecl>,
    /// ADR 0057: top-level `extern "C" { fn … ; }` foreign-function
    /// declarations. Always present (empty for programs with no FFI). Each
    /// is a body-less C-ABI function declaration resolved against libc at
    /// link time; raw `extern`s live by convention in `std/sys/*` /
    /// `std/math/float` wrapper modules.
    pub externs: Vec<ExternFnDecl>,
    pub span: Span,
}

/// ADR 0057: a single `extern "C"` foreign-function declaration —
/// `fn name(p1: T, …) -> T;` with NO body. The name IS the C symbol
/// (un-mangled); params/return are restricted to the FFI-safe set
/// (`i64`/`f64`) and may not be `secret` (the constant-time fence). The
/// compiler emits an LLVM `declare` so `cc` resolves it against the platform
/// C library at link.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ExternFnDecl {
    pub name: String,
    pub name_span: Span,
    pub params: Vec<Param>,
    pub return_type: TypeExpr,
    /// ADR 0057 A9: native libraries declared via `extern "C" link("user32") {…}`
    /// — threaded into the link so a consumer of the module needs no `--link`
    /// flag. Empty for a plain `extern "C" {…}`. Every decl in one block shares
    /// the block's list. Pure link metadata: resolve/types/codegen ignore it; the
    /// driver collects it from the parsed program.
    pub link_libs: Vec<String>,
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
    /// Phase D.6 (1/N) / ADR 0037 D3: `pub` exports this item across
    /// modules (file-as-module visibility); un-`pub` is module-private.
    /// A no-op single-file (nothing imports it); enforced at resolve when
    /// a `use` crosses a module boundary.
    pub visibility: Visibility,
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
    /// Postfix effect-row annotation per ADR 0019 D1 (C3.0). Each
    /// entry is the spanned name of a declared effect. Empty for
    /// fns without an annotation. The annotation lives between the
    /// return type and the body: `fn f() -> i64 ! { Io, Net }`.
    /// Resolve rejects any non-empty `effect_row` at C3.0 with
    /// `ResolveError::EffectAnnotationNotYet` until C3.2 ships the
    /// effect-check query.
    pub effect_row: Vec<Spanned<String>>,
    /// ADR 0059: `true` iff declared `export "C" fn` — a C-ABI export emitted
    /// under its un-mangled C symbol (so other languages can call into
    /// Sentinel). Default `false`. Its signature is restricted to the FFI-safe
    /// set + the secret-fence (validated at type-check), like an `extern`.
    pub is_export_c: bool,
    pub body: Block,
    pub span: Span,
}

/// A function parameter with a mandatory type annotation
/// (`name: type`) per ADR 0012 D1. C2 / ADR 0017 D2 adds the
/// optional `mut` prefix: `mut x: T` lets the callee re-assign or
/// `&mut`-borrow its local copy. Param mutability is *binding-local*
/// — distinct from the caller passing a `&mut T` (which IS observable
/// to the caller).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Param {
    pub mutable: bool,
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
    /// Phase D.6 (1/N) / ADR 0037 D3: `pub` exports this type across
    /// modules; un-`pub` is module-private. No-op single-file.
    pub visibility: Visibility,
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

/// Phase D.1 / ADR 0032: a top-level sum-type (enum) declaration —
/// `enum Name { V1, V2(T), V3(T1, T2) }`. Variants are unit or carry a
/// positional tuple payload. Non-generic at the D.1 MVP (generic enums
/// are a fast-follow per ADR 0032 D9). Resolve assigns its [`EnumId`] +
/// types interns `Type::Enum` at D.1 (3/N); codegen lowers it at (4/N).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct EnumDecl {
    /// Phase D.6 (1/N) / ADR 0037 D3: `pub` exports this type across
    /// modules; un-`pub` is module-private. No-op single-file.
    pub visibility: Visibility,
    pub name: String,
    pub name_span: Span,
    pub variants: Vec<VariantDecl>,
    pub span: Span,
}

/// Phase D.1 / ADR 0032: one variant of an [`EnumDecl`]. `payloads` is
/// empty for a unit variant, else the positional payload field types.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct VariantDecl {
    pub name: String,
    pub name_span: Span,
    pub payloads: Vec<TypeExpr>,
    pub span: Span,
}

/// Phase D.1 / ADR 0032: one arm of a `match` expression —
/// `pattern => body`.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct MatchArm {
    pub pattern: Pattern,
    pub body: Expr,
    pub span: Span,
}

/// Phase D.1 / ADR 0032: a `match`-arm pattern. The D.1 MVP supports a
/// qualified variant pattern (`Enum::Variant` / `Enum::Variant(b1, b2)`,
/// binding the payload positionally) and the `_` wildcard. Nested / or-
/// / literal patterns are out of scope per ADR 0032 D10.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Pattern {
    /// `Enum::Variant` (unit) or `Enum::Variant(b1, b2)` (binds the
    /// payload fields positionally; a binding may be `_`).
    Variant {
        enum_name: String,
        enum_name_span: Span,
        variant: String,
        variant_span: Span,
        bindings: Vec<Spanned<String>>,
        span: Span,
    },
    /// The `_` wildcard (catch-all).
    Wildcard(Span),
}

/// A C3.0+ top-level effect declaration per ADR 0019 D4:
/// `effect Name { op_decl; op_decl; ... }`. The operations
/// declared inside are *invocable* via `perform Name.op(args)`
/// once handler runtime lands (ADR 0020); at C3.0 / C3.1 /
/// C3.2 they're declared-but-not-invokable per ADR 0019 D8.
/// Empty effect declarations (no ops) are allowed.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct EffectDecl {
    /// Phase D.6 (1/N) / ADR 0037 D3: `pub` exports this effect across
    /// modules; un-`pub` is module-private. No-op single-file.
    pub visibility: Visibility,
    pub name: String,
    pub name_span: Span,
    pub ops: Vec<OpDecl>,
    pub span: Span,
}

/// A single operation declaration inside an
/// [`EffectDecl`]: `op_name(p1: T, ...) -> RetT` (the return
/// type is optional — defaults to `i64` if omitted, matching the
/// Sentinel-Mini Phase B convention). Operations cannot themselves
/// carry effect rows at C3.0 (a perform-site discharges into the
/// surrounding effect-row; the op's own row is fixed by the
/// effect it belongs to).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct OpDecl {
    pub name: String,
    pub name_span: Span,
    pub params: Vec<Param>,
    /// Optional return type. `None` means the op returns the unit-
    /// equivalent (currently `i64` per Sentinel's lack of a unit
    /// type; revisits once unit lands).
    pub return_type: Option<TypeExpr>,
    pub span: Span,
}

/// A C4.1+ top-level class declaration per ADR 0021 D1 + ADR
/// 0022 D1: `class Name { field_decl* init_decl? method_decl* }`.
/// At C4.1 minimum each class carries `fields`, an optional `init`,
/// and `methods`; generic classes (`class Pair<A, B>`) are deferred
/// per ADR 0022 D1. Per ADR 0022 D4 a class with at least one
/// field MUST have an `init`; classes with zero fields may omit it
/// (typed AST synthesises an empty `init() { }`). The parser does
/// not enforce this — the typing layer does. Class items are
/// pre-grouped here (fields / init / methods are each in their own
/// vec) rather than interleaved, simplifying downstream passes.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ClassDecl {
    pub name: String,
    pub name_span: Span,
    pub fields: Vec<ClassField>,
    pub init: Option<InitDef>,
    pub methods: Vec<MethodDef>,
    /// C4.3 / ADR 0021 D6: delegation declarations. Each
    /// `delegate field: T to Trait;` synthesizes a default impl
    /// of `Trait` for the enclosing class that forwards every
    /// trait method to `self.field.method(args)`. Resolved at
    /// resolve time per the C4.3 design (treat delegation as
    /// syntactic sugar over impl + field).
    pub delegates: Vec<DelegateDecl>,
    pub span: Span,
}

/// A single field declaration inside a class body: `('pub')? 'let'
/// name ':' type ';'`. Mirrors [`StructField`] but adds optional
/// visibility (`pub`). Visibility is parsed at C4.1 but enforcement
/// is deferred to Phase C5's module system per ADR 0022 D2.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ClassField {
    pub visibility: Visibility,
    pub name: String,
    pub name_span: Span,
    pub ty: TypeExpr,
    pub span: Span,
}

/// The `init(args) { body }` constructor declaration inside a
/// class body per ADR 0022 D4. Per the same D-decision, `init`
/// has no return-type annotation — it implicitly returns the
/// fully-constructed class. The body must definite-assign every
/// declared field before returning (enforced at C4.1 type-check
/// via the new dataflow analysis).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct InitDef {
    pub visibility: Visibility,
    pub params: Vec<Param>,
    pub body: Block,
    pub span: Span,
}

/// A method declaration inside a class body per ADR 0022 D3.
/// Differs from [`FnDef`] only by the implicit `self: &Self` /
/// `self: &mut Self` first parameter, captured here as the
/// `self_kind` field. The remaining `params` exclude the `self`
/// parameter (the typing layer synthesises it). Methods may
/// declare an `effect_row` exactly as free fns do.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct MethodDef {
    pub visibility: Visibility,
    pub name: String,
    pub name_span: Span,
    pub self_kind: SelfKind,
    pub params: Vec<Param>,
    pub return_type: TypeExpr,
    pub effect_row: Vec<Spanned<String>>,
    pub body: Block,
    pub span: Span,
}

/// A C4.3+ delegation declaration inside a class body per ADR
/// 0021 D6: `('pub')? 'delegate' Ident ':' type 'to' Ident ';'`.
/// Combines a field declaration (`field_name: ty`) with a trait
/// auto-forwarding to that field. At resolve time, each delegate
/// expands into:
///   1. A regular [`ClassField`]-equivalent (the inner field).
///   2. A synthesized default impl of `trait_name` for the
///      enclosing class — one method per trait method, body
///      `self.field_name.method(args)`.
///
/// `to` is a positional Ident (the C4.0 lexer keeps it as a plain
/// Ident per the smallest-surface principle); the parser
/// recognises it after the type-expression.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct DelegateDecl {
    pub visibility: Visibility,
    /// Inner field name (e.g., `writer` in
    /// `delegate writer: FileSink to Writer;`).
    pub field_name: String,
    pub field_name_span: Span,
    /// The inner field's type (e.g., `FileSink`).
    pub ty: TypeExpr,
    /// The trait whose methods get auto-forwarded.
    pub trait_name: String,
    pub trait_name_span: Span,
    /// Span covering the full `delegate ... ;` form.
    pub span: Span,
}

/// A C4.2+ top-level trait declaration per ADR 0021 D4 + ADR
/// 0023 D1: `trait Name { method_sig* }`. Trait methods declare
/// signatures only (no default bodies at C4.2 minimum per ADR
/// 0023 D2). Empty trait declarations are allowed structurally
/// — they're marker traits at C4.2. Resolve / types / codegen
/// at C4.2 (1/N) leave traits untouched; (2/N) brings them up.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct TraitDecl {
    /// Phase D.6 (1/N) / ADR 0037 D3: `pub` exports this trait across
    /// modules; un-`pub` is module-private. No-op single-file.
    pub visibility: Visibility,
    pub name: String,
    pub name_span: Span,
    pub methods: Vec<TraitMethodSig>,
    pub span: Span,
}

/// A single method signature inside a trait declaration per ADR
/// 0023 D2. Same shape as [`MethodDef`] but with no body —
/// terminated by `;` at the source level. The first parameter
/// is the mandatory `self: &Self` / `self: &mut Self` receiver
/// per ADR 0022 D3.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct TraitMethodSig {
    pub name: String,
    pub name_span: Span,
    pub self_kind: SelfKind,
    pub params: Vec<Param>,
    pub return_type: TypeExpr,
    pub effect_row: Vec<Spanned<String>>,
    pub span: Span,
}

/// A C4.2+ top-level impl declaration per ADR 0021 D5 + ADR
/// 0023 D3/D4: `impl name? as Trait for Type { method_def* }`.
/// When `name` is `None`, this is the default impl for
/// `(Trait, Type)` in the current scope. When `Some`, it's a
/// named impl that coexists with the default + other named
/// impls per ADR 0021 D7's scope-local coherence.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ImplDecl {
    /// `None` for the default impl; `Some(name)` for a named impl.
    pub name: Option<String>,
    /// Span of just the impl name when present, useful for
    /// `DuplicateImplName` diagnostics.
    pub name_span: Option<Span>,
    pub trait_name: String,
    pub trait_name_span: Span,
    pub type_name: String,
    pub type_name_span: Span,
    pub methods: Vec<ImplMethodDef>,
    pub span: Span,
}

/// A single method implementation inside an impl block per ADR
/// 0023 D3. Same shape as [`MethodDef`] (visibility + self_kind +
/// params + return_type + effect_row + body) but parsed in the
/// context of an impl rather than a class. Stored separately so
/// downstream passes can iterate impl methods without confusion
/// with class methods.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ImplMethodDef {
    pub visibility: Visibility,
    pub name: String,
    pub name_span: Span,
    pub self_kind: SelfKind,
    pub params: Vec<Param>,
    pub return_type: TypeExpr,
    pub effect_row: Vec<Spanned<String>>,
    pub body: Block,
    pub span: Span,
}

/// The receiver kind of a method's `self` parameter per ADR 0022
/// D3 + ADR 0017 D6's borrow rules. `Shared` for `self: &Self`
/// (read-only access); `Exclusive` for `self: &mut Self`
/// (write access). `Owned` (`self: Self`) is reserved for a
/// later sub-phase — by-value methods are uncommon at C4.1 and
/// would interact with the C2 drop machinery non-trivially.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SelfKind {
    Shared,
    Exclusive,
}

/// Visibility marker per ADR 0022 D2. Parsed at C4.1 but
/// effectively a no-op until Phase C5 adds the module system.
/// `Private` is the default (no `pub` prefix); `Public` is
/// emitted when the prefix is present.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Visibility {
    Private,
    Public,
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
    /// `&T` (shared) or `&mut T` (exclusive) reference type per ADR
    /// 0017 D1. The parser accepts any inner; the type checker
    /// rejects nested refs (`&&T` → [`TypeError::NestedRef`]) and
    /// refs in array elements / struct fields
    /// ([`TypeError::RefInArray`] / [`TypeError::RefInStructField`])
    /// since first-class refs need named regions per ADR 0017 D7 /
    /// D12.
    Ref {
        mutable: bool,
        inner: Box<TypeExpr>,
    },
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
    /// `secret T` — the C3 / ADR 0019 D5 secret-qualifier type
    /// prefix. Nested `secret secret T` is rejected at parse
    /// (`ParseError::DoubleSecret`). The parser accepts the form
    /// at C3.0; the type checker rejects with
    /// `TypeError::SecretNotYet` until C3.1 ships
    /// `Type::Secret(SecretId)` + the five static CT rejections.
    Secret(Box<TypeExpr>),
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
            // ADR 0058: render the literal's value (`{:?}` keeps a decimal
            // point so the rendered form re-lexes as a float, e.g. `1.0`).
            ExprKind::FloatLit(bits) => write!(f, "{:?}", f64::from_bits(*bits)),
            ExprKind::BoolLit(b) => write!(f, "{b}"),
            ExprKind::NullLit => write!(f, "null"),
            // D.2 / ADR 0033 D2: literals render as their decoded
            // bytes — a string IS a `[u8]`, so the bytes are the truth.
            ExprKind::CharLit(b) => write!(f, "(char {b})"),
            ExprKind::StringLit(bytes) => {
                write!(f, "(string")?;
                for b in bytes {
                    write!(f, " {b}")?;
                }
                write!(f, ")")
            }
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
            ExprKind::Declassify(inner) => {
                write!(f, "(declassify {})", inner.kind)
            }
            ExprKind::Cast(inner, te) => {
                write!(f, "(cast {} {})", inner.kind, te.kind)
            }
            ExprKind::Handle { body, arms, return_arm } => {
                write!(f, "(handle {}", body.kind)?;
                for arm in arms {
                    write!(f, " (arm {}.{} (", arm.effect.kind, arm.op.kind)?;
                    let mut first = true;
                    for p in &arm.param_names {
                        if !first {
                            write!(f, " ")?;
                        }
                        first = false;
                        write!(f, "{}", p.kind)?;
                    }
                    write!(f, ") {})", arm.body.kind)?;
                }
                if let Some(ra) = return_arm {
                    write!(f, " (return {} {})", ra.value_name.kind, ra.body.kind)?;
                }
                write!(f, ")")
            }
            ExprKind::Perform { effect, op, args } => {
                write!(f, "(perform {}.{}", effect.kind, op.kind)?;
                for a in args {
                    write!(f, " {}", a.kind)?;
                }
                write!(f, ")")
            }
            ExprKind::MethodCall { target, method, args, .. } => {
                write!(f, "(. {} {}", target.kind, method)?;
                for a in args {
                    write!(f, " {}", a.kind)?;
                }
                write!(f, ")")
            }
            ExprKind::ClassInit { class_name, args, .. } => {
                write!(f, "({class_name}::init")?;
                for a in args {
                    write!(f, " {}", a.kind)?;
                }
                write!(f, ")")
            }
            ExprKind::QualifiedCall { impl_name, method, args, .. } => {
                write!(f, "({impl_name}::{method}")?;
                for a in args {
                    write!(f, " {}", a.kind)?;
                }
                write!(f, ")")
            }
            ExprKind::Scope { mode, body } => {
                let mode_str = match mode {
                    ScopeMode::Concurrent => "concurrent",
                };
                write!(f, "(scope-{mode_str} {})", **body)
            }
            ExprKind::Spawn { call_expr } => {
                write!(f, "(spawn {})", call_expr.kind)
            }
            ExprKind::Await { task_expr } => {
                write!(f, "(await {})", task_expr.kind)
            }
            ExprKind::Match { scrutinee, arms } => {
                write!(f, "(match {}", scrutinee.kind)?;
                for arm in arms {
                    write!(f, " (")?;
                    match &arm.pattern {
                        Pattern::Variant { enum_name, variant, bindings, .. } => {
                            write!(f, "{enum_name}::{variant}")?;
                            for b in bindings {
                                write!(f, " {}", b.kind)?;
                            }
                        }
                        Pattern::Wildcard(_) => write!(f, "_")?,
                    }
                    write!(f, " {})", arm.body.kind)?;
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
            StmtKind::Let { mutable, name, ty_annot, value, .. } => {
                let kw = if *mutable { "let-mut" } else { "let" };
                match ty_annot {
                    Some(ty) => write!(f, "({kw} {name}: {} {})", ty.kind, value.kind),
                    None => write!(f, "({kw} {name} {})", value.kind),
                }
            }
            StmtKind::Assign { target, value } => {
                write!(f, "(assign {} {})", target.kind, value.kind)
            }
            StmtKind::While { cond, body } => {
                write!(f, "(while {} {})", cond.kind, body)
            }
            StmtKind::Break => write!(f, "(break)"),
            StmtKind::Continue => write!(f, "(continue)"),
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
            TypeExprKind::Ref { mutable, inner } => {
                if *mutable {
                    write!(f, "&mut {}", inner.kind)
                } else {
                    write!(f, "&{}", inner.kind)
                }
            }
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
            TypeExprKind::Secret(inner) => write!(f, "secret {}", inner.kind),
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

/// Program prints each top-level item on its own line. Effects
/// come first (declaration order, ADR 0019 D4), then structs,
/// then fns. Each is rendered by its own Display impl.
impl fmt::Display for Program {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut first = true;
        for u in &self.uses {
            if !first {
                writeln!(f)?;
            }
            first = false;
            write!(f, "{u}")?;
        }
        for e in &self.effects {
            if !first {
                writeln!(f)?;
            }
            first = false;
            write!(f, "{e}")?;
        }
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

/// `(effect Name (op_name (params) -> ret_ty) ...)`.
impl fmt::Display for EffectDecl {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "(effect {}", self.name)?;
        for op in &self.ops {
            write!(f, " {op}")?;
        }
        write!(f, ")")
    }
}

/// `(op_name (params) -> ret_ty)`; ret_ty omitted if not declared.
impl fmt::Display for OpDecl {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "({}", self.name)?;
        write!(f, " (")?;
        let mut first = true;
        for p in &self.params {
            if !first {
                write!(f, " ")?;
            }
            first = false;
            write!(f, "{}: {}", p.name, p.ty.kind)?;
        }
        write!(f, ")")?;
        if let Some(rt) = &self.return_type {
            write!(f, " -> {}", rt.kind)?;
        }
        write!(f, ")")
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
        write!(f, ") -> {}", self.return_type.kind)?;
        if !self.effect_row.is_empty() {
            write!(f, " !")?;
            for e in &self.effect_row {
                write!(f, " {}", e.kind)?;
            }
        }
        write!(f, " {})", self.body)
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
    fn display_char_lit() {
        // D.2 / ADR 0033 D2: a char literal renders its decoded byte.
        let e = Spanned { kind: ExprKind::CharLit(b'A'), span: 0..3 };
        assert_eq!(e.to_string(), "(char 65)");
    }

    #[test]
    fn display_string_lit() {
        // D.2 / ADR 0033 D2: a string literal renders its decoded bytes
        // (`"let"` → 108 101 116). An empty string renders `(string)`.
        let e = Spanned { kind: ExprKind::StringLit(vec![108, 101, 116]), span: 0..5 };
        assert_eq!(e.to_string(), "(string 108 101 116)");
        let empty = Spanned { kind: ExprKind::StringLit(vec![]), span: 0..2 };
        assert_eq!(empty.to_string(), "(string)");
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
                mutable: false,
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
                mutable: false,
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
            visibility: Visibility::Private,
            name: "main".to_string(),
            name_span: 0..4,
            type_params: vec![],
            params: vec![],
            return_type: ty_i64(0..3),
            effect_row: vec![],
            is_export_c: false,
            body: Block { stmts: vec![], tail, span: body_span.clone() },
            span: 0..body_span.end,
        }
    }

    #[test]
    fn display_program_one_main() {
        let p = Program {
            uses: vec![],
            fns: vec![main_fn(lit(42, 0..2))],
            structs: vec![],
            effects: vec![],
            classes: vec![],
            traits: vec![],
            impls: vec![],
            enums: vec![],
            externs: vec![],
            span: 0..2,
        };
        assert_eq!(p.to_string(), "(fn main () -> i64 (block 42))");
    }

    #[test]
    fn display_program_two_fns() {
        let double = FnDef {
            visibility: Visibility::Private,
            name: "double".to_string(),
            name_span: 0..6,
            type_params: vec![],
            params: vec![Param {
                mutable: false,
                name: "x".to_string(),
                span: 0..1,
                ty: ty_i64(0..3),
            }],
            return_type: ty_i64(0..3),
            effect_row: vec![],
            is_export_c: false,
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
        let p = Program {
            uses: vec![],
            fns: vec![double, main],
            structs: vec![],
            effects: vec![],
            classes: vec![],
            traits: vec![],
            impls: vec![],
            enums: vec![],
            externs: vec![],
            span: 0..20,
        };
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
            visibility: Visibility::Private,
            name: "add".to_string(),
            name_span: 0..3,
            type_params: vec![],
            params: vec![
                Param { mutable: false, name: "a".to_string(), span: 0..1, ty: ty_i64(0..3) },
                Param { mutable: false, name: "b".to_string(), span: 0..1, ty: ty_i64(0..3) },
            ],
            return_type: ty_i64(0..3),
            effect_row: vec![],
            is_export_c: false,
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
                mutable: false,
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
            visibility: Visibility::Private,
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
            visibility: Visibility::Private,
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

    // ----- C2.0.2 / ADR 0017: refs, mut, deref, assignment -----

    #[test]
    fn display_unary_ref() {
        let e = Spanned {
            kind: ExprKind::Unary(
                UnaryOp::Ref,
                Box::new(Spanned { kind: ExprKind::Var("x".to_string()), span: 1..2 }),
            ),
            span: 0..2,
        };
        assert_eq!(e.to_string(), "(& x)");
    }

    #[test]
    fn display_unary_ref_mut() {
        let e = Spanned {
            kind: ExprKind::Unary(
                UnaryOp::RefMut,
                Box::new(Spanned { kind: ExprKind::Var("x".to_string()), span: 5..6 }),
            ),
            span: 0..6,
        };
        assert_eq!(e.to_string(), "(&mut x)");
    }

    #[test]
    fn display_unary_deref() {
        let e = Spanned {
            kind: ExprKind::Unary(
                UnaryOp::Deref,
                Box::new(Spanned { kind: ExprKind::Var("p".to_string()), span: 1..2 }),
            ),
            span: 0..2,
        };
        assert_eq!(e.to_string(), "(* p)");
    }

    #[test]
    fn unaryop_ref_symbols() {
        assert_eq!(UnaryOp::Ref.symbol(), "&");
        assert_eq!(UnaryOp::RefMut.symbol(), "&mut");
        assert_eq!(UnaryOp::Deref.symbol(), "*");
    }

    #[test]
    fn display_type_expr_ref() {
        let inner = ty_i64(1..4);
        let r = Spanned {
            kind: TypeExprKind::Ref { mutable: false, inner: Box::new(inner) },
            span: 0..4,
        };
        assert_eq!(r.kind.to_string(), "&i64");
    }

    #[test]
    fn display_type_expr_ref_mut() {
        let inner = ty_i64(5..8);
        let r = Spanned {
            kind: TypeExprKind::Ref { mutable: true, inner: Box::new(inner) },
            span: 0..8,
        };
        assert_eq!(r.kind.to_string(), "&mut i64");
    }

    #[test]
    fn display_let_mut_no_annotation() {
        let s = Spanned {
            kind: StmtKind::Let {
                mutable: true,
                name: "x".to_string(),
                name_span: 8..9,
                ty_annot: None,
                value: lit(5, 12..13),
            },
            span: 0..14,
        };
        assert_eq!(s.to_string(), "(let-mut x 5)");
    }

    #[test]
    fn display_assign_stmt() {
        let target = Spanned { kind: ExprKind::Var("x".to_string()), span: 0..1 };
        let value = lit(7, 4..5);
        let s = Spanned {
            kind: StmtKind::Assign { target, value },
            span: 0..6,
        };
        assert_eq!(s.to_string(), "(assign x 7)");
    }

    #[test]
    fn display_program_with_struct_and_fn() {
        let s = StructDecl {
            visibility: Visibility::Private,
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
            uses: vec![],
            fns: vec![main_fn(lit(7, 0..1))],
            structs: vec![s],
            effects: vec![],
            classes: vec![],
            traits: vec![],
            impls: vec![],
            enums: vec![],
            externs: vec![],
            span: 0..30,
        };
        // Structs first, then fns.
        assert_eq!(
            p.to_string(),
            "(struct P (x: i64))\n(fn main () -> i64 (block 7))"
        );
    }

    // ----- C3.4 / ADR 0020: handle + perform display -----

    fn spanned_name(s: &str, span: Span) -> Spanned<String> {
        Spanned { kind: s.to_string(), span }
    }

    #[test]
    fn display_perform_zero_args() {
        let e = Spanned {
            kind: ExprKind::Perform {
                effect: spanned_name("Io", 8..10),
                op: spanned_name("read", 11..15),
                args: vec![],
            },
            span: 0..17,
        };
        assert_eq!(e.to_string(), "(perform Io.read)");
    }

    #[test]
    fn display_perform_with_args() {
        let e = Spanned {
            kind: ExprKind::Perform {
                effect: spanned_name("Io", 8..10),
                op: spanned_name("log", 11..14),
                args: vec![lit(42, 15..17)],
            },
            span: 0..18,
        };
        assert_eq!(e.to_string(), "(perform Io.log 42)");
    }

    #[test]
    fn display_handle_basic() {
        let body = Box::new(lit(42, 7..9));
        let arm = HandlerArm {
            effect: spanned_name("Io", 17..19),
            op: spanned_name("read", 20..24),
            param_names: vec![spanned_name("k", 25..26)],
            body: lit(7, 31..32),
            span: 17..32,
        };
        let e = Spanned {
            kind: ExprKind::Handle {
                body,
                arms: vec![arm],
                return_arm: None,
            },
            span: 0..33,
        };
        assert_eq!(e.to_string(), "(handle 42 (arm Io.read (k) 7))");
    }

    #[test]
    fn display_handle_with_return_arm() {
        let body = Box::new(lit(5, 7..8));
        let arm = HandlerArm {
            effect: spanned_name("Io", 0..2),
            op: spanned_name("log", 3..6),
            param_names: vec![spanned_name("msg", 7..10), spanned_name("k", 12..13)],
            body: Spanned { kind: ExprKind::Var("msg".to_string()), span: 17..20 },
            span: 0..20,
        };
        let return_arm = ReturnArm {
            value_name: spanned_name("v", 22..23),
            body: Spanned { kind: ExprKind::Var("v".to_string()), span: 27..28 },
            span: 22..28,
        };
        let e = Spanned {
            kind: ExprKind::Handle {
                body,
                arms: vec![arm],
                return_arm: Some(Box::new(return_arm)),
            },
            span: 0..30,
        };
        assert_eq!(
            e.to_string(),
            "(handle 5 (arm Io.log (msg k) msg) (return v v))"
        );
    }
}
