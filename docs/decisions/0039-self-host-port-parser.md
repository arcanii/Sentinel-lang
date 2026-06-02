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
landed `match` expressions + patterns, **increment-6 (A9)** landed struct
literals, **increment-7 (A10)** landed the effect/concurrency leaf forms
(`declassify` / `perform` / `scope` / `spawn` / `.await`), and **increment-8 (A11)**
landed `handle` — **closing the (2b) expression grammar** (every `ExprKind` the
oracle emits now parses). **(2c) is now UNDERWAY:** **(2c-1, A12)** landed the
statement grammar (a real `Block` of statements + tail), **(2c-2, A13)** landed
`let` type annotations + a `parse_type`, and **(2c-3, A14)** landed `fn` definitions
— **closing the (2c) fn-level grammar** (every `Stmt` / `Expr` / `TypeExpr` /
`Pattern` + the fn header now parse). Remaining: **(2d)** the top-level decls
(struct / enum / trait / impl / class / effect / use). The ADR flips fully as they
close. See ## Amendments.

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
- **A10 — (2b) increment-7: the effect / concurrency leaf forms.**
  `declassify(e)` → `(declassify e)`; `perform Eff.op(args)` →
  `(perform Eff op args…)`; `scope concurrent { block }` → `(scope (block …))`;
  `spawn <postfix>` → `(spawn …)`; and the `.await` **postfix** → `(await target)`.
  `declassify` / `perform` / `scope` / `spawn` are keyword-led atom cases in
  `parse_atom` (`scope` skips the positional `concurrent` ident then `parse_block`;
  `spawn` parses its target via `parse_postfix`; `perform` reuses `parse_args`);
  `.await` is checked right after the `.` in `parse_postfix_rest`, before the
  field/method dispatch. Five new keyword tags (36–40) + `is_kw_*` helpers; `Expr`
  gains `Declassify(Expr)` / `Perform([u8], [u8], Args)` / `Scope(Expr)` /
  `Spawn(Expr)` / `Await(Expr)`. **The `scope` (and `while`) body stays
  statement-free** until (2c): a body with a `;`-separated statement — and the `;`
  token itself — is (2c) territory, so the seeds use statement-free bodies.
  Verified: 102 differential seeds (each form alone + composed, e.g. a `match` arm
  `declassify(perform Tls.verify(mac))`, `if ready { spawn w(x) } else {
  compute().await }`, `scope concurrent { declassify(perform Net.recv()) }`) match
  `snc ast`; leak-free under `leaks --atExit`. **Remaining in (2b):** only
  `handle … with { arms }` — which closes the expression grammar.
- **A11 — (2b) increment-8: `handle` — closes the expression grammar.**
  `handle <body> with { Eff.op(params) => arm, … return v => arm }` →
  `(handle body (arm Eff op armbody)… (return v body))` (a `parse_atom` keyword
  case). **Two subtleties, both handled faithfully:** (i) the handler arm's
  **params are parsed but NOT dumped** (skipped to the closing `)`; the dump is
  just `(arm Eff op body)`); (ii) the optional `return v => body` arm is kept
  **separate** from the handler-arm list and **dumps LAST** regardless of its
  source position — the arm parse fills a **`&mut Ret` out-param** when it sees
  `return` (mirroring the Rust `return_arm`). The out-param **assigns an enum value
  through a `&mut` ref** (`*ret = Ret::YesRet(…)`) — the first non-primitive `&mut`
  assignment in the port (the cursor is `&mut i64`); **de-risked by a probe** (it
  compiles, runs, and is leak-free; `return`-not-last verified to still dump last).
  `Expr` gains `Handle(Expr, HArms, Ret)`; new `HArms = HEnd | HCell([u8], [u8],
  Expr, HArms)` and `Ret = NoRet | YesRet([u8], Expr)`; three keyword tags
  (`handle` 41 / `with` 42 / `return` 43). Verified: 110 differential seeds (single/
  multi-arm handlers, return arms incl. return-not-last + empty params, composed
  forms like `g(handle …)` and `handle perform … with …`) match `snc ast`;
  leak-free under `leaks --atExit`. **(2b) the full expression grammar is COMPLETE**
  — operators, atoms, calls, postfix, `::` paths, arrays, `if`/blocks,
  `match`/patterns, struct literals, declassify/perform/scope/spawn/await, handle.
  **Next: (2c)** statements (`let` / assign / `while` / `break` / `continue` /
  expr-stmt — turning the statement-free `BlockE` into a real block) + `fn`
  definitions with params/return-type/effect-row; then **(2d)** the top-level decls.
