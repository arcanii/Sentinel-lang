# ADR 0071: `Shared<T>` shared ownership + `Mutex<T>` — the bounded shared-state escape hatch (ADR 0066 M1.4)

Status: **ACCEPTED for M1.4a (`Shared<T>`) — implemented 2026-07-02 — and for M1.4b
(`Mutex<T>`) — implemented 2026-07-15/16/17, slices 1–4 incl. the D5a deadlock tiers
(see the implementation log + the D5 amendment); M1.4c (secret) PROPOSED — M1.4c-1 is
implemented snc-side 2026-07-19 (see the M1.4c implementation log + the D6/D4
amendment), and it stays PROPOSED until the scg mirror (M1.4c-1b) lands.** Design
PINNED with maintainer sign-off 2026-07-02. This is the M1.4 sub-phase of the ADR 0066 threading roadmap, broken out into
its own ADR per **ADR 0066 D5** ("blocked on first designing a runtime-refcounted
`Shared<T>` handle … a language feature in its own right, arguably bigger than the mutex
it unblocks, and warranting its own ADR"). All D-points D1–D9 are PINNED.

**M1.4a (`Shared<T>`, public word-scalar `T`) is DONE** (slices: runtime cell `d73322c` →
FnId-base shift 38→40 `e1e1c9f` → `Type::Shared` + lowering `d3eafe6` → refcount clone/drop
accounting `8f0a2c6` → named-Shared-return guard `c18d7be`). It delivers D2 (the first
`Copy`-for-the-checker YET drop-emitting handle), D3's drop half (a hard-coded
`sentinel_shared_release` drop arm, no `Drop` trait), D8's net-new refcount cell, and D9's
full self-host mirror (byte-identical `snc llvm` ≡ `scg`, both bootstrap fixed points green).
**One deliberately-guarded gap:** returning a NAMED `Shared` binding (a bare-`Var` tail /
`return`) is rejected (`SharedReturnNotSupported`) — the drop-drain transfer exemption works
in inkwell but its byte-identical oracle+scg mirror is a tracked follow-on (needs a reliable
direct-`Var`-tail signal the append-only selfhost dialect's `mvbv` can't give for compound
tails); returning `shared_new(...)`/a call directly (an rvalue transfer) works. **M1.4b
(`Mutex<T>` = `Shared<SentinelMutex<T>>`, D4/D5) and M1.4c (secret `T`, D6) are next** — the
co-ownership/refcount problem is now solved once by `Shared<T>`. M1.4-0 (the D5 gate,
committed `6d39952`) was done first as designed.

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
  (`NullableInner` has no `Secret` variant). D4 originally reused that shape for `lock()`
  on secret `T`; **the 2026-07-19 amendment corrects that** — a guard-returning `lock()`
  hands back a public handle, so the ordinary `?Guard<secret T>` suffices and no
  `OpenResult` is needed.
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

**Amendment (2026-07-17, maintainer-pinned at slice-4 implementation) — two deviations
from the original D5 text above, plus one hardening:**

1. **Opt-in = the `SENTINEL_DEADLOCK_DETECT` env var** (runtime-gated, read once per
   process, on = anything but empty/`0`) — NOT a `--detect-deadlocks` driver flag or a
   debug-build default. A driver flag would have to bake an enable call into the emitted
   program (oracle-moving churn — both compilers + the scg mirror — for a debug-only
   tier), and the "broker's opt-in `--record`" this section cites as precedent turned
   out to be an in-crate constructor option, not a CLI flag, so there was no literal
   flag precedent to mirror. The env var keeps slice 4 a pure runtime change (zero
   IR/ABI/driver surface; the differential is untouched by construction) and works on
   already-built binaries. A future `snc run --detect-deadlocks` convenience can simply
   set the env var without touching codegen.
2. **A detected cycle folds into the existing `LockTimeout`/null arm** — returned
   IMMEDIATELY, with the cycle reported on stderr (stable `deadlock detected` prefix) —
   NOT a new typed `Deadlock` status. In-language, `LockTimeout` itself only ever
   surfaces as the `?Guard` null arm (there is no distinct timeout value a program can
   see), so a distinct `Deadlock` status would be a new source-level surface (a third
   status/shape through both compilers + scg) bought for a debug tier; this section's
   own intent — "a typed error instead of hanging" — is met by the null arm. The
   Deadlock-vs-Timeout distinction lives in the stderr report.
