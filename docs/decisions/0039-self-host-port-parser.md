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
`ExprKind` variants); **increment-1 (A4) has LANDED:** the complete
operator-precedence ladder + scalar atom leaves. Remaining (2b) increments + slices
(2c)–(2d) per D6 grow it toward the full corpus; the ADR flips fully as they close.
See ## Amendments.

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
