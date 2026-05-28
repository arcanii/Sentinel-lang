# ADR 0024: C4.4 — structured concurrency (scope / spawn / await) + Async effect + runtime scheduler

Status: PROPOSED — to flip to ACCEPTED (or ACCEPTED-WITH-AMENDMENTS)
at C4.4 close. This ADR details the surface syntax + typing
rules + runtime architecture for Phase C4.4 per ADR 0021 D8 +
D9. The scheduler is the largest new runtime component since
Phase A's broker; this ADR carries the substantive design call
mirroring the role of ADR 0020 for the C3 handler runtime.

Date: 2026-05-29
Related:
  - **0021** (Phase C4 kickoff — PROPOSED): the umbrella ADR.
    D8 picks `scope concurrent { ... } + spawn expr + .await`
    as the surface; D9 picks async-as-effect via the C3
    handler runtime. ADR 0024 fills in the surface + typing
    + scheduler detail.
  - **0019** (Phase C3 kickoff — ACCEPTED-WITH-AMENDMENTS): the
    effect system. The Async effect declared here is a regular
    effect from the effect-row perspective.
  - **0020** (Handler runtime — ACCEPTED-WITH-AMENDMENTS):
    one-shot continuations + Kont* runtime. D2 deferred
    multi-shot continuations indefinitely. ADR 0024 D6 amends
    ADR 0021 D9: at C4.4 minimum we ship a **direct runtime
    API** rather than handler-based dispatch (which would need
    multi-shot to model the scheduler cleanly).
  - **0023** (C4.2 trait + impl — ACCEPTED-WITH-AMENDMENTS):
    the Task<T> type interacts with trait dispatch as a
    typical generic-shaped type at the user surface; at C4.4
    minimum Task is monomorphic per a sub-iteration.

## Context

C4.3 closed with delegation; the trait + impl + delegation
surface is complete. C4.4 adds the structured-concurrency
surface so user programs can express parallelism explicitly
within a cancellation-bounded `scope`.

Three design tensions resolved up front:

1. **Async-as-effect vs direct runtime API**. ADR 0021 D9
   committed to async-as-effect via the C3 handler runtime.
   But ADR 0020 D2 deferred multi-shot continuations
   indefinitely — and the scheduler model naturally needs
   multi-shot (each task is a separate resumption of the
   captured continuation). Reconciling: C4.4 ships a **direct
   runtime API** (sentinel_task_spawn / sentinel_task_await /
   sentinel_scope_enter / sentinel_scope_exit). The Async
   effect annotation is a typing-layer marker that `spawn` and
   `await` operations require — the effect-row machinery
   from ADR 0019 D2/D3 validates the annotation. The
   "handle-and-dispatch" lowering envisioned by ADR 0021 D9
   is deferred until multi-shot continuations land. This is
   an amendment to ADR 0021 D9.

2. **Threading vs cooperative scheduling**. At C4.4 minimum we
   pick **thread-per-spawn** (libc pthread_create or Rust
   std::thread::spawn). This is the simplest viable runtime —
   no scheduler state, no work-stealing queue, no fiber
   machinery. The user surface (scope/spawn/await/Task<T>) is
   identical to what a real work-stealing pool would provide;
   the implementation can be swapped without breaking the
   surface. **Real work-stealing pool deferred** to a follow-on
   sub-phase (C4.4b) or post-C4 perf work.

3. **Spawn surface — function calls only at C4.4 minimum**.
   `spawn fn_name(args)` is supported; `spawn { ... }` (arbitrary
   block) and `spawn closure_value(args)` (closure spawn) are
   deferred. Restricting to fn calls means the spawned work has
   a known C-ABI entry point — no closure environment capture
   needed at C4.4 minimum. The compiler emits a per-spawn-site
   wrapper that calls the named fn and stores the result in
   the Task struct.

## Decisions

### D1. `scope concurrent { ... }` block grammar.

A scope block is a new expression form:

    scope concurrent {
        stmt;
        stmt;
        tail_expr
    }

The `concurrent` keyword is positional (a plain Ident at the
lexer per C4.0 D11). Other scope modes (e.g., `scope
sequential`, `scope race`) are reserved for future ADRs.