3. **Wait edges are deadline-stamped** (a pre-commit adversarial-review finding, three
   lenses converging): a timed-out waiter retires its wait edge only after
   `try_lock_for` returns, so for a moment the edge outlives the wait — following that
   stale edge reported cycles that no longer existed (a false positive + a spurious
   null arm). `find_cycle(now)` treats an edge past its stamped expiry as absent; an
   unexpired edge is always a true commitment (`try_lock_for` cannot give up before its
   deadline, and the stamp is taken before the blocking call, so it expires no later
   than the physical timeout — the residual error direction is a benign missed
   detection, backstopped by the always-on `LockTimeout`). Non-blocking tries
   (`timeout ≤ 0`) neither cycle-check nor publish — they never wait.

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

**Amendment (2026-07-19, at M1.4c-1 implementation) — how D6.1/D6.2 were actually built,
plus a correction to D4's secret `lock()` shape:**

1. **D6.1's representability fix = pre-interned secret scalars at FIXED `SecretId`s.** The
   root cause was sharper than "the interner rejects secrets": the container element maps
   (`shared_id_for`/`mutex_id_for`/`guard_id_for`, and `channel_chanid_for`) are
   **table-free** — they map a `Type` to a fixed id so a container type resolves without
   threading an interner through the checker. A `Type::Secret(SecretId)` cannot participate
   because a `SecretId` is assigned in source-encounter ORDER, so it is not a knowable
   constant. Fix: pre-intern the secretable word-scalars (`i64`/`i32`/`u8`/`bool`,
   `SECRET_SCALARS`) at fixed `SecretId`s 0..=3 before any other interning, and extend the
   container maps with slots **6..=9**. `secret f64` is absent by design (a type error —
   float ops are not constant-time) and `secret ptr` too (a secret address is itself a leak
   vector). Interning those four unconditionally only fixes the secrets table's NUMBERING,
   which is invisible to the differential: no dump iterates the interner tables, the dumps
   render secrets structurally (`secret i64`, always passing the program — `<secret#N>` is a
   no-program *diagnostic* fallback) and mangling is structural (`sec_{inner}`). Verified:
   the only churn was one ui snapshot (`<secret#0>` → `<secret#2>`).
2. **D4 CORRECTION — secret `lock()` returns the ordinary `?Guard<secret T>`, NOT an
   `OpenResult`.** D4's `OpenResult` prescription predates the guard design (slice 2b-ii):
   it was chosen because `?(secret T)` is unrepresentable, but `lock()` no longer returns the
   value — it returns a **guard whose payload is the public mutex-cell handle**, and whose
   valid bit is public control data (the lock outcome, D5). Only the protected element that
   `*g` yields is secret. So `?Guard<secret T>` is directly representable and needs no new
   shape: every guard pin, drop arm and deref is reused unchanged. A second divergent
   `lock()` surface would have been pure cost.
3. **D6.2's memory policy is applied PER CELL, page-refcounted.** The broker's arenas are
   capacity-bounded slabs while these cells are individually heap-allocated and unbounded in
   number, so the policy uses the broker's *primitives* (`lock_memory`/`secure_zero`, newly
   exposed) rather than an arena: new runtime symbols `sentinel_shared_new_secret` /
   `sentinel_mutex_new_secret` (abi 49→**51**) allocate a cell that is mlocked at birth, and
   the rc==0 path volatile-zeroes its **value slot only** (the refcount/lock words are public
   control data, and zeroing a live `parking_lot::Mutex`'s own state before dropping it would
   be scribbling on a live object) exactly once, never per-clone. **mlock failure is
   FAIL-CLOSED (abort)** — continuing would silently downgrade a security property.
4. **Two hazards the adversarial review reproduced, and the fixes they forced.** Locking is
   **page-refcounted** (lock on 0→1, unlock on 1→0) because `munlock`/`VirtualUnlock` are
   page-granular and do **not** nest: the naive per-cell version unlocked the page holding a
   freed cell's still-live secret SIBLINGS (4 of 5 in the repro), losing the very property
   the birth-time lock aborts to preserve. And because each tiny cell pinned a whole page
   while Windows caps `VirtualLock` by the ~150 KiB default minimum working set, the
   fail-closed abort was reachable by ordinary programs (~4942 live cells); page-refcounting
   plus `grow_locked_memory_budget` (`SetProcessWorkingSetSize`, the documented remedy) lifts
   it — the repro now passes at 6000 and 50000 live cells. **Residual, stated honestly:** a
   hard platform ceiling still exists, and pathologically scattered cells could still reach
   it; the abort is now genuine exhaustion rather than ordinary load.
