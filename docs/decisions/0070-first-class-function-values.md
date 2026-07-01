# ADR 0070: Non-capturing first-class function values (`Fn<T,R>`)

Status: **ACCEPTED — v1 IMPLEMENTED, GENERALIZED, THEN UNIFIED WITH `f(x)`,
all same-day (snc-side, 2026-07-01).** Four-check green; both bootstrap
fixed points byte-identical (all 9 selfhost differential stages) after the
v1 landing, the M-cont generalization, and the D3-revisit unification
below. v1 shipped `Fn<i64,i64>` only; the M-cont amendment generalized to
any word-scalar `Fn<T,R>` in the same session, mirroring the
`Task<i64>`→`Task<T>` / `Channel<i64>`→`Channel<T>` precedent; the
D3-revisit amendment (end of this document) then closed D3's own named
follow-up — `f(x)` now works directly on a `Fn`-typed variable,
type-checking to the identical shape as `apply(f, x)`, without touching
ADR 0020 D5's resolve-stage dispatch at all.

Date: 2026-07-01 (v1); 2026-07-01 (M-cont generalization, same day);
2026-07-01 (D3-revisit unification, same day)

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
bound **local variable**: `crates/sentinel-resolve/src/lib.rs:4034-4100`
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

**Superseded/extended (2026-07-01, D3-revisit):** this follow-up is now
done — see the Amendment 2 section at the end of this document. The
resume-kont dispatch above is generalized, not touched at the resolve
stage: `f(x)` and `apply(f, x)` are unified at the *types* stage instead,
leaving this D3 paragraph's resolve-stage description accurate as
historical record of the v1 decision.

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
- ~~Generalize `Fn<i64,i64>` to word-scalar params/return (the
  `Channel<T>`-M1.2b-cont-style follow-up) once v1 is proven in real use.~~
  **DONE same-day — see the M-cont amendment at the end of this document.**
- ~~Unify `apply(f, x)` with ordinary `f(x)` call syntax — requires carefully
  generalizing the `ident(args)`-over-a-local-var resolve dispatch to
  disambiguate kont vs. Fn by the var's type, not unconditionally assume
  kont. Differential-critical; do carefully, in a focused session.~~ **DONE
  same-day — see the D3-revisit amendment at the end of this document. The
  resolve-stage dispatch itself is untouched; disambiguation happens at the
  types stage instead.**
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

---

## Amendment (M-cont, same day): generalize `Fn<i64,i64>` → `Fn<T,R>` over any word-scalar

**PINNED (2026-07-01):** the Revisit item above, done immediately rather than
deferred — the natural continuation of the same session, following the
`Task<i64>`→`Task<T>` (M1.1) / `Channel<i64>`→`Channel<T>` (M1.2→M1.2b-cont)
precedent of shipping the smallest concrete instantiation first and
generalizing next.

### D10. Representation — arithmetic signature id, not an interner table.