The scope's *expression value* is its tail (same as a regular
block). The scope's *concurrency contract*: every Task spawned
inside the block must be awaited or auto-await at exit per D9.

### D2. `spawn expr` grammar.

`spawn` is a prefix unary operator over an expression. At C4.4
minimum the expression is restricted to a **function call**:

    spawn fn_name(arg1, arg2, ...)

with `fn_name` resolving to a non-effecting (Async-only is
fine) fn in scope. Arbitrary spawnable expressions (blocks,
method calls, closures) are deferred per D10.

`spawn` produces a value of type `Task<T>` where T is the
return type of the spawned function call. The spawn site
records the task in the enclosing `scope concurrent` for
auto-await at scope exit per D9.

### D3. `expr.await` postfix grammar.

`task.await` is a postfix form over a `Task<T>`-typed
expression. The result type is T. `.await` blocks the caller
until the spawned task produces a value.

`await` may appear anywhere a Task<T> is in scope — not
restricted to inside `scope concurrent` blocks. (The scope
block bounds task lifetime, not await location.)

### D4. `Type::Task(TaskId)` interner extension.

Add `Type::Task(TaskId)` as the tenth interner-table-style
variant (0014 + 0015 + 0016 + 0017 + 0019 + 0020 + 0022 +
0023 + 0024). `TaskData { result_ty: Type }` interned in
`TypedProgram.tasks: Vec<TaskData>`. Preserves
`Type: Copy + Hash` (TaskId is u32). Each distinct spawn
site's result type produces one interned TaskData entry.

### D5. `Async` built-in effect.

`Async` is auto-registered as a built-in effect at the start
of the resolve pass (alongside the runtime fn builtins). It
declares no ops at C4.4 minimum — the marker alone suffices
for effect-row tracking. The effect-row checker requires
that any fn containing a `spawn` or `.await` declares
`! { Async }` in its effect-row annotation.

This differs from ADR 0021 D9's vision that Async would have
operations `spawn<T>(body: () -> T) -> Task<T>` and
`await<T>(t: Task<T>) -> T`. At C4.4 minimum spawn/await are
**built-in expression forms** rather than effect operations —
the typing-layer marker captures the effect-tracking
discipline without requiring the (deferred) handler-runtime
dispatch.

### D6. Scheduler architecture: thread-per-spawn at C4.4 minimum.

Each `spawn fn(args)` allocates:
  - A heap-allocated `Task` struct: `{ done: AtomicBool, result: i64, thread_handle: pthread_t }`.
  - A per-spawn-site C-callable wrapper fn: receives args,
    calls the spawned fn, stores result in Task, sets done.
  - A `pthread_create` (or std::thread::spawn) of the wrapper.

`.await` calls `pthread_join` on the task's thread handle,
reads `result`, frees the Task.

`scope concurrent { ... }` at entry pushes a per-scope
TaskRegistry to track spawned-but-not-yet-awaited tasks. At
exit, walks the registry and auto-awaits each remaining task.
Cancellation on early scope exit is **DEFERRED** — at C4.4
minimum, an early `return` inside a scope still auto-awaits
all spawned tasks (slower than cancellation but simpler).

**Real work-stealing scheduler with cooperative tasks**
deferred to C4.4(b) or post-C4. The thread-per-spawn
implementation lets the user-facing surface ship + run
end-to-end at C4.4 minimum.

### D7. Runtime API (new symbols in sentinel-runtime).

Five new runtime symbols:

  - `sentinel_task_spawn(wrapper_fn: ptr, args_storage: ptr, args_size: i64) -> *Task` — allocates Task + thread.
  - `sentinel_task_await(task: *Task) -> i64` — joins, returns result, frees Task.
  - `sentinel_scope_enter() -> *ScopeCtx` — allocates a per-scope task registry.
  - `sentinel_scope_register(scope: *ScopeCtx, task: *Task)` — registers task in scope.
  - `sentinel_scope_exit(scope: *ScopeCtx)` — auto-awaits + frees remaining tasks; frees ScopeCtx.

The Task and ScopeCtx structs are opaque to codegen (passed as
ptr). Layout lives in sentinel-runtime with stable-layout
tests like ADR 0020 D7's SentinelKont.

