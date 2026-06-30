# ADR 0066: Threading + multi-processing roadmap — channels, shared state, processes, IPC, actors

Status: **ACCEPTED (roadmap)** — the roadmap shape + D-points were approved
by the maintainer 2026-06-29; **each sub-phase (M1.1, M1.2, …) is separately
ratified as it lands** per the both-bootstrap-fixed-points rhythm.
Implementation began at **M1.1** (generic `Task<T>` + typed spawn args).
**Landed + self-hosted so far:** M1.1 (generic `Task<T>`); M1.2 (`Channel<i64>`
+ the 4 channel builtins + 4 `sentinel_channel_*` runtime symbols); **M1.2b** —
`Channel<T>` reaches type-annotation position (the `resolve_type_expr` "Channel"
arm in both compilers, `Channel<i64>` minimum), so a fn can take a channel
endpoint as a parameter (D4's worker pattern), with the cross-thread
producer/consumer fixture `tests/pass/c66_channel_worker` (2026-06-30; no new
runtime symbols / ABI change — front-end only, plus a behavior-preserving
spawn-arg-lowering alignment across all three emitters); and **M1.3** (the worker
pattern, D1) as EXAMPLES — `examples/lang/worker_pool.sentinel` (a two-worker
fan-out/fan-in pool, built both `--separate` and merged) + the 2-channel-arg
`tests/pass/c66_channel_pipeline` relay fixture; and **M2.1** (process spawn + the `Subprocess` capability effect, D7)
— `process_spawn(path:[u8], args:[[u8]]) -> Process` + `process_wait(Process) ->
i64` over `std::process::Command`, `Type::Process` (a plain handle → ptr), the
`sentinel_process_*` runtime symbols (abi-v1 §3/§5), built on **nested arrays (ADR
0068)** for the `[[u8]]` argv; `process_spawn` carries the auto-registered built-in
`Subprocess` effect, so a spawning fn declares `! { Subprocess }` and it BUBBLES to
`main` (Pass-3 exemption — unlike `Async`, which a `scope` discharges); and **M2.2**
(byte-pipe IPC, D7/D8) — `process_spawn` pipes the child's stdin/stdout +
`process_write(Process, [u8]) -> i64` / `process_read(Process) -> [u8]`; the
cross-process secret fence (D8) is structural — the pipe payload is the public
`[u8]`, so a `[secret u8]` can't cross (rejected as a type mismatch; ui fixture
`c66_process_secret_fence`). Across M2.1+M2.2 the symbol set grew 28→32 and the
FnId base shifted 25→29 in both compilers. Fixture `tests/pass/c66_process` (spawn
+ write/read IR) + the `sentinel_process_*` runtime unit tests (real `cmd /c exit
42` / `sh -c` spawn + a `cat`/`findstr` pipe round-trip). The `?T` scalar
generalization (`?u8`/`?f64`/`?ptr`) also landed (enabler for generic channel
elements). **Next (M2.3):** typed channels over pipes — `send`/`recv` of
serializable *public* `T` across the process boundary (serialization + the fence);
generic word-scalar channel *elements*; a reusable worker-pool library.
This ADR lays out the complete threading + multi-processing vision with
pinned D-points for each piece, **implemented incrementally** across
sub-phases. It is the umbrella over the near-term maintainer ask (flagged
2026-06-27): richer threading (shared state + synchronization beyond the
single structured scope) and genuinely-new multi-processing (process spawn +
IPC). The maintainer's scoping calls (2026-06-29): **threads first,
processes later**; **full roadmap ADR** (this document); the **comms model
and the `secret` boundary fence are designed here** (see D2 / D8 / D8a).

Date: 2026-06-29

## Related

- **0024** (C4.4 structured concurrency — ACCEPTED-WITH-AMENDMENTS): the
  foundation this builds on. The surface `scope concurrent { spawn f(args);
  … }` + `.await`, the **`Async`** direct-runtime effect (discharged by the
  scope so `main` stays effect-free — NOT handler-dispatched), and the
  thread-per-spawn runtime (`sentinel_task_spawn` already runs the wrapper
  on a real `std::thread`; `SentinelTask`/`SentinelScopeCtx`; the D9
  ownership model). ADR 0024 D10 deferred *exactly* this ADR's subject:
  channels, mutexes, atomics, cross-thread `&`/`&mut`, and generic
  `Task<T>`. This ADR is the planned lift of those deferrals.
- **0020** (handler runtime — ACCEPTED-WITH-AMENDMENTS): one-shot
  continuations, deep handlers. Concurrency primitives that compose with
  `perform`/`handle` must mind the kont lifecycle (`sentinel_kont_resume` /
  `_free`, ADR 0065 D6).
- **0019** (effects kickoff): the effect-row machinery. A new **`Subprocess`**
  capability effect (D7) joins `Async` as an auto-registered built-in.
- **0029** (stable ABI — FROZEN at `abi-v1`): every new runtime symbol +
  struct layout below is an `abi-v1` addition recorded in `docs/abi-v1.md`
  §3/§5 and pinned by the `abi_v1_*` stability tests, in the same commit
  (the pre-1.0 change discipline).
- **0057 / 0059** (FFI import / C-ABI export): the **secret fence**
  precedent. A `secret` crossing `extern "C"` / `export "C"` is rejected
  because the value leaves the verified program over a public-scalar ABI.
  D8 generalizes that fence to the cross-**process** boundary and argues
  the in-process thread boundary needs **no** fence.
- **0060 / 0062** (Windows host support / conditional compilation): the
  cross-platform discipline. Threading + process spawn ride **`std`**
  (`std::thread`, `std::sync::mpsc`, `std::process::Command`) which is
  cross-platform; per-OS library code (if any) uses the ADR 0062
  `b_<os>.sentinel` file-level pattern. Do NOT add another Unix-only
  subsystem (sockets are `#[cfg(unix)]` — ADR 0056; that coupling is the
  anti-pattern to avoid).
- **0028** (broker integration): the substrate for the §6 far-future
  shared-memory-segment vision (generational handles across `fork`).
- **0008** (secret / constant-time): the guarantee D8 protects. The MIR
  pass `sentinel::mir::secret_leak` is the taint oracle that must keep
  guarding the *receiver* of any in-process message.