5. **D6.3's honesty caveat, restated concretely.** The cells' bytes are locked and scrubbed,
   but this does **not** hide a secret from co-resident code, and a value read out of a
   container transits ordinary registers/stack slots that are **not** scrubbed. Lock
   contention latency and the `LockTimeout`/deadlock deadlines remain **outside** the
   README's constant-time boundary. What *is* guaranteed is unchanged and machine-checked:
   the qualifier survives the container, so every downstream use is still `secret_leak`-checked.

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

## M1.4b implementation log (slices)

`Mutex<T>` (public `T`) was built in the same slice rhythm as M1.4a, each slice landing
four-check-green with both bootstrap fixed points byte-identical. Status: **COMPLETE
(2026-07-17)** — slice 3c + the D5a deadlock tiers (slice 4) have closed, flipping the
ADR to ACCEPTED for M1.4b; M1.4c (secret) remains.

- **slice 1** — the `SentinelMutex<T>` runtime cell (a `parking_lot`-backed near-clone of
  `SentinelChannel`) + the 5 C-ABI symbols (`sentinel_mutex_new`/`_lock`/`_try_lock_for`/
  `_clone`/`_release`/`_unlock`).
- **slice 2a** — the coordinated FnId-base shift 40→42 in both compilers (`mutex_new` = 40,
  `lock` = 41).
- **slice 2b-i** — `Type::Mutex` + `mutex_new(v) -> Mutex<T>` (interner kind 18), lowering to
  the refcounted cell handle.
- **slice 2b-ii** — `lock(m) -> ?Guard<T>` (`Type::Guard` interner kind 19; `?Guard` lowers
  to `{ i1, ptr }`, the `recv` `?T` shape). At this slice the guard + a bound mutex still
  **leak** — a bound-and-locked mutex was unsound (freeing a still-locked cell trips the
  runtime free-while-locked `debug_assert!`), so the fixture used an unbound rvalue.
- **slice 3a** — the `Mutex<T>` handle refcount clone/drop accounting (`Mutex<T> =
  Shared<SentinelMutex<T>>`, D4), mirroring `Shared`'s `#new + #clone == #release` invariant.

### slice 3b — the guard's unlock-on-drop (D3 drop-content + the conservative no-escape pin)

Makes a **bound** `let g = lock(m)` sound. Decisions taken (all pinned by the maintainer):

1. **Guard payload = the mutex cell handle `m`** (not the protected-slot ptr the runtime
   writes to `*out`). Keeps `Type::Guard` a single `ptr` / `?Guard = { i1, ptr }`; the guard's
   drop needs `m` for `sentinel_mutex_unlock`. (Reading the protected value via `*g` is
   deferred to **slice 3c** — it needs a runtime `sentinel_mutex_data_ptr(m)` accessor.)
2. **The `?Guard` conditionally unlocks on drop.** A bound `?Guard`'s scope-exit drop, on the
   VALID arm (a `lock()` success), calls `sentinel_mutex_unlock(m)` (`force_unlock`, no
   refcount change); on the timeout arm nothing is held → nothing unlocks. It reuses the
   existing null-guarded conditional-drop shape (the enum box-free arm). Drops fire in
   reverse-declaration order, so `g`'s unlock precedes the owning `Mutex`'s
   `sentinel_mutex_release` — the cell is unlocked before it can be freed.
3. **`Guard`/`?Guard` are MOVE, not Copy** (unlike `Shared`/`Mutex`/`Channel`). A guard has no
   refcount, so a duplicated guard would double-`force_unlock` a cell locked once. Move-tracking
   makes `let g2 = g` consume `g` → exactly one unlock.
4. **Guard no-escape = the CONSERVATIVE PIN: `lock()` may only be the direct RHS of an
   IMMUTABLE `let`** (`TypeError::GuardNotLetBound`, ui fixture `c71_guard_not_let_bound`).
   Blocks the fresh-`lock()` escapes — a block tail, a call argument, a `return`, a
   reassignment, a `let mut`. The FULL D3 no-escape (guard-VAR reshuffles into an outer scope)
   is a documented **deferred hardening** — those residual escapes are contrived and caught by
   the runtime free-while-locked assert (⚠ which is `debug_assert!`, compiled out in release —
   the reason the static pin rule is needed).

