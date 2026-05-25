# Introduction to Sentinel programming

This guide teaches the basics of writing and running Sentinel programs
using the current bootstrap compiler (`snc`). It covers everything in
the language as of **Phase C0**: function definitions, `let`-bindings,
arithmetic, `if`/`else`, blocks, and `print`. That's enough to write
small numerical programs that compile to native binaries via LLVM.

It is **not** a tour of the eventual Sentinel language. The type
system, `secret` qualifier, effects, regions, and the broker
integration are all under construction — see
[`SENTINEL_DESIGN.md`](SENTINEL_DESIGN.md) and
[`STATE.md`](STATE.md) for the full picture. This guide stays close
to what you can compile and run today.

If anything in this guide disagrees with the actual compiler,
the compiler wins. File the discrepancy as a docs bug.

## Contents

- [What you can do today](#what-you-can-do-today)
- [Setup](#setup)
- [Your first program](#your-first-program)
- [Variables and `let`](#variables-and-let)
- [Arithmetic](#arithmetic)
- [Functions](#functions)
- [Blocks are expressions](#blocks-are-expressions)
- [`if`/`else`](#ifelse)
- [Comments](#comments)
- [A worked example](#a-worked-example)
- [What doesn't work yet](#what-doesnt-work-yet)
- [Where Sentinel is headed](#where-sentinel-is-headed)

## What you can do today

The C0 language has exactly one type: 64-bit signed integer (`i64`).
Every value, parameter, return type, and `let`-binding is `i64`.
There is no `bool`, no string, no float, no struct, no array. The
type system arrives at **Phase C1.3** (per
[ADR 0011](decisions/0011-phase-c1-kickoff-and-type-system-plan.md)).

What you get today is enough to write programs like:

```sentinel
fn double(x) { x * 2 }

fn pick(cond, a, b) {
    if cond { a } else { b }
}

fn main() {
    let x = 5;
    let y = pick(x, double(x), 0);
    print(y)
}
```

That's the `c05_go_no_go` acceptance program. It prints `10` and
exits with code 0.

## Setup

You need Rust stable (1.80+) and LLVM 18. The compiler is currently
macOS-only because `.cargo/config.toml` hard-codes Homebrew paths;
cross-platform support is a future concern.

```bash
brew install llvm@18
echo 'export LLVM_SYS_180_PREFIX=/opt/homebrew/opt/llvm@18' >> ~/.zshrc
git clone https://github.com/arcanii/Sentinel-lang.git
cd Sentinel-lang
cargo build --workspace
```

After the workspace builds, the compiler driver is at
`target/debug/snc`. You can run it directly or via cargo:

```bash
cargo run --bin snc -- help
```

`snc` has two subcommands:

| Command                          | What it does                                                         |
| -------------------------------- | -------------------------------------------------------------------- |
| `snc parse <file>`               | Lex and parse `<file>`; pretty-print the AST as an s-expression.      |
| `snc build <file> [-o <output>]` | Compile `<file>` to a native executable. Output defaults to `<stem>`. |

## Your first program

Sentinel source files use the `.sentinel` extension. The smallest
valid program is a `main` function with a tail expression:

```sentinel
// hello.sentinel
fn main() {
    0
}
```

Compile and run it:

```bash
cargo run --bin snc -- build hello.sentinel -o hello
./hello
echo "exit=$?"     # exit=0
```

A program's exit code is the value of `main`'s tail expression
(truncated from `i64` to the C ABI's 32-bit `int`). So
`fn main() { 42 }` exits with code 42. This is a temporary
convention — once Sentinel grows a real return-channel story
(strings, structured output), exit codes will go back to meaning
"success or failure."

To print numbers to stdout, use the built-in `print`:

```sentinel
fn main() {
    print(42)
}
```

Compile, run, observe:

```bash
cargo run --bin snc -- build hello.sentinel -o hello
./hello              # writes "42\n" to stdout
echo "exit=$?"       # exit=0
```

`print(x)` writes `x` to stdout as ASCII decimal followed by a
newline, and returns 0. So the program above prints `42` and
then exits with code 0 (because `print`'s return value of 0 is
`main`'s tail expression).

## Variables and `let`

`let` introduces an immutable binding inside a function or block:

```sentinel
fn main() {
    let x = 5;
    let y = x + 3;
    print(y)              // 8
}
```

Each `let` is a statement and must end with a semicolon. The
binding is in scope for the rest of the enclosing block. There
is no shadowing — declaring `let x = 1; let x = 2;` in the same
block is a compile error (`RedeclaredVariable`). Block-scoped
shadowing arrives with the type system later.

Bindings are not yet mutable. There is no `let mut` or
assignment statement; once you `let x = ...`, `x` keeps that
value for its scope. Mutability is a deliberate omission until
the type system can reason about it correctly.

## Arithmetic

The four arithmetic operators work as you'd expect:

| Operator | Meaning           | Example   |
| -------- | ----------------- | --------- |
| `+`      | addition          | `1 + 2`   |
| `-`      | subtraction       | `5 - 3`   |
| `*`      | multiplication    | `4 * 7`   |
| `/`      | integer division  | `9 / 4`   |
| `-` (unary) | negation       | `-5`      |

Precedence is the conventional one: unary `-` binds tightest,
then `* /`, then `+ -`. All binary operators are left-associative.
Parentheses override precedence:

```sentinel
fn main() {
    print(1 + 2 * 3)        // 7  — multiplication first
    print((1 + 2) * 3)      // 9
    print(-2 * 3)           // -6 — unary minus binds tighter than *
    print(1 - -2)           // 3  — second `-` is unary
}
```

Division is integer division (`9 / 4 == 2`), and dividing by zero
is undefined at runtime — the compiler does not yet insert a
check. Don't divide by zero.

## Functions

A function definition is `fn name(params) { body }`. The body
is a block; its tail expression is the return value. Every
function must have a body — there are no forward declarations
separate from definitions.

```sentinel
fn double(x) {
    x * 2
}

fn add(a, b) {
    a + b
}

fn main() {
    print(double(7))         // 14
    print(add(3, 4))         // 7
}
```

A program is one or more `fn` definitions at the top level, and
exactly one of them must be named `main`. `main` takes no
parameters and is the entry point.

Functions can refer to each other in either direction — forward
references are fine because the compiler does a two-pass lower
(signatures first, then bodies):

```sentinel
fn main() {
    print(triple(4))         // 12
}

fn triple(x) {
    x * 3
}
```

`print` is a reserved function name and is provided by the
runtime. You can't define your own `fn print(x) { ... }`; trying
to is a compile error.

## Blocks are expressions

A `{ ... }` block can appear anywhere an expression can. Its
value is its tail expression:

```sentinel
fn main() {
    let r = { let y = 4; y + 1 };
    print(r * 2)             // 10
}
```

Blocks contain zero or more `;`-terminated statements followed
by exactly one tail expression (no `;` at the end). Empty blocks
are not allowed; `{ }` is a parse error. The trailing-expression
requirement is a deliberate choice — it forces every block to
have a value, which keeps the C1 type system honest.

You can mix expression-statements and `let`s freely:

```sentinel
fn main() {
    let x = 5;
    print(x);                 // expression-statement: evaluate, discard
    print(x * 2);             //   "
    x + 1                     // tail: this is main's return value
}
```

The program above prints `5` and `10`, then exits with code 6.

## `if`/`else`

`if` is an expression. The condition is evaluated, the
corresponding branch's block runs, and the block's tail
expression is the value of the whole `if`:

```sentinel
fn main() {
    let x = 5;
    let y = if x { x * 2 } else { 0 };
    print(y)                  // 10
}
```

Two important constraints for C0:

1. **`else` is mandatory.** Every `if` needs an `else` branch.
   This will relax once the type system arrives and can prove
   that a missing `else` is OK (e.g., when both branches type
   to a `?T` and `None` is the implicit fallthrough).

2. **The condition uses C-style truthy.** Since there is no
   `bool` yet, the condition is an `i64` and `0` is false; any
   other value is true. So `if x { ... }` reads as "if x is
   nonzero." This will change at C1.3 when `bool` lands and
   conditions become strictly `bool`-typed.

`else if` chains work the obvious way and parse as nested
`if`/`else`:

```sentinel
fn classify(x) {
    if x {
        if x - 100 { 1 } else { 2 }
    } else {
        3
    }
}
```

Because `if` is an expression at the top of the expression grammar,
it can't sit bare inside arithmetic. Wrap it in parens if you
need to:

```sentinel
fn main() {
    let r = (if 1 { 2 } else { 3 }) + 4;
    print(r)                  // 6
}
```

## Comments

Line comments start with `//` and run to end-of-line. There are
no block comments yet.

```sentinel
// This is a comment.
fn main() {
    let x = 5;        // and so is this
    print(x)
}
```

## A worked example

Here's the `c05_go_no_go.sentinel` fixture, the canonical "C0 is
complete" program:

```sentinel
fn double(x) { x * 2 }

fn pick(cond, a, b) {
    if cond { a } else { b }
}

fn main() {
    let x = 5;
    let y = pick(x, double(x), 0);
    print(y)
}
```

Reading through it:

1. `fn double(x) { x * 2 }` — a one-argument function returning
   twice its input. The body is a single expression, the
   block's tail.
2. `fn pick(cond, a, b) {...}` — a three-argument function that
   returns `a` if `cond` is nonzero, otherwise `b`. This is the
   conditional move from the C standard library, written as
   ordinary control flow.
3. `fn main() {...}` — the entry point. Binds `x = 5`, computes
   `y = pick(5, double(5), 0)`, prints `y`.
4. `double(5)` evaluates to `10`. `pick(5, 10, 0)` evaluates to
   `10` because `5` is nonzero (truthy). So `y == 10`. `print(10)`
   writes `10` to stdout and returns 0. `main`'s tail expression
   is the return of `print`, so the program exits 0.

To run it yourself:

```bash
cargo run --bin snc -- build tests/pass/c05_go_no_go.sentinel -o /tmp/go_no_go
/tmp/go_no_go     # prints "10", exits 0
```

The other 21 fixtures under [`tests/pass/`](../tests/pass/) cover
every C0 feature individually — read them as a tour of the
surface.

## What doesn't work yet

Sentinel-the-language is deliberately small at C0. The things
below are not bugs, they're roadmap. Most arrive in Phase C1:

- **No types or annotations.** `i64` is everything. `fn f(x: i64)`
  is a parse error today; the annotation grammar lands at
  **C1.2**.
- **No `bool`, `true`, `false`.** Conditions are C-style truthy.
  `bool` arrives at **C1.3**, and `if 5 { ... }` will become a
  type error then.
- **No comparison operators.** `==`, `<`, `>`, `!=` don't exist.
  Use truthy `if x { ... }` to test for nonzero. Comparisons
  arrive at **C1.3** with `bool`.
- **No `&&`, `||`, `!`.** No logical operators yet.
- **No strings.** `print` takes an `i64`. Strings need either a
  pointer type or a built-in `String` type; neither is in C0.
- **No floats, no other integer widths.** `i32` and friends
  arrive at **C1.3**.
- **No mutability.** `let mut`, `=` as assignment, and the
  region/ownership story all arrive at **C2**.
- **No structs, arrays, tuples, enums.** Structs land at
  **C1.4**, arrays at **C1.6**, generics at **C1.7**.
- **No `?T` nullability.** Arrives at **C1.5**.
- **No effects, no `secret`, no broker integration.** These are
  the language-level security thesis; they're the whole point of
  Sentinel but they need the type system underneath them first.
  Effects integrate at **C3**, `secret` at **C3**, broker at
  **C5**.
- **No imports, no modules, no separate compilation.** Every
  program is a single file.
- **No standard library.** `print` is the only built-in. Math
  functions, I/O, collections all wait for the language to
  reach a stable shape.

When you write something that should work and the compiler
disagrees, check this list first. If it looks like it should be
in C0 and isn't working, that's a real bug — open an issue or
check [`STATE.md`](STATE.md) for the authoritative feature list.

## Where Sentinel is headed

Sentinel exists to solve a specific class of problem: the
security failures that dominate modern incidents — supply-chain
attacks, side channels, secret disclosure, untrusted code
execution. The language-level features for those (effects as
capabilities, the `secret` qualifier with a constant-time check,
named regions, the broker as a programmable runtime) are all
*designed* but not yet *implemented* in the production compiler.

What you're using today is the bootstrap pipeline: the smallest
slice of compiler infrastructure that can lex, parse, type-check,
and emit native code. It exists to prove the architecture works.
The security thesis lands as the type system, effect system, and
secret qualifier come up through Phase C1, C2, and C3.

If you want the long version of what Sentinel is *for*, read
[`SENTINEL_SUMMARY.md`](SENTINEL_SUMMARY.md) (one-page pitch),
[`SENTINEL_DESIGN.md`](SENTINEL_DESIGN.md) (full design), and
[`HANDOVER.md`](HANDOVER.md) (the implementation plan).

If you want to follow along as features land,
[`STATE.md`](STATE.md) is updated at every sub-phase boundary.

Welcome aboard.