- **SENTINEL_DESIGN2 §6** (cross-process safety via `@shared` segments +
  robust futexes) and **§8.2** (actors with statically-checked message
  protocols; cross-process actors via broker serialization) — the
  long-horizon vision this roadmap converges toward (Movement 3).

## Context

Sentinel ships **structured concurrency** (ADR 0024): real OS threads via
`spawn` inside one `scope concurrent`, auto-awaited at scope exit. But the
surface is deliberately minimal — `Task<i64>` only, `i64` spawn args,
**owned args only** (no `&`/`&mut` across threads), and **no way for two
running tasks to share state or communicate**. ADR 0024 D10 listed the
deferred pieces verbatim: *channels, mutexes, atomics, condition variables;
cross-thread sharing of mutable references; generic `Task<T>`.* This ADR is
the design for lifting those, plus the genuinely-new multi-processing layer
(process spawn + IPC) that SENTINEL_DESIGN2 §6 deferred post-1.0.

The shape of the answer is constrained — not freely chosen — by three
load-bearing Sentinel invariants:

1. **The lexical, over-rejecting borrow checker (1.0, pre-Polonius).**
   Shared mutable `&mut` across threads is *precisely* the aliasing the
   checker forbids. ADR 0024 D10 already banned `&`/`&mut` in spawn args
   on soundness grounds. A model where values are **moved** between threads
   (message passing / ownership transfer) fits the checker *with the
   grain*; a model built on shared `&mut` + a lock fights it hardest and
   would need either Polonius (post-1.0) or a new shared-ownership type
   (Sentinel has no `Arc`).

2. **Constant-time `secret`.** The guarantee is about *control flow and
   memory-access patterns* — `secret_leak` rejects a secret reaching a
   branch, an index, or a divisor — **not** about memory isolation. This
   distinction is the crux of the fence analysis (D8): in-process threads
   share one address space, so moving a secret between them creates no new
   leak as long as the secret *type* travels with the value and the
   receiver's code is still compiled through `secret_leak`.

3. **Cross-platform from the start (Unix + Windows).** The existing OS
   layer is POSIX-coupled (sockets are `#[cfg(unix)]`). Threading and
   process spawn must ride cross-platform `std`, with `cfg` only where
   `std` genuinely doesn't reach (the §6 shared-memory segment is the one
   piece that does — it is therefore the *last*, Unix-first item).

These three force the architecture below far more than free preference
does: **message passing / ownership transfer is the spine**, shared-memory +
locks is a bounded, late escape hatch, and actors are the convergence
point that unifies in-process and cross-process under one typed-protocol
surface.

## The roadmap at a glance

Three movements, implemented in order. Each sub-phase lands four-check
green; each oracle-moving sub-phase follows the full both-fixed-points
rhythm (Rust `snc` + fixtures → re-bless the per-stage differential →
mirror into `selfhost/*.sentinel` → both bootstrap fixed points
byte-identical → mark the sub-phase ACCEPTED).

| Movement | Sub-phase | Deliverable | Oracle-moving? |
|----------|-----------|-------------|----------------|
| **1 — threading** | M1.1 | **✅ done** — Generic `Task<T>` + `T`-typed spawn args (lift ADR 0024 A3/D7) | yes (typing + codegen) |
| | M1.2 | **✅ done** — **Channels** — `Channel<T>` typed message passing (mpsc), ownership-transfer `send`/`recv` | yes (new `Type`, builtins, runtime symbols) |
| | M1.3 | **✅ done** — Worker pattern — long-lived tasks within a scope wired by channels (library + examples; no new surface) | no (library) |
| | M1.4 | `Mutex<T>` + atomics — the bounded shared-state escape hatch, with **runtime deadlock detection → typed error** (D5) | yes (new `Type` + shared-handle machinery) — **gated on a shared-ownership story** |
| **2 — multi-processing** | M2.1 | **✅ done** — **Process spawn** — `Process` handle over `std::process::Command`; `Subprocess` capability effect | yes (new `Type` + effect + runtime symbols) |
| | M2.2 | **✅ done** — **Byte-pipe IPC** — child stdin/stdout as byte streams; the cross-process **secret fence** (D8) | yes (runtime symbols + fence rule) |
| | M2.3 | Typed channels over pipes — `send`/`recv` of serializable *public* `T` across the process boundary | yes (serialization + fence) |
| | M2.4 | **`SealedChannel<secret T>`** — the AEAD-encrypted secret-cross-process path (D8a), built on the verified-constant-time `aead`/`x25519` stdlib | yes (its own ADR — Sentinel↔Sentinel only) |
| **3 — actors** | M3.x | Typed-mailbox actors (SENTINEL_DESIGN2 §8.2) as sugar over channels; same syntax in-process + cross-process | yes (large; its own ADR) |
| **far future** | — | `@shared` shared-memory segments + robust futexes (SENTINEL_DESIGN2 §6), broker-backed, Unix-first | yes (its own ADR) |

## Decisions

### D1. Structured concurrency is preserved — no detached threads.

The maintainer's phrase "a thread/worker abstraction beyond the single
structured scope" is realized as **multiple long-lived worker tasks
*within* a scope, wired together by channels** — the classic worker-pool —
**not** as unstructured detached OS threads. ADR 0024's invariant ("every
concurrent task has a parent scope and cannot outlive it; leaked tasks are
impossible by construction", SENTINEL_DESIGN2 §8.1) is a deliberate safety
property and a selling point. A detached `thread::spawn` that outlives its
scope would reintroduce leaked tasks and break the cancellation hierarchy.

So the "beyond a single scope" richness comes from (a) generic `Task<T>`
(M1.1) so tasks carry real results, (b) channels (M1.2) so tasks
*communicate* rather than just fork-join, and (c) nested / sibling scopes.
A future ADR may add `scope` modes (`scope sequential`, `scope race`, a
cancellation scope) but the scope-bounded-lifetime invariant stays.

### D2. The primary sharing/communication model is **channels + ownership transfer** (message passing).

This is the spine of the whole roadmap and the resolution of the
maintainer's "what is the recommendation aligned on Sentinel design?"

A **typed channel** carries values of a fixed element type `T` between
tasks. `send(ch, v)` **moves** `v` into the channel (the borrow checker
sees `v` as consumed, exactly as it sees a value moved into a function);
`recv(ch)` produces an owned `T` in the receiving task. No `&`/`&mut`
crosses the boundary, so no cross-thread aliasing analysis is required and
the lexical borrow checker accepts the program *with the grain*.

