# ADR 0007: Effect handler design

Status: ACCEPTED
Date: 2026-05-24
Related: 0003 (let-rec restriction), 0005 (effect surface), 0006 (effect rows)

## Context

B2.3 landed row-polymorphic effect inference. `perform L x` contributes
`Cons{L, arg, ret, Empty}` to the ambient row; `infer_top` strictly rejects
any residual row, `infer_program` is permissive (ADR 0006 D6). The runtime
half is still a placeholder: `EvalError::EffectNotYetSupported` fires from
`eval.rs:51` (perform site) and `eval.rs:161` (operation dispatch stub).
B3 is the phase that fills that hole.

Handler implementation has enough entangled design choices that discovering
them during coding would lead to churn. This ADR pins down surface syntax,
AST shape, typing rule, row subtraction algorithm, runtime model,
interactions with existing decisions, and the criterion for B3 being
complete.

## Decisions

### D1. Surface syntax

The handler form is:

    handle e with {
      L1(x, k) => body1,
      L2(x, k) => body2,
      return v => ret_body
    }

The `return` arm is optional. When omitted, it defaults to `return v => v`,
which forces `T1 = T2` in the typing rule (D3) and makes the handle
expression's type equal to the body's type. When present, `T1` and `T2`
may differ.

The arm separator is `,`. This is consistent with future record syntax and
leaves `|` free for a possible `match` construct. Arms may span multiple
lines; the parser is not whitespace-sensitive.

`handle ... with { ... }` is an expression at the same precedence as `if`.
The body `e` extends as far right as the expression grammar allows, then
`with` is required. No first-class handler value form is introduced in B3;
handlers are syntactic. First-class handlers can be revisited later
without breaking source.

Handler arm bodies are checked against the outer row (the row after L is
discharged), not the inner row. This is what makes the continuation type
work (D3) and is the point of row subtraction (D4).

### D2. AST shape

    pub struct HandlerArm {
        pub label: String,
        pub arg: String,
        pub kont: String,
        pub body: Expr,
        pub span: Span,
    }

    pub struct ReturnArm {
        pub var: String,
        pub body: Expr,
        pub span: Span,
    }

    ExprKind::Handle {
        body: Box<Expr>,
        arms: Vec<HandlerArm>,
        ret_arm: Option<ReturnArm>,
        span: Span,
    }

Arms are `Vec<HandlerArm>`, not `HashMap`, to preserve source order for
diagnostics and keep `Debug`/`Clone` derivation consistent with the rest
of the AST. Duplicate-label detection within a single handler is a
typing-phase check (`DuplicateHandlerArm`), not a parser check.

The continuation parameter `k` is bound only in the handler arm body, not
in the return arm. We record this because it is the most common point of
reader confusion.

### D3. Typing rule

Premises:

- `Gamma |- e : T1 ! rho_inner`
- `rho_inner` is equivalent (up to row reordering) to
  `{L_i : A_i -> B_i | rho_outer}` where the `L_i` are exactly the labels
  named in the handler arms.
- For each arm `L_i(x_i, k_i) => body_i`:
  `Gamma, x_i : A_i, k_i : B_i -> T2 ! rho_outer |- body_i : T2 ! rho_outer`
- For the return arm `return v => ret_body`:
  `Gamma, v : T1 |- ret_body : T2 ! rho_outer`

Conclusion:

- `Gamma |- handle e with { ... } : T2 ! rho_outer`

Key points:

Every label appearing in a handler arm must appear in `rho_inner`. If not,
typing fails with `HandlerLabelNotInRow { label, span }`. The opposite
case, labels in `rho_inner` not covered by arms, is fine; they pass
through into `rho_outer`.

The continuation's return type is `T2`, not `T1`. This is the deep handler
invariant: invoking `k` runs the rest of the body under this handler, so
it produces whatever the handler ultimately produces.

The continuation's effect row is `rho_outer`. The L effects are discharged
by the time `k` is invoked, so `body_i` is checked against the outer row.

Shallow handlers would type `k : B_i -> T1 ! rho_inner`. We choose deep
(see D5).

### D4. Row subtraction

B3.1 introduces a new operation, dual to `row_union`:

    row_split(rho, L) -> Result<((Ty, Ty), Row), TypeError>

Cases:

1. `rho = Cons{label: L, arg, ret, tail}`: return `Ok(((arg, ret), tail))`.

