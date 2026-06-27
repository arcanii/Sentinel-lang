# SESSION_PREP.md — Paste-ready prompt for the next chat session

> **⚠ HISTORICAL — do not use as current state.** This boot prompt is frozen at
> the **C3.5(d)** milestone (pre-1.0). The language has since reached 1.0 and
> self-hosts. To resume work, use **[`HANDOVER.md`](HANDOVER.md) §0** +
> **[`STATE.md`](STATE.md)** (the source of truth); this file is kept only for
> provenance and may be deleted.

Copy everything between the two horizontal rules below into the first
message of a fresh chat. It's a self-contained boot prompt: tells the
agent what's been done, where the code lives, what to read first, and
what's next.

---

Continuing Sentinel-lang work. Repo:
https://github.com/arcanii/Sentinel-lang

Local HEAD: `dc0aeab` (docs(c3.5d): STATE banner + HANDOVER §0.2/§0.3
+ ADR 0020 D9 status). Last feat commit: `d35218c` (feat(c3.5d):
unified embedded-perform shape per ADR 0020 D7). Branch state:
verify with `git status` at session start. Working tree clean.

## Where we are

Phase A (broker) + Phase B (effects-proto) + Phase C0 (bootstrap
MVP) + Phase C1 (full type system, 8 sub-phases) + Phase C2 (refs
+ borrow check + RAII drop, 6 sub-phases) + **Phase C3 typing
layer (6 sub-phases through C3.3) all complete** + **Phase C3
runtime layer (5 sub-phases through C3.5(d)) in flight**.
Sub-phases C3.5(e) → C3.6 → C3.7 remain per ADR 0020 D9.
**1065 active workspace tests + 1 doctest.**

ADR status:
  - 0001-0014 + 0016 + 0017: ACCEPTED (or ACCEPTED-WITH-AMENDMENTS).
  - 0015 (C1.6 arrays): ACCEPTED-WITH-AMENDMENTS.
  - 0017 (Phase C2): ACCEPTED-WITH-AMENDMENTS. All 14 D-decisions
    exercised across C2.0.1 → C2.5.
  - 0018 (Polonius migration plan): PROPOSED. Documents the
    lexical → flow-sensitive borrow-check migration; no
    migration code yet.
  - 0019 (Phase C3 typing): ACCEPTED-WITH-AMENDMENTS. All 14
    D-decisions exercised across C3.0 → C3.3. Amendments: A1
    SecretEscapesPolymorphism subsumed by monomorphic generics;
    A2 runtime builtins declared effect-free.
  - **0020 (handler runtime): PROPOSED.** 12 D-decisions; picks
    free-monad reification over CPS / stack-saved; deep + one-
    shot handlers. **8 of 12 D-decisions exercised so far** (D2
    one-shot, D4 surface + default return-arm, D5 AST+parser+
    resolve, D6 effect discharge, D7 all 5 runtime symbols, D11
    fn-main integration, partial D1 free-monad lowering via the
    frame chain). Flips to ACCEPTED at C3.7 close.

Twenty-three go/no-go programs run end-to-end via snc:

| Fixture                                        | Sub-phase | Stdout / Exit |
|------------------------------------------------|-----------|---------------|
| c05_go_no_go                                   | C0        | "10"          |
| c14_go_no_go                                   | C1.4      | "7"           |
| c15_go_no_go                                   | C1.5      | "142"         |
| c16_go_no_go                                   | C1.6      | "15"          |
| c17_go_no_go                                   | C1.7      | "42"          |
| c20_go_no_go                                   | C2.0.2    | "53"          |
| c21_go_no_go                                   | C2.1      | "168"         |
| c22_go_no_go                                   | C2.2      | "35"          |
| c23_go_no_go                                   | C2.3      | "100"         |
| c24_go_no_go                                   | C2.4      | "160"         |
| c25_go_no_go                                   | C2.5      | "190"         |
| c31_go_no_go                                   | C3.1      | "100"         |
| c32_go_no_go                                   | C3.2      | "42"          |
| c33_go_no_go                                   | C3.3      | "42"          |
| c35_handle_inline_perform                      | C3.5(a)   | exit 42       |
| c35_handle_log_returns_msg                     | C3.5(a)   | exit 42       |
| c35b_handle_fn_call_body                       | C3.5(b)   | exit 42       |
| c35b_handle_multi_arm                          | C3.5(b)   | exit 42       |
| c35b_handle_pure_return                        | C3.5(b)   | exit 42       |
| c35c_let_bound_perform                         | C3.5(c)   | exit 42       |
| c35c_let_bound_perform_with_capture            | C3.5(c)   | exit 42       |
| c35d_binop_with_perform                        | C3.5(d)   | exit 42       |
| c35d_perform_with_capture_and_binop            | C3.5(d)   | exit 42       |
| c35d_perform_in_call_arg                       | C3.5(d)   | exit 42       |

