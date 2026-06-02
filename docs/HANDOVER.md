# HANDOVER.md — Sentinel Bootstrap Compiler Implementation

This document is the practical handover for starting work on the Sentinel
bootstrap compiler in Rust. It assumes you have read SENTINEL_DESIGN.md
and SENTINEL_DESIGN2.md and have decided to proceed with the staged
validation approach described in Section 16.3 of the design document.

Read this top to bottom once before writing any code. Then use it as a
reference as you work through the milestones.

---

## 0. Current Implementation Status

> This section is the canonical "where the codebase is right now"
> pointer. For per-crate detail and design decisions, read
> docs/STATE.md and the ADRs under docs/decisions/.

**Phase A — sentinel-broker — complete.** Generational arenas
(bump + slab), scoped budgets, diagnostics, recording mode,
secret-memory policy. 69 active tests + 1 doctest. See STATE.md
Section A. ADR 0001 (staged validation) is the umbrella.

**Phase B — sentinel-effects-proto (Sentinel-Mini) — complete.**
Research-grade tree-walking interpreter validating Sentinel's
effect-system design before the production compiler commits.
226 tests (203 lib + 23 integration). All three HANDOVER §5.2
validation demos landed (supply-chain, async-as-effect,
password-verify). The crate is explicitly throwaway per
HANDOVER §5; deletion-eligible once C3 absorbs its lessons.
ADRs 0002-0008 are authoritative. See STATE.md Section B.

**Phase C0 — bootstrap compiler MVP — complete.** The new
production-shape crates (sentinel-syntax, sentinel-ast,
sentinel-codegen, sentinel-driver, sentinel-runtime) now ship a
full lex → parse → AST → two-pass LLVM IR → object → cc-linked
executable pipeline via the `snc` binary. The ADR 0010 appendix
go/no-go program runs:

    fn double(x) { x * 2 }
    fn pick(cond, a, b) { if cond { a } else { b } }
    fn main() {
        let x = 5;
        let y = pick(x, double(x), 0);
        print(y)
    }
    // stdout: "10\n", exit 0

Six sub-phases C0.0-C0.5 shipped across twelve feat+docs commits.
22 pass-test fixtures cover the full surface. ADRs 0009 (Phase C
kickoff) and 0010 (concrete C0 surface) are ACCEPTED. Everything
is `i64` per ADR 0009 ("no type system in C0"); `bool` arrives at
C1.3. See STATE.md Section C.