2. `rho = Cons{label: L', arg, ret, tail}` with `L' != L`: recurse on
   `tail`. If it returns `Ok(((a, r), rest))`, return
   `Ok(((a, r), Cons{L', arg, ret, rest}))`. This is row equivalence up
   to reordering.

3. `rho = Row::Var(rho_var)`: mint fresh `alpha`, `beta`, `rho'`. Unify
   `rho_var := Cons{L, alpha, beta, rho'}`. Return
   `Ok(((alpha, beta), Row::Var(rho')))`. This case is what makes
   handlers compose with row-polymorphic callers.

4. `rho = Row::Empty`: return `Err(HandlerLabelNotInRow { label: L, span })`.

ADR 0006's "Var-in-contribution treated as Empty" rule does not apply
here. That rule was for union, where treating an unresolved row variable
permissively kept inference total. Subtraction is different: the residual
row really must be a fresh tail variable so callers can extend it.
Implementers should not reuse `cons_onto` semantics for `row_split`.

Duplicate labels in rows. Rows may contain semantically-equivalent
duplicates (e.g. two `perform Console.Print` sites unioning to a row that
mentions `Console.Print` twice if signatures differ, which is itself a
union error; or trivially deduped if identical). `row_split` peels the
leftmost occurrence. This is sound under deep handlers because the
continuation re-handles on each invocation, so a second occurrence would
hit the same handler again on the next perform. Duplicate labels in a
single handler's arms are forbidden (`DuplicateHandlerArm`).

### D5. Runtime model

Deep handlers, one-shot continuations, free-monad-style operation
reification.

Evaluation returns a step enum rather than a bare value:

    enum Step {
        Value(Value),
        Op { label: String, arg: Value, kont: Continuation },
    }

`Continuation` is a reified frame stack (concretely a `Vec<Frame>` where
each `Frame` captures the local environment and the remaining expression
to evaluate, or equivalently a boxed `FnOnce(Value) -> Step`; pick the
data form for `Clone`/`Debug` ergonomics).

`perform L x` evaluates `x` to a value `v`, then returns
`Step::Op { label: L, arg: v, kont: Continuation::empty() }`.

Each evaluation frame, when its sub-evaluation yields `Step::Op`,
prepends itself to `kont` and re-raises. This is the "bubbling" that
free-monad reification gives for free.

`handle e with H` evaluates `e`:

- If `e` yields `Step::Value(v)`, invoke the return arm with `v` bound
  to its parameter; the result is the handle's result.
- If `e` yields `Step::Op { label: L, arg, kont }` and `H` has an arm
  for `L`, bind `x := arg` and `k := \v. handle (kont.resume(v)) with H`,
  then evaluate the arm body. The `handle ... with H` re-wrap on
  resumption is what makes the handler deep.
- If `H` has no arm for `L`, prepend the handle frame onto `kont` and
  re-raise. Outer handlers get a shot.

One-shot enforcement. `Continuation` carries an internal
`Cell<Option<Frames>>`; `resume` calls `take()` and panics or returns
`EvalError::ContinuationAlreadyResumed` on the second invocation.
Multi-shot via continuation cloning is out of scope for B3 and recorded
under "Considered and rejected" below.
### D5 implementation note (B3.2 landed)

B3.2 implemented D5 across three commits: B3.2a (bdda217) scaffolding,
B3.2b (a9cefb1) substantive runtime, B3.2c (8e3de20) positive coverage.
Concrete shape choices made during implementation:

`Continuation` is a `Vec<Frame>` enum, not a boxed `FnOnce`. The data
form was chosen for `Debug`/`Clone` ergonomics and for keeping the
"what eval was about to do next" states explicit and greppable. Eight
frame variants enumerated by counting resume-points in the existing
eval arms (LetBody, LetRecBody, IfBranch, AppArg, AppCall, BinOpRight,
BinOpApply, PerformReify); a ninth, HandleFwd, was added in B3.2b to
record handlers that a non-matching Op walks past, so resumed
computations get re-wrapped through the same handler (the deep re-wrap
travels via this frame, not via the arm-body `\v. handle (kont.resume v)
with H` closure the ADR sketches).