`Channel<T>`'s generalization kept `Type::Channel(ChanId)` but PRE-INTERNED
all 6 word-scalar element types at fixed `ChanId`s during builtin-sig setup
(`channel_chanid_for`/`channel_elem_for`), specifically to avoid threading
the `channels` table through `check_call`. `Fn<T,R>` has **two** axes
(param, return) — pre-interning all 36 word-scalar × word-scalar
combinations would work but is more machinery than needed. Instead:
`Type::Fn(FnValueSigId)` where the id is **computed arithmetically**
(`param_index * 6 + ret_index` over the same 6-element word-scalar
enumeration `channel_chanid_for` uses, independently duplicated rather than
reused so the two features don't couple on an incidental shared numbering)
via `fn_value_sig_id_for(param_ty, ret_ty) -> Option<FnValueSigId>` /
`fn_value_sig_param_ret(id) -> (Type, Type)` (`sentinel-types`). **No
interner table, no `TypedProgram` field, no threading through
`resolve_type_expr` or the check pipeline at all** — a pure function pair,
lower-machinery than even `Channel<T>`'s pre-intern trick.

**PINNED: arithmetic id over 6×6 word-scalars; no interner table.**

### D11. `apply` becomes `check_call`-special-cased (context-typed from `f`).

v1's `apply` had ONE fixed concrete signature, so it needed no special-casing
(the generic call-checking path handled it via a registered
`TypedFnSignature`). Generalized, `apply(f: Fn<T,R>, x: T) -> R`'s shape
depends on the CALL SITE's `f` argument, so it now follows the
`process_recv`/`recv` pattern: special-cased in `check_call`, type-checks
`args[0]` first, extracts `(param_ty, ret_ty)` from its `Type::Fn(id)` via
`fn_value_sig_param_ret`, then checks `args[1]` against `param_ty` and
returns `ret_ty`. No `type_args` needed (unlike `process_recv`, which needs
`type_args` for codegen's decode step) — `apply`'s codegen (`lower_apply`)
derives both LLVM types directly from the typed AST (`x.ty` for the param,
the `Call` node's own `expr.ty` for the return), so nothing needs to ride
through `type_args`. A first argument that isn't `Type::Fn` at all gets a
dedicated `ApplyTargetNotFn` diagnostic (not `CallArgMismatch`, which would
misleadingly imply a single "expected" type when any word-scalar `Fn<T,R>`
is acceptable).

### D12. Self-host mirror — unaffected; no new corpus risk.

The v1 GOTCHA (D7: `tests/ui` fixtures sweep into the resolve-stage
differential when they resolve cleanly) does **not** recur here — verified,
not assumed. Both `tests/ui/` fixtures were re-examined:
`c70_fn_value_ineligible.sentinel` is unchanged (still calls `apply` by
name, still covered by the existing `builtin_id`/`Expr::Var` resolve-stage
mirror from v1); the replacement `c70_fn_type_args_unsupported.sentinel`
(`Fn<u128, i64>`, now genuinely unsupported since `u128` isn't a
word-scalar) only exercises the "Fn" TYPE-EXPRESSION arm, which carries no
semantic type info at the resolve stage (type names are uninterpreted
strings until the TYPES stage) — so it needs no resolve-stage mirror. All 9
selfhost differential stages confirmed byte-identical after the
generalization with **zero further `selfhost/` changes**.

### Consequences (amendment)

- **Positive:** the worker-pool motivation now covers non-`i64` operations
  (byte transforms, float transforms, bool predicates) — the realistic 80%
  case, not just an `i64`-shaped demo.
- **Positive:** lower implementation cost than `Channel<T>`'s own
  generalization (no table, no threading) — the two-axis problem turned out
  simpler than the one-axis precedent once framed arithmetically.
- **Neutral:** `Fn<T,R>` stays restricted to word-scalar T/R (no aggregates,
  no `secret`, no generics) — same boundary as v1, just wider within it.

Demonstrator `examples/lang/fn_value_generic.sentinel` (`Fn<u8,u8>` /
`Fn<f64,f64>` / `Fn<bool,bool>`, runtime-verified, exit 42, both
`--separate` and merged); `tests/ui/c70_fn_type_args_unsupported.sentinel`
updated to `Fn<u128, i64>` (still genuinely rejected). Four-check green; all
9 selfhost differential stages byte-identical, zero `selfhost/` changes
beyond what v1 already required.

---

## Amendment 2 (D3-revisit, same day): unify `apply(f, x)` with ordinary `f(x)` call syntax

**PINNED (2026-07-01):** closes D3's own named follow-up (chosen off the
post-M-cont fresh-session decision menu). `f(x)` now works directly on a
`Fn`-typed local variable — `apply(f, x)` remains valid too; the two
spellings are unified, not one replacing the other.

### D13. Disambiguation lives at the TYPES stage; ADR 0020 D5's resolve dispatch is untouched.

D3 deferred this specifically because generalizing the *resolve-stage*
dispatch (`vars.get(callee)` unconditionally means "resume a continuation")
to disambiguate kont-vs-Fn looked like it required touching that
differential-critical, security-relevant branch directly. It doesn't:
resolve has no type information at all, and never needed any — it already
only encodes the scoping fact "the callee names a bound local variable,"
deferring validation to the types stage (this was already true pre-ADR-0070,
for the Kont-only case). `check_resume_kont_expr`
(`crates/sentinel-types/src/lib.rs`) is restructured from a two-way
`match kont_ty { Type::Kont(id) => ..., other => Err(Mismatch) }` into a
three-way match: `Type::Kont(kont_id)` keeps the *exact* pre-existing body
(byte-for-byte unchanged); `Type::Fn(sig_id)` is new; any other type is
`TypeError::CalleeNotCallable` (below). Resolve's own dispatch
(`crates/sentinel-resolve/src/lib.rs:4034-4100`) is not modified at all —
not even a rename — so ADR 0020 D5's "vars win over fns" security posture
carries forward unexamined and unweakened.

### D14. `f(x)` desugars to the identical `TypedExprKind::Call{id: APPLY_FN_ID, ...}` shape `apply(f, x)` already produces.

The new `Type::Fn` arm decodes `(param_ty, ret_ty)` via the existing
`fn_value_sig_param_ret(sig_id)`, type-checks the call's one argument
against `param_ty`, hand-builds a `TypedExpr` for the callee
(`TypedExprKind::Var(kont)` with `ty: kont_ty` — reusing what `env.get`
already returned rather than re-dispatching through `check_expr`'s general
`Var` arm, which carries its own `Type::Kont`-smuggling guard,
`TypeError::KontUsedAsValue`, that resume/apply dispatch has always
deliberately bypassed), and returns `TypedExprKind::Call{id: APPLY_FN_ID,
args: [f, x], type_args: []}` — the *exact* node `check_call`'s `apply`
branch produces for `apply(f, x)`. Because the shape is byte-identical,
every downstream consumer (borrow-check, effect-check, MIR, both codegen
backends) already handles it correctly with **zero new code** — proven at
runtime by the existing `fn_value*.sentinel` demonstrators, which already
exercised `apply(f, x)` producing this shape. No new runtime symbol, no ABI
change, and — unlike almost every other change in this session's
history — **no `FnId` renumbering**: this reuses the existing
`APPLY_FN_ID` rather than registering a new builtin, so there is no
user-fn base shift and no lockstep sed across `selfhost/`.

### D15. Three new diagnostics, and a latent `apply` bug fixed along the way.

`CalleeNotCallable { got, span }` replaces the old
`Mismatch{expected: Type::Kont(KontId(u32::MAX)), ...}` sentinel-value hack
(an implementation detail, not a documented contract — no fixture or
snapshot referenced it) for "a bound local var was called but is neither
Kont nor Fn." `FnValueArityMismatch` mirrors `KontArityMismatch` for "a
`Fn<T,R>` value called with the wrong number of arguments" (always exactly
one). `FnValueArgMismatch` is the direct-call twin of `apply`'s own
`CallArgMismatch`, kept as a separate diagnostic rather than reused because
`CallArgMismatch`'s message names the callee (e.g. "argument 1 of `apply`
expects...") and there is no `apply` token in the source to name for
`op(true)` — the same reasoning that motivated `ApplyTargetNotFn`'s
original existence (D3 v1).

**A genuine pre-existing bug, discovered while wiring up
`FnValueArgMismatch` and fixed in the same diff:** the new arm's
`check_expr(&args[0], Some(param_ty), ...)` never reached its own manual
`if x_typed.ty != param_ty` check — `check_expr` routes an `Some(expected)`
hint through `coerce_to_expected`, which throws a generic
`TypeError::Mismatch` on disagreement *before* returning, exactly
pre-empting the intended diagnostic. Tracing this down surfaced that
`check_call`'s existing `apply` branch has the **identical** latent bug:
`apply(f, x)` with a wrongly-typed `x` has, since the M-cont amendment,
always raised a generic `Mismatch` rather than the intended
`CallArgMismatch{callee: "apply", ...}` — dead code, never caught because
no fixture exercised an `apply` argument-type mismatch. Both sites are
fixed the same way, mirroring a precedent already in this file's own
codebase (`check_handle_expr`'s arm-body check passes `None` instead of
`Some(outer_ty)` for exactly this reason, documented in its own inline
comment): pass `None` instead of `Some(param_ty)`, then let the manual
check raise the specific diagnostic. This loses no legitimate coercion —
`Fn<T,R>`'s parameter is always a plain concrete word-scalar, never a
`coerce_to_expected` widening target (`?T` / `secret T` / `[secret u8]`).
Confirmed safe via a full `tests/ui` sweep: no existing fixture exercises
an `apply` argument-type mismatch, so nothing depended on the old
(unreachable-diagnostic) behavior.

### D16. Self-host impact: none, verified — not assumed.

Resolve's dump format is unchanged (D13), so `selfhost/resolve/dump.sentinel`
needs no mirror. The three new `tests/ui/c70_*.sentinel` fixtures are
permanent TYPES-stage rejects: they resolve cleanly (so they *do* sweep
into `selfhost_resolve.rs`'s corpus test — expected and safe, resolve is
untouched) but fail at `snc types`, and `selfhost_types.rs`'s corpus test
explicitly skips any fixture whose oracle (`snc types`) doesn't exit
successfully. This matters because `selfhost/types/borrow_arms.sentinel`'s
`dump_te_call` (scg's own types-stage mirror) has **no gate on the resumed
var's type at all** today — it doesn't even implement the pre-existing
Kont-only restriction. That gap is real but is **provably unreachable** by
this change's own fixtures, confirmed by reading the corpus-collection code
directly. Separately, `crates/sentinel-driver/src/llvm_dump.rs` (the
second, independent ~4200-line codegen-differential-oracle backend, not a
thin printer) already unconditionally rejects any `apply`/`Type::Fn` call
via its `FIRST_USER_FN` fallthrough — confirmed zero references to
`APPLY_FN_ID`/`Type::Fn` in that file — so it needs no change, for a
structurally different reason than the other files (pre-existing total
exclusion of the whole feature, not pre-existing correctness). All 9
selfhost differential stages confirmed byte-identical, both bootstrap fixed
points hold.

### Consequences (amendment 2)

- **Positive:** `f(x)` is now as ergonomic as an ordinary function call for
  a `Fn<T,R>` value — `apply(f, x)` remains available, so nothing that used
  it needs to change.
- **Positive:** a real, previously-undiscovered diagnostic bug in the
  already-shipped `apply` argument-type check is fixed as a direct side
  effect of unifying the two spellings.
- **Neutral:** the two spellings' happy paths are byte-identical after
  type-check; their diagnostics on a bad argument *type* specifically use
  different (both now-correct) messages (`CallArgMismatch` naming `apply`
  vs. `FnValueArgMismatch` naming no callee), since there is no `apply`
  token in the source for the direct-call spelling to name.

No shared helper was extracted between `check_call`'s `apply` branch and
the new `Type::Fn` arm — `check_call` already has eight near-duplicate
builtin special-cases and none share a helper (each decodes its
type/signature through a genuinely different mechanism), and the two sites
here obtain their `f` value differently (one via `check_expr` on an
arbitrary expression, the other by reading `env.get`'s result directly).
Drift between the two spellings is instead guarded by a new unit test,
`adr0070_direct_fn_value_call_matches_apply_call`
(`crates/sentinel-types/src/lib.rs`), asserting `apply(op, 5)` and `op(5)`
type-check to the same `Call{id: APPLY_FN_ID, ...}` shape and return type —
an actually-exercised guarantee, not just a structural one. Demonstrators:
`examples/lang/fn_value.sentinel` gained a `call_direct` helper alongside
the existing `apply_to`, and `examples/lang/fn_value_generic.sentinel`'s
`apply_bool` switched to direct-call syntax (proving the unification holds
for a non-`i64` `Fn<T,R>` instantiation too) — both still exit 42, both
`--separate` and merged. Three new `tests/ui/c70_*.sentinel` fixtures
(`callee_not_callable`, `fn_value_arity_mismatch`, `fn_value_arg_mismatch`)
pin the three new diagnostics. Four-check green; all 9 selfhost
differential stages byte-identical, both bootstrap fixed points hold.

## Revisit (amendment 2)
- Allow `Async`-effecting fns as `Fn` values once there's an effect-row
  story for indirect calls (D4) — unaffected by this amendment.
- Consider `spawn op(x)` for an indirect (Fn-valued) target — unaffected by
  this amendment; still needs a generic spawn wrapper.
- The scg lowering mirror (D7/D12) — still bundled with the existing
  tracked sealed/stdio/arg/generic-channel scg-mirror follow-up. This
  amendment adds no new scg-mirror surface (D16): the new diagnostics are
  types-stage-only and their fixtures are types-corpus-excluded by
  construction.
- Full capturing closures (ADR 0024 D10) remain a distinct, larger future
  ADR — unaffected by this amendment.
