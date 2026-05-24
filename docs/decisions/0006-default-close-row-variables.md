# ADR 0006 -- Default-Close Unconstrained Row Variables at Top Level

**Status:** Accepted (2026-05-24). Refines ADR 0005 D2 / D6.
Governs B2.3b1 implementation.

## Context

ADR 0005 D2 specifies that `ExprKind::Lambda` mints a fresh row variable
`ρ` for the arrow it constructs, and that the body's row contribution
unifies with ρ. For a pure lambda (one that performs no effects), ρ is
unconstrained: nothing in inference forces ρ := Empty.

This is fine inside the inferer -- ρ is just an unsolved unification
variable -- but it surfaces in two places:

1.  **`infer_top` return type.** A program like `fn(x) => x` has inferred
    type `'a -[ρ]-> 'a` where ρ is a fresh row var. After the final
    `s.apply(&t)`, ρ is *still* a free row var (no substitution binds
    it). The returned `Ty` therefore renders as `'a -> {'ra} 'a` per
    `Row::Display`, which is technically correct but surprising:
    "where did `'ra` come from?"

2.  **Existing B1/B2 test assertions.** Tests like
    `letrec_factorial_typechecks_at_int_to_int` assert
    `ty_of(src) == Ty::arrow(Ty::Int, Ty::Int)` -- equality against a
    specific empty-row arrow. With ρ free, the assertion becomes
    `Ty::Fun(Int, Row::Var(ρ), Int) == Ty::Fun(Int, Row::Empty, Int)`,
    which fails.

ADR 0004's Revisit section explicitly anticipated this:

