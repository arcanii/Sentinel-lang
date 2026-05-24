# ADR 0004: Row representation and effect surface for Sentinel-Mini B2

## Status

Accepted, 2026-05. Refines ADR 0002 (which pinned that rows arrive in
B2 but deliberately left their representation and surface open).

## Context

ADR 0002 committed B2 to widen `Ty::Fun` with an effect row and to add
effect declarations to Sentinel-Mini, but explicitly deferred the
representation and surface choices to "B2 should deliberately choose
its row design" (ADR 0002 §Consequences/Neutral). Going into the B2.0
refactor we have to pin those choices, because they shape the type
representation, the unifier signature, the parser, and the diagnostic
text the user sees.

ADR 0003 §R1-§ R3 also constrained B2: rows extend `Ty::Fun`, row
unification is a separate function, and `TypeError` gains row-specific
variants. Those are upstream of this ADR; we honour them.

## Decision

### Row representation (D1)

`RowVar` is a distinct kind, parallel to `TyVar`. Concretely:

    pub struct RowVar(pub u32);

    pub enum Row {
        Empty,
        Var(RowVar),
        Cons { label: String, arg: Box<Ty>, ret: Box<Ty>, tail: Box<Row> },
    }

Substitutions carry two maps:

    pub struct Subst {
        ty_map:  HashMap<TyVar,  Ty>,
        row_map: HashMap<RowVar, Row>,
    }

A `TyVar` substitution can never bind a row (and vice versa); this is
enforced by the type of the map, not by convention.

The `Cons` variant carries the effect's signature alongside the label
(`arg` and `ret` types) so the unifier does not need an external lookup
table to check that two occurrences of the same effect label agree on
their signature. This is denormalised but local and cheap; the lookup
table still exists at the inference entry point for resolving `do Label`.

`TyVarSupply` gains `fresh_row_var()` and `fresh_row()` methods. It is
kept under its existing name despite the extended responsibility, to avoid
a mechanical rename across all call sites.

### Rows are open by default at introduction (D2)

When the inferer encounters `fn(x) => body`, it constructs the arrow
with a fresh `Row::Var(��)` tail. Effect labels are added to that tail
via cons-cells during inference of `body`. Function application
unifies the callee arrow's row tail with the caller's ambient row,
which is the standard Koka/Eff shape.

There is no closed-row surface in B2. A pure function over a closed
row is expressible only via the empty row that the top-level inferer
produces for pure programs; user-written `pure` annotations are
deferred (B3 or later).

### Effect declaration and invocation surface (D3, D4, D5)

Top-level surface:

    effect Print : Int -> Bool;
    effect Ask   : Int -> Int;

    let f = fn(x) => do Print(x + 1) in
    if f(42) then do Ask(0) else 0

Lexical rules:

  - new tokens `Effect`, `Do`, `Colon`, `Semicolon`, `Arrow`
  - `->` is now a token (it was previously not recognised -- Mini did
    not need it)

Syntactic rules:

  - effect labels are ordinary identifiers; the *parser* requires the
    first character to be uppercase ASCII and emits
    `ParseError::EffectLabelNotCapitalised { span }` otherwise
  - each effect declaration has exactly one argument type and one
    result type; both are required
  - declarations come before the body; a program is
    `effect_decl* body_expr`
  - `do Label(arg)` is an expression

Resolution of `do Label(arg)`:

  - the parser does not own an effect environment. It parses
    `Perform { label, arg, span }` and moves on
  - the inferer receives a map of declared effects (::arg / ::ret)
    built from `Program.effects` and consults it at each `Perform`
  - an unknown label produces
    `TypeError::EffectNotInScope { name, span }` rather than a
    `ParseError`, because the environment for value vars already
    lives in `infer` and per ADR 0003 §O3 the carry-span-on-the-variant pattern fits
    naturally there. Keeping the parser stateless also avoids a
    parser-side failure mode where an unknown label and a genuine
    syntax error fight for which diagnostic wins

Result type semantics:

  - `do Print(arg)` has the result type declared for `Print`
  - the row containing this `do` operation is extended with a
    `Cons { label: "Print", arg: Int, ret: Bool, tail: ρ }` cell

Single-argument restriction is intentional and matches the surface
of B1's lambdas (single-parameter). Multi-arg and zero-arg can be
added once Mini has tuples or unit; B2 does not introduce either.

### Program-level AST (D6)

A new top-level AST node:

    pub struct Program {
        pub effects: Vec<EffectDecl>,
        pub body: Expr,
    }

    pub struct EffectDecl {
        pub name: String,
        pub arg:  Ty,
        pub ret:  Ty,
        pub span: Span,
    }

`run()` keeps its existing signature (`run(&str) -> Result<Value,
MiniError>`); internally it now parses to `Program`, builds the
effect environment from `program.effects`, then infers and evaluates
`program.body`.

A program with no `effect` declarations parses to `Program { effects:
vec![], body }`; this is the path every B1 test program already on
disk continues to take, and is what keeps the existing 95 tests green
through B2.0-B2.1.

## Reasoning

*Editorial note, carried from the scoping discussion*: an earlier
draft of this ADR put the effect-label resolution check in the parser;
on review that conflicted with ADR 0003 §O3 (spans on the error variant
that lives where the environment already does). The check now lives in
inference as `TypeError::EffectNotInScope`. Recorded here rather than
silently edited away because a future reader deserves the reason.

