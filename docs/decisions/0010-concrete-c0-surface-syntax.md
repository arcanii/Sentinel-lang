# ADR 0010: Concrete C0 surface syntax

Status: ACCEPTED (C0.1 parser at 7e32e8c committed to the D8 arithmetic precedence ladder + D1 decimal-only literals + D3 `//`-only comments + D12 whitespace insensitivity; D2 idents, D4-D7 structure, D8 control-flow half, D9 if/else, D10 calls, D11 `print` await C0.3-0.5 per the Revisit clauses)
Date: 2026-05-25
Related: 0009 (Phase C kickoff; D8 deferred this decision to the
present ADR), 0002 (decide-now-or-later asymmetry that justified the
deferral)

## Context

ADR 0009 D8 deferred the concrete C0 surface syntax to a follow-up
ADR written after C0.0 shipped the lexer, on the reasoning that
parser-targeting questions are easier to argue against a real token
set than a sketch. C0.0 landed at `8f37381` with the token set:

  - Keywords: `let`, `fn`, `if`, `else`
  - Punctuation: `+ - * / = ( ) { } , ; ->`
  - `Ident`: `[A-Za-z_][A-Za-z0-9_]*`
  - `IntLit`: `[0-9]+`
  - Skipped: `[ \t\r\n]+`, `//[^\n]*`

The 1.0 surface target is SENTINEL_DESIGN2.md §15.1, but C0 is a
deliberately small subset: no type system (everything `i64`), no
regions, no effects, no `secret`, no classes, no traits. C0 ships
just enough syntax to get from a source file to a runnable binary
that exercises `let`, arithmetic, `if`/`else`, and function calls
(ADR 0009 D6).

Sentinel-Mini (`crates/sentinel-effects-proto`) is the structural
reference for parser style but diverges from Sentinel proper in
several places relevant here — most notably it is purely expression-
based with no `fn` keyword (uses `let f = \x -> ...` instead) and
no statement/expression distinction. C0 follows Sentinel 1.0's
intended surface where the two diverge.

This ADR pins down the surface for C0.1 onward. C0.0's lexer
already constrained some decisions (which tokens exist); this ADR
fills in the rest (how they compose).

## Decision

Twelve D-numbered sub-decisions.

### D1. Integer literals: decimal only.

C0 accepts `[0-9]+`. No hex (`0x…`), binary (`0b…`), or underscore
separators (`1_000_000`). Each is a one-line lexer change when
ergonomic complaints arise, with no semantic interaction; deferring
costs nothing.

Leading zeros are accepted and decimal-interpreted (`0042` is
`42`). This matches Sentinel-Mini and Rust; it diverges from C's
octal interpretation, deliberately.

### D2. Identifier syntax: as the lexer already accepts.

`[A-Za-z_][A-Za-z0-9_]*`. ASCII only in C0; Unicode identifiers
deferred to C5+ (LSP polish era), in line with Rust's policy on
non-ASCII identifiers.

The reserved set is exactly the four lexer keywords (`let`, `fn`,
`if`, `else`). No additional reserved-but-unused keywords in C0;
this avoids the "we reserved `match` and now somebody named a
variable `match`" backwards-compatibility tax. Words that become
keywords in C1-C5 are added at the sub-phase that needs them.

### D3. Comments: `//` line comments only in C0.

Block comments (`/* … */`) are deferred. The grammar question they
raise — does Sentinel permit nested block comments? — is a 1.0
surface question that does not need to be resolved before C0.1.
Add when needed.

### D4. Top-level structure: programs are `fn` definitions.

