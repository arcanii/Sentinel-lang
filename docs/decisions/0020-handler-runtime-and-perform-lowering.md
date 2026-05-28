# ADR 0020: Handler runtime + `perform` lowering (closes ADR 0019 D8)

Status: PROPOSED — to flip to ACCEPTED (or ACCEPTED-WITH-AMENDMENTS)
as the handler sub-phases land. This ADR closes the D8 deferral
from ADR 0019 — handler runtime + `perform Op(args)` semantics
in the production bootstrap compiler. The substantive design
call: which lowering strategy for `handle e with { ... }` —
free-monad reification (Phase B's approach), CPS transform, or
stack-saved continuations.

Date: 2026-05-28
Related:
  - **0019** (Phase C3 kickoff — ACCEPTED-WITH-AMENDMENTS): the
    D8 deferral that this ADR closes. ADR 0019 D8 said "the
    handler runtime is a substantive design call that justifies
    its own ADR." This is that ADR.
  - **0007** (Phase B effect handlers — ACCEPTED): the Phase B
    research-grade interpreter validated free-monad-style
    reification with deep handlers + one-shot continuations
    across ~30 tests + the supply-chain validation demo. C3.4
    absorbs the design; rebuilds the implementation against the
    production compiler shape per the Phase B crate's
    "throwaway by intent" framing.
  - **0017** (Phase C2 kickoff — ACCEPTED-WITH-AMENDMENTS): the
    "ship the typing layer minimum, defer the runtime layer"
    precedent applies here too. ADR 0019 shipped effect TYPING;
    this ADR ships effect RUNTIME. Same pattern as lexical-
    borrow-check shipping at C2.1 and Polonius deferred to ADR
    0018.

## Context

ADR 0019 D8 deferred the handler runtime to a follow-on ADR per
the lexical-first / Polonius-later precedent. C3.0 reserved the
`handle` / `with` / `perform` keywords at the lexer level; the
surface parses but neither type-checks nor runs. ADR 0019's D-
decision retrospective at C3.3 close marked D8 as the next-
largest single chunk of Phase C effect-system work.

The Phase B research-grade interpreter (`sentinel-effects-proto`)
validated the typing rules and the runtime semantics across
~30 handler tests + the supply-chain validation demo. The
production compiler inherits the *design*; the *implementation*
needs to choose a lowering strategy that fits LLVM codegen.

**Effect handlers in one sentence**: a handler intercepts
operations performed by an expression and lets the handler arm
decide what to do — supply a value, abort, or transform via the
continuation. The classical effect-handler operations are I/O
(handlers can mock or log), exceptions (handlers can recover or
re-raise), nondeterminism (handlers can try multiple
continuations), and async (handlers can suspend and resume
later). C3.4's minimum-viable surface ships exceptions + I/O
shapes; nondeterminism + async are deferred.

The Phase B handler runtime, from ADR 0007:

  - **Surface**: `handle e with { L1(x, k) => body1, L2(x, k)
    => body2, return v => ret_body }`. Arms separated by `,`;
    `return` arm is optional (defaults to `return v => v`).
  - **Semantics**: deep handlers + one-shot continuations.
    `perform L x` reifies as `Step::Op { label: L, arg: x,
    kont: Continuation }`. Each evaluation frame prepends
    itself to `kont` and re-raises on `Step::Op`. `handle e
    with H` evaluates `e`; on `Step::Op { L, arg, kont }`
    matching an arm in `H`, binds `x := arg`, `k := \v. handle
    (kont.resume(v)) with H` (the deep-handler re-wrap), and
    evaluates the arm body.
  - **Implementation**: `Continuation` is a `Vec<Frame>` with
    eight frame variants (LetBody, LetRecBody, IfBranch, AppArg,
    AppCall, BinOpRight, BinOpApply, PerformReify, HandleFwd).
    `Cell<Option<Frames>>` enforces one-shot via `take()` —
    second `resume()` panics or returns `ContinuationAlreadyResumed`.

What the production compiler must build new (not just port):

  - **Codegen for `perform` / `handle`**. Phase B's tree-walking
    interpreter has no codegen analog. The choice of lowering
    strategy (D1 below) determines how invasive the IR rewrite
    is, what runtime support is needed, and what the resulting
    binary's overhead looks like.
  - **Continuation memory representation**. Phase B used `Box<Vec
    <Frame>>` on the heap. The production compiler needs a
    concrete `repr(C)` struct that LLVM can lower + that
    `sentinel_alloc` can allocate. Frame variants become
    discriminated unions or function pointers.
  - **Operation-arg packing**. Phase B used `Value` (a sum type).
    The production compiler types each operation per ADR 0019
    D4 — operations have concrete `Vec<TypedParam>` + return
    `Type`. The packing strategy depends on the lowering choice.
  - **Effect discharge in the type-checker**. ADR 0019's
    effect-row machinery tracks which effects a fn uses; the
    `handle e with H` form REMOVES the handled effects from the
    surrounding row. This was deferred at C3.2 since neither
    handle nor perform exists yet.

The C3.3 lexer state going into ADR 0020:

  - **Keywords**: `let, fn, if, else, true, false, struct, null,
    mut, effect, secret, declassify, handle, with, perform`
  - **Punctuation**: unchanged from C2.0.1.
  - `handle` / `with` / `perform` are reserved but their AST +
    parser productions don't exist yet (ADR 0019 C3.0 only
    added effect_decl + effect_row + secret + declassify).