**Why a distinct `RowVar` kind (D1).** The cost is ~30 LoC of
parallel structure in `Subst::apply`, `free_vars`, and `compose`. The
win is a *type-level* guarantee that the inferer can never bind a
`TyVar` to a row or a `RowVar` to a non-row monotype. In a research
prototype that's about to grow handlers in B3 -- where the dance
between effect-row variables and continuation type variables gets
intricate -- paying the LoC up front avoids a class of bug that's
notoriously hard to debug when it does sneak in. ADR 0003 §O1
recommends the stub-then-real sequencing precisely because B1
demonstrated that the prototype benefits from physical, not
conventional, separations.

**Why open rows by default (D2).** Effect *polymorphism* is the
whole research question Phase B is meant to answer (HANDOVER §U lists
"effect handlers" and "capability check" as the demos). A
closed-by-default row trivially rejects every higher-order example
that motivates the system. The alternative -- closed by default with
an opt-in syntax for "this function is polymorphic over its
remaining effects" -- is the worse default because every interesting
example pays the syntax tax and the boring ones get nothing.

**Why a declared effect surface with capitalised labels (D3).** Three
considerations: (i) requiring a declaration gives `EffectNotInScope`
errors a clean span to point at (the use site) and a clean
alternative-name list (the declared labels), neither of which is
available with an implicit "any identifier in `do` position is an
effect" rule; (ii) capitalised labels are visually distinct from
value identifiers in inferred-type rendering (`int -> bool / {Print
| ρ}` reads better than the same with `print`), which matters more
than it sounds because Phase C will inherit this rendering;
(iii) Mini has no other use for capitalised identifiers yet, so the
namespace is free and we don't burn a syntactic option we'd want
back later.

**Why mandatory `arg -> ret` signatures on effect decls (D4, D5).**
An effect without a payload signature degenerates to "this row
contains the label `Print`" with no constraint on what arguments
`Print` consumes or produces. That makes the type system unable to
catch the most basic kind of mistake (`do Print(true)` when `Print`
expects an `Int`), which is precisely the kind of mistake the
prototype exists to validate the system *can* catch. The result type
serves a parallel role: it gives `do Print(...)` a real type the
surrounding expression can be constrained against, instead of
producing a free type variable that would silently unify with
anything.

**Why a `Program` AST node (D6).** Keeping "one big expression" as
the surface -- by making `effect Label : ... ;` a binding form like
`let` -- would force the AST to thread effect declarations through
`ExprKind` and force the inferer to handle them in the middle of
expression inference. The wrapper costs ~15 LoC and keeps a clean
phase split: parse a program, build the effect environment, infer
the body in that environment.

## Consequences

### Negative

- The `Subst` shape change cascades. `apply` becomes
  `apply_ty(&Ty) -> Ty` plus `apply_row(&Row) -> Row`. `compose` walks
  both maps. `free_vars` returns a struct, not a single set. Estimated
  touch count: ~15 sites in `infer.rs` beyond the ones already counted
  in ADR 0002's 20-40 estimate. Realistic combined total: 35-55.
- The inferer gains an "ambient row" parameter. This is a new kind
  of threading through `infer` that did not exist in B1; getting it
  right is the meat of B2.3. Containment: keep `ambient_row` as a
  distinct parameter (not a field on `TypeEnv`) so a future reader
  sees that each recursive `infer` call decides what row to pass.
- Comparing cons-cell rows up to label permutation is not free. The
  Rémy rewrite rule runs in O(n²) on row size; fine for the ~1-5
  effects-per-function range this prototype will ever see, but not a
  shape to carry into Phase C unchanged.

### Positive

- The unifier and the diagnostic text both gain symmetry. A row
  mismatch produces "expected row `{Print | ρ}`, found `{Ask | σ}`"
  using the same `Display` machinery as a type mismatch.
- Effect decls give the eventual B3 handler syntax a place to anchor:
  `handle e with { Print(x) => ..., Ask(x) => ... }` will dispatch on
  declared labels.
- B3's capability check (HANDOVER §5: "importing a module restricts
  its effects") is easier to bolt on because the set of effects in
  scope is already a first-class thing.

### Neutral

- The dual-map `Subst` is uglier than a single map. If we discover in
  B3 that some unification rule wants to bind type and row vars in
  one step, the asymmetry will sting. Mitigation: keep the public
  surface of `Subst` (`apply`, `compose`, `singleton`,
  `singleton_row`) covering both cases so the refactor stays
  localised.

## Alternatives considered

- **Shared `TyVar` for both kinds**, distinguishing by where they
  appear. Rejected per D1 reasoning above.
- **Implicit effect labels** (any `do <ident>(arg)` introduces an
  effect, no declaration needed). Rejected per D3 reasoning above --
  loses the "not in scope" diagnostic and gives the system no place
  to validate effect signatures.
- **Closed rows by default**. Rejected per D2 reasoning above --
  makes effect polymorphism opt-in, which inverts the prototype's
  charter.
- **Effects-as-binding-form within a single expression tree** (no
  `Program` wrapper). Rejected per D6 reasoning above -- costs more
  in inferer complexity than the AST wrapper saves.

## Revisit

Revisit if B3 (handlers) reveals that:
  - the single-arg / single-result effect signature is structurally
    insufficient for handler resumption (likely candidate: handlers
    need to access the resumption continuation's type), in which
    case D4/D5 grow,
  - the closed-row-only-at-toplevel rule produces confusing top-level
    inferred types (e.g. `int / {Print}` when the user expected `int`
    because there's no row at the top level), in which case D2 needs
    an explicit "default-close at top level" rule,
  - inference-side `EffectNotInScope` produces worse error messages
    than a parser-side check would have.

*End of ADR.*