**Phase C1.0 — Salsa retrofit — complete.**
**Phase C1.1 — sentinel-resolve crate lift — complete.**
**Phase C1.2 — annotation grammar + sentinel-types::check() — complete.**
**Phase C1.3 — bool + i32 + comparison + logical operators; ADR 0010 D9 retired — complete.**
**Phase C1.4 — struct definitions + field access + struct literals — complete.**
**Phase C1.5 — nullable types `?T` + null literal + unwrap_or / is_some builtins — complete (D10 deferred; retired at C1.6).**
**Phase C1.6 — arrays `[T]` + indexing `a[i]` + `len` builtin + heap runtime + ADR 0014 D10 unlock — complete.**
**Phase C1.7 — witness-table generics (generic fns + generic structs + monomorphisation) — complete. Phase C1 closes.**
**Phase C2.0.1 — lexer (`&` token + `mut` keyword) — complete.**
**Phase C2.0.2 — refs / mutability / deref / assignment infrastructure — complete.**
**Phase C2.1 — shared-only lexical borrow checker — complete.**
**Phase C2.2 — `&mut T` + shared-XOR-mutable rule — complete.**
**Phase C2.3 — move semantics + use-after-move — complete.**
**Phase C2.4 — RAII / drop + `sentinel_free` (closes the C1.6+ heap-leak deferral) — complete.**
**Phase C2.5 — polish + Polonius migration plan + struct-field recursive drop + ADR 0017 → ACCEPTED-WITH-AMENDMENTS — complete. Phase C2 closes.**
**Phase C3.0(a) — lexer: six new keywords (`effect`, `secret`, `declassify`, `handle`, `with`, `perform`) — complete.**
**Phase C3.0(b) — AST + parser + resolve pass-through + types-layer rejection for effect_decl / effect_row / `secret T` / `declassify(e)` — complete.**
**Phase C3.1 — secret typing: `Type::Secret(SecretId)` interner + `declassify(e)` + implicit `T → secret T` widening + 2 of 4 CT rejections (SecretBranch, SecretInRefDeref) — complete.**
**Phase C3.1b — operator-secret-preserving rules + SecretDivisor — complete.**
**Phase C3.2(a) — effect_decl + effect_row data model in resolve + types — complete.**
**Phase C3.2(b) — sentinel-effect-check crate + effect_check_query salsa pass — complete.**
**Phase C3.3 — typing-layer close-out: c33_go_no_go fixture + ADR 0019 → ACCEPTED-WITH-AMENDMENTS — complete. Phase C3 typing layer closes.**
**Phase C3.4 — handler runtime typing layer per ADR 0020 D5+D6: AST + parser + resolve + Type::Kont interner + type-check + effect discharge; codegen lands at C3.5/C3.6 — complete.**
**Phase C3.5(a) — restricted-case handler codegen per ADR 0020 D7: 3 new runtime symbols + lower Perform/ResumeKont/Handle (body must be a direct `perform`); end-to-end runnable for the inline case — complete.**
**Phase C3.5(b) — effecting fn ABI + handle-of-call per ADR 0020 D7: effecting fns return Kont* at the IR level; sentinel_kont_pure wraps pure values via PURE_RETURN_OP_ID; handle codegen unified around a runtime switch — complete.**
**Phase C3.5(c) — let-bound perform via per-let resumer fns + sentinel_kont_push per ADR 0020 D7: first piece of per-eval-site frame reification; SentinelKont grows a frame-chain; sentinel_kont_resume replays frames head→tail — complete.**
**Phase C3.5(d) — unified embedded-perform shape per ADR 0020 D7: count_performs / find_unique_perform / substitute_perform_with_var walkers; supports binop / struct-lit / fn-call-arg / index / etc. with single embedded perform — complete.**
**Phase C3.5(e) — chained effecting lets via resumer-can-perform per ADR 0020 D7: sentinel_kont_resume returns *mut SentinelKont (bubble-aware); handle becomes a dispatch loop with alloca'd current_kont_slot; compile_effecting_fn_with_chained_lets emits N per-let resumers — complete.**
**Phase C3.6(a) — non-identity return arm per ADR 0020 D4: lower_handle binds return arm value + lowers body in pure_block; HandleContext carries return_arm so k(v)'s pure-unwrap path applies it per Phase B's deep-handler re-wrap — complete.**
**Phase C3.6(b) — nested handles per ADR 0020 D3: handle_depth counter detects nesting; inner handles emit Kont*-typed merge values; switch default propagates un-caught op to outer's dispatch via the merge — complete.**
**Phase C3.7 — handle body lift + phase-go fixtures + ADR 0020 → ACCEPTED-WITH-AMENDMENTS — complete. Phase C3 closes.**
**Phase C4.0 — lexer keywords for classes / traits / delegation / structured concurrency per ADR 0021 D11 — complete.**
**Phase C4.1 (1/N) — class AST + parser per ADR 0022 D1-D4 — complete.**
**Phase C4.1 (2/N) — resolve / types / codegen wiring + postfix `.method(args)` + `Name::init(args)` per ADR 0022 D3+D5+D7+D9 — complete. Definite-assignment via flat any-assigned check (branch-aware merge deferred); ADR 0022 D11 phase-go (Point with manhattan/translate) runs end-to-end at exit 42.**
**Phase C4.1 close — ADR 0022 → ACCEPTED-WITH-AMENDMENTS — complete. Phase C4.1 closes.** Two amendments: A1 D4 definite-assignment is partial (flat any-assigned, branch-aware merge + InitFieldReadBeforeAssign deferred); A2 D8 general `Self` in type position deferred (only positional `self: &Self` via parse_self_param).
**ADR 0023 PROPOSED — concrete C4.2 trait + impl surface — docs-only.** Twelve D-decisions covering trait declarations (D1+D2), default + named impl block grammar (D3+D4), three dispatch paths (D5: receiver-typed + qualified-named + bounded-generic), method-call resolution (D6), `Self` resolution via `Type::TraitSelf(TraitId)` (D7), typing pipeline (D8: two new resolve passes + one new types pass), per-impl codegen with witness tables (D9), out-of-scope list (D10), lexer state recap (D11), and the c42_go_no_go phase-go (D12). Bounded-generic + named-impl pairing deferred at C4.2 minimum per D5/D10 amendment — defaults only for Path 3.
**Phase C4.2 (1/N) — trait + impl AST + parser per ADR 0023 D1+D3+D4 — complete.** Trait declarations + default-and-named impl blocks + `ImplName::method(args)` qualified calls parse end-to-end at AST + parser. Downstream resolve rejects with TraitDeclNotYet / ImplDeclNotYet / QualifiedCallNotYet diagnostics until C4.2 (2/N) lands the impl table + dispatch + codegen. +14 tests (1138 total).
**Phase C4.2 (2/N) — resolve / types / codegen wiring + Path 1 + Path 2 dispatch per ADR 0023 D5+D6+D8+D9 — complete. ADR 0023 → ACCEPTED-WITH-AMENDMENTS.** Trait + impl declarations flow through the full pipeline. Receiver-typed dispatch (`s.write(10)` → default impl when class has no class-method `write`) and qualified-named dispatch (`Doubling::write(&mut s, 16)` → named impl) ship end-to-end. Three amendments at C4.2 close: A1 D5 Path 3 (bounded-generic dispatch) DEFERRED — needs `<W: Writer>` bounded-generic surface; A2 D9 witness-table values not emitted (scaffolding for Path 3); A3 D7 `Type::TraitSelf(TraitId)` interner SHIPPED but unused (params/returns don't reference Self, only positional via self_kind — mirrors C4.1 A2). +16 tests (1154 total). ADR 0023 D12 phase-go (FileSink with default + named Doubling Writer impls) runs at exit 42.
**Phase C4.3 — delegation auto-forwarders per ADR 0021 D6 — complete.** `delegate field: T to Trait;` inside a class body synthesizes a default impl of `Trait` for the class that auto-forwards every trait method to `self.field.method(args)`. Synthesis happens entirely at the resolve layer — Pass 0e extension registers delegate impls alongside user impls (coherence collision = DuplicateDefaultImpl); Pass 3 (resolve_class_decl) adds the synthesized field; new Pass 4.5 synthesizes the per-trait-method auto-forwarder bodies. Types + codegen unchanged. No detail ADR — lands under ADR 0021 D6 + C4.0 D11 lexer reservation. +14 tests (1168 total). c43_go_no_go phase-go (Logger delegates Writer to FileSink; `l.write(42)` returns 42) runs at exit 42.
**ADR 0024 PROPOSED — C4.4 structured concurrency surface + Async effect + runtime scheduler — docs-only.** Twelve D-decisions covering scope/spawn/await grammar (D1-D3), Type::Task interner (D4), Async built-in effect (D5), thread-per-spawn scheduler (D6), 5 new runtime symbols (D7), lowering (D8 + D9), out-of-scope (D10), lexer recap (D11), c44_go_no_go phase-go (D12), 2-sub-iteration split (D13). Key amendment to ADR 0021 D9: direct runtime API rather than async-as-effect (multi-shot continuations from ADR 0020 D2 would be required). User surface identical to the async-as-effect vision.
**Phase C4.4 (1/N) — scope / spawn / await AST + parser per ADR 0024 D1+D2+D3 — complete.** Structured-concurrency surface parses end-to-end at AST + parser. Downstream resolve rejects with ScopeNotYet / SpawnNotYet / AwaitNotYet until C4.4 (2/N) lands the typing + runtime + codegen. +9 tests (1177 total). ADR 0024 stays PROPOSED.
**Phase C4.4 (2/N runtime) — sentinel-runtime symbols per ADR 0024 D6+D7 — complete.** Thread-per-spawn substrate ships: sentinel_task_spawn / sentinel_task_await / sentinel_scope_enter / sentinel_scope_exit / sentinel_scope_register + SentinelTask (32-byte C-stable struct) + SentinelScopeCtx. Uses std::thread internally; real work-stealing scheduler is a deferred follow-on per ADR 0024 D6. Cancellation on early scope exit DEFERRED per ADR 0024 D9. +6 runtime tests (1182 total active workspace). ADR 0024 stays PROPOSED. The typing layer + codegen wiring + c44_go_no_go phase-go land in a follow-on iteration.
**Phase C4.4 (2/N) — types + codegen + phase-go per ADR 0024 D4+D5+D8 — complete. ADR 0024 → ACCEPTED-WITH-AMENDMENTS. Phase C4.4 + Phase C4 close.** The `scope concurrent { spawn fn(args); expr.await }` surface compiles + runs end-to-end. Types: `Type::Task(TaskId)` (tenth interner variant) + `TaskData` + `intern_task` + `TypedProgram.tasks` threaded through check_expr; `TypedExprKind::Scope/Spawn/Await`; spawn validates a Call target returning i64 (Task<i64>-only per D7), await requires a `Type::Task` receiver; 3 TypeErrors (SpawnMustBeCall / SpawnResultMustBeI64 / AwaitOnNonTask). Resolve: scope/spawn/await pass-through (NotYet dropped); built-in `Async` effect auto-registered (appended after user effects — deviation from D5's "EffectId(0)"). Effect-check: spawn/await contribute Async; `scope concurrent` discharges Async (handler-style); spawn/await outside a scope bubbles Async to main → rejected (D5 discipline SHIPPED). Codegen: 5 runtime externs + per-spawn-target wrapper synthesized in a compile_to_object pre-walk (before CodegenCtx — it lacks `&Module`); lower scope/spawn/await; Async-only fns keep the value ABI not the C3 Kont* ABI (`uses_kont_abi` excludes Async). Runtime fix: `_pad` → `owned` flag so explicit `.await` inside a scope is safe against the scope's exit-time auto-await (closes a UAF/double-free in the C4.4 2/N symbols). +1 pass fixture (c44_go_no_go exit 42) + 3 UI fixtures + 2 effect-check tests + 1 runtime test (~1188 total active workspace). Amendments A1 (work-stealing deferred), A2 (cancellation deferred), A3 (Task<i64> + i64 args only), A4 (Async discipline shipped, 2 deviations), A5 (explicit `Task<T>` annotations deferred — use inference). Four-check suite green.
**Phase C4.5 — close-out per ADR 0021 D13+D14 — complete. ADR 0021 → ACCEPTED-WITH-AMENDMENTS. Phase C4 closes.** Combined full-surface phase-go `tests/pass/c4_go_no_go.sentinel` (class + `&mut Self`/`&Self` methods + init + trait + impl + delegation + scope/spawn/await in one program; exit 42) + `tests/pass/c4_named_impl.sentinel` (two named impls of one (trait,type) co-existing via qualified calls; exit 42). The D13 phase-go's `spawn lb.write(42)` (a method call) was adapted to spawn a free fn `buffered_write` that drives the class/delegation surface on the worker thread, since ADR 0024 D2 restricts spawn to a direct fn call (ADR 0021 amendment A2). ADR 0021 amendments: A1 (D9 async-as-effect superseded by ADR 0024's direct-runtime API — surface identical, lowering differs), A2 (D13 phase-go adapted), A3 (per-sub-phase amendments roll-up), A4 (D14 estimate beaten), D10/D12 out-of-scope confirmed (actors → C5). +2 driver pass-tests (123 driver pass; ~1191 total active workspace). Docs-only sub-phase. Four-check suite green. **Next: Phase C5 → Sentinel 1.0** (broker integration + constant-time secret codegen + cross-process + actors + stable ABI + reproducible builds + tooling per HANDOVER §6.2). **ADR 0025 PROPOSED drafted** (Phase C5 kickoff / productionization plan — 14 D-decisions, 8-sub-phase split; resume at C5.0).
**Phase C5.0 — go/no-go decision + test infra (D11) + reproducible-build audit (D8) — complete.** Per ADR 0025: the 1.0 go/no-go program is a single-process, single-file TLS 1.3 handshake (D1/D13), resolving D6 (cross-process) + D9 (modules) both → post-1.0. D11: `cargo nextest` adopted (`.config/nextest.toml`) + the 15 driver UI rejections migrated from `stderr.contains(code)` to `insta` blessed full-diagnostic snapshots (`crates/sentinel-driver/tests/ui.rs`, portable via relative-path snc invocation). D8: the reproducible-build audit found the C0–C4 build already byte-identical across independent `snc` processes (codegen's std `HashMap`s are lookup-only; emission walks source-ordered `Vec`s; mach-O has no timestamp), locked in by `crates/sentinel-driver/tests/repro.rs` (compile-twice + diff). 3 commits (`3908cf6` decision docs + `a217707` D11 feat + `5fe7fd3` D8 feat); four-check green via `cargo nextest run --workspace` (1195 tests) + `cargo test --doc`.
**ADR 0026 PROPOSED — C5.1/C5.2 HIR/MIR pipeline + constant-time secret codegen — docs-only.** Ten D-decisions: HIR desugar stage (`hir_query` — dispatch-resolved + monomorphic + drops-explicit + secret-preserved; D1), minimal SSA MIR (`mir_query`; D2), codegen re-targets `TypedProgram`→HIR with MIR as the analysis substrate + a documented escape hatch if the re-target over-runs (D3), constant-time secret emission (branch-free select + ADR 0008 speculation barriers; x86-64/aarch64; D4), the MIR constant-time verification pass (taint-track secrets in SSA, `sentinel::mir::secret_leak` diagnostic; D5), secret taint representation (D7), out-of-scope (D8: full opt suite, codegen-consumes-MIR SSA lowering, oblivious secret indexing), phase-go (D9: `c51` behaviour-preservation across the whole pass suite + `c52_secret_ct` + `c52_secret_leak`), 4-sub-phase split C5.1a→C5.2b (D10).
**Phase C5.1a (1/N) — HIR pipeline seam introduced; ADR 0026 D3 escape hatch INVOKED — complete.** `sentinel-hir` is now a real stage: a pure `lower_to_hir(&TypedProgram, &DropPlan) -> HirProgram` the driver calls after borrow-check, with `compile_to_object` consuming `&HirProgram` (a thin borrowing bundle of the typed program + drop plan at this increment). Behaviour-preserving by construction — all 1195 tests pass + every `tests/repro.rs` object byte-identical (`cdbc483`). The **D3 escape hatch was then INVOKED** (decided with the developer): codegen couples to the typed tree at ~295 `TypedExprKind` / 342 `TypedExpr` refs across 90 signatures, so a *thick*-HIR migration is a multi-session high-risk rewrite not required for the 1.0 constant-time-`secret` capability. Codegen STAYS on the typed program (via the seam, `HirProgram::program()`); the thick HIR desugar (dispatch/mono/explicit-drops) + the codegen-consumes-HIR migration are **post-1.0** (still Phase-D-valuable); **C5.1a closes at the seam**. Next: **C5.1b** — `sentinel-mir` + `mir_query`, an SSA/CFG lowered from the typed program (via the seam) for the C5.2 D5 constant-time verification; then C5.2 constant-time emission (D4, a codegen pass) + verification (D5).
**Phase C5.1b (1/N) — MIR data model (minimal SSA/CFG) — complete.** `sentinel-mir` is no longer a stub: `MirProgram` / `MirFunction` / `MirBlock` (SSA block-params = the phi-equivalent) / `MirInst` / `MirOp` / `MirTerminator` / `MirValue`, built to host the C5.2 D5 constant-time verification. Each SSA value carries its `Type`, so secrecy reads off `Type::Secret(_)` (`MirFunction::is_secret` — the taint seed); the three D5 sinks are representable (a `Branch` condition, a `Load` index, a `Binary` Div/Rem operand); non-secret-relevant constructs funnel through `MirOp::Opaque` / `MirTerminator::Unreachable` carrying their operands so taint stays sound. Additive — nothing consumes MIR yet (codegen stays on the typed program per the escape hatch); zero regression risk; 1195 tests green (`1b0a10d`). Next: **C5.1b (2/N)** — `lower_to_mir` (typed fn bodies → MIR SSA); then C5.2 = D5 verification + D4 constant-time emission (codegen pass).
**Phase C5.1b (2/N) — `lower_to_mir`: typed fn bodies → MIR SSA — complete.** `sentinel-mir` now lowers each free function's type-checked body into a `MirFunction` in SSA/CFG form. No loops in the surface ⇒ the CFG is a DAG and SSA falls out of one structured walk (no dominance-frontier phi placement): `if`/`&&`/`||` → `MirTerminator::Branch` into fresh blocks reconciled at a merge block via SSA block-params, with a variable reassigned on one arm threaded through a merge param (deterministic, `VarId`-sorted `BTreeMap` env). `&&`/`||` lower as control flow because `secret bool && secret bool` type-checks (`SecretBranch` only rejects `if`) — a short-circuit branch on a secret is the leak the C5.2 D5 pass must see. The three D5 sinks lower precisely (`if`/short-circuit cond → `Branch`; `a[i]`/`*p` → `MirOp::Load` so a secret index *or* address is visible; `a / b` → `Binary(Div)`); `declassify(e)` → `MirOp::Declassify` (the one taint sink); everything else → `MirOp::Opaque` carrying its operands so taint can't vanish. Scope: top-level fns only (class/impl/init method bodies a mechanical follow-on); generic defs as-is (`TypeParam` is never secret); no monomorphisation (MIR is analysis-only per the D3 escape hatch). Additive — nothing consumes MIR yet (the D5 pass at C5.2 is its first consumer; the driver will call `lower_to_mir(hir.program())` then); zero regression risk; codegen stays on the typed program. +7 lowering tests (1202 total) (`1a223c8`). ADR 0026 stays PROPOSED (flips at C5.2 close). Next: **C5.2** — D5 constant-time verification (`sentinel::mir::secret_leak`) + D4 constant-time emission (codegen pass) — the 1.0 headline.
**Phase C5.2b (1/N) — the D5 constant-time verification pass — complete.** `verify_constant_time(&MirProgram) -> Vec<SecretLeak>` is the first consumer of `lower_to_mir` and the machine-checkable form of ADR 0008's guarantee: it rejects any `secret` value reaching a conditional-branch condition, a load index/address, or a division divisor, emitting a `sentinel::mir::secret_leak` what/why/how diagnostic (`SinkKind` names the sink). **Taint oracle:** each SSA value carries its `Type`, and the type checker's operator-secret-preserving rules already computed the taint fixpoint (`declassify` clears; fn-signature boundaries respected), so the pass reads taint off the type (`is_secret`) and inspects each sink — no separate def-use propagation at the typed-program level (the ADR's forward propagation is only needed once MIR is lowered from *post-optimisation* code → post-1.0; recorded as a **D5 amendment**). The one leak this catches beyond the C3.1 source rejections is `secret bool && secret bool` (a secret short-circuit `Branch`), which type-checks because `SecretBranch` only rejects `if`. MIR data model gains a `span` on `MirInst` + `MirTerminator::Branch`, threaded in `lower_to_mir`, so the diagnostic points at source. Additive — not yet wired into the driver (C5.2b (2/N)); zero regression risk. +4 verify tests (1206 total) (`9bcc271`). ADR 0026 stays PROPOSED. Next: **C5.2b (2/N)** — wire D5 into the driver (a real `secret_leak` compile error) + `c52_secret_leak` (UI snapshot) + `c52_secret_ct` (branch-free masked-select pass) fixtures.
**Phase C5.2b (2/N) — the D5 verification is wired into `snc` + c52 phase-go — complete.** `snc build` runs the constant-time check: after `check_query` the driver lowers the typed program to MIR (`lower_to_mir` — now a real pipeline consumer) and runs `verify_constant_time`; a `secret` reaching a conditional branch / load index|address / division divisor is a `sentinel::mir::secret_leak` compile error (exit 1) gating codegen. (Codegen still consumes the typed program via the HIR seam per the D3 escape hatch; MIR stays analysis-only.) Fixtures (ADR 0026 D9): `c52_secret_leak` (UI) — `secret bool && secret bool` type-checks (`SecretBranch` only rejects `if`) but lowers to a secret short-circuit `Branch` → rejected (`insta` snapshot, label on the short-circuited operand); `c52_secret_ct` (pass) — a branch-free masked select over secrets (`c*a + (1-c)*b`) compiles, runs, **passes** D5, exit 42. **c51 bar holds**: existing pass/ui fixtures unchanged + `tests/repro.rs` byte-identical (D5 runs before, and gates, an unchanged codegen). +2 tests (1208 total) (`e81bdbf`). ADR 0026 stays PROPOSED. Next: **C5.2a** — D4 constant-time *emission* (codegen pass: branch-free select + ADR 0008 speculation barriers, x86-64/aarch64); **open question: does the 1.0 go/no-go even need D4?** A branch-free *arithmetic* primitive already passes D5 on the existing codegen (no bitwise/`select` ops in the surface), so D4 may be scoped out of 1.0 — settle with the developer before building it. ADR 0026 flips once C5.2a lands or is consciously scoped out.
**ADR 0027 PROPOSED — bitwise operators (`& | ^`, then `<< >> ~`) — docs-only.** Decision (with the developer): do bitwise operators next, **deferring C5.2a/D4** — the go/no-go's constant-time `Finished` MAC verify is an XOR-accumulate compare that needs `^`/`|`, and the surface has none (`BinOp` = only Add/Sub/Mul/Div). Ten D-decisions: target `& | ^ << >> ~` in two waves (D1) — **C5.3 = `& | ^`** (token-clean: new `Pipe`/`Caret`, reuse `Amp` as infix bit-and with prefix `&` still borrow — D2; Rust precedence `&`>`^`>`|` between cmp and add, ladder gains parse_bitor/bitxor/bitand — D3); extend `BinOp` (no new ExprKind/TypedExprKind/MirOp variants — D4); secret-preserving integer-only typing mirroring C3.1b arithmetic, **no new SecretXxx rejection** (bitwise is constant-time, the *sanctioned* secret computation — D5); LLVM and/or/xor codegen (D6); **MIR + D5 need no change** (the `Binary` arm is op-generic; bitwise ops are non-sinks — D7); **C5.4 = `<< >> ~`** with the `>>`-vs-nested-generic-close split (Rust-style: the type-arg parser splits `Shr` into two `>` — D9). Out of scope (D8): shifts/complement at C5.3, rotate, bitwise-on-bool, compound-assign, `[secret T]` arrays (the flat ArrayElem subset has no Secret variant — a separate deferred surface). Phase-go (D10): `c53_bitwise` + `c53_ct_eq` (a real XOR-accumulate constant-time equality over scalar secrets that passes D5 — upgrading the C5.2b faked masked-select). Next: **C5.3 (1/N)** — lexer (`Pipe` + `Caret` tokens).
**Phase C5.3 (1/N) — lexer: bitwise `|` (`Pipe`) + `^` (`Caret`) tokens — complete.** First wave of the bitwise surface per ADR 0027 D2: two new logos tokens; longest-match keeps `||` → `PipePipe`; the infix bitwise-and **reuses** `&` (`Amp`), to be disambiguated from the borrow prefix by parser position at 2/N. No `<<`/`>>` (C5.4 — the `>>`/nested-generic-close split). +4 lexer tests (longest-match `|`/`||` + `&`/`&&` regressions + packed bitwise); additive (parser doesn't consume them yet); four-check green (1212) (`b3d1d48`). Next: **C5.3 (2/N)** — `& | ^` end-to-end: extend `BinOp` (Display + all exhaustive matches in one pass); parser precedence ladder gains `parse_bitor`/`parse_bitxor`/`parse_bitand` between `parse_cmp` and `parse_add`; secret-preserving integer typing mirroring C3.1b (no new TypeError/SecretXxx); codegen and/or/xor; MIR+D5 unchanged (Binary arm op-generic, bitwise non-sink) + `c53_bitwise`/`c53_ct_eq` fixtures; ADR 0027 flip.
**Phase C5.3 (2/N) — bitwise `& | ^` surface end-to-end. ADR 0027 → ACCEPTED-WITH-AMENDMENTS.** The operators compile + run. Surface was small because the `Binary` pipeline is op-generic (resolve passes `BinOp` through; types' Binary handler op-agnostic except the `Div`→`SecretDivisor` check; `lower_to_mir`/D5 handle `Binary` generically) — so only AST + parser + codegen changed. AST: `BinOp` += `BitAnd`/`BitOr`/`BitXor` (+ symbol(); no new ExprKind/TypedExprKind/MirOp). Parser: levels `parse_bitor`→`parse_bitxor`→`parse_bitand` between cmp and add (`&`>`^`>`|`); infix `&` = bit-and, prefix `&` = borrow (positional). Types: NO change — inherits C3.1b secret-preserving integer rule (mixed secret/public → Mismatch; bool → Mismatch); **no new SecretXxx** (bitwise is constant-time, the sanctioned secret computation). Codegen: LLVM and/or/xor. MIR+D5: unchanged (bitwise non-sink). Fixtures: `c53_bitwise` (`5 & 6 ^ 3 | 8`==15) + `c53_ct_eq` (constant-time equality over secrets — XOR-accumulate+OR-reduce+declassify — compiles, runs, passes D5; the go/no-go MAC-verify shape). +9 tests (1221 total) (`76bfea3`). **ADR 0027 amendment A1:** `<< >> ~` (C5.4) deferred — the constant-time compare needs only `^`/`|`; shifts (with the `>>`/generic-close split) are a follow-on if the go/no-go computes hashes in-language. Next: **developer-scope call** — C5.4 shifts, OR begin assembling the TLS go/no-go (constant-time compare now writable), OR another C5 productionization sub-phase.
**ADR 0028 PROPOSED — broker integration (D4) — docs-only.** Developer chose broker integration as the next productionization sub-phase (**C5.4**). Finding that shaped it: the Phase A broker is an *arena* allocator (bump = bulk-free / `free` unimplemented; slab = fixed-size slots; typed `Handle<T>`) that does **not** fit a drop-in `sentinel_alloc` (arbitrary-size, individual-free, raw `*u8`); and there's no secret heap data yet (`[secret T]` unrepresentable) so the secret-memory policy is scaffold. ADR 0028's design (10 D-decisions): map Sentinel scopes → broker bump arenas so individual free becomes scope-exit **bulk** free (D1/D2), reusing the borrow-check **`DropPlan`** (moved=escapes vs dropped-at-scope-exit=safe-to-arena) so **no new escape analysis** is needed (D3); ship a **runtime-only foundation first** (C5.4 (1/N), D4): process-wide `Broker` backing `sentinel_alloc`/`free` via a size-classed slab pool + ptr→handle registry — **c51-safe because codegen is untouched** (objects byte-identical) — unlocking budgets/recording/stats; then the **scope→arena codegen** (C5.4 (2/N), D3, may defer post-1.0). Budgets = the go/no-go hook (`within_budget`; a `scope budget(N)` surface deferred — D5). Secret-policy scaffold (D6). Numbering: ADR 0025 D14's "0027 = broker" superseded (0027 = bitwise; broker = **0028**); the bitwise *shift* wave (ADR 0027 A1) is unnumbered-deferred, not C5.4. Next: **C5.4 (1/N)** — the runtime-only broker foundation.
**Phase C5.4 (1/N) — broker-arena substrate (ADR 0028) — complete.** The Phase A broker backs a scope-arena C-ABI in the runtime. Finding: the broker is a safe *handle* allocator (bump bulk-frees, `free` unimplemented; slab fixed-size) with no public raw pointer, so a drop-in `sentinel_alloc` doesn't fit — added a public raw-bytes API (`Arena::alloc_bytes`/`ArenaHandle::alloc_bytes` → `NonNull<u8>`, exposing the strategy's `alloc_raw`). Runtime: a process-wide lazy `Broker` + `sentinel_arena_enter` (create bump arena) / `sentinel_arena_alloc` (16-byte-aligned bump alloc) / `sentinel_arena_exit` (`destroy_arena` + drop → `BumpStrategy::drop` frees the backing buffer). **Additive + c51-safe**: codegen still emits `sentinel_alloc`/`sentinel_free` (libc) and does NOT call the arena fns yet → objects byte-identical. (The ADR's "runtime-only" was slightly off — a small broker API addition was needed — and the malloc-replacement framing was set aside: the broker is arena-, not malloc-, shaped.) +5 tests (1226 total) (`b49a5ef`). ADR 0028 stays PROPOSED. Next: **C5.4 (2/N)** — the scope→arena codegen: route a scope's non-escaping heap allocations (those the borrow-check `DropPlan` frees at that scope exit, hence provably non-escaping) into the scope arena, replacing N per-binding `sentinel_free`s with one `sentinel_arena_exit`. The careful (UAF-sensitive) part — do it fresh; full escape analysis stays post-1.0 (ADR 0026 D2).
**Phase C5.4 (2/N) — the scope→arena codegen; ADR 0028 → ACCEPTED-WITH-AMENDMENTS. Phase C5.4 closes.** Codegen routes a scope's **non-escaping** primitive array-literal heap buffers into a broker bump arena (`sentinel_arena_enter`/`_alloc`) and replaces that scope's per-binding `sentinel_free`s with **one** `sentinel_arena_exit` at scope exit. A program-wide `compute_arena_routed` pre-pass produces a `HashSet<VarId>` = *exactly* the bindings `emit_scope_drops` frees (`∉ moved ∧ ≠ tail_returned_var(&block.tail)`), restricted to `let x = [i64/i32/bool array literal]` in **non-generic, non-effecting fns**; that **one set drives both** the alloc-routing (`lower_stmt`→`lower_array_lit`) and the free-skip (`emit_scope_drops`), so they cannot diverge. The per-scope arena handle lives in a new `ScopeFrame` (replacing the bare `Vec<VarId>`; a `push` method keeps the 12 push sites unchanged), created **lazily** on first routed alloc → scopes routing nothing stay byte-identical. **Airtight argument:** the routed set is a strict *subset* of the proven-non-escaping free set, so routing is as safe as today's free (same bindings, lifetime, point; bulk vs individual). **Verified by reasoning + disassembly** (routed scopes emit `arena_enter`/`_alloc`/`_exit` + **zero** `sentinel_free`/`_alloc`; moved/returned arrays stay on libc — a single negative case `c24_moved_array_no_double_free` confirms) + the c24/c25 array-RAII guards. **Amendment A2 — ADR 0028's "verified UAF hole" was wrong about the mechanism:** a tail-returned array (`fn make() -> [i64] { let a=[1,2,3]; a }`) **IS** in `moved_sources` (the borrow checker walks the tail `Var` as a *consuming move* before snapshotting the `DropPlan`; empirically dumped), so `∉ moved` alone already excludes returned arrays — the `tail_returned` half is belt-and-suspenders for heap types, kept anyway to mirror `emit_scope_drops` exactly. +1 fixture (`c54_scope_arena` — body-scope + nested-block arenas, exit 42); 1227 tests (`8e7b38f`). Four-check green. Deferred (post-1.0): per-scope arena *sizing* (capacity 0 → runtime 1 MiB default), routing in methods/generics/effecting fns, non-primitive-element arrays, `scope budget(N)` surface, full escape analysis (ADR 0026 D2). **Next: developer-scope call** — assemble the TLS go/no-go (constant-time compare + scope arenas now both in hand), or another C5 productionization sub-phase (stable ABI ADR 0025 D7 / LSP D10).
**ADR 0029 PROPOSED — stable ABI (D7) — docs-only.** Recommended + drafted as the next C5 sub-phase (no codegen hazard; a prerequisite for the go/no-go's runtime↔codegen link + Phase D self-hosting). Ten D-decisions: define + **document** (`docs/abi-v1.md`) + **freeze at `abi-v1`** + **test** the ABI — calling convention (D3: C ABI / SysV+AAPCS64; main→i32; ordinary fns by value; effecting fns→`*SentinelKont`; class init→out_ptr), the `Type`→LLVM **layout catalog** (D4: structs field-order, `[T]`=`{i64,ptr}`, `?primitive`=`{i1,T}`, `?Struct`=`{i1,ptr}`, ref/kont/task=opaque ptr, `secret T`≡`T`), **name mangling** (D5: `base__<tag>`, `arr_`/`opt_`/`ref_`/`sec_`, `Name__init`/`Name__method`/`Name__Type__Trait__method`), the **runtime-symbol contract** (D6: the ~18 `sentinel_*` + the `#[repr(C)]` `SentinelKont`/`Frame`/`Task`/`ScopeCtx` layouts), a **layout-stability test suite** (D7: extend the size/align asserts + DataLayout `Type`-layout asserts + mangling/symbol golden tests — the enforcement, drift→red test), versioning (D8: `abi-v1` freeze; reproducible builds folded in, already guarded by `repro.rs`; mangling length-prefixing is the one `abi-v2` soft-spot). **No emitted bytes change** → c51 bar holds by construction. Out of scope (D9): the separate-compilation linker/module surface (ADR 0025 D9, post-1.0), cross-arch beyond x86-64/aarch64, FFI header gen. Sub-phase split D7 (1/N) spec+struct/mangling/symbol tests, (2/N) `Type`-layout DataLayout asserts + flip. Next: **D7 (1/N)**.
**Phase C5 D7 (1/N) — the stable-ABI spec + layout-stability tests (ADR 0029) — complete.** `docs/abi-v1.md` documents + **freezes** the ABI codegen already emits (calling convention, the `Type`→LLVM layout catalog, the `#[repr(C)]` runtime struct layouts, name mangling, the ~18 `sentinel_*` runtime-symbol contract), each cross-linked to its bootstrap source. Stability tests pin it so a drift turns a test red rather than silently miscompiling: `abi_v1_struct_layouts_are_stable` (size/align + `offset_of!` for `SentinelKont`/`Frame`/`Task`/`ScopeCtx`) + `abi_v1_runtime_symbol_set` (addresses of all 18 runtime symbols → rename/removal is a compile error) in sentinel-runtime; `abi_v1_mangling_is_stable` (golden strings for `mangle_type`/`mangle_mono_name`) in sentinel-codegen. **No emitted bytes change** (documents/tests existing behaviour) → c51 bar + `repro.rs` hold by construction; reproducible builds (D8) fold in. +3 tests (1230) (`0304a9c`). **ADR 0029 stays PROPOSED** (flip is at 2/N). Next: **D7 (2/N)** — the `Type`-layout DataLayout assertions (query the lowered LLVM type's size/align/field-offsets through the target `DataLayout`, assert the `abi-v1` values) + a negative "drift turns it red" check + the ADR flip. (May then move to LSP D10 or the TLS go/no-go.)
**Phase C5 D7 (2/N) — `Type`-layout DataLayout assertions; ADR 0029 → ACCEPTED-WITH-AMENDMENTS. Phase C5 D7 (stable ABI) closes.** `abi_v1_type_layouts_via_datalayout` (sentinel-codegen) lowers each `Type` via the real `llvm_basic_type` and asserts its size / alignment / struct-field offsets **and field types** through the target `DataLayout` (same target setup as `compile_to_object`) — the concrete `abi-v1` §2 byte layouts (i1=1, i32=4, i64=8, `[T]`/`?T`=16 with the inner at offset 8, ptr=8). **Amendment A1:** field-type asserts were added beyond the ADR's "size+offsets" wording — equal-sized fields' *order* (e.g. `[T]`'s `{i64 len, ptr data}`) isn't pinned by offsets alone; the negative check (deliberately reordering the array fields) was verified to turn the test **red**, then reverted. **A2:** `Struct`/`Class`/`GenericInstance` layouts (which need codegen's pass-0 `struct_types` cache to lower a real `Type::Struct(id)`) are pinned via a representative `{i64,i64}` struct built as codegen builds user structs (`struct_type(fields, false)`); the cache-free arms (scalars, `[T]`, `?T`, ref/kont/task) go through `llvm_basic_type` directly. No emitted bytes change → c51 bar + `repro.rs` hold. +1 test (1231). Four-check green. **Next: developer-scope call** — LSP (ADR 0025 D10) or assemble the TLS 1.3 go/no-go (D13): both 1.0 headline capabilities (constant-time `secret` compare + broker scope arenas) **and** a frozen `abi-v1` are now in hand. Deferred (post-1.0, ADR 0029 D9): the separate-compilation linker/module surface, cross-arch beyond x86-64/aarch64, a length-prefixed mangling scheme (`abi-v2`).
**ADR 0030 PROPOSED — the 1.0 go/no-go: a TLS-1.3-handshake-shaped program (D13) — docs-only.** Opened after a **readiness/scoping pass** against the current surface, which found the go/no-go is an *assembly of already-proven patterns* (constant-time `Finished` verify = `c53_ct_eq`; state machine + trait + delegation = `c4_go_no_go`; I/O-as-effects = `c37_go_no_go`; bounded iteration = recursion, verified working) + deliberate modelling choices, **not** a dependency on big new machinery. Nine D-decisions: the close-bar goal (D1: single-process single-file handshake — accept → ECDHE → HKDF → `Finished` verify — that compiles + runs + **passes D5 constant-time verification**; closing it = **Sentinel 1.0**); reduced handshake-shaped crypto over `secret` scalars at fixed sizes (D2: a Montgomery-ladder *step*, an HKDF-`expand`-shaped fixed mix, the `c53_ct_eq` compare — not real AES/X25519/SHA-256, which are ecosystem per §15.3); **D3 — descope the connection actor** (a *deviation* from C5.0 D5: a sequential single-process handshake needs no mailbox → drops the largest remaining language-design sub-phase off the 1.0 path; actors → post-1.0; **developer's call**); modelling choices (D4: bytes/labels→`i64`, secret material→`secret` scalars, iteration→recursion — no new surface); **shifts `<< >> ~` as a conditional JIT prerequisite** (D5: land ADR 0027 A1 only *iff* a reduced primitive needs them; the chosen shapes use only `+ - * & | ^`); the constant-time bar (D6: must pass `verify_constant_time` — the decisive 1.0 validation); out of scope (D7: real crypto, `u8`/byte type + string literals, actors, modules, cross-process, loops); phase-go (D8: `c5_go_no_go` fixture green + D5-clean → flip ADR 0025 → ACCEPTED); 3-sub split (D9: skeleton → CT primitives → close). Gaps found but non-blocking for the reduced program: no loops (recursion substitutes), no bytes/strings (model as `i64`), no shifts/`%` (reduced primitives avoid; `%` isn't even lexed), no `[secret T]` arrays (secret scalars). Next: **go/no-go (1/N)** — the skeleton (state machine + cipher trait + `Net`/error effects + 4-stage flow, stubbed crypto; compiles + runs).
**Phase C5 go/no-go (1/N) — the TLS-handshake-shaped skeleton (ADR 0030) — complete.** `tests/pass/c5_go_no_go.sentinel` composes the full surface the go/no-go needs in one single-file program — a handshake state-machine `class` (`&mut Self` `ecdhe` method + init) + a `Kdf` cipher-suite `trait`/`impl` (receiver-typed `suite.derive` dispatch) + a `Net` I/O `effect` + `handle … with` + the 4-stage flow (accept/recv → ECDHE → HKDF → `Finished`) — with **stubbed** crypto, and runs end-to-end to **exit 42** (handler resumes recv→5; 5*9=45; 45+3=48; finished_diff(48,48)=0; 42-0=42). **It compiled on the first try** — empirical confirmation of ADR 0030's scoping verdict (the go/no-go is an assembly of proven patterns, not new machinery). +1 test (1232) (`9e2ef6a`). **ADR 0030 stays PROPOSED.** Next: **go/no-go (2/N)** — fill the constant-time primitives over `secret` scalars (a Montgomery-ladder step + cswap, an HKDF-`expand`-shaped fixed mix, the `c53_ct_eq` `Finished` verify) and make the program **pass the D5 constant-time check** (`verify_constant_time`) — the decisive 1.0 validation; land ADR 0027 A1 (shifts) first *iff* a primitive needs it. Then (3/N): close → **declare Sentinel 1.0** + flip ADR 0025 → ACCEPTED.
**Phase C5 go/no-go (2/N) — constant-time crypto over secrets; the close bar is MET (ADR 0030 D8) — complete.** `tests/pass/c5_go_no_go.sentinel` now does real **constant-time** crypto over `secret` scalars: a Montgomery-ladder step + branch-free `cswap` (`mask = sec(0) - bit`), an HKDF-`expand`-shaped mix via the `Kdf` trait, and the `c53_ct_eq` `Finished` verify (XOR-accumulate + `declassify`). It **passes the D5 constant-time check** (`verify_constant_time` gates `snc build`) and runs to exit 42 — the headline 1.0 capability (express + *prove* constant-time crypto) exercised end-to-end. Constant-time by construction: every secret op is `+ - * ^ & |` (no D5 sink), the lone `declassify` is the `Finished` accumulator, no secret reaches a branch/index/divisor (verified the secret typing is live — a deliberate secret array index is rejected at type-check). **Ergonomic finding:** C3.1b makes a mixed secret/public op a type error (no in-expression widening), so constant-time code lifts public constants/labels into the secret domain first — here via a `sec(x) { let s: secret i64 = x; s }` helper (widening happens only at a `let` with a `secret` annotation, not at a return). 1232 tests (`e5a40e9`). **ADR 0030 stays PROPOSED.** Next: **go/no-go (3/N) — the formal 1.0 declaration** (flip ADR 0025 → ACCEPTED + declare Sentinel 1.0). **That milestone call is intentionally left to the developer**; the substantive close bar (program runs + passes D5) is met.
**🎉 SENTINEL 1.0 (2026-05-30) — go/no-go (3/N); Phase C5 + Phase C close.** The developer declared 1.0: **ADR 0025 (Phase C5 kickoff) + ADR 0030 (go/no-go) → ACCEPTED-WITH-AMENDMENTS.** The close bar was met (the constant-time TLS-handshake go/no-go runs + passes D5). The 1.0 language = full types + witness-table generics + borrow check + RAII + `secret`/effect typing + handler runtime + classes/traits/delegation + structured concurrency + **machine-verified constant-time `secret`** + bitwise `& | ^` + broker scope arenas + a frozen `abi-v1`; single-process, single-file, loop-free-by-design. 1232 tests, four-check green. Scoped out of 1.0 (analysed follow-ons): constant-time *emission* (ADR 0026 D4), shifts `<< >> ~` (ADR 0027 A1), actors (ADR 0030 D3), LSP (ADR 0025 D10), `[secret T]` arrays, modules, cross-process, a `u8` type, loops, full escape analysis. **Next: Phase D — self-hosting (ADR 0031 PROPOSED).** ⚠ Self-hosting is a *major* multi-stage effort: the 1.0 language has no strings / file I/O / growable collections / modules, all of which a compiler-in-Sentinel needs, so **Phase D opens with a language + stdlib build-out, NOT lexer-in-Sentinel** — see ADR 0031's honest readiness assessment + staged path.
**ADR 0031 PROPOSED — Phase D kickoff: self-hosting — docs-only.** Opens the project's largest phase. **Honest readiness verdict:** the 1.0 language *cannot* self-host yet — verified gaps (none at 1.0): no sum types / `match` (an AST is a sum type — the biggest blocker), no strings / `char` / byte type (a compiler is text processing), no growable collections (`Vec`/`Map` — only fixed `[T]`), no file I/O (only `sentinel_print`), no modules/multi-file, no loops (recursion-only). **Strategy (D2):** language+stdlib build-out FIRST, then incremental self-host, keeping the Rust `snc` as the **reference oracle** (every Sentinel-written stage differentially validated against it on the fixture corpus), converging on a **bootstrap fixed-point** (the Sentinel compiler compiles itself byte-identically — which is *why* C5 shipped `abi-v1` + reproducible builds). **Prerequisite roadmap (D4, each its own ADR):** sum types + `match` → strings + byte type → growable collections → file I/O (stdlib) → modules → loops; also retires the thick-HIR/MIR migration + full escape analysis + shifts. **Self-host sequence (D5):** lexer → parser → resolve → types → HIR/MIR → codegen, in Sentinel, each matching the Rust stage before replacing it. **First sub-phase D.1 = sum types + pattern matching** (its own PROPOSED ADR next — the foundational AST-enabler; a C1–C4-style type-system + codegen feature). No timeline promise; Phase D is plausibly the longest phase. Next: **Phase D.1 (sum types + `match`)** — write its kickoff ADR, then implement.
**ADR 0032 PROPOSED + Phase D.1 (1/N) — sum types + pattern matching: the kickoff + lexer.** ADR 0032 designs `enum`/`match` end-to-end (12 D-decisions): surface (`enum Name { V, V(T), V(T,T) }` + `Name::Variant(args)` + exhaustive `match s { Pat => e }`); `Type::Enum` interner variant; the abi-v1 layout **`{ i32 tag, ptr payload }`** heap-boxed (necessary for *recursive* enums — the AST case — reusing the `?Struct` rationale + drop path); `match` → LLVM `switch`; RAII payload drop; the constant-time guard (a secret-tagged match is a branch sink; no `secret enum` at MVP); generic enums (`Option`/`Result`) a fast-follow via mono (D9); MVP out-of-scope (named-field variants, or-/nested patterns, guards). **D.1 (1/N) ships the lexer:** two new logos keyword tokens (`enum`, `match`); `=>`/`::`/`_`-as-`Ident` already exist, so the lexer surface is complete. Additive (parser consumes at 2/N). +3 lexer tests (1235) (`87e955c`). Four-check green. Next: **D.1 (2/N)** — AST + parser (`EnumDecl`, `ExprKind::Match`, `Pattern`; `parse_enum_decl`/`parse_match`).
**Phase D.1 (2/N) — enum + match AST + parser; resolve rejects (NotYet) — complete.** Per ADR 0032 D8. **AST** (sentinel-ast): `EnumDecl` + `VariantDecl` (unit + positional-tuple-payload variants) on `Program.enums`; `ExprKind::Match { scrutinee, arms }` + `MatchArm` + `Pattern` (qualified `Enum::Variant(binds)` + `_` wildcard); the s-expr `Display` covers both. **Parser** (sentinel-syntax): top-level `enum` dispatch + `parse_enum_decl`/`parse_variant`; `match` dispatched in `parse_expr` alongside `if`, with `parse_match_expr`/`parse_match_arm`/`parse_pattern`/`parse_pattern_binding` (scrutinee forbids struct literals like the if-cond; arms comma-separated; `=>`/`::`/`_` reuse existing tokens). **Additive**: resolve rejects non-empty `enums` (`EnumDeclNotYet`) + `match` (`MatchNotYet`) until 3/N; **blast radius contained to ast+syntax+resolve** (downstream crates match the resolved/typed parallel trees, which gain no `Match` variant — confirmed by build). +7 tests (1242) (`e368a72`). Four-check green. Next: **D.1 (3/N) — resolve + types**: the `Type::Enum` interner variant (+ `EnumData`/`VariantData`), variant construction + `match` type-check + **exhaustiveness** (NonExhaustiveMatch / UnknownVariant); then (4/N) codegen (`{tag,ptr}` layout + `switch` + drop + abi-v1 entry + `c5d1_enum`), (D.1b) generic enums.
**Phase D.1 (3/N) — enum + match the type layer; `enum`/`match` now TYPE-CHECK end to end (codegen rejects until 4/N) — complete.** Per ADR 0032 D8 (the resolve + types slice). **resolve**: `EnumId` + `ResolvedEnumDecl`/`ResolvedVariantDecl` on `ResolvedProgram.enums`; a Pass-0 enum table (names share the type namespace with structs/classes/traits → `RedefinedEnum`; in-enum dup → `DuplicateVariant`); the `Name::Variant(args)`/`Name::Variant()` construction is **disambiguated** from `ImplName::method` / `Class::init` *at resolve* (the AST parses all three as `QualifiedCall`/`ClassInit`; when the leading name is an enum → `ResolvedExprKind::EnumConstruct`); `match` → `ResolvedExprKind::Match` + `ResolvedPattern`/`ResolvedMatchArm` with per-arm payload-binding `VarId`s scoped into the arm body only (snapshot/restore of `vars`, mirroring `resolve_handle_expr`; `_` slots get a VarId but stay out of scope; same-name twice → `DuplicatePatternBinding`); the `EnumDeclNotYet`/`MatchNotYet` rejections are dropped. `ImplCtx` grew an `enum_table` field (Copy bundle — no per-fn signature churn). **types**: `Type::Enum(EnumId)` (the 11th interner-style `Copy`-`Type` variant) + `EnumData`/`VariantData` + `TypedProgram.enums` + `enum_data` accessor; the `Type::Enum` cascade got real-or-rejecting arms across every exhaustive `Type` match (`type_display`/`Display`/`to_nullable_inner`/`to_array_elem`/`substitute`/`try_substitute`/`contains_type_param`); enum names resolve in **type position** (`resolve_type_expr` precedence struct→class→enum→primitive — threaded a new `enum_table` param to its ~20 call sites); `EnumConstruct` type-checks (variant→index, payload arity + per-arg coercion → `Type::Enum`); `match` type-checks (scrutinee is `Type::Enum`; arm bodies checked with `expected` pushed down + unified; pattern bindings typed from variant payloads + bound in env with save/restore; **exhaustiveness** = every variant covered or a `_`). Five new `TypeError`s: `UnknownVariant`, `VariantPayloadArityMismatch`, `MatchScrutineeNotEnum`, `NonExhaustiveMatch`, `MatchArmTypeMismatch`. **Directly-recursive enums type-check** (the AST enabler — heap-boxed payloads per ADR 0032 D4 need no nullable indirection, unlike recursive structs; verified). **downstream** (the new `Resolved`/`Typed` `Match`+`EnumConstruct` variants forced coordinated arms — the C1.3.2-4 cascade): codegen's `llvm_basic_type` lowers `Type::Enum` → the abi-v1 `{ i32 tag, ptr payload }` (heap-boxed/recursive-safe, ?Struct-style) so **enum-typed signatures lower**, `mangle_type` renders by name; the construction/`match` *expression* lowering rejects with a clean `CodegenError::EnumCodegenNotYet` (the ~8 pre-pass walkers recurse into children so they don't panic; drop is a no-op gated by `field_type_needs_drop=false`). MIR → `MirOp::Opaque` carrying operands (taint-safe; no `secret enum` ⇒ a match tag is never secret — the D7 sink is a future guard). effect-check + borrow-check pass-through walks (enum is **Move** — owns its heap payload; `match` arms move-merge like `if`/`else`). So `enum`/`match` **type-check but codegen rejects until (4/N)**. +27 tests (1265) incl. a `c5d1_non_exhaustive_match` UI snapshot. Four-check green. Next: **D.1 (4/N)** — codegen: the `{tag,ptr}` construction (alloc payload + set tag) + `match`→LLVM `switch` (D5) + recursive payload drop (D6) + the abi-v1 enum-layout entry + stability test + the `c5d1_enum` pass fixture; **ADR 0032 flips to ACCEPTED**. Then (D.1b) generic enums (`Option`/`Result`) via mono.
**Phase D.1 (4/N) — enum + match codegen; the D.1 MVP closes. ADR 0032 → ACCEPTED-WITH-AMENDMENTS — complete.** `enum`/`match` compile + run end to end (ADR 0032 D4/D5/D6/D11). **Construction (D4):** `lower_enum_construct` builds the `{ i32 tag, ptr payload }` — heap-box (`sentinel_alloc`) a struct of the variant's payload fields + store the args, `null` payload for unit variants, tag = variant discriminant; `enum_payload_struct_type` is the single source of truth for the payload layout (construct / match / drop share it). **`match` (D5):** `lower_match` extracts the tag + payload ptr, emits an LLVM `switch` into one block per variant arm (each GEP/loads the payload fields into the bindings' alloca slots keyed by `VarId`, lowers the body), reconciling arm results through a result-alloca merge block (the `if`-merge machinery); `_` = switch default, else the default is `unreachable` (exhaustiveness is a type-check guarantee — no runtime fallback). **Drop (D6) — amendment A1:** scope-exit drop loads the `{tag,ptr}` and `sentinel_free`s the payload if non-null (the `?Struct` drop arm with a null test); `field_type_needs_drop`(`Enum`) is true iff some variant carries a payload (pure-unit enums stay drop-free). Recursive *payload-field* drop is **deferred** — a heap-typed payload / recursive enum leaks its nested boxes here, because inline expansion of a recursive enum's drop is infinite (needs synthesized per-enum drop *functions*); leaks only, no UAF/double-free (verified: a returned enum escapes the callee, a value moved into a fn is freed once). **Surface:** bare `Enum::Variant` unit *construction* now parses (a small `parse_postfix` branch — `Name::Seg` with no `(` → a 0-arg `QualifiedCall`, mapped to a unit `EnumConstruct` at resolve when `Name` is an enum) — matching ADR D2 + the pattern surface; no regression (bare `Name::method` for a non-enum, previously a parse error, is now a resolve error — no test depended on it). **abi-v1:** §2 gains the `Enum` = `{ i32 tag, ptr payload }` entry + a DataLayout stability assertion (16 bytes, align 8, tag@0 / ptr@8); the `(3/N)` `EnumCodegenNotYet` reject is removed. **c51 bar holds** (enum paths additive; existing emission unchanged + `repro.rs` byte-identical). Phase-go `tests/pass/c5d1_enum.sentinel` (Shape `Unit`/`Circle(i64)`/`Rect(i64,i64)` constructed + `match`ed → exit 42). +2 tests (1268). Four-check green. **Next: developer-scope call** — **D.1b** (generic enums `Option`/`Result` via the witness-table mono machinery, ADR 0016 reuse + the recursive-payload-drop follow-on), OR the next ADR 0031 D4 prerequisite (strings + a byte type, or growable collections). The recursive-enum *drop* gap is the carried-forward debt — see the (5/N) investigation below (the box-free drop is **leak-free for the standard recursive-consume walk**, empirically; it leaks only when a payload binding isn't moved out or an enum is dropped unmatched — narrower than first stated; the full fix needs the payload-ownership model, not just drop fns).
**Phase D.1 (5/N) — recursive-drop investigation: it needs the payload-ownership model, NOT just synthesized drop fns. Code reverted to (4/N) box-free; docs-only.** Attempted the A1 follow-on (synthesize a per-enum `drop_<Enum>(ptr)` *function* so a recursive enum's drop is a runtime *call*, not infinite inline expansion, and recurse-drop the active variant's payload fields). **Built, then reverted — it double-frees** (empirically a `Tree` sum aborts, exit 133). The bug is **ownership**, not codegen: `match t { Node(l, r) => sum(l) + sum(r) }` loads `l`/`r` out of `t`'s payload and *moves* them into `sum(...)` (which frees their boxes), but `t` is also dropped at scope exit (the match reads it non-consumingly), so `drop_Tree(t)` recurse-frees the **same** child boxes again. **Correct fix = the payload-ownership model** (Rust semantics): (1) `match` **consumes** the scrutinee (partial move — not dropped at the enclosing scope); (2) a by-value binding **owns** its payload field, and the `match` frees the payload **box** without recurse-dropping the fields; (3) bindings are registered in the **drop plan** as arm-scoped locals, so an un-moved binding is dropped at arm-scope exit (reusing `emit_scope_drops` + the moved-set); (4) the synthesized `drop_<Enum>` fn is then sound, for the *non-match* drop paths (a `let`-bound never-matched enum; an un-moved binding). This is a **coordinated borrow-check + drop-plan + codegen change** (real partial-move tracking for enum payloads) — a proper sub-phase, bigger than "add drop fns". Recorded in ADR 0032 (A1 follow-up). **Box-free (leak-safe, no UAF) stays the shipped MVP behavior**; the tree still runs to exit 42 (leaking children). No code change (reverted); 1268 tests, four-check green. **Follow-up (empirical, via `leaks --atExit`):** the box-free debt is **narrower than first stated** — the standard recursive-consume walk (`match` + recurse-with-move on all children — the AST-evaluator shape) AND flat enums are **leak-free** (0 leaks); leaks occur only when a payload binding isn't moved out (bind-and-ignore) or an enum is dropped unmatched. So the ownership model is a *completeness* fix, not a self-hosting blocker, and is now **verifiable** via `leaks`. (Corrected "an AST leaks per node" in the docs.)
**ADR 0033 PROPOSED — Phase D.2 kickoff: strings + a byte (`u8`) type — docs-only.** The developer chose to iterate the ADR 0031 D4 roadmap in order; strings is next (a self-hosted lexer's input is text). ADR 0033 designs it end-to-end (9 D-decisions): the load-bearing call is **a string IS a `[u8]`** (byte array) — maximal reuse of the C1.6 array machinery (`len`/index/RAII drop/move/escape/arena/`abi-v1` `{i64,ptr}` all apply unchanged); plus `Type::U8` (an integer-scalar primitive → `i8`, reusing the op-generic arithmetic/cmp/bitwise + secret pipelines — almost no new typing surface; the next exhaustive-`Type`-match cascade after `Type::Enum`); char literals `'a'` (→ `u8`, escapes `\n \t \r \0 \\ \' \xHH`); string literals `"…"` (→ `[u8]`, **heap-copied** from a private global `[N x i8]` constant so they drop/move uniformly with no global-free hazard); the lexer ops `s[i]`/`len(s)` (reused) + a `str_eq` builtin + `u8`↔`i64` conversions (mixed-width stays a type error). 4-sub-phase split (lexer → AST/parser → `Type::U8`+cascade → codegen/runtime + `c5d2_strings` phase-go). Out of scope (D8): a nominal growable `String` (collections sub-phase D.3), UTF-8/Unicode code points, concat/substring/slice, `[u8]` mutation, signed `i8`. Next: **D.2 (1/N)** — the lexer.
**Phase D.2 (1/N) — lexer: string + char literals — complete.** Per ADR 0033 D7. Two new logos `TokenKind`s: `StringLit` (`"…"`) + `CharLit` (`'…'`), recognised with escape-aware regexes (`([^"\\\n]|\\.)*` between quotes — a `\`-escape or any non-quote/non-backslash/non-newline char; excluding raw newlines makes an unterminated literal fail fast). **Recognise-not-decode**, exactly like `IntLit`: the byte value(s) are recovered from the span at parse time (2/N), so `TokenKind` stays payload-free. Confirmed **`u8` needs no keyword token** — it lexes as an `Ident`, recognised as a primitive type name at the types layer like `i64`/`i32`/`bool` (verified: `u8` → `Ident`, `u8_to_i64` → one `Ident` by longest-match). Additive — the new tokens cascade nowhere (no exhaustive `TokenKind` match downstream); existing programs unaffected. +7 lexer tests (1275 total), four-check green. Next: **D.2 (2/N)** — AST + parser (`ExprKind::CharLit`/`StringLit` + escape decoding; `u8` in `TypeExpr`).
**Phase D.2 (2/N) — AST + parser: char/string literals — complete (`f310f15`).** Per ADR 0033 D7 (2/N), mirroring the enum/`match` (2/N) shape — the blast radius stays in **ast + syntax + resolve** (verified: every codegen/types/mir/borrow/effect match is on the *typed/resolved* tree, so adding raw-`ExprKind` variants touches only the AST `Display` + `resolve_expr`). AST: `ExprKind` += `CharLit(u8)` + `StringLit(Vec<u8>)` carrying the **decoded** bytes (a string IS a `[u8]` — D3), with s-expr `Display` arms `(char N)` / `(string b0 b1 …)`. Parser: **decode the span at parse time** (like `IntLit`) — strip the quotes by byte index, then `decode_byte_literal` processes the escapes `\n \t \r \0 \\ \' \"` and `\xHH` (two hex digits → one byte via `hex_digit`) into the bytes; non-escape bytes (incl. multi-byte UTF-8) pass through verbatim, so a string is exactly its UTF-8 source bytes. A char literal must decode to **exactly one byte** — `''` (empty) / `'ab'` (too many) / a multi-byte source char are rejected (`CharLitNotSingleByte`); an unknown escape letter or a malformed `\x` (non-hex or < 2 digits) is rejected (`InvalidEscape`). Two new `ParseError`s + their `query.rs` diagnostic arms; the decoder is bounds-checked (panic-free) via `.get(..).ok_or(())?`. `u8` needed **no type-parser change** — it already parses as a `TypeExpr` `Ident`; `[u8]` / `u8` in param/return position are parse-confirmed by test. Resolve: **rejects** char/string literals with `CharStringLitNotYet` (an or-pattern arm before the type layer) — so **`ResolvedExprKind` gains no new variant** and the downstream typed-tree crates are untouched until 3/N brings up `Type::U8`. Additive end-to-end. +21 tests (2 ast `Display`, 17 parser decode/validate/reject + `u8`-in-`TypeExpr`, 2 resolve reject) — **1296 total**, four-check green. Next: **D.2 (3/N)** — the type layer: `Type::U8` + the cascade across every exhaustive `Type` match + `ArrayElem::U8`; char → `u8`, string → `[u8]`; `str_eq` + `u8`↔`i64` conversion builtins typed; resolve → typed mirrors.
**Phase D.2 (3/N) — the type layer: `Type::U8` + char/string typing + byte builtins — complete (`56f69f0`).** Per ADR 0033 D3/D4/D5, mirroring the enum (3/N) discipline (land the `Type` cascade together → build → semantics → codegen-rejects-until-(4/N)). char/string now **type-check** end to end; the blast radius is **types + codegen + borrow + mir + effect + resolve** (the enum-(3/N) surface; no ast/syntax — those landed at 2/N). **`Type::U8`** is a new primitive variant (no interner, like `I32`/`Bool`) that cascades across every exhaustive `Type` match — found via the "add the variant, let the compiler enumerate the cascade" discipline: types (`is_int` += U8 / `to_array_elem` → `ArrayElem::U8` / `to_nullable_inner` → `None` (no `?u8` at MVP) / `substitute` / `try_substitute` / `contains_type_param` / `type_display` / `Display`), codegen (`llvm_basic_type` → `i8` / `llvm_int_type` → `i8` / `mangle_type` → `u8` / the needs-drop + drop-emit no-drop groups), borrow (`is_copy` → Copy — a `u8` byte is a 1-byte copy scalar; a `[u8]` is `Array(_)` → Move). **`ArrayElem::U8`** is the one genuinely new array element (`[u8]` IS the string — ADR D3). `u8` resolves in type position like `i64`/`i32`/`bool` (`resolve_type_expr`); `[u8]` flows through the existing array type-expr path. `ResolvedExprKind`/`TypedExprKind` gain `CharLit(u8)`/`StringLit(Vec<u8>)`; the type checker assigns char → `Type::U8`, string → `Type::Array(ArrayElem::U8)`. **The op-generic pipeline absorbs `u8` with ONE change** (`is_int += U8`): arithmetic + bitwise type via `Binary`, comparison via `Cmp` (→ bool), and mixed-width `u8 + i64` stays a `Mismatch` (the existing `l.ty != r.ty` operand check — no new `TypeError`); `secret u8` inherits the C3.1b secret-preserving rules for free. **Three runtime builtins typed** (concrete, non-generic): `str_eq([u8],[u8]) → bool`, `u8_to_i64(u8) → i64`, `i64_to_u8(i64) → u8` — registered as `FnId(4..=6)` in resolve + `TypedFnSignature`s in types, so **user fns shift +3** (main: 4 → 7; the ~5 hardcoded-`FnId` test sites in resolve/types/effect/borrow updated). resolve drops the (2/N) `CharStringLitNotYet` reject (now produces the literals); mir lowers literals to `MirOp::Opaque` (public constants — taint-safe for the D5 pass); effect/borrow treat them as pure leaves (a string's owned `[u8]` drops via its binding's type, like any array). **Codegen lowers `u8` → `i8`** so the cascade is real (a `u8` fn body `c + c` compiles to an object, exit 0) but **rejects char/string literals + the three builtin calls** with `CodegenError::StringCodegenNotYetSupported` until (4/N) (the enum-(3/N) codegen-rejects-until-(4/N) discipline; **empirically verified**: `let s = "hi"` type-checks then rejects cleanly at codegen — exit 1, no panic). +13 tests (12 type-layer unit: char/string typing, `u8` arith/cmp/bitwise, mixed-width + no-implicit-widen rejects, `[u8]` index → `u8`, `str_eq`/conversions typing + a `[i64]`-arg reject, `secret u8`; + the `c5d2_mixed_width` UI snapshot — `u8 + i64` Mismatch renders as "expected u8, found i64") — **1309 total**; the 2 resolve (2/N) reject-tests became resolve-tests. Four-check green. Next: **D.2 (4/N)** — codegen + runtime: the `i8` char constant; the string-literal private global `[N x i8]` + heap copy (so `[u8]` drops/moves uniformly — ADR D6); `sentinel_str_eq`; the `zext`/`trunc` conversions; `abi-v1` `u8` entry + tests + the `c5d2_strings` phase-go (verified leak-free via `leaks --atExit`); ADR 0033 flip.
**Phase D.2 (4/N) — codegen + runtime: the strings + `u8` MVP runs end to end — complete (`891ec98`). ADR 0033 → ACCEPTED-WITH-AMENDMENTS. Phase D.2 closes.** Per ADR 0033 D6/D9 — the (3/N) `StringCodegenNotYetSupported` rejects are replaced by real lowering. **Codegen:** a char literal is an `i8` constant; a string literal **heap-copies** its decoded bytes (`sentinel_alloc(N)` + N `i8` stores) into an owned `[u8]` that drops/moves via the existing array paths; `u8` lowers to `i8` with **unsigned** ops — `udiv` + unsigned `icmp` predicates (`is_unsigned = strip_secret(lhs.ty) == U8`), so a byte `≥ 0x80` compares large not negative; `u8_to_i64` is a `zext`, `i64_to_u8` a `trunc`; `str_eq` lowers to a call to the new runtime `sentinel_str_eq(ptr, i64, ptr, i64) -> i1` (extract `{len,ptr}` from each `[u8]` struct value). **Runtime:** `sentinel_str_eq` — equal length + byte-wise equality (Rust `extern "C" -> bool` lowers to `i1 zeroext`, matching codegen's `i1` decl; `#[allow(not_unsafe_ptr_arg_deref)]` per the existing runtime convention); the `[u8]` args are **borrowed** (the C2.3 runtime-builtin rule treats them non-consuming, like `len`), so `str_eq` does **not** free them — the caller's bindings drop them. **abi-v1:** `u8` → `i8` (size 1, align 1, mangles `u8`; `[u8]` → `arr_u8`, same `{i64,ptr}` layout); `sentinel_str_eq` joins the symbol contract (now **19**); the doc + `abi_v1_type_layouts_via_datalayout` / `abi_v1_mangling_is_stable` / `abi_v1_runtime_symbol_set` tests pin it. **Verified empirically** (exit-code-is-the-answer + `leaks --atExit`): `tests/pass/c5d2_strings` parses the 2-digit source "42" → exit 42, **0 leaks** (char-lit `u8` compare via `is_digit`, `u8`↔`i64` via `digit_val`, `[u8]` indexing, `str_eq` over bound keywords); `tests/pass/c5d2_u8_unsigned` pins the unsigned paths (`200 > 100` true, `200 / 100 == 2`) → exit 42, **0 leaks**; a `u8` fn body compiles to an object; the **c51 repro bar holds** (an unused `sentinel_str_eq` decl doesn't change emitted objects). **Amendments:** **A1** string literals heap-copy via **direct byte-stores**, not D6's private global `[N x i8]` + `memcpy` — `CodegenCtx` holds no `&Module` to add a global from a lowering method (the same constraint that pre-walks spawn wrappers); identical owned-heap-copy semantics, the global deferred as a measured optimisation. **A2** inline string-literal **arguments** to borrowing builtins leak — the **pre-existing general temporary-drop gap** (empirically: `len([i64-array])` temporaries leak identically when unreachable; **not** D.2-introduced), tied to the deferred full escape analysis (ADR 0026 D2, post-1.0). **Bound variables are leak-free** (the phase-go binds every literal, as a real lexer holds its data); a future `&[u8]` builtin signature is the cleaner long-term form. **A3** `str_eq` args are **borrowed, not consumed** (the C2.3 builtin rule). +2 pass fixtures (**1311 total**), four-check green. **Phase D.2 (strings + a `u8` byte type) MVP closes.** Next: the ADR 0031 D4 roadmap continues — **growable collections** (ADR 0034 PROPOSED) → file I/O (stdlib) → modules → loops, then the self-host port.
**ADR 0034 PROPOSED — Phase D.3 kickoff: growable collections (`Vec<T>`; `String` = `Vec<u8>`) — docs-only.** Per ADR 0031 D4 item 3 — a lexer accumulates an identifier byte-by-byte and a parser accumulates token/node lists, neither expressible with the fixed `[T]` array (no `push`/growth). ADR 0034 designs it end-to-end (10 D-decisions): the load-bearing lever is **a `Vec<T>` is `[T]` plus capacity + mutation** — `Type::Vec(VecElem)` mirrors `Type::Array(ArrayElem)` exactly (the same flat element subset `I64`/`I32`/`Bool`/`U8`/`Struct`, an `abi-v1` `{ i64 len, i64 cap, ptr data }` layout, element-generic builtins that recover `T` from the typed arg like array-index/`len` already do), so it **reuses the array index/bounds-check/move/drop machinery with NO new monomorphisation and NO lexer/parser change** (`Vec<u8>` already parses as a `Generic { name: "Vec", args: [u8] }` TypeExpr; the types layer recognises the name). **`String` = `Vec<u8>`** (a growable byte buffer — the 0033 "a string is its bytes" lever; not a separate nominal type). New pieces: a **capacity field + growth** (`push` reallocs `max(1, cap*2)` on overflow — libc `realloc`, NOT the broker bump arena which can't realloc); **`push(&mut v, x)`** — the **first heap-mutation primitive**, reusing `&mut T` + the C2.2 shared-XOR-mutable rule; `vec_new()` (element type inferred from the binding annotation, like `null`'s `?T`); `len(v)` (extend the existing builtin to `Vec`); `v[i]` (reuse the C1.6 bounds-checked `Index`). `Vec<T>` is a **builtin generic** (like `[T]`), NOT a `class Vec<T>` (generic classes deferred, ADR 0022 D1). Drop frees the buffer; **primitive-element `Vec` (`Vec<u8>`/`Vec<i64>`) is leak-free**, droppable-element `Vec` (`Vec<Struct>`/`Vec<[u8]>` recursive element drop) is **deferred** (the enum-A1-shaped follow-on). One new runtime symbol (`sentinel_vec_grow`/`realloc`). **3-sub-phase split (D9):** (1/N) `Type::Vec` + the cascade + `vec_new`/`push`/`len` typed + codegen + growth runtime + `&mut Vec` borrow + primitive-element drop (end to end — a growable `Vec<u8>`/`Vec<i64>`); (2/N) `v[i]` + `pop` + the `Vec<u8>`→`[u8]` bridge (`str_eq` a built string against a keyword); (3/N) close — `c5d3_collections` phase-go (leak-free via `leaks --atExit`) + `abi-v1` `Vec` entry + ADR flip. Out of scope (D8): a `Map`/`HashMap` (its own ADR), droppable-element `Vec` drop, `Vec`-in-generic-fns (`VecElem::TypeParam`), `with_capacity`/`insert`/`remove`/slicing/iterators/`for`, a `?T`-returning `pop`, `secret Vec`, broker-backing. Next: **D.3 (1/N)** — `Type::Vec` + the cascade + `vec_new`/`push`/`len` end to end.

**Phase D.3 (1/N) — growable `Vec<T>`: `Type::Vec` + `vec_new`/`push`/`len` end to end — complete (`a64883c`). ADR 0034 stays PROPOSED (3-sub-phase split; (1/N) Amendments recorded).** Per ADR 0034 D9 (1/N): a growable, owned, mutable `Vec<T>` — `[T]` plus a capacity field + mutation. **types:** `Type::Vec(VecElem)` (the flat element subset mirroring `ArrayElem`) + the full exhaustive-`Type`-match cascade (`substitute` / `try_substitute` / `contains_type_param` / `unify_one` — the last gets an explicit `(Vec,Vec)` arm so generic inference binds the element — plus `Display`, the `to_nullable_inner` / `to_array_elem` None groups, and `is_vec` / `to_vec_elem`); `resolve_type_expr` recognises `Vec<T>` as a builtin generic (flat element via `to_vec_elem`, else the new `VecElementNotSupported`); `vec_new<T>() -> Vec<T>` (element pinned from the binding / return annotation — the body-tail expected-type seeding extended to `Vec`) and `push<T>(&mut Vec<T>, T) -> i64` type through the **uniform generic-call path** (no special-casing), while `len` gets a contained `check_call` overload over `[T]` + `Vec<T>` (the `[T]` error path preserved exactly). **codegen:** `Vec` → `{ i64 len, i64 cap, ptr data }` (data is **field 2**); `lower_vec_new` builds `{0,0,null}`; `lower_push` loads the `&mut Vec`, grows via `sentinel_realloc` to `max(1, cap*2)*sizeof(T)` when `len==cap` (the grow block stores cap+data back, the continuation re-loads — no PHI), writes `data[len]=x`, bumps `len`; drop frees field 2 (null-safe); `len` reuses the field-0 extract. **runtime:** one new symbol `sentinel_realloc` (libc realloc; `realloc(null,n)==malloc` serves the first push). **borrow-check:** a `&mut Vec` builtin arg registers a **mutable borrow** (extends the ADR 0033 A3 runtime-builtin-arg rule to references), so `push` participates in shared-XOR-mutable and a non-`mut` `Vec` push is rejected (`BorrowMutOfImmutable`); `Vec` is **Move**. Builtins shift the FnId base (vec_new=7, push=8; main 7→9 — fixed the hardcoded-FnId test sites in resolve / effect-check / borrow-check / types). **abi-v1:** §2 `Vec` layout (`{i64,i64,ptr}`, 24/8, data@16) + §4 `vec_` mangling + §5 `sentinel_realloc` (now **20** symbols); `abi_v1_type_layouts_via_datalayout` + `abi_v1_runtime_symbol_set` pin it. **Amendments (ADR 0034):** A1 `String`=`Vec<u8>` deferred to (2/N) with the bridge; A2 return-type pushdown extended to `Vec`; A3 `len` overload special-case; A4 `sentinel_realloc` (not `sentinel_vec_grow`); A5 `VecElementNotSupported`; arena routing unchanged (a `Vec` init is a `Call`, not an `ArrayLit`, so `is_primitive_array_lit` already excludes it). **Verified** (exit-code + `leaks --atExit`): `tests/pass/c5d3_collections` builds a multi-growth `Vec<i64>` (6 pushes), a char-pushed `Vec<u8>`, and a `Vec` moved out of a helper (the escape path) → **exit 67, 0 leaks**; +13 tests (**1324 total**), four-check green. DEFERRED: (2/N) `v[i]` (the `Index` node carries `ArrayElem` + hard-codes the field-1 data ptr — real typed-tree + codegen work) + `pop` + the `Vec<u8>`→`[u8]` bridge + the `String` alias; (3/N) the richer phase-go + the ADR flip; (D8) droppable-element `Vec` drop. **Phase D.3 (1/N) lands.** Next: **D.3 (2/N)**.

**Phase D.3 (2/N) — `Vec` `v[i]` / `pop` / the `Vec<u8>`->`[u8]` bridge / `String` — the growable-`Vec` MVP is COMPLETE (`8430b0a`). ADR 0034 → ACCEPTED-WITH-AMENDMENTS.** Folded in the thin (3/N) close (comprehensive phase-go + ADR flip), since these four pieces exhaust the D.3 MVP (the rest is D8-deferred) and the abi-v1 Vec entry already landed in (1/N). **`v[i]`:** reuses the C1.6 bounds-checked `Index` with NO new typed node — the type checker accepts a `Vec` target (its `VecElem` demotes to the structurally identical `ArrayElem` for the node), and `lower_index` reads the data pointer from **field 2** (`Vec`) vs **field 1** (array), keyed on the secret-stripped target type; `len` (field 0) + the OOB trap are reused verbatim. **`pop<T>(&mut Vec<T>) -> T`** and **`vec_to_array<T>(Vec<T>) -> [T]`** (the bridge) are new builtins (FnId 9 / 10; main 9→11) flowing the uniform generic-call path like `push`: `pop` decrements `len` (buffer retained) and traps on empty; `vec_to_array` is non-consuming (`memcpy`s the live `len*sizeof(T)` bytes into a fresh `sentinel_alloc`'d `[T]`, so the Vec + array own independent buffers — both freed), keeping `str_eq`'s `[u8]` surface unchanged. **`String` = `Vec<u8>`** (Amendment A1 resolved): the bare name resolves to `Type::Vec(VecElem::U8)` in `resolve_type_expr`; a string *literal* is still a `[u8]` (so `let s: String = "hi"` is a Mismatch — build via `vec_new`+`push`; the bridge closes the loop the other way). **Amendments B1** (`v[i]` reuses Index, no `VecIndex` node — no typed-tree cascade), **B2** (`pop`/`vec_to_array` uniform-path builtins), **B3** (`String` alias). **Verified** (exit-code + `leaks --atExit`): `v[i]` reads + OOB trap, `pop` + empty-pop trap (exit 134), the bridge (positive + negative `str_eq`, non-consuming, empty), `String` build; the comprehensive `c5d3_collections` (`Vec<i64>` push/index/pop/len + escape; `String` "let" built/indexed/bridged/`str_eq`'d) runs at **exit 55, 0 leaks**. +9 type-layer tests (**1334 total**), four-check green. **Phase D.3 MVP closes; ADR 0034 ACCEPTED-WITH-AMENDMENTS.** Next: **D.4 — file I/O (ADR 0031 D4 item 4; ADR 0035 to be written).**

**ADR 0035 PROPOSED — Phase D.4 kickoff: file I/O via a minimal stdlib (`read_file` / `write_file`) — docs-only.** Per ADR 0031 D4 item 4 — a self-hosting compiler must read its source + write its artifact, and the only runtime I/O is `sentinel_print(i64)`. ADR 0035 designs it (10 D-decisions). **Load-bearing call (D2): file I/O = runtime builtins (like `print`), NOT the algebraic-effect/handler machinery** — ADR 0020 effects are *resumable user computations* (a `handle` arm resumes via `k`); OS I/O is irreversible side effects, and the effect-check forbids an effectful `main` + provides no runtime handler, so an `Io` effect would force a `handle` whose arm *still* calls a syscall builtin (pure ceremony). This **amends ADR 0031 D4's "effects + handlers" framing**; the effect-ROW promotion (`print`/I/O → `! { Io }`) stays a deferred orthogonal concern (D3). **Surface (D4):** `read_file(path: [u8]) -> [u8]` (whole file → owned byte array, reusing the `{len,ptr}` array machinery), `write_file(path: [u8], data: [u8]) -> i64` (create/truncate), `print_bytes([u8]) -> i64` (stdout) — all builtins in the `print`/`str_eq` mould, backed by new libc `sentinel_read_file`/`sentinel_write_file`/`sentinel_print_bytes` wrappers joining `abi-v1` §5. **Error model (D5):** panic-on-failure (abort like OOB/bad-alloc); a recoverable `?[u8]`/`Result` is deferred (wants D.1b generic enums). Paths are `[u8]`, NUL-terminated by the wrapper; `write_file`/`print_bytes` borrow their args (the ADR 0033 A3 rule). **2-sub-phase split (D9):** (1/N) `read_file`+`write_file` + the 2 runtime symbols + a write-then-read-back round-trip phase-go; (2/N) close — `print_bytes` + abi-v1 + ADR flip. Out of scope (D8): recoverable errors, the `Io` effect row, streaming/handles/`seek`/`fd`s, `read_stdin`, directories/`stat`, append, `Vec<u8>`-return, `secret` I/O. **3 OPEN DESIGN POINTS (settle before (1/N)):** (1) effects-vs-builtins (proposed: builtins), (2) error model (proposed: panic), (3) MVP surface (proposed: read_file+write_file, +`print_bytes` in 2/N; is `read_stdin` in?). Next: **D.4 (1/N)** — settle the open points, then `read_file`+`write_file` end to end.

**Phase D.4 (1/N) — file I/O: `read_file` + `write_file` end to end — complete (`2c530f6`). ADR 0035 stays PROPOSED ((1/N) landed; the 3 open design points resolved at the proposed defaults).** Per ADR 0035: file I/O = **runtime builtins** (like `print`), NOT algebraic effects (D2). **`read_file(path: [u8]) -> [u8]`** reads a whole file into a fresh owned byte array; **`write_file(path: [u8], data: [u8]) -> i64`** creates/truncates + writes (returns 0); both **panic on failure** (D5). Typed as non-generic `[u8]` builtins (the `str_eq` template); dispatched in `lower_call`. **Runtime (Amendment A1):** uses Rust `std::fs::read`/`write` (the runtime is a Rust crate linking `std`), NOT raw libc `fopen`/`fread` (D6's sketch); `read_file` **copies** the bytes into a `sentinel_alloc`'d (libc-malloc) buffer so the caller's `Type::Array` scope-exit drop frees it. Paths build a Unix `OsStr` from raw `[u8]` (non-UTF-8 OK), aborting on an embedded NUL. **ABI (A2):** `sentinel_read_file(path_ptr, path_len, out_len: *i64) -> data_ptr` (out-param for the count); `sentinel_write_file(path_ptr, path_len, data_ptr, data_len) -> i64`; `write_file`'s args borrowed (ADR 0033 A3). abi-v1 now **22** symbols. Builtins FnId 0..=12 (read_file=11, write_file=12; main 11→13 — fixed the hardcoded-FnId test sites). **Verified** (exit-code + `leaks --atExit`): a write-then-read-back round-trip (`str_eq` + a `back[i]` byte spot-check + `len`), exact-byte fidelity, the missing-file abort (exit 134, clear message); phase-go `c5d4_file_io` round-trips "hello" → **exit 5, 0 leaks** (the harness removes the temp file). +5 tests (**1339 total**), four-check green. ⚠ inline string-literal ARGS to read_file/write_file leak (the ADR 0033 A2 temp-drop gap — bind paths/payloads). DEFERRED: (2/N) `print_bytes` (stdout) + the ADR flip; (D8) recoverable errors / `Io` effect row / streaming / `read_stdin` / directories. **Phase D.4 (1/N) lands.** Next: **D.4 (2/N)** — `print_bytes` + close.

**Phase D.4 (2/N) — `print_bytes` (stdout) — the file-I/O MVP is COMPLETE (`fb1b51b`). ADR 0035 → ACCEPTED-WITH-AMENDMENTS.** **`print_bytes(data: [u8]) -> i64`** writes a byte array to stdout — the byte/string companion to `print` (one i64). The `write_file` template minus the path: a runtime builtin (FnId 13; main 13→14), arg borrowed (ADR 0033 A3), backed by `sentinel_print_bytes(data_ptr, data_len)` (abi-v1 now **23** symbols). **Amendment B1:** writes **exactly** `data_len` bytes — NO added newline (unlike `print`'s `println!`) — then **flushes** stdout, so the bytes are visible before the C-ABI `main` return and interleave correctly with `print` (shared `std::io::stdout`; `od -c`-verified: `print_bytes("AB"); print(7); print_bytes("AB")` → `AB7\nAB`). **Verified** (exit + stdout + `leaks --atExit`): the comprehensive phase-go `c5d4_file_io` now round-trips a file (write → read → `str_eq` + `back[i]` + `len`) AND `print_bytes` the read-back content, asserting **both exit 5 AND stdout "hello"**, 0 leaks. +2 type-layer tests (**1341 total**), four-check green. **Phase D.4 MVP closes; ADR 0035 ACCEPTED-WITH-AMENDMENTS** (read_file + write_file + print_bytes; recoverable errors / `Io` effect row / streaming / `read_stdin` / directories stay deferred per D8). Next: **D.5** — the next ADR 0031 D4 prerequisite (#5 modules or #6 loops; dev picks — loops recommended as smaller + unblocking iteration), then the self-host port.

**ADR 0036 PROPOSED — Phase D.5 kickoff: loops (`while`) — docs-only.** Per ADR 0031 D4 item 6 (the dev chose loops over #5 modules). The surface has been recursion-only by design since 1.0; a compiler's iteration-heavy passes (scan a byte buffer, drain a token `Vec`) want bounded, stack-safe iteration. ADR 0036 designs it (10 D-decisions). **Load-bearing calls: (D3) a loop is a STATEMENT** — `StmtKind::While { cond, body }` alongside `Let`/`Assign`/`Expr`, NOT an expression (a loop has no value; Sentinel has no unit type, so an expression form would force a synthetic `i64` 0). **(D4) `while` lowers to the first BACKWARD CFG branch** — three blocks `loop_cond` / `loop_body` / `loop_after` with a back-edge body→cond (all prior control flow — `if`/`match` — merged *forward* into a tree CFG). **(D5) per-iteration drop** — the body is a `lower_block` scope, so its bindings drop each iteration via the back-edge (a body that allocates each pass is leak-free, not accumulating — the load-bearing correctness property). **(D8, the key risk) the loop-carried move rule** — the borrow checker walks the body once but it runs N times, so moving an *outer* binding inside the body is a use-after-move on re-entry; proposed: conservatively **reject moving an outer Move-typed binding in a `while` body**. **Surface (D2):** `while <bool> { <body> }`; loop-carried state is a `let mut` outside + `Assign` inside (`i = i + 1`); `cond` must be `bool`. NO new `Type`, no cascade, no FnId-shift (not a builtin). `while` is a NEW lexer token (`for` is taken by `impl … for …`). **2-sub-phase split (D9):** (1/N) `while` (token + parser + `StmtKind::While` + bool-cond rule + the D8 move rule + back-edge codegen + per-iteration drop); (2/N) `break`/`continue` (branch to loop_after/loop_cond + a loop-target stack). Out of scope (D8): `for` / ranges / iterators, labeled break, `break`-with-value / loop-as-expression, do-while, a termination check (`while true {}` is well-formed). **3 OPEN DESIGN POINTS (settle before (1/N)):** (1) the loop-carried move rule (conservative reject vs. dataflow), (2) break/continue → (2/N), (3) while-as-statement. Next: **D.5 (1/N)** — settle the open points, then `while` end to end.

**Phase D.5 (1/N) — the `while` loop — end to end — complete (`adec9c3`). ADR 0036 stays PROPOSED ((1/N) landed; the 3 open design points resolved at the proposed defaults).** A `while` loop through the WHOLE pipeline. **(D3) a loop is a STATEMENT** (`StmtKind::While { cond, body }`, not an expression — no loop value, no unit type): lexer (new `while` token; `for` is taken by `impl … for …`), parser (statement position; `parse_loop_body` = `parse_block_inner(allow_stmt_only=true)` synthesises a discarded unit tail for a statement-only body — Amendment A1, since Sentinel blocks require a tail per ADR 0010 D6; struct lits forbidden in the cond like `if`), AST/resolve (body scope, snapshot/restore vars)/types (cond must be `bool`; `secret bool` → SecretBranch; body value discarded) + the StmtKind cascade across mir/effect-check/codegen (8 codegen walk sites + lower_stmt). **(D4) the FIRST backward CFG branch:** `lower_stmt::While` emits `loop_cond`/`loop_body`/`loop_after` with a back-edge body→cond; the body lowers via `lower_block` so its bindings **drop per-iteration** (D5 — a body allocating each pass is leak-free). **(D8, the key risk) the loop-carried move rule:** moving an *outer* Move-typed binding inside the cond/body is a use-after-move on re-entry → rejected (`MovedInLoopBody`, a new BorrowError); implemented by snapshotting in-scope + moved sets before the loop and flagging any outer binding newly moved. A body-local move is fine; loop-carried `Assign`/`push(&mut v)` are fine. **Amendment A2 (load-bearing codegen fix): entry-block alloca hoisting.** A body `alloca` emitted inline in `loop_body` runs every iteration → stack grows → overflow at large N (verified: a 2M-iteration body-`let` loop SIGSEGV'd). Fix: a `loop_depth` counter (bumped around `lower_block(while-body)`); when >0, per-binding allocas (`let`/`if`-result/`match`-result) go to the fn entry block (executed once, slot reused) via `binding_alloca`. Non-loop codegen (`loop_depth==0`) keeps the inline alloca → **byte-identical** to pre-D.5 (c51 bar holds). **No new `Type`, no `Type` cascade, no FnId-shift.** **Verified** (exit + `leaks --atExit`): counter loop, Vec-built-in-loop, body-allocating loop (leak-free), 2M-iteration loop (no overflow), zero-iteration loops, loop-carried-move rejection; phase-go `c5d5_loops` → **exit 67, 0 leaks**. +9 tests (3 type + 3 borrow + 2 parser + the fixture; **1350 total**), four-check green. DEFERRED: (2/N) `break`/`continue` + the ADR flip; (D8) `for`/ranges/iterators, labeled break, `break`-with-value, termination check. **Phase D.5 (1/N) lands.** Next: **D.5 (2/N)** — `break`/`continue` + close.

**Phase D.5 (2/N) — `break` / `continue` — end to end — complete. ADR 0036 → ACCEPTED-WITH-AMENDMENTS (D.5 closed).** Loops gain early exit / skip. **`break` / `continue` are payload-free STATEMENTS** (`StmtKind::Break`/`Continue` alongside `While`; new `break`/`continue` lexer keywords) branching to the innermost enclosing loop's `loop_after` (break) / `loop_cond` (continue). Pipeline: lexer tokens (+ ident-prefix regression) → AST/resolve/types/mir/effect-check/borrow/codegen StmtKind cascade (the resolve/mir/effect-check/borrow arms are no-ops — no sub-expr, no move, no effect). **(C2) the loop-target STACK:** a `LoopTarget { cond_bb, after_bb, scope_floor }` pushed onto `CodegenCtx::loop_targets` entering a `while` body, popped on exit; `break`/`continue` read the top (innermost — no labels). **(C1) the load-bearing drains-before-branch:** a break/continue branches out of the *middle* of the body, skipping `lower_block`'s end-of-body `emit_scope_drops`, so codegen **drops every scope frame from the top down to the loop body BEFORE branching** — `emit_loop_exit_drops(scope_floor)` (the body scope + any nested `if`/block scopes, innermost first; `scope_floor` = the body frame's index captured at loop entry). `emit_scope_drops` was split into a per-frame `emit_frame_drops` to share the logic. Each runtime path frees a binding exactly once (early-exit drop, or body-end drop on fall-through — mutually exclusive blocks); **verified leak-free** with a `[u8]` live across a break AND a continue, incl. a nested inner loop (inner break drains only the inner scope). **(C3) first mid-block divergence:** Sentinel has no early `return`, so break/continue is the first construct to terminate a block mid-stream — the statically-lowered, now-dead remainder parks on a fresh `after_loopctl` block (never append to a terminated block; covers a stmt-only body's synth unit tail + `lower_if`'s store/merge). **(C4) out-of-loop rejection:** `break`/`continue` outside any loop → `TypeError::LoopControlOutsideLoop` (names the kw), via a `loop_depth: u32` on `VarTypeEnv` (bumped around a `while` body — threads through nested `if`/`match`; legal iff `>0`; fresh per fn so no break across a fn). **(C5) ergonomic note:** a *conditional* break uses the tail idiom `if c { break; 0 } else { 0 };` (`if` requires `else` + a tail per ADR 0010/0013 — pre-existing, not break's fault; cleaner ergonomics is a Revisit). **No new `Type`, no cascade beyond the StmtKind arms, no FnId-shift.** Phase-go `c5d5_break_continue` (break-terminated sum 15 + continue-filtered evens 30 + two loops that break/continue with a `[u8]` live, 30+40) → **exit 115, 0 leaks**; `c5d5_loops` still exit 67. +11 tests (5 type + 3 parser + 2 lexer + the fixture; **1361 total**), four-check green. **Phase D.5 COMPLETE.** Next: **#5 modules** (ADR 0031 D4 — the last prerequisite before the self-host port).

**ADR 0037 PROPOSED — Phase D.6 kickoff: modules / multi-file — docs-only.** The sixth and **last** ADR 0031 D4 prerequisite before the self-host port. Two decisions settled with the language owner: **(1) module surface = file-as-module + `use`** — a file IS a module, its path relative to the source root (the entry file's dir) IS its module path; `use a::b::Item;` imports a `pub` item; `pub` (parsed since C4.1, a no-op) becomes the cross-module visibility gate; NO `mod` blocks (the Go/Python shape, not Rust's in-file tree). **(2) compilation model = TRUE separate compilation** (NOT a whole-program multi-file merge) — each module compiles to its own `.o` independently, cross-module refs resolved at LINK time via stable `abi-v1`-keyed symbols. **The biggest architectural D-change:** it breaks 3 whole-program codegen assumptions — `collect_mono_instantiations` (whole-program generic-instance discovery), the single `fns: HashMap<FnId, FunctionValue>` map, and `self.fns.get(&id)` call resolution — and makes cross-unit symbols ABI surface (the current bare-source-name mangling is single-file-only + not collision-free → D7 = a module-qualified, length-prefixed `abi-v1` mangling amendment, test-enforced). **Sub-phase split (D9):** **(1/N)** surface + resolve module graph (per-unit ID spaces + namespaces + visibility) + per-unit type-check against imported signatures + **non-generic** separate compilation (per-unit `.o`, module-qualified mangling, extern-symbol cross-module calls + types, deterministic link); **(2/N)** **cross-module generics** (per-unit instantiation + `linkonce_odr` dedup — the C++ template model) + cross-module trait/impl methods; **(3/N)** incremental caching (Salsa) + per-unit `.o` repro. NO new runtime `sentinel_*` symbols (a front-end + linking concern). 4 OPEN DESIGN POINTS (settle at (1/N)): import cycles (lean allow); amend `abi-v1` vs bump `abi-v2` (lean amend); source root = entry-file dir; `use a::b::c` = item `c` in module `a::b`. **D.6 (1/N) IN PROGRESS (multi-file COMPILES + RUNS, via the owner-chosen lower-risk Path A merge, not yet true per-unit separate compilation):** `use` front-end + module-graph discovery + top-level `pub` + import resolution/visibility + the merge (`merge_modules` qualifies EVERY top-level item's name by module path — fn/struct/enum/trait/effect/class/named-impl — + rewrites all call/type/trait/effect references via a per-module `Renamer` → existing pipeline → one object → link). Cross-module `pub fn`/`pub struct`/`pub enum`+`match`/`pub trait`/`pub effect` all compile + run, same-named items across modules coexist, cross-module GENERICS work (whole-program mono over the merged graph), and the merged path runs effect-check (an unhandled-effect `main` is rejected). FOLLOW-UPS: the true per-unit back end (objects + module-qualified `abi-v1` mangling + multi-object link, incl. per-unit `linkonce_odr` generics); span-accurate multi-source diagnostics. The language gate for the self-host port (D5) is effectively cleared. See §0.3 RESUME-AT + ADR 0037 Implementation notes.
**ADR 0038 → ACCEPTED-WITH-AMENDMENTS — Phase D movement 2: the self-host port — kickoff + (1/N) lexer-in-Sentinel COMPLETE.** Movement 1 (the language/stdlib build-out, ADR 0031 D2: D.1 sum types → D.6 modules) is **complete**, so the self-hosting gate is cleared and **movement 2** (ADR 0031 D5 — port `snc` to Sentinel stage by stage, each differentially validated against the Rust `snc` oracle) opened. **(1/N) the LEXER landed:** `snc lex` (oracle) + `selfhost/lexer.sentinel` (the first compiler stage in Sentinel, all 69 `TokenKind`s) + a corpus differential test (139/139 clean-lexing fixtures match `snc lex`). Amendments: A1 direct dump emission (no Token enum yet — (2/N) adds the token list); A2 worked around two Sentinel quirks (flat per-fn var namespace; deep-if tail `&mut` borrow conflict); A3 lex-error parity deferred; A4 reads a fixed `input.sentinel` (no argv yet). **The differential-oracle method (D2):** the Rust `snc` gains a canonical stage-dump subcommand per ported stage; the Sentinel stage emits the byte-identical dump; a test diffs both over the `tests/pass` + `tests/ui` corpus. **First sub-phase (D3–D7) = the lexer:** add `snc lex <file>` (a line-oriented token dump `<KIND> <start> <end> [<lexeme>]`, variant *names* not discriminants — D4); write `selfhost/lexer.sentinel` (a new `selfhost/` `.sentinel` tree, growing into a D.6 module graph — D5) reproducing the Rust lexer's 69-variant `TokenKind` stream; a differential test asserts a corpus-wide match (D10). **Back-end-agnostic (Related/D8):** the port is Sentinel *source*, indifferent to merge vs per-unit objects, so it builds on the Path A merge and does NOT gate on the per-unit back end (ADR 0037 follow-up). Out of scope at (1/N): parser+ stages (each its own ADR), lexer *error* parity (follow-on), perf. Indicative split (D9): lexer → parser → resolve → types → HIR/MIR → codegen, each with its own oracle dump. The Rust `snc` stays the production compiler + oracle until the bootstrap fixed-point bakes (ADR 0031 D6). Next: **self-host port (2/N) — the parser** (`snc parse` is the oracle; grow `selfhost/lexer.sentinel` to RETURN a token list the parser consumes; its own ADR).
**ADR 0039 → ACCEPTED-WITH-AMENDMENTS — Phase D self-host port (2/N): the parser-in-Sentinel; (2a) LANDED.** The compiler's biggest stage, so it is explicitly sub-sliced. **Oracle (D2):** the existing `snc parse` `Display` is NOT complete (`Program`'s `Display` omits enums/traits/impls/classes), so add a new `snc ast <file>` — a complete, regular, S-expression-style canonical AST dump (every decl/stmt/expr/type/pattern; node *names* not tags; golden-tested; a dev surface, not abi-v1), which the Sentinel parser reproduces byte-for-byte (diffed over the corpus). **Token model (D3):** refactor `selfhost/lexer.sentinel` to RETURN a token stream as **struct-of-arrays of scalars** (`kinds`/`starts`/`ends`: `Vec<i64>`; lexemes re-sliced from `src`) — dodges the D.3 `Vec<struct-with-[u8]>` drop gap; tags stay internal; the lexer keeps its (1/N) dump so its test stays green. **AST model (D4):** Sentinel recursive enums/structs mirroring `sentinel-ast` — ⚠ the AST is the deepest recursive structure yet, and ADR 0032 A1's box-free recursive-enum drop is UNTESTED at AST scale, so **(2a) gates on a recursive-AST build→dump→drop `leaks` validation** (if it leaks/UAFs, land D.1b payload-ownership first). **Recursive descent (D5)** mirrors the Rust parser (token cursor + precedence-climbing). **Sub-slices (D6):** (2a) lexer-returns-tokens + AST scaffold + drop-validation + minimal expr parser + `snc ast` + seed diff; (2b) full expressions; (2c) statements + fns; (2d) the remaining decls + oracle completeness. Out of scope: resolve+, parser ERROR parity (happy-path first), perf. Reuses ADR 0038 A2 quirk-workarounds (flat per-fn namespace; deep-`if` tail-borrow). **(2a) LANDED** (amendments A1–A3): A1 `snc ast` oracle (run_ast + ast_dump.rs, golden-tested); A2 recursive-AST drop gate (selfhost_ast_drop.sentinel, 0 leaks — no D.1b needed); A3 parser structure settled by probe — `Vec<non-primitive>` unsupported (AST = recursive `Expr` enum returned by value + consuming-dumped, NOT an arena), refs index via explicit `(*r)[i]` (auto `r[i]` fails; the recursion enabler), left-assoc folds via recursion not loops (moved-in-loop rule), match arms need commas. `selfhost/parser.sentinel` (the 2nd Sentinel stage) parses paramless `fn`-bodied integer arithmetic → matches `snc ast` (tests/selfhost_parse.rs, 5 seeds, leak-free). Next: **(2b)** full expressions (vars/calls/if/match/struct-lit/…), then (2c) stmts+fns, (2d) decls — each growing the parser + diff corpus toward the full tests/pass+tests/ui set.
**Phase D self-host port (2/N) parser — (2b) increment-1: full operator-precedence expressions — complete (`0e84f36`). ADR 0039 amendment A4.** D6's "(2b) full expressions" row spans ~28 `ExprKind` variants, so (2b) is itself sub-sliced; **increment-1** grows `selfhost/parser.sentinel` from (2a)'s integer arithmetic to the **complete operator-precedence ladder**, mirroring the Rust parser exactly (`parse_expr → or → and → cmp → bitor → bitxor → bitand → add → mul → unary → atom`) so the AST *tree shape* — hence the `snc ast` dump — matches byte-for-byte. New surface vs (2a): logical `|| &&` (short-circuit precedence), the six **non-associative** comparisons `== != < <= > >=`, bitwise `| ^ &` (`&`>`^`>`|`), prefix unary `- !`, and the **scalar atom leaves** — integer / `true` / `false` / `null` literals + variable references (plus the existing int + parens). The `Expr` enum gains `Bool(bool)` / `Null` / `Var([u8])` / `Unary(i64, Expr)` + a unified `Binary(i64, Expr, Expr)` whose i64 **op-code** encodes both the dump category (`binop`/`cmp`/`logic`) and the operator symbol; the consuming recursive dump maps it back. `true`/`false`/`null` lex as identifiers but parse to the literal nodes the oracle emits (in-place byte compare, no allocation), **never** `(var …)`. The internal tokenizer is extended to longest-match all the new operators (`==`/`=`, `!=`/`!`, `<=` `>=` `&&` `||` plus `| ^ &`), keeping `->` + the (2a) set. Reuses every proven idiom (ADR 0038 A2 / ADR 0039 A3): recursive `Expr` by value + consuming `match` (no `Vec<Expr>`); shared token arrays + `src` indexed via `(*r)[i]`; a `&mut i64` cursor; left-assoc folds via `parse_X_rest` accumulator recursion (a loop accumulator trips moved-in-loop); flat per-fn unique locals; the dump computes prefix+symbol as `[u8]` values FIRST then emits (sibling `&mut out` borrows in `if` tails read as overlapping). **Verified:** the differential test now diffs **26 seeds** spanning every precedence level (incl. two interleaving the whole ladder) against `snc ast` — all byte-identical; leak-free under `leaks --atExit` (the recursive `Var` / `Unary` / `Binary` payloads drop via the consuming dump). **1402 tests, four-check green.** **Deferred to later (2b) increments:** postfix (call / index / field / method), `if`/`match` expressions, struct/array literals, perform/handle, qualified-call / class-init / scope / spawn / await / declassify; then (2c) statements + fns-with-params/blocks, (2d) the top-level decls.
**Phase D self-host port (2/N) parser — (2b) increment-2: function calls + the postfix chain — complete (`1b7d17c`). ADR 0039 amendment A5.** Adds, mirroring the Rust parser, free calls `f(args)` → `(call f …)` (an *atom* case — the callee is a NAME, not an expr; only a postfix `.m(args)` calls a value) and the **postfix chain** applied left-to-right over an atom: field `t.field` → `(field t field)`, index `t[i]` → `(index t i)`, method `t.m(args)` → `(method t m …)`. A new `parse_postfix` layer sits between `parse_unary` and `parse_atom` (`parse_unary` now falls through to it). **The data-model call:** an argument list is variadic and `Vec<non-primitive>` is unsupported, so args are a **second enum `Args = End | Cell(Expr, Args)`, mutually recursive with `Expr`** (`Expr` gains `Call([u8], Args)` / `Method(Expr, [u8], Args)` / `Field(Expr, [u8])` / `Index(Expr, Expr)`) — extending (2a)'s single-self-recursive-enum drop gate to **two mutually-recursive enums + enum-typed payloads**, which was **DE-RISKED by a probe first** (build → consuming-dump → `leaks`: compiles, correct, 0 leaks) before growing the parser, the same probe-first discipline as the (2a) structure work. `parse_args` builds the cons-list head-first by recursion + consumes the closing `)`; the postfix chain folds via `parse_postfix_rest` accumulator recursion (a loop accumulator trips moved-in-loop); the tokenizer gains `.` `[` `]` `,` (tags 26–29). **Verified:** the differential test now diffs **45 seeds** — calls (zero/one/many/expr/nested args), field/index/method, and chains like `a.b(c)[d].e` and `x.foo(1).bar[k].baz(y, z)` — all byte-identical to `snc ast`; leak-free under `leaks --atExit` (the `Args` cons-list + nested `Expr`s drop via the consuming dump). **1402 tests, four-check green.** **Still deferred to later (2b) increments:** the `::` paths (qualified-call / class-init / enum construction), struct + array literals, `if`/`match`, perform/handle, scope/spawn/await/declassify; then (2c) statements + fns-with-params/blocks, (2d) the top-level decls.
**Phase D self-host port (2/N) parser — (2b) increment-3: `::` paths + array literals — complete (`aa3307a`). ADR 0039 amendment A6.** Adds the identifier-prefixed `::` forms (parsed in `parse_atom` after an ident) + array literals, all reusing A5's `Args` cons-list: `Name::method(args)` → `(qcall Name method …)`; `Name::init(args)` → `(class-init Name …)` (the `init` name **with parens** is the only class-init form); a **paren-less** `Name::tail` (e.g. bare enum-unit `Enum::Variant`) → a qualified call with empty args — the enum-vs-impl meaning is a *resolve* concern, so the parser emits a uniform `(qcall …)` (matching the Rust parser). An **atom-position `[`** is an array literal `[e1, e2, …]` → `(array …)`, distinct from the **post-atom `[`** index operator (A5) by position (`parse_atom` vs `parse_postfix_rest`). `Expr` gains `Qcall([u8], [u8], Args)` / `ClassInit([u8], Args)` / `Array(Args)`; `parse_args` is generalised with a **terminator-tag** param (`)`=5 for call args, `]`=28 for array elements); the tokenizer gains `::` (30) + `:` (31); an `is_kw_init` slice-compare picks `init` (the self-contained tokenizer has no `init` keyword). **Verified:** the differential test now diffs **59 seeds** — qcall (with/without args/parens), class-init (with/without args), bare-init→qcall, arrays (empty/expr elems), array-then-index `[1,2][0]`, and deep nests like `g(A::b(x), [1, h(3)], Point::init(y, z))` and `[A::b(), c.d][0].e` — all byte-identical to `snc ast`; leak-free under `leaks --atExit`. **1402 tests, four-check green.** **Still deferred to later (2b) increments:** struct literals, `if`/`match`, perform/handle, scope/spawn/await/declassify; then (2c) statements + fns-with-params/blocks, (2d) the top-level decls.
**Phase D self-host port (2/N) parser — (2b) increment-4: `if`-expressions + brace blocks — complete (`4837622`). ADR 0039 amendment A7.** Adds the first **control-flow expression** + the **block** machinery: `if <cond> { <then> } else { <else> }` → `(if cond (block then) (block else))`, and a brace block `{ <expr> }` → `(block expr)`. `if` is dispatched at the **TOP of `parse_expr`** (so it is a full expression, never an operator operand — matching the Rust parser, whose `parse_add` operand is `parse_mul`, not `parse_expr`); `else` is **mandatory** (Sentinel has no bare `if`), and `else if` chains by wrapping the inner `if` in a block (matching the oracle). A brace block is also a `parse_atom` case. **Blocks are statement-FREE for now** — `BlockE(Expr)` holds just the tail; the full statement list lands at (2c), when `BlockE` grows a statement cons-list. `if` / `else` are **tagged in the tokenizer** (32 / 33) like `fn` (new `is_kw_if` / `is_kw_else`), so the parser dispatches + consumes them by tag. `Expr` gains `If(Expr, Expr, Expr)` + `BlockE(Expr)`; `parse_block` + `parse_if` (recursing for `else if`). **Verified:** the differential test now diffs **68 seeds** — basic `if`, cond exprs, `else if` chains, nested `if`, brace blocks, and `if` inside call args / array elements — all byte-identical to `snc ast`; leak-free under `leaks --atExit`. **1402 tests, four-check green.** **Still deferred to later (2b) increments:** `match` (the last control-flow expr — adds a `Pattern` enum + `parse_pattern` + arms), struct literals (need the Rust `allow_struct_lit` flag), perform/handle, scope/spawn/await, declassify.
**Phase D self-host port (2/N) parser — (2b) increment-5: `match` expressions + patterns — complete (`6e89d2a`). ADR 0039 amendment A8.** Adds the last control-flow expression: `match <scrutinee> { pat => body, … }` → `(match scrut (arm pat body)…)`, dispatched at the **top of `parse_expr`** alongside `if` (a `match` keyword tag); arms comma-separated (trailing comma allowed); arm **bodies are expressions** (`parse_expr`, not blocks — matching the Rust `parse_match_arm`). Patterns are the `_` wildcard → `(pat _)` or a qualified variant `Enum::Variant` with an optional **positional binding list** → `(pat Enum Variant b1 b2)` (each binding an ident, itself possibly `_`). **The data model is the deepest mutual recursion yet — four enums in a cycle** (`Expr → Arms → {Pattern → Binds, Expr}`): `Expr` gains `Match(Expr, Arms)`; new `Arms = ArmEnd | ArmCell(Pattern, Expr, Arms)`, `Pattern = PatWild | PatVariant([u8], [u8], Binds)`, `Binds = BindEnd | BindCell([u8], Binds)` — **de-risked by a probe first** (build → consuming-dump → `leaks`: 0 leaks), as with A5's `Args`. `parse_match` / `parse_arms` / `parse_pattern` / `parse_binds` build them by recursion (the cons-lists consume their closing bracket); the tokenizer gains the `match` keyword (34) + `=>` FatArrow (35) + `is_kw_match` / `is_wildcard`. **Verified:** the differential test now diffs **78 seeds** — multi-arm, single/multi/wildcard bindings, match-on-call scrutinee, if/match/call arm bodies, nested `match`, `match` in call args, trailing comma, and an AST-walker shape `match parse(t) { Node::Bin(op, l, r) => eval(l) + eval(r), … }` — all byte-identical to `snc ast`; leak-free under `leaks --atExit`. **1402 tests, four-check green.** **Still deferred to later (2b) increments:** struct literals (need the Rust `allow_struct_lit` flag), perform/handle, scope/spawn/await, declassify.
**Phase D self-host port (2/N) parser — (2b) increment-6: struct literals — complete (`4935335`). ADR 0039 amendment A9.** `Name { f1: e1, f2: e2 }` → `(struct-lit Name (field f1 e1) (field f2 e2))`. **The disambiguation is the story.** The Rust parser threads a stateful `allow_struct_lit` flag through the *whole* expression descent (set `false` in an `if`/`while`/`match` head so `if x { … }` reads `x` as the cond, not `x { … }` as a struct lit). Rather than thread a `bool` through ~19 parse functions, the port uses a **context-free lookahead `{ Ident :`** — a brace, an identifier, then a **single** colon — which *only ever* begins a struct literal (no block / match-body / if-body starts with a single-colon `Ident :`: no statement form is `name :`, and variant patterns use `::`). On all clean-parsing input this yields the identical AST to the flag, with **no threading** — the same "different implementation, byte-identical output" trade the lexer made. Verified that heads stay conditions: `if x { P { a: 1 } } else { Q { b: 2 } }`, `match s { St::A => P { v: 1 }, … }`, `(P { x: 1 }).x`. `Expr` gains `StructLit([u8], Fields)`; new `Fields = FieldEnd | FieldCell([u8], Expr, Fields)`; `parse_fields` parses `name : value` pairs (trailing comma OK); no tokenizer change (`:` already tag 31). **Documented limitation:** an **empty** struct literal `Name {}` is deferred (no `field :` to key on; collides with an empty `match`/`while` body — the flag is the eventual fix). **Verified:** the differential test now diffs **88 seeds** — single/multi-field, expr values, nested structs, structs in call args / arrays, trailing comma, and the head-disambiguation seeds — all byte-identical to `snc ast`; leak-free under `leaks --atExit`. **1402 tests, four-check green.** **Still deferred to later (2b) increments:** perform/handle, scope/spawn/await, declassify.
**Phase D self-host port (2/N) parser — (2b) increment-7: declassify / perform / scope / spawn / await — complete (`af76636`). ADR 0039 amendment A10.** The effect/concurrency leaf expressions: `declassify(e)` → `(declassify e)`; `perform Eff.op(args)` → `(perform Eff op args…)`; `scope concurrent { block }` → `(scope (block …))`; `spawn <postfix>` → `(spawn …)`; and the `.await` **postfix** → `(await target)`. `declassify`/`perform`/`scope`/`spawn` are keyword-led atom cases in `parse_atom` (`scope` skips the positional `concurrent` ident then `parse_block`; `spawn` parses its target via `parse_postfix`; `perform` reuses `parse_args`); `.await` is checked right after the `.` in `parse_postfix_rest`, before the field/method dispatch. Five new keyword tags (declassify 36 / perform 37 / scope 38 / spawn 39 / await 40) + `is_kw_*` helpers; `Expr` gains `Declassify`/`Perform([u8],[u8],Args)`/`Scope`/`Spawn`/`Await`. **The `scope` (and `while`) body stays statement-free** until (2c): a body with a `;`-separated statement — and the `;` token itself — is (2c) territory, so the seeds use statement-free bodies. **Verified:** the differential test now diffs **102 seeds** — each form alone + composed (`g(perform E.op(1))`, `declassify(x) + perform E.op()`, a `match` arm `declassify(perform Tls.verify(mac))`, `if ready { spawn w(x) } else { compute().await }`, `scope concurrent { declassify(perform Net.recv()) }`) — all byte-identical to `snc ast`; leak-free under `leaks --atExit`. **1402 tests, four-check green.** **Remaining in (2b):** only `handle … with { arms }` — which closes the expression grammar.
**Phase D self-host port (2/N) parser — (2b) increment-8: `handle` — the EXPRESSION GRAMMAR IS COMPLETE — (`189990e`). ADR 0039 amendment A11.** `handle <body> with { Eff.op(params) => arm, … return v => arm }` → `(handle body (arm Eff op armbody)… (return v body))` (a `parse_atom` keyword case). **Two subtleties, both handled faithfully:** (i) handler-arm **params are parsed but NOT dumped** (skipped to the closing `)`; the dump is just `(arm Eff op body)`); (ii) the optional `return v => body` arm is kept **separate** from the handler-arm list and **dumps LAST** regardless of source position — the arm parse fills a **`&mut Ret` out-param** when it sees `return` (mirroring the Rust `return_arm`). That out-param **assigns an enum through a `&mut` ref** (`*ret = Ret::YesRet(…)`) — the first non-primitive `&mut` assignment in the port (the cursor is `&mut i64`); **de-risked by a probe** (compiles, runs, leak-free; return-not-last verified to still dump last). `Expr` gains `Handle(Expr, HArms, Ret)`; new `HArms = HEnd | HCell([u8],[u8],Expr,HArms)` and `Ret = NoRet | YesRet([u8],Expr)`; three keyword tags (handle 41 / with 42 / return 43). **Verified:** the differential test now diffs **110 seeds** — single/multi-arm handlers, return arms (incl. return-not-last + empty params), and composed forms (`g(handle …)`, `handle perform … with …`) — all byte-identical to `snc ast`; leak-free under `leaks --atExit`. **1402 tests, four-check green.** **(2b) the full expression grammar is COMPLETE** — operators, atoms, calls, postfix, `::` paths, arrays, `if`/blocks, `match`/patterns, struct literals, declassify/perform/scope/spawn/await, handle. **Next: (2c)** statements (`let`/assign/`while`/`break`/`continue`/expr-stmt — turning the statement-free `BlockE` into a real block) + `fn`-with-params/return-type/effect-row; then **(2d)** the top-level decls.
**Phase D self-host port (2/N) parser — (2c-1): statements + real blocks — complete (`b293c62`). ADR 0039 amendment A12.** A block is now `{ <stmt>* <tail> }` → `(block <stmt>… <tail>)`. Statements: `let [mut] name = e` → `(let [mut] name _ e)` (the `_` is the not-yet-supported type annotation — (2c-2)); `target = e` → `(assign target e)`; `while cond { body }` → `(while cond (block …))`; `break` → `(break)`; `continue` → `(continue)`; an expr-statement → `(expr e)`. The block loop (`parse_stmts`) mirrors `parse_block_inner` — dispatch `let`/`while`/`break`/`continue` by keyword tag, else parse an expr and classify by what follows (`=` → assign-stmt, `;` → expr-stmt, else → the tail). **The block tail is a `&mut Expr` out-param** (the A11 `&mut Ret` technique) defaulting to a **nullary `Expr::SynthZero`** that dumps `(int 0)` — the oracle's synth unit tail for a statement-only `while` body. **⚠ Leak found + fixed in-flight:** the default was first `Expr::Int(0)`, whose `i64` payload is heap-**boxed**, and `*tail = ex` through the ref doesn't free the old enum (consistent with A11 — `NoRet` is nullary), so the boxed `Int(0)` leaked once per overwritten tail (2 leaks / 32 bytes). A nullary default is leak-free + dumps identically. **Reusable rule:** a `&mut Enum` out-param's default must be a payload-free variant. `Expr` `BlockE(Expr)` → `Block(Stmts, Expr)` + `SynthZero`; new `Stmts`/`Stmt` enums; the fn body is now a real block. New tokens: `;` (44) + let/mut/while/break/continue (45–49). **Verified:** the differential test now diffs **122 seeds** — let/assign/expr-stmt, `while` with break/continue/assign bodies, statement-only `while` bodies (synth tail), nested statements, composites — all byte-identical to `snc ast`; leak-free under `leaks --atExit`. Existing statement-free seeds unchanged (a zero-statement block dumps identically). **1402 tests, four-check green.** **Next: (2c-2)** `let` type annotations + a `parse_type`; **(2c-3)** `fn` definitions.
**Phase D self-host port (2/N) parser — (2c-2): `let` type annotations + a `parse_type` — complete (`5f47546`). ADR 0039 amendment A13.** The optional `: type` on a `let` → `(let [mut] name <type> e)` (vs `_` when absent). `parse_type` mirrors the Rust one: `secret T` → `(secret T)`; `&T`/`&mut T` → `(ref T)`/`(refmut T)`; `?T` → `(opt T)`; `[T]` → `(arr T)`; `Ident` → the name; `Ident<args>` → `(generic Ident args…)` (the generic arg list is a cons-list terminated by `>`). **Nested generics close without a `>>` split:** the tokenizer has no `>>`, so `Vec<Box<i64>>` lexes its trailing `>>` as two `Gt` tokens and each `>` closes one level (the Rust parser needs an explicit `Shr`-into-two-`>` split; the port sidesteps it). New recursive `TypeE` enum + `TyArgs` cons-list + a `TyOpt` (`NoTy`/`SomeTy`); `Stmt::SLet` gains the `TyOpt` field. New tokens: `?` (Question 50) + the `secret` keyword (51). In `parse_let`, after the name a `:` opens the annotation. **Verified:** the differential test now diffs **135 seeds** — i64/bool idents, `[u8]` arrays, `Vec<i64>`/`Map<i64,[u8]>`/`Box<Vec<i64>>` generics (incl. nesting), `?T`, `&T`/`&mut T`, `secret T`, `secret [u8]`, mixed annotated/un-annotated lets — all byte-identical to `snc ast`; leak-free under `leaks --atExit`. **1402 tests, four-check green.** **Next: (2c-3)** `fn` definitions (params/return-type/effect-row — reusing `parse_type`); then **(2d)** the top-level decls.
**Phase D self-host port (2/N) parser — (2c-3): `fn` definitions — closes the fn-level grammar — complete (`b2a9c3b`). ADR 0039 amendment A14.** `main`'s hard-coded paramless `fn NAME() -> TYPE` header is replaced by a real fn parse: `fn name <type-params>? ( [mut] p: T, … ) -> RET ! { eff, … }? { body }` → `(fn name ((param [mut] p <type>) …) <ret> <block>)`. The param list is a `Params` cons-list; **the param-list dump has no leading space before the first param**, so a first/rest split (`dump_params` + `dump_params_rest` over a shared `dump_param_body`). The `-> RET` return type now routes through `parse_type`/`dump_type` (so a non-`Ident` return like `[u8]`/`?T`/`Vec<T>`/`secret T` dumps right — previously dumped raw via `append_slice`). **Generic type-params `<…>` + the postfix effect row `! { … }` are parsed-and-SKIPPED** — `dump_fn` emits neither (confirmed against `ast_dump.rs`); `skip_type_params` is depth-balanced over `<`/`>`, `skip_effect_row` skips to the `}`. No new tokens. **⚠ Sentinel-`if`-is-an-expression reminder:** `skip_type_params` first used statement-only `if` branches + a bare `if` (no `else`) → compile error ("blocks must end with an expression"); rewrote it as `depth = if … { depth+1 } else if … { depth-1 } else { depth }` inside a `while depth > 0` loop. **Verified:** the differential test now diffs **148 seeds** — params (single/multi/`mut`), `[u8]`/`?T`/`Vec<T>`/`secret` return types, ref params, generic fns (type-params skipped), effect-row fns (skipped), multi-fn programs, and a composite generic+secret+effect+statements handler — all byte-identical to `snc ast`; leak-free under `leaks --atExit`. **This CLOSES the fn-level grammar** — every `Stmt`/`Expr`/`TypeExpr`/`Pattern` + the fn header now parse. **1402 tests, four-check green.** **Next: (2d)** the top-level decls (struct/enum/trait/impl/class/effect/use) + completing `snc ast`'s `Program` dumper for them — the last parser slice.
Phase C2 (regions + refs + mutability + borrow check + RAII drop
per HANDOVER §6.2 / §6.3) is **complete** per ADR 0017 (now
ACCEPTED-WITH-AMENDMENTS, 6 sub-phases, ~6 effective sessions
actual vs ADR 0017 D9 estimate "6-13 sessions" — low end of
the range). ADR 0018 (Polonius migration plan) PROPOSED;
records the plan only, no migration code yet. Phase C3 (effect
system + secret typing per ADR 0019) is **typing-layer
complete** as of C3.3 — ADR 0019 ACCEPTED-WITH-AMENDMENTS with
all D-decisions exercised except D8 (handler runtime, deferred
to ADR 0020) and D9 (async, deferred indefinitely). Phase C3
runtime layer (ADR 0020) is **complete** per ADR 0020 (now
ACCEPTED-WITH-AMENDMENTS, 8 sub-phases C3.4 → C3.7,
~9 effective sessions vs ADR estimate "5-9 sessions"; D2
multi-shot relaxation deferred indefinitely per the
amendment). All twelve D-decisions exercised modulo D2: D1
free-monad lowering, D2 one-shot (multi-shot deferred), D3
deep-handler + propagation, D4 surface + default + non-
identity return arm, D5 AST + parser + resolve, D6 effect
discharge, D7 all 5 runtime symbols, D8 op-arg packing (single
i64 at MVP), D9 sub-phase split, D10 out-of-scope deferrals,
D11 main integration, D12 phase-go (c37_go_no_go +
c37_handle_return + c37_perform_outside_handle). The handler
surface compiles and runs end-to-end for: direct-perform
bodies, fn-call-that-performs bodies (effecting fn ABI returns
Kont*), pure-bodied effecting fns (PURE_RETURN wrap), let-
bound performs, any pure surrounding context with a single
embedded perform (binop, struct-lit, fn-call-arg, index, etc.),
chained effecting lets where each let's RHS is a direct
perform / effecting call, `return v => body` arms that
transform the resumed/returned value per Phase B's deep-
handler re-wrap, nested handles where an inner handle's un-
caught op propagates to the outer handle's dispatch, and
**any-expression handle body** including pure i64 wrapped via
sentinel_kont_pure. Five runtime symbols in place:
sentinel_perform_op, sentinel_kont_resume (bubble-aware,
returns Kont*), sentinel_kont_panic_resumed, sentinel_kont_pure
+ sentinel_kont_consume_pure, sentinel_kont_push. Pipeline at
Phase C3 close (unchanged since C3.3): parse → resolve →
check → **effect_check** → borrow_check → codegen.
Phase C1 (type system per HANDOVER §6.2) is **complete** per ADR
0011 (now ACCEPTED, 8 sub-phases, ADR's honest 5-6 month estimate
beaten — actual elapsed across C1.0a through C1.7.4b was ~10-12
sessions, ~5-6x faster than estimated; the infrastructure
investment compounded). All eight sub-phases landed:

  - **C1.0a** (09dc8c3): foundation crate `sentinel-base` hosting
    the `#[salsa::db]` SentinelDb trait, `#[salsa::input]`
    SourceFile, and `#[salsa::accumulator]` Diagnostic. Salsa 0.18
    is in the dep graph.
  - **C1.0b** (557cc60): `sentinel-syntax::query` exposes
    `lex_query` and `parse_query` as `#[salsa::tracked]` queries.
    Errors route through the Diagnostic accumulator. sentinel-driver
    instantiates a concrete `SentinelDatabase`.
  - **C1.0c** (8b58644, decision-only commit): codegen stays
    outside the salsa query graph through Phase C1.0. Three options
    weighed in the ADR 0011 D1 amendment; chosen option is "don't
    wrap codegen at all" because (a) it gets rewritten at C1.2 for
    typed HIR, (b) LLVM `'ctx` lifetimes don't trivially fit
    salsa's query model, (c) LSP/check-only tooling that wants
    incremental rebuild exits after types-not-codegen anyway.
  - **C1.1.1** (438dd16): sentinel-resolve crate populated.
    VarId(u32) / FnId(u32) stable identifiers; FnSignature lookup
    table; parallel-tree resolved AST mirroring sentinel-ast's
    Program shape with name references replaced by IDs;
    ResolveError with the 6 name-resolution variants migrated from
    CodegenError; pure `resolve(program)` entry point;
    `resolve_query(db, file)` `#[salsa::tracked]` wrapper chaining
    on parse_query. 21 unit tests (positive paths + each error
    variant + 4 salsa query smoke).
  - **C1.1.2** (9374edf): codegen consumes &ResolvedProgram;
    driver pipeline becomes parse_query → resolve_query → codegen.
    Codegen loses ~200 lines (name resolution is gone).
  - **ADR 0012** (6ab3661, PROPOSED): concrete C1 surface syntax
    — annotation grammar (D1-D4 for C1.2), bool/comparison/logical
    operators (D5-D8 for C1.3), lexer additions (D9), hard-break
    fixture rewrite plan (D10).
  - **C1.2.1** (af16655): lexer `:` token landed per ADR 0012 D9.
  - **C1.2.2** (90965a5): AST gains `TypeExpr` (Spanned<Ident at
    C1.2>), `FnDef.return_type`, `Param.ty`, `StmtKind::Let.ty_annot`
    (Option); parser wires `fn name(p: T) -> T` and optional
    `let x: T = ...`; resolve carries TypeExpr through;
    22 pass-test fixtures + 1 UI fixture mechanically rewritten
    via committed Python script per ADR 0012 D10.
  - **C1.2.3** (ded07bc): sentinel-types crate populated.
    `Type::I64` universe (C1.3 widens), parallel-tree TypedProgram
    with `ty: Type` on every TypedExpr, 4 TypeError variants
    (UnknownType active at C1.2; Mismatch / ReturnTypeMismatch /
    CallArgMismatch dormant until C1.3 multi-type expressions),
    pure `check()` + `#[salsa::tracked]` `check_query`.
  - **C1.2.4** (c9a21ff): codegen consumes &TypedProgram; driver
    pipeline becomes parse_query → resolve_query → check_query
    → codegen. `check_query::accumulated::<Diagnostic>` picks up
    lex / parse / resolve / types diagnostics transitively — full
    four-stage front-end is now query-shaped.
  - **C1.3.1** (2801a81): lexer adds the 11 C1.3 tokens per ADR
    0012 D9 — `true` and `false` keywords, six comparison ops
    (`== != < <= > >=`), three logical ops (`&& || !`). logos's
    longest-match handles the precedence-aware lexing
    (`==` beats `=`, `!=` beats `!`, `<=` beats `<`, `>=` beats
    `>`). 9 new lexer tests.
  - **C1.3.2-4** (cd1c0d4): the bool + comparison + logical
    surface lands end-to-end as a single coordinated commit
    because the AST / resolve / types / codegen parallel-tree
    enums need their exhaustive matches updated together.
    AST gains `ExprKind::BoolLit(bool)`, `ExprKind::Cmp(CmpOp,
    l, r)`, `ExprKind::Logic(LogicOp, l, r)`, `UnaryOp::Not`,
    new `CmpOp` and `LogicOp` enums. Parser inserts the
    or → and → cmp precedence levels per ADR 0012 D7;
    comparisons are non-associative per D6 (chained cmp surfaces
    as `ParseError::ChainedComparison`). Resolve passes the new
    variants through unchanged. Types widens its universe to
    `{ I64, I32, Bool }`; operator-typing rules per ADR 0012 (arith
    rejects bool; cmp same → Bool; logic Bool, Bool → Bool;
    unary `!` Bool → Bool). Codegen drops `i64_type` from the
    ctx in favour of an `llvm_int_type(Type)` helper that picks
    between `i1` / `i32` / `i64`; vars HashMap stores
    `(PointerValue, Type)`; comparisons lower via
    `build_int_compare` with the right `IntPredicate`; logicals
    lower as PHI-based short-circuit; unary `!` is `xor x, 1`.
    Activates the dormant `Mismatch` / `ReturnTypeMismatch` /
    `CallArgMismatch` variants from C1.2. +49 unit tests
    (+7 ast / +21 parser / +18 types / +10 codegen) — clean
    pipeline change with no behavior regressions.
  - **C1.3.5** (ba5fd9d): retires ADR 0010 D9's C-style truthy.
    Type checker now requires `cond.ty == Bool` for `if`;
    codegen drops the legacy compare-NE-zero path (debug_assert
    pins the invariant). Six C0 if-using fixtures rewritten
    mechanically: `if 1` → `if true`, `if 0` → `if false`,
    `if x` (x: i64) → `if x != 0`; c05_go_no_go restructured to
    use `is_positive(x): bool` + `pick(cond: bool, ...)` per
    the ADR 0012 appendix's C1.3 phase-go shape. Seven new
    c13_* pass-test fixtures land (bool_literal, comparison,
    logical_and/or, unary_not, short_circuit_and/or). The
    short-circuit fixtures specifically pin the PHI-based
    codegen — if a future change ever regresses to eager
    evaluation, the side effect of the skipped `print(99)` will
    surface in stdout and the test fails. +8 tests over step 2
    (+1 types if_condition_rejects_non_bool + 7 c13 fixtures).
  - **ADR 0013** (e93635b, PROPOSED→ACCEPTED): concrete C1.4
    surface — struct declaration grammar (D1), postfix field
    access (D2), struct literal grammar (D3) with parser-
    disambiguation D3a (no struct lit in if-cond, parens
    escape), struct types in type position (D4) extending ADR
    0012 D3's "primitives are identifiers" pattern, nominal
    type equality (D5), struct == deferred to C1.5+ (D6),
    recursive-struct detection at type-check time (D7), lexer
    additions (D8: `struct` keyword + `.` token), tuple /
    unit / derives / methods / generics all out of scope at
    C1.4 (D9, D10), `fn main() -> i64` invariant stays (D11),
    phase-go program spec (D12).
  - **C1.4.1** (f34b401): lexer adds `struct` keyword + `.`
    token per ADR 0013 D8. Two new TokenKind variants. logos
    longest-match not relevant — no `..` / `.=` neighbours
    until ranges arrive at C2+. +4 new lexer tests including
    the `structure` / `structured` ident-prefix-vs-keyword
    regression.
  - **C1.4.2-6** (aa8f252): the struct surface lands end-to-end
    as a single coordinated commit because the parallel-tree
    pattern requires it. AST gains `StructDecl` + `StructField`
    at top-level; `Program` gains `structs: Vec<StructDecl>`
    alongside `fns`. New `ExprKind` variants: `StructLit`
    (Rust-style `Name { field: expr, ... }`), `FieldAccess`
    (postfix `expr.field`). Parser handles all of it with a
    new `allow_struct_lit: bool` mode flag for D3a (forbids
    bare struct lit in if-cond; parens escape). The parser also
    gains a `parse_postfix()` wrapper that consumes `.field`
    chains after any atom — sets up the pattern for C1.6
    arrays' `[index]` and C4 methods' `.method()`. Resolve
    adds `StructId(u32)`, struct table built in pass 0 (before
    fn signatures), `ResolveError::RedefinedStruct` +
    `UndefinedStruct`. Types widens the universe to
    `{ I64, I32, Bool, Struct(StructId) }`; `resolve_type_expr`
    looks up Idents against the struct table; check_expr handles
    StructLit (validates field set matches decl, reorders to
    decl order so codegen iterates by index) and FieldAccess
    (validates target is struct, looks up field index); cycle
    detection emits `TypeError::RecursiveStruct` for direct
    + mutual cycles; four new error variants in total
    (FieldAccessOnNonStruct, UnknownField, MissingField,
    RecursiveStruct). **Codegen's value type widens from
    `IntValue<'ctx>` to `BasicValueEnum<'ctx>`** so struct
    values flow through the same machinery; new pass 0 declares
    LLVM struct types; new `llvm_basic_type` helper replaces
    `llvm_int_type` for the storage-type path (int helper
    retained for arithmetic operand coercion); StructLit lowers
    via `build_insert_value` chain from `get_undef`; FieldAccess
    lowers via `build_extract_value`. +63 tests across all
    crates. Five new c14_* pass-test fixtures land
    (struct_basic, struct_nested, struct_in_if,
    struct_bool_field, c14_go_no_go).

  - **ADR 0014** (3cb1238, PROPOSED→ACCEPTED-WITH-AMENDMENTS):
    concrete C1.5 surface — postfix `?T` type syntax (D1),
    `null` keyword literal (D2) with bidirectional context
    inference, implicit `T → ?T` widening at expression position
    (D3), `Type::Nullable(NullableInner)` flat-subset
    representation (D4 — amended from Box<Type> for Copy
    preservation), bidirectional checking infrastructure (D5),
    no-nested-nullables (D6), `==` / `!=` against `null` (D7),
    lexer additions (D8: `null` keyword + `?` token), generic
    builtins `unwrap_or` / `is_some` (D9), recursive-struct
    cycle-check relaxation (D10 — DEFERRED to C1.6+ because
    `?T = { i1, T }` flat representation can't actually break
    cycles without heap), out-of-scope list (D11: pattern
    matching, force-unwrap `x!`, optional chaining `?.`,
    null-coalesce `??`, `?` propagation, flow typing).
  - **C1.5.1** (dff8642): lexer adds `null` keyword + `?` token
    per ADR 0014 D8. Two new TokenKind variants. The `?` token
    is reserved for type-position only at C1.5; the
    expression-position uses (`?.`, `??`, `x!`, `?`
    propagation) are deferred per D11. +6 new lexer tests.
  - **C1.5.2-6** (1d0adae): the nullable surface lands end-to-end.
    AST gains `ExprKind::NullLit` + `TypeExprKind::Nullable`.
    Parser handles the optional `?` prefix in parse_type with
    `ParseError::NestedNullable` rejection for `??T`; null
    literal recognition in parse_atom. Resolve adds
    `ResolvedExprKind::NullLit` and pre-registers the two
    generic builtins (`unwrap_or` at FnId(1), `is_some` at
    FnId(2)); user fns now start at FnId(3). Types widens the
    universe to `{ I64, I32, Bool, Struct, Nullable }` via the
    flat NullableInner subset enum (D4 amendment — keeps Type
    Copy); adds bidirectional `check_expr(expr, expected:
    Option<Type>, ...)` infrastructure; `coerce_to_expected`
    inserts `TypedExprKind::WidenToNullable` wrappers for the
    implicit T→?T widening; `TypeError::AmbiguousNull` for bare
    `let x = null;`; Cmp rule extended for `x == null` /
    `null == x` comparing discriminator bits; unwrap_or /
    is_some special-cased at the Call typing arm with
    type-from-arg inference. Codegen lowers `?T` as LLVM
    `{ i1, T }`; null lit as const `{ i1 false, T zero }`;
    WidenToNullable via build_insert_value; unwrap_or via
    build_select; is_some via build_extract_value(0); Cmp on
    nullable extracts valid bits and compares. Pass 0 splits
    into "declare opaque struct types" then "set bodies" to
    handle forward references through `?Other` fields. +42
    tests across all crates. Six new c15_* pass-test fixtures
    land (null_literal, widen, eq_null, nullable_struct_field,
    maybe_compose, c15_go_no_go).

  - **ADR 0015** (8924d38, PROPOSED→ACCEPTED-WITH-AMENDMENTS):
    concrete C1.6 surface — `[T]` array type syntax (D1),
    `[e1, e2, ...]` array literal (D2), postfix `a[i]` indexing
    (D3), `len(a) -> i64` builtin (D4), empty array needs
    annotation (D5), `Type::Array(ArrayElem)` flat subset
    representation (D6 — amended to depth-1: NullableInner and
    ArrayElem stay primitive-only, no `?[T]` / `[?T]` at C1.6
    because mutual enum recursion would force Box and break
    Type's Copy), bidirectional element typing (D7), lexer
    additions (D8: `[` + `]`), heap runtime (D9:
    `sentinel_alloc` + `sentinel_panic_oob`, no `free`),
    bounds-check semantics (D10: 0 <= idx < len; panic_oob on
    failure), ADR 0014 D10 unlock implemented (D11: `?Struct`
    codegen switches to heap-indirect `{ i1, ptr }` so
    recursive structs through `?T` work; cycle detector
    relaxes), out-of-scope list (D12: mutable indexing,
    slicing, push/pop, multi-dim, methods, free, ==), fn main
    invariant stays (D13). The ADR 0014 D10 deferral retires
    here.
  - **C1.6.1** (3cfd49f): lexer adds `[` and `]` tokens per
    ADR 0015 D8. Two new TokenKind variants disambiguated by
    the parser into three roles: array type / array literal /
    postfix index. +6 new lexer tests.
  - **C1.6.2-6** (8c5bbbe): the array surface + heap runtime
    + ADR 0014 D10 unlock land end-to-end. sentinel-runtime
    gains `sentinel_alloc` (libc malloc wrapper + abort on
    failure) and `sentinel_panic_oob` (abort with diagnostic).
    AST gains `ExprKind::ArrayLit` + `ExprKind::Index` +
    `TypeExprKind::Array`. Parser handles `[T]` in parse_type,
    `[...]` in parse_atom (with empty-needs-annotation per D5),
    `a[i]` in parse_postfix alongside `.field`. Resolve
    pre-registers `len` builtin at FnId(3); user fns now start
    at FnId(4). Types widens with `Type::Array(ArrayElem)` flat
    subset (D6 amendment — primitives only; no `?[T]` / `[?T]`),
    bidirectional element typing (D7), array literal / index /
    len typing rules (D2/D3/D4), four new TypeError variants
    (AmbiguousEmptyArray, IndexOnNonArray, IndexNotInt,
    NestedArray), and **the cycle-detector relaxation** that
    closes ADR 0014 D10: only direct struct edges contribute to
    cycles; `?Struct` edges break them via heap indirection.
    Codegen: array as `{ i64 len, ptr data }`; ArrayLit lowers
    to alloc+store+insert_value; Index lowers to bounds-check
    + GEP + load (two basic blocks idx_ok/idx_oob); len
    extract_value(0); the `?Struct` representation switches
    from inline `{ i1, T }` to heap-indirect `{ i1, ptr }`;
    `WidenToNullable` for struct types allocates+stores +
    wraps in pointer. +52 tests across all crates. Seven new
    c16_* pass-test fixtures (array_basic, empty_array,
    array_as_arg, array_of_struct, array_in_struct,
    linked_list_node, c16_go_no_go).

  - **ADR 0016** (e411ded, PROPOSED→ACCEPTED): concrete C1.7
    surface — generic fn syntax `fn name<T>(x: T) -> T` (D1),
    generic struct syntax `struct Box<T> { ... }` (D2), type args
    in type position `Box<i64>` (D3), no turbofish at call sites
    (D4) with iterative bidirectional inference, no new lexer
    tokens (D5 — `<` and `>` reused from C1.3 comparisons),
    interned-instance `Type::GenericInstance(GenericInstanceId)`
    representation preserving `Type: Copy` (D6a), monomorphic
    codegen (D7) chosen over witness tables because unbounded
    generics trivialise them to "empty", builtins typing routes
    through the unified inference path (D8a) but codegen stays
    special (D8b — no Sentinel-1.7 source bodies for force-
    unwrap / pattern-matching / runtime-metadata extraction),
    resolve-side type-param scoping with DuplicateTypeParam
    diagnostic (D9), out-of-scope list (D10: bounds, lifetimes,
    HKT, const generics, turbofish, generic methods), `fn main`
    not generic (D11), Pair<A,B> phase-go (D12).
  - **C1.7 scaffolding** (c1e5083): AST + parser + resolve
    infrastructure. AST gains `TypeParam` struct, `type_params:
    Vec<TypeParam>` on `FnDef` / `StructDecl`, and
    `TypeExprKind::Generic { name, args }`. Parser gains
    `parse_type_params` / `parse_type_args` helpers and the
    `Ident<...>` branch in `parse_type`. Resolve gains
    `TypeParamId`, `ResolvedTypeParam`, `DuplicateTypeParam`
    error, and `FnSignature.type_params_count` (builtins flagged
    as generic with count=1). +19 parser tests + 7 resolve tests
    = 777 total.
  - **C1.7.4a** (d32a9fe): types crate generic-fn typing +
    builtin re-route. New `Type::TypeParam(TypeParamId)` +
    matching variants on `NullableInner` / `ArrayElem`. New
    helpers `Type::substitute`, `try_substitute`,
    `contains_type_param`. Builtin signatures rewritten with
    real `Type::TypeParam` (`unwrap_or<T>(?T, T) -> T`,
    `is_some<T>(?T) -> bool`, `len<T>([T]) -> i64`); the
    ~75 LOC of special-cased Call branches in check_expr
    collapse into one unified `check_call`. Iterative
    bidirectional inference handles null literals via
    fixed-point typing (`unwrap_or(null, 0)` works:
    arg[1]=0→I64 binds T=I64; arg[0]=null re-checked with
    expected `?I64`). `TypedExprKind::Call` gains
    `type_args: Vec<Type>`. New error variants: GenericMain,
    AmbiguousTypeArg, TypeArgInferenceConflict,
    GenericStructNotYetSupported (placeholder for C1.7.4b).
    Codegen: skip generic user fn declarations + bodies
    + emit `CodegenError::GenericCallNotYetSupported` at
    user generic-fn call sites pending C1.7.5. +12 types
    tests = 789 total.
  - **C1.7.5** (ad7e10d): codegen monomorphization for user
    generic fns. `TypedFnDef::substitute` deep-clones a generic
    fn with TypeParams substituted to concrete types — the
    monomorphic def looks no different from a non-generic fn
    to compile_fn. Worklist algorithm
    (`collect_mono_instantiations`) walks non-generic fn bodies
    seeding instantiations, then transitively processes each
    pending instance under its substitution. Per-instance LLVM
    fn declaration with mangled name (`id__i64`, etc.) via
    `mangle_mono_name` + `mangle_type`. Builtin lowering stays
    inline per ADR 0016 D8b. Four new c17 fixtures
    (c17_id stdout "42", c17_two_instantiations "41",
    c17_generic_nullable "100", c17_generic_array "6"). +4
    pass tests = 793 total.
  - **C1.7.4b** (2c6c652): generic structs end-to-end + ADR
    0016 D12 phase-go. New `Type::GenericInstance(GenericInstanceId)`
    variant + interner table on TypedProgram. NullableInner /
    ArrayElem gain `GenericInstance` variants (partially
    closing the ADR 0015 D6 deferral: `?Box<i64>` and
    `[Box<i64>]` now work; `?[T]` and `[?T]` stay deferred).
    `Type::substitute` extended to take `&mut Vec<GenericInstanceData>`
    for interner-extending substitution. Same threading
    through TypedFnDef/Block/Stmt/Expr::substitute. New
    `check_call` extensions: unify_one recurses into
    GenericInstance args; bidirectional pushdown extended for
    generic-instance returns (so `fn make_pair<A, B>(...) -> Pair<A, B>
    { Pair { ... } }` works). Codegen pass 0 splits into
    declare-then-set-bodies passes for both regular structs and
    generic-struct instances, with abstract instances
    (TypeParam-using args) filtered out via
    `arg_contains_typeparam`. Two new fixtures (c17_box stdout
    "42", c17_go_no_go stdout "42" — the full ADR 0016 D12
    Pair<A,B> + make_pair / fst / snd / pick_int program). +5
    types tests + 2 pass tests = 798 total.

  - **C1.7 docs commit** (4028dd7): STATE.md banner refresh +
    HANDOVER §0 close-out + ADR 0011/0016 flips to ACCEPTED.

  - **ADR 0017** (ea4bcfd, **ACCEPTED-WITH-AMENDMENTS at
    C2.5 close**): Phase C2 kickoff. 14 D-decisions covering
    reference syntax (`&T` / `&mut T` per D1), mutability
    (`let mut` + `mut` param prefix per D2), borrow-take +
    dereference (`&expr` / `&mut expr` / `*expr` per D3 / D4),
    lvalue / rvalue distinction (D5), borrow-checker formulation
    (lexical first, Polonius later per D6), region representation
    (lexical only at C2 minimum per D7; named regions deferred),
    drop / RAII (auto-drop at scope exit + `sentinel_free` per
    D8 closes the C1.6+ heap-leak deferral), move semantics +
    use-after-move (D9), lexer additions (D10), interned
    `Type::Ref(RefId)` (D11 — the fifth ADR running to preserve
    `Type: Copy` via internment), out-of-scope items (D12),
    `fn main` invariant (D13), and phase-go program spec (D14).
    Sub-phase split table: C2.0 (infrastructure) → C2.1
    (shared borrow checker) → C2.2 (`&mut` + XOR — the largest)
    → C2.3 (move semantics) → C2.4 (RAII + drop) → C2.5
    (Polonius migration plan + STATE.md / HANDOVER close-out).
    Three amendments at C2.5: A1 the C2.4 recursive-field-drop
    closure slipped to C2.5(a); A2 the Polonius plan shipped as
    standalone ADR 0018; A3 partial-move-through-field-projection
    soundness gap documented for closure in a follow-on sub-phase.

  - **ADR 0018** (PROPOSED): Polonius migration plan — lexical
    → flow-sensitive borrow check via `polonius-engine 0.13`.
    Six D-decisions: D1 (trigger: empirical friction not
    principle); D2 (preserved surface: BorrowError variants +
    DropPlan + pipeline shape stay); D3 (adopt polonius-engine
    library; vendor-fork fallback); D4 (representation changes:
    CFG + origins + loans + liveness); D5 (three-step rollout:
    fact generator → output lowering → flip default); D6 (out-
    of-scope: field-precise borrows + first-class refs +
    closures + traits). No migration code at C2.5; ADR records
    the plan only.

  - **C2.5** (this session): Phase C2 close-out. Four
    deliverables: (a) recursive struct + generic-instance
    field drop closes the C2.4 known gap — `emit_drop_struct_fields`
    now threads `&TypedProgram` and iterates `program.struct_decl(id).fields`
    (substituting through `program.generic_instance(id).args` for
    generic instances), with a `field_type_needs_drop(ty, program)`
    helper short-circuiting pure-data fields and a cycle guard
    for `?Struct` recursion. (b) ADR 0018 ships the Polonius
    migration plan. (c) `docs/borrow-check-limitations.md`
    documents two known lexical over-rejections + a soundness
    gap (partial move through field projection + drop ⇒ double-
    free). (d) STATE.md banner + HANDOVER §0 + ADR 0017 flip.
    +4 c25 fixtures: c25_struct_field_drop (stdout "19"),
    c25_nested_struct_drop ("607"), c25_generic_struct_array_drop
    ("66"), c25_go_no_go ("190" — the D14 phase-go combining
    XOR + move + recursive field drop). Workspace test count
    935.

  - **C2.0.1** (d7b18c2): lexer adds `&` (Amp) token + `mut`
    keyword per ADR 0017 D10. The `*` token already exists for
    multiplication from C0; per D10 the parser disambiguates
    dereference vs multiplication positionally at C2.0.2. No
    `'a` lifetime syntax at C2 minimum per D7. logos longest-
    match handles `&&` (AmpAmp) staying a single token. +10
    new lexer tests = 808 total.

  - **C2.4** (8d72679): RAII / drop + `sentinel_free` runtime
    symbol per ADR 0017 D8. **Closes the C1.6+ heap-leak
    deferral** that's been open since arrays + `?Struct`
    payloads landed. Auto-drop at scope exit for un-moved heap-
    backed bindings. Integration with C2.3 via new `DropPlan`
    artifact from borrow checker — per-fn `BTreeMap<FnId,
    BTreeSet<VarId>>` of moved-source VarIds. Salsa pipeline
    becomes parse → resolve → check → borrow_check (returns
    `Option<DropPlan>`) → codegen (consumes DropPlan).
    sentinel-runtime gains `sentinel_free(ptr)` (libc free
    wrapper). CodegenCtx gains `scope_stack`, `current_fn_id`,
    `free_fn`, `drop_plan` field. compile_fn / lower_block push/
    pop scopes; emit_scope_drops fires at exit (reverse decl
    order; skips moved + tail-returned). emit_drop_for_binding
    dispatches on Type: Array → free data ptr; ?Struct → cond-
    branch free payload if valid; primitive / ref → no-op.
    Struct field recursive drops DEFERRED (known gap; closes
    at C2.5). FnId / VarId gain PartialOrd + Ord. +4 c24
    fixtures: c24_array_dropped (24), c24_moved_array_no_double_free
    (66), c24_nested_blocks_drop (10), c24_go_no_go (160 —
    phase-go). +2 borrow-check unit tests (DropPlan), +2
    runtime tests (sentinel_free).

  - **C2.3** (50c826b): move semantics + use-after-move
    detection per ADR 0017 D9. Adds `FnCtx.moved: HashMap<VarId,
    Span>` tracking which bindings have been consumed +
    `is_copy_type(ty)` classification (Copy = primitives + refs +
    ?Copy-inner; Move = struct / array / generic-instance /
    ?Move-inner / TypeParam-conservative). Var(x) reads in
    CONSUMING context transition Live → Moved; subsequent reads
    surface `BorrowError::UseAfterMove` with three-label miette
    diagnostic (decl_span / move_span / use_span). Non-consuming
    contexts (postfix receivers `p.field` / `xs[i]`, lvalue
    operands `&x` / `&mut x`, runtime-builtin call args `len(xs)`
    / `is_some(x)` / `unwrap_or(x, d)` / `print(x)`) check
    use-after-move without transitioning. Branch-aware merge at
    if/else: snapshot before each branch + restore between +
    merge after with "moved in either branch → moved after" —
    this is what makes `if c { fst(p) } else { snd(p) }` accept.
    c17_go_no_go fixture updated for move semantics (previous
    `pick_int(true, p) + pick_int(false, p)` double-used p; now
    constructs p1 + p2). +12 borrow-check unit tests + 4 driver
    pass-test fixtures (c23_move_struct / c23_branch_isolation /
    c23_array_move / c23_go_no_go). c23_go_no_go runs: stdout
    "100\n", exit 0 (Account struct, transfer moves src + dst,
    balance_of in if/else).

  - **C2.2** (4a0ca92): `&mut T` + shared-XOR-mutable rule per
    ADR 0017 D6. Extends sentinel-borrow-check with place-
    tracking + transient/rooted borrow lifetimes + five new
    `BorrowError` variants. The XOR invariant: at any point a
    place P is either (a) borrow-free, (b) has N ≥ 1 shared
    borrows, or (c) has exactly one mutable borrow.
    Per-source-VarId `PlaceState { shared: Vec, mut_borrow:
    Option }` in FnCtx; each `&x` / `&mut x` adds a
    `BorrowInstance` with lifetime `Transient` (default) or
    `UntilScope(depth)` (rooted in a ref-typed let).
    `clear_transients()` runs after every stmt — this is what
    keeps c20_go_no_go's `add(&a, &b);` + `increment(&mut a);`
    valid (shared borrows from `add` are transient + die
    before `increment`'s `&mut a` is taken).
    `promote_transients(depth)` rooting fires at ref-typed
    `let r = &x;` or equivalent assign — promotes any new
    transients to live until the binding's scope pops.
    `BorrowSource::Incoming` now carries a VarId payload so
    place-tracking routes conflicts through the param's place
    key. New `walk_assign_target` (LHS lvalue walk + write-
    conflict check on Var leaves) + `walk_expr_lvalue` (`& x`
    / `&mut x` operand walk; no read-check on inner Var). Five
    new error variants: MutableBorrowOfShared (& then &mut),
    SharedBorrowOfMutable (&mut then &), BorrowConflict (two
    &mut), WriteWhileBorrowed (`x = v;` while borrowed),
    ReadWhileMutBorrowed (reading `x` while &mut x active).
    +14 borrow-check unit tests + 4 driver pass-test fixtures
    (c22_multi_shared / c22_scoped_mut / c22_transient_then_mut /
    c22_go_no_go). c22_go_no_go runs: stdout "35\n", exit 0
    (shared block computes 20; mut block increments x to 15;
    35 total).

  - **C2.1** (64edf3d): shared-only lexical borrow checker per
    ADR 0017 D6. New crate `sentinel-borrow-check` (~600 LOC
    including tests). New salsa-tracked query `borrow_check_query`
    chains on `check_query`; the pipeline becomes parse_query →
    resolve_query → check_query → borrow_check_query → codegen
    with diagnostics accumulating transitively. Driver wires the
    gate: a borrow failure blocks codegen + exits with code 1.
    Two `BorrowError` variants at C2.1: `OutlivesSource`
    (canonical use-after-scope) and `ReturnsLocalRef` (a fn
    returns a `&T` whose source is fn-local — per ADR 0017 D7's
    "second-class refs" rule). Borrow-source representation is
    a bounded enum `{ Local(VarId), Incoming, LocalAnonymous }`;
    the analysis is per-fn with limited inter-procedural
    reasoning (a call returning a ref inherits the most-
    restrictive source among its ref args). Inner blocks push/
    pop scopes; let-stmts record source from RHS before declaring
    the binding; assign-stmts to ref-typed Vars update the
    recorded source. ADR 0017 D6's lexical-first formulation is
    now exercised; D7's second-class-refs rule enforced.
    +15 borrow-check unit tests + 4 driver pass-test fixtures
    (c21_borrow_local_ok / c21_pass_through_ref / c21_reborrow /
    c21_go_no_go). c21_go_no_go runs: stdout "168\n", exit 0
    (`sum_two(&a, &b) + triple(&a) + triple(&b)` = 42 + 30 + 96).

  - **C2.0.2** (9516ebb): bundled AST + parser + resolve +
    types + codegen for refs end-to-end per ADR 0017 D1-D5 +
    D11. AST gains `UnaryOp::Ref` / `UnaryOp::RefMut` /
    `UnaryOp::Deref`; `TypeExprKind::Ref { mutable, inner }`;
    `StmtKind::Let.mutable`; `StmtKind::Assign { target, value }`;
    `Param.mutable`. Parser handles `&T` / `&mut T` (with
    whitespace tolerance), `&expr` / `&mut expr` / `*expr`
    prefix unaries, `let mut`, `mut` params, and assignment
    statements (after parsing an expression at stmt position,
    a following `=` triggers Assign-statement parsing).
    Resolve passes the new variants through with `mutable`
    bits threaded on ResolvedParam / ResolvedStmtKind::Let;
    new ResolvedStmtKind::Assign. Types adds `Type::Ref(RefId)`
    + `RefData { mutable, inner }` + `intern_ref` per ADR
    0017 D11, mirroring the C1.7.4b GenericInstance interner
    pattern (keeps `Type: Copy`). `NullableInner` gains `Ref`
    for `?&T`; ArrayElem stays primitive-only (refs in arrays
    rejected at parse-array-type time with `RefInArray`).
    Type::substitute extended to recurse through Ref (clone,
    substitute inner, re-intern). unify_one extended for Ref
    (mutability match + inner recursion — enables generic+ref
    inference). VarTypeEnv becomes `HashMap<VarId, (Type,
    bool)>` to track mutability. New TypeError variants:
    NestedRef, RefInArray, RefInStructField, BorrowOfRvalue,
    AssignToRvalue, AssignToImmutable, BorrowMutOfImmutable,
    DerefOfNonRef, AssignThroughSharedRef,
    IndexAssignNotSupported. check_expr dispatches Unary
    Ref/RefMut/Deref: Ref requires lvalue; RefMut requires
    mutable lvalue; Deref requires Type::Ref operand and
    returns its inner. check_stmt's Assign arm validates LHS
    is a mutable lvalue (recursive through field-access),
    pushes target.ty down to RHS for widening, and Mismatch's
    on type disagreement. Codegen lowers refs as LLVM opaque
    pointers (LLVM 15+ no-typed-pointer). New
    `lower_lvalue_ptr` helper handles Var → alloca ptr, `*r` →
    load r's value (the ptr), `p.field` → struct_gep into the
    field; assignment and `&` / `&mut` both delegate through
    it. `*r` lowers as load-from-pointer of the inner type
    (looked up via `program.refs[id].inner`). Tests: +62 (870
    total) — +8 ast, +23 syntax, +21 types, +6 codegen, +4
    driver pass-test fixtures (c20_ref_basic / c20_mut_basic /
    c20_deref_basic / c20_go_no_go). c20_go_no_go runs:
    stdout "53\n", exit 0 (the full ADR 0017 D14 program with
    `add(&a, &b)` shared-borrows + `increment(&mut a)`
    exclusive-borrow + `let mut a` + deref-assignment + print).

**Phase C2 closes at C2.5.** All six sub-phases shipped
(C2.0.1 + C2.0.2 + C2.1 + C2.2 + C2.3 + C2.4 + C2.5 — seven
feat/docs commits across ~6 effective sessions vs the ADR 0017
D9 estimate "6-13 sessions across 5-6 sub-phases" — low end
of the range). ADR 0017 ACCEPTED-WITH-AMENDMENTS; ADR 0018
(Polonius migration plan) PROPOSED. Two C2 follow-ons
documented but deferred: (a) Polonius migration per ADR 0018
(trigger: empirical friction, not a calendar date); (b)
per-(VarId, FieldPath) move state to close the documented
partial-move-through-field-projection soundness gap (highest-
priority post-C2 work; see `docs/borrow-check-limitations.md`).

**Next: Phase C3** — effect-system integration from Phase B
Sentinel-Mini per HANDOVER §6.2. Pre-flight: write ADR 0019
PROPOSED covering effect-row representation in sentinel-types
+ effect annotations + inference vs annotation + `secret T`
qualifier promotion + handler lowering. See §0.2 for the
patterns to argue.

**Workspace test count**: 935 active across all crates (+1
doctest at sentinel-broker; +4 over C2.4: +4 driver pass-test
fixtures (c25_*)). All four check-suite checks green. c05 go/
no-go (C1.3 bool flow) runs: stdout "10", exit 0. c14 go/no-go
(C1.4 struct flow) runs: stdout "7", exit 0. c15 go/no-go (C1.5
nullable flow) runs: stdout "142", exit 0. c16 go/no-go (C1.6
array flow) runs: stdout "15", exit 0. c17 go/no-go (C1.7
generics flow — updated for move semantics) runs: stdout
"42", exit 0. c20 go/no-go (C2.0.2 refs+mut+assign flow) runs:
stdout "53", exit 0. c21 go/no-go (C2.1 shared-borrow flow)
runs: stdout "168", exit 0. c22 go/no-go (C2.2 XOR alternation
flow) runs: stdout "35", exit 0. c23 go/no-go (C2.3 move
semantics) runs: stdout "100", exit 0. c24 go/no-go (C2.4 RAII
/ drop) runs: stdout "160", exit 0. **c25 go/no-go (C2.5 D14
— XOR + move + recursive field drop on Bag-with-array) runs:
stdout "190", exit 0.** See STATE.md "Conventions" for the
per-crate breakdown.

**ADR status**:

  - 0001 staged-validation                       ACCEPTED
  - 0002 effect-rows-in-mini                     ACCEPTED
  - 0003 b1-retrospective                        ACCEPTED
  - 0004 row-representation-and-effect-surface   ACCEPTED
  - 0005 effect-inference-judgment               ACCEPTED
  - 0006 default-close-row-variables             ACCEPTED
  - 0007 effect-handlers                         ACCEPTED
  - 0008 secret-qualifier-and-constant-time      ACCEPTED
  - 0009 phase-c-kickoff-and-c0-plan             ACCEPTED (all C0
                                                 sub-phases done)
  - 0010 concrete-c0-surface-syntax              ACCEPTED (all
                                                 D-decisions exercised)
  - 0011 phase-c1-kickoff-and-type-system-plan   ACCEPTED — all 12
                                                 D-decisions exercised
                                                 across C1.0 through
                                                 C1.7. D6's eight-
                                                 sub-phase budget is
                                                 closed (every C1.x
                                                 done). D12's perf
                                                 discipline measured:
                                                 sub-second cold
                                                 builds + sub-100ms
                                                 incremental rebuilds
                                                 on the current
                                                 corpus.
  - 0012 concrete-c1-surface-syntax              ACCEPTED — every
                                                 D-decision exercised
                                                 across C1.2-3
  - 0013 concrete-c1-4-struct-syntax             ACCEPTED — every
                                                 D-decision exercised
                                                 across C1.4
  - 0014 concrete-c1-5-nullable-syntax           ACCEPTED — D1-D11
                                                 all fully exercised:
                                                 D1-D9 + D11 at C1.5;
                                                 D10 retires at C1.6
                                                 via ADR 0015 D11
                                                 (`?Struct` heap
                                                 indirection unlocks
                                                 the recursive-
                                                 struct relaxation).
                                                 D4 representation
                                                 stays as the flat
                                                 NullableInner subset
                                                 enum (C1.5
                                                 amendment)
  - 0015 concrete-c1-6-array-syntax              ACCEPTED-WITH-
                                                 AMENDMENTS — D1-D5
                                                 + D7-D13 all fully
                                                 exercised; D6 amended
                                                 (NullableInner +
                                                 ArrayElem stay
                                                 primitive-only,
                                                 deferring `?[T]` and
                                                 `[?T]` to a future
                                                 ADR; C1.7.4b partially
                                                 closes by adding
                                                 GenericInstance
                                                 variants — `?Box<i64>`
                                                 and `[Box<i64>]` work,
                                                 but `?[T]` and `[?T]`
                                                 stay deferred); D11
                                                 implementation
                                                 closes ADR 0014 D10
  - 0016 concrete-c1-7-generics-syntax           ACCEPTED — all 12
                                                 D-decisions exercised
                                                 cleanly across the
                                                 C1.7 scaffolding +
                                                 4a + 5 + 4b commits.
                                                 No amendments — each
                                                 D-decision survived
                                                 implementation as
                                                 drafted.
  - 0017 phase-c2-kickoff-and-region-plan        ACCEPTED-WITH-
                                                 AMENDMENTS — all
                                                 14 D-decisions
                                                 exercised across
                                                 C2.0.1 + C2.0.2 +
                                                 C2.1 + C2.2 +
                                                 C2.3 + C2.4 +
                                                 C2.5. Three
                                                 amendments at
                                                 C2.5: A1 the
                                                 C2.4 recursive-
                                                 field-drop
                                                 closure slipped
                                                 to C2.5(a); A2
                                                 the Polonius
                                                 plan shipped as
                                                 standalone ADR
                                                 0018; A3 the
                                                 partial-move-
                                                 through-field-
                                                 projection
                                                 soundness gap
                                                 documented for
                                                 closure in a
                                                 follow-on sub-
                                                 phase.
  - 0018 polonius-migration-plan                 PROPOSED —
                                                 documents the
                                                 lexical → flow-
                                                 sensitive
                                                 borrow-check
                                                 migration via
                                                 polonius-engine
                                                 0.13. Six D-
                                                 decisions: D1
                                                 trigger
                                                 (empirical
                                                 friction); D2
                                                 preserved
                                                 surface
                                                 (BorrowError
                                                 variants +
                                                 DropPlan stay);
                                                 D3 adopt
                                                 polonius-engine;
                                                 D4 representation
                                                 changes (CFG +
                                                 origins + loans
                                                 + liveness); D5
                                                 three-step
                                                 rollout; D6 out-
                                                 of-scope items.
                                                 No migration
                                                 code at C2.5;
                                                 ADR records the
                                                 plan only.
  - 0019 phase-c3-kickoff-and-effects-plan       ACCEPTED-WITH-
                                                 AMENDMENTS — all
                                                 14 D-decisions
                                                 exercised across
                                                 C3.0 + C3.1 +
                                                 C3.1b + C3.2 +
                                                 C3.3. Two
                                                 amendments at
                                                 C3.3: A1
                                                 SecretEscapesPolymorphism
                                                 subsumed by
                                                 monomorphic
                                                 generics +
                                                 SecretFlow-via-
                                                 Mismatch; A2
                                                 runtime builtins
                                                 declared effect-
                                                 free. D3 (RowId
                                                 interner) and D8
                                                 (handler runtime)
                                                 both deferred —
                                                 RowId becomes
                                                 useful when
                                                 handler runtime
                                                 lands at ADR
                                                 0020; D9 (async)
                                                 deferred
                                                 indefinitely.
  - 0020 phase-c3-handler-runtime                ACCEPTED-WITH-
                                                 AMENDMENTS — all
                                                 twelve D-decisions
                                                 exercised modulo
                                                 D2 across eight
                                                 sub-phases (C3.4
                                                 + C3.5(a/b/c/d/e)
                                                 + C3.6(a/b) +
                                                 C3.7). The
                                                 amendment: D2's
                                                 multi-shot
                                                 relaxation is
                                                 deferred
                                                 indefinitely;
                                                 one-shot suffices
                                                 for the bootstrap.
  - 0021 phase-c4-kickoff                        PROPOSED — Phase
                                                 C4 umbrella
                                                 (classes + traits
                                                 + delegation +
                                                 structured
                                                 concurrency). 14
                                                 D-decisions; six
                                                 sub-phases C4.0
                                                 → C4.5; 8-12
                                                 sessions estimate.
                                                 C4.0 + C4.1 done;
                                                 flips at C4.5
                                                 close.
  - 0022 concrete-c4-1-class-syntax              ACCEPTED-WITH-
                                                 AMENDMENTS — all
                                                 eleven D-decisions
                                                 exercised across
                                                 C4.1 (1/N) AST +
                                                 parser and C4.1
                                                 (2/N) resolve /
                                                 types / codegen +
                                                 method-call + init
                                                 call + close-out.
                                                 Two amendments at
                                                 C4.1 close: A1 D4
                                                 definite-assignment
                                                 ships as a flat
                                                 any-assigned check
                                                 (branch-aware
                                                 merge +
                                                 InitFieldReadBeforeAssign
                                                 deferred); A2 D8
                                                 general `Self` in
                                                 type position
                                                 deferred (only
                                                 positional `self:
                                                 &Self` via
                                                 parse_self_param).
  - 0023 concrete-c4-2-trait-impl-syntax         ACCEPTED-WITH-
                                                 AMENDMENTS —
                                                 flipped at C4.2
                                                 (2/N) close. All
                                                 twelve D-decisions
                                                 landed modulo
                                                 three amendments:
                                                 A1 D5 Path 3
                                                 (bounded-generic
                                                 dispatch via
                                                 witness tables)
                                                 DEFERRED — needs
                                                 `<W: Writer>`
                                                 surface (parser +
                                                 AST + resolve +
                                                 types + codegen
                                                 monomorphisation
                                                 extension); A2
                                                 D9 witness-table
                                                 values not emitted
                                                 (scaffolding for
                                                 Path 3); A3 D7
                                                 `Type::TraitSelf`
                                                 interner SHIPPED
                                                 but unused
                                                 (params/returns
                                                 don't reference
                                                 Self, only
                                                 positional via
                                                 self_kind —
                                                 mirrors C4.1 A2).
                                                 Implementation
                                                 footprint: ~2
                                                 sessions across
                                                 C4.2 (1/N) + (2/N)
                                                 — low end of the
                                                 ADR 0021 D14
                                                 estimate.
  - 0024 c4-4-structured-concurrency             PROPOSED — C4.4
                                                 surface
                                                 (scope / spawn /
                                                 await) + Async
                                                 built-in effect +
                                                 thread-per-spawn
                                                 runtime scheduler.
                                                 Twelve D-decisions.
                                                 C4.4 (1/N) parser
                                                 layer done; C4.4
                                                 (2/N runtime)
                                                 done (5 new C-ABI
                                                 symbols + Task +
                                                 ScopeCtx structs).
                                                 What remains: types
                                                 + codegen + phase-
                                                 go fixture. ADR
                                                 flips at C4.4
                                                 (2/N) close.

### 0.1 Working norms (carry forward into Phase C3)

Original Phase-A norms, augmented with Phase-B and Phase-C
lessons:

- **Trust STATE.md, not the git log.** Commit messages are dense
  and miss design rationale. Always read docs/STATE.md and this
  file before doing anything; never infer state from git log alone.

- **Terminal quirk: nested heredocs break.** This developer's
  terminal mangles `<<EOF ... <<INNER ... INNER ... EOF`-style
  scripts. Use one of: (a) base64-encoded python3 -c blocks,
  (b) write a script to /tmp/ via a single non-nested heredoc and
  then execute it, or (c) single non-nested heredocs only.

- **Small patches, build between each.** The session that built
  Phase A7 took four diagnostic/fix iterations because the
  initial patch was too ambitious. Better practice: land the
  type/trait changes first, build, then add the implementations,
  build, then add the tests. Same lesson held for Phase C0:
  small sub-phase commits + cargo test after each beats one big
  commit.

- **Honest disclosure beats confident-but-wrong.** This developer
  values being told when something is uncertain or guessed at
  ("I'm not sure if BudgetScope::within_budget emits BudgetClosed
  on rejection, so I included an assertion to find out") over
  patches presented as definitely-correct that turn out not to be.
  The C1.0b pause is an example: rather than land a half-working
  retrofit, the session committed the working sentinel-base alone
  and documented C1.0b's path forward.

- **Minimal ceremony.** "go", "proceed", short replies are the
  norm. Long preambles are unwelcome.

- **Examples held to -D warnings.** Don't allow lint debt in
  examples; they're educational artifacts. Same for tests/pass/
  fixtures (Phase C).

- **Check before overwriting docs.** When patching documentation
  files via Python, always check `p.exists()` and read existing
  content first. Prefer merge/append patterns for docs/. Phase A
  hard-learned lesson on BACKLOG.md.

New norms learned during Phase B and Phase C:

- **ADR-first per phase boundary.** ADR 0002 was the Phase B
  kickoff; ADR 0009 was Phase C kickoff; ADR 0011 was Phase C1
  kickoff. Each landed PROPOSED before the first feat commit,
  became ACCEPTED at sub-phase completion. Continue the pattern
  for Phase D and beyond.

- **feat + docs commit pairs per sub-phase.** Each sub-phase
  ships as a feat commit (code + tests) followed by a docs
  commit (STATE.md refresh + ADR status updates). The docs
  commit also backfills the hash that the feat commit produced.
  See the C0.0-C0.5 history for the rhythm.

- **The pure-function pipeline discipline (ADR 0009 D1a).**
  C0 held it — `lex`, `parse`, `compile_to_object` are all
  `(input) -> (output, diagnostics)`. C1.0a starts cashing in
  the payoff by retrofitting Salsa. Keep new pipeline stages
  pure-function until salsa wrapping happens at a known
  sub-phase.

- **cargo clippy --workspace --all-targets -D warnings** is
  part of the standard four-check suite alongside build / test
  / test --doc. Don't let clippy debt accumulate; it has caused
  full re-sweep commits before (4182ff6 cleared pre-B4.0 lints).

- **No pushes from the assistant.** Commits land locally; the
  dev pushes via GitHub Desktop in batches. Never run `git push`.

- **macOS-only assumption.** `.cargo/config.toml` hardcodes brew
  paths (LLVM_SYS_180_PREFIX=/opt/homebrew/opt/llvm@18, link
  search at /opt/homebrew/lib + /usr/local/lib). Cross-platform
  is a future concern; right now LLVM 18 must be `brew install`-ed.

- **Mode B working conventions** (from Phase B onward): paste-
  direct zsh; anchor-guarded Python patches via `/tmp/`; no
  nested heredocs; when a Python script needs multi-line Rust
  text put it in a separate `cat > /tmp/foo.txt <<'RSEOF' …`
  block and `read_text()` it from Python (triple-quoted Python
  strings inside a bash heredoc can mangle terminals); cargo
  test -p <crate> after each patch.

### 0.2 Next session opening (Phase C5 kickoff)

> **Phase C4 is COMPLETE — all of C4.0 through C4.5 shipped;
> ADR 0021 ACCEPTED-WITH-AMENDMENTS.** C4.4 (2/N) landed the
> structured-concurrency types + codegen + phase-go (ADR 0024);
> C4.5 closed Phase C4 with the combined full-surface phase-go
> (`tests/pass/c4_go_no_go.sentinel` — class + trait + impl +
> delegation + scope/spawn/await, exit 42) + `c4_named_impl`
> (two named impls co-existing) + the ADR 0021 flip. The
> detailed C4.4 (2/N) punch list retained below is HISTORICAL —
> every item shipped; do NOT re-do it.
>
> **Resume at Phase C5 kickoff** per HANDOVER §6.2 (NOT "Phase
> D" — the roadmap after C4 is C5 per ADR 0021's close): broker
> integration, cross-process safety, reproducible-build
> guarantees, stable ABI definition, LSP/tooling polish, plus
> **actors** (deferred from C4 per ADR 0021 D10) + constant-time
> secret codegen (deferred from C3 per ADR 0019 D12). Sentinel's
> 1.0 release is at C5 close. **ADR 0025 PROPOSED is drafted**
> (`docs/decisions/0025-phase-c5-kickoff-and-productionization-plan.md`)
> — 14 D-decisions + an 8-sub-phase split (C5.0–C5.8). **C5.0 is
> COMPLETE:** the go/no-go program is CHOSEN — a single-process,
> single-file TLS 1.3 handshake (ADR 0025 `## C5.0 resolution`) — which
> pins **D6 (cross-process) → post-1.0** and **D9 (modules) →
> post-1.0** (the handshake is self-contained + fits one file) and
> forces the headline constant-time `secret` capability (D3) an HTTP
> server would leave unexercised; **D11** test infra landed (`cargo
> nextest` + `insta` full-diagnostic UI snapshots, replacing the ad-hoc
> `stderr.contains` checks); and the **D8** reproducible-build audit
> found the C0–C4 build already byte-identical across independent `snc`
> processes (locked in by `crates/sentinel-driver/tests/repro.rs`).
> **ADR 0026 PROPOSED is drafted** (the C5.1/C5.2 HIR/MIR pipeline +
> constant-time secret codegen surface — 10 D-decisions, 4-sub-phase
> split C5.1a→C5.2b). **C5.1a (1/N) is DONE** — the thin HIR seam
> (`lower_to_hir`; codegen consumes `HirProgram`), behaviour-preserving
> (1195 tests + repro byte-identical). **The D3 escape hatch is then
> INVOKED** (codegen has ~295 `TypedExprKind` refs across 90 signatures —
> a thick-HIR migration is multi-session/high-risk and not needed for the
> 1.0 constant-time capability): codegen STAYS on the typed program (via
> the seam), the thick HIR desugar + codegen migration go **post-1.0**,
> and **C5.1a closes at the seam**. **C5.1b (1/N) is then DONE** — the
> `sentinel-mir` data model: a minimal SSA/CFG (`MirProgram` /
> `MirFunction` / `MirBlock` with SSA block-params / `MirOp` /
> `MirTerminator`), every value carrying its `Type` so secrecy reads off
> `Type::Secret(_)` (`is_secret`), the three D5 sinks representable, the
> rest via `Opaque`; additive (nothing consumes MIR yet). **C5.1b (2/N)
> is then DONE** — `lower_to_mir` lowers typed fn bodies → MIR SSA
> (`if`/`&&`/`||` → `Branch` + merge block-params; a variable reassigned
> on one arm threaded through a merge param; the three D5 sinks precise —
> `Branch` cond / `Load` index+address / `Binary(Div)` — and `declassify`
> → `MirOp::Declassify`, the rest `Opaque`); top-level fns only; still
> additive (nothing consumes MIR yet). **C5.2b (1/N) is then DONE** — the
> D5 verification pass `verify_constant_time(&MirProgram)` (type-directed
> taint: each value's `Type` already carries the fixpoint, so the pass
> reads `is_secret` and inspects the branch/load/div sinks →
> `sentinel::mir::secret_leak`), additive (not yet driver-wired).
> **C5.2b (2/N) is then DONE** — D5 is wired into `snc` (after
> `check_query`: `lower_to_mir` → `verify_constant_time` → render
> `secret_leak` + exit 1, gating codegen) with the `c52_secret_leak` (UI)
> and `c52_secret_ct` (branch-free masked-select, pass) fixtures; the c51
> bar holds (`repro.rs` byte-identical). **The C5.2a/D4 open question is
> resolved (with the developer): defer D4 and do bitwise operators next**
> — the go/no-go's constant-time `Finished` MAC verify is an XOR-accumulate
> compare that needs `^`/`|`, and the surface has none. **C5.3 (1/N) lexer
> is DONE** (`Pipe`+`Caret` tokens, additive). **C5.3 (2/N) is then DONE**
> — `& | ^` end-to-end (AST `BinOp` + parser precedence + codegen
> and/or/xor; types/MIR/D5 unchanged — `Binary` is op-generic), with
> `c53_bitwise` + `c53_ct_eq` (the real constant-time MAC-verify shape
> passing D5); **ADR 0027 → ACCEPTED-WITH-AMENDMENTS** (A1: `<< >> ~`/C5.4
> deferred). **The fork is resolved (with the developer): broker
> integration next (C5.4, ADR 0028 PROPOSED).** **C5.4 (1/N) is DONE** —
> the broker-arena substrate: a public broker raw-bytes API
> (`Arena::alloc_bytes` → `NonNull<u8>`, exposing the strategy's
> `alloc_raw`) + runtime C-ABI `sentinel_arena_enter`/`_alloc`/`_exit` on
> a process-wide lazy `Broker` (bump arenas; `_exit` destroys → frees the
> backing buffer). Additive + c51-safe (codegen untouched → objects
> byte-identical). Refinements vs the ADR: not "runtime-only" (a small
> broker API was added), and the malloc-replacement framing (slab pool /
> ptr→handle registry) was dropped — the broker is arena-, not malloc-,
> shaped. **C5.4 (2/N) is then DONE — Phase C5.4 closes; ADR 0028 →
> ACCEPTED-WITH-AMENDMENTS.** The scope→arena codegen routes a scope's
> non-escaping primitive array-literal heap buffers into a per-scope
> broker bump arena and replaces that scope's per-binding `sentinel_free`s
> with one `sentinel_arena_exit`; escaping/moved/returned values stay on
> `sentinel_alloc` (libc). A `compute_arena_routed` pre-pass produces a
> single `HashSet<VarId>` = exactly `emit_scope_drops`'s free set
> (`∉ moved ∧ ≠ tail_returned_var(&block.tail)`), narrowed to
> `let x = [i64/i32/bool array literal]` in non-generic non-effecting fns;
> that one set drives both alloc-routing and free-skip, and the per-scope
> arena handle lives in a new `ScopeFrame` created lazily. UAF-safety came
> from reasoning (routed ⊆ proven-non-escaping free set) + disassembly +
> the c24/c25 guards + `c54_scope_arena`. **Amendment A2: the prep's
> "verified UAF hole" was wrong** — a tail-returned array IS in
> `moved_sources` (the borrow checker walks the tail `Var` as a consuming
> move before the L626 snapshot; empirically dumped), so `∉ moved` alone
> already excludes returned arrays (both checks kept anyway to mirror
> `emit_scope_drops`). **Resume at a developer-scope call:** assemble the
> TLS 1.3 go/no-go (constant-time compare + scope arenas both in hand), or
> another C5 sub-phase — stable ABI (ADR 0025 D7) or LSP (D10). Deferred
> (post-1.0): per-scope arena *sizing*, routing in methods/generics/
> effecting fns, `scope budget` surface, full escape analysis; the bitwise
> shift wave (`<< >> ~`, ADR 0027 A1) + D4 constant-time emission + the
> ADR 0026 flip remain deferred follow-ons.
>
> **Available C4 follow-ons** (none blocking C5): work-stealing
> scheduler (ADR 0024 A1), scope cancellation (A2), `Task<T>`
> for T≠i64 + non-i64 spawn args (A3), explicit `Task<T>`
> type-position annotations via threading `tasks` through
> `resolve_type_expr` (A5), Path-3 bounded-generic dispatch
> (ADR 0023 A1), Polonius migration (ADR 0018), and the
> partial-move-through-field-projection soundness gap
> (`docs/borrow-check-limitations.md`).
>
> Historical C4.4 (2/N) punch list (all shipped) follows.

**Prior-art reference**: the prior session attempted the
types+codegen layer end-to-end and rolled back the partial
work (kept only the runtime scaffolding). Lessons learned + a
known sticking point for the next attempt are captured below.

**C4.4 (2/N types) deliverables** (per ADR 0024 D4 + D5):

  - Add `Type::Task(TaskId)` as the tenth interner-table-
    style variant (preserve `Type: Copy + Hash`); add
    `TaskData { result_ty: Type }`; add `intern_task` helper
    matching `intern_kont`'s shape.
  - Add `tasks: Vec<TaskData>` to `TypedProgram`. Thread
    `tasks: &mut Vec<TaskData>` through `check_expr` and its
    callees — there are many call sites; a sed/python pass
    after `konts` is the cleanest mechanical update (see the
    prior session's commit history for the pattern).
  - Add `TypedExprKind::Scope { mode, body: Box<TypedBlock> }`,
    `Spawn { call: Box<TypedExpr>, task_id: TaskId }`,
    `Await { task_expr: Box<TypedExpr>, task_id: TaskId }`.
  - In `check_expr`: Scope types as the body's tail type;
    Spawn validates the inner is a `TypedExprKind::Call`
    (per ADR 0024 D2), restricts call's return type to I64
    (per ADR 0024 D7), interns `Task<I64>`; Await requires
    the receiver be `Type::Task(_)` and produces the
    interned TaskData's result_ty.
  - Three new `TypeError` variants: `SpawnMustBeCall`,
    `SpawnResultMustBeI64`, `AwaitOnNonTask`.
  - Update all Type-match sites for `Type::Task(_)` arms
    (≈8 sites: try_to_nullable_inner, try_to_array_elem,
    substitute, type_display, Display impl,
    coerce_to_expected match, is_nullable, llvm_basic_type's
    callers).
  - Update all TypedExprKind-match sites for Scope/Spawn/
    Await arms in: `substitute`, `walk_expr_for_mono`,
    `expr_performs`, `count_performs`, `find_unique_perform`,
    `walk_collect_var_refs`, `find_var_name_in_expr`,
    `substitute_perform_with_var`. **Prior attempt missed
    `substitute_perform_with_var` and one other** — the
    error site list is preserved in the prior session's
    last build output (2 remaining match arms at codegen
    lines ~5572 and ~5852).
  - Drop the C4.4 (1/N) NotYet variants from `ResolveError`
    AND from the resolve `check_expr` arms (replace with
    full pass-through). Drop the C4.4 (1/N) `unreachable!`
    stub in types' check_expr.

  **Decision deferred** (ADR 0024 D5 — Async effect-row
  enforcement): the prior attempt's MVP plan was to skip
  effect-row enforcement entirely at C4.4 (2/N) and treat
  spawn/await as effect-free. This keeps the phase-go
  trivially typecheckable but contradicts ADR 0024 D5.
  Recommended: ship the enforcement at C4.4 (2/N) — auto-
  register the Async effect in resolve (pre-register
  `EffectId(0)` as Async before user effects), make
  spawn/await contribute Async to effect-check's row, make
  `scope concurrent { ... }` discharge Async at the
  effect-check layer. This requires ~50 extra LOC in
  sentinel-effect-check (a new TypedExprKind arm) + a small
  resolve change. Document any deferral as an amendment to
  ADR 0024 D5.

**C4.4 (2/N codegen) deliverables** (per ADR 0024 D8):

  - **Add 5 runtime-fn externs to CodegenCtx + declare in
    pass 1**: task_spawn_fn / task_await_fn / scope_enter_fn /
    scope_exit_fn / scope_register_fn. Signatures per ADR
    0024 D7 (all opaque `ptr` types except task_await
    returns i64; scope_enter takes no args; etc.).
  - **Per-spawn-fn wrapper synthesis** (the tricky bit). One
    wrapper per unique spawn-target FnId (not per spawn
    site). The wrapper signature is
    `void wrapper(*Task task_ptr, *u8 args)`. Body: GEPs
    into args at i64 offsets to unpack each arg, calls the
    target fn, stores result in task_ptr->result (offset 0),
    writes 1 into task_ptr->done (offset 8).
    **Sticking point from prior attempt**: synthesizing
    wrappers from inside CodegenCtx fails because CodegenCtx
    doesn't hold a `&Module` reference. The cleanest fix:
    pre-walk the typed program in compile_to_object (BEFORE
    CodegenCtx is created) via a `collect_spawn_targets` fn
    that returns a `Vec<FnId>`. Then for each FnId, declare
    the wrapper via `module.add_function(...)` and emit its
    body using a fresh `context.create_builder()` (local to
    the pre-walk loop). Pass the resulting
    `spawn_wrappers: HashMap<FnId, FunctionValue<'ctx>>` to
    CodegenCtx by ownership. The prior attempt's wrapper
    code is at `/dev/null` (reverted) but the pattern is in
    the runtime tests showing the wrapper's expected shape.
  - **Lower Spawn at the call site**: `sentinel_alloc(N*8)`
    for args storage; lower each arg + store at offset i*8
    via GEP; call sentinel_task_spawn(wrapper_ptr,
    args_storage, args_size_bytes); if a current_scope is
    pushed on CodegenCtx, also call sentinel_scope_register.
  - **Lower Await**: lower task to a ptr value + call
    sentinel_task_await + return the i64 result.
  - **Lower Scope**: call sentinel_scope_enter to get a
    ScopeCtx*; push `current_scope: Option<PointerValue>`
    on CodegenCtx (save+restore for nested scopes); lower
    body; call sentinel_scope_exit; pop current_scope.

  **Drop emission**: add `Type::Task(_) => {}` arm to
  emit_drop_for_binding — Task cleanup is the runtime's job
  (sentinel_task_await / sentinel_scope_exit free the
  Task). Add `Type::Task(_) => false` to
  field_type_needs_drop_inner.

**Phase-go fixture** per ADR 0024 D12: `c44_go_no_go.sentinel`
— `scope concurrent { let t = spawn double(21); t.await }`
returns 42 via the thread + auto-await.

**UI fixtures**: c44_spawn_non_fn_call (D2 restriction —
spawn target isn't a Call shape), c44_await_on_non_task (D3
— await receiver isn't Task<T>), c44_spawn_result_must_be_i64
(D7 — spawned fn returns non-i64).

**Close-out**: ADR 0024 PROPOSED → ACCEPTED or
ACCEPTED-WITH-AMENDMENTS. Likely amendments per the prior
attempt's learning: A1 D6 thread-per-spawn vs work-stealing
(thread-per-spawn shipped; work-stealing deferred); A2 D9
cancellation on early exit (deferred); A3 D7 Task<T>
restricted to T=I64 (broader T deferred); A4 D5 (if Async
effect-row enforcement gets deferred to C4.5).

**Estimated footprint after the runtime is already in**:
~700-900 LOC across types (~300) + codegen (~400) + fixtures
(~80). Achievable in 1-1.5 sessions of focused work.

**Lessons from the prior C4.4 (2/N) attempt** (preserved for
the next attempt):

  - The `tasks` plumbing through `check_expr` is mechanical
    but the chain is long (≈40 call sites). Use a Python /
    sed script to add `tasks` next to `konts` in both
    signatures and call sites; the prior attempt's pattern
    was `'        konts,$' → '        konts,\n        tasks,'`.
  - The TypedExprKind walks (10+ helper fns) all need the
    same three new arms — easier to do them all in one pass
    via grep+edit rather than chasing build errors.
  - **The codegen wrapper-synthesis approach matters**:
    inline-during-lower_expr DOES NOT WORK because
    CodegenCtx lacks a `&Module` reference. The clean
    pattern is to pre-walk in compile_to_object before
    CodegenCtx exists.
  - The `wrapper_builder.build_*` patterns must match
    inkwell's BuilderError types — chains of `.expect("call")`
    work but verify the inkwell version's Result vs panic
    semantics before committing. The prior attempt's
    wrapper code had `wrapper_builder.build_call(...)
    .expect("call")` which compiles cleanly in inkwell 0.4+.
  - Once spawn lowering exists, validate with
    `cargo run -p sentinel-driver -- tests/pass/c44_go_no_go.sentinel`
    before adding the scope cleanup arm — bug-isolation is
    easier in stages.

**Alternative — Path 3 bounded-generic follow-on** (per ADR
0023 A1 amendment):

  - Parser: `fn use_writer<W: Writer>(w: &mut W) -> i64` — new
    `<T: Bound>` syntax in `parse_type_params`.
  - AST: `TypeParam.bounds: Vec<Spanned<String>>`.
  - Resolve: link bound name → TraitId.
  - Types: bounded `Type::TypeParam` representation; bound
    used at call sites to validate that the concrete mono
    instance satisfies the bound.
  - Codegen: per-instance witness-table GEP + indirect call
    OR (simpler) per-instance specialisation via the C1.7.5
    monomorphisation worklist with the trait's default impl
    inlined at the call site.
  - Estimate: ~1 session.
  - Detail ADR: amend ADR 0023 OR write a follow-on ADR.

**C4.1 follow-ons remain available** (none blocking C4.3):

**C4.1 follow-ons remain available** (none blocking C4.2):

  - **Branch-aware definite-assignment**: extend the flat
    any-assigned check with if/else snapshot+merge mirroring
    the C2 borrow CFG; add a UI fixture pinning the per-arm
    bitmap (init assigning `x` only in the if-branch).
  - **InitFieldReadBeforeAssign**: detect reads of
    `self.field` before the field's first assignment within
    the init body.
  - **Non-lvalue receiver MethodCall**: support
    `Point::init(1, 2).manhattan()` directly via alloca +
    store + GEP detour at the call site.
  - **`Block.tail` → `Option<Expr>`**: closes the placeholder-
    `0` workaround in init bodies. Cross-cutting parser +
    AST + typing + codegen change.
  - **`ClassConstructionMustUseInit` promotion**: move from
    types-level to resolve-level so struct-lit-on-class
    gives the clearer diagnostic instead of UndefinedStruct.
  - **Generic classes**: `class Pair<A, B>` per ADR 0022 D1's
    revisit trigger. Resolve+types+codegen monomorphisation
    extension; AST already supports the surface shape. The bootstrap language surface at C3.7 close has the
full memory-safety + secret-typing + effect-system trifecta
from HANDOVER §6.2:

  - Memory safety: Phase C2's lexical borrow check + RAII
    drop (ADR 0017 ACCEPTED-WITH-AMENDMENTS).
  - Secret typing: Phase C3.1's `Type::Secret(SecretId)` +
    declassify + implicit widening + four CT rejections
    (ADR 0019).
  - Effect system: Phase C3.2/3.3's effect-row machinery +
    main-must-be-effect-free (ADR 0019).
  - Handler runtime: Phase C3.4 → C3.7's deep-handler one-
    shot continuations + bubble-aware resume + nested
    handles + non-identity return arms (ADR 0020).

What's deferred past Phase C3:

  - **Multi-shot continuations** (ADR 0020 D2 amendment).
    The upgrade path stays mechanical: deep-clone the kont's
    frame chain + captured-state on each resume entry.
    Promotes to a follow-on perf ADR if Sentinel surfaces a
    use case.
  - **Non-i64-returning ops**: placeholder type hardcoded
    to i64. A future ADR can generalise (per-fn resumer
    types, or boxed values).
  - **Embedded performs inside chained-let RHSes**:
    e.g. `let a = perform Op() + 1`. The C3.5(d) embedded-
    shape detector requires `stmts.len() == 0`; the C3.5(e)
    chained-lets detector requires each let RHS to be a
    direct perform / effecting call. The union case stays
    a follow-on extension to C3.5.
  - **Polonius migration** (ADR 0018): flow-sensitive
    borrow-check. PROPOSED only; the lexical formulation
    serves the C3 bootstrap minimum.
  - **Partial-move-through-field-projection soundness gap**
    (docs/borrow-check-limitations.md): a documented C2
    soundness hole. Highest-priority post-C2 work on the
    borrow-check side.

For **Phase C4** per HANDOVER §6.2: classes + traits + named
impls + delegation + structured concurrency. ADR 0021 is the
phase kickoff (PROPOSED). The sub-phase plan from ADR 0021
D14 (C4.0 complete; six total):

  - C4.0 lexer (1 session, **DONE**): class / trait / impl /
    init / delegate / scope / spawn / await / Self / self /
    as / for keywords reserved.
  - C4.1 class + method + init (2-3 sessions): parallel-
    tree mirror across AST / parser / resolve / types /
    codegen + definite-assignment check.
  - C4.2 traits + named impls (2-3 sessions): trait decls +
    impl blocks (default + named) + witness-table dispatch.
  - C4.3 delegation (1 session): auto-forwarder codegen for
    `delegate inner: T to Trait`.
  - C4.4 structured concurrency (2-3 sessions): Async effect
    + scope/spawn/await surface + runtime scheduler
    (substantial new runtime work; warrants ADR 0024 at
    sub-phase open).
  - C4.5 close-out (0-1 session): phase-go + ADR 0021
    PROPOSED → ACCEPTED flip.

Per-sub-phase ADRs follow ADR 0021: ADR 0022 (C4.1 surface),
ADR 0023 (C4.2 surface + dispatch), ADR 0024 (C4.4
scheduler + Async effect) — each PROPOSED at its sub-phase
open.

**C3.7 retrospective** (this session): ADR 0020 D9 estimated
0-1 sessions for C3.7. Actual: ~0.3 sessions. The substantive
piece was the body-restriction lift: `lower_handle` now accepts
any expression, wrapping pure i64 bodies via
sentinel_kont_pure so the dispatch loop is uniform. The empty-
arms case (`handle X with { return v => body }`) fell out from
the existing merge phi logic — just take the result type from
pure_val when arm_results is empty. The phase-go fixtures are
mechanical given the C3.5(c) let-shape coverage. **ADR 0020
flips PROPOSED → ACCEPTED-WITH-AMENDMENTS** with D2 multi-shot
deferred indefinitely (Phase B validation demos all worked
one-shot; the bootstrap minimum doesn't surface a multi-shot
use case). Phase C3 closes here.

**Pre-C3.7 — C3.6(b) retrospective**: ADR 0020 D9 estimated
1 session for C3.6(b). Actual: ~0.4 sessions. The substantive
piece was understanding what "inner emits Kont*" meant at the
IR layer + how arms/pure/propagate paths each contribute
Kont*s to the merge. Once the model was clear, the edit fell
out: arms wrap their i64 via sentinel_kont_pure; pure-path
passes current_kont through (or re-wraps after return arm);
default routes to a propagate block. Design notes:

  - **handle_depth counter vs handle_stack.len()**:
    handle_stack is pushed AFTER body lowering, so its
    .len() doesn't reflect "we're inside a handle's body
    lowering". A separate counter incremented at entry +
    decremented at exit gives the right reading during the
    body-lowering recursion.
  - **Wrap-then-unwrap cost**: nested handles allocate +
    free more konts than necessary (each arm wraps i64 →
    pure_kont; outer's pure-path consume_pures it). For C3
    minimum perf budget this is acceptable; a future ADR
    can revisit (e.g., elision when outer is statically
    known not to need the wrap).
  - **Type-check already supports this**: the partial-
    handle case (body has more effects than the handler
    catches) was already valid in the typing layer per
    ADR 0019 D2. C3.6(b) is purely a codegen change.

Workspace test delta: +2 tests (1072 total) — +2 driver
pass-tests (c36b_nested_handle_basic,
c36b_nested_handle_inner_full).

**Pre-C3.6(b) — C3.6(a) retrospective**: ADR 0020 D9 estimates
1-2 sessions for all of C3.6 (return arms + nested + multi-
shot). C3.6(a) alone was ~0.3 sessions — quick once the
`HandleContext` extension fell out. Design notes:

  - **Deep-handler re-wrap via k(v) path**: Phase B's `k :=
    \v. handle (kont.resume v) with H` semantics push the
    return arm into the k(v) call site. Without this, the
    return arm only fires for the pure-body case (which is
    rare in practice). With it, the return arm fires
    uniformly — matching what users expect when they write
    `return v => transform(v)`.
  - **HandleContext non-Copy**: storing TypedReturnArm
    (which contains TypedExpr — non-Copy) forced dropping
    the Copy derive. lower_resume_kont now snapshots via
    `last().unwrap().clone()` once at function entry.
  - **No re-typing pass needed**: the return arm's body was
    already typed during C3.4; codegen just lowers the
    existing TypedExpr.

Workspace test delta at C3.6(a) close: +2 tests (1070 total)
— +2 driver pass-tests (c36a_return_arm_transform,
c36a_return_arm_after_resume).

**C3.5(e) retrospective** (this session): ADR 0020 D9
estimated "1-2 sessions" for C3.5(e). Actual: ~1 session. The
substantive piece was the runtime + lower_handle restructure
to support bubble. Once `sentinel_kont_resume` returns a kont*
and the handle becomes a dispatch loop, chained-lets codegen
is straightforward — each resumer is a smaller chained-lets
fn. Design notes:

  - **Uniform Kont* return**: changing
    `sentinel_kont_resume`'s return type to `*mut SentinelKont`
    unifies the pure-return and bubble cases at the caller
    site — both go through the same `load op_id → check
    PURE_RETURN_OP_ID` check. The "always wrap final value
    in pure-return" cost is one extra alloc/free pair per
    `k(v)` call in trivial cases, which is acceptable at C3
    minimum perf budget.
  - **Alloca-backed dispatch slot**: using an alloca for
    `current_kont_slot` instead of an LLVM phi node avoided
    threading PhiValue references through the
    `handle_stack`. The phi-based alternative is equivalent
    and would generate slightly tighter IR after mem2reg.
  - **HandleContext stack**: lower_resume_kont consults
    `self.handle_stack.last()` to find its enclosing
    handle's loop block + slot. Pushed on lower_handle
    entry; popped on exit. Nested handles will need this
    plus an "ownership" rule (an inner handle's resumer
    shouldn't bubble to an outer handle's loop) — a C3.6
    concern.
  - **Captures: precise vs conservative**:
    `compute_chained_lets_captures` walks
    `lets[i+1..].rhs + tail` and excludes future-let-bound
    ids + the let_i value param. Conservative
    "everything-in-scope" would over-allocate; precise
    walking matches what each resumer actually reads.

Workspace test delta: +3 tests (1068 total) — +3 driver
pass-tests (c35e_chained_perform,
c35e_chained_perform_with_capture,
c35e_chained_dependent_perform).

**Pre-C3.5(e) — C3.5(d) retrospective**: ADR 0020 D9
estimated "1 session" for C3.5(d). Actual: ~1 session. The
unified approach (count_performs / find_unique_perform /
substitute_perform_with_var walkers + detect_embedded_perform_shape)
turned out to be a clean generalisation of C3.5(c)'s
per-let approach — they share the resumer codegen pattern
(alloca for placeholder + captureds; lower substituted body;
wrap via sentinel_kont_pure). The substitution walker is
~150 LOC of mechanical recursion through TypedExprKind variants.
Design notes:

  - **Disjoint shape detection**: let-shape (stmts.len() == 1
    with effecting let-RHS) and embedded-shape (stmts.len()
    == 0 with single embedded perform) don't overlap. compile_fn
    dispatches to either path or falls back to the C3.5(b)
    validate-and-lower path.
  - **Per-shape resumer map**: each shape gets its own
    HashMap entry. embedded_perform_resumers stores the
    substituted tail (as TypedExpr) + placeholder VarId
    alongside the resumer FunctionValue + captured VarIds.
    Stored at pass-1 detection time so compile_fn doesn't
    re-substitute.
  - **Placeholder VarId = u32::MAX**: synthetic constant
    chosen to avoid collision with resolve-assigned VarIds
    (which grow from 0). Each compile_fn run binds the
    placeholder in its own env; no cross-fn collision since
    env is reset per compile_fn.

Workspace test delta at C3.5(d) close: +3 tests (1065 total)
— +3 driver pass-tests (c35d_binop_with_perform,
c35d_perform_with_capture_and_binop, c35d_perform_in_call_arg).

**Pre-C3.5(d) — C3.5(c) retrospective**: ADR 0020 D9
estimated "1-2 sessions" for C3.5(c). Actual: ~1 session.
The substantive piece was the per-let resumer fn codegen +
captured-state struct layout. The runtime extensions
(SentinelFrame, sentinel_kont_push, resume's drain loop) were
small (~50 LOC added). The codegen for the let-shape was
larger (~200 LOC: detect_let_shape + collect_captured_vars +
walk_collect_var_refs + compile_effecting_fn_with_let). Design
choices:

  - **Pre-declare resumers in pass 1**, looked up by FnId in
    `let_resumers: HashMap<FnId, (FunctionValue, Vec<VarId>)>`.
    Avoids the lifetime gymnastics of holding a `&Module` in
    CodegenCtx; CodegenCtx still owns just the FunctionValues
    + captured-VarId lists.
  - **Captured struct via byte-offset GEP**, not LLVM struct
    type. Each captured var is an i64; the struct is `i64[N]`
    on the heap, accessed via `i8`-indexed GEP at offsets
    `0, 8, 16, ...`. Keeps the codegen layout-stable without
    needing per-fn LLVM struct types.
  - **Resumer signature uniform** `ptr (i64, ptr)` so every
    `sentinel_kont_push` call site shares one fn-pointer type
    on the runtime side. The resumer always returns a pure-
    return kont at C3.5(c) MVP (resumers don't perform); nested
    perform inside resumers lands at C3.5(d) / C3.6.
  - **MVP restrictions**: single let-stmt only (no chains of
    effecting lets); let-bound type must be i64; captured vars
    are filtered to fn params only (no earlier let-bindings
    since stmts.len() == 1).

Workspace test delta from C3.5(b) close: +1 test (1062 total)
— +2 driver pass-tests (c35c_let_bound_perform,
c35c_let_bound_perform_with_capture), -1 driver UI test
(c35b_effecting_fn_let_bound_perform was retired since the
shape it asserted now succeeds).

**Pre-C3.5(c) — C3.5(b) retrospective**: the effecting fn
ABI piece took ~1 session within the original 1-2 session
C3.5(b) budget. Three substantive design choices:

  - **Effecting fn ABI returns Kont*** (plain ptr, no
    struct). Avoids the calling-convention overhead of a
    multi-value return on every call. Tradeoff: a wrap is
    needed when the body is pure (handled via
    sentinel_kont_pure + the PURE_RETURN_OP_ID sentinel).
  - **Unified runtime switch in lower_handle**: the C3.5(a)
    static arm dispatch was retired in favour of always
    emitting a switch on the kont's op_id. Adds 4-5 LLVM
    instructions per handle but simplifies codegen and
    handles multi-arm cases naturally. The switch includes
    a dedicated PURE_RETURN_OP_ID case that calls
    sentinel_kont_consume_pure to unwrap pure values — the
    runtime expression of ADR 0020 D4's default
    `return v => v`.
  - **Validation, not transformation**, for the limited
    case. Effecting fns whose body's tail produces a kont
    (Perform / Call-to-effecting / Block-thereof) lower
    directly; pure tails get wrapped. Tails that mix
    perform with surrounding pure context (e.g.,
    `perform Op() + 1`) require frame reification and
    surface `effecting_fn_body_not_direct` — deferred to
    C3.5(c).

Workspace test delta from C3.5(a) close: +3 tests (1061
total). Three new pass fixtures (`c35b_handle_fn_call_body`,
`c35b_handle_multi_arm`, `c35b_handle_pure_return`) + one
UI fixture (`c35b_effecting_fn_let_bound_perform`); the
previous c34 UI fixture (asserting codegen rejected
do_work() bodies) was retired because that shape now
compiles + runs end-to-end.

**Pre-C3.5(b) — C3.5(a) retrospective**: ADR 0020 D9
estimated "2-3 sessions" for the full C3.5. Actual for the
restricted-case slice: ~1 session. Restriction shrank the
problem: with handle body fixed to a direct Perform, the
arm dispatch becomes a compile-time choice (no runtime
op_id switch needed), and frame reification at
intermediate evaluation sites is skipped entirely. The
kont struct (24-byte `{ op_id: u32, _pad: u32, arg: i64,
consumed: u8 }`) is the minimal payload that the
follow-on C3.5(b) work will extend with a frames vector.
The novel pieces:

  - **Static arm dispatch**: instead of emitting a switch
    on the kont's op_id at runtime, we lookup the matching
    arm at compile time (resolve guarantees uniqueness via
    DuplicateHandlerArm) and emit a direct call into the
    arm's body. Saves a load + switch in IR and keeps the
    restricted-case path minimal.
  - **GEP-based arg read**: the handler arm's op-param
    VarId is bound to the value at byte offset 8 of the
    kont struct (after `op_id: u32 + _pad: u32`). Codegen
    emits a `getelementptr i8` + `load i64` pair. Layout
    is asserted stable via a runtime test
    (`sentinel_kont_struct_layout_is_stable`).
  - **Opaque-pointer Type::Kont**: `llvm_basic_type` for
    `Type::Kont(_)` returns a plain `ptr`. The
    underlying struct layout lives in sentinel-runtime;
    codegen reads fields via byte-offset GEP rather than
    declaring an LLVM-level struct type.

Workspace test delta: +6 tests (1058 total) — +4 sentinel-
runtime (kont layout / initialisation / resume / round-trip),
+2 driver pass-tests (c35_*). The previous c34 UI fixture
(do_work() body) now surfaces the more specific
`handle_body_not_direct_perform` diagnostic — the driver
test was updated accordingly.

**Three C3 follow-ons are documented but deferred**:

  - **C3.6 — handle codegen for general case** (frame
    reification, the substantive runtime piece). Overlaps
    with C3.5(b) since the same call-site machinery is
    needed.
  - **Partial-move-through-field-projection soundness gap**:
    still open from C2; postfix `.field` on a Move-typed
    binding is non-consuming, leading to double-free at
    drop. Documented in
    `docs/borrow-check-limitations.md`.
  - **ADR 0018 (PROPOSED)**: Polonius migration plan, also
    from C2. Trigger is empirical friction.

**C3.4 retrospective** (kept for reference): ADR 0020 D9
estimated "1-2 sessions" for C3.4. Actual: ~1 session. The
AST + parser additions were mechanical given the C3.0(a)
reserved keywords (`handle`, `with`, `perform`) and the C3.2
effect data model. Two new lexer tokens were needed
(`FatArrow` / `=>` and `Return` contextual keyword) — small
lexer delta. The seventh interner table (`Type::Kont`)
followed the established Secret/Ref/GenericInstance pattern.

Alternative path (defer remaining handler runtime): start
**Phase C4** (traits + structured concurrency) per HANDOVER
§6.2 with a new ADR 0021 PROPOSED. C4 is larger in volume
but lower per-piece risk than the C3.5/C3.6 runtime work.
Either choice is defensible. The handler-runtime path
completes the Phase B effect-system vision and is the
natural "close what we started" move; the traits path is
the larger landing zone for the production-shape language.

C2 retrospective (estimate vs actual): ADR 0017 D9 estimated
"6-13 sessions across 5-6 sub-phases"; actual was ~6 sessions
across 6 sub-phases (split into C2.0.1 + C2.0.2 + C2.1 + C2.2
+ C2.3 + C2.4 + C2.5 = seven feat/docs commits). Low end of
the estimate range. C1's "1-session-per-sub-phase" rhythm
DIDN'T fully carry to C2 — C2.2 (&mut + XOR), C2.3 (move),
C2.4 (RAII drop) each used a full session because the borrow
checker is genuinely novel machinery. The infrastructure
investment compounded on the *surrounding* pieces (lexer +
parser + AST + resolve were trivial deltas); the substantive
work was in `sentinel-borrow-check` (~1500 LOC) and the codegen
drop emission. Notes captured in STATE.md decisions for
each C2.x sub-phase.

C1.7 retrospective (estimate vs actual): ADR 0011 D6 estimated
"4-6 weeks" (the longest single C1 sub-phase); actual was ~1
session across 5 commits (e411ded ADR 0016 PROPOSED + c1e5083
scaffolding + d32a9fe types + ad7e10d codegen + 2c6c652 generic
structs end-to-end). Faster than estimated, in line with the
C1.4/5/6 pattern. The substantive pieces:
(a) the interned-instance design choice (`Type::GenericInstance(
GenericInstanceId)` with the args in a program-level table)
preserved `Type: Copy` and avoided a ~30-site clone-cascade
refactor; (b) the eager-substitute approach for codegen
(`TypedFnDef::substitute` deep-clone + lower the substituted
def via the existing per-fn path) avoided the lazy-substitution
audit risk; (c) the unified `check_call` consolidation —
deleting the ~75 LOC of special-cased C1.5/6 builtin Call
branches — is the cleanest payoff of the C1.7 design. Notes
captured in STATE.md decisions 82-93.

C1.6 retrospective (kept for reference): "3-4 weeks" estimated;
~1 session actual. Notes in STATE.md decisions 74-81.

C1.5 retrospective (kept for reference): "2-3 weeks" estimated;
~1 session actual. Notes in STATE.md decisions 65-73.

C1.4 retrospective (kept for reference): "3-4 weeks" estimated;
~1 session actual. Notes in STATE.md decisions 54-64.

C1.3 retrospective (kept for reference): "2 weeks" estimated;
~1 session actual. Notes in STATE.md decisions 46-53.

**C1 overall retrospective**: ADR 0011 D6's honest 22-28 week
budget for all of C1 was generous; actual elapsed across C1.0a
through C1.7.4b was ~10-12 sessions (~5-6x faster). The
infrastructure investment (Salsa retrofit + per-pass crate split
+ parallel-tree pattern + per-sub-phase ADR-first discipline)
compounded heavily — each sub-phase reused the same scaffolding
and the same five-step rhythm (ADR → lexer → bundled AST/parser/
resolve/types/codegen → fixtures → docs). C2's region work
likely won't compound the same way (borrow checking is novel
machinery) but the ADR-first norm, the parallel-tree pattern,
and the salsa pipeline all carry forward intact.

C1.5 retrospective (kept for reference): "2-3 weeks" estimated;
~1 session actual. The bidirectional checking infrastructure
and the D4/D10 amendments were the highest-thought-cost pieces.
Notes in STATE.md decisions 65-73.

C1.4 retrospective (kept for reference): "3-4 weeks" estimated;
~1 session actual. The codegen value-type widening from
`IntValue<'ctx>` to `BasicValueEnum<'ctx>` was the substantive
change. Notes in STATE.md decisions 54-64.

C1.3 retrospective (kept for reference): "2 weeks" estimated;
~1 session actual. Notes in STATE.md decisions 46-53.

### 0.3 Quick-status block for session start

For pasting into a fresh chat to bootstrap context:

    Continuing Sentinel-lang work. Repo: https://github.com/arcanii/Sentinel-lang
    (Rust workspace under crates/, building the `snc` bootstrap compiler.)
    Local HEAD: verify with `git log -1` — expect the **self-host PARSER (2d-8)
    docs** commit (the parser-stage close), atop its feat
    (`feat(selfhost): parser (2d-8) — close the full corpus`, `d2da83c`), atop the
    feat+docs pairs for (2d-7) `class` (`447d9ab`) … (2d-1) `use` + dispatch
    (`debaa24`), the (2c) fn-level grammar ((2c-3) `b2a9c3b` … (2c-1) `b293c62`),
    the (2b) full-expression increments (`189990e` … `0e84f36`), the (2a) parser
    feat (`8d6aa6e`) + `snc ast` oracle (`7f10740`) + recursive-AST drop gate +
    ADR 0039, atop the lexer (1/N) + the D.6 cross-module work. (Run `git log
    --oneline -40` for the full chain.) Clean tree; **1410 tests** — the curated
    `selfhost_parse` seeds (192) + a new corpus differential
    (`sentinel_parser_matches_oracle_on_corpus`, the D8 phase-go: the Sentinel
    parser vs `snc ast` over all 139 clean-parsing `tests/pass`+`tests/ui`
    fixtures) + `tests/ast.rs` goldens for every decl kind; four-check green via
    `cargo nextest run --workspace` + `cargo test
    --doc --workspace` + `cargo clippy --workspace --all-targets -- -D warnings`
    (+ `cargo build`). macOS + LLVM 18.
    READ: docs/STATE.md top banner + HANDOVER §0/§0.1/§0.3 + **ADR 0039**
    (the (2/N) parser — **now ACCEPTED / COMPLETE**: (2a)+(2b) the full expression
    grammar, (2c) statements/types/fn-defs, (2d) the top-level decls + (2d-8) the
    full-corpus close — `selfhost/parser.sentinel` matches `snc ast` over every
    clean-parsing fixture, leak-free; **RESUME AT below = the NEXT port stage,
    resolve — its own ADR (write it PROPOSED first)**) +
    **ADR 0038** (the port's
    (1/N) lexer — DONE — + the differential-oracle method the parser reuses) +
    **ADR 0031** (the Phase D roadmap — movement 1 complete; D5 = the self-host
    sequence) + auto-memory sentinel_selfhost_port (+ sentinel_d6_modules_surface
    for the Path A merge).

    PHASE D = self-hosting (post-1.0; ADR 0031). Opens with a
    language/stdlib build-out, keeping the Rust `snc` as the differential
    oracle, converging on a byte-identical bootstrap fixed-point. **1.0 is
    DECLARED**; the headline 1.0 capability — constant-time `secret` — is
    delivered + machine-verified.

    Recent sessions (all four-check green, feat+docs pairs):
    - **D.1 — sum types (`enum`) + pattern matching (`match`)** MVP COMPLETE
      (ADR 0032 → ACCEPTED-WITH-AMENDMENTS): (1/N) lexer (`87e955c`), (2/N)
      AST + parser (`e368a72`), (3/N) the type layer — `Type::Enum(EnumId)` +
      construction + `match` type-check + **exhaustiveness** (`6381b21`),
      (4/N) codegen — `{i32 tag, ptr payload}` + an LLVM `switch` + box-free
      drop + `c5d1_enum` exit 42 (`cf44e9b`). ⚠ DEBT (A1): recursive-enum
      payload-field drop is box-free only — leak-free for the standard
      recursive-consume walk (verified via `leaks`), leaks only on
      bind-and-ignore / unmatched-drop; the real fix is the payload-ownership
      model (NOT drop fns alone — those double-free), bundle w/ D.1b. See
      [[sentinel_d1_enum_surface]].
    - **D.2 — strings + a `u8` byte type** MVP COMPLETE (ADR 0033 →
      ACCEPTED-WITH-AMENDMENTS): (1/N) lexer `StringLit`/`CharLit` (`21ff065`),
      (2/N) AST + parser + the parse-time escape decoder (`f310f15`), (3/N) the
      type layer — `Type::U8` + the cascade + `ArrayElem::U8`; char→`u8`,
      string→`[u8]`; `str_eq`/conversion builtins typed; the op-generic
      pipeline absorbs `u8` via one `is_int` change; codegen rejected the
      literals until 4/N (`56f69f0`), (4/N) codegen + runtime — `i8` +
      `udiv`/unsigned-compare; string heap-copy via byte-stores;
      `sentinel_str_eq`; `zext`/`trunc` conversions; `abi-v1` `u8` entry;
      `c5d2_strings` + `c5d2_u8_unsigned` phase-gos at exit 42, **0 leaks**
      (`891ec98`). A string IS a `[u8]`; `u8` IS `i8`. ⚠ builtins are now
      `FnId(0..=6)` → user fns start at 7; inline string-lit ARGS to borrowing
      builtins leak (the pre-existing temp-drop gap — bind them). See
      [[sentinel_d2_strings_surface]].
    - **ADR 0034 PROPOSED** (`86b7f9c`): Phase D.3 kickoff — growable
      collections. A `Vec<T>` is `[T]` + capacity + mutation
      (`Type::Vec(VecElem)` mirrors `Type::Array(ArrayElem)`); `String` =
      `Vec<u8>`; `push(&mut v, x)` is the first heap-mutation primitive; no
      new generics / no lexer-parser change. 1311 tests.
    - **D.3 (1/N) — growable `Vec<T>`** COMPLETE (`a64883c` feat; ADR 0034
      stays PROPOSED, (1/N) Amendments recorded): `Type::Vec(VecElem)` + the
      cascade; `vec_new<T>()->Vec<T>` / `push<T>(&mut Vec<T>,T)->i64` /
      `len` (overloaded over `[T]`+`Vec<T>`), typed + codegen
      (`{i64 len,i64 cap,ptr data}`, data is field 2) + the `sentinel_realloc`
      growth runtime + `&mut Vec` mutable-borrow + primitive-element drop.
      End to end: `c5d3_collections` exit 67, **0 leaks** (multi-growth +
      a Vec moved out of a helper). vec_new/push flow the uniform
      generic-call path (explicit `(Vec,Vec)` `unify_one` arm); builtins
      shift the FnId base (main 7→9). Amendments A1–A5 (String→2/N,
      return-type pushdown, len special-case, sentinel_realloc,
      VecElementNotSupported). 1324 tests. See
      [[sentinel_d3_collections_surface]].

    HISTORICAL (movement-1 close — SUPERSEDED; the current resume point is at
    the TOP of §0.3 above = the parser stage is COMPLETE, next is resolve):
    **Phase D.6 (1/N) — modules / multi-file — per
    ADR 0037.** Modules was the **last** ADR 0031 D4 language prerequisite
    before the lexer→parser→… self-host port (D5). **Phase D.5 — loops — is
    COMPLETE** (ADR 0036 → ACCEPTED-WITH-AMENDMENTS: (1/N) `while` + (2/N)
    `break`/`continue`; exit 67 + 115, 0 leaks) — recap in
    [[sentinel_d5_loops_surface]].
    **D.6 (1/N) PROGRESS (2 increments committed this session):**
    (i) **front-end** (`d02f4a3`) — reserved `use` lexer keyword + `UseDecl`
    AST + `Program.uses` + parser (`use a::b::Item;`, ≥2 segments, `;`);
    resolve gates a non-empty `uses` with `UseDeclNotYet` (backstop).
    (ii) **module-graph discovery** (`e7db419`) — the DRIVER follows `use`
    edges from the entry, mapping `use a::b::Item` → module `a::b` → file
    `<root>/a/b.sentinel` (source root = entry's dir; last segment = item;
    cycles fine); a missing `use`d file → ModuleNotFound; single-file
    unaffected; a discovered multi-module graph is REPORTED + GATED at the
    driver pending per-unit wiring. `discover_module_graph` in
    sentinel-driver/main.rs; tests in `tests/modules.rs`.
    (iii) **top-level `pub`** (`26dfb5a`) — `visibility: Visibility` on
    FnDef/StructDecl/EnumDecl/TraitDecl/EffectDecl, parsed from a `pub`
    before the item (`pub` on use/class/impl rejected); the C4.1 no-op
    single-file.
    (iv) **cross-module import resolution + visibility** (`fe9d110`) —
    sentinel-resolve `ModuleUnit` + `resolve_imports`: each `use a::b::Item`
    must name a graph module `a::b` declaring a `pub` `Item`, else
    `ModuleNotFound`/`UnknownImport`/`PrivateItem`. `discover_module_graph`
    now carries each module's `Program` (entry first); the driver validates
    imports BEFORE the multi-module gate. 1376 tests, green.
    **ARCHITECTURE DECISION (owner, this session): take the lower-risk
    PATH A** — whole-graph front-end + per-unit codegen/link — and reach true
    per-unit resolve EVENTUALLY (folded into the (3/N) caching sub-phase).
    (v) **the merge — MULTI-FILE COMPILES + RUNS** (`db9b6e4`) —
    `sentinel_resolve::merge_modules` merges the graph into ONE `Program`:
    qualifies each module's fn names by module path (`util::add` → symbol
    `util$add`; `$` is collision-free) + rewrites cross-module + own call
    refs per module scope (an exhaustive, compiler-checked `rewrite_expr`
    walk) + keeps the entry's `main`; the driver compiles the merged program
    via direct pipeline calls (`run_build_merged`) → one object → link.
    VERIFIED end-to-end: cross-module `pub fn` call (exit 5); same-named
    private `helper`s in two modules coexist (exit 41, 0 leaks); private
    import → PrivateItem. 1378 tests, green.
    (vi) **cross-module TYPES** (`3571ec2`) — `merge_modules` now also
    qualifies struct + enum names by module path (`geo::Point` → `geo$Point`)
    + rewrites EVERY type reference per module scope: all signature TypeExprs
    (params/returns/fields/variant payloads/trait+effect+class+impl sigs +
    delegate types), `let` annotations, struct literals, enum construction
    (`QualifiedCall`/`ClassInit` heads), and `match` variant patterns — via a
    per-module `Renamer` (the name map + the in-scope type-param set, which is
    NEVER qualified). Trait/effect/class NAMES stay unqualified (cross-module
    trait/effect *use* is the next increment) but their bodies' refs to
    qualified structs/enums ARE rewritten. VERIFIED end-to-end: a
    cross-module `pub struct` (annotation/literal/arg, exit 42); a
    cross-module `pub enum` + `match` (exit 52); same-named `struct Item`s
    across modules coexist (exit 42); a cross-module struct as another
    module's field type, 3 modules deep (exit 42). 1384 tests, green.
    (vii) **cross-module TRAITS / EFFECTS / CLASSES** (`7fd7817`) — the same
    `Renamer` now also qualifies trait + effect + class + named-impl names
    (so EVERY top-level item kind is qualified; same-named ones coexist) +
    rewrites their refs: `impl as Trait for Type` heads (both names),
    `perform`/`handle` effect names, fn/method/trait/impl effect rows
    (`! { Net, Io }`), delegate `to Trait` names, and named-impl
    `QualifiedCall` heads. Op + method names stay unqualified (scoped within
    their effect/trait). The import gate (`is_qualified_kind`) is dropped —
    every validated `use` maps to its qualified symbol. VERIFIED end-to-end: a
    cross-module `pub trait` impl'd + dispatched (exit 42); same-named
    `class`es coexist (exit 42); a cross-module `pub effect` performed in one
    module + handled in the entry through the handler runtime (exit 42). 1389
    tests, green.
    (viii) **cross-module GENERICS verified** (`53a9aba`, test-only) — under
    Path A the merged graph is one Program, so `collect_mono_instantiations`
    runs whole-program and a generic instantiated in a DIFFERENT module than
    its definition is emitted like any single-file instance. Pinned by 3
    fixtures: a `Box<i64>` struct, an `id<T>` fn, and `Pair`/`make_pair`/`fst`
    over `Pair<i64, i64>` — all exit 42. (The true per-unit `linkonce_odr`
    story is still ADR 0037 (2/N).) 1392 tests, green.
    (ix) **effect-check parity** (`7af1dce`) — `run_build_merged` now calls the
    pure `sentinel_effect_check::effect_check(&typed)` between type-check and
    borrow-check (matching the salsa `borrow_check_query`→`effect_check_query`
    chain the single-file path uses), rejecting on any `EffectError`. Added the
    `sentinel-effect-check` dep to the driver. So a multi-file `main` with an
    unhandled effect is rejected (new negative test), not miscompiled; the
    well-formed handle-discharges-it program still runs. 1393 tests, green.
    **D.6 IS DONE ENOUGH — movement 1's language gate is CLEARED.** The merged
    path is complete + sound (multi-file + every cross-module item kind +
    generics + effect-check). Its remaining follow-ups are now **independent
    deferred tracks, NOT blockers**: (i) the per-unit separate-compilation back
    end (per-unit `.o` + module-qualified length-prefixed `abi-v1` mangling +
    multi-object link + per-unit `linkonce_odr` generics, ADR 0037 (2/N)); (ii)
    span-accurate multi-source diagnostics. They can be done whenever; the
    self-host port does not need them.
    **SELF-HOST PORT (1/N) — LEXER — DONE.** Movement 2 of Phase D (ADR 0031 D5)
    — port `snc` to Sentinel stage by stage, each **differentially validated
    against the Rust `snc` oracle**. The lexer landed in three pieces:
    `snc lex <file>` (the oracle: a canonical token dump, one line per token
    `<KIND> <start> <end> [<lexeme>]`, variant *names*, `<lexeme>` only for
    `Ident`/`IntLit`/`StringLit`/`CharLit`, trailing `EOF`; golden-tested in
    `tests/lex.rs`); `selfhost/lexer.sentinel` (the first compiler stage in
    Sentinel, all 69 `TokenKind`s, emitting the dump directly into a `Vec<u8>` —
    no Token enum yet); and `tests/selfhost_lex.rs` (compiles the Sentinel lexer
    + asserts its dump == `snc lex` for all 139 clean-lexing fixtures). Amend:
    A1 direct emission (no Token enum); A2 two Sentinel quirks (flat per-fn var
    namespace → unique branch locals; deep-if tail `&mut` borrow conflict →
    compute-then-emit-once); A3 lex-error parity deferred (the one `@` fixture is
    excluded); A4 reads a fixed `./input.sentinel` (no argv yet — the test sets
    the cwd).
    **SELF-HOST PORT (2/N): the PARSER — ADR 0039 ACCEPTED-WITH-AMENDMENTS; (2a)
    + (2b) expressions + (2c) statements/types/fn-defs COMPLETE (the fn-level grammar).** ✅ `snc ast` oracle (`run_ast`+`ast_dump.rs`,
    golden `tests/ast.rs`) → the regular S-expr target, e.g. `(fn main () i64
    (block (binop + (int 1) (binop * (int 2) (int 3)))))`. ✅ recursive-AST drop
    gate (`tests/pass/selfhost_ast_drop.sentinel`, 0 leaks → no D.1b needed). ✅
    `selfhost/parser.sentinel` (the 2nd Sentinel stage). **(2a):** paramless
    `fn`-bodied integer arithmetic (precedence/parens/left-assoc/multi-fn). **(2b)
    increment-1 (A4):** the COMPLETE operator-precedence ladder mirroring the Rust
    parser (`expr → or → and → cmp → bitor → bitxor → bitand → add → mul → unary →
    atom`) — `|| &&`, the six non-assoc comparisons `== != < <= > >=`, bitwise
    `| ^ &`, prefix unary `- !`, + the scalar atom leaves (int / `true` / `false` /
    `null` lits + variable refs). `Expr` gained `Bool`/`Null`/`Var([u8])`/`Unary` +
    a unified `Binary(op-code,…)` (the i64 op-code encodes dump category + symbol).
    **(2b) increment-2 (A5):** function calls `f(args)` → `(call …)` (an ATOM case
    — the callee is a name, not an expr; only postfix `.m(args)` calls a value) +
    the POSTFIX chain (`parse_postfix` between `parse_unary` and `parse_atom`): field
    `t.field`, index `t[i]`, method `t.m(args)`. Arg lists = a **second
    mutually-recursive cons-list enum** `Args = End | Cell(Expr, Args)` (since
    `Vec<non-primitive>` is unsupported — de-risked by a probe; `Expr` gained
    `Call`/`Method`/`Field`/`Index`). **(2b) increment-3 (A6):** the `::` paths in
    `parse_atom` after an ident — `A::b(args)` → `(qcall …)`, `Name::init(args)` →
    `(class-init …)`, paren-less `Enum::Variant` → a qcall with empty args — plus
    array literals `[e1, …]` → `(array …)` (atom-position `[`, vs the postfix index
    `[`). `Expr` gained `Qcall`/`ClassInit`/`Array`; `parse_args` generalised with a
    terminator-tag param; tokenizer gained `::` (30) + `:` (31). Matches `snc ast`
    over **59 seeds** (now incl. `::` paths + arrays + nests `[A::b(), c.d][0].e`),
    leak-free. **(2b) increment-4 (A7):** `if <cond> { <then> } else { <else> }` →
    `(if cond (block then) (block else))` — dispatched at the TOP of `parse_expr`
    (a full expression, never an operator operand), `else` mandatory, `else if`
    chains — plus brace blocks `{ <expr> }` → `(block …)`. `Expr` gained `If` +
    `BlockE` (a statement-FREE block = just its tail; statements at (2c)); `if` /
    `else` tagged in the tokenizer (32 / 33). Matches `snc ast` over **68 seeds**
    (now incl. `if`/`else if` chains, nested `if`, blocks), leak-free. **(2b)
    increment-5 (A8):** `match <scrut> { pat => body, … }` → `(match scrut (arm pat
    body)…)` — also dispatched at the top of `parse_expr` (a `match` keyword tag);
    arm bodies are exprs; patterns are `_` → `(pat _)` or `Enum::Variant(b1, b2)` →
    `(pat Enum Variant b1 b2)`. The deepest mutual recursion yet — four enums
    (`Expr → Arms → {Pattern → Binds, Expr}`, de-risked by a probe); `Expr` gained
    `Match` + the `Arms`/`Pattern`/`Binds` enums; tokenizer gained `match` (34) +
    `=>` (35). Matches `snc ast` over **78 seeds** (now incl. `match`/patterns +
    an AST-walker shape), leak-free. **(2b) increment-6 (A9):** struct literals
    `Name { f: v, … }` → `(struct-lit Name (field f v)…)`, disambiguated from an
    `if`/`match` head's block by a **context-free `{ Ident :` lookahead** (a block
    never starts with a single-colon `Ident :`) — NO `allow_struct_lit` threading.
    `Expr` gained `StructLit` + a `Fields` cons-list (empty `Name {}` deferred).
    Matches `snc ast` over **88 seeds** (now incl. struct lits + head-disambiguation),
    leak-free. **(2b) increment-7 (A10):** the effect / concurrency leaf forms —
    `declassify(e)`, `perform Eff.op(args)` → `(perform Eff op args…)`,
    `scope concurrent { block }` → `(scope (block …))`, `spawn <call>`, and the
    `.await` POSTFIX → `(await target)`. `Expr` gained
    `Declassify`/`Perform`/`Scope`/`Spawn`/`Await` + 5 keyword tags (scope/while
    bodies stay statement-free until (2c)). Matches `snc ast` over **102 seeds**,
    leak-free. **(2b) increment-8 (A11) — CLOSES the expression grammar:** `handle
    <body> with { Eff.op(params) => arm, … return v => arm }` → `(handle body (arm
    Eff op armbody)… (return v body))`. Handler-arm params parsed but NOT dumped;
    the optional `return` arm kept SEPARATE (a `&mut Ret` out-param — `*ret =
    Ret::YesRet(…)`, the first non-primitive `&mut` assignment, de-risked by a
    probe) so it dumps LAST regardless of source order. `Expr` gained `Handle` +
    `HArms`/`Ret`; tags `handle`(41)/`with`(42)/`return`(43). Matches `snc ast` over
    **110 seeds** (incl. return-not-last); leak-free. **Every `ExprKind` now parses.**
    **(2c-1) (A12):** a block is now `{ <stmt>* <tail> }` → `(block <stmt>… <tail>)`
    — `let [mut] name = e` → `(let [mut] name _ e)`, `target = e` → `(assign …)`,
    `while cond { body }` → `(while …)`, `break`/`continue`, expr-stmt → `(expr e)`;
    the fn body is a real block. Block tail = a `&mut Expr` out-param defaulting to a
    NULLARY `SynthZero` (dumps `(int 0)` — the synth tail for a stmt-only `while`
    body; a boxed `Int(0)` default leaked since `*tail = e` doesn't free the old
    enum). `Expr` gained `Block(Stmts, Expr)`/`SynthZero` + `Stmts`/`Stmt`; tokens
    `;`(44) + let/mut/while/break/continue (45–49). **122 seeds**, leak-free.
    **(2c-2) (A13):** the optional `let` `: type` annotation → `(let [mut] name
    <type> e)` (vs `_`), via a `parse_type` mirroring the Rust one (`secret T` /
    `&T`/`&mut T` / `?T` / `[T]` / `Ident` / `Ident<args>`); nested generics close
    without a `>>` split (the tokenizer has no `>>`). New `TypeE`/`TyArgs`/`TyOpt`
    enums; tokens `?`(50) + `secret`(51). **135 seeds**, leak-free.
    **(2c-3) (A14) — CLOSES the fn-level grammar:** full `fn` defs — `fn name <T>?
    ( [mut] p: T, … ) -> RET ! { eff }? { body }` → `(fn name ((param [mut] p
    <type>) …) <ret> <block>)`; the return type routes through `parse_type` (so
    `[u8]`/`?T`/`Vec<T>` returns dump right); generic type-params + the effect row
    are parsed-and-SKIPPED (the dump emits neither). New `Params` enum. **148 seeds**,
    leak-free. ⚠ Sentinel `if` is an EXPRESSION (every branch needs a tail + an
    `else`) — `skip_type_params` is `depth = if … else if … else …` in a `while
    depth > 0` loop.
    🔑 **PROVEN STRUCTURE (reuse for (2d)):** recursive `Expr` enum
    returned BY VALUE + CONSUMING recursive `match` dump (`Vec<non-primitive>` is
    unsupported, so NO arena/value-stack); recursive-descent helpers share the
    token arrays + `src` as `&Vec<i64>`/`&[u8]` indexed via **`(*r)[i]`** (auto
    `r[i]` fails) with a **`&mut i64` cursor**; left-assoc folds via **recursion**
    (`parse_X` + `parse_X_rest(acc)` — a loop accumulator trips moved-in-loop);
    match arms comma-separated; flat per-fn unique locals; the dump computes
    prefix+symbol as `[u8]` values FIRST then emits (sibling `&mut out` borrows in
    `if` tails read as overlapping). parser.sentinel is self-contained (its own
    minimal tokenizer); sharing the full lexer via a D.6 module is a follow-on.
    **RESUME HERE → (2d) the TOP-LEVEL DECLS (the LAST parser slice).** (2b) + (2c)
    are COMPLETE — every `Stmt`/`Expr`/`TypeExpr`/`Pattern` + the fn header now parse,
    over **148 seeds**. (2d) adds the remaining top-level item kinds beyond `fn`:
    **struct** (`struct Name { f: T, … }`), **enum** (`enum Name { V, V(T), … }`),
    **trait** (`trait T { fn sig; … }`), **impl** (`impl T for Ty { … }` + named
    impls), **class** (`class Name { fields; init; methods; delegate }`), **effect**
    (`effect E { op(…) -> T; … }`), and **use** (`use a::b::Item;`). ⚠ This needs the
    oracle side too: ADR 0039 D2/A1 — `snc ast`'s `dump`/`dump_program` currently
    emits only the `fns` (the seed corpus is fn-only); **extend `ast_dump.rs` to dump
    every decl kind in source order** (the (2d) part of D2), then mirror each in
    `selfhost/parser.sentinel`. `main`'s top loop currently assumes every item is a
    `fn` (it does `cur+1` to skip `fn`); generalise it to dispatch on the leading
    token (`fn`/`struct`(7→tag?)/`enum`/`trait`/`impl`/`class`/`effect`/`use`/`pub`).
    Note `pub` is contextual + `use`/`enum`/`match`/etc. are already lexer keywords;
    several decl keywords (struct/trait/impl/class/effect/pub/use) need tokenizer
    tags. Land it incrementally (one decl kind per increment, each oracle-validated),
    closing toward the full `tests/pass` + `tests/ui` corpus (D8). Once (2d) matches
    the corpus, the parser stage is DONE — next port stage = resolve (its own ADR).
    The goal (D8): `selfhost/parser.sentinel` matches `snc ast` over the whole
    `tests/pass` + `tests/ui` corpus, like the lexer does for `snc lex`.
    Back-end-agnostic (Path A merge); Rust `snc` stays the oracle. **Before coding:
    read ADR 0039 + its Amendments A1–A14.**
    **ADR 0037 — settled decisions (with the language owner):** (a) **module
    surface = file-as-module + `use`** — a file *is* a module, its path
    relative to the source root (the entry file's dir) *is* its module path;
    `use a::b::Item;` imports a `pub` item; `pub` (parsed since C4.1, a no-op)
    becomes the visibility gate; NO `mod` blocks. (b) **compilation model =
    TRUE separate compilation** (not whole-program merge) — each module → its
    own `.o`, cross-module refs resolved at link via stable `abi-v1`-keyed
    symbols. **Why it's the biggest D-change:** it breaks 3 whole-program
    codegen assumptions — `collect_mono_instantiations` (whole-program mono
    discovery), the single `fns: HashMap<FnId, FunctionValue>` map, and
    `self.fns.get(&id)` call resolution (a cross-unit callee is unknown) — and
    turns cross-unit symbols into ABI surface (the bare-source-name mangling is
    single-file-only + not collision-free; D7 = a module-qualified,
    length-prefixed `abi-v1` mangling amendment). **Sub-phase split (ADR 0037
    D9):** **(1/N)** the surface (`use` token + parse + top-level `pub`) + the
    resolve **module graph** (discover files by following `use`; per-unit ID
    spaces + namespaces; visibility) + per-unit type-check against imported
    **signatures** + **non-generic** separate compilation (per-unit `.o`,
    module-qualified mangling, extern-symbol cross-module calls + types,
    deterministic link); **(2/N)** **cross-module generics** (per-unit
    instantiation + `linkonce_odr` dedup — the C++ template model; `pub`
    generic bodies cross the boundary) + cross-module trait/impl methods;
    **(3/N)** incremental caching (Salsa) + per-unit `.o` repro. Emit/link
    today: parse→…→codegen to ONE LLVM module `"sentinel"` → ONE `.o` → `cc`
    links it + `libsentinel_runtime.a` (`compile_to_object` in
    sentinel-codegen; `link()` in sentinel-driver/main.rs). **Before coding:**
    re-read **ADR 0037** (esp. D3 resolve graph, D5 codegen-per-unit, D6
    cross-module generics → (2/N), D7 mangling) + the emit/link path above +
    **ADR 0029** (the frozen `abi-v1` mangling/symbol tests the D7 amendment
    must update in the same commit). See [[sentinel_d5_loops_surface]].

    CARRIED-FORWARD DEBT (not blocking D.3): **D.1 A1 — recursive-enum
    payload drop is box-free only** (leak-free for the standard
    recursive-consume walk; leaks only on bind-and-ignore / drop-unmatched;
    NO UAF). The real fix is the payload-ownership model (match-consumes-
    scrutinee + drop-plan-registered bindings, NOT drop fns alone — those
    double-free), best bundled with **D.1b** (generic enums `Option`/`Result`
    + the leak-completeness fix). See [[sentinel_d1_enum_surface]]. Also
    **droppable-element `Vec` drop** (`Vec<Struct>` / `Vec<[u8]>` per-element
    free — ADR 0034 D8; the enum-A1-shaped follow-on; primitive-element Vecs
    are leak-free today).
    ROADMAP (ADR 0031 D4): after D.3 — file I/O (stdlib) -> modules -> loops,
    then the self-host port (lexer -> parser -> ... in Sentinel,
    differentially validated against the Rust `snc` oracle).

    DEFERRED (none blocking; recorded in ADRs): C5.2a/D4 constant-time
    EMISSION (branch-free arithmetic/bitwise already passes D5 on existing
    codegen → D4 likely scoped out of 1.0; ADR 0026 flips once settled);
    bitwise SHIFTS `<< >> ~` (ADR 0027 A1; need the `>>`/nested-generic-close
    split); `[secret T]` arrays (would activate the broker secret-memory
    policy); full escape analysis; `scope budget(N)` surface; cross-process /
    modules / actors — all post-1.0 per ADR 0025.

    ADR STATUS: 0025 **ACCEPTED-WITH-AMENDMENTS** (Phase C5 kickoff — closed
    at Sentinel 1.0; C5 sub-phase roll-up). 0026 PROPOSED (C5.1/C5.2; flips
    when the deferred D4 emission/HIR-MIR migration lands post-1.0). 0027
    ACCEPTED-WITH-AMENDMENTS (bitwise; shifts deferred). 0028
    ACCEPTED-WITH-AMENDMENTS (broker scope arenas; A2 corrects the "UAF
    hole"). 0029 ACCEPTED-WITH-AMENDMENTS (stable abi-v1; frozen + tested).
    0030 **ACCEPTED-WITH-AMENDMENTS** (the 1.0 go/no-go — runs + passes D5;
    3/N declared 1.0). 0031 **PROPOSED** (Phase D kickoff — self-hosting;
    honest readiness verdict + the language/stdlib prerequisite roadmap;
    first sub-phase D.1 = sum types + `match`). 0032 **ACCEPTED-WITH-
    AMENDMENTS** (D.1 sum types + pattern matching MVP — 1/N lexer + 2/N
    AST+parser + 3/N type layer + 4/N codegen; `enum`/`match` compile + run
    end to end. A1: recursive payload-field drop deferred — box-free drop
    leaks recursive-enum nested boxes; needs synthesized per-enum drop fns.
    A2: inline-small-enum opt deferred. A3: generic enums → D.1b). 0033
    **ACCEPTED-WITH-AMENDMENTS** (D.2 strings + a `u8` byte type MVP — 1/N
    lexer + 2/N AST+parser + 3/N type layer + 4/N codegen/runtime; a string IS
    a `[u8]`, `u8` is an integer scalar; char/string literals + indexing +
    `str_eq` + `u8`↔`i64` conversions compile + run end to end, leak-free. A1:
    string heap copy via direct byte-stores not a global (`CodegenCtx` lacks
    `&Module`). A2: inline string-literal args to borrowing builtins inherit
    the pre-existing temporary-drop gap; bound vars are leak-free. A3: `str_eq`
    args borrowed not consumed). 0034 **ACCEPTED-WITH-AMENDMENTS** (D.3
    growable collections — the MVP is complete: `Vec<T>` is `[T]` + capacity
    + mutation, `Type::Vec(VecElem)` mirrors `Type::Array(ArrayElem)`,
    `String` = `Vec<u8>`; no new generics / no lexer-parser change.
    **(1/N)** `Type::Vec` + cascade + `vec_new`/`push`/`len` +
    `sentinel_realloc` growth + `&mut Vec` borrow + drop (`a64883c`);
    **(2/N)** `v[i]` (reuses Index, field-2 data ptr) + `pop` +
    `vec_to_array` (the `Vec<u8>`->`[u8]` bridge) + `String` + the
    comprehensive `c5d3` phase-go (`8430b0a`, exit 55 / 0 leaks).
    Amendments A1–A5 (1/N) + B1–B3 (2/N) in the ADR. Builtins FnId 0..=10.
    DEFERRED (D8): `Map`, droppable-element `Vec` drop, generic-fn `Vec`,
    `with_capacity`/`insert`/slicing/iterators, `secret Vec`, broker-backing).
    0035 **ACCEPTED-WITH-AMENDMENTS** (D.4 file I/O — the MVP is complete:
    `read_file`/`write_file`/`print_bytes` as runtime builtins like `print`,
    NOT algebraic effects (D2 amends ADR 0031 D4's "effects + handlers");
    panic-on-failure (D5); paths + content are `[u8]`; `std::fs`-backed
    `sentinel_*` wrappers join abi-v1 (23 symbols). **(1/N)** `read_file` +
    `write_file` (`2c530f6`); **(2/N)** `print_bytes` (`fb1b51b`); c5d4 exit
    5 / stdout "hello" / 0 leaks. The 3 open design points resolved at the
    proposed defaults; Amendments A1 (std::fs not raw libc) + A2 (out-param
    read ABI) + B1 (print_bytes: exact bytes, no newline, flushed). Builtins
    FnId 0..=13. DEFERRED (D8): recoverable errors / `Io` effect row /
    streaming / `read_stdin` / directories).
    0036 **ACCEPTED-WITH-AMENDMENTS** (D.5 loops — `while` + `break`/
    `continue`. (D3) a loop is a STATEMENT (`StmtKind::While`/`Break`/
    `Continue`), not an expression; (D4) the first backward CFG branch
    (cond→body→cond); (D5) per-iteration body-scope drop; (D8) the loop-
    carried move rule — reject moving an outer Move-typed binding inside a
    `while` body (`MovedInLoopBody`). **(1/N)** shipped `while` end to end
    (`adec9c3`, c5d5_loops exit 67); Amendments A1 (stmt-only body →
    synthesised unit tail) + A2 (entry-block alloca hoist via `loop_depth`).
    **(2/N)** shipped `break`/`continue` (this session) — payload-free
    statements branching to the innermost loop's `loop_after`/`loop_cond` via
    a loop-target stack; the load-bearing **drains-before-branch** (drop every
    scope frame down to the loop body before branching — `emit_loop_exit_drops`
    /`emit_frame_drops`; leak-free with a heap binding live at the branch);
    first mid-block divergence parks on an `after_loopctl` dead block;
    out-of-loop rejection (`LoopControlOutsideLoop`) via a `loop_depth` on the
    type env. Amendments C1–C5 (C5 = the conditional-break tail-idiom
    ergonomic note). c5d5_break_continue exit 115 / 0 leaks; 1361 tests. No new
    `Type` / no FnId-shift; `break`/`continue` are new lexer tokens. DEFERRED
    (D8): `for`/ranges/iterators, labeled break, `break`-with-value /
    loop-as-expression, a termination check).
    Optional C4 follow-ons (none blocking): work-stealing scheduler
    (ADR 0024 A1), scope cancellation (A2), Task<T>/spawn-args beyond i64
    (A3), Path-3 bounded-generic dispatch (ADR 0023 A1).
    Branch state: verify with `git status` at session start.

---

## 1. Scope of This Document

This is not a specification of Sentinel. The design documents are the
specification, and they are still partly under-specified by intent. This
document covers how to *build* the bootstrap compiler: environment,
tooling, architecture, milestones, testing, and the order in which to
attack the work.

The audience is a senior engineer or small team (one to three people)
with Rust experience and some compiler or PL background. If you have not
written a compiler before, plan to spend the first two weeks reading
"Crafting Interpreters" and the rustc dev guide before starting on
milestone zero.

---

## 2. Strategic Approach

Do not start by building the full Sentinel compiler. The design document
explicitly recommends a staged validation: prove the broker idea works
as a Rust library, prove the effects system works as a research
prototype, and only then commit to building the full language. This
handover document follows that staging.

The milestones below are organized as four phases. Phase A and Phase B
are the validation prototypes. Phase C is the bootstrap compiler proper.
Phase D is the path to self-hosting. Each phase has a clear go/no-go
decision point at the end. Do not skip the decision points. If Phase A
produces a broker that nobody wants to use, building Phase C is wasted
effort.

The expected calendar time, with a small focused team, is roughly:
Phase A six months, Phase B nine months (overlapping the second half of
Phase A), Phase C twelve to eighteen months, Phase D another nine to
twelve months. This is honest, not optimistic. Most language projects
underestimate by 2-3x; budget accordingly.

---

## 3. Environment Setup (macOS)

### 3.1 Toolchain

Install Rust via rustup. Use the stable channel for the compiler itself
and pin the version in `rust-toolchain.toml` so contributors get
reproducible builds.

    curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
    rustup default stable
    rustup component add rustfmt clippy rust-analyzer
    rustup target add aarch64-apple-darwin x86_64-apple-darwin

Install LLVM via Homebrew. Pin to a specific major version because
`inkwell` and LLVM's C API are version-coupled.

    brew install llvm@18
    echo 'export PATH="/opt/homebrew/opt/llvm@18/bin:$PATH"' >> ~/.zshrc
    echo 'export LLVM_SYS_180_PREFIX=/opt/homebrew/opt/llvm@18' >> ~/.zshrc

Install supporting tools:

    brew install cmake ninja just ripgrep fd jq
    cargo install cargo-nextest cargo-insta cargo-deny mdbook

`just` is used as the command runner instead of `make`. `cargo-nextest`
is significantly faster than the default test runner for compiler test
suites. `cargo-insta` is used for snapshot testing of compiler output.

### 3.2 Repository Layout

Create a single workspace repository. Do not split into multiple repos
yet; the dependency churn early on will make multi-repo intolerable.

    sentinel/
    ├── Cargo.toml              # workspace manifest
    ├── rust-toolchain.toml
    ├── justfile
    ├── .github/workflows/
    ├── docs/
    │   ├── SENTINEL_DESIGN.md
    │   ├── SENTINEL_DESIGN2.md
    │   └── HANDOVER.md         # this file
    ├── crates/
    │   ├── sentinel-broker/        # Phase A deliverable
    │   ├── sentinel-effects-proto/ # Phase B deliverable
    │   ├── sentinel-syntax/        # lexer + parser + CST
    │   ├── sentinel-ast/           # AST types
    │   ├── sentinel-resolve/       # name resolution
    │   ├── sentinel-types/         # type/region/effect checking
    │   ├── sentinel-hir/           # typed HIR
    │   ├── sentinel-mir/           # SSA-form IR
    │   ├── sentinel-codegen/       # LLVM lowering
    │   ├── sentinel-driver/        # the `snc` binary
    │   ├── sentinel-runtime/       # the broker as runtime library
    │   └── sentinel-lsp/           # language server
    ├── tests/
    │   ├── ui/                     # compile-error tests
    │   ├── pass/                   # programs that should compile and run
    │   └── snapshots/              # insta snapshots
    └── examples/

### 3.3 Initial Workspace Manifest

The top-level `Cargo.toml` declares the workspace and pins dependency
versions centrally. Every member crate inherits from `[workspace.deps]`
rather than declaring its own versions. This avoids version drift, which
is the most common source of pain in multi-crate Rust projects.

Key dependencies to pin from day one: `logos` for lexing, `chumsky` or
hand-written recursive descent for parsing (recommend hand-written for
better error messages), `salsa` for the query engine, `inkwell` for
LLVM, `cranelift` for the debug backend, `bumpalo` and `typed-arena`
for AST allocation, `indexmap`, `rustc-hash`, `smallvec`, `tracing`,
`thiserror`, `miette` for diagnostics, `insta` for snapshot tests.

### 3.4 Build Commands

Define common commands in a `justfile`:

    default: build

    build:
        cargo build --workspace

    test:
        cargo nextest run --workspace

    fmt:
        cargo fmt --all

    lint:
        cargo clippy --workspace --all-targets -- -D warnings

    snc *args:
        cargo run --bin snc -- {{args}}

    check-all: fmt lint test

    bless:
        INSTA_UPDATE=always cargo nextest run --workspace

---

## 4. Phase A — The Broker Prototype

**Goal**: build the memory broker as a standalone Rust crate, ship it,
get real users, learn whether the API actually proves out.

**Duration**: three to six months.

**Go/no-go criterion**: at least three real Rust projects (yours or
external) have adopted the broker for non-trivial work and the API has
stabilized through their feedback. If after six months no one wants to
use it, the broker idea is wrong or the API is wrong, and Sentinel
should pause.

### 4.1 What to Build

The broker crate exposes a `Broker` struct that owns allocation policy
for a process. It provides:

  - Generational arenas with O(1) bulk free and safe dangling-handle
    detection. Handles are `(arena_id, slot_index, generation)` triples.
    Access through a handle checks the generation atomically.
  - Programmable allocation strategies as trait objects. The default
    strategies are bump, slab, and system-malloc, but users can plug in
    their own.
  - Memory budgets with structured-scope semantics. The Rust API uses a
    builder pattern since Rust does not have effect handlers.
  - Statistics queries (live bytes, peak, fragmentation, allocation
    counts by tag).
  - A recording mode that captures every allocation event into a ring
    buffer for deterministic replay.
  - Secret-memory policy with mlock, no-core-dump exclusion, and
    zero-on-free. On macOS this uses `mlock(2)`, `madvise(MADV_NOCORE)`
    where available, and explicit zero with a barrier on free.

What to *defer*: cross-process shared memory (hard, do it in Phase C
when you have the full language to express it). Memory-hard secret
storage (research, post-1.0). Argon2id integration (use the `argon2`
crate as a separate library, not part of the broker yet).

### 4.2 API Sketch

    use sentinel_broker::{Broker, Arena, Budget, Handle, Secret};

    let broker = Broker::new();
    let arena = broker.create_arena("request", 4 * 1024 * 1024);
    let handle: Handle<Request> = arena.alloc(Request::new());

    let req: &Request = handle.get()?; // returns Err if invalidated
    arena.drop(); // all handles into this arena now return Err

    let budget = Budget::new(8 * 1024 * 1024);
    budget.scope(|alloc| {
        let v: Vec<u8, _> = Vec::with_capacity_in(1000, alloc);
        // ...
    }).map_err(|over| /* graceful fallback */)?;

    let key: Secret<[u8; 32]> = broker.alloc_secret([0u8; 32]);
    // key is mlock'd, zeroed on drop, excluded from Debug output

### 4.3 Validation

Write three example programs that use the broker for real work:

  - A small HTTP server with per-request arenas.
  - A parser combinator library that uses bump allocation for AST nodes.
  - A key-value store with the budget API enforcing memory limits.

If writing these is awkward, the API is wrong. Iterate.

Publish the crate to crates.io as `sentinel-broker` once the API feels
stable. Watch what users do with it. The point is to discover whether
the broker concept survives contact with real code.

---

## 5. Phase B — The Effects Prototype

**Goal**: build a small interpreted language with algebraic effects and
the `secret` qualifier, just enough to learn whether the effects-as-
capabilities story works in practice.

**Duration**: six to nine months, can start at month three of Phase A.

**Go/no-go criterion**: you can write a small program that demonstrates
supply-chain capability enforcement, async-as-effect, and constant-time
operations on `secret` data, and the ergonomics feel reasonable.

### 5.1 What to Build

A tree-walking interpreter for a tiny language — call it Sentinel-Mini.
No classes, no regions, no broker integration. The point is to validate
the effect system in isolation.

Required features:

  - Hindley-Milner-style type inference with effect rows.
  - Effect declarations: `pure`, `io`, `network`, `throw`, `await`.
  - Effect handlers with `handle expr with { ... }` syntax.
  - The `secret T` qualifier with constant-time equality and a
    "no branching on secret" check.
  - A capability check: importing a module restricts its effects.

What to *defer*: anything not directly testing the effect system.
Performance is irrelevant; this is a research artifact.

### 5.2 Validation

Write three example programs:

  - A "supply chain attack" demo where importing a JSON parser fails
    because it declares the `network` effect.
  - An async demo where the same function runs synchronously in tests
    and asynchronously in production by swapping the effect handler.
  - A constant-time password verification demo that fails to compile
    if you try to branch on the comparison result.

Publish a short paper or technical report at the end of Phase B
documenting what worked and what did not. This is genuinely useful
output even if Sentinel never proceeds.

---

## 6. Phase C — The Bootstrap Compiler

**Goal**: build the production Sentinel compiler in Rust, targeting the
1.0 subset defined in SENTINEL_DESIGN2.md Section 15.

**Duration**: twelve to eighteen months.

**Go/no-go criterion**: the compiler can compile a non-trivial Sentinel
program (target: a TLS handshake implementation, or an HTTP server)
that exercises all 1.0 features.

### 6.1 Architecture

The compiler is a query-based pipeline built on Salsa. Every phase is
expressed as a memoized query over inputs; incremental recompilation
is foundational, not retrofitted.

The pipeline:

    source file
      -> [sentinel-syntax]    lexer + parser -> CST
      -> [sentinel-ast]       CST -> AST lowering
      -> [sentinel-resolve]   name resolution, module graph
      -> [sentinel-types]     type, region, nullability, secrecy,
                              effect inference and checking
      -> [sentinel-hir]       typed HIR with all qualifiers resolved
      -> [sentinel-mir]       SSA lowering, escape analysis,
                              bounds-check elision, constant-time
                              verification on secret data
      -> [sentinel-codegen]   LLVM IR via inkwell, or Cranelift for
                              fast debug builds

The driver crate (`snc`) wires the queries together and exposes the
command-line interface. The LSP crate (`sentinel-lsp`) reuses the
exact same query engine, which is the entire point of using Salsa.

### 6.2 Implementation Order Within Phase C

Build the pipeline end-to-end for the smallest possible language
subset first, then expand. This is the rustc approach and it works.

**C0 (month 1-3)**: lexer, parser, AST for a subset with only `let`,
arithmetic, `if`, and function calls. End-to-end compilation to LLVM
that produces a runnable binary. No type system yet; everything is
i64. The goal is to prove the pipeline plumbing works.

**C1 (month 3-6)**: bring up the type system. Add `struct`, basic
generics, and references. Implement non-nullable types and the `?T`
optional. Bounds-checked array access. At the end of C1 the compiler
should reject all the "obvious" memory-safety violations.

**C2 (month 6-9)**: regions and ownership. Named regions, second-class
references by default, move semantics, `&` and `&mut` borrows. This is
the hardest single phase; budget pessimistically. Use the Polonius
formulation of borrow checking; it generalizes more cleanly than the
NLL formulation when you add regions.

**C3 (month 9-12)**: effects. Integrate the lessons from Phase B.
Effect inference, effect handlers, async-as-effect, capability
enforcement at the module boundary. Add the `secret` qualifier with
constant-time operations and the speculation-barrier insertion in
codegen.

**C4 (month 12-15)**: classes, traits with named implementations,
delegation, structured concurrency, actors. Most of this is
"reasonable language design plumbing" rather than novel work, but the
volume is significant.

**C5 (month 15-18)**: broker integration, cross-process safety,
reproducible-build guarantees, stable ABI definition, LSP and tooling
polish.

### 6.3 Diagnostics

Diagnostic quality is not optional. The borrow checker, region
checker, and effect checker will produce confusing errors by default,
and Sentinel's whole pitch depends on these errors being
comprehensible. Use `miette` for rich diagnostics from day one.
Allocate at least 15% of compiler engineering time to error message
quality. Steal Elm's and Rust's diagnostic conventions shamelessly.

Every error should answer three questions: what is wrong, why is it
wrong, and what should I do about it. Test diagnostics with snapshot
tests so regressions are visible in PRs.

### 6.4 Testing Strategy

Three layers:

  - **Unit tests** in each crate for individual functions and types.
    Standard Rust practice.
  - **UI tests** in `tests/ui/`. Each is a Sentinel program plus an
    expected stderr. Modeled on rustc's UI test suite. These catch
    regressions in diagnostics and in what the compiler accepts or
    rejects.
  - **Execution tests** in `tests/pass/`. Each is a Sentinel program
    plus expected stdout. The test runner compiles and runs the
    program and compares output.

Use `cargo-insta` for snapshot management. Every PR runs the full
suite via `cargo nextest`. CI fails on any unblessed snapshot
difference.

### 6.5 Performance Targets

Compile time is part of the value proposition. Set targets early and
measure continuously:

  - Clean build of a 10K-line program: under 30 seconds.
  - Incremental build after a one-line change: under 1 second.
  - LSP "go to definition" latency: under 50ms p95.

These are aspirational but they shape architecture decisions. If you
hit a fork in the road, take the path that preserves these targets.

---

## 7. Phase D — Self-Hosting

**Goal**: rewrite the compiler in Sentinel, reach the four-stage
fixed point described in SENTINEL_DESIGN.md.

**Duration**: nine to twelve months after Phase C completes.

**Go/no-go criterion**: stage-three compiler compiles its own source
to a binary that, fed its own source, produces a byte-identical
binary.

### 7.1 Staging

Follow the four-stage plan from the design document exactly. Do not
attempt to self-host all at once; the half-and-half configurations
(Sentinel parser feeding Rust type checker, etc.) are what surface
language ergonomics problems.

Stage one is the easiest and most informative: port the lexer and
parser. If writing the parser in Sentinel is unpleasant, the
language is wrong, and you find out cheaply.

### 7.2 Keep the Rust Bootstrap Alive

Do not delete the Rust bootstrap when self-hosting succeeds. Maintain
it indefinitely as a reproducibility anchor and a defense against
trusting-trust attacks. Pin which Sentinel version the self-hosted
compiler is written in, separately from which Sentinel version it
implements. Every Sentinel release should be buildable from the Rust
bootstrap.

---

## 8. Open Questions to Resolve Early

These are listed in design document Section 18 but they need
*decisions* before Phase C, not just acknowledgment.

**Effects with traits**: can trait methods declare effects
polymorphically? If yes, design the row-polymorphism story now. If no,
document the workaround. This decision shapes the entire type system
and must be made before C3.

**Region inference vs explicit regions**: the design says "named,
visible regions" but practical ergonomics likely require some
inference. Decide where the line is before C2. Recommend: regions are
inferable within a function body but must be explicit at function
boundaries when more than one region is involved.

**Async runtime**: even with effects-as-async, you need a default
scheduler. Will Sentinel ship its own, or wrap an existing one (Tokio
via FFI)? Decide before C3. Recommend: ship a minimal scheduler in
the standard library, allow user-defined schedulers via effect
handlers.

**Stable ABI scope**: a stable ABI for the whole language is
extremely ambitious. Restrict to `extern "sentinel-stable"`
declarations explicitly, like Swift did with `@frozen`. Decide the
exact subset before C5.

**Generic dispatch default**: witness tables vs monomorphization.
The design says witness tables by default, but measure both on
realistic code before committing. Decide before C1.

Document each decision in `docs/decisions/NNNN-title.md` using the
Architecture Decision Record format. Future contributors will need
the reasoning, not just the outcome.

---

## 9. Team and Process

### 9.1 Minimum Viable Team

A realistic minimum is two senior engineers with compiler experience
plus one engineer doing tooling, build infrastructure, and developer
experience. A single person can do Phase A and start Phase B but
cannot realistically complete Phase C alone in any reasonable
timeframe.

If you only have one person, do Phase A, do a reduced Phase B, and
write a thorough postmortem. That alone is a significant contribution.

### 9.2 Process

Use a monorepo. Use trunk-based development with short-lived feature
branches. Require PRs to pass `just check-all` before merge. Require
ADRs for any decision that touches language semantics. Hold a weekly
design review focused on the open questions in Section 8.

Do not chase contributors aggressively in the first year. A small
focused team makes faster progress than a large unfocused one, and
language projects are particularly vulnerable to bikeshedding when
contributors arrive before the core design is stable.

### 9.3 Communication

Maintain a public design log as `mdbook` in `docs/`. Every significant
decision lands as a chapter. Publish quarterly progress reports. This
discipline forces clarity on what you actually built versus what you
planned, and it builds the credibility needed when you eventually
want adopters.

---

## 10. Day One Checklist

When you sit down to actually start:

  - Clone or create the `sentinel` repository with the layout in
    Section 3.2.
  - Install the toolchain from Section 3.1.
  - Copy SENTINEL_DESIGN.md, SENTINEL_DESIGN2.md, and HANDOVER.md
    into `docs/`.
  - Initialize the workspace with empty crates matching Section 3.2.
  - Set up CI to run `just check-all` on every PR.
  - Create `docs/decisions/0001-staged-validation.md` recording the
    decision to follow the Phase A through D plan.
  - Start Phase A milestone one: scaffold `sentinel-broker` with
    the `Broker::new()` constructor, the simplest possible arena, and
    a test that allocates and frees a value.

Ship something on day one. The hardest part of starting a multi-year
project is starting; the rest is iteration.

---

## 11. What to Do When Stuck

You will get stuck. Specific places it tends to happen:

  - **Borrow checker design** in C2. Read the Polonius papers, read
    the rustc dev guide chapter on NLL, look at how Hylo handles
    second-class references. Allocate four weeks of design time
    before writing code.
  - **Effect inference** in C3. Read the Koka and Effekt papers. The
    row polymorphism formulation is the standard one; implement it
    even though it is harder than the alternatives, because the
    alternatives do not compose.
  - **LLVM integration** anywhere. `inkwell` papers over most of the
    pain but not all. When in doubt, write the LLVM IR by hand first,
    confirm it does what you want, then figure out how to generate it
    from `inkwell`.
  - **Diagnostics that confuse users**. Find five people unfamiliar
    with Sentinel, show them the error, ask them to explain it. Their
    confusion is more informative than any internal review.

The general rule: when stuck for more than three days, write the
problem down as a design document, share it for review, and timebox
the resolution. Languages die from indecision more often than from
bad decisions.

---

## 12. Closing

Sentinel is an ambitious project, and the honest assessment in
SENTINEL_DESIGN2.md applies: most language projects at this level of
ambition do not reach widespread adoption. That is not a reason not to
build it. The ideas — programmable runtime, regions, effects-as-
capabilities, the `secret` qualifier — are worth exploring even if the
full language never ships at scale. Each phase produces value
independently. Each phase has a clear go/no-go decision. Each phase
teaches you something the next phase needs.

Build Phase A. See what happens. Decide from there.

Good luck.

*End of document.*