Pipeline (unchanged since C3.3):

    parse_query → resolve_query → check_query
                → effect_check_query → borrow_check_query → codegen

The full type universe: `{ I64, I32, Bool, Struct(StructId),
Nullable(NullableInner), Array(ArrayElem), TypeParam(TypeParamId),
GenericInstance(GenericInstanceId), Ref(RefId), Secret(SecretId),
Kont(KontId) }`. Seventh interner-table ADR preserving
`Type: Copy + Hash`.

## Handler runtime state at C3.5(d) close

Five runtime symbols in `sentinel-runtime`:

  - `sentinel_perform_op(op_id: u32, arg: i64) -> *mut SentinelKont`
  - `sentinel_kont_resume(kont, value: i64) -> i64` — drains the
    kont's frame chain head→tail, calling each resumer with the
    accumulated value, freeing each frame + its captured state +
    the result kont (assumed pure-return wrap at C3.5(d)).
  - `sentinel_kont_panic_resumed() -> never` — one-shot abort.
  - `sentinel_kont_pure(value: i64) -> *mut SentinelKont` — wraps
    a pure value with `PURE_RETURN_OP_ID = u32::MAX`.
  - `sentinel_kont_consume_pure(kont) -> i64` — symmetric unwrap.
  - `sentinel_kont_push(kont, resumer, captured)` — adds a frame
    onto the kont's chain.

`SentinelKont` layout (32 bytes, 8-byte aligned):
`{ op_id: u32, _pad: u32, arg: i64, consumed: u8, _pad2: [u8; 7],
frames_head: *mut SentinelFrame }`. Codegen reads `op_id` at
offset 0 + `arg` at offset 8 via byte-offset GEP.

`SentinelFrame`: `{ resumer: extern "C" fn(i64, *mut u8) -> *mut
SentinelKont, captured: *mut u8, next: *mut SentinelFrame }`.

**Codegen paths** in `compile_fn`:

  1. **Let-shape** (C3.5(c)): body = `{ let v: i64 = effecting_rhs;
     pure_tail }`. Detected via `detect_let_shape`; per-fn
     resumer pre-declared in pass 1. Captureds are fn params
     referenced in the tail.
  2. **Embedded-perform shape** (C3.5(d)): body = `{ }` stmts +
     tail contains exactly one perform anywhere (binop,
     struct-lit, fn-call arg, index, etc.). Detected via
     `detect_embedded_perform_shape`; tail is substituted with
     `Var(placeholder)` for the resumer.
  3. **Tail-produces-kont** (C3.5(b)): body's tail is direct
     Perform OR call-to-effecting-fn. Goes through the regular
     compile_fn path; lower_expr produces the kont.
  4. **Pure-bodied effecting fn** (C3.5(b)): body's tail is
     pure. Wrapped via `sentinel_kont_pure` at fn return.
  5. **Fallback** (C3.5(b)): validate-and-lower; rejects with
     `effecting_fn_body_not_direct` otherwise.

`lower_handle` (in lower_expr): body must be Perform OR
call-to-effecting-fn. Always emits a runtime switch on the
kont's op_id with one case per arm + a dedicated `PURE_RETURN_OP_ID`
case calling `sentinel_kont_consume_pure`. Result is a phi over
all cases' arm-body values.

