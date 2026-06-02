# ADR 0039: Phase D self-host port — (2/N) parser-in-Sentinel

Status: **ACCEPTED-WITH-AMENDMENTS** — the second sub-phase of the self-host port
(ADR 0031 D5 / ADR 0038 D9), after the lexer (1/N). Ports the **parser** to
Sentinel: tokens → AST, differentially validated against the Rust `snc` oracle.
The compiler's **largest** stage, so it is explicitly sub-sliced (D6). **(2a) has
LANDED:** the `snc ast` oracle (A1) + the recursive-AST drop gate (A2) + the
parser-structure de-risk (A3) + `selfhost/parser.sentinel`, a recursive-descent
expression parser that matches `snc ast` for integer arithmetic (precedence,
parens, left-assoc, multi-fn) — `tests/selfhost_parse.rs`, leak-free. **(2b) is now
UNDERWAY** — D6's "full expressions" slice is itself sub-sliced (it spans ~28
`ExprKind` variants); **increment-1 (A4)** landed the complete operator-precedence
ladder + scalar atom leaves, **increment-2 (A5)** landed function calls + the
postfix chain (field / index / method), **increment-3 (A6)** landed the `::`
paths (qualified call / class init / unit construction) + array literals,
**increment-4 (A7)** landed `if`-expressions + brace blocks, **increment-5 (A8)**
landed `match` expressions + patterns, and **increment-6 (A9)** landed struct
literals. Remaining (2b) increments + slices (2c)–(2d) per D6 grow it toward the
full corpus; the ADR flips fully as they close. See ## Amendments.

## Amendments (in progress — (2a) + (2b) landing)

- **A1 — `snc ast` oracle landed (D2).** A `snc ast <file>` subcommand
  (`run_ast` + `crates/sentinel-driver/src/ast_dump.rs`) emits a complete,
  regular, fully-tagged S-expr dump (e.g. `(fn main () i64 (block (binop + (int
  1) (binop * (int 2) (int 3)))))`), golden-tested (`tests/ast.rs`); covers the
  function-level grammar (fns + all exprs/stmts/types/patterns), non-fn decls in
  (2d).
- **A2 — recursive-AST drop gate PASSED (D4 cleared).** A recursive-enum AST
  (i64/`[u8]`/recursive payloads) builds + consuming-`match`-walks + drops
  leak-free at AST scale (`tests/pass/selfhost_ast_drop.sentinel`, 0 leaks) — so
  the parser needs **no D.1b payload-ownership fix first**.