At C4.4 minimum Task result_ty is restricted to i64. Other
result types (bool / structs / classes / Task<Task<T>>) are
deferred. Generalizing to arbitrary T needs either runtime
type-erasure (boxed values) or per-type runtime variants.

### D8. Lowering strategy.

For `spawn fn_name(args)` at compile time:
  1. Synthesize a wrapper fn `__spawn_wrapper_{n}` with C-ABI
     signature `void wrapper(*Task task_ptr, *ArgsStorage args)`.
     Body: unpacks args from args storage, calls fn_name(args),
     stores result in task_ptr.
  2. At the call site: allocate ArgsStorage on the heap, pack
     args, call sentinel_task_spawn(wrapper, args, size).
  3. Get a Task* back; wrap it as an LLVM ptr-typed value
     (`Type::Task(TaskId)` lowers to ptr).

For `task.await`:
  1. Lower task to a ptr value.
  2. Call `sentinel_task_await(task_ptr)`.
  3. Return the i64 result.

For `scope concurrent { ... }`:
  1. Call `sentinel_scope_enter()` to get a ScopeCtx*.
  2. Lower the body — each spawn inside also calls
     `sentinel_scope_register(scope, task)` after spawning.
  3. Lower the tail expression.
  4. Call `sentinel_scope_exit(scope)` to auto-await + cleanup.
  5. Return the tail expression's value.

The ScopeCtx* is threaded through codegen via a new
`current_scope: Option<PointerValue>` field on CodegenCtx.
Nested scopes save+restore.

### D9. Scope semantics — auto-await on exit, no cancellation.

When a `scope concurrent { ... }` block exits normally (tail
expression produces a value), the scope-exit code auto-awaits
all spawned-but-not-yet-awaited tasks. The tail expression's
value is returned (tasks' values are discarded if never
awaited explicitly).

Early exit via `return` / `panic` / early-error is **not
specially handled** at C4.4 minimum — the scope-exit code
runs at the lexical end of the block; if Rust-style
guard-based scope-end machinery surfaces (RAII drop runs the
auto-await), great; if not, the scope is leaked to the OS on
panic. **Cancellation propagation is DEFERRED** to a follow-
on.

### D10. Out-of-scope at C4.4.

The following stay deferred to follow-on ADRs or post-C4:

  - **Cancellation propagation** on early scope exit. D9.
  - **Generic Task<T> for T != i64**. Restricted to Task<i64>
    at C4.4 minimum. Generalizing to arbitrary T needs runtime
    type-erasure or per-type variants — straightforward but
    not C4.4 minimum.
  - **Spawn arbitrary expressions** (`spawn { ... }`,
    `spawn closure(args)`). Restricted to fn-call expressions.
  - **Real work-stealing scheduler with fibers**. Thread-per-
    spawn at minimum; work-stealing is a follow-on perf upgrade.
  - **Async-as-effect via the C3 handler runtime**. Requires
    multi-shot continuations (ADR 0020 D2 deferral).
  - **`scope sequential` / `scope race` modes**. Only
    `concurrent` at C4.4 minimum.
  - **Effect-row composition with Async**. Async is auto-
    registered but at C4.4 minimum spawned fns can declare
    other effects too; the effect-row union just adds Async.
    No special handling of mixed effects.
  - **Task chaining (task.then(...) / task.map(...))**.
  - **Channels, mutexes, atomics, condition variables**.
  - **Cross-thread sharing of mutable references**. The
    borrow checker's lexical model doesn't track cross-thread
    aliasing; spawned fns at C4.4 minimum must take owned
    args (no `&` / `&mut` refs allowed). This is a soundness-
    motivated restriction; lifting it requires extending the
    borrow check.

### D11. Lexer (no new tokens at C4.4).

C4.0 reserved `scope`, `spawn`, `await`. `concurrent` stays
as a positional Ident (mirroring `to` for delegations). C4.4
activates them at the parser layer. No lexer changes.

### D12. Phase-go program.

At `tests/pass/c44_go_no_go.sentinel`:

    fn double(x: i64) -> i64 ! { Async } {
        x * 2
    }

    fn main() -> i64 {
        let result: i64 = scope concurrent {
            let a: Task<i64> = spawn double(21);
            let b: Task<i64> = spawn double(10);
            a.await + b.await + 0
        };
        result
    }

