# ADR 0005 — Effect Inference Judgment for Sentinel-Mini B2.3

**Status:** Accepted (2026-05-24). Refines ADR 0002 and ADR 0004. Governs the B2.3a/B2.3b implementation.

## Context

B2.0 – B2.2 added the row machinery (`Row` enum, `RowVar`, `unify_row`, `Ty::arrow_with`) and the
effect surface (`effect Label : TyExpr ;` declarations, `do Label(arg)` operations). Rows exist but
every arrow is still minted with `Row::Empty`. B2.3 is the patch where rows start carrying their
weight: lambdas mint fresh rows, applications unify caller/callee rows, perform contributes a
`Cons` to the ambient row. The shape of *how* rows are threaded has several reasonable
formulations; this ADR pins them down before the code lands.

## Decisions

### D1: Judgment shape — effect-on-judgment (β), returning three-tuple

The inference function signature changes from

```rust
pub fn infer(env: &TypeEnv, expr: &Expr, supply: &mut TyVarSupply)
    -> Result<(Subst, Ty), TypeError>
```

to

```rust
pub fn infer(
    env: &TypeEnv,
    eff_env: &EffectEnv,
    expr: &Expr,
    supply: &mut TyVarSupply,
) -> Result<(Subst, Ty, Row), TypeError>
```

The returned `Row` is the **effect contribution** of this expression — what effects its
evaluation will perform. The caller decides whether to unify it with an ambient row,
accumulate it into a parent expression's row, or check it against `Row::Empty` at the top level.

*Why not (α) effect-on-arrow only:* would require either an out-parameter or rewriting every
arm to thread an accumulator. Less natural for handlers (B3), which need to talk about
"the row of the expression being handled."

*Why not also pass `ambient: &Row`:* the ambient-passing style (Koka-like) adds a parameter
every call site has to thread without buying anything for B2.3's scope. The caller can
synthesize an ambient by unifying contributions.

### D2: Per-variant effect contributions

| Variant | Type | Row contribution |
|---|---|---|
| `Int`, `Bool`, `Var` | unchanged | `Row::Empty` |
| `Lambda { param, body }` | `param_ty -[ρ]-> body_ty` where ρ is fresh; body's row unified with ρ | `Row::Empty` (the closure value is pure; *calling* it is what performs effects) |
| `App { callee, arg }` | result var | `arg_row ∪ callee_row ∪ ρ_call` where callee is unified against `arg_ty -[ρ_call]-> result_var` |
| `Let { value, body }` | `body_ty` | `value_row ∪ body_row` |
| `LetRec { value, body }` | `body_ty` (see D4) | `value_row ∪ body_row` |
| `If { cond, then, else }` | unified branch type | `cond_row ∪ then_row ∪ else_row` |
| `BinOp { lhs, rhs }` | per `binop_signature` | `lhs_row ∪ rhs_row` |
| `Perform { label, arg }` | declared ret type from `eff_env` | `Cons(label, decl_arg_ty, decl_ret_ty, Row::Empty)` |

"Union" of rows is implemented as iterated `unify_row` against a fresh tail variable,
accumulating the result. Honest disclosure: a simpler alternative is to thread a single
accumulator row variable through the entire `{infer}` walk and have every contributing arm
unify into it. This is equivalent but harder to test in isolation. We use the "return
contribution, unify at parent" style.

### D3: Lambda is the only row introducer

Only `ExprKind::Lambda` calls `supply.fresh_row()` to mint a row variable that will appear
in resulting types. App also calls `fresh_row` for its unification target, but that row var
lives only long enough to be unified against the callee's arrow and then gets resolved.
Rationale: rows on arrows describe "what effects a call will perform," and only lambdas
create arrow types.

### D4: LetRec adopts lambda's row machinery

Per the existing B1 invariant (parser-enforced: `let rec` RHS must be a syntactic lambda),
the RHS *is* a lambda, so it already mints its own row variable inside its own `Lambda` arm.
`LetRec`'s job stays as it was in B1: stub the recursive name with a fresh monomorphic `Ty`
variable, infer the RHS lambda (which now has the form `arg -[ρ]-> ret`), unify the stub
against the inferred arrow, generalize for the body. The row variable ρ becomes part of
the generalized scheme.

**Open question pushed to B3:** should row variables be generalizable alongside type
variables? For B2.3 we generalize them (D5); the standard ML value restriction does not
apply here because Sentinel-Mini has no mutable refs. If row generalization causes
incoherence, B3 can introduce the restriction.

### D5: Generalization includes row variables

`generalize(ty, env_free)` currently quantifies free type variables of `ty` not in `env_free`.
B2.3 extends `Scheme` to also carry quantified row variables, and `generalize` collects free
row vars from `ty` minus those free in env. `instantiate` mints fresh row vars for the
quantified ones.

`Scheme { vars: Vec<TyVar>, row_vars: Vec<RowVar>, ty: Ty }`. Display becomes
`forall 'a 'b ρ0 ρ1. ...`. Existing `Scheme::mono(ty)` and tests stay green because the new
field is empty by default.

### D6: Strict `infer_top`, permissive `infer_program` ()

