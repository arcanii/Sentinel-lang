# ADR 0070: Non-capturing first-class function values (`Fn<i64,i64>` v1)

Status: **ACCEPTED — v1 IMPLEMENTED (snc-side, 2026-07-01).** Four-check
green; both bootstrap fixed points byte-identical (all 9 selfhost
differential stages, including the resolve-stage `apply`/`FnRef` mirror
D7 required — see the correction note there).

Date: 2026-07-01

## Related

- **0024** (C4.4 structured concurrency): D2/D10 explicitly deferred closures
  — *"Closures require capture analysis + per-closure env structs + lifetime
  tracking... Closures are an ergonomic improvement to land in a follow-on."*
  This ADR is that follow-on, scoped to the **non-capturing** subset (no
  environment, no lifetime tracking) — full capturing closures remain
  deferred, same rationale, a separate future ADR.
- **0066** (threading + multi-processing): the stated motivation. The
  *reusable worker-pool LIBRARY* (M1.3's only remaining gap) is blocked
  because a worker body can't be parameterized by an operation — only a
  hand-written, differently-named worker per operation. `examples/lang/
  worker_pool.sentinel` hardcodes `x * x`.
- **0020** (handler runtime / perform lowering, D5): the existing kont
  resume-call dispatch this ADR deliberately does **not** touch (see D3).
- **0068** / **0069** (nested arrays / sealed channel): the most recent
  precedent for "ship the smallest concrete shape as a v1 minimum, interned
  generalization later" — `Channel<i64>` (M1.2), `Task<i64>` (M1.1),
  `SealedChannel<secret i64>` (M2.4a) all shipped this way before
  generalizing.

## Context

Sentinel has no notion of a function as a runtime value today. A `FnId` is a
pure compile-time dispatch tag — `crates/sentinel-ast/src/lib.rs`'s
`ExprKind` has no `Fn`/function-pointer variant, and `crates/sentinel-types/
src/lib.rs`'s `Type` enum has no `Fn` variant. The only place a function
reference crosses into "data" is the ADR 0024 spawn-wrapper symbol name
(`__spawn_wrapper_<id>`), baked at compile time — never a value a program can
hold, pass, or compare.

This blocks writing a *generic* worker/pool abstraction: every worker body
must be a distinct named `fn`, because there is no way to say "take an
operation as a parameter and apply it." Per ADR 0024 D10, full closures
(captured environment, lifetimes interacting with the lexical borrow checker)
are real, larger work and stay deferred. But the worker-pool gap doesn't
actually need captures — it needs a **non-capturing function pointer**: a
plain top-level `fn` referenced by name, passed as an ordinary Copy value,
and invoked indirectly. That is this ADR's entire scope.

## Decisions

### D1. Scope — one fixed monomorphic shape, `Fn<i64,i64>`. **PINNED.**

Mirrors the `Task<i64>`-at-C4.4 / `Channel<i64>`-at-M1.2 /
`SealedChannel<secret i64>`-at-M2.4a precedent: ship the smallest concrete
instantiation first, generalize (word-scalar params/return, arity > 1) in a
documented follow-on once the mechanism is proven. v1 supports exactly
one-parameter `i64 -> i64` functions.

**PINNED (2026-07-01): `Fn<i64,i64>` only; word-scalar generalization and
arity > 1 are named follow-ups (Revisit), not silently dropped.**

### D2. Representation — a plain unit `Type::Fn`, not an interned variant.

Unlike `Channel<T>`/`Task<T>` (interned from day one because their *element*
generalizes), `Fn<i64,i64>`'s v1 shape is fixed, so it follows the
`Type::SealedChannel`-at-M2.4a precedent instead: a plain unit variant
(`Type::Fn`, no interner table), `Copy` (it lowers to a bare LLVM function
pointer — pointer-like, owns nothing, like `Type::Process`/`Type::Ptr`).
Generalizing later means promoting it to an interned `Fn(FnSigId)`, the same
migration `SealedChannel` → `SealedChannel(SealId)` already has on its own
backlog (M2.4c) — a known, accepted incremental path in this codebase, not a
new kind of churn.

### D3. Indirect invocation is a dedicated builtin (`apply`), not `f(x)` syntax. **PINNED.**

Sentinel already special-cases `ident(args)` where `ident` resolves to a
bound **local variable**: `crates/sentinel-resolve/src/lib.rs:3999-4020`
(ADR 0020 D5) unconditionally treats that as a continuation resume-call
(`k(arg)` inside a handler arm) — "vars win over fns." Until now, a callable
local var could *only* be a kont parameter, so this was safe by absence of
alternatives. Making `Fn`-typed local vars *also* callable via the same
`ident(args)` syntax means that dispatch branch has to disambiguate kont vs.
Fn — i.e. touching the differential-critical, security-relevant
effect-handler resume-call path. That is exactly the "rush-dangerous"
category this project's own session handoffs flag for *other* deferred work
(the scg sealed/stdio/arg builtin mirror), and there is no reason to take
that risk for a feature whose whole premise is "the safe, small slice."

Instead, v1 adds a builtin **`apply(f: Fn<i64,i64>, x: i64) -> i64`**,
special-cased in `check_call` exactly like `channel_new`/`send`/`recv`
(ADR 0066 M1.2/M2.3b) and the `sealed_channel`/`sealed_process` bridge
(ADR 0069). This resolves through the **existing** `fn_table` lookup branch
(`crates/sentinel-resolve/src/lib.rs:4020+`), never the `vars`-shadowing
branch — zero overlap with resume-kont dispatch.

**PINNED (2026-07-01): `apply(f, x)` builtin for v1; unifying with ordinary
`f(x)` call syntax is a named follow-up, gated on carefully generalizing the
resume-kont dispatch — not done here.**

### D4. Eligibility — non-generic, non-builtin, non-method, fully effect-free. **PINNED.**

A bare fn name resolves to a `Type::Fn` value (`ResolvedExprKind::FnRef`)
only if the referenced fn is:
- non-generic (`type_params_count == 0`),
- a plain user fn — not a builtin/runtime symbol, not `extern "C"`, not `main`,
- exactly signature `(i64) -> i64`,
- has an **empty** effect row.

The empty-effect-row restriction (not even `Async`, which `spawn`'s own
target restriction allows) is deliberate: `apply`'s own effect row would
otherwise have to depend on its *argument's* effects to preserve the
"every effect is declared at its call site, bubbling to `main`" invariant —
a real generic-effects mechanism, not v1 scope. Requiring full purity lets
`apply` itself stay effect-free, like `len`/`unwrap_or`.

**PINNED (2026-07-01): effect row must be empty; effecting `Fn` values
(even `Async`-only) are a named follow-up.**

### D5. No FFI / no secret. `secret Fn<...>` and crossing the FFI fence are out of scope.

A `Type::Fn` value is a raw, in-process code pointer — it is not
FFI-safe data (no ABI committed for it crossing `extern "C"`) and it is not a
secretable type (a function pointer carries no secret-classified data; there
is nothing to taint). Both are rejected at type-check, the same posture as
`secret f64`/`secret Process`.

### D6. No new runtime symbol, no ABI change; one new builtin `FnId`.

`apply` is purely an LLVM-IR-level feature: `FnRef` lowers to taking the
address of an existing LLVM function (`@fn_name` as a pointer constant — no
wrapper synthesis, simpler than the ADR 0024 spawn wrapper which *does* pack
args into a struct); `apply` lowers to an ordinary indirect `call` through
that pointer with the fixed `i64 (i64)` signature. `abi-v1` is untouched.
The one new builtin shifts the user-fn `FnId` base by exactly **+1** (the
same mechanical move every M2.x builtin addition made this session) — this
*is* differential-critical (the base must stay in lockstep between `snc` and
`scg`, see D7) even though the feature's *lowering* is snc-only.

### D7. Self-host mirror — FnId base shift + the RESOLVE-stage mirror; type-check/codegen lowering is a deferred follow-up. **PINNED.**

Following the `return`-ADR-0065-stage-1/2 / M2.3b / M2.4a pattern: the
demonstrator (`examples/lang/fn_value.sentinel`) stays in `examples/`, never
`tests/pass` — so `scg` never has to TYPE-CHECK or CODEGEN a program using
`Type::Fn`/`FnRef`/`apply`. The **mechanical** FnId-base sed (`37→38` across
`selfhost/{resolve,types,effects}.sentinel`, the same `N +`/`- N`/`>= N`/
`< N` pattern used for every prior shift) is **not deferrable** — skipping it
diverges the whole differential corpus, not just this feature.

**Correction discovered during implementation:** the `tests/ui/` rejection
fixtures (D4's `c70_fn_value_ineligible.sentinel`) are swept into the
**RESOLVE-stage** differential too — `selfhost_resolve.rs`'s corpus check
scans every `tests/pass` + `tests/ui` fixture and requires a byte-identical
`snc resolve` dump for any that resolve cleanly, REGARDLESS of whether they
later fail at type-check. A ui fixture demonstrating D4 inherently resolves
cleanly (eligibility is a type-check concern), so `Type::Fn`'s v1 scope
in fact required TWO small resolve-stage additions beyond the FnId-base sed:
(1) `selfhost/resolve.sentinel`'s `builtin_id` gained an `"apply" → 37` arm
(the fixture calls `apply` by name); (2) `selfhost/resolve/dump.sentinel`'s
`Expr::Var` arm gained the same `sc_lookup` → `fn_lookup` fallback as the
Rust resolver, rendering `(fnref #N)`. Both are small, mechanical, resolve-
stage-only additions — NOT the type-check/codegen lowering, which remains
genuinely deferred (scg's type-checker still has no `Type::Fn`, so it would
reject `c70_fn_value_ineligible.sentinel` too, just via a different
diagnostic — the differential only requires RESOLVE-stage byte-parity, not
identical diagnostics). The type-check/codegen lowering mirror (an interner
kind for `Type::Fn`, `is_move_type` arm, cg arms in
`selfhost/types/cg*.sentinel`) is still deferred, added to the same tracked
"scg mirror of the M2.x builtins" follow-up `docs/HANDOVER.md` already lists.

**Lesson for future ADRs choosing this deferral pattern:** "keep the
demonstrator in `examples/`" only shields the TYPE-CHECK/CODEGEN stages from
the differential. Any `tests/ui/` (or `tests/pass/`) fixture that RESOLVES
cleanly — which most type-error and even some borrow/effect-error fixtures
do — is swept into the resolve-stage (and possibly borrow/effect-stage)
corpus regardless of where it's ultimately rejected. Check each corpus
harness's discovery scope (`collect_fixtures`-style functions in
`crates/sentinel-driver/tests/selfhost_*.rs`) before assuming a ui fixture
is differential-free.

**PINNED (2026-07-01): FnId-base sed + the two resolve-stage additions
above land in this commit; type-check/codegen lowering mirror deferred to
the existing tracked scg-mirror follow-up.**

## Consequences

### Positive
- Unblocks the stated worker-pool motivation: a worker fn can take an
  `op: Fn<i64,i64>` parameter and `apply(op, x)` it, instead of one
  hand-written worker per operation.
- Zero new runtime symbol, zero ABI change, zero borrow-checker capture
  machinery — the smallest version of "functions as values" that is still
  useful, matching the user's explicit scope choice.
- Does not touch the differential-critical resume-kont dispatch at all (D3).

### Negative
- `apply(f, x)` is less ergonomic than `f(x)` — an accepted v1 trade (D3),
  not a permanent design point.
- One more monomorphic-then-generalize migration debt, same shape as
  `SealedChannel`'s already-accepted M2.4a → M2.4c path.
- Effect-free-only eligibility (D4) means a spawn-eligible (`Async`-capable)
  fn cannot yet be passed as a Fn value — narrows the immediate worker-pool
  use case to non-effecting operations.

### Neutral
- One more `Type` unit variant (no `Copy + Hash` regression — it's a
  zero-payload variant, cheaper than even an interned id).

## Revisit
- Generalize `Fn<i64,i64>` to word-scalar params/return (the
  `Channel<T>`-M1.2b-cont-style follow-up) once v1 is proven in real use.
- Unify `apply(f, x)` with ordinary `f(x)` call syntax — requires carefully
  generalizing the `ident(args)`-over-a-local-var resolve dispatch
  (`crates/sentinel-resolve/src/lib.rs:3999-4020`) to disambiguate kont vs.
  Fn by the var's type, not unconditionally assume kont. Differential-
  critical; do carefully, in a focused session.
- Allow `Async`-effecting fns as `Fn` values once there's an effect-row
  story for indirect calls (D4).
- Consider `spawn op(x)` for an indirect (Fn-valued) target — needs a
  generic spawn wrapper (today's wrapper bakes the target `FnId` into the
  `__spawn_wrapper_<id>` symbol name at compile time).
- The scg lowering mirror (D7) — bundle with the existing tracked sealed/
  stdio/arg/generic-channel scg-mirror follow-up in `docs/HANDOVER.md`.
- Full capturing closures (ADR 0024 D10) remain a distinct, larger future
  ADR — this ADR does not change that assessment.

## Estimated footprint
| Piece | Deliverable | Rough LOC |
|-------|-------------|-----------|
| Front end | `Type::Fn`, `FnRef` resolve/type-check, `apply` builtin + eligibility checks | ~150 |
| Codegen | inkwell `FnRef` address-of + `apply` indirect call | ~60 |
| Mechanical | exhaustiveness arms across borrow-check/effect-check/mir/driver dumps | ~30 sites |
| selfhost | FnId-base sed + `builtin_id`/`Expr::Var` resolve-stage mirror (type-check/codegen lowering deferred) | mechanical, ~15 lines |
| Tests | 1 example, 2-3 ui fixtures | ~100 |

---

**▶ SIGN-OFF (2026-07-01):** v1 scope approved (non-capturing `Fn<i64,i64>`
+ `apply` builtin, D1-D7 as PINNED above) via the plan-mode review for this
session. Implementation proceeded snc-side, ADR-rhythm.

**▶ DONE (2026-07-01):** `Type::Fn` + `FnRef` + the `apply` builtin
implemented (resolve/types/borrow-check/effect-check/mir/codegen), FnId base
37→38 (mirrored into `selfhost/`), demonstrator `examples/lang/fn_value.sentinel`
(runtime-verified, exit 42, both `--separate` and merged), two `tests/ui/`
rejections (`c70_fn_value_ineligible`, `c70_fn_type_args_unsupported`,
snapshotted). Four-check green; all 9 selfhost differential stages
byte-identical (incl. the resolve-stage `apply`/`FnRef` mirror D7 required).
Deferred follow-ups tracked in Revisit: generalized `Fn<T,R>`, `f(x)` call
syntax, effecting Fn values, `spawn` of an indirect target, the scg
type-check/codegen lowering mirror.