Expected: exit 42 (21*2 + 10*2 = 42 + 20 = 62 — wait, let me
recompute: 21*2 = 42, 10*2 = 20, sum = 62. Let me use
`a.await - b.await` for a target of 42 — 42 - 20 = 22, still
not 42. Use `a.await + 0`: 42 alone. Or pick spawn args that
sum to 42: spawn double(15) and spawn double(6) yields 30+12=42.
Simpler: just spawn double(21) and await its result — 42.)

Adjusted:

    fn main() -> i64 {
        let result: i64 = scope concurrent {
            let t: Task<i64> = spawn double(21);
            t.await
        };
        result
    }

Exercises: scope concurrent block, spawn of a fn call producing
Task<i64>, .await postfix, fn-level Async effect annotation,
scope-exit cleanup. Exit 42.

### D13. Sub-phase split.

C4.4 splits into two sub-iterations mirroring C4.1 / C4.2:

  - **C4.4 (1/N)**: AST + parser + lexer-already-done +
    resolve pass-through that surfaces "scope/spawn/await
    not yet supported" diagnostics. Mirrors C3.0 / C4.1
    (1/N) / C4.2 (1/N). Estimated 0.5-1 session.

  - **C4.4 (2/N)**: types layer (Type::Task interner, Async
    built-in effect, spawn/await typing) + runtime symbols
    + codegen lowering + the c44_go_no_go phase-go. The
    substantive runtime work. Estimated 1.5-2 sessions.

Total C4.4: 2-3 sessions per the ADR 0021 D14 estimate.

## Reasoning

**Direct runtime API over async-as-effect (D6 amendment to
ADR 0021 D9)**. The cleanest implementation of async-as-effect
requires multi-shot continuations — each task is conceptually
a fresh resumption of the spawn site's captured continuation,
and the scheduler picks which resumption to drive next. ADR
0020 D2 deferred multi-shot indefinitely, with the rationale
"one-shot suffices for the bootstrap". For C4.4 we honor that
deferral by choosing a direct runtime API. The user surface is
identical (`scope`/`spawn`/`await`); only the lowering
strategy differs. If multi-shot lands later, the lowering can
be retrofitted without changing the surface.

**Thread-per-spawn for the C4.4 minimum (D6)**. The
work-stealing scheduler envisioned in ADR 0021 D9 is a
substantial runtime component (1k+ LOC). Shipping thread-per-
spawn first proves the surface works end-to-end and gives a
concrete baseline. The scheduler swap is a runtime-only
change (no surface impact); a follow-on sub-phase can land it.

**Fn-call-only spawn (D2 + D10)**. Closures require capture
analysis + per-closure env structs + lifetime tracking. The
fn-call restriction sidesteps all of this — the spawned work
has a known C-ABI entry point that takes its declared params.
Most useful spawn sites are fn calls anyway. Closures are an
ergonomic improvement to land in a follow-on.

**Task<i64> only (D7 + D10)**. Type-erased Task<T> requires
either a runtime type-tag + dispatch (similar to the C3 kont
PURE_RETURN_OP_ID sentinel) or per-type runtime variants
(monomorphized Task structs per T). Restricting to Task<i64>
at minimum lets the runtime ship with one concrete Task
struct + one set of spawn/await/scope symbols. Generalizing
is straightforward but extra LOC.

## Consequences

### Positive

- Ships structured concurrency surface end-to-end at C4.4
  minimum — programs can express parallelism explicitly.
- Direct runtime API is straightforward to implement,
  understand, and test. No multi-shot continuation gymnastics.
- The surface (`scope`/`spawn`/`await`/`Task<T>`) is the same
  as what a real work-stealing pool would provide — the
  scheduler can be upgraded without breaking user code.
- The Async effect annotation discipline catches "fn calls
  scheduler primitives but doesn't declare Async" at typing
  time, preserving the C3 effect-row discipline.
- Builds on the C3 effect-row machinery + the C4.1/C4.2 class
  + trait surface without churn — most of the new code is in
  sentinel-runtime + the codegen lowering.

### Negative

- Thread-per-spawn is slow at scale (kernel thread overhead).
  Acceptable for the C4 bootstrap minimum; future perf work
  upgrades to a real scheduler.