Rationale (each clause is a Sentinel-design alignment, not a preference):

- **Borrow-checker grain.** Moving values needs no Polonius and no shared
  handle. Shared `&mut` + a lock would need one or the other; Sentinel has
  neither at 1.0. (See D5 for the Mutex cost this avoids.)
- **No `Arc`.** Message passing requires zero shared-ownership machinery.
- **Constant-time secret (D8).** Single-owner-at-a-time means no
  shared-mutable secret state; the secret type rides the channel so the
  receiver is still `secret_leak`-checked.
- **Effects compose.** Channel ops live under the existing `Async` effect
  discipline within a scope.
- **Actors are message passing (Movement 3).** SENTINEL_DESIGN2 §8.2
  actors sit directly on channels; "cross-process actors use the same
  syntax" means channels are the one substrate that unifies in-process and
  cross-process. Choosing channels makes M1 → M2 → M3 a straight line.
- **Cross-platform.** `std::sync::mpsc` is cross-platform std.

Shared-memory + locks (the SENTINEL_DESIGN2 §6 model) is **not** the
primary model — it is the bounded escape hatch in D5, deferred and gated.

### D3. Channel surface — a library-style `Type::Channel(ChanId)` interner variant + builtins.

Mirror the shipped `Type::Task(TaskId)` / `Type::Vec` precedent exactly,
to minimize lexer/parser churn (no new keywords) and stay consistent with
how the language already models generic container types:

