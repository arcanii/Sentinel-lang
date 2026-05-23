# ADR 0003: B1 Retrospective

- Status: Accepted
- Date: 2026-05-23
- Related: ADR 0001 (staged validation), ADR 0002 (effect rows in Mini)

## Context

Phase B1 of `sentinel-effects-proto` completed across five commits
(abfb3d9, b3589ea, 24a3db8, 72c0996, e6b06cd). It added:

- Span-tracked AST (`Spanned<ExprKind>`), preserved through
  parsing and available to diagnostics.
- `let rec` with runtime knot-tying via `OnceLock` and
  proper HM generalization in the body (monomorphic
  recursive occurrence inside the RHS).
- A Hindley-Milner type inference driver (Algorithm W) wired
  into `run()`, so type errors abort before evaluation.
- Hand-rolled caret diagnostics (`diag.rs`, ~110 LoC,
  zero new deps), exposed via `MiniError::render(src)`.

This document captures what we learned that affects B2's
design, and what we're deliberately carrying forward.

## Observations from B1

**O1. The stub-then-generalize sequencing for `let rec` worked.**
B1.5 landed a monomorphic `let rec` stub (factorial
typed, polymorphic identity did not) and B1.6 replaced
the stub with real generalization in ~25 lines of change.
The intermediate state was releasable (all tests green,
runtime semantics unchanged). Repeat this shape for B2:
land effect-empty-always first, then effect rows with real
unification.

**O2. Eager substitution maps were fine.**
`Subst` is a `HashMap<TyVar, Ty>` with `apply` called at
`compose` time. No union-find, no path-compression. 41 infer
tests run in under 1ms total in dev builds. Do not rewrite
for B2; gate any change on actual profile data.

**O3. Per-variant `span` fields preferred over a blanket
`Spanned<Error>` wrapper.**
`ParseError::UnexpectedEof` genuinely has no token to point
at; `EvalError` variants currently have none (backlog item,
see BACKLOG §0.4). The shape `enum FooError { X { span: Span, ... },
YZ; }` keeps the absence honest. B2's effect-checker
should follow the same rule.

**O4. `Option<Span>` in `diag::render` is the right API.**
Span-less rendering collapses to `severity: message`. No
fake spans, no synthetic "end of file" locations. Keep this
invariant in B2: if an effect-row error can't point at a
specific operation, emit None; don't fabricate.

**O5. Hand-rolled diagnostics are cheap.**
`diag.rs` is ~110 LoC and has 7 tests. Compared to adopting
miette (a new top-level dep, a new error trait, and new runtime
printing) this was the right call for a prototype. Reconsider
miette at Phase C, not B2.

**O6. zsh-paste-safe script discipline matters.**
Two scripts in B1 aborted on unquoted parentheses and bare
`exit` commands; one closed the user's terminal. All
subsequent scripts used: no `exit`, no `unquoted parens` in
echoes, all logic inside a function called at the bottom,
python3 for any file surgery that doesn't fit in sed. Make
this an explicit pattern in HANDOVER §0.1 for B2.

## Recommendations for B2

**R1. Rows extend `Ty::Fun`.**
Current shape: `Fun(Box<Ty>, Box<Ty>)`. B2 shape:
`Fun(Box<Ty>, Row, Box<Ty>)` where `Row` is itself a
discriminated union (closed empty, open var, cons-cell).
The refactor touches ~20-40 call sites (per ADR 0002's
estimate); measure actual touch count after B2 prototype.

**R2. Unification of rows is a separate function.**
Do not inline row-unification into `unify`; give it its
own function (`unify_row(Subst, Row, Row, Span)`). This
mirrors the Koka/Effekt-paper shape and keeps `unify`
readable.

**R3. `TypeError` gains row-specific variants.**
`LabelMismatch`, `RowIncompatible`, etc. Reference Spans at
the operation site, not at the function type.

**R4. Keep `run()`'s shape.**
render-at-toplevel (`{ lex->parse->infer->eval }`) should not
change for B2; effects are additive inside infer.

**R5. `let rec`-missing-lambda is a parser rule.
**
If handlers (B3) can appear as RHS, relax then. Until B3 has
a design, keep the restriction.

## Open questions carried into B2

- Should equality (`==`) be constrained by a type class
  (`Eq a`) in B1's successor, or do we defer the class
  system entirely to Phase C? B1's runtime-rejection is a
  hold-my-beer shape; it's time to decide.
- Where does the broker fit into the evaluator? HANDOVER
  places this as an optional B? milestone. If not in B2,
  when? Candidate: after effect handlers (B3), before C?

## References

- ADR 0001: staged validation (Phases A/B/C/D).
- ADR 0002: effect rows deferred to B2.
- docs/STATE.md §B: authoritative crate state.
- crates/sentinel-effects-proto/src/infer.rs: the inference
  driver this retrospective discusses.

*End of ADR.*