**Where the change lands (four backends, in lockstep):** the borrow-check crate flips
`Guard`/`?Guard` to Move + adds the `GuardNotLetBound` pin (shared by inkwell + the oracle);
the inkwell backend + the `snc llvm` **oracle** (`llvm_dump.rs`) both grow the
`sentinel_mutex_unlock` decl, the `lock()` payload-is-`m` change, and the `Guard` / `?Guard`
drop arms; the self-hosted **scg** (`selfhost/types/*.sentinel`) mirrors the codegen +
drop + Move byte-identically. **The types-stage pin rule is snc-only** — scg is a dump-only
port with no rejection machinery and only ever runs on oracle-accepted programs (the ui
rejection fixture is auto-skipped by every self-host differential), exactly as the peer
`SharedReturnNotSupported` / `MutexReturnNotSupported` guards are snc-only. The differential
fixture `tests/pass/c71_mutex_lock` was rewritten from the old unbound rvalue (now rejected by
the pin) to the sound bound form `let m = mutex_new(42); let g = lock(m); is_some(g)` (exit 42,
the clean exit being the unlock/leak proof).

### slice 3c — the `*g` guard deref (read + write the protected value)

Delivers the read-modify-write the whole feature exists for: `*g` READS and `*g = v` WRITES
the protected element through the held lock. New runtime C-ABI symbol
`sentinel_mutex_data(m, valid) -> *mut i64` (the abi count moves 48 → 49) returns the guard's
protected slot; it **ABORTS** (the `sentinel_panic_oob` posture — maintainer-pinned) when
`valid == 0` (a deref of a **timed-out** guard, i.e. one that did not acquire) or `m` is null,
because reading/writing the slot without holding the lock is a data race. Check `is_some(g)`
before `*g`. Keeping the valid-check in the runtime keeps all three codegen backends'
`*g` emission branch-free — extract `valid` + the cell handle `m` from the `{ i1, ptr }`
`?Guard`, `zext` the (public) valid bit, `call sentinel_mutex_data`, then `load` + decode (read)
or encode + `store` (write) on the i64 slot. The deref is **non-consuming** on the Move-typed
guard so it stays live for its unlock-on-drop, and the repeated `*g` of an RMW does not
use-after-move. Fixture `tests/pass/c71_mutex_deref` (`let m = mutex_new(36); let g = lock(m);
let v = *g; *g = v + 6; *g` → 42).

**Guard soundness (hardened by an adversarial review that caught a use-after-free BEFORE it
was committed):** the guard-deref is confined to the pinned shape — `*g` where `g` is the
directly `let`-bound guard Var, in read or assign-target position only. Everything else is a
type-check rejection (snc-only, like the peer pins; the ui fixtures are differential-skipped):
- **`& *g` / `&mut *g` (`GuardBorrowNotAllowed`, ui `c71_guard_no_borrow`).** A reference into
  the mutex cell's protected slot must not be formed: it is rooted nowhere (`?Guard` binds no
  `ref_source`), so it escaped `OutlivesSource`/`ReturnsLocalRef` — a block-tail or `return & *g`
  aliased a cell freed at the guard/mutex scope exit (a use-after-free). Only in-place `*g` is
  allowed; a guard-borrow lifetime model is a deferred hardening.
- **a COMPUTED guard operand — `*{ g }`, `*(if c { g } else { g2 })` (`GuardDerefNotVar`, ui
  `c71_guard_deref_computed`).** A non-Var operand fell to a *consuming* borrow-walk that marked
  the guard moved, skipping its unlock drop (freeing a still-locked cell) and diverging from the
  self-host mirror; the pin to the direct `let`-bound Var (mirroring `GuardNotLetBound`) forbids
  it.

Codegen note: the inkwell `*g = v` lowers **place-then-value** to match the oracle + scg
(observable via the `sentinel_mutex_data` abort). The `*g` on a **secret** element stays out of
scope (public-`T` only until M1.4c) — a secret write is a plain type mismatch against the
public guard element.

### slice 4 — the D5a opt-in `Deadlock` wait-for-graph tier (runtime-only, non-oracle-moving)