Continuation captured at the perform site is reified as a new
`Value::Resumption { kont, arms, ret_arm, env }` rather than the
ADR's `\v. handle (kont.resume v) with H` synthesised closure. The
bundled-context form keeps the deep re-wrap data with the resumption
value, lets `apply` dispatch on `Value` (sibling to `Value::Closure`)
without an App-site lookup into handler state, and avoids requiring a
builtin-function mechanism the prototype does not have. `handle_step`
is the shared dispatch hub used both by the `Handle` arm at evaluation
start and by `apply` at resume re-wrap.

`Continuation` derives `Clone` because `Value` is `Clone` and
`Value::Resumption` holds `Continuation` by value. The one-shot
guarantee therefore holds per-Continuation-instance (the
`Cell<bool>` resumed-flag fires `EvalError::ContinuationAlreadyResumed`
on second resume of the same Continuation) rather than
per-logical-resumption (a cloned `Value::Resumption` is two
independently-resumable Continuations). Nothing in eval clones a
Resumption today; user-level multi-shot via `Value::Clone` is not
statically prevented. Stricter alternatives are recorded inline at
the `Continuation` definition: a move-only variant, or
`Rc<Cell<bool>>` for a shared flag.

`EvalError::EffectNotYetSupported` and
`EvalError::HandlersNotYetSupported` were both deleted in B3.2b once
their producing arms gained real implementations.
`EvalError::UnhandledOpAtTopLevel { label }` and
`EvalError::ContinuationAlreadyResumed` were added: the former as
defence-in-depth at `crate::run` mirroring B2.2b's posture (the type
system's default-close at `infer_program` should prevent open rows at
the top level; the runtime checks anyway), the latter to actively
enforce one-shot on second resume.


### D6. Interaction with let-rec

ADR 0003 flagged that the `LetRecNotLambda` parser restriction might be
relaxed when handlers arrive. B3 does not relax it. Handler bodies are
expression-position, not let-rec RHS, so the existing restriction does
not block any handler code. We will revisit only if a concrete example
requires it. This closes ADR 0003's open question without committing to
work.

### D7. Interaction with row generalization

Handler arms introduce `k : B -> T2 ! rho_outer`. `rho_outer` is
typically a fresh row variable (case 3 of `row_split`). After typing the
handle expression, `rho_outer` should remain free in the result type and
therefore be quantified by `generalize` at the enclosing `let` boundary.

