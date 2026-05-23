# ADR 0002: Pure HM in B1, effect rows added in B2

## Status

Accepted, 2026-05. Supersedes nothing.

## Context

Phase B of the staged-validation plan (ADR 0001, HANDOVER §5) builds
Sentinel-Mini, a research-grade interpreter whose purpose is to
validate Sentinel's effect-system design before Phase C commits to a
production compiler. The B1 milestone introduces a static type system;
B2 adds effect declarations and B3 adds handlers.

A real choice presents itself at the boundary between B1 and B2:

**Option A — row-ready HM in B1.** Build the B1 type system with
effect-row infrastructure already present in `Ty`, `Scheme`, and
`unify`. Concretely: function types are `Fun(Ty, Row, Ty)` from day
one; rows are either empty, a variable, or `Cons(label, row)`; the
unifier knows how to unify rows (with the standard Rémy-style
row-tail trick). At B1 every program is inferred to have the empty
row, so no surface syntax changes. B2 then *only* adds effect
declarations and the syntactic surface for them — no inferer
rewrite.

**Option B — pure HM in B1, layer effects in B2.** B1's `Ty` is
`Int | Bool | Var | Fun(Ty, Ty)`. The unifier is the textbook two-arm
implementation. B2 rewrites the type representation to add rows and
extends `unify` accordingly. The B1 implementation is smaller and
strictly easier to test in isolation.

HANDOVER §8 calls out "row polymorphism formulation is the standard
one; implement it even though it is harder than the alternatives,
because the alternatives do not compose" — but that guidance is
addressed at the Phase C production compiler, not the Phase B
prototype, and is about *eventually* using rows, not about *when* to
introduce them within the prototype.

Sentinel-Mini's stated charter (HANDOVER §5, STATE.md §B) is research
code that is "explicitly expected to be thrown away or rewritten once
its lessons are absorbed." This shapes the answer.

## Decision

**Adopt Option B: pure HM in B1, layer effect rows in B2.**

B1's type representation is the minimal one needed to typecheck the
B0 surface plus `letrec`:

    enum Ty { Int, Bool, Var(TyVar), Fun(Box<Ty>, Box<Ty>) }
    struct Scheme { vars: Vec<TyVar>, ty: Ty }

(Indented as a code block rather than fenced, to keep this ADR safe
inside shell heredocs in patch scripts. The production rendering is
unaffected — Markdown indented code blocks render identically.)

B2 will widen `Ty::Fun` to `Fun(Box<Ty>, Row, Box<Ty>)`, introduce
`Row`, and extend `unify` with the row-tail rule. Existing B1 call
sites that construct or destructure `Fun(_, _)` will need a
mechanical update; this is a one-commit refactor across a research
crate where the migration surface is the prototype's own code, not
external users.

## Reasoning

Four reasons, in order of weight:

1. **B1 is itself a learning instrument.** The point of writing the
   inferer at all is to find out what the inferer needs. Committing
   to a row representation before having written a single typing
   rule risks designing rows that fit textbook examples but not the
   things B2 actually wants to express (effect subtyping?
   presence/absence polymorphism? labelled vs unlabelled rows?). The
   B2 design should be informed by what B1 taught us about how the
   inferer wants to be shaped, not anticipated in advance.

2. **The migration is cheap and contained.** Sentinel-effects-proto
   has no external users. STATE.md §B explicitly frames the crate as
   throwaway research code. The "we'll have to rewrite the inferer"
   cost that the row-ready-from-day-one argument is trying to avoid
   is small in absolute terms: an inferer for B1's surface fits in
   one file, and the mechanical Fun(a, b) -> Fun(a, row, b) walk
   is the kind of refactor a working type system catches all of at
   compile time.

3. **Two failure modes are asymmetric.** If we go row-ready in B1
   and the row design turns out to be wrong, we pay the cost twice:
   once to build it, once to redesign it. If we go pure-HM in B1
   and the migration turns out to be painful, we pay it once, with
   full B1 lessons in hand. The asymmetry favours the pure-HM path.

4. **Smaller B1 means earlier B1.** B1 is the prerequisite to
   everything downstream. Keeping it small means we get to B2
   (which is where the actually novel research lives) sooner and
   with a more debuggable foundation.

## Consequences

### Negative

- B2 includes a non-trivial refactor: every `Ty::Fun` construction
  and pattern match across `infer.rs` will touch the row. Estimated
  scope: ~20-40 call sites in one file. We will land this as the
  first commit of B2 (call it B2.0), with no behavioural change —
  every function gets the empty row, every unification adds a
  trivial empty-row unification step — so any test failure
  immediately localises to the refactor itself.

- We do not get the option of writing test programs in B1 that
  illustrate row behaviour. This is fine; the B1 acceptance criteria
  (HM + letrec + spans) do not require it.

### Positive

- B1's `Ty`, `Scheme`, and `unify` fit on a page each. Reviewable.
  Comparable to every HM textbook implementation, which means the
  reference material applies directly.

- B1 ships sooner. The effect-row research happens in B2 with the
  full inferer-as-it-actually-shipped to build on, not against a
  paper sketch.

- The ADR creates a forcing function for B2 to *deliberately*
  choose its row design — labelled vs unlabelled, presence
  polymorphism, subtyping vs equality — rather than inheriting
  whatever was guessed at in B1.

### Neutral

- The Fun(a, b) -> Fun(a, row, b) migration in B2 is a useful test
  of the codebase's refactorability and of the test suite's
  coverage. If a refactor of that scope is hard, that's diagnostic
  of a structural problem worth finding out.

## Alternatives considered

- **Option A (row-ready B1)** is genuinely defensible. It would be
  the right call if (a) Sentinel-Mini had external users today,
  which it does not; (b) we had a confident, fixed design for what
  effect rows should look like in Sentinel, which we do not (the
  whole *point* of Phase B is to find out); or (c) the B1 schedule
  was generous enough to absorb the extra design work, which is not
  the framing in HANDOVER §5.

- A third option — skip the type system entirely and go straight to
  dynamically-checked effects — was rejected because HANDOVER §5
  explicitly lists "Hindley-Milner-style type inference with effect
  rows" as a B-phase requirement and because effects whose checking
  is dynamic do not validate the *static* capability-enforcement
  story that is Sentinel's headline.

## Revisit

If B2's first attempt at row inference reveals that the B1 `Ty`
shape is structurally incompatible with rows in some way not
anticipated here (for example, if we discover we want rows on
*every* type, not just function arrows), revisit this ADR with a
postmortem and update.