Closes D5's second tier per the 2026-07-17 amendment above (env-var opt-in; a detected
cycle folds into the existing null arm + a stderr report). **Pure runtime change** in
`sentinel-runtime`: a process-wide `WaitForGraph` (`holders`: cell address → holding
`ThreadId`; `waits`: blocked `ThreadId` → (awaited address, wait-expiry `Instant`))
behind the lazy `OnceLock<parking_lot::Mutex<..>>` this section specified, keyed by the
public lock handle's raw address. On a blocking `lock()`/`try_lock_for` with the tier on:
cycle-check under the meta-lock BEFORE blocking (on a hit: report + return status 1
without waiting); else publish the deadline-stamped wait edge, block WITHOUT the
meta-lock, then retire the wait edge + record the holder edge in one atomic meta-lock
section. `unlock` retires the holder edge BEFORE `force_unlock` (a momentarily-stale
"free" edge is a benign missed detection; a stale "held" edge after the unlock could
false-report); the rc==0 free path scrubs any pathological leftover edge (address-reuse
defense for release builds, where the free-while-locked check is compiled out). The
meta-lock is a strict leaf (never held across a blocking call), and `find_cycle` is
bounded by a seen-set (a foreign cycle formed by racing checks can neither hang the walk
nor be misattributed). **Zero compiler surface:** no new runtime symbol (abi count stays
49), no FnId/IR/driver change — all 9 self-host differential stages and both bootstrap
fixed points are untouched by construction (and verified green).

**A pre-commit adversarial review (5 lenses + mutation testing) caught a real false
positive:** the stale wait edge of a timed-out waiter (amendment point 3) — fixed by the
deadline stamp. Its mutation pass also proved four coverage holes, each now closed by a
dedicated test: the detect-on TIMEOUT arm (a timed-out acquire must retire its edge and
never overwrite the true holder's — the `is_some`-guard mutant survived the old suite),
the contended-handoff holder-edge integrity (the retire-before-`force_unlock` ordering
mutant survived), the release-path scrub (given a `mutex_release_impl(m, detect)` test
seam), and the env parse's FALSE side (`deadlock_env_value_on`, split out because the
`OnceLock` gate freezes per process). Verification: 8 runtime unit tests (self-cycle
instant; deterministic AB-BA via a published-edge spin; pure `find_cycle` incl. foreign
cycles + expired edges; off-path inert; the four above) + the end-to-end driver test
`crates/sentinel-driver/tests/deadlock.rs` — a real compiled self-deadlock program
(`let g = lock(m); let g2 = lock(m)`) run with `SENTINEL_DEADLOCK_DETECT=1` exits 42 via
the null arm in <8s (vs the 10s deadline) with the cycle on stderr. No `tests/pass`
fixture (the tier is env-gated; the driver test owns end-to-end). Operational note for
this box: `cargo test` does NOT regenerate `target/debug/sentinel_runtime.lib` — a
`cargo build` must precede any driver-test run after a runtime change, or snc links the
stale staticlib.

## M1.4c implementation log (secret containers)

Status: **M1.4c-1 (snc-side) DONE 2026-07-19; the scg mirror is M1.4c-1b and the ADR
stays PROPOSED for M1.4c until it lands.** `Channel<secret T>` is **M1.4c-2** (see below).

### M1.4c-1 — `Shared<secret T>` / `Mutex<secret T>`, snc-side

Delivers D6.1 (representability), D6.2 (memory policy) and the D4 correction, all
detailed in the 2026-07-19 amendment above. Surface: a secret element is now spellable
in `Shared`/`Mutex`/`Guard`; `shared_get`/`*g` return `secret T`; `lock()` yields the
ordinary `?Guard<secret T>`. Codegen is erasure — the element ENCODES/DECODES as its
inner scalar in both backends, and only the CONSTRUCTOR differs (the `_secret` symbol,
abi 49→51). `is_spawn_word_scalar` already admitted `Shared`/`Mutex` element-agnostically,
so capturing a secret container into a spawned worker came free.