- **A3 — parser structure, settled by probe (revises D3/D5).** Two current-Sentinel
  realities shape the build:
  - **`Vec<non-primitive>` is unsupported** (`Vec<Expr>` / `Vec<struct>` →
    `vec_element_not_supported`). So D3's struct-of-arrays applies only to the
    *token stream* (`Vec<i64>`); the **AST is a recursive `Expr` enum returned BY
    VALUE** from parse fns (not an arena/`Vec` of nodes), dumped by a consuming
    recursive `match` (A2's shape).
  - **Refs are not auto-indexable** (`r[i]` on `&Vec`/`&[u8]` →
    `index_on_non_array`), **but explicit deref `(*r)[i]` works** (verified on
    `&Vec<i64>` + `&[u8]`). This is the enabler for D5's recursive descent:
    helpers share the token arrays + `src` by **shared ref + `(*x)[i]`**, with a
    **`&mut i64` cursor**. (Without it the parser would have been blocked on a
    language gap; explicit deref avoids needing one.)
  - Minor: **`match` arms need comma separators even with block bodies**.
- **A4 — (2b) increment-1: the full operator-precedence expression grammar
  (D6 "full expressions", sub-sliced).** D6's (2b) row lists the whole expression
  surface (call / field / index / `if` / `match` / struct-lit / method+qualified
  call / perform / handle / …); rather than land it in one step, (2b) is sub-sliced.
  **Increment-1** grows `selfhost/parser.sentinel` from (2a)'s integer arithmetic to
  the **complete operator-precedence ladder**, mirroring the Rust parser exactly
  (`parse_expr → or → and → cmp → bitor → bitxor → bitand → add → mul → unary →
  atom`) so the AST *tree shape* — hence the `snc ast` dump — matches byte-for-byte:
  logical `|| &&` (short-circuit precedence), the six **non-associative**
  comparisons `== != < <= > >=`, bitwise `| ^ &` (`&`>`^`>`|`), additive `+ -`,
  multiplicative `* /`, prefix unary `- !`, parens — plus the **scalar atom leaves**:
  integer / `true` / `false` / `null` literals + variable references. The `Expr`
  enum gains `Bool(bool)` / `Null` / `Var([u8])` / `Unary(i64, Expr)` and a unified
  `Binary(i64, Expr, Expr)` whose i64 **op-code** encodes both the dump category
  (`binop`/`cmp`/`logic`) and the symbol; the consuming dump maps it back.
  `true`/`false`/`null` lex as identifiers but parse to the literal nodes the oracle
  emits (in-place byte compare, no allocation), **never** `(var …)`. The internal
  tokenizer is extended to longest-match all the new operators. Verified: 26
  differential seeds (every level, incl. two interleaving the whole ladder) match
  `snc ast`; leak-free under `leaks --atExit`. **Deferred to later (2b) increments:**
  postfix (call / index / field / method), `if`/`match` expressions, struct/array
  literals, perform/handle, qualified-call / class-init / scope / spawn / await /
  declassify — i.e. every `Expr` form that is not a pure operator/atom.
- **A5 — (2b) increment-2: function calls + the postfix chain.** Adds, mirroring
  the Rust parser, free calls `f(args)` → `(call f …)` (an *atom* case — the callee
  is a name, not an expr; only a postfix `.m(args)` calls a value) and the **postfix
  chain** applied left-to-right over an atom: field `t.field` → `(field t field)`,
  index `t[i]` → `(index t i)`, method `t.m(args)` → `(method t m …)`. A new
  `parse_postfix` layer sits between `parse_unary` and `parse_atom`. **The data-model
  call (revises D4):** an **argument list is variadic**, and `Vec<non-primitive>` is
  unsupported (A3), so args are a **second enum `Args = End | Cell(Expr, Args)`,
  mutually recursive with `Expr`** (`Expr` gains `Call([u8], Args)` /
  `Method(Expr, [u8], Args)` / `Field(Expr, [u8])` / `Index(Expr, Expr)`).
  This extends A2's drop gate (a *single* self-recursive enum) to **two
  mutually-recursive enums + enum-typed payloads** — DE-RISKED by a probe first
  (build → consuming-dump → `leaks`: compiles, correct, 0 leaks) before the parser
  was grown, the same probe-first discipline as A3. `parse_args` builds the cons-list
  head-first by recursion and consumes the closing `)`; the postfix chain folds via
  `parse_postfix_rest` accumulator recursion (a loop accumulator trips moved-in-loop);
  the tokenizer gains `.` `[` `]` `,` (tags 26–29). Verified: 45 differential seeds
  (incl. chains `a.b(c)[d].e`, `x.foo(1).bar[k].baz(y, z)`, nested-arg calls) match
  `snc ast`; leak-free under `leaks --atExit`. **Still deferred to later (2b)
  increments:** the `::` paths (qualified-call / class-init / enum construction),
  struct + array literals, `if`/`match`, perform/handle, scope/spawn/await/declassify.
- **A6 — (2b) increment-3: the `::` paths + array literals.** Adds the
  identifier-prefixed `::` forms (parsed in `parse_atom` after an ident) and array
  literals, all reusing A5's `Args` cons-list. `Name::method(args)` →
  `(qcall Name method …)`; `Name::init(args)` → `(class-init Name …)` (the **`init`
  name with parens** is the only class-init form); a **paren-less** `Name::tail`
  (e.g. bare enum-unit construction `Enum::Variant`) → a qualified call with empty
  args — the enum-vs-impl meaning is a *resolve* concern, so the parser emits a
  uniform `(qcall …)`, matching the Rust parser exactly. An **atom-position `[`** is
  an array literal `[e1, e2, …]` → `(array …)`, distinct from the **post-atom `[`**
  index operator (A5) by position (`parse_atom` vs `parse_postfix_rest`). `Expr` gains
  `Qcall([u8], [u8], Args)` / `ClassInit([u8], Args)` / `Array(Args)`; `parse_args` is
  generalised with a **terminator-tag** param (`)`=5 for call args, `]`=28 for array
  elements); the tokenizer gains `::` (30) + `:` (31); an `is_kw_init` slice-compare
  picks `init` (the self-contained tokenizer has no `init` keyword). Verified: 59
  differential seeds (qcall with/without args/parens, class-init with/without args,
  bare-init→qcall, arrays empty/expr-elems, array-then-index `[1,2][0]`, deep nests
  like `g(A::b(x), [1, h(3)], Point::init(y, z))` and `[A::b(), c.d][0].e`) match
  `snc ast`; leak-free under `leaks --atExit`. **Still deferred to later (2b)
  increments:** struct literals, `if`/`match`, perform/handle,
  scope/spawn/await/declassify.
- **A7 — (2b) increment-4: `if`-expressions + brace blocks.** Adds the first
  **control-flow expression** and the **block** machinery. `if <cond> { <then> }
  else { <else> }` → `(if cond (block then) (block else))`; a brace block
  `{ <expr> }` → `(block expr)`. `if` is dispatched at the **top of `parse_expr`**
  (so it is a full expression, never an operator operand — matching the Rust
  parser, where `parse_add`'s operand is `parse_mul`, not `parse_expr`); `else` is
  **mandatory** (Sentinel has no bare `if`), and `else if` chains by wrapping the
  inner `if` in a block (matching the oracle's `Block { stmts: [], tail: inner_if }`).
  A brace block is also an atom case (`parse_atom` on `{`). **Blocks are
  statement-FREE for now** — `BlockE(Expr)` holds just the tail; the full
  statement list (let / assign / while / break / continue / expr-stmt) lands at
  (2c), at which point `BlockE` grows a statement cons-list. `if` / `else` are
  **tagged in the tokenizer** (32 / 33) like `fn` (new `is_kw_if` / `is_kw_else`
  slice-compares), so the parser dispatches + consumes them by tag. `Expr` gains
  `If(Expr, Expr, Expr)` + `BlockE(Expr)`; `parse_block` + `parse_if` (the latter
  recursing for `else if`). Verified: 68 differential seeds (basic `if`, cond
  exprs, `else if` chains, nested `if`, brace blocks, `if` inside call args / array
  elements) match `snc ast`; leak-free under `leaks --atExit`. **Still deferred to
  later (2b) increments:** `match` (the last control-flow expr — adds a `Pattern`
  enum + `parse_pattern` + arms), struct literals (need the Rust `allow_struct_lit`
  flag), perform/handle, scope/spawn/await, declassify.
- **A8 — (2b) increment-5: `match` expressions + patterns.** Adds the last
  control-flow expression. `match <scrutinee> { pat => body, … }` →
  `(match scrut (arm pat body)…)`, dispatched at the **top of `parse_expr`**
  alongside `if` (a `match` keyword tag); arms comma-separated (trailing comma
  allowed); arm **bodies are expressions** (`parse_expr`, not blocks — matching the
  Rust `parse_match_arm`). Patterns are the `_` wildcard → `(pat _)` or a qualified
  variant `Enum::Variant` with an optional **positional binding list** →
  `(pat Enum Variant b1 b2)` (each binding an ident, itself possibly `_`). **The
  data model is the deepest mutual recursion yet — four enums in a cycle**
  (`Expr → Arms → {Pattern → Binds, Expr}`): `Expr` gains `Match(Expr, Arms)`; new
  `Arms = ArmEnd | ArmCell(Pattern, Expr, Arms)`, `Pattern = PatWild |
  PatVariant([u8], [u8], Binds)`, `Binds = BindEnd | BindCell([u8], Binds)` —
  **de-risked by a probe first** (build → consuming-dump → `leaks`: 0 leaks), as with
  A5's `Args`. `parse_match` / `parse_arms` / `parse_pattern` / `parse_binds` build
  them by recursion (the cons-lists consume their closing bracket); the tokenizer
  gains the `match` keyword (34) + `=>` FatArrow (35) + `is_kw_match` / `is_wildcard`.
  Verified: 78 differential seeds (multi-arm, single/multi/wildcard bindings,
  match-on-call scrutinee, if/match/call arm bodies, nested `match`, `match` in call
  args, trailing comma, and an AST-walker shape `match parse(t) { Node::Bin(op, l, r)
  => eval(l) + eval(r), … }`) match `snc ast`; leak-free under `leaks --atExit`.
  **Still deferred to later (2b) increments:** struct literals (need the Rust
  `allow_struct_lit` flag), perform/handle, scope/spawn/await, declassify.
- **A9 — (2b) increment-6: struct literals, via a context-free lookahead (revises
  the `allow_struct_lit` framing).** `Name { f1: e1, f2: e2 }` →
  `(struct-lit Name (field f1 e1) (field f2 e2))`. **The disambiguation is the
  story.** The Rust parser threads a stateful `allow_struct_lit` flag through the
  *whole* expression descent — set `false` while parsing an `if`/`while`/`match`
  head so `if x { … }` reads `x` as the condition (not `x { … }` as a struct lit),
  re-enabled inside `(`/`[`/arg/body positions. Porting that flag means threading a
  `bool` through ~19 parse functions. **The port instead uses a context-free
  lookahead: `{ Ident :`** (a brace, an identifier, then a **single** colon) — which
  *only ever* begins a struct literal, because no block / match-body / if-body can
  start with a single-colon `Ident :` (no statement form is `name :`; variant
  patterns use `::`, not `:`). So on all **clean-parsing** input the lookahead
  produces the identical AST to the flag, with **no threading** — the same
  "different implementation, byte-identical output" trade the lexer (1/N) made
  (direct emission vs a `Token` enum). Verified directly that heads stay
  conditions: `if x { P { a: 1 } } else { Q { b: 2 } }`, `match s { St::A => P { v:
  1 }, … }`, `(P { x: 1 }).x`. `Expr` gains `StructLit([u8], Fields)`; new
  `Fields = FieldEnd | FieldCell([u8], Expr, Fields)` (the proven cons-list shape);
  `parse_fields` parses `name : value` pairs up to the closing `}`; no tokenizer
  change (`:` is already tag 31). **Documented limitation:** an **empty** struct
  literal `Name {}` is deferred — it has no `field :` to key on, and `{}` collides
  with an empty `match` / `while` body; the `allow_struct_lit` flag is the eventual
  fix iff the full corpus needs empty struct literals. Verified: 88 differential
  seeds (single/multi-field, expr values, nested structs, structs in call args /
  arrays, trailing comma, head-disambiguation) match `snc ast`; leak-free under
  `leaks --atExit`. **Still deferred to later (2b) increments:** perform/handle,
  scope/spawn/await, declassify.

Date: 2026-06-02
Related:
  - **0038** (self-host port kickoff + lexer): establishes the **differential-
    oracle method** (a canonical `snc <stage>` dump the Sentinel stage reproduces
    byte-for-byte, diffed over `tests/pass` + `tests/ui`) and the `selfhost/`
    tree. This ADR is the next stage under that method. Reuses A2's hard-won
    Sentinel-language workarounds (flat per-fn var namespace; deep-`if` tail-borrow).
  - **0031** (Phase D kickoff): D5 stage order lexer → **parser** → resolve → … .
  - **0032** (sum types + `match`): the AST is modelled as Sentinel recursive
    enums; **D.1 amendment A1's recursive-enum drop is box-free only** — an AST is
    deeply recursive, so this sub-phase must validate that building + dropping a
    real AST is leak-/UAF-safe (a first-slice gate, D6).
  - **0009 §6 / D4**: the Rust parser is hand-written recursive descent; the
    Sentinel port mirrors it (a token cursor + precedence-climbing for expressions).

## Context

The lexer (1/N) proved the differential-oracle method end-to-end: `snc lex` dumps
the token stream, `selfhost/lexer.sentinel` reproduces it, and a corpus test
asserts byte-identical output (139/139 clean fixtures). The parser is the next
stage — and the **biggest single stage** of the compiler: it consumes the token
stream and builds the full AST (`sentinel-ast`: `ExprKind` ~30 variants,
`StmtKind`, the seven top-level decl kinds, `TypeExpr`, `Pattern`). Two facts
shape the design:

  1. **The existing `snc parse` is NOT a complete oracle.** It pretty-prints the
     AST via the `Display` impls (a readable S-expression: `(fn add (a: i64 b: i64)
     -> i64 (block (+ a b)))`), but `Program`'s `Display` only serializes `uses` /
     `effects` / `structs` / `fns` — it **silently omits** enums, traits, impls,
     and classes. A differential oracle must cover the **whole** AST deterministically.
  2. **The lexer (1/N) emits a dump; it does not return tokens.** Per ADR 0038 A1,
     the token model was deferred. The parser needs the lexer to **return a token
     stream**, so (2/N) opens with that refactor.

## Decision

### D1. Goal.

Port the parser to Sentinel: `selfhost/parser.sentinel` consumes the token stream
from `selfhost/lexer.sentinel` and builds the AST, emitting a **canonical AST
dump** byte-identical to a Rust `snc` oracle over the corpus. Each later stage
(resolve → …) remains its own ADR.

### D2. The oracle — a complete canonical AST dump (`snc ast <file>`).

Add a Rust subcommand `snc ast <file>` that parses and emits a **complete,
deterministic, S-expression-style** dump of the whole `Program` — every decl kind
(fn / struct / enum / trait / impl / class / effect / use), every `Stmt`, `Expr`,
`TypeExpr`, and `Pattern`. It reuses the *style* of the existing `Display` but is
**complete** (covers the omitted decl kinds) and **purpose-built for
reproducibility** from Sentinel (regular, fully-parenthesized, no
context-dependent spacing). The existing `snc parse` Display is left as-is (a
human affordance); `snc ast` is the machine oracle. Like `snc lex`'s dump, it is a
**dev/validation surface, not `abi-v1`** — freely amendable. Pinned by a golden
test; the Sentinel parser diffs against it.

*(Alternative considered: extend the `Display` impls to completeness and match them
directly. Rejected for (2/N): the Display has context-dependent formatting that is
fiddly to reproduce exactly, and changing it risks existing readers. A fresh,
regular dump is cleaner — same call ADR 0038 made for `snc lex`.)*

### D3. The token model + lexer refactor (opens (2a)).

Refactor `selfhost/lexer.sentinel` to **return a token stream** rather than only
printing. To sidestep the D.3 "primitive-element drop" limitation (a
`Vec<struct-with-a-[u8]-field>` would not drop the inner array), the stream is
**struct-of-arrays of scalars**: `kinds: Vec<i64>` (integer tag per token),
`starts: Vec<i64>`, `ends: Vec<i64>`. A token is `(kinds[i], starts[i], ends[i])`;
the parser re-slices `src[start..end]` from the source `[u8]` when it needs a
lexeme (identifier text, literal bytes). Kind **tags are internal** to the Sentinel
compiler (the canonical dump uses kind/node **names**, never tags, so the two
implementations needn't agree on numbering). The lexer keeps its (1/N) dumping
ability (lex → stream → dump) so `tests/selfhost_lex.rs` stays green.

### D4. The AST model in Sentinel.

Model the AST as Sentinel enums + structs mirroring `sentinel-ast`: an `ExprKind`
enum (recursive — `Expr` payloads hold `Expr`s), `StmtKind`, the decl structs, a
`TypeExpr` enum, a `Pattern` enum. **Recursive-enum risk (ADR 0032 A1):** the AST
is the deepest recursive structure the language has hosted; D.1 A1's box-free
recursive-enum drop is leak-free for the standard consume walk but is **untested
at AST scale**. So **(2a) includes a recursive-AST build → dump → drop validation**
(`leaks --atExit`) as a gate *before* the full parser is built — if it leaks or
UAFs, the D.1b payload-ownership fix is pulled in first. Payloads that are arrays
(e.g. an `Ident`'s name `[u8]`, a `Call`'s arg list) interact with the same drop
path; the validation must exercise them.

### D5. Recursive-descent structure.

Mirror the Rust parser (ADR 0009 D4): a token **cursor** (an index into the
struct-of-arrays stream) + hand-written recursive descent, with
**precedence-climbing** for the expression grammar (the Rust parser's
`parse_add`/`parse_mul`/… ladder + the C5.3 bitwise levels). Reuses ADR 0038 A2's
workarounds (unique per-fn locals; keep borrowing calls out of deep-`if` tails).

### D6. Sub-slicing (the parser is big — staged, each oracle-validated).

| Slice  | Scope                                                                 |
|--------|-----------------------------------------------------------------------|
| (2a)   | lexer-returns-tokens refactor + the AST-enum scaffold + the recursive-|
|        | AST drop validation + a minimal **expression** parser (literals +     |
|        | precedence binary/unary) + `snc ast` (canonical dump) + a seed diff.  |
| (2b)   | full **expressions** (call, field, index, `if`, `match`, struct lit,  |
|        | method/qualified call, perform/handle, …).                            |
| (2c)   | **statements + `fn` definitions** (block, `let`, assign, `while`,     |
|        | `break`/`continue`, params, return type, effect row).                 |
| (2d)   | the remaining **top-level decls** (struct / enum / trait / impl /     |
|        | class / effect / `use`) — and the oracle's completeness (D2).         |

Coverage grows from a seed fixture set toward the **full corpus**; the slice lands
only when its dump matches `snc ast` for its share of `tests/pass` + `tests/ui`.

### D7. Out of scope.

Resolve and later stages (own ADRs); **parser ERROR/diagnostic parity** (happy-path
AST production first, as with the lexer — ADR 0038 D6); performance. The Sentinel
parser may organise into multiple `selfhost/*.sentinel` files (dogfooding D.6
modules) as it grows.

### D8. Phase-go.

`selfhost/parser.sentinel` (compiled by `snc`) emits a canonical AST dump
**byte-identical** to `snc ast` for every clean-parsing fixture in `tests/pass` +
`tests/ui` (a differential test, mirroring `tests/selfhost_lex.rs`), with the
recursive-AST drop validation leak-clean.

## Reasoning

**Why a fresh `snc ast` dump, not the existing `Display`.** The Display is
incomplete (omits four decl kinds) and context-formatted; a regular, complete dump
is both correct and far easier to reproduce from Sentinel — the same trade ADR 0038
made for `snc lex`, now proven.

**Why validate recursive-AST drop first.** The parser is large; discovering a
recursive-enum drop defect *after* building it would be expensive. A small early
gate (build a hand-made AST, dump it, drop it, check `leaks`) de-risks the whole
sub-phase and tells us early whether D.1b must come first.

**Why struct-of-arrays tokens.** It keeps the token stream within D.3's
primitive-element `Vec` drop guarantee (no `Vec<struct-with-heap>` leak), needs no
new language feature, and re-slicing lexemes from `src` is cheap.

## Consequences

### Positive
- The second compiler stage in Sentinel, oracle-validated — momentum on the port.
- A complete `snc ast` dump is a reusable tool + the substrate for the resolve
  stage's oracle later.
- Forces (and validates) the recursive-enum machinery at real scale — useful
  signal for the rest of the port.

### Negative
- The largest stage; multiple slices; the recursive-enum drop debt may surface and
  pull D.1b onto the critical path.

### Neutral
- The Rust `snc` stays the production compiler + oracle throughout (ADR 0031 D6).
  `snc ast` adds a dev surface, not ABI.

## Revisit

PROPOSED until (2a) lands, then ACCEPTED-WITH-AMENDMENTS as slices close. Triggers:
- **D4 recursive drop**: if AST drop leaks/UAFs, land the D.1b payload-ownership
  fix before continuing the parser.
- **D2 dump format**: refine the canonical S-expr if it proves awkward to emit from
  Sentinel (it is a dev contract, freely amendable).
- **D3 token model**: if `Vec<scalar-struct>` turns out to work cleanly, prefer a
  `Token` struct over struct-of-arrays for readability.
- **D6 ordering**: reorder slices if a later one is needed to validate an earlier.