`infer_top(&Expr)b  — the B1 entry, called by ~10 existing infer tests — synthesizes a fresh
row, runs `infer`, applies the final substitution to the returned row, and:

- If the resolved row is `Row::Empty`: succeeds, returns the type.
- Otherwise: returns `TypeError::UnhandledEffects { row, span }`.

Every existing B1 test program is pure, so every existing call to `infer_top` will resolve
to `Row::Empty` and pass unchanged. (This is the invariant we rely on for the
"behavior-identical refactor" claim in B2.3a.)

`infer_program(&Program)` — the B2.2b entry — builds `EffectEnv` from `program.effects`,
runs `infer`, and **returns the type ignoring the residual row**. A program like
`effect Print : Int -> Bool ; do Print(1)` will type-check successfully in B2.3, with the
`Print` effect floating up to the top level unhandled. B3 will tighten this: handlers will
be the only way to discharge effects, and `infer_program` will become strict in the same
sense `infer_top` is now.

### D7: Unknown effect label is a distinct error

```rust
#[error("unknown effect: no declaration found for {label:?}")]
UnknownEffect { label: String, span: Span },
```

Distinct from `Unbound { name, span }`. Effects and values live in disjoint namespaces;
`do Print(1)` without a declaration is not the same as `print(1)` without a `let print = ...`.
The diagnostic should reflect that.

### D8: Surface `TyExpr` → `Ty` conversion is pure

Declared effect signatures (`effect Print : Int -> Bool ;`) are converted from `TyExpr` to
`Ty` with `Row::Empty` on every arrow. ADR 0004 left this implicit; making it explicit
about here: declared types in B2.3 are first-order, effect-free on their internal arrows.
Higher-order effectful signatures (e.g., `effect Map : (Int -> {ε} Int) -> List -> List`)
require surface row syntax that B2.3 does not introduce. Pushed to B?.

### D9: Phase split — B2.3a refactor, B2.3b semantics (LANDED)

**B2.3a (refactor, behavior-identical):**

- `infer` signature changes to three-tuple.
- All call sites threaded.
- `Scheme` gains `row_vars: Vec<RowVar>` (empty default).
- New `EffectEnv` type alias; passed to `infer` but unused.
- All arms return `Row::Empty` as their contribution.
- Lambda still uses `Ty::arrow(a, b)` (empty row).
- Perform still errors with `EffectNotYetSupported`.
- All 134 existing tests must pass unchanged.

**B2.3b (semantics):**

- Lambda mints fresh ρ via `supply.fresh_row()`, uses `arrow_with`.
- App unifies callee against `arrow_with(arg_ty, ρ_app, result_var)`.
- Perform looks up label in `EffectEnv`, contributes `Cons`.
- `TypeError::EffectNotYetSupported` removed.
- New `TypeError::UnknownEffect` and `TypeError::UnhandledEffects`.
- `infer_top` strict check on residual row.
- `infer_program` builds `EffectEnv` from `program.effects`.
- Generalization extended to row variables.
- New tests: ~8 in `infer.rs` (Perform with declared effect, unknown effect, lambda row
  var introduction, app row unification, let-row-polymorphism, infer_top rejects unhandled,
  infer_program accepts unhandled), 2 integration tests.

Target test count: 134 → 134 (B2.3a, zero net change) → ~144 (B2.3b).

## Consequences

**Positive:**

- Clean B2.3a/B2.3b bisection point: any test regression between the two clearly attributes
  to semantics, not refactor.
- Strict/permissive split (D6) preserves every B1 test without modification.
- `EffectEnv` is a typed surface, not stringly-coupled — B3 handlers will extend `EffectEnv`
  with handler-discharge information.
- D7's separate `UnknownEffect` produces better diagnostics than reusing `Unbound`.

**Negative:**

- `Scheme` shape changes (D5) ripple to any external consumer. Internal-only at B2 scope;
  no consumers outside the crate.
- "Union of rows = iterated unify_row" (D2) is O(n2) in the number of contributions per
  expression. Fine at B2 scale; revisit if profiling demands.
- D8 means surface-declared signatures can't carry effects on their internal arrows.
  Acceptable for B2; pushed to B?.

**Risks:**

- Row generalization (D5) may interact badly with `LetRec`. If it does, B2.3 retrospectives
  will document the fix.
- "Lambda's row contribution is `Row::Empty`" (D2) is the standard formulation but readers
  from imperative effects literature may expect otherwise. The ADR is the place this is
  decided; future contributors disagreeing should open ADR 0006.

## Alternatives considered and rejected

- *(α) Effect-on-arrow only.** Cheaper for B2.3 alone, more painful for B3 handlers.
  Rejected per D1.
- **Ambient-row-passing (Koka style).** Adds a parameter without buying anything for B2.3.
  Rejected per D1.
- **(γ) Strict everywhere.** Rejected for ergonomic reasons; having `infer_top` and
  `infer_program` differ in strictness maps exactly to the "B2 vs B3" semantic boundary.
- **(δ) Permissive everywhere.** Rejected: every B1 test that currently asserts
  `infer_top(expr).is_err()` could newly succeed with a non-empty residual row.
- **Reusing `TypeError::Unbound` for unknown effects.** Rejected per D7.
- **Single B2.3 commit.** Rejected per D9.

## References

- ADR 0002 (effect rows in mini)
- ADR 0003 (B1 retrospective)
- ADR 0004 (row representation and effect surface)
- Leijen, Daan. "Extensible Effects." 2014. (Koka-style ambient passing, considered and rejected.)
- Rémy, Didier. "Type inference for records in a natural extension of ML." 1989. (Source of `unify_row`.)
- Lindley & Cheney. "Row-based effect types for database integration." 2012. (Closer to the formulation adopted.)