## Workspace crates (lib test counts)

  - `sentinel-base` — Salsa db + Diagnostic accumulator (3).
  - `sentinel-syntax` — lexer + parser (293 lib).
  - `sentinel-ast` — AST types (54).
  - `sentinel-resolve` — name resolution (58).
  - `sentinel-types` — type checking (152).
  - `sentinel-borrow-check` — lexical borrow check + DropPlan
    (43).
  - `sentinel-effect-check` — effect inference + main-must-be-
    effect-free (11).
  - `sentinel-codegen` — LLVM IR via inkwell (47).
  - `sentinel-runtime` — `sentinel_alloc/print/panic_oob/free` +
    handler runtime (8).
  - `sentinel-driver` — `snc` binary; 95 pass-tests via
    `tests/pass.rs`.
  - `sentinel-broker` — Phase A allocator crate (62 lib + 5
    integration + 2 proptest).
  - `sentinel-effects-proto` — Phase B research interpreter
    (203 lib + 23 integration; kept for reference).
  - `sentinel-{hir,mir,lsp}` — scaffolds for later phases
    (1 smoke test each).

## Read in order (for full context)

1. `docs/HANDOVER.md` §0 in full — the canonical "where the
   codebase is right now" pointer.
2. `docs/STATE.md` — last-updated banner for C3.5(d) details.
3. `docs/decisions/0020-handler-runtime-and-perform-lowering.md`
   — PROPOSED. The pre-flight design ADR for C3.4-C3.7. D9
   sub-phase table now shows C3.5(e) as next.
4. `crates/sentinel-runtime/src/lib.rs` — the 5 handler runtime
   symbols + SentinelKont/SentinelFrame structs. Read
   `sentinel_kont_resume` to understand the frame-replay loop —
   the C3.5(e) work extends this loop.
5. `crates/sentinel-codegen/src/lib.rs` — focus on:
   - `compile_fn` dispatch (around line ~1300).
   - `compile_effecting_fn_with_let` (C3.5(c)).
   - `compile_effecting_fn_with_embedded_perform` (C3.5(d)).
   - `lower_handle` (the runtime switch).
   - `detect_let_shape` + `detect_embedded_perform_shape`.
6. `tests/pass/c35*` — the seven C3.5-era fixtures.

## Sanity check at session start

```bash
cargo build --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace                  # expect 1065 passing
cargo run --bin snc -- build tests/pass/c33_go_no_go.sentinel -o /tmp/c33
/tmp/c33 && echo "exit=$?"              # expect "42" then "exit=0"

# Smoke-test the latest handler fixtures:
cargo run --bin snc -- build tests/pass/c35d_perform_in_call_arg.sentinel -o /tmp/c35d
/tmp/c35d; echo "exit=$?"               # expect "exit=42"
```

If everything is green, you're booted.

## Next: C3.5(e) — chained effecting lets via resumers-can-perform

Per ADR 0020 D9 the remaining handler-runtime sub-phases:

  - **C3.5(e)** — chained effecting lets + multiple performs in
    tail (1-2 sessions). **Next.**
  - **C3.6** — return arms with non-identity transforms + nested
    handles + multi-shot continuations per D2 relaxation (1-2
    sessions).
  - **C3.7** — polish + phase-go (c37 fixture per ADR 0020 D12)
    + ADR 0020 PROPOSED → ACCEPTED flip (0-1 sessions).

### What C3.5(e) unblocks

Programs like:

    fn do_two_io() -> i64 ! { Io } {
        let a: i64 = perform Io.read();
        let b: i64 = perform Io.read();
        a + b
    }

Currently surfaces `effecting_fn_body_not_direct` because:

  - `detect_let_shape` requires `stmts.len() == 1` (single let).
  - `detect_embedded_perform_shape` requires `count_performs(tail)
    == 1` (no nested performs).
  - The fallback validate path rejects any non-tail perform.

ADR 0020 D12's phase-go fixture is also blocked on this — it
uses a `let logged = perform Io.log(x); x + logged` shape (one
effecting let + a binop tail using the let-bound var). This is
*within* C3.5(c)'s coverage but the multi-perform extension at
C3.5(e) is what closes the general case.

### Why resumers must be able to perform