> the closed-row-only-at-toplevel rule produces confusing top-level
> inferred types (e.g. `int / {Print}` when the user expected `int`
> because there's no row at the top level), in which case D2 needs an
> explicit "default-close at top level" rule

This is that rule.

## Decisions

### D1: Default-close unconstrained row variables at `infer_top`

After `s.apply(&t)` in `infer_top`, walk the resulting `Ty` and replace
every `Row::Var(v)` (that the substitution did not resolve) with
`Row::Empty`. The walk is a pure transformation on `Ty`, implemented as
`Ty::close_rows() -> Ty`.

Specifically: any row variable that, after substitution, is *still* a
`Row::Var(v)` -- i.e., no constraint anywhere in the inference forced it
to take a specific shape -- is replaced with `Row::Empty` in the
returned type. The substitution itself is not modified; this is a
post-processing zonk on the surface type only.

### D2: Order with respect to the residual-row check

`infer_top` performs three steps in this order:

1.  Resolve the returned row contribution: `let resolved = s.apply_row(&r)`.
2.  **Strict residual check.** If `resolved != Row::Empty`, return
    `TypeError::UnhandledEffects { row: resolved, span }`.
3.  Resolve and default-close the returned type:
    `Ok(s.apply(&t).close_rows())`.

The residual-row check fires on the *returned row contribution* (an
expression's effects). Default-close acts on row variables *embedded
inside arrow types* in the returned `Ty`. They are orthogonal axes;
pinning the order makes the implementation unambiguous.

### D3: `infer_program` also default-closes the returned type

`infer_program` is permissive about the residual row contribution per
ADR 0005 D6 (effects floating up to the top level are accepted in B2),
but it should still return a *closed* type. A program like
`effect Print : Int -> Bool ; fn(x) => x` should return
`'a -> 'a`, not `'a -[ρ]-> 'a`. The default-close walk applies
uniformly; only the residual-row check differs between `infer_top`
(strict) and `infer_program` (permissive).

### D4: Generalization interacts cleanly

ADR 0005 D5 specifies that `generalize` quantifies free row variables.
Inside a let-binding, the bound scheme is `forall 'a ρ. 'a -[ρ]-> 'a`.
At each instantiation site (`Let { value, body }`), `instantiate` mints
fresh row vars for each quantified ρ. Default-close runs only at
`infer_top` / `infer_program`, *not* inside `instantiate` or
`generalize` -- so row polymorphism inside a program is preserved.

Concretely: the test
`b23b_let_row_polymorphism` (if added in B2.3b2) checks that
`let f = fn(x) => x in let g = fn(x) => x in 1` types to `Int` even
though `f` and `g` each have row-polymorphic schemes. Default-close
only affects what the *outermost* `infer_top` returns to the caller,
which in this case is just `Int` -- no row vars to close.

### D5: Default-close is a `Ty` method, not a `Subst` operation

The walk lives as `impl Ty { pub fn close_rows(self) -> Ty }`, not as a
modification to `Subst::apply`. Rationale: default-close is a *policy*
("what does an unconstrained row mean at the top level?"), not a
*resolution* ("what does this row var resolve to?"). Keeping it
separate means the substitution machinery stays semantically pure --
`apply` always resolves what's bound, never invents what isn't. Only
top-level entry points (`infer_top`, `infer_program`) apply the policy.

This also keeps B3 free to change the policy without touching `Subst`.
If handlers introduce a case where a free row var at the top level
*should* be an error (because it represents an effect with no
handler), B3 can replace `close_rows()` with a stricter check at the
same call sites.

## Reasoning

**Why default-close instead of explicit `pure` annotation (rejected):**
ADR 0004 D2 already rejected closed-rows-by-default as the *inference*
policy because effect polymorphism is Phase B's whole point. But the
*surface presentation* of a fully-inferred top-level type can be closed
without affecting inference: closed-at-presentation is what users
expect ("I wrote a pure function, I should see no effects in its
type"), and matches both Koka and Haskell's defaulting behavior for
ambiguous polymorphic variables.

**Why a separate ADR rather than folding into 0005:** ADR 0005 D2 and
D6 jointly imply a free row variable can survive to `infer_top`'s
return, but neither addresses what to do about it. Discovering this
during B2.3b1 implementation -- as actually happened -- means the
decision deserves its own discoverable record. Future contributors
reading 0005 alone would not know this rule exists; reading 0006
alongside 0005 gives the full picture.

**Why `infer_program` also default-closes (D3):** symmetry. The only
difference between `infer_program` and `infer_top` is residual-row
strictness; type-presentation policy should be uniform.

**Why a `Ty` method rather than threading through `Subst` (D5):**
keeps `Subst` invariants stable. `Subst::apply` should always be
sound: applying twice is identical to applying once, and `apply` never
loses information. Folding default-close into `apply` would break the
second property (it converts unbound row vars into Empty, discarding
the fact they were free). The `Ty` method makes the discarding explicit
at the policy site.

## Consequences

### Positive

- All 41 existing infer tests that destructure `Ty::Fun(a, _, b)` and
  the one that asserts equality against `Ty::arrow(Int, Int)` continue
  to pass under B2.3b1 unchanged.
- Inferred top-level types are user-readable: a pure lambda displays
  as `'a -> 'a`, not `'a -> {'ra} 'a`.
- Row polymorphism inside let-bindings is preserved (D4); only the
  outermost surface is closed.

### Negative

- A user who *wants* to see the unsolved row variable for debugging
  cannot at the top level. Mitigation: add a debug entry point if
  needed in B3 (`infer_top_raw` or similar) -- not a B2.3 concern.
- One additional `Ty` walk on every top-level inference. O(size of
  returned type); negligible at any realistic scale.

### Neutral

- Default-close is a policy decision that could go the other way in
  a future language (e.g., one that wanted to *force* users to
  annotate purity). Sentinel-Mini's stance is "infer purity silently
  when it can"; Phase C may revisit when capability-checking lands.

## Alternatives considered and rejected

- **Leave free row vars in the surface type.** Rejected: surprising
  user-facing output, breaks existing test assertions for no
  semantic gain.
- **Generalize at `infer_top` instead of defaulting.** Would return
  `forall ρ. 'a -[ρ]-> 'a` as a `Scheme` instead of a `Ty`. Rejected:
  changes the return type of `infer_top` (cascades to every caller)
  and offers no benefit over defaulting at this scope -- nothing
  consumes a top-level `Scheme` in B2.
- **Default-close inside `Subst::apply`.** Rejected per D5
  reasoning -- breaks `apply`'s idempotence invariant.
- **Default-close inside `instantiate` / `generalize`.** Rejected
  per D4 -- destroys row polymorphism inside let-bindings.

## References

- ADR 0004 (row representation; Revisit section anticipates this)
- ADR 0005 D2, D6 (effect-inference judgment; created the gap this
  ADR fills)
- Leijen, Daan. "Koka: Programming with Row Polymorphic Effect Types."
  2014. (Koka's default-close behavior for unconstrained ambient rows.)
- GHC user's guide, "Defaulting." (Haskell's analogous defaulting for
  ambiguous type-class variables; same pattern.)

## Amendment (2026-05-24, B2.3b1 implementation)

D1 originally specified default-close only on the returned `Ty` at
`infer_top` / `infer_program`. Implementation revealed a second
locus where unconstrained row variables surface: **row contributions
returned by `infer`**. Whenever `App` unifies a callee of type
`Ty::Var(_)` against `arrow_with(t_arg, ρ_call, result)` -- which
happens for every recursive call and every higher-order parameter
application -- `ρ_call` ends up free in the App's row contribution.

Extended rule: a free `Row::Var` in a *contribution* (the third
component of `infer`'s return tuple, and the operands of
`row_union`) is treated as `Row::Empty`. Concretely:

1. `infer_top` calls `.close()` on the resolved residual row before
   the strict `UnhandledEffects` check (so unconstrained ρ_call
   never trips the check).
2. `row_union` / `cons_onto` / `cons_or_unify` treat `Row::Var`
   operands as `Row::Empty` rather than `unreachable!`.

This preserves D4 (row polymorphism inside let-bindings is
unaffected; the change only touches contributions, never types
inside `Scheme`s). It also sharpens what the strict residual-row
check actually catches: declared effects (Cons-chains terminating
in Empty) that escaped a handler, never unsolved row variables.

B3 revisits when handlers bind row variables that *must* propagate
to caller contributions.