Canary test, to be added in B3.1: write a state-handler shaped function

    let run_state = fn s => fn comp =>
      handle comp() with {
        Get((), k)  => k(s),
        Put(s', k)  => k(()),
        return v    => v
      }

bind it with `let`, instantiate it twice in different effect contexts,
and confirm the residual row variables are freshened independently. If
`infer_program`'s permissive close-row pass (ADR 0006 D6) eagerly closes
`rho_outer` to `Empty`, `run_state` collapses to monomorphic and the test
fails. This is the canary; the fix, if needed, is to scope the permissive
close so that only top-level residual rows are closed, not rows under
generalized let-bindings.

### D8. Discharge semantics for UnhandledEffects

`infer_top`'s strict residual-row check is already reachable post
B2.3b2-a: any program performing an effect without an enclosing handler
hits it. B3 makes the discharge side reachable too. We keep the existing
asymmetry between `infer_top` (strict, what a hypothetical `main` would
use) and `infer_program` (permissive, what tests and the REPL use) and
document it as a deliberate program/REPL distinction rather than a TODO.

The B3.1 commit should:

- Add a positive test: program with `perform L x` inside `handle ... with`
  type-checks under `infer_top`, no residual row.
- Add a negative test: program with `perform L x` and no enclosing handler
  is rejected by `infer_top` with `UnhandledEffects`.
- Extend `TypeError::UnhandledEffects` to carry the residual row (or at
  least the set of unhandled label names) so the diagnostic tells users
  what they forgot to handle. The current variant is a placeholder
  shaped for the empty-info case.

### D9. Completion markers

B3 is complete when:

- `crates/sentinel-effects-proto/src/eval.rs:51` (perform-site
  placeholder) no longer returns `EvalError::EffectNotYetSupported`. It
  returns `Step::Op { ... }` (or whatever the final runtime API shape
  is) instead.
- `crates/sentinel-effects-proto/src/eval.rs:161` (dispatch stub) is
  either gone or implements actual handler dispatch.
- The `EvalError::EffectNotYetSupported` variant is removed from the
  error enum entirely, parallel to how B2.3b2-b removed
  `TypeError::EffectNotYetSupported`.
- All tests pass, including the D7 canary and the D8 positive/negative
  pair.

Phase breakdown:

- B3.0: surface (lexer + parser). New tokens `handle`, `with`, `return`
  (already lexed? confirm during implementation), `=>` if not already
  present. `ExprKind::Handle` parsed but rejected by inference and eval
  with a clear "not yet implemented" path. Parallel to B2.2a.
- B3.1: typing. `row_split`, `HandlerLabelNotInRow`,
  `DuplicateHandlerArm`, the D3 typing rule, the D7 canary, the D8
  positive/negative pair, the extended `UnhandledEffects` payload.
- B3.2: runtime. `Step` enum, frame reification, `Continuation`,
  `handle` evaluation, removal of `EvalError::EffectNotYetSupported`.
  Probably multiple commits.


### D9 amendment (2026-05-24, B3.0 + B3.1 landed)

Completion criteria status:

- ~~`TypeError::HandlersNotYetSupported` placeholder removed.~~ Done in
  B3.1b (e7958e1). The variant + its `lib.rs` span arm are gone; the
  typing rule per D3 fires for any `ExprKind::Handle`.
- `crates/sentinel-effects-proto/src/eval.rs:51` perform-site
  placeholder: **still in place.** `EvalError::EffectNotYetSupported`
  fires here. B3.2 replaces with operation reification per D5.
- `crates/sentinel-effects-proto/src/eval.rs:161` dispatch stub:
  **still in place.** `ExprKind::Handle` returns
  `EvalError::HandlersNotYetSupported` (added in B3.0). B3.2 replaces
  with deep-handler dispatch.
- `EvalError::EffectNotYetSupported` variant: **still present.** B3.2
  deliverable.
- `EvalError::HandlersNotYetSupported` variant: **still present.** B3.2
  deliverable, parallel to `EffectNotYetSupported`.

Phase breakdown status:

- B3.0 (surface): landed 821b16a. Lexer keywords `handle`/`with`/`return`
  (`return` is now globally reserved); braces `{`/`}`;
  `ExprKind::Handle` with `HandlerArm`/`ReturnArm`; parser at
  `parse_expr` precedence; `ParseError::HandlerArmLabelNotUpper` and
  `ParseError::EmptyHandler`.
- B3.1 (typing): landed across febf379 (row_split +
  HandlerLabelNotInRow) and e7958e1 (typing rule per D3 +
  DuplicateHandlerArm). Notable divergences from the original plan:
  (a) the D7 canary collapses to `b31b_handle_identity_discharges_effect`
  because `infer_program`'s default-close policy (ADR 0006 D6) hides
  observable row polymorphism at the public type level; the canary's
  load-bearing property — that generalize/instantiate compose with
  handler typing — is satisfied transitively by all `b31b_*` tests
  passing without modifying the generalize/instantiate machinery.
  (b) The D8 positive/negative pair is unreachable through
  `infer_program` (permissive) and uncallable through `infer_top` (no
  effect env); strictness coverage comes indirectly through
  `b31b_handle_two_arms_discharges_both` and
  `pipeline_handle_typechecks_then_runtime_is_placeholder`. (c)
  `TypeError::UnhandledEffects` payload extension was reviewed and
  judged unnecessary — `Row`'s `Display` renders residuals as
  `{Label1, Label2 | tail}` which is already human-readable.
- B3.2 (runtime): not started. Plan unchanged from the original ADR.
### D9 amendment (2026-05-25, B3.2 landed)

All completion criteria now satisfied:

- ~~`crates/sentinel-effects-proto/src/eval.rs:51` perform-site
  placeholder.~~ Replaced in B3.2b (a9cefb1) with operation reification
  per D5. `ExprKind::Perform { label, arg, .. }` evaluates `arg`; on
  `Step::Value(v)` produces `Step::Op { label, arg: v, kont:
  Continuation::empty() }`; on `Step::Op` pushes
  `Frame::PerformReify { label }` onto the kont and re-raises.
- ~~`crates/sentinel-effects-proto/src/eval.rs:161` dispatch stub.~~
  Replaced in B3.2b (a9cefb1) with deep-handler dispatch via
  `handle_step`. `ExprKind::Handle { body, arms, ret_arm }` eager-Arcs
  arms and ret_arm once (so subsequent `Resumption`/`HandleFwd`
  constructions share them cheaply), evaluates the body, and routes
  the resulting `Step` through `handle_step`.
- ~~`EvalError::EffectNotYetSupported` variant.~~ Removed in B3.2b
  (a9cefb1).
- ~~`EvalError::HandlersNotYetSupported` variant.~~ Removed in B3.2b
  (a9cefb1).
- ~~All tests pass.~~ 169 lib + 14 integration + 0 doctests. The D7
  canary and D8 positive/negative pair both remain landed-as-no-ops
  per the B3.1 amendment above; the substantive positive coverage for
  B3.2 lives in `tests/integration.rs` as
  `pipeline_handle_resumes_with_arg_through_run` (B3.2b),
  `pipeline_two_effect_handler_discharges_both`,
  `pipeline_arm_body_computes_with_op_arg_before_resume`,
  `pipeline_arm_body_uses_outer_let_binding`, and
  `pipeline_return_arm_runs_on_resumed_value` (B3.2c).

Phase breakdown status:

- B3.0 (surface): landed 821b16a. (Status unchanged from B3.1 amendment.)
- B3.1 (typing): landed febf379 + e7958e1. (Status unchanged from B3.1
  amendment.)
- B3.2 (runtime): landed bdda217 (B3.2a scaffolding: `Step` enum,
  `Continuation` with `todo!` resume, eight `Frame` variants,
  `UnhandledOpAtTopLevel` + `ContinuationAlreadyResumed` variants
  added; no behavior change), a9cefb1 (B3.2b substantive: `resume`
  body filled in, `Perform` reifies, `Handle` dispatches via
  `handle_step`, `Value::Resumption` variant added, `Frame::HandleFwd`
  variant added, both placeholder error variants deleted), 8e3de20
  (B3.2c positive coverage: four integration tests).

ADR 0007 status is now ACCEPTED in full; no qualifiers remain.


## Considered and rejected (for B3)

Shallow handlers. Type `k : B_i -> T1 ! rho_inner` instead of
`B_i -> T2 ! rho_outer`. Useful for generators, iterators, and any
pattern where the handler wants to inspect intermediate results. Rejected
for B3 because deep is the more common case in the literature we are
modelling after (Koka, Eff, OCaml 5) and shallow can be encoded on top
of deep with an explicit reflection step. Revisit if a use case appears.

Multi-shot continuations. Resume `k` more than once per perform. Requires
either continuation cloning (which demands deep value-copy semantics
throughout the runtime, including for closures over mutable state) or
persistent data structures pervasively. Rejected for B3 because the cost
is global and no concrete use case in the prototype's roadmap requires
it. One-shot is enforced at runtime so multi-shot can be added later
without a silent semantic shift.

CPS-transformed evaluator. Whole-program transform that makes `perform`
trivially capture the continuation. Rejected because it would replace
the current tree-walking evaluator wholesale and pulls in style choices
(Plotkin selective, etc.) that are not earning their cost in a research
prototype.

Stack-copying continuations. Either `setjmp`/`longjmp` (unsafe, fragile
across Rust unwind boundaries) or a hand-rolled green-thread runtime
(large engineering effort). Rejected for the prototype.

First-class handler values. `handler { ... }` as an expression that
produces a value of type `Handler<rho_in, rho_out, T1, T2>`, applicable
via a separate `with H handle e` form. Rejected for B3 to keep scope
contained; can be layered on later as sugar over the syntactic form
without breaking existing source.
CPS-state state handler in B3.2c. A real state handler in the
Eff/Koka sense (Get/Put paired effects threading mutable state
through resumption) requires either multi-argument continuations
(`k(state, value)`) or CPS-state encoding where the handle's result
type is `state -> result`. The prototype's unary lambdas/apps preclude
the former; the latter is expressible but the test program becomes
~6 source lines and the trace is long enough to obscure what is being
tested. Deferred. The B3.2c
`pipeline_arm_body_uses_outer_let_binding` test exercises the
load-bearing mechanic (an outer Let binding is visible inside the
arm body via the env bundled into `Value::Resumption`) which is what
a CPS-state handler would also depend on. A proper state-handler test
is filed against whatever phase introduces multi-arg functions or
records.