- Restricting Task<T> to T=i64 limits expressiveness. The
  c44_go_no_go phase-go is i64-only; broader programs need the
  type-erased follow-on.
- Cancellation deferral means scope-bounded resource cleanup
  may leak on early exit. Documented and accepted at C4.4
  minimum.
- Amendment to ADR 0021 D9 (async-as-effect → direct API)
  partially walks back the original vision. The async-as-effect
  rewrite remains available once multi-shot continuations land
  — the path forward is documented.

### Neutral

- No new lexer tokens — the C4.0 reservation pays off here.
  Parser activation only.
- The Async effect joins the user-declarable effects machinery
  with no special-casing at the effect-row level — it just
  happens to be auto-registered like the runtime fn builtins.
- The borrow-check restriction (no `&` / `&mut` refs in spawn
  args at C4.4 minimum) is conservative but sound. A future
  ADR can lift it with cross-thread aliasing analysis.

## Alternatives considered

- **Async-as-effect at C4.4 minimum** (per ADR 0021 D9).
  Rejected: requires multi-shot continuations (ADR 0020 D2
  deferred). Returning to this path is a future amendment when
  multi-shot lands.

- **Real work-stealing scheduler at C4.4 minimum**. Rejected:
  too much new runtime code (per-thread queues, work stealing,
  task state machines) for one sub-phase. Threading is the
  cheaper baseline; work-stealing is a follow-on.

- **Closure-based spawn** (`spawn { ... }`). Rejected: closure
  capture machinery isn't in the bootstrap yet. Fn-call-only
  is the minimum useful surface.

- **Generic Task<T> at C4.4 minimum**. Rejected: needs runtime
  type-erasure or per-T variants. Task<i64> is the simplest
  baseline; generalization is mechanical when the use case
  surfaces.

- **Scope cancellation on early exit at C4.4 minimum**.
  Rejected: requires either RAII drop integration (the C2.4
  drop_plan machinery would need to know about scope exit) or
  explicit unwind protection. Auto-await on exit is simpler.

## Revisit

This ADR is **PROPOSED** until C4.4 closes. Per-D revisit
triggers:

- **D6 (scheduler)**: revisit when user code surfaces perf
  pressure from kernel thread overhead. The fiber-based
  work-stealing pool is the planned upgrade.
- **D7 (runtime API + Task<i64>-only)**: revisit when user
  code wants Task<bool>, Task<struct>, Task<Task<T>>, etc.
  Type-erased Task is the planned generalization.
- **D9 (no cancellation)**: revisit when user code surfaces
  resource-leak issues from early scope exit. RAII drop
  integration is the planned mechanism.
- **D6 vs ADR 0021 D9 (direct API vs async-as-effect)**:
  revisit when multi-shot continuations land (per ADR 0020
  D2 deferral). Async-as-effect lowering becomes feasible
  at that point; the user surface stays the same.

## Estimated implementation footprint

| Layer                                   | LOC estimate |
|-----------------------------------------|--------------|
| Lexer (no changes — C4.0 already)       | 0            |
| AST (+ScopeExpr / SpawnExpr / AwaitExpr)| ~80          |
| Parser (+parse_scope / parse_spawn /    | ~250         |
|         postfix .await dispatch)        |              |
| Resolve (pass-through + Async effect    | ~150         |
|          auto-registration; NotYet at 1/N) |          |
| Types (Type::Task + spawn/await typing  | ~300         |
|        + Async effect-row plumbing)     |              |
| sentinel-runtime (5 new symbols +       | ~250         |
|        Task + ScopeCtx structs)         |              |
| Codegen (lower scope / spawn / await +  | ~400         |
|        per-spawn wrapper synthesis)     |              |
| Fixtures (c44_go_no_go + UI)            | ~80          |
| Tests across all crates                 | +30-50       |
| **Total**                               | **~1500 LOC**|

This is comparable to ADR 0023's C4.2 footprint (~1100 LOC)
but slightly higher due to the new runtime component. Two
sub-iterations split it cleanly: (1/N) parser + AST + resolve
NotYet (~500 LOC); (2/N) types + runtime + codegen + fixtures
(~1000 LOC).