- Add **`Type::Channel(ChanId)`** as an interner-table variant (the
  pattern of ADR 0024 D4's `Type::Task`), `ChannelData { elem_ty: Type }`
  interned in `TypedProgram.channels`. Preserves `Type: Copy + Hash`
  (`ChanId` is `u32`). Lowers to `ptr` at the ABI boundary.
- **Builtins** (not methods, to avoid trait dispatch), in the style of the
  `Vec` builtins (`vec_new`/`push`/…):
  - `channel_new<T>() -> Channel<T>` — bounded vs unbounded: **unbounded
    mpsc at the M1.2 minimum** (`std::sync::mpsc::channel`); a bounded
    variant (`sync_channel`) is a follow-up flag.
  - `send<T>(ch: Channel<T>, v: T)` — moves `v` (ownership transfer).
  - `recv<T>(ch: Channel<T>) -> T` — blocks until a value arrives; the
    element type is preserved (so `recv` of a `Channel<secret u8>` yields
    `secret u8`, the type-system taint travelling with the value — D8).
  - `channel_close<T>(ch: Channel<T>)` — drops the sender side; a `recv`
    on a closed, empty channel is a terminal condition (see D4 for the
    error story).

A single handle that is both sender and receiver keeps the M1.2 minimum
small. Splitting into distinct `Sender<T>` / `Receiver<T>` types (so the
move semantics statically separate the two ends across tasks, the Rust
shape) is the **recommended M1.2 refinement** and is called out in D11 —
it interacts with how spawn captures the endpoints, which D4 addresses.

### D4. How a spawned task gets a channel endpoint.

ADR 0024 spawns **fn calls with owned `i64` args**. To wire workers, a
spawned fn must receive a channel endpoint as an argument. M1.1 (generic
`Task<T>` + `T`-typed args) is the prerequisite: once spawn args may be
arbitrary owned `T`, a `Channel<T>` (lowering to `ptr`) is a legal arg.
The endpoint is **moved** into the spawned fn (consumed by the caller) —
which is why the `Sender`/`Receiver` split (D11) is the clean refinement:
the producer task is spawned with the `Sender`, the consumer keeps the
`Receiver`, and the move semantics make the split a compile-time fact
rather than a runtime convention.

`recv` on a closed-and-drained channel must surface as a **typed terminal
value, not a panic** (the project rule: no `panic!` on user-program
input). Options, to be pinned at M1.2: (a) `recv` returns the nullable
`?T` the language already has (ADR 0014), `null` = channel closed; or
(b) a `recv_opt`/`try_recv` split. Recommendation: **(a)**, reusing `?T`,
so no new sum type is invented and the closed-channel case is handled with
the existing `match`/`if let` surface.

### D5. Shared state — `Mutex<T>` + atomics — is a **bounded, deferred escape hatch (M1.4), gated on a shared-ownership story; if built, it carries runtime deadlock detection.**

Some state is genuinely shared (a counter, an accumulator) and awkward to
model as messages. For those, a `Mutex<T>` (lock yields a scoped guard;
unlock on guard drop) and a small set of atomics. But this is the
borrow-checker-hardest piece and is **explicitly last in Movement 1**. The
negatives are sharp enough to enumerate, because together they are why it
is deferred rather than shipped:

1. **Two missing prerequisites, the first dominating.**
   - *No shared ownership.* A `Mutex<T>` is only useful if **two tasks
     hold a handle to the same mutex** — co-ownership. Sentinel has no
     `Arc`, and the lexical borrow checker cannot express "this handle is
     co-owned by N threads." So M1.4 is blocked on first designing a
     runtime-refcounted `Shared<T>` handle (treated as
     clone-by-refcount, freed when the last clone drops) — **a language
     feature in its own right, arguably bigger than the mutex it unblocks**,
     and warranting its own ADR.
   - *No deterministic drop.* The guard releases the lock on scope exit via
     drop, but explicit-drop rewriting is **deferred post-1.0**
     (project-context). A self-unlocking guard needs drop machinery the
     compiler doesn't fully have yet. Second prerequisite.

2. **It reintroduces deadlock — a bug class the language otherwise designs
   out.** SENTINEL_DESIGN2 §1 lists "no deadlocked cross-process locks" as a
   goal; channels/ownership-transfer largely avoid lock-ordering deadlock,
   and the type system cannot prevent it (deadlock-freedom is undecidable).
   Adding mutexes adds back a whole category of un-typed failure. (This is
   what D5's runtime-detection sub-decision below mitigates.)

3. **Poisoning needs a typed error path.** If a lock holder dies, the result
   must be a typed `LockPoisoned` (the §6 robust-futex story), not a hang —
   more surface, and it leans on Sentinel's thin panic/unwind story.

4. **It muddies the secret reasoning.** A `Mutex<secret T>` is shared-mutable
   secret state. `secret_leak` still guards each thread's *use* (so it is
   not a leak per se), but it dissolves the clean single-owner argument
   channels give for free, and lock-acquisition contention becomes a
   potential side channel the moment the *decision to lock* is
   secret-dependent (already forbidden as a secret branch, but the surface
   for the mistake is larger).

**The cost of deferring it (the honest counter-argument):** fine-grained
shared state — a counter, a read-mostly cache — modeled as an owning actor
task means a **channel round-trip per access, serialized through one
task**. Against an atomic `fetch_add` (one instruction) or an `RwLock`,
that is orders of magnitude slower, and atomics specifically are simple yet
lumped behind the same `Shared<T>` gate. **Mitigant:** Sentinel's
crypto/security niche is mostly embarrassingly-parallel or sequential —
fine-grained shared mutable state is rarer here than in general systems
code — so the deferral costs less than it would in a general-purpose
language, but it is a genuine gap, not a non-issue.

Decision: **M1.4 is gated behind the shared-ownership design.** Until then,
channels (D2) cover the real use cases — a "shared counter" becomes an
owning task that receives increment messages and answers queries (the actor
pattern in miniature, foreshadowing Movement 3). Atomics on a *single*
shared cell could land earlier than a general `Mutex<T>` (a runtime-owned,
refcounted cell sidesteps the language-level `Shared<T>`), but still need
the shared handle. **Recommendation: do not build Mutex until a real
program in `examples/` proves channels are insufficient** — the "build real
programs → find the gap → fix it" discipline that drove ADRs 0047–0054.

#### D5a. Runtime deadlock detection → a typed error (the compile/runtime division of labor).

Because deadlock-freedom is undecidable, the compiler cannot reject a
lock-ordering deadlock statically (negative #2). But **Sentinel is active at
both compile time and run time** — the same division of labor as
`secret_leak` (compile-time taint) paired with the broker's generational
use-after-free detection (runtime). So if/when M1.4 lands, the lock runtime
**detects deadlock and surfaces a typed error instead of hanging** — turning
a silent forever-hang into an observable, recoverable, debuggable condition,
consistent with the project rule *"return a typed error rather than abort"*
(the broker's `BrokerError`; SENTINEL_DESIGN2 §1's "typed error rather than
aborting"; §6's `LockPoisoned`).

Two tiers, both cross-platform:

- **Always-on, cheap — bounded-wait → `LockTimeout`.** `lock()` blocks with
  a runtime-configurable deadline; on expiry it returns a typed
  `LockTimeout` rather than blocking forever. Not true cycle detection
  (false positives under heavy contention, false negatives if the deadlock
  outlasts the deadline), but trivially cross-platform and near-zero cost,
  and it guarantees liveness — a deadlocked program reports instead of
  wedging. Good enough for the "rare runtime case" backstop.
- **Debug/opt-in, precise — wait-for-graph cycle detection → `Deadlock`.**
  The runtime maintains a global wait-for graph: which thread holds each
  lock, which lock each blocked thread waits on. On `lock()`, before
  blocking, it checks whether acquiring would close a cycle (the holder I'd
  wait on is transitively waiting on a lock I hold); if so it returns a
  typed `Deadlock` carrying the cycle (the threads + locks involved) for
  diagnosis. This is the classic detection algorithm. It costs a meta-lock
  touched on every acquire/release, so it is **opt-in** — a debug-build
  default or a `--detect-deadlocks` flag, mirroring the broker's opt-in
  `--record` mode (observation machinery you turn on, off the release hot
  path).

Both tiers make lock acquisition a **fallible** operation — `lock()` yields
the typed-error surface of D4 (`?T` or a result), not an infallible guard —
which is the honest shape for an operation that can deadlock or poison, and
keeps the language's "no hang, no abort, surface a diagnostic" posture even
in the escape hatch. **Constant-time note:** the wait-for graph is over lock
*identity and ownership* (public control data — the decision to lock is
already required public, negative #4), never over secret values, so
detection introduces no secret-dependent timing channel. The same wait-for
machinery generalizes to **channel deadlock** (all tasks of a scope blocked
on `recv` with no live sender — a cycle the scope's exit-time auto-await can
observe), a natural follow-on once locks have paid for the graph.

### D6. Generic `Task<T>` + typed spawn args (M1.1) — lift the ADR 0024 i64-only restriction.

ADR 0024 A3/D7 restricted `Task<T>` and spawn args to `i64` (the wrapper
packs/unpacks `i64` slots). Channels carrying structs, and tasks returning
real results, need arbitrary owned `T`. Two implementation routes (ADR
0024 D7 named both): **per-type monomorphized** wrappers (the spawn site
already synthesizes a per-site wrapper — extend it to pack/unpack the
actual arg/result layout) vs **runtime type-erasure** (boxed values).
**Recommendation: per-type monomorphization** — it reuses the existing
per-spawn-site wrapper machinery (ADR 0024 D8), stays within the
value-ABI, and avoids a boxed-value runtime. `secret`-typed results are
fine (the result is a `secret T`, the awaiter receives `secret T`; D8).

### D7. Multi-processing — `Process` over `std::process::Command`; a `Subprocess` capability effect.

Process spawn is **cross-platform via `std::process::Command`** (no
`cfg'd` fork/CreateProcess needed for the spawn-a-child case — `std`
abstracts both `posix_spawn`/`fork`+`exec` and `CreateProcess`). Surface:

- **`Type::Process`** (a plain handle type, no element type — not interner-
  generic) lowering to `ptr`; runtime `SentinelProcess` struct.
- Builtins: `process_spawn(path: [u8], args: [[u8]]) -> Process`,
  `process_wait(p: Process) -> i64` (exit status), and the pipe handles in
  M2.2. Cancellation/kill (`process_kill`) is a follow-up.
- A new **`Subprocess`** built-in effect (auto-registered like `Async`,
  ADR 0024 A4), so a fn that spawns a process declares `! { Subprocess }`.
  This realizes SENTINEL_DESIGN2 §7.2's "subprocess as a declared
  capability" — the effect *is* the capability, enforced transitively by
  the effect-row checker. `main` may handle/discharge it analogously to how
  `scope` discharges `Async`, **or** `Subprocess` may legitimately bubble
  to `main` (a program that spawns processes is honestly a subprocess-using
  program) — pin which at M2.1. Recommendation: **bubble to `main`** (it is
  a real capability of the program, unlike `Async` which is an
  implementation detail the scope hides); `main`'s effect-freedom is an
  `Async`-specific affordance, not a general one.

  **PINNED (M2.1, 2026-06-30): bubble to `main`.** Implemented: `process_spawn`
  carries `Subprocess`; Pass-3's main-effect-free check EXEMPTS `Subprocess`
  (every other effect must still be handled before `main`). It is a capability
  effect (process runtime), not perform-based — so a `! { Subprocess }` fn is
  exempt from the Kont* ABI and returns its value directly. `process_wait` /
  `process_write` / `process_read` are effect-free (they operate on an
  already-acquired handle; spawning is the capability-acquiring op).

### D8. The `secret` fence — the boundary is "does the value leave the verified single-address-space program?", **not** the thread count.

This is the security-critical analysis the maintainer asked to be worked
out here (Q3 was left "undecided — analyze in the ADR"). It is the call
that, if wrong, is a constant-time **security bug**. The resolution:

**In-process thread boundary → NO fence. Cross-process / IPC boundary →
fence (identical to the FFI fence).**

**IMPLEMENTED (M2.2, 2026-06-30): the byte-pipe fence is structural.** The
M2.2 IPC surface (`process_write(p, [u8])` / `process_read(p) -> [u8]`) carries
only the **public** type `[u8]`, so a `secret` cannot cross by construction: a
secret-tainted byte array is `[secret u8]` (ADR 0047), a distinct type from `[u8]`
(`secret u8 != u8`, with no implicit secret→public coercion), so it is rejected at
the call as a type mismatch — the type system *is* the fence, exactly as it is for
FFI. `declassify` remains the only sanctioned way to send formerly-secret data over
a pipe. Pinned by ui fixture `c66_process_secret_fence`. (The richer M2.3/M2.4
payloads — typed public `T` over pipes, then the `SealedChannel<secret T>` encrypted
escape — get their own per-sub-phase fence treatment, D8a.)

The reasoning rests on what the constant-time guarantee actually is. The
guarantee (ADR 0008, README boundaries) is that **the program contains no
secret-dependent branch, memory index, or divisor** — `secret_leak`
rejects those *sinks*. It is **not** a memory-isolation guarantee: a secret
sitting in heap or stack is already readable by any code in the same
address space; secrecy is enforced by *what the program is allowed to do
with the value's type*, not by hiding the bytes.

Consequences for each boundary:

- **In-process (thread spawn / in-process channel).** The value stays in
  one address space, and the **receiving function is still compiled through
  `sentinel::mir::secret_leak`**. Critically, the secret *type* rides the
  channel: a `Channel<secret u8>` yields `secret u8` on `recv` (D3), so the
  receiver's code that touches it is taint-checked exactly as if the value
  had arrived by an ordinary function argument. Moving a `secret` into a
  spawned thread is therefore **no different, for the constant-time
  oracle, from passing it to a function** — and we do not fence ordinary
  function arguments. So **no fence is needed in-process**, and allowing it
  is a *feature*: it enables parallel constant-time crypto over secret data
  (e.g. hashing independent secret blocks across worker tasks), which the
  fence would needlessly forbid.

  **Implementation invariant this rests on** (must be preserved by M1.2,
  enforced by a fixture in `tests/pass/` and a rejection in `tests/ui/`):
  the channel element type **carries `secret`** end-to-end — `recv` must
  *not* strip the qualifier. If a future optimization ever let `recv` widen
  or drop the secret type, that would be the security regression to catch.

- **Cross-process (pipe / serialization / shared segment).** Here the
  value is **serialized to bytes that leave the verified program**: through
  the kernel into another process image that may not be Sentinel-compiled
  (or may be the same binary, but the bytes still transit an OS boundary
  the verifier does not model). This is *exactly* the FFI situation that
  ADRs 0057/0059 already fence: the value ABI is public scalars, and the
  constant-time guarantee ends at the boundary. Therefore **a `secret`
  crossing a process boundary is rejected**, generalizing the existing FFI
  secret fence to IPC. A program that must send secret data cross-process
  must **`declassify` first** — the same explicit, auditable discipline as
  FFI (and `declassify` is the only sanctioned qualifier-drop, ADR 0008).

The unifying rule — **"fence ⇔ the secret leaves the verified
single-address-space program"** — is not a new concept; it is the precise
generalization of the FFI/export fence already in the language. It is the
conservative, principled position: it never weakens the guarantee
(cross-process is fenced exactly like FFI), and it declines to *over*-fence
the in-process case where `secret_leak` already does the work.

**The fence bundles two distinct properties — separate them.** The bare
"reject the secret cross-process" rule conflates:

- **Constant-time** (no secret-dependent branch / index / divisor). For
  **same-binary IPC the receiver is itself compiled through `secret_leak`**,
  so constant-time is *already preserved* regardless of how the bytes
  travelled. Only for true FFI (an arbitrary, unverified C peer) is
  constant-time at risk on the far side.
- **Confidentiality in transit** (the bytes are observable in a kernel pipe
  buffer, in swap, in a core dump, or by another UID). This is *not*
  preserved by raw IPC, and it is the property genuinely at risk even when
  both ends are verified Sentinel.

For FFI both are at risk; for same-binary IPC only confidentiality-in-transit
is. The bare fence treats them identically, which is its main negative — it
**over-fences the same-binary case**, where the only real exposure is the
bytes in transit, not constant-time.

**Negatives of the bare fence** (why D8a exists):

1. **It blocks privilege separation — a *core* security pattern, which is
   ironic for a security language.** The chief reason to use multiple
   processes in security software is isolation: a key-holding daemon, a
   privsep signer (OpenSSH's model), software-HSM emulation. The bare fence
   says *to put your secret behind a process boundary you must `declassify`
   it at the boundary* — abandoning the typed guarantee exactly where you
   wanted isolation most.
2. **`declassify`-and-re-widen opens an unprotected window** at the trust
   boundary: a span where the value is public-typed and the compiler will
   not catch a stray log or branch. The machine-checked guarantee degrades
   to developer discipline at the worst place for it.
3. **It bifurcates the channel API** (secret-carrying channels behave
   differently in- vs cross-process), cutting against the "one unified
   message-passing surface" goal.

### D8a. The encrypted escape — a `SealedChannel<secret T>` is the third sanctioned secret-cross-process path (alongside `declassify`).

The property split points straight at the fix. **Encryption is a
cryptographic declassify:**

    seal  : secret T   × secret Key → public Ciphertext     (AEAD encrypt)
    open  : public Ciphertext × secret Key → secret T        (AEAD decrypt)

Emitting the ciphertext as *public* is sound under AEAD (IND-CCA) security —
the ciphertext reveals nothing about the plaintext to a holder of neither
the key nor the plaintext. This is a recognized sound declassification in
the security-types literature ("cryptographically-masked flows",
Askarov–Hedin–Sabelfeld 2006). The receiver `open`s and the value
**re-emerges `secret T`**, so the type-level taint is preserved end-to-end
with **no plaintext `declassify`**, and the verified receiver's
`secret_leak` keeps constant-time intact. It addresses precisely the
property the bare fence left exposed (confidentiality-in-transit) without
touching the one already covered (constant-time, via the verified receiver).

**It is already with the grain — the primitives exist, verified
constant-time.** `aead` (ChaCha20-Poly1305), `x25519` KEX, `ed25519` ship in
the stdlib, and `std/net/ssh*` already runs curve25519 KEX → per-record
`chacha20-poly1305@openssh.com` encryption over a real socket. A
`SealedChannel` is that same machine pointed at a pipe instead of a TCP
socket.

**Decision: add `SealedChannel<secret T>` (M2.4) as a third sanctioned
secret-cross-process path.** The fence rule becomes: *a `secret` may cross a
process boundary only by (a) `declassify` first, or (b) a `SealedChannel`,
whose `seal`/`open` is the verified-constant-time stdlib crypto.* A raw
`ProcessChannel<secret T>` write stays rejected. This mirrors the language's
existing shape — "`declassify` is the only sanctioned qualifier-drop"
becomes "`declassify` or a `SealedChannel` is the only sanctioned
secret-cross-process path."

**Three caveats that MUST stay in the design, because getting them wrong is
a *silent* security failure** (the type says "secret preserved" while it is
not):

1. **It is a different *kind* of guarantee** — confidentiality-in-transit
   under **cryptographic assumptions + correct key management**, not
   machine-verified constant-time. The CT property is preserved by the
   verified receiver independently; `SealedChannel` must never be sold as
   "extending machine-verified CT across processes."
2. **Key management is the whole ballgame, and the silent-failure surface.**
   It needs an *authenticated* key exchange (x25519), a **fresh nonce per
   message**, and ideally forward secrecy — a mini-Noise/TLS handshake over
   the pipe (which the ssh stack prototypes). For `fork`, the child inherits
   the key in copy-on-write memory (no exchange); for `spawn`-exec, run the
   handshake — **never** pass the key via env/argv. And **ciphertext length
   leaks plaintext length**, so true CT wants padded / fixed-length messages.
3. **Scope it to Sentinel↔Sentinel** (both ends verified). For
   Sentinel↔foreign-C IPC encryption does not help — the foreign side `open`s
   and can then branch on the plaintext — so there the `declassify` fence
   stays the only path. `SealedChannel` also suits **message-passing IPC
   (pipes/channels)** cleanly but **shared-memory segments (§6) poorly** (it
   forfeits zero-copy) — one more vote for message-passing as the spine.

**Threat-model honesty.** On one machine in one trust domain, a local
attacker who can read the pipe can usually `ptrace`/read process memory too —
where the plaintext lives before/after sealing — so sealing *same-trust
local* IPC is mostly defense-in-depth (swap, core dumps, persistence). The
**decisive** win is when the IPC crosses a **trust boundary** — a different
UID, a sandbox, or the network — where the plaintext-in-memory argument does
not apply and sealing is genuinely necessary. That is also exactly the
privilege-separation case the bare fence was blocking, so D8a *resolves* the
D8 tension rather than merely softening it.

Because this is security-critical and crypto-bearing, **M2.4 gets its own
ADR** (key-exchange protocol, nonce discipline, padding, the
`SealedChannel`/`ProcessChannel` type split that makes the fence a static
property of the type). D8/D8a fix the *rule*; the M2.4 ADR fixes the
*mechanism*.

### D9. Cross-platform discipline — `std` first, `cfg` only at the edges.

- Threads: `std::thread` (already used by ADR 0024). Cross-platform.
- Channels: `std::sync::mpsc`. Cross-platform.
- Mutex/atomics (M1.4): `std::sync::Mutex` / `std::sync::atomic`.
  Cross-platform.
- Process spawn (M2.1): `std::process::Command`. Cross-platform.
- Byte-pipe IPC (M2.2): `std::process::Stdio` pipes. Cross-platform.
- **The one genuinely OS-divergent piece** is the §6 shared-memory segment
  + robust futexes (POSIX `shm_open`/`mmap`/`pthread_mutexattr_setrobust`
  vs Windows `CreateFileMapping`/named mutexes). It is therefore the
  **last, far-future** item, and when it lands it uses the ADR 0062
  file-level `b_<os>.sentinel` conditional-compilation pattern for any
  per-OS *library* code and `#[cfg(...)]` for the *runtime* split — never a
  bare `#[cfg(unix)]` that silently drops Windows (the ADR 0056 socket
  coupling is the mistake not to repeat).

### D10. Runtime symbols + ABI (`abi-v1` additions).

Every new runtime symbol and `#[repr(C)]` struct below is added to
`docs/abi-v1.md` §3/§5 and the `abi_v1_runtime_symbol_set` /
`abi_v1_struct_layout` stability tests **in the same commit** (the pre-1.0
discipline, ADR 0029). Provisional set:

- **M1.2 channels:** `sentinel_channel_new() -> ptr`,
  `sentinel_channel_send(ch: ptr, payload: ptr, size: i64)`,
  `sentinel_channel_recv(ch: ptr, out: ptr) -> i64` (return = 0 ok / 1
  closed, the `?T` story of D4), `sentinel_channel_close(ch: ptr)`. The
  `SentinelChannel` struct (mpsc sender+receiver, Box-wrapped) gets a
  layout test like `SentinelTask`. Element payload is passed by pointer
  + size (the per-type monomorphized pack/unpack of D6).
- **M2.1 processes:** `sentinel_process_spawn(path, path_len, argv, argc)
  -> ptr`, `sentinel_process_wait(p: ptr) -> i64`,
  plus `sentinel_process_stdin/stdout` pipe accessors at M2.2. The
  `SentinelProcess` struct wraps `std::process::Child`.
- These are **new** symbols (no change to the frozen `sentinel_task_*` /
  `sentinel_scope_*` set), so existing artifacts keep linking; the ABI
  grows, it does not break.

### D11. Recommended refinements (named so M-phases don't paint into a corner).

- **`Sender<T>` / `Receiver<T>` split** (D3): the clean shape — the two
  endpoints are distinct types so the producer/consumer split across tasks
  is a compile-time move, not a convention. Land at M1.2 if cheap; else
  M1.3.
- **Bounded channels** (`sync_channel`): backpressure. A flag on
  `channel_new`. Follow-up.
- **`select` over multiple channels**: needed for non-trivial worker
  topologies. A later surface addition.
- **Cancellation scope** (`scope race`, early-exit cancellation — ADR 0024
  A2/D9 deferred): orthogonal but related; its own follow-up.

### D12. Out of scope for this roadmap (named, with where they go).

- **Detached/unstructured threads** — rejected by D1 (breaks the structured-
  concurrency invariant), not deferred.
- **`@shared` shared-memory segments + robust futexes** (SENTINEL_DESIGN2
  §6) — far-future, its own ADR, broker-backed (ADR 0028), Unix-first.
- **Full actor system** (SENTINEL_DESIGN2 §8.2) — Movement 3, its own ADR;
  this roadmap only guarantees the channel substrate it will build on.
- **Work-stealing scheduler / fibers** (ADR 0024 D6 deferral) — a
  runtime-only perf upgrade, surface-invisible, unrelated to this surface
  work.
- **Async-as-effect via the handler runtime** (ADR 0024 D6 vs ADR 0021
  D9) — still blocked on multi-shot continuations (ADR 0020 D2); unchanged
  by this ADR.

## Reasoning

**Why message passing is the spine, not a preference.** The single most
constraining fact is the lexical borrow checker. It cannot express shared
mutable aliasing across threads, and it has no shared-ownership type to
build a `Mutex` handle on. Every other concurrency model either fights that
(shared `&mut` + lock) or requires a prerequisite the language doesn't have
(`Arc`). Channels need *nothing new* from the checker — a `send` is a move,
which the checker already models perfectly. This is the same "design with
the grain of the checker, scope the borrow tighter rather than weaken the
checker" principle that governs the rest of the language
(`docs/borrow-check-limitations.md`).

**Why channels also serve the constant-time and actor goals.** The same
single-owner-at-a-time property that satisfies the borrow checker also
means no shared-mutable secret state (D8), and channels are literally what
the §8.2 actor vision is built from. One model — message passing —
simultaneously fits the checker, protects the secret guarantee, and is the
straight-line path to the long-term actor north star. That convergence is
why it is the *recommendation aligned on Sentinel's design*, not merely a
safe default.

**Why the fence is a boundary property, not a thread property (D8).** The
temptation is to fence every thread hop "to be safe." But that
misunderstands the guarantee: `secret_leak` constrains *the program's
sinks*, and it keeps running on the receiver of an in-process message. The
honest boundary is whether the value escapes the verified address space —
which is exactly the FFI boundary the language already fences. Fencing
in-process would forbid legitimate parallel constant-time crypto while
adding zero safety. The conservative *and* correct call is to fence the
real escape (cross-process) and trust `secret_leak` in-process.

**Why threads-first (the maintainer's call) is also the right ordering.**
Threads build directly on the shipped `std::thread` runtime; the new
surface (generic Task, channels) is reusable wholesale by the process layer
(typed channels over pipes, M2.3, are the same `send`/`recv` surface).
Doing processes first would mean designing IPC before the in-process
message-passing surface it should mirror — backwards.

## Consequences

### Positive

- One coherent message-passing surface spans in-process threading,
  cross-process IPC, and (eventually) actors — exactly SENTINEL_DESIGN2
  §8.2's "same syntax cross-process" promise.
- Fits the lexical borrow checker with no weakening and no Polonius
  dependency; the structured-concurrency safety invariant is preserved.
- The secret fence generalizes the existing FFI fence — no new security
  concept, the guarantee is provably not weakened, and parallel
  constant-time crypto over secret data becomes expressible. The
  `SealedChannel` escape (D8a) restores privilege-separation across a process
  boundary *without* abandoning the secret type, reusing the
  verified-constant-time crypto stdlib.
- Locks, if built, never silently hang: runtime deadlock detection (D5a)
  surfaces a typed `LockTimeout` / `Deadlock`, keeping the language's
  "no hang, no abort, surface a diagnostic" posture even in the escape hatch
  — the compile-time-can't-decide / runtime-catches division of labor.
- Cross-platform by construction (`std`-backed); the one OS-divergent piece
  is isolated to the far-future §6 item.
- Each piece is a small, independently-shippable, ADR-0024-shaped increment
  ("C4.4 minimum" style), each landing four-check green and (where
  oracle-moving) both-fixed-points green.

### Negative

- `Mutex<T>` is deferred behind a shared-ownership (`Shared<T>`/refcount)
  design *and* deterministic drop (both missing at 1.0 — D5); programs that
  genuinely want fine-grained shared mutable state must, for now, model it as
  an owning task, which costs a channel round-trip per access vs an atomic /
  `RwLock`. Mitigated by Sentinel's mostly-parallel-or-sequential niche, but
  a real expressiveness/perf gap. If built, it reintroduces deadlock — a bug
  class the language otherwise designs out — mitigated (not eliminated) by
  the D5a runtime detection, which makes the failure observable, not absent.
- Generic `Task<T>` (D6) via monomorphization grows code size per spawned
  type (the usual monomorphization trade); acceptable, matches the rest of
  the compiler.
- The cross-process secret fence (D8) means secret data cannot transparently
  cross a process boundary: it must be `declassify`d *or* sent through a
  `SealedChannel` (D8a). The sealed path restores the use case but trades
  the machine-verified guarantee for a confidentiality guarantee under
  cryptographic assumptions + correct key management — a sharper, more
  assumption-laden edge than the in-process case, and its own ADR (M2.4).
- A full roadmap ADR commits more design up front than a tight first
  increment would; mitigated by everything past M1.2 being explicitly
  revisable (PROPOSED, per-D revisit triggers below).

### Neutral

- Channels add a `Type::Channel(ChanId)` interner variant — the seventh+
  such variant, a well-worn pattern (`Task`/`Vec`/…); no `Type: Copy`
  regression (`ChanId` is `u32`).
- `Subprocess` joins `Async` as an auto-registered built-in effect — the
  effect-row machinery already handles auto-registration with no
  special-casing.
- New runtime symbols grow `abi-v1` additively; no existing symbol changes,
  so already-compiled artifacts keep linking.

## Alternatives considered

- **Shared-memory + `Mutex` as the primary model** (SENTINEL_DESIGN2 §6).
  Rejected as primary: fights the lexical borrow checker hardest, needs a
  shared-ownership type the language lacks, and the §6 segment machinery is
  the one genuinely Unix-coupled piece. Retained as the bounded, far-future
  escape hatch (D5 / D12), not the spine.
- **Actors as the first deliverable** (SENTINEL_DESIGN2 §8.2). Rejected as
  *first*: a much larger surface (typed protocols, mailbox typing, the
  compiler tracking who-may-talk-to-whom). Retained as Movement 3, built on
  the channel substrate this ADR ships first — so the actor ADR inherits a
  working message-passing layer instead of inventing one.
- **Fence `secret` at every thread boundary** (the strict Q3 option).
  Rejected: over-fences. It would forbid parallel constant-time crypto over
  secret data while adding no safety, because `secret_leak` already guards
  the in-process receiver (D8). The boundary, not the thread count, is the
  honest fence.
- **A bare cross-process fence with `declassify` as the only escape.**
  Rejected as *sufficient*: it blocks the privilege-separation pattern that
  is a core reason a security language wants processes at all (D8 negatives).
  Kept as one of two escapes; the `SealedChannel` (D8a) is the other, for the
  Sentinel↔Sentinel case where the receiver is verified.
- **Transparent always-encrypt on every `ProcessChannel`** (encryption hidden
  by the runtime). Rejected: hides key management — the exact thing whose
  mishandling is a silent security failure (nonce reuse, no forward secrecy,
  no authenticated exchange). `SealedChannel` makes the encryption a visible,
  typed, opt-in path so the developer can reason about the key story; the
  mechanics are handled by the verified library, but the choice is explicit.
- **Compile-time deadlock prevention** (reject lock-ordering deadlock
  statically). Rejected: undecidable in general; a sound static
  approximation (lock hierarchies / ordering types) is a large feature for an
  escape hatch that should stay small. Runtime detection → a typed error
  (D5a) is the proportionate answer, matching the broker's runtime-UAF
  precedent.
- **Detached threads / a raw `thread::spawn` surface.** Rejected by D1:
  breaks structured concurrency's leaked-tasks-impossible invariant. The
  worker-pool-within-a-scope pattern (D1) gives the same expressiveness
  without abandoning the safety property.
- **Processes-first.** Rejected (and counter to the maintainer's call):
  would design IPC before the in-process message-passing surface it should
  mirror.

## Revisit

This ADR is **PROPOSED**; each movement is ratified as its sub-phases land.
Per-D revisit triggers:

- **D2 (channels as spine):** revisit only if a real `examples/` program
  proves message passing is insufficient for a core use case — that is the
  trigger to pull M1.4 (Mutex) forward.
- **D5 (Mutex deferral):** revisit when the shared-ownership (`Shared<T>` /
  refcounted handle) design exists, or a benchmark shows the owning-task
  pattern is a real bottleneck. **D5a (deadlock detection):** revisit the
  always-on tier (timeout vs cycle-detection default) once locks have real
  contention data; extend the wait-for graph to channel deadlock then.
- **D8a (`SealedChannel`):** its own ADR at M2.4 — revisit the key-exchange
  protocol, nonce/forward-secrecy discipline, and the length-padding rule;
  any weakness there is a confidentiality regression, report privately.
- **D6 (generic Task via monomorphization):** revisit if code-size from
  monomorphized spawn wrappers becomes a problem (→ type-erasure).
- **D8 (the secret fence):** the security-critical decision — revisit if
  the in-process invariant (the channel must carry `secret` end-to-end to
  the `secret_leak`-checked receiver) is ever at risk, or if a
  same-binary-different-process IPC case suggests a narrower fence. Any
  change here is a constant-time-guarantee change → report privately if a
  leak is found, not a public issue.
- **D7 (`Subprocess` bubbles to `main`):** revisit if a sandboxing use case
  (SENTINEL_DESIGN2 §7.3) wants `main` to *mask* subprocess capability
  rather than carry it.
- **Movement 3 (actors) / the §6 segment:** each gets its own ADR when
  reached; this roadmap only commits to not foreclosing them.

## Estimated implementation footprint (per sub-phase, ADR-0024-scale)

| Sub-phase | Rough LOC | Oracle-moving rhythm |
|-----------|-----------|----------------------|
| M1.1 generic `Task<T>` + typed args | ~300 | yes — re-bless + selfhost mirror |
| M1.2 channels (`Type::Channel` + 4 builtins + 4 runtime symbols) | ~600 | yes — full rhythm + `abi-v1` |
| M1.3 worker pattern (library + examples) | ~150 | no (library) |
| M1.4 `Mutex<T>` + shared-ownership prerequisite + deadlock detection (D5a) | ~800+ | yes — likely its own ADR first |
| M2.1 process spawn + `Subprocess` effect | ~500 | yes — full rhythm + `abi-v1` |
| M2.2 byte-pipe IPC + cross-process secret fence | ~400 | yes — fence fixtures (pass + ui) |
| M2.3 typed channels over pipes (public `T`) | ~500 | yes — serialization + fence mechanism |
| M2.4 `SealedChannel<secret T>` (AEAD over pipes) | ~600 | yes — its own ADR; reuses `aead`/`x25519` |
| Movement 3 (actors) | (own ADR) | — |

Each sub-phase is independently shippable and four-check green; M1.1 → M1.2
is the near-term critical path (generic Task unblocks channel endpoints as
spawn args, D4).