- **A12 — (2c-1): the statement grammar + real blocks (opens slice (2c)).** A block
  is now `{ <stmt>* <tail> }` → `(block <stmt>… <tail>)`. Statements: `let [mut]
  name = e` → `(let [mut] name _ e)` (the `_` is the type annotation, added at
  (2c-2)); `target = e` → `(assign target e)`; `while cond { body }` → `(while cond
  (block …))`; `break` → `(break)`; `continue` → `(continue)`; an expr-statement →
  `(expr e)`. The block loop (`parse_stmts`) mirrors `parse_block_inner`: dispatch
  `let`/`while`/`break`/`continue` by keyword tag, else parse an expr and classify
  by what follows (`=` → assign-stmt, `;` → expr-stmt, else → the tail). **The block
  tail is written into a `&mut Expr` out-param** (the A11 technique), whose default
  is a **nullary `Expr::SynthZero`** that dumps `(int 0)` — exactly the oracle's
  synthesised unit tail for a statement-only `while` body. **⚠ Leak found + fixed
  in-flight:** the tail default was first `Expr::Int(0)`, whose `i64` payload is
  heap-**boxed**, and `*tail = ex` through the `&mut` ref does **not** free the old
  enum (consistent with A11, where `NoRet` is nullary), so the boxed `Int(0)` leaked
  once per overwritten tail (`leaks --atExit`: 2 leaks / 32 bytes). A **nullary**
  default (no box) is leak-free and dumps identically — recorded as a reusable
  rule: a `&mut Enum` out-param's default must be a payload-free variant. `Expr`
  `BlockE(Expr)` → `Block(Stmts, Expr)` + `SynthZero`; new `Stmts`/`Stmt` enums; the
  fn body is now a real block. New tokens: `;` (44) + `let`/`mut`/`while`/`break`/
  `continue` (45–49). Verified: 122 differential seeds (let/assign/expr-stmt, `while`
  with break/continue/assign bodies, statement-only `while` bodies, nested
  statements, composites) match `snc ast`; leak-free. **Next:** (2c-2) `let` type
  annotations + `parse_type`; (2c-3) `fn` definitions.
- **A13 — (2c-2): `let` type annotations + a `parse_type`.** The optional `: type`
  on a `let` → `(let [mut] name <type> e)` (vs `_`). `parse_type` mirrors the Rust
  one: `secret T` → `(secret T)`, `&T`/`&mut T` → `(ref T)`/`(refmut T)`, `?T` →
  `(opt T)`, `[T]` → `(arr T)`, `Ident` → the name, `Ident<args>` →
  `(generic Ident args…)` (the generic arg list is a cons-list terminated by `>`).
  **Nested generics close without a `>>` split:** the tokenizer has no `>>`, so
  `Vec<Box<i64>>` lexes its trailing `>>` as two `Gt` tokens and each `>` closes one
  level (the Rust parser needs an explicit `Shr`-into-two-`>` split; the port
  sidesteps it). New recursive `TypeE` enum + `TyArgs` cons-list + a `TyOpt`
  (`NoTy`/`SomeTy`); `Stmt::SLet` gains the `TyOpt` field. New tokens: `?` (50) + the
  `secret` keyword (51). Verified: 135 differential seeds (i64/bool idents, `[u8]`
  arrays, `Vec<i64>`/`Map<i64,[u8]>`/`Box<Vec<i64>>` generics incl. nesting, `?T`,
  `&T`/`&mut T`, `secret T`, `secret [u8]`, mixed annotated/un-annotated lets) match
  `snc ast`; leak-free. **Next:** (2c-3) `fn` definitions (params/return-type/effect
  row — reusing `parse_type`); then (2d) the top-level decls.