A program is `item*`, where `item = fn_def`. The function named
`main` (no parameters, returns `i64` implicitly) is the program
entry point. No top-level `let`, no module declarations, no
`use`/`import` in C0. (Mirrors Rust's top-level shape.)

Functions may reference each other in any order — forward
references are supported, matching ADR 0009 D6's "function calls
(forward-declared, fixed signatures)" phrasing. C0.4's codegen
collects function declarations in pass one, emits bodies in pass
two.

### D5. `fn` syntax: no type annotations in C0.

    fn_def      = 'fn' Ident '(' param_list ')' block
    param_list  = (Ident (',' Ident)*)? ','?

No `: Type` annotations on parameters. No `-> ReturnType`
annotation. Everything is `i64` per ADR 0009. C1 widens the
grammar to support annotations (`fn name(x: T, y: T) -> T block`).
The `->` token already exists in C0.0's lexer specifically to
preserve this future-proofing — `->` is reserved.

Trailing comma in `param_list` is permitted. (Rust convention; low
cost, makes multi-line param lists diff-friendly.)

### D6. Block syntax: `{ stmt* expr }`.

A block contains zero or more statements followed by exactly one
trailing expression. The trailing expression is the block's value.

    block       = '{' stmt* expr '}'

No empty blocks. A function body is a block, so every function has
a trailing expression which is its return value. Without unit type
(`()` in Sentinel 1.0 but not yet in C0), the "no trailing
expression" case has no value to return, so the grammar forbids it.

Blocks are expressions — they can appear anywhere a primary
expression can — which is what makes `if cond block else block` an
expression rather than a statement.

### D7. Statements: `let` bindings and expression-statements.

    stmt        = let_stmt | expr_stmt
    let_stmt    = 'let' Ident '=' expr ';'
    expr_stmt   = expr ';'

`let` requires a trailing `;`. Expression-statements end in `;`.
The trailing block expression (D6) is identified syntactically by
the absence of `;` before the closing `}`.

No `let mut` in C0 (no mutation). Bindings are non-reassignable;
shadowing via re-declaration with the same name in an inner block
is permitted (Rust-style). C0 has no nested scope tooling beyond
nested blocks themselves.

No type annotations on `let` (`let x: T = …`) in C0. Added in C1.

### D8. Expression grammar: standard precedence ladder.

From lowest to highest precedence:

    expr        = if_expr | add_expr
    add_expr    = mul_expr (('+' | '-') mul_expr)*       left-assoc
    mul_expr    = unary    (('*' | '/') unary)*          left-assoc
    unary       = '-' unary | atom
    atom        = IntLit
                | Ident
                | call
                | '(' expr ')'
                | block
    call        = Ident '(' arg_list ')'
    arg_list    = (expr (',' expr)*)? ','?

Left-associative throughout. Unary minus binds tighter than `*`:
`-2 * 3` is `(-2) * 3 = -6`, and `-2 * -3` is `6`.

No comparison operators (`< > <= >= == !=`) in C0. They wait for
C1's type system, when `bool` exists and comparison can produce a
real boolean rather than an i64-as-bool hack.

No bitwise operators (`& | ^ ~ << >>`) in C0. Not needed by D6's
phase-go criterion. Added when there's a use case.

No assignment operators (`+= -=`) and no compound assignment in
C0 — there is no mutation.

Function calls are restricted to `Ident '(' … ')'` (no
higher-order calls like `(some_expr)(args)`). First-class functions
are a C4+ feature; C0's codegen emits direct calls to LLVM
function symbols.

Trailing comma in `arg_list` is permitted (parallels D5).

### D9. `if`/`else`: expression form, mandatory else, C-style truthy.

    if_expr     = 'if' expr block else_branch
    else_branch = 'else' (if_expr | block)

`if` is an expression. Both `then` and `else` are mandatory — without
a unit type to fill the "else-less" hole, single-armed `if` has no
value. Else-if chains parse via the alternative `else if_expr`
production.

The condition is an `i64` expression; **0 is false, any non-zero
value is true**. This is a deliberate, temporary C-style hack to
get a working branching primitive before C1 introduces `bool`.

When C1 lands `bool`, D9 is revisited: the condition becomes
`bool`-typed and `if (5 + 3)` becomes a type error rather than
"true because 8 is non-zero." Existing C0 programs will need to
write `if some_cmp(x, 0)` or `if x != 0` to remain valid. That
migration is acceptable because C0 programs are test fixtures, not
production code.

Alternatives rejected:

  - **Defer `if` to C1.** Rejected: would invalidate ADR 0009 D6's
    C0.4 row and remove the only branching primitive from C0,
    making the C0.5 go/no-go (multi-function program with branching)
    impossible.

  - **Add `bool` as a one-off pre-C1 type.** Rejected: introducing
    a type *would* be the type system. ADR 0009's "no type system
    in C0" is the bigger commitment; the C-style truthiness hack
    is the smaller one.

  - **Restrict condition to literal `0` or `1`.** Rejected: makes
    `if` syntactically useless because no computation can flow into
    the condition without a comparison operator.

### D10. Function calls: direct, by-name, no first-class functions.

    call        = Ident '(' arg_list ')'
    arg_list    = (expr (',' expr)*)? ','?

`Ident` is the function name. The codegen resolves it directly to
an LLVM symbol. Arity mismatches surface at LLVM IR verification
time (codegen-level diagnostic, not parser-level — the parser
accepts any arity, the codegen checks against the function's
declared parameter count).

No method-call syntax (`obj.method(args)`). No qualified calls
(`module::function(args)`). No higher-order calls. All deferred to
later phases.

### D11. The `print` built-in.

`print(x)` writes the `i64` value `x` to stdout as ASCII decimal,
followed by a newline. The compiler treats `print` as a normal
function call; the runtime (`sentinel-runtime`) provides the
implementation as an exported symbol that the linker resolves.
C0.2's codegen emits `declare i64 @sentinel_print(i64)` at the
top of each module and rewrites `print(x)` calls into
`call i64 @sentinel_print(i64 x)`.

The return value of `print` is `0` (no current need for an error
code; refined when there is one). C0 programs that ignore the
return value use `print(x);` as an expression-statement.

The name `print` is **not** reserved at the lexer level — it is an
identifier that codegen happens to recognize. User code is free to
shadow it by defining `fn print(x) { … }`; in C0 with no name
resolution, the user-defined `print` wins by virtue of being
declared in the same module. C1's name resolution formalizes the
runtime-builtins mechanism.

`print` is intentionally the **only** built-in in C0. The phase-go
criterion (ADR 0009 D6, C0.5: "multi-fn program with let/if/
arithmetic produces correct stdout") needs exactly one I/O
primitive. Other I/O (`println` with strings, `read_line`, etc.)
waits until C0 has a string type, which is not in C0.

### D12. Whitespace and program layout.

Whitespace is insignificant beyond token separation. No
indentation-based layout. No required line breaks. Style
conventions (formatter, line-length budget) are not specified by
the language and will be a `rustfmt`-style tool in later phases.

## Reasoning

The decisions cluster around four themes:

**Pin down what C0 needs; defer everything else.** D1 (decimal
only), D3 (`//` only), D8 (no comparisons, no bitwise, no compound
assign), D10 (no higher-order calls), D11 (only `print`) all defer
features that have no payoff at C0.5's go/no-go target. The pattern
is the same one ADR 0009 D1 used for Salsa: if you decide now and
guess wrong, you pay twice; if you defer, you decide once with
more information. The "more information" here is "we have a working
parser and codegen to argue against."

**Surface-future-compatible where it costs nothing.** D5's `->`
token already exists in C0.0's lexer specifically so that
C1's `fn name(x: T) -> T` doesn't require a lexer change. D7's
`let x = expr;` will become `let x: T = expr;` in C1 — a parser
extension, not a rewrite. D2's keyword discipline (no
reserved-but-unused words) keeps the future migration path open
for whichever keywords later phases choose.

**Take the hacky win when the alternative is "do less."** D9's
C-style truthy condition is the clearest example. The alternative
("no `if` in C0 because no `bool`") would make C0.5's go/no-go
impossible. The alternative ("introduce `bool` as a one-off
type") would mean introducing the type system — the very thing
ADR 0009 committed to *not* doing in C0. The C-style hack is the
smallest possible commitment.

**Mirror Sentinel 1.0 where it does not cost C0 anything.** D4
(`fn`-based top-level with `main`), D6 (block-as-expression, Rust-
style), D7 (`let` syntax), D9 (`if` as expression with else-if
chains), D10 (call syntax) all match where SENTINEL_DESIGN2.md
§15.1 points. Diverging would create a migration cost at C1 for
no C0 benefit.

## Consequences

### Positive

- C0.1's parser implementation is well-bounded: a fixed precedence
  ladder, six production rules in expression-land, four in
  statement-land, three at top-level. The hand-written recursive
  descent will fit in a few hundred lines.

- D11 collapses the I/O question to one symbol and one
  codegen-time substitution. No name resolution layer needed in
  C0.

- D5's `->` reservation means C1 can extend the grammar without a
  lexer touch. The pattern (reserve tokens early at the lexer,
  extend the grammar later) generalizes.

- C0.5's go/no-go program (the "multi-fn program with let/if/
  arithmetic produces correct stdout" target) has a concrete form
  the test runner can assert against. See Appendix below.

### Negative

- D9's C-style truthiness is the only intentional ugliness in C0.
  It buys branching at a future migration cost. Migration scope:
  every C0 program that uses `if` will need updates when C1 lands
  `bool`. Mitigated by C0 programs being test fixtures, not
  production code.

- D7's no-mutation rule means C0 programs cannot express loops
  (no `while`, no `for`, and `let` is single-assignment). All C0
  programs are bounded by their static structure. This is fine
  for the C0.5 go/no-go; mutation enters C1+ along with the type
  system.

- D11's user-shadowing of `print` is a small surprise vector
  (user `fn print` silently replaces the built-in). It is fixable
  in C1 via name resolution.

### Neutral

- D2's "no reserved-but-unused keywords" means choosing whether to
  reserve `secret`, `region`, `effect`, `trait`, etc. is a per-
  phase decision rather than a one-time list at C0. This is the
  right shape — premature reservation costs nothing only if you
  pick the right list, and there's no way to predict the right
  list from C0.

- D8's standard precedence ladder is the same one every C-family
  language uses. The decision is mostly to confirm we are not
  doing something exotic (e.g., uniform left-to-right evaluation
  with explicit grouping, à la APL or Smalltalk).

## Alternatives considered

- **`fn`-less surface (Sentinel-Mini-style `let f = \x -> body`).**
  Rejected: SENTINEL_DESIGN2.md §15.1 reads as `fn`-keyword based;
  diverging from 1.0 in C0 creates a migration cost for no benefit.

- **Expression-only language (no statements, no `;`).** Rejected:
  D11's `print` side effect needs a way to be sequenced; without
  statements, sequencing must be encoded with bindings (`let _ =
  print(1); print(2)`) which is uglier than `print(1); print(2)`
  and forces an unused-binding name. Statements are the cheaper
  win.

- **Layout-significant whitespace (Python/Haskell-style).**
  Rejected: D6's brace-delimited blocks match Sentinel 1.0 and
  Rust; layout-sensitivity would diverge for no C0 benefit and
  surfaces edge cases (mixed tabs, line continuation) the parser
  does not need.

- **Require explicit `i64` annotations even though everything is
  `i64`.** Rejected for C0 (overhead with no benefit at this
  scale), but the `->` token reservation in D5 means C1 can pull
  this back in without a lexer change.

## Revisit

This ADR is **PROPOSED** until C0.1's parser lands. At that point
it becomes ACCEPTED and the status line gets a hash-stamped
confirmation in the ADR 0008 / 0009 style.

Per-D revisit triggers:

- **D1** (integer literal forms): revisit if C0 test programs grow
  numbers larger than three or four digits — underscore separators
  become real ergonomics. Hex/binary are revisited at C3 (effects)
  when constant-time code in `secret` contexts wants bit-manipulation
  literals.

- **D3** (block comments): revisit at C1 if any C0 program grows
  large enough to want a comment longer than one line.

- **D5** (no type annotations): revisited at C1.0 when the type
  system arrives. `fn name(x: T, y: T) -> T block` is the planned
  shape; the `->` token already exists.

- **D7** (no mutation): revisited at C2 along with regions and
  ownership. `let mut` (or whatever syntax mutation gets) is a C2
  concern.

- **D8** (no comparisons): revisited at C1 with the type system.
  Comparison operators produce `bool`, which is a C1 type.

- **D9** (C-style truthy): revisited at C1 when `bool` arrives.
  The condition becomes `bool`-typed; C0's C-style truthiness
  becomes a type error that programs must rewrite.

- **D10** (no higher-order calls): revisited at C4 when classes,
  traits, and the function-as-value story land.

- **D11** (`print` as the only built-in, name-collision possible):
  revisited at C1 when name resolution formalizes runtime-provided
  builtins. The shadowing surprise vector closes then.

## Appendix: C0.5 go/no-go target program

For reference, the canonical C0.5 phase-go program might be:

    fn double(x) {
        x * 2
    }

    fn pick(cond, a, b) {
        if cond { a } else { b }
    }

    fn main() {
        let x = 5;
        let y = pick(x, double(x), 0);
        print(y)
    }

Expected stdout: `10\n`. The program exercises every C0 feature:
multi-function definition, forward reference (`main` calls
`double` and `pick` which are defined before), `let` binding,
arithmetic, function call with arguments, `if` as expression,
C-style truthy condition (`x = 5` is non-zero so the `then`
branch fires), and the `print` built-in.

`tests/pass/c05_go_no_go.sentinel` paired with
`tests/pass/c05_go_no_go.stdout` containing `10\n` is the
concrete acceptance fixture for C0.5.