C3.4 picks up the remaining surface (handle/with/perform AST +
parser) AND ships the runtime / codegen.

## Decision

Twelve D-numbered sub-decisions covering lowering strategy
(D1), continuation kind (D2), handler depth (D3), surface
syntax (D4), AST + parser (D5), effect discharge typing (D6),
runtime symbols (D7), operation-arg packing (D8), sub-phase
split (D9), out-of-scope items (D10), `fn main` integration
(D11), and the phase-go program (D12).

### D1. Lowering strategy: free-monad reification.

The substantive design call. Three options:

  - **Free-monad reification** (Phase B's approach). Each `perform
    Op(arg)` site allocates a continuation frame on the heap +
    returns a tagged "operation pending" value. Each evaluation
    frame in the call stack checks for "operation pending" and
    prepends itself to the continuation if so. `handle e with H`
    drives the evaluation; on operation match, binds `x := arg`,
    `k := closure-capturing-the-continuation`, evaluates the
    arm body. Heap-allocated frames; indirect dispatch.

  - **CPS transform**. Compile each fn so it takes an additional
    "current continuation" parameter. Effects become calls to
    handler-provided functions. No runtime overhead at the call
    boundary — the continuation IS the program structure. Cost:
    a substantial AST-rewriting pass before codegen + every fn
    sig changes shape + interop with non-CPS code (e.g., the
    runtime print) needs trampolining.

  - **Stack-saved continuations**. At `perform` time, copy the
    current native stack into a heap buffer; resume by copying
    back. Smallest runtime overhead between performs; biggest
    assembly-level work (alloca tricks, careful frame
    accounting, OS-specific stack handling).

**Decision: free-monad reification, following the Phase B
precedent.** Reasoning:

  - **Phase B validated it.** ~30 handler tests + the supply-
    chain demo pass on the free-monad runtime; the typing rules
    + runtime semantics are known-good.
  - **No AST rewrite.** CPS would force every existing
    `TypedFnDef` through a transformation pass before codegen.
    The borrow-check + drop machinery (ADR 0017 D8) would have
    to be re-thought against CPS-shape signatures. Free-monad
    keeps the existing IR shape; effects layer on top.
  - **Debuggability.** Free-monad keeps continuations as
    inspectable heap-allocated frames; stack-saved continuations
    are opaque memory blobs. Phase B's debugger could pretty-
    print continuation state; the production compiler inherits
    that.
  - **No assembly tricks.** Stack-saved continuations interact
    with the OS-specific stack layout, signal-safety, and the C
    ABI. Phase B's tree-walking interpreter sidestepped this
    entirely; the production compiler shouldn't take it on now.
  - **Performance is not a C3 concern.** Free-monad has the
    biggest per-perform overhead (heap allocation + indirect
    dispatch). For the C3 minimum, this is acceptable — most
    Sentinel programs at the bootstrap stage will have low
    perform rates. Future ADRs can revisit (e.g., a CPS-on-
    optimization-pass after profiling).

Concretely: `perform Op(arg)` lowers to a runtime call
`sentinel_perform_op(label_id, arg_packed) -> kont*`; the
returned `kont*` flows back up the call chain until a `handle`
catches it. Each evaluation frame in between adds itself to
the continuation via the `sentinel_kont_push` runtime helper.

### D2. Continuation kind: one-shot only at C3.4 minimum.

Phase B's continuations were one-shot — calling `resume(v)` a
second time panics. Multi-shot continuations (you can resume the
same continuation any number of times) are strictly more
expressive: they enable nondeterminism, probabilistic
programming, time-travel debugging, and certain backtracking
patterns. But they need:

  - **Deep copy at every resume** — the continuation's heap
    state has to be cloned so the next resume doesn't see the
    previous resume's side effects. This is expensive and
    requires reasoning about which state is "captured" vs
    "shared."
  - **Sharing analysis.** When a continuation closes over a
    mutable ref / array / struct, the copy semantics get
    subtle — the user probably DOESN'T want the captured state
    cloned, but the continuation machinery doesn't know.

**Decision: one-shot only at C3.4.** Reasoning:

  - Phase B used one-shot + the validation demos passed.
  - Multi-shot adds substantial implementation complexity for
    a Phase D-or-later use case.
  - The one-shot → multi-shot upgrade is mechanical: replace
    the `Cell<Option<Frames>>::take()` pattern with a deep-
    clone + return.

Runtime enforcement: each `kont*` has a `consumed: bool` flag
checked + set on `resume`. Second resume panics with a clear
diagnostic (`sentinel_panic_kont_resumed`).

### D3. Handler depth: deep handlers.

Two semantic flavors:

  - **Deep handlers** (Phase B's choice): when the arm's body
    calls the continuation, the handler RE-WRAPS the
    continuation's tail. So subsequent effects in the tail are
    re-caught by the same handler.
  - **Shallow handlers**: the arm's body runs without re-
    wrapping. Effects in the continuation's tail bubble through.

**Decision: deep handlers.** Phase B used them; they're the
default in most languages with effect handlers (Koka, Eff, OCaml
5). Shallow handlers are useful for one-off interception (e.g.,
a `with_timeout(e) { ... }` block) but can be modeled as deep
handlers with a flag.

### D4. Surface syntax: Phase B's `handle ... with { Op(x, k) => body, return v => ret }`.

The handler grammar from ADR 0007 D1:

    handle_expr  = 'handle' expr 'with' '{' handler_arm
                   (',' handler_arm)* ','? '}'
    handler_arm  = ('return' Ident '=>' expr)
                 | (EffectName '.' OpName '(' Ident (',' Ident)* ')' '=>' expr)

Examples:

    handle do_work() with {
        Io.log(msg, k) => k(0),
        Io.read(k) => k(42),
        return v => v
    }

    handle pure_compute() with { return v => v }   // identity

Notes:
- Effect.op syntax (`Io.log`) is qualified to disambiguate
  operations across effects with the same op name (e.g.,
  `Net.read` vs `File.read`).
- The continuation binding `k` is the last param in each arm.
  The handler arm's body MUST eventually call `k(v)` to resume,
  or return without calling `k` to abort.
- The `return` arm is optional. Default: `return v => v`.

C3.0 already lexed `handle`, `with`, `perform`. C3.4 adds the
AST + parser productions.

The `perform` expression syntax:

    perform_expr = 'perform' EffectName '.' OpName '(' args? ')'

Example: `perform Io.log(msg)`.

### D5. AST + parser additions at C3.4.

Three new `ExprKind` variants:

    ExprKind::Handle {
        body: Box<Expr>,
        arms: Vec<HandlerArm>,
        return_arm: Option<ReturnArm>,
        span: Span,
    }

    ExprKind::Perform {
        effect: Spanned<String>,
        op: Spanned<String>,
        args: Vec<Expr>,
        span: Span,
    }

    HandlerArm {
        effect: Spanned<String>,
        op: Spanned<String>,
        param_names: Vec<Spanned<String>>,    // last is the kont binding
        body: Expr,
        span: Span,
    }

    ReturnArm {
        value_name: Spanned<String>,
        body: Expr,
        span: Span,
    }

Parser changes:
- `parse_atom` adds the `Handle` keyword arm (returns
  `ExprKind::Handle`).
- `parse_atom` adds the `Perform` keyword arm.
- New helpers `parse_handler_arm` + `parse_return_arm`.

Resolve mirrors `Handle` + `Perform` with EffectId / op-index
references (handlers per ADR 0019 D4 already have ops indexed
inside the effect).

### D6. Effect discharge in the type checker.

When `handle e with H` is type-checked:
- The body `e` has its inferred row computed via the existing
  ADR 0019 D2 fixed-point pass.
- The set of effects HANDLED by `H` (the union of EffectIds
  appearing in arm `Op` references, modulo duplicates) is
  REMOVED from the body's row.
- The handle-expr's outer row is the body's row MINUS the
  handled set.

So:

    fn main() -> i64 {
        handle do_io() with { Io.read(k) => k(42) }  // body
                                                      // perform Io.read
                                                      // discharged
    }

After handle-discharge, the main body's row is empty per D11.

For PARTIAL handling (a body uses effects {Io, Net}; handler
covers {Io}):
- The handle-expr's outer row is {Net}.
- That row continues to bubble to main per the normal D13 path.

New `EffectError` variant:
- `MissingHandler` — `perform Io.read()` outside any `handle`
  with an Io arm. (At C3.4 minimum, `perform` outside ANY
  handle is rejected at type-check; future ADRs may relax for
  effect-row-polymorphic callees.)

Alternatively: the perform's effect bubbles up to the enclosing
fn's row, which then needs to be either annotated or handled.
If main contains an unhandled perform, the standard D13
UnhandledEffect fires.

### D7. Runtime symbols: four new helpers in sentinel-runtime.

New runtime symbols (extern "C" fns):

  - `sentinel_perform_op(label_id: u32, arg_packed: ptr) -> kont*`
    — invoked at every `perform` site. Allocates a kont via
    `sentinel_alloc`; tags it with the label; returns the
    pointer up the stack.
  - `sentinel_kont_push(kont: kont*, frame: frame_data*)`
    — invoked at every "evaluation-frame-that-could-be-
    captured" site. Adds the frame to the continuation chain.
    Frame data is a discriminated union (tag + body) emitted by
    codegen.
  - `sentinel_kont_resume(kont: kont*, value: ptr) -> ptr` —
    invoked at every `k(v)` call inside a handler arm. Walks
    the kont's frames in reverse + evaluates each. Returns the
    final value.
  - `sentinel_kont_panic_resumed() -> !` — one-shot enforcement;
    second resume aborts with a clean diagnostic. Pairs with a
    `consumed: bool` flag on the kont struct (D2).

Frame data is a per-frame-shape struct: e.g., a LetBody frame
carries `{ binding_id: VarId, body_ptr: fn_ptr }`. The codegen
emits one frame-data type per `TypedExprKind` variant that
needs reification. Phase B had eight; the production compiler
will likely have similar.

### D8. Operation-arg packing: per-effect uniform layout.

Each operation has typed params + return type per ADR 0019 D4.
For runtime dispatch:
- Args are packed into a struct that's allocated by the perform
  site + read by the handler arm.
- For single-arg ops (`Io.log(msg)`), packing is identity —
  just pass the arg pointer.
- For multi-arg ops, codegen synthesizes a per-op struct +
  copies args in/out.

The op's return type is similarly packed: the kont's resume
takes the value of the return type and writes it to a designated
slot in the kont struct.

Per-effect uniform layout: the runtime doesn't need to know op-
specific shapes. The codegen-emitted handler arm knows the
shape statically + casts the arg pointer.

### D9. Sub-phase split.

A rough split into 4-5 sub-phases:

| Sub  | Title                                                          | Estimate     | Status |
|------|----------------------------------------------------------------|--------------|--------|
| C3.4 | AST + parser for `handle ... with { ... }` + `perform Op(args)`. | 1-2 sessions |        |
|      | Resolve mirrors. Effect discharge in the type checker per D6.  |              |        |
|      | Continuation type representation (no codegen yet).             |              |        |
| C3.5 | Codegen for `perform` — emit `sentinel_perform_op` calls;      | 2-3 sessions |        |
|      | frame reification at each evaluation site. Runtime symbols     |              |        |
|      | added to sentinel-runtime (sentinel_perform_op +               |              |        |
|      | sentinel_kont_push + sentinel_kont_panic_resumed).             |              |        |
| C3.6 | Codegen for `handle` — dispatch on label; resume call;         | 2-3 sessions |        |
|      | sentinel_kont_resume runtime symbol. The substantive runtime   |              |        |
|      | piece.                                                         |              |        |
| C3.7 | Polish + phase-go programs + STATE.md / HANDOVER refresh +     | 0-1 sessions |        |
|      | ADR 0020 PROPOSED → ACCEPTED flip.                             |              |        |

Total: 5-9 sessions across 4 sub-phases. The C3.5 + C3.6
substantive halves cost the most; C3.4 (AST + parser) is
mechanical given the C3.0 lexer + ADR 0019 D4 effect data
model; C3.7 is docs.

### D10. Out of scope at C3.4.

The following are explicitly deferred:

  - **Multi-shot continuations** (D2). Phase D+ territory; the
    one-shot → multi-shot upgrade is mechanical.
  - **Async runtime** (futures, schedulers, I/O loop) — ADR
    0019 D9 deferred this indefinitely. Async-as-effect is the
    typing shape but the runtime machinery is its own ADR.
  - **Continuation introspection** (`kont.tag`, `kont.frames`,
    debugger integration). Useful for tooling but doesn't ship
    at C3.4 minimum.
  - **Multi-handler composition** (handler-of-handler). Phase B
    handled this via deep handlers naturally; the production
    compiler inherits but doesn't add new syntax.
  - **Effect polymorphism via row variables in user code**
    (`fn foo[ρ: Effect]() -> i64 ! { ρ }`). ADR 0019 D12
    out-of-scope. Closes alongside named regions ADR.
  - **Operation-level type parameters** (`effect Io { read<T>()
    -> T }`). C4 traits territory.
  - **Optimizer-driven CPS pass for hot-path performance**. A
    post-C3.7 perf ADR.
  - **User-defined `return` arms with non-identity
    transformations**. Phase B supported `return v => g(v)`;
    C3.4 minimum accepts the syntax but the typing rule is
    "return arm's body type = handle-expr's outer type." Future
    ADRs may add monadic-bind shapes.
  - **Cancellation / abort effects**. The "handler-arm-doesn't-
    call-k" pattern handles abort naturally (Phase B-style);
    no separate machinery.

### D11. `fn main` integration: handlers discharge effects.

ADR 0019 D13's "main must be effect-free" remains in force,
BUT handlers can now discharge effects so a fn-with-effect can
be wrapped in `handle` inside main. Example:

    effect Io { log(msg: i64) -> i64; }
    fn do_work() -> i64 ! { Io } { perform Io.log(42) }
    fn main() -> i64 {
        handle do_work() with {
            Io.log(msg, k) => k(0)
        }
        // body's row {Io} discharged; main's row empty per D13.
    }

This is the canonical use case: main wraps an effect-bearing
computation in a handler that supplies the effect's
implementation (logging, I/O, mock).

### D12. Phase-go program spec.

At `tests/pass/c37_go_no_go.sentinel`. Exercises D1-D11 in one
program:

    effect Io {
        log(msg: i64) -> i64;
    }

    fn do_work(x: i64) -> i64 ! { Io } {
        let logged: i64 = perform Io.log(x);
        x + logged
    }

    fn main() -> i64 {
        let result: i64 = handle do_work(42) with {
            Io.log(msg, k) => k(msg + 1)
            // The handler increments msg by 1 before resuming;
            // do_work's `logged` binding sees 43; total = 42 + 43 = 85.
        };
        print(result)
    }

Expected: stdout `85\n`, exit 0. Exercises:
  - effect Io declaration (ADR 0019 D4)
  - `perform Io.log(...)` (this ADR D5)
  - `handle ... with { ... }` (D4)
  - Handler arm with continuation binding `k` (D5)
  - Continuation resume `k(msg + 1)` (D7)
  - Effect discharge: do_work's row {Io} → handle's outer row
    {} (D6)
  - main satisfies D11 / ADR 0019 D13.

A secondary fixture `c37_handle_return.sentinel` exercises the
optional `return` arm with a transformation:

    fn main() -> i64 {
        handle 42 with { return v => v * 2 }
        // No ops to perform; just the return arm. Result: 84.
    }

Expected: stdout `84\n`, exit 0.

A negative companion at `tests/ui/c37_perform_outside_handle.sentinel`:

    effect Io { log(msg: i64) -> i64; }
    fn main() -> i64 {
        perform Io.log(42)   // No handle wrapping; effect bubbles.
    }

Expected: snc rejects with `sentinel::effect::unhandled_effect`
on main + exit code 1. The existing ADR 0019 D13 path catches
this without ADR 0020 needing a new variant — the body's
inferred row is {Io} and there's no handle to discharge it.

## Sub-phase split

A rough split per the D9 table:

| Sub  | Title                                                          | Estimate     | Status |
|------|----------------------------------------------------------------|--------------|--------|
| C3.4 | AST + parser + resolve mirror + effect discharge in type-check | 1-2 sessions | next   |
| C3.5 | Codegen for `perform` + 3 runtime symbols                      | 2-3 sessions |        |
| C3.6 | Codegen for `handle` + sentinel_kont_resume                    | 2-3 sessions |        |
| C3.7 | Polish + phase-go programs + ADR 0020 flip + STATE close       | 0-1 sessions |        |

Honest total: 5-9 sessions across 4 sub-phases. The substantive
risk is in C3.5 + C3.6 — the runtime symbol design + frame-
data representation are codegen-heavy. C3.4 is mechanical given
the existing AST machinery from C3.0 + the ADR 0019 D4 effect
infrastructure.

## Reasoning

The decisions cluster around four themes.

**Free-monad reification over CPS / stack-saved.** D1 picks the
proven Phase B strategy. CPS would force an AST rewrite +
trampolining for runtime interop; stack-saved would need
OS-specific assembly + signal-safety reasoning. Free-monad ships
the smallest design risk while still completing the effect-
system surface.

**One-shot first; multi-shot later.** D2 mirrors Phase B's
choice. Multi-shot is strictly more expressive but adds copy-
on-resume complexity. The upgrade path is mechanical (replace
`Cell<Option<...>>::take()` with deep-clone + return).

**Continue the ADR-first pattern.** This ADR is written before
any C3.4 code — same as ADR 0017 for Phase C2, ADR 0019 for
Phase C3 typing. The substantive design call (D1's lowering
strategy) gets settled on paper.

**Production-compiler scope, not language redesign.** D10's
out-of-scope list is long. Effect polymorphism via row
variables, async runtime, multi-shot continuations, effect
polymorphism in operations — all deferred to follow-on ADRs.
C3.4-C3.7 ships the minimum that makes `handle ... with` and
`perform Op` work; the surface stays small.

## Consequences

### Positive

- The Phase B effect-system vision completes: rows + handlers
  + perform + secrets are all in the production compiler.
- The free-monad runtime is the proven path. Phase B's tests
  + supply-chain demo validated the typing rules + runtime
  semantics; the production compiler inherits them.
- `fn main` can finally wrap effect-bearing computations in
  handlers (D11). The ADR 0019 D13 constraint stops being
  restrictive — programs CAN have effects, they just have to
  be discharged before main's body completes.
- The continuation representation as heap-allocated frames
  with discriminated-union tags is debuggable (frame data is
  inspectable in a debugger; matches Phase B's `Frame` enum
  shape).
- Runtime overhead is per-perform-and-resume only. Programs
  with no effects (existing Sentinel programs c05 through c33)
  pay zero — no codegen changes hit their lowering path.

### Negative

- ~1000-1600 LOC across crates (rough; the actual is unknown
  until C3.4 ships). Larger than ADR 0019's C3 typing layer
  (~800 LOC total).
- 4 new runtime symbols (sentinel_perform_op +
  sentinel_kont_push + sentinel_kont_resume +
  sentinel_kont_panic_resumed). The C3 runtime API surface
  grows substantially.
- Heap-allocated continuation frames + indirect dispatch at
  perform sites. Performance is acceptable but not zero-cost.
  A future ADR may add a CPS-on-optimization pass.
- The "every evaluation frame could be captured" constraint
  forces codegen to emit `sentinel_kont_push` calls at LOTS of
  sites — let-bodies, if-branches, binary-op RHS, etc. Subtle
  for borrow-check + drop interactions (the captured frame
  closes over scope-local values; the moved-source set needs
  to be aware).

### Neutral

- D2's one-shot is a deliberate restriction, not a permanent
  decision. Multi-shot is a known-shape upgrade.
- D3's deep handlers match the field standard (Koka, Eff,
  OCaml 5). Shallow handlers can be expressed via flags.

## Alternatives considered

- **CPS transform** (D1). Rejected: would force an AST rewrite
  + the borrow-check + drop machinery would have to be re-
  thought against CPS-shape signatures. Phase B's tree-walking
  interpreter avoided this; the production compiler should
  too.

- **Stack-saved continuations** (D1). Rejected: OS-specific
  assembly + signal-safety + interop with the C ABI is a
  substantial design call on its own. Performance gain isn't
  worth the implementation complexity at C3.4.

- **Multi-shot continuations as default** (D2). Rejected:
  copy-on-resume + sharing-analysis is Phase D+ territory.
  One-shot covers exceptions + I/O + sync use cases.

- **Shallow handlers as default** (D3). Rejected: shallow
  handlers are a special case; modeling shallow via a deep-
  handler-with-flag is straightforward.

- **`try ... catch` syntax for handlers** (D4). Rejected: try/
  catch suggests exceptions specifically. Effect handlers
  generalize beyond exceptions (async, nondeterminism, I/O
  mocking). `handle ... with` matches Phase B + Koka + Eff.

- **Per-effect runtime symbols** (D7). Rejected: would require
  the runtime to know every user-declared effect at link time.
  A single set of symbols + label_id dispatch is simpler.

- **Ship CPS optimization at C3.4** (D1). Rejected as an
  interim choice: combining free-monad correctness + CPS
  performance would be C3.4 + a follow-on ADR; doing both in
  one phase risks overshooting the budget.

## Revisit

This ADR is **PROPOSED** until C3's sub-phases land. Per-D
revisit triggers:

- **D1 (free-monad lowering)**: revisit if profiling shows
  hot-path effect calls dominate program runtime. A CPS pass
  ADR can land alongside or independent of this ADR's flips.
- **D2 (one-shot continuations)**: revisit when nondeterminism
  / probabilistic programming / time-travel debugging surfaces
  as a Sentinel use case.
- **D3 (deep handlers)**: revisit at the first user-reported
  case where deep handler wrapping causes confusion. Adding a
  `shallow` keyword to handlers is a small AST + typing-rule
  addition.
- **D4 (surface syntax)**: revisit if `Effect.op` qualification
  surfaces as confusing in user testing. Alternative:
  unqualified `log(msg, k) => ...` with type-system-based
  disambiguation.
- **D8 (operation-arg packing)**: revisit if the per-op struct
  layout overhead matters for performance profiles. Uniform
  pointer-based dispatch is simpler but adds an allocation per
  perform.
- **D9 (sub-phase split)**: revisit at the start of C3.5. The
  codegen-runtime split is the unknown; C3.5 + C3.6 may
  collapse to one sub-phase or stretch to three.
- **D10 (out-of-scope items)**: each gets its own future ADR.
  Multi-shot continuations are the next-largest single chunk.
- **D11 (main integration)**: revisit if the "always-handle-
  effects-in-main" pattern surfaces as boilerplate. A
  `main !{Io}` allowance (with implicit Io handler) is the
  natural ergonomic upgrade.

## Appendix: estimated implementation footprint

For session-budget planning. Numbers are rough; actual is
unknown until C3.5 + C3.6 ship.

  - **C3.4** (AST + parser + resolve + type-check, ~500-700
    LOC):
    - sentinel-syntax (lexer): 0 (handle/with/perform already
      reserved at C3.0(a))
    - sentinel-ast: +100 (Handle + Perform + HandlerArm +
      ReturnArm)
    - sentinel-syntax (parser): +150-200 (parse_handle_expr +
      parse_perform_expr + parse_handler_arm + parse_return_arm)
    - sentinel-resolve: +50 (mirror; EffectId + op-index
      lookup)
    - sentinel-types: +200-300 (TypedExprKind::Handle +
      Perform; effect discharge in check_expr; MissingHandler /
      OperationArityMismatch / OperationNotInEffect errors)
    - tests + fixtures: +50

  - **C3.5** (perform codegen, ~400-600 LOC):
    - sentinel-codegen: +300-500 (lower Perform; emit
      sentinel_perform_op + sentinel_kont_push at evaluation
      frames; frame-data structs)
    - sentinel-runtime: +50-100 (3 new symbols)
    - tests: +30-50

  - **C3.6** (handle codegen, ~400-600 LOC):
    - sentinel-codegen: +200-300 (lower Handle: arm dispatch
      on label_id; resume call; deep-wrap of kont)
    - sentinel-runtime: +50-100 (sentinel_kont_resume; one-shot
      enforcement)
    - tests: +50-100

  - **C3.7** (polish + close-out, ~100-200 LOC):
    - phase-go fixtures (c37_*)
    - STATE.md + HANDOVER §0 close-out
    - ADR 0020 PROPOSED → ACCEPTED flip
    - +5 driver pass-tests for phase-go

  - **Total at C3 minimum runtime**: ~1400-2100 LOC across
    crates. In line with ADR 0019's C2-class typing-layer
    investment but concentrated in codegen rather than the
    type checker.

  - **Estimated session budget at C3.4-C3.7**: 5-9 sessions
    across 4 sub-phases. Compare ADR 0019's "4-6 sessions
    typing layer" — handler runtime is the bigger half of C3
    by codegen LOC (most C3 typing work was algorithmic; most
    C3.4+ runtime work is mechanical-but-bulky).

After C3.7: Phase C4 (traits + structured concurrency per
HANDOVER §6.2). The language surface at C3.7 close has the
full memory-safety + secret + effect-system trifecta; traits +
delegation + actors round out the "production-shape language"
arc per HANDOVER §6.2's month-12-15 budget.