- **A14 — (2c-3): `fn` definitions (closes the fn-level grammar).** `main`'s
  hard-coded paramless `fn NAME() -> TYPE` header is replaced by a real fn parse:
  `fn name <type-params>? ( [mut] p: T, … ) -> RET ! { eff, … }? { body }` →
  `(fn name ((param [mut] p <type>) …) <ret> <block>)`. The param list is a `Params`
  cons-list; **the param-list dump has no leading space before the first param**
  (it sits right after the list's open paren), so a first/rest split
  (`dump_params` + `dump_params_rest` over a shared `dump_param_body`). The `-> RET`
  return type now routes through `parse_type`/`dump_type` (so a non-`Ident` return
  like `[u8]`/`?T`/`Vec<T>`/`secret T` dumps right — it was previously dumped raw via
  `append_slice`). **Generic type-params `<…>` and the postfix effect row `! { … }`
  are parsed-and-SKIPPED** — `dump_fn` emits neither (confirmed against
  `ast_dump.rs`); `skip_type_params` is depth-balanced over `<`/`>`, `skip_effect_row`
  skips to the `}`. No new tokens. **⚠ Sentinel-`if`-is-an-expression reminder:**
  `skip_type_params` first used statement-only `if` branches + a bare `if` (no
  `else`) → a compile error ("blocks must end with an expression"); rewrote it as
  `depth = if … { depth+1 } else if … { depth-1 } else { depth }` inside a `while
  depth > 0` loop. Verified: 148 differential seeds (params single/multi/`mut`,
  `[u8]`/`?T`/`Vec<T>`/`secret` return types, ref params, generic fns,
  effect-row fns, multi-fn programs, a composite generic+secret+effect+statements
  handler) match `snc ast`; leak-free. **This closes the fn-level grammar.** Next:
  (2d) the top-level decls (struct / enum / trait / impl / class / effect / use) +
  completing `snc ast`'s `Program` dumper for them — the last parser slice.
- **A15 — (2d-1): `use` decls + the top-level-decl dispatch (opens slice (2d)).**
  `use a::b::Item;` → `(use a b Item)` (each path segment space-separated).
  **The oracle now dumps every decl in SOURCE ORDER.** The parsed `Program`
  buckets decls into per-kind `Vec`s (uses / fns / structs / …), losing
  cross-kind source order; `ast_dump::dump` re-collates them into one list
  **sorted by span start** — exactly the order the Sentinel parser emits them as
  it scans the token stream top-to-bottom (so the Sentinel side needs no
  bucketing across top-level decls; it just dumps each as it parses). A tagged
  `Item` enum drives the dispatch (it grows a variant per (2d) increment). On the
  parser side, `main`'s fn-only loop becomes a `dump_item` dispatcher on the
  leading token (`use`=58, else `fn`); the fn body is factored into
  `dump_fn_decl`, and `dump_use_decl` emits the path inline. **An optional `pub`
  prefix is parsed-and-skipped** (the dump omits visibility, like fn generics /
  effect rows) — matching `parse_program`'s `parse_optional_visibility`. ⚠ The
  dispatch lives in a **single `dump_item` helper** that re-passes the ref params
  to the per-kind dumpers, because **sibling `if`-tail `&mut` borrows of a
  *local* conflict** under the lexical borrow checker (the ADR 0038 A2 quirk:
  `&mut out` / `&mut cur` in two arms read as overlapping) — but **re-passing a
  `&mut` *param* across if-tails is fine** (the proven `parse_postfix_rest`
  shape). New tokenizer tags `use`(58) + `pub`(59) + `is_kw_use` / `is_kw_pub`
  (52–61 reserved for the decl keywords). Verified: 152 differential seeds (4 new
  `use` seeds, incl. multi-`use` + fn) match `snc ast`; leak-free under `leaks
  --atExit`; a new `tests/ast.rs` golden pins the source-order `use` dump. Next:
  (2d-2) `struct` decls.
- **A16 — (2d-2): `struct` decls.** `struct Name <T>? { f: T, … }` →
  `(struct Name (field f <type>) …)`; empty → `(struct Name)`. Generic
  type-params + visibility omitted (as the fn dump omits them); field types route
  through `parse_type` / `dump_type`. Oracle: `Item` gains a `Struct` variant +
  `dump_struct`. Parser: tokenizer tag `struct`(52) + `is_kw_struct`;
  `dump_struct_decl` + a recursive `dump_struct_fields` that **parses + dumps each
  `name : type` inline** (consuming the closing `}`, comma-skipping — no AST node,
  mirroring `dump_use_decl`); `dump_item` gains the struct arm. Verified: 159
  differential seeds (7 new — fields, empty, trailing comma, complex field types
  incl. `?`/`&`/`secret`/nested generics, source-order `struct`/`fn`
  interleaving) match `snc ast`; leak-free under `leaks --atExit`; a new
  `tests/ast.rs` golden pins the struct dump. Next: (2d-3) `enum` decls.
- **A17 — (2d-3): `enum` decls.** `enum Name { V1, V2(T), … }` →
  `(enum Name (variant V1) (variant V2 <type>) …)`; a unit variant has no payload
  types, a payload variant lists them positionally; empty → `(enum Name)`. Enums
  are **non-generic** (no type-params to skip — `enum Name<T>` doesn't parse in
  the Rust oracle either); payload types route through `parse_type`. Oracle:
  `Item` gains an `Enum` variant + `dump_enum`. Parser: tokenizer tag `enum`(53) +
  `is_kw_enum`; `dump_enum_decl` + a recursive `dump_variants` (name + optional
  payload list) + `dump_payloads` (parse + dump each positional type inline);
  `dump_item` gains the enum arm. Verified: 166 differential seeds (7 new —
  unit/payload variants, empty, trailing comma, recursive payloads like
  `Bin(Node, Node)`, `struct`+`enum`+`fn` source-order interleaving) match `snc
  ast`; leak-free under `leaks --atExit`; a new `tests/ast.rs` golden pins the
  enum dump. Next: (2d-4) `effect` decls.
- **A18 — (2d-4): `effect` decls.** `effect Name { op(p: T) -> R; … }` →
  `(effect Name (op op (<params>) <ret>) …)`; an op's params dump like fn params;
  a **missing** op return type dumps `_` (reusing the `TyOpt` / `dump_tyopt`
  `let`-annotation convention). Empty → `(effect Name)`. Oracle: `Item` gains an
  `Effect` variant + `dump_effect`. Parser: tokenizer tag `effect`(57) +
  `is_kw_effect`; `dump_effect_decl` + a recursive `dump_ops` (reuses
  `parse_params`; the optional `-> ret` parses into a `TyOpt` exactly like
  `parse_let`, sidestepping a two-borrowing-arm `if`; the `;` terminators are
  skipped); `dump_item` gains the effect arm. Verified: 172 differential seeds (6
  new — ops with/without params + return type, empty, trailing `;`,
  `effect`+`struct`+`fn`-with-effect-row interleaving) match `snc ast`; leak-free
  under `leaks --atExit`; a new `tests/ast.rs` golden pins the effect dump. Next:
  (2d-5) `trait` decls.
- **A19 — (2d-5): `trait` decls + the method machinery.** `trait Name { fn
  m(self: &Self, p: T) -> R; … }` → `(trait Name (method m <shared|exclusive>
  (<params>) <ret>) …)`. Trait method sigs have **no body** (`;`-terminated); the
  `self` receiver dumps as its kind word (`shared`/`exclusive`), the non-self
  params dump like fn params, the effect row is omitted. Empty → `(trait Name)`.
  **Introduces the method machinery shared by impl (2d-6) + class (2d-7):** (i)
  `parse_self_kind` consumes `self : & [mut] Self` **positionally** (`self` /
  `Self` lex as plain idents in the self-contained tokenizer) and returns whether
  it is exclusive; (ii) `dump_method_head` parses `fn name ( self [, params] ) ->
  ret [! {eff}]` and emits ` (method name <self> (<params>) <ret>` with **no
  closing paren** — the caller adds `)` (trait) or ` <block>)` (impl/class). 🔑
  After the self receiver, **`parse_params` consumes the rest of the list** (it
  already skips a leading `,` and ends on `)`, so it handles both `(self: &Self)`
  and `(self: &Self, x: T)` with no special-casing). Oracle: `Item` gains a
  `Trait` variant + `dump_trait` + a `self_kind_word`. Parser: tokenizer tag
  `trait`(54) + `is_kw_trait`; `dump_trait_decl` + `dump_trait_methods`;
  `dump_item` gains the trait arm. Verified: 178 differential seeds (6 new —
  `&Self`/`&mut Self` receivers, extra params, complex returns, effect-row
  methods, empty, `enum`+`trait`+`struct` interleaving) match `snc ast`; leak-free
  under `leaks --atExit`; a new `tests/ast.rs` golden pins the trait dump. Next:
  (2d-6) `impl` decls.

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
