# ADR 0071: `Shared<T>` shared ownership + `Mutex<T>` — the bounded shared-state escape hatch (ADR 0066 M1.4)

Status: **PROPOSED — design PINNED (maintainer sign-off 2026-07-02); implementation not
started (M1.4-0 next).** This is the M1.4 sub-phase of the ADR 0066 threading roadmap,
broken out into its own ADR per **ADR 0066 D5** ("blocked on first designing a
runtime-refcounted `Shared<T>` handle … a language feature in its own right, arguably
bigger than the mutex it unblocks, and warranting its own ADR") and the roadmap table
(line 866, "M1.4 … yes — likely its own ADR first"). All D-points D1–D9 below are
PINNED; the first step is the M1.4-0 motivating example (D1), which is *also the D5 gate*
— if it shows channels suffice, M1.4a is reconsidered before any machinery is built.

**Maintainer decisions taken (2026-07-02), pinned below:** the **full `Shared<T>` +
`Mutex<T>`** path (not the leaked-atomic-cell MVP), and **secret shared state
(`Shared<secret T>`/`Mutex<secret T>`) IS in scope for v1** (phased last, as M1.4c). No
code yet — this ADR is the design of record; implementation follows the oracle-moving
rhythm (PROPOSED → Rust `snc` + fixtures → re-bless the differential → mirror into
`selfhost/` → both bootstrap fixed points green → ACCEPTED).

**One precondition is consciously being waived.** ADR 0066 D5's own recommendation was
"do not build Mutex until a real program in `examples/` proves channels are
insufficient," and no such program exists yet. The maintainer has elected to proceed;
accordingly the **first implementation step (before any type/runtime machinery) is to
write that motivating `examples/` program** (D1) — so the "build real programs → find the
gap → fix it" discipline (ADRs 0047–0054) is honored as the opening move rather than
skipped, and the exact shared-state shape it needs grounds the `Shared<T>` API.

Date: 2026-07-02

## Related

- **ADR 0066** (threading & multiprocessing roadmap) — **D5/D5a** pin M1.4: `Mutex<T>`
  is gated on a runtime-refcounted `Shared<T>` + deterministic drop; `lock()` is
  fallible; two deadlock-detection tiers (`LockTimeout` always-on, `Deadlock` opt-in
  wait-for-graph over public lock identity). **D2** (channels spine), **D8** (the
  secret fence is a *boundary* property — no in-process fence, since the receiver is
  still `secret_leak`-checked). This ADR implements D5/D5a.
- **ADR 0024** (structured concurrency) — the `Type::Task` interner-variant + runtime-
  handle + per-spawn-wrapper templates every new handle type follows; **D10** deferred
  channels/mutexes/atomics, now being lifted in stages.
- **ADR 0069** (`SealedChannel<secret T>`) — the `OpenResult { ok: i64, v: secret i64 }`
  precedent for returning a secret value when `?(secret T)` is unrepresentable
  (`NullableInner` has no `Secret` variant); this ADR reuses that shape for `lock()` on
  secret `T`.
- **ADR 0017 D8** — user-defined `fn drop(&mut self)` is out of scope pre-traits; this
  ADR deliberately does **not** introduce a general `Drop` trait, using a hard-coded
  drop-content arm for the two new runtime handle types instead.
- **SENTINEL_DESIGN2 §6/§1** — the original shared-memory + robust-futex + `LockPoisoned`
  cross-process vision (superseded as the *primary* model by ADR 0066's channels-first
  pivot, retained as the eventual far-future cross-process escape hatch); §1's "no data
  races, no deadlocked locks, typed error not abort" goals this ADR upholds.

## Context

Sentinel's concurrency spine is channels + ownership-transfer (ADR 0066 D2): a `send`
is a move, and the lexical borrow checker needs *nothing new* for it. That covers most
real use cases via the actor pattern (a "shared counter" becomes an owning task that
receives increment messages). But some state is genuinely shared and awkward as
messages, and the actor round-trip — a channel hop serialized through one task per
access — is orders of magnitude slower than an in-place `fetch_add` or a short critical
section (ADR 0066 D5's honest counter-argument).

**Why this is the borrow-checker-hardest piece.** Every concurrency handle today —
`Task`, `Channel`, `Process`, `SealedChannel`, `Fn` — is `is_copy_type == true` and
**deliberately leaked**. The runtime says so verbatim: `sentinel_channel_close` "only
drops the inner Sender … there is nothing safe to free … a Copy handle has no single
owner (no Arc/refcount yet)" (`crates/sentinel-runtime/src/lib.rs:1287-1291`). That
copy-and-leak pattern is *exactly* what gives frictionless N-way co-ownership across
tasks with zero borrow-checker work: the checker sees an infinitely-duplicable scalar
and enforces nothing about how many tasks hold it (`crates/sentinel-borrow-check/
src/lib.rs:1428-1490`). Passing a `Channel` into `spawn` records no move; both sides
keep a copy; neither drops it.

`Shared<T>` cannot leak — D5 requires it "freed when the last clone drops." So it must
be the **first handle that is `Copy`-for-the-checker (frictionless duplication, no
move-tracking, none of the checker's lexical over-rejections) *and* drop-emitting (a
refcount decrement on every scope-exit, freeing the cell at zero).** That combination
does not exist in the type lattice today: every `Copy` type has `needs_drop == false`;
every `needs_drop` type is move-tracked (`crates/sentinel-codegen/src/lib.rs:1969,
2016-2090`). **Reconciling those is the whole of the M1.4 design cost.** The `Mutex`
itself, once co-ownership exists, is a modest increment.

**What is already solved (the research that grounds this ADR confirmed against live
code):**
- **Drop *timing* is fully working and reusable.** Scope-exit drop fires correctly at
  every boundary — normal fall-through (`emit_frame_drops`, codegen:4436), loop
  `break`/`continue` (`emit_loop_exit_drops`, :4502), early `return`
  (`emit_return_drops`, :4524, ADR 0065) — in reverse declaration order, move-aware,
  single-free across mutually-exclusive blocks. A guard bound to a `let` already drops
  at the right point with **no new machinery**.
- **Drop *content* is a small, localized gap.** Today `emit_drop_for_binding`
  (codegen:4590) can only emit structural `sentinel_free` on embedded pointers. Adding
  a match arm that emits `call @sentinel_shared_release(h)` / `@sentinel_mutex_unlock(h)`
  instead — exactly how the `Array` arm emits a free — needs **no general `Drop` trait**
  (respects ADR 0017 D8). Localized to the drop dispatch in Rust + the selfhost
  `cg_emit_drop` mirror (`selfhost/types/cg.sentinel:1046`).
- **The secret analysis is clean.** No new `secret_leak` sink is needed: a
  secret-dependent *decision to lock* is already a rejected secret branch
  (`crates/sentinel-types/src/lib.rs:8493`). In-process sharing dissolves no
  constant-time invariant (D8: secrecy is a type property, not byte-hiding; the receiver
  is still `secret_leak`-checked).

## Decisions

### D1. Scope & sequencing — full `Shared<T>` then `Mutex<T> = Shared<SentinelMutex<T>>`, secret last. **PINNED (path choice).**

Build the refcounted `Shared<T>` handle first (the real prerequisite, "bigger than the
mutex"), then layer `Mutex<T>` on top so the co-ownership/refcount problem is solved
**once**. Three implementation sub-phases, each landing four-check-green with both
bootstrap fixed points byte-identical before the next:

- **M1.4-0 (motivating example, no compiler change).** Write 1–2 `examples/lang/`
  programs that attempt genuinely-shared state (a concurrent counter / read-mostly
  cache / parallel accumulator) with the existing spawn+`Channel` actor pattern, plus a
  short written analysis of where it is inadequate (channel round-trip per access,
  serialization through one task vs an in-place update). This honors D5's gate as the
  opening move and pins the concrete API `Shared<T>` must serve. If it shows channels
  suffice, stop here and revisit — cheap insurance on the roadmap's heaviest item.
- **M1.4a — `Shared<T>` (public `T`).** The first `Copy`-yet-drop-emitting handle (D2):
  the runtime refcount cell, the clone/drop codegen accounting, the new drop-content
  arm. Delivers shared ownership on its own (a `Shared<i64>` a producer and consumer
  both hold), independent of any lock.
- **M1.4b — `Mutex<T>` (public `T`).** `Mutex<T> = Shared<SentinelMutex<T>>` (D4): the
  lock/unlock builtins, the fallible `lock()` (D4), the scope-bound guard whose drop
  unlocks (D3), and the D5a deadlock tiers (D5).
- **M1.4c — secret `T`.** `Shared<secret T>`/`Mutex<secret T>` (D6): the container-
  interner secret-representability fix (which also unblocks `Channel<secret T>`), the
  broker-backed secret allocation (mlock/zero), and the `OpenResult`-shaped secret
  `lock()`. Phased last so the hard Copy+drop machinery and the lock machinery are
  proven on public `T` first; secret is in scope for v1, not deferred to a later ADR.

### D2. `Shared<T>` is the first `Copy`-for-the-checker + drop-emitting handle. **PINNED (mechanism).**

`is_copy_type(Shared<T>) == true` — so the borrow checker treats it as freely
duplicable exactly like `Channel`, with **no move-tracking and none of the documented
lexical over-rejections** (`docs/borrow-check-limitations.md`). This is the chosen
resolution of the central tension; the rejected alternative (model `Shared<T>` as a
Move type with an explicit `.clone()` refcount bump) is cheaper to implement — it reuses
the existing move/drop machinery verbatim — but sacrifices the frictionless handle
ergonomics every other concurrency type has and imports the checker's false-rejections,
which is the wrong trade for the language's flagship escape hatch.

The new machinery is the **clone/drop accounting**, emitted structurally by codegen (no
new pass), because a `Copy` type has no move-set enumerating its duplication sites:

- **`refcount++` at each duplication of a Shared value *from a named binding into a new
  owner*** — a `let y = x` / by-value argument / spawn capture where the source `x` is a
  named place that will itself be dropped. (An *rvalue* source — e.g. `use_fn(shared_new(v))`
  passed directly — transfers its single unit to the callee and gets **no** `++`, exactly
  as a Move rvalue transfers today. Codegen already distinguishes "load from a slot" from
  "a fresh value," which is precisely this named-place-vs-rvalue distinction.)
- **`refcount--` at each named binding's scope-exit drop** (via the D3 drop arm).

The load-bearing invariant is **#increments == #decrements**, so `shared_new` (which
returns `rc = 1`) is balanced by exactly one net `--` reaching zero. This is verified
two ways: the runtime refcount must never underflow (a debug assertion), and a
`tests/pass` fixture must round-trip a `Shared` through multiple bindings/tasks and
assert the cell is freed exactly once (leak-checked, mirroring
`c24_moved_array_no_double_free`). A `Shared<T>` as a **struct/class field** flips that
type's `needs_drop` true and recurses a `--` in the aggregate drop walk (unlike a
`Channel` field, which is never dropped) — the ADR 0046 field-precise drop walk is the
mechanism it plugs into.

### D3. Deterministic drop reuses the existing scope-exit machinery; a hard-coded drop-content arm, not a `Drop` trait. **PINNED.**

The "deterministic drop" prerequisite (D5's second) is *half already met*: the timing
half is fully solved (Context). The missing half is the drop *content*. Add a dedicated
arm to `emit_drop_for_binding` (Rust) + `cg_emit_drop` (selfhost) that, for a `Shared`
or `Guard` handle, emits the runtime release/unlock call instead of / in addition to a
`sentinel_free` — the same special-case pattern the codebase already uses to make
`Array`/`Vec` emit a free and `Task`/`Channel` emit nothing. `cg_needs_drop` /
`field_type_needs_drop_inner` must report `true` for `Shared`/`Mutex`/`Guard` so they
are recorded for scope-exit drop. **No general user-`Drop` trait** (ADR 0017 D8 stays
deferred). Both compilers change in lockstep or the self-host fixed point breaks.

**Guard non-escape.** A `Mutex` guard's unlock-on-drop is only sound if the guard cannot
outlive the critical section — it must not be returnable or stored past its lock scope.
This is **new borrow-checker logic** (a scope-bound / no-escape check on the guard type),
layered on the existing lexical no-escape posture — the one genuinely new *static* rule
in this ADR. A guard is created only on `lock()`'s success arm, so on the `LockTimeout`/
error arm no guard exists and nothing is dropped (the existing null-guarded `?Struct`
drop arms already model "drop only on the present arm").

### D4. `Mutex<T> = Shared<SentinelMutex<T>>`; `lock()` is fallible, yields a scope-bound guard. **PINNED.**

`Mutex<T>` is a `Shared` handle whose cell is a `SentinelMutex<T>` — so all the
co-ownership/refcount/drop work is solved once by `Shared<T>` (D2/D3) and `Mutex` adds
only the lock primitive. The runtime cell is a near-clone of the existing
`SentinelChannel`, which already wraps `std::sync::Mutex` behind a raw pointer shared by
copying and is `Sync` (`runtime:1296-1299`).

`lock()` is **fallible** (ADR 0066 D5a — it can deadlock or time out, so it is not an
infallible guard):
- **public `T`:** `lock() -> ?Guard<T>` (the `?T` shape channel `recv` already produces
  from an `(i64 status, out-ptr)` runtime return); the success arm binds a guard whose
  `*guard` reads/writes the protected `T` and whose scope-exit drop unlocks.
- **secret `T`:** `?(secret T)` is unrepresentable (`NullableInner` has no `Secret`
  variant, `types:775`), so `lock()` returns an **`OpenResult`-shaped struct** — a
  public `ok`/verdict field plus a guard/value field (ADR 0069's precedent, D6).

Atomics on a `Shared`-backed cell (`fetch_add`/`load`/`store`) come for free as
lock-free operations on the same cell shape and are the fast path for the "shared
counter" case, deadlock-free by construction.

### D5. Deadlock detection — two tiers, over public lock identity only. **PINNED (carried from ADR 0066 D5a).**

Deadlock-freedom is undecidable, so the compiler cannot reject it statically; Sentinel's
compile+runtime division of labor (like `secret_leak` + the broker's runtime UAF
detection) makes the runtime surface a **typed error instead of hanging**:
- **Always-on, cheap — `LockTimeout`.** `lock()` blocks with a runtime-configurable
  deadline; on expiry it returns the typed error rather than blocking forever.
  Implemented via `parking_lot::Mutex::try_lock_for(Duration)` (parking_lot is already a
  workspace dep, used by the broker) — cross-platform, near-zero cost, guarantees
  liveness. Not true cycle detection (false positives under heavy contention), but the
  honest "no hang" backstop.
- **Debug/opt-in, precise — `Deadlock`.** A process-wide wait-for graph (which thread
  holds each lock, which lock each blocked thread waits on); on `lock()`, before
  blocking, check whether acquiring closes a cycle and if so return a typed `Deadlock`
  carrying the cycle. Costs a meta-lock per acquire/release, so it is opt-in — a
  debug-build default or `--detect-deadlocks`, mirroring the broker's opt-in `--record`.
  Implemented as a lazy `OnceLock<Mutex<WaitForGraph>>` singleton keyed by the **public
  lock handle's raw pointer address** — the "public lock identity" D5a requires.

**Constant-time:** the wait-for graph is over lock identity + ownership (public control
data — the decision to lock is already required-public, D6), never over secret values,
so detection introduces **no secret-dependent timing channel**. The same machinery
generalizes later to channel deadlock (a follow-on, not in M1.4).

### D6. Secret shared state is in scope for v1 (phased M1.4c) and is *safe*, with two representation fixes and one honesty caveat. **PINNED (security-relevant).**

`Shared<secret T>`/`Mutex<secret T>` are **coherent and allowed** — not fenced like
FFI/cross-process — because they are **in-process**: one address space, and the
receiver is still fully `secret_leak`-checked (ADR 0066 D8's reasoning; fencing
in-process would be inconsistent with D8 and would block the parallel-constant-time-crypto
use case that is Sentinel's niche). **No new `secret_leak` sink is required**: a
secret-dependent decision to acquire/hold a lock is already a rejected secret branch.
Three consequences the M1.4c implementation must honor:

1. **Container-interner secret gap (prerequisite).** Secret elements cannot currently be
   *spelled* in the container interner — `channel_chanid_for` returns `None` for any
   `Type::Secret(_)` (`types:1365`), and the type-annotation resolver rejects a secret
   element (`ChannelElementNotSupported`, `types:1937`). Whatever generalizes the
   `Shared`/container element set to secret closes this — and it **also unblocks
   `Channel<secret T>`**, which D8 already assumes exists. A `tests/pass` fixture must
   prove `lock()`/read does not strip the `secret` qualifier (D8's named invariant), and
   a `tests/ui` fixture must prove a secret reaching a branch *inside* a lock-holding
   function is still rejected.
2. **Secret memory policy.** A `Shared<secret T>` cell's backing bytes must honor the
   broker's `mlock` + zero-on-free `SecretStrategy` (`broker/src/secret.rs`) — so the
   `Shared` allocator is **pluggable** (broker-backed `SecretPolicy::STRICT` for secret
   payloads, plain heap for public), and the refcounted free-on-last-drop path zeroes the
   cell exactly once (not per-clone).
3. **Honesty caveat (must be documented, not over-claimed).** `Mutex<secret T>` preserves
   machine-verified constant-time (every *use* is still checked) but does **not** hide
   the secret bytes from co-resident code (it never did) and does **not** make
   lock-contention latency constant. Contention latency and the `LockTimeout` deadline
   are runtime scheduling artifacts **outside the README's stated constant-time boundary**
   (which covers program-level branch/memory-address/divisor sinks, checked
   pre-optimization) — they must be stated plainly so a reader does not infer contention
   is constant-time. Over-claiming the guarantee is itself a bug (project-context).

Because this decision touches the constant-time guarantee, the secret paths get **extra
adversarial verification** at implementation, and any change that let a secret reach a
branch/index/divisor through a `Shared`/`Mutex` without a `secret_leak` rejection is a
security bug → private report, not a public PR.

### D7. Poisoning — v1 has no in-process poisoning case; reserve `LockPoisoned` for the future cross-process story. **PINNED.**

Rust's `std::sync::Mutex` poisons when a holder panics mid-critical-section. Sentinel's
runtime aborts the whole process on panic (a thin, no-unwind panic story) — so a holder
that fails **takes the entire process down**, and there is no live-lock-with-dead-holder
poisoning case *in-process*. v1 therefore uses `parking_lot` (which does not poison) and
relies on the `LockTimeout`/`Deadlock` tiers for liveness. `LockPoisoned` (SENTINEL_DESIGN2
§6) is reserved for the eventual cross-process robust-futex story, where a holder
*process* can die independently while the lock lives in shared memory — a separate future
ADR.

### D8. Runtime: net-new refcount cell (not broker-backed for the count); pluggable allocation for secret. **PINNED.**

The broker does **not** provide clone-by-refcount-freed-on-last-drop for free — its
`Arc<Arena>` is arena-granular and its `Handle` generation is a validity tag, not a
lifetime count (`broker/src/{arena,handle,ids}.rs`). `Shared<T>` is therefore a net-new
runtime cell — `struct SentinelShared { rc: AtomicUsize, value: UnsafeCell<T> }`,
essentially `Arc`'s inner behind the C-ABI: `sentinel_shared_clone` does `rc.fetch_add`,
`sentinel_shared_release` does `fetch_sub` and frees at zero. Small but genuinely new;
the broker contributes the *proven atomic-counter + UAF-detection posture* to imitate,
not a drop-in allocator. For **secret** payloads, the cell's bytes are allocated through
the broker's `SecretStrategy` (D6.2), so the allocator is pluggable.

New C-ABI symbols (unprefixed, `_S` is reserved — `abi-v1.md:232`): at minimum
`sentinel_shared_new` / `_clone` / `_release` / `_get`, `sentinel_mutex_new` / `_lock` /
`_try_lock_for` / `_unlock`, and the atomic ops. Each requires the mechanical four-step
registration: define in `runtime/src/lib.rs`; **bump the `abi_v1_runtime_symbol_set`
count assertion** (currently 38, `runtime:2053` — a hard tripwire); declare in codegen
(`codegen:616-675`); add an `abi-v1.md §5` row. The exact final symbol set + count is
pinned in the M1.4a/b implementation PRs.

### D9. This is oracle-moving — full self-host mirror, no deferral. **PINNED (process).**

New `Type` variants (`Shared` interner kind 17, `Mutex` kind 18, plus the `Guard` shape),
new builtins (shifting the user-fn FnId base), and new codegen all alter `snc`'s emitted
IR — so this follows the full rhythm and the scg mirror is **not** deferrable (it is in
the differential). The touchpoints are the well-trodden "add a handle type" recipe (5
worked examples: Task/Channel/Process/SealedChannel/Fn):
- **Rust** (`sentinel-types`): `Type` enum variant + `intern_*` helper + `TypedProgram`
  field; `no_nullable`/`no_array`/`no_vec` + TypeParam-passthrough arms; `type_display`
  + `Debug`; `resolve_type_expr` name arm; builtin signature registration; `check_call`
  dispatch arms; new `TypeError` variants. (`sentinel-resolve`): `*_FN_ID` consts +
  the user-fn-base shift. (`sentinel-codegen`): runtime-fn declarations; `cgo_ty` → ptr;
  `is_move_type`/`needs_drop` arms (**Shared/Mutex diverge here — the first `needs_drop`
  handles**); mono-mangle; `lower_*` + dispatch. (`sentinel-borrow-check`):
  `is_copy_type` returns true for `Shared`/`Mutex` (D2) but a new `needs_drop` path; the
  guard no-escape check (D3).
- **selfhost** (byte-identical): `intern.sentinel` new kinds 17/18 + `mk_shared`/
  `mk_mutex`; `cg.sentinel` `cg_is_shared`/`cg_is_mutex` predicates + `cgo_ty` + the
  `cg_needs_drop`/`cg_emit_drop` arms (the new drop-content); `cg_effects.sentinel`
  builtin dispatch + the FnId-base arithmetic mirrored in **every** dispatch arm; MIR
  callee names. Both bootstrap fixed points must stay green at every sub-phase.

## Consequences

### Positive
- Closes the last M1 concurrency gap: genuine fine-grained shared mutable state (a
  `fetch_add` counter, a read-mostly cache) without the actor round-trip, and the
  first real shared-ownership primitive the language has had.
- `Shared<T>` is reusable beyond `Mutex` (shared read-only data, atomics) and is the
  foundation the eventual cross-process `@shared`/robust-futex story (SENTINEL_DESIGN2
  §6) will build on.
- Unblocks `Channel<secret T>` as a side effect of the D6.1 container-interner fix.
- No general `Drop` trait needed (D3) — the RAII benefit without the ADR 0017 D8 cost.

### Negative
- Reintroduces **deadlock**, a bug class the channels spine designs out — mitigated but
  not eliminated by the D5 runtime detection (typed error, not a hang).
- `Shared<T>` is the first handle to break the frictionless copy-and-leak pattern; the
  clone/drop accounting (D2) is genuinely new machinery and the highest-risk part, and
  it must be mirrored byte-identically into `scg`.
- Largest single item on the roadmap; a mistake in the refcount accounting is a
  use-after-free or leak, and in the guard no-escape check a use-after-unlock — both
  serious.
- `Mutex<secret T>` muddies the clean single-owner secret story (D6) even though it is
  safe; the honesty caveat must be maintained so the guarantee is not over-claimed.

### Neutral
- `lock()` being fallible (`?Guard` / `OpenResult`) is more surface than an infallible
  guard but is the honest shape for an operation that can time out or deadlock, and
  matches the language's "no hang, surface a diagnostic" posture.
- SENTINEL_DESIGN2 §6's cross-process segments/robust-futex/`LockPoisoned` stay a
  separate far-future ADR; only the `LockPoisoned` vocabulary is reserved here (D7).

## Revisit

- **The D5 gate (M1.4-0).** If the motivating example shows channels suffice, halt and
  reconsider before building M1.4a — the cheapest possible off-ramp.
- **Copy-vs-drop mechanism (D2).** If the structural clone/drop accounting proves too
  error-prone in practice, fall back to the Move-with-explicit-`.clone()` model (reuses
  existing move/drop machinery at an ergonomic cost).
- **Deadlock detection depth (D5).** Extend the wait-for graph to channel deadlock once
  locks have paid for it, per ADR 0066 D5a.

## Estimated footprint (per sub-phase)

| Sub-phase | Rough size | Notes |
|---|---|---|
| M1.4-0 motivating example | ~1 day | pure `examples/` + analysis; the D5 gate |
| M1.4a `Shared<T>` (public) | ~600+ LOC across both compilers | the hard part: refcount cell + clone/drop accounting + the new `needs_drop` handle category + full selfhost mirror |
| M1.4b `Mutex<T>` (public) | ~400+ | lock/unlock + fallible `lock()` + guard no-escape + D5a two tiers |
| M1.4c secret `T` | ~300+ | container-interner secret fix (also unblocks `Channel<secret T>`) + broker-backed secret alloc + `OpenResult` lock shape + adversarial verification |
