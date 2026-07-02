# ADR 0071 M1.4-0 — the D5 gate: do channels suffice for shared state?

**Purpose.** ADR 0066 D5 pinned "do not build `Mutex` until a real program in `examples/`
proves channels are insufficient." This is that exercise: two real, compiling,
deterministic Sentinel programs that attempt genuinely-shared state with the existing
spawn + `Channel` actor spine, plus the honest weigh-up. It is the M1.4-0 step of ADR
0071 and gates M1.4a (the `Shared<T>` machinery).

**Verdict in one line:** channels are **sufficient** for commutative fan-in accumulation
(the common "shared counter" shape) but hit a **hard wall** — not merely a throughput
cost — on worker-side *correlated* read-modify-write. The gap is real; its incidence in
Sentinel's own (mostly embarrassingly-parallel crypto/security) domain is low. Both
readings are laid out below so the go/no-go is an informed maintainer call.

## What was built

Both build with `snc build` and run deterministically → exit 42 (verified 5× each).

- **`examples/lang/shared_counter_via_channel.sentinel`** — a shared counter as an
  owning `counter` task: three workers concurrently `send` increments into one channel;
  the task folds them into its private `total` and returns it at EOF. No shared mutable
  state exists — `total` has exactly one owner. **Channels handle this cleanly.**
- **`examples/lang/shared_sequence_via_channel.sentinel`** — a shared sequence/ID
  allocator: seven workers each `send` a request and read back a value from a shared
  `replies` channel. It runs, but *only because the result we check (the final sequence
  value) is order-independent* — see the wall below.

## The finding

### Where channels are sufficient (the common case)

A shared counter / accumulator is a **commutative, associative fold** (sum, count, max).
The ADR 0066 D2 actor pattern covers it exactly: one owning task, workers send values
that *move* into the channel, no lock, no data race (single owner), no deadlock (no wait
cycle). The only cost is throughput — every update is a `send` serialized through one
task where a `Mutex`/atomic would be an in-place `fetch_add`. For Sentinel's typical
coarse-grained accumulation that cost is acceptable. **This is not a gap.**

### Where channels hit a hard wall (the narrow but real case)

The moment a worker must run a **non-commutative read-modify-write and use its own
result** — allocate *my* unique id, then write to `slot[my_id]`; read the shared state,
decide based on it, act on the outcome — channels in Sentinel fail, and not gracefully:

1. **Replies are unaddressed.** With one shared `replies` channel, the value a worker
   reads back may be the answer to a *different* worker's request. For a bare counter
   that is harmless; for "my id, which I then use," it is simply wrong.
2. **There is no clean fix in current Sentinel**, specifically:
   - You cannot pass a per-worker reply-channel handle *through* the request, because
     channels carry only word-scalars (`Channel<i64>`), not `Channel<Channel<i64>>` (ADR
     0066 M1.2b element set).
   - You cannot `select`/poll over N per-worker reply channels — Sentinel has no
     multi-channel select — so N static reply channels don't compose either.
   - The only correct encodings are to **serialize** the workers (one request in flight
     at a time, defeating the concurrency that is the whole point) or to **pre-generate**
     all results work-stealing (only valid when the result doesn't depend on per-worker
     state).

A `Mutex<i64>` dissolves this in four lines — the worker runs its own critical section in
place, gets its own value, and uses it directly:

```
fn claimer(seq: Mutex<i64>) -> i64 {
    let g = lock(seq);      // guard on success
    let mine = *g;          // read
    *g = mine + 6;          // atomic read-modify-write
    mine                    // MY value — correlated, usable in place (g drops -> unlock)
}
```

So the gap is not "channels are slower here." It is "channels **cannot express** this
concurrently in Sentinel today." That is a genuine expressiveness wall.

## The weigh-up (both readings, honestly)

**For proceeding to M1.4a:**
- The wall is real and concrete, not hypothetical — a correlated RMW is a normal
  concurrency need, and Sentinel currently has *no* concurrent way to do it.
- `Shared<T>` (the actual M1.4a deliverable) is useful **beyond** `Mutex`: shared
  read-only config/tables across workers without copying, lock-free atomics for a hot
  counter (the throughput case above), and the foundation the eventual cross-process
  `@shared` story (SENTINEL_DESIGN2 §6) builds on. The gate is about `Mutex`, but M1.4a
  earns its keep on shared ownership alone.
- Closing the M1.4c secret path also closes the standing `Channel<secret T>` gap that ADR
  0066 D8 already assumes exists.

**For pausing after M1.4-0:**
- This exercise did **not** surface a *Sentinel-domain* program that hits the wall. The
  crypto/security workloads in `examples/` (worker pools, SSH sessions, sealed channels)
  are fan-out/fan-in — channel-friendly by construction. The correlated-RMW wall is real
  in general systems code but rare in this domain.
- `Mutex` reintroduces deadlock, a bug class the channels spine deliberately designs out
  (mitigated but not eliminated by D5a's runtime detection).
- M1.4 is the heaviest single item left on the roadmap (D2's Copy-vs-drop tension); the
  cost/benefit against a low-incidence gap is a real judgment call.

## Recommendation

The gate is **satisfied** in the strict sense: a real, concrete inadequacy was
demonstrated (not merely a perf gap). The honest caveat is that its **in-domain
incidence is low** — no crypto/security example here needed it. The maintainer decision
(taken 2026-07-02: proceed to the full `Shared<T>` + `Mutex`) is defensible primarily on
`Shared<T>`'s standalone value (shared read-only data + atomics + the secret-container
unblock), with the correlated-RMW `Mutex` case as the pinning motivation. Recorded here
so the narrowness is on the record, and so a future reader knows the escape hatch was
built on a *demonstrated* gap, thin as its domain incidence is.

**Next:** M1.4a — the `Shared<T>` refcounted handle (ADR 0071 D2/D3/D8).