Today: every resumer's body wraps its result via
`sentinel_kont_pure` and returns the wrap. The runtime resume
loop assumes every result kont is a pure-return wrap; it unwraps
the `arg` field and moves to the next frame.

With chained lets, the second `let b = perform Io.read()` lives
inside the FIRST let's resumer body. When that resumer runs (at
resume time, with `a` bound to the first resumed value), it hits
another perform — which returns a real op-perform kont, NOT a
pure-return wrap.

The runtime needs to handle this case:

  1. Resumer R1 returns kont K2 (op-perform, NOT pure-return).
  2. Drain loop sees op_id != PURE_RETURN_OP_ID.
  3. The remaining frames in K1's chain (frames AFTER R1) belong
     to the COMPUTATION after `let b = ...`. Those frames need to
     be moved onto K2 so when K2 eventually resumes, those
     downstream frames execute.
  4. Drain loop returns K2 to handle, which dispatches the
     matching arm.

The "frame migration" step is the substantive piece. Frame
ownership shifts from K1 to K2; K1's chain is shortened to just
the frames already drained (which is empty since we're at the
problematic frame).

### Concrete C3.5(e) tasks

1. **Runtime**: extend `sentinel_kont_resume` to detect
   non-pure-return result konts. When detected, splice the
   remaining frames onto the new kont, return the new kont from
   resume (changing the return type, OR adding a separate
   `sentinel_kont_resume_or_bubble` symbol).

   Decision needed: how does the arm body's `k(v)` call see this?
   Currently the arm body's `k(v)` lowers as `i64 =
   sentinel_kont_resume(kont, v)` and the i64 flows into the
   arm's expression. If the resumer performs, the arm body sees
   a kont* instead — but its return type is i64. This is the
   same value-vs-kont problem we've avoided so far.

   One option: arm body's ResumeKont lowering takes the
   effecting fn ABI shape — returns either a value (i64) or a
   kont* (bubble). The arm body's outer Handle dispatches: if
   value, use directly; if kont, RE-DISPATCH on the new kont.

2. **Codegen**: detect chained-effecting-let shape:
   `stmts: [Let { effecting }, Let { effecting }, ...], tail: pure`.
   Emit a CHAIN of per-let resumers. Each resumer's body
   evaluates the next let + the rest. The runtime's resume loop
   walks each in turn.

   Alternative: a single resumer that takes a "stage" index
   and dispatches via switch. Simpler but less canonical.

3. **Tests**: c35e_chained_perform fixture, c35e_multi_arm_chained
   fixture, etc.

### Alternative scope for C3.5(e)

If the resumers-can-perform work is too substantive in one
session, alternative scope:

  - Only the runtime extension (no codegen changes yet). Set up
    `sentinel_kont_resume_or_bubble` for future codegen to
    consume.
  - Or: implement chained lets via single resumer that handles
    multiple stages internally (no resumer-perform recursion).
    Restricted to `let a = perform Op(); let b = perform Op(); ...`
    where every let is a direct perform (no captured surrounding
    context per let). Resumer's body is a switch on stage.

## Alternative path: skip to C3.6 (return arms + nested handles)

C3.6 is independent of C3.5(e) for the most part. Return arms
need handle codegen to dispatch to a transform when no perform
fires; multi-shot needs the kont's `consumed` flag relaxed +
deep-clone-on-resume. Nested handles need scoped frame ownership.

Each piece is smaller than C3.5(e). Could be done in any order.

## Working norms (from HANDOVER §0.1)

- **Trust STATE.md, not the git log.**
- **Small patches, build between each.** Cargo build + cargo
  test + cargo clippy after every meaningful change.
- **ADR-first per phase boundary.** ADR 0020 PROPOSED; C3.5(e)
  lands code per its D9 sub-phase split.
- **feat + docs commit pairs per sub-phase.** Code in the feat
  commit; STATE.md + HANDOVER §0 refresh + ADR status update in
  the matching docs commit.
- **cargo clippy --workspace --all-targets -D warnings** is
  part of the four-check suite.
- **Minimal ceremony.** Short replies are the norm.

---

End of paste block.