**Verification (D6's two named fixtures).** `examples/lang/secret_shared.sentinel` (exit
42) is the positive one: its `secret i64` annotations only type-check while the container
read PRESERVES the qualifier, and reaching a public exit code needs exactly one sanctioned
`declassify` — so a compile *is* the invariant check. `tests/ui/c71_secret_mutex_branch`
is the negative one: `if *g > 0` on a `Mutex<secret i64>` is still rejected
(`sentinel::types::secret_branch` — the types stage catches an `if` on a secret condition
before MIR's `secret_leak` sees it, which is also why that fixture is excluded from the
types-stage differential corpus). Together they pin that the container did not become a
laundering hole.

**The adversarial review (5 lenses, 53 agents) confirmed 16 findings over 5 root causes —
all fixed before commit, three of them reproduced.** The two page-locking hazards and their
fixes are recorded in amendment point 4. The other three:
- **CRITICAL — the oracle emitted INVALID IR.** `RuntimeSyms::merge` folds each function's
  used-symbol flags into the module set; the two new `_secret` flags were never added to it,
  so the `declare` lines were dropped while the calls were emitted. `llvm-as` rejects the
  result — on this change's own fixture. Invisible to the suite (the corpus had no secret
  fixture, and the smoke test only asserts an exit code), so it would have shipped and then
  detonated in the scg mirror slice. Fixed + verified by assembling the oracle dump.
- **MEDIUM — UB in the `Shared` scrub:** it wrote through a pointer derived from a *shared*
  reference to a non-`UnsafeCell` field, violating `secure_zero`'s own contract; now derived
  from the raw pointer. (The `Mutex` side is fine — its slot is inside `parking_lot`'s own
  `UnsafeCell`, and `data_ptr()` is the sanctioned accessor.)
- **MEDIUM — the policy was unasserted:** deleting the scrub arms passed the entire suite.
  Three tests now pin page-refcounting, the secret flag's effect, and a large live
  population. They serialize on the process-global page table — a race the same review
  surfaced (53/53 alone, 51/53 under load).

⚠ **Review-hygiene note for future sessions:** the review's mutation pass left two planted
`if false` guards in the working tree (disabling BOTH scrub arms) and a scratch test dir.
They were caught by a pre-commit diff audit. **Always grep the diff for `if false`/`MUTANT`/
scratch files after a mutation-testing review** — a disabled security scrub is exactly the
kind of change that passes every test.

### M1.4c-1b (NEXT) — the scg mirror: element-generic containers

`c71_secret_shared` cannot enter the differential corpus yet: **scg has no element-generic
container path at all.** Its `builtin_ret` (`selfhost/types/interner.sentinel`) and its
`Shared<…>`/`Mutex<…>` annotation arm both hardcode the i64 element handle (0), so this is
the FIRST corpus-bound container with a non-i64 element. The demonstrator therefore lives
in `examples/` (outside the differential) per the M2.3b / M1.2b-cont precedent, and the
mirror is its own slice. Its four sites are already mapped:
1. **Call typing** — a `dump_container_call` helper (mirroring `dump_apply_call`'s
   "type is dynamic, decoded from the concrete arg's own interned handle" shape) dispatched
   from `dump_te_call` for FnIds 38/39/40/41: `shared_new(v) -> mk_shared(elem_of_arg)`,
   `shared_get(s) -> ta[s]`, `mutex_new`, `lock -> mk_nullable(mk_guard(ta[m]))`. scg's
   interner is STRUCTURAL (`intern_type(kind, elem, 0)`), so `Shared<secret i64>` is natively
   expressible and `render_type` already recurses correctly — this is threading, not new
   representation.
2. **cg** (`cg_effects.sentinel` fids 38/40) — choose the `_secret` symbol when the return
   type's element is a secret (kind 3), reading it off the `rt` handle already passed in.
3. **Declare group** — `cg_used_sharednewsecret`/`cg_used_mutexnewsecret` fields + tyctx
   init + declare lines + the `cg_anydecl` OR-chain.
4. **Deref decode** — the `*g` arm must strip the secret before its i64-only gate.
Once green, move the fixture back to `tests/pass/` (satisfying D6's "a `tests/pass` fixture"
requirement literally) and flip the ADR to ACCEPTED for M1.4c.

### M1.4c-2 (deferred) — `Channel<secret T>`

D6.1's fix also unblocks it (`channel_chanid_for` gains the same secret slots), but its
memory-policy story is genuinely different and deserves its own decision: a channel's
in-transit values sit in `std::sync::mpsc` queue nodes that Sentinel does not allocate, so
they can be neither mlocked nor scrubbed without replacing the queue. Deferred rather than
bundled.
