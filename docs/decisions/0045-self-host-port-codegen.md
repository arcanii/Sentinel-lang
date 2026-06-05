# ADR 0045: Phase D self-host port — (8/N) codegen + the bootstrap fixed-point

Status: **PROPOSED** — the eighth and **final** sub-phase of the self-host port (ADR 0031
D5 / ADR 0038 D9), after the lexer (1/N), parser (2/N, ADR 0039), resolve (3/N, ADR 0040),
types (4/N, ADR 0041), effect-check (5/N, ADR 0042), borrow-check (6/N, ADR 0043), and MIR
+ const-time (7/N, ADR 0044) — all ACCEPTED. Flips to ACCEPTED(-WITH-AMENDMENTS) as the
slices land, recording deviations as numbered amendments (the 0039–0044 cadence). Ports
**`compile_to_object`** (`crates/sentinel-codegen`, 8263 lines — the TypedProgram + DropPlan
→ LLVM IR → object transform) to Sentinel, emitting **textual LLVM IR (`.ll`)** rather than
calling inkwell (Sentinel has no LLVM/inkwell FFI). This is the **bootstrap-critical
transform** and the stage that closes the loop: the **bootstrap fixed-point** — the Sentinel
compiler compiling itself — which is *why* C5 shipped `abi-v1` + reproducible builds (ADR
0029). With it, the whole pipeline — lexer → … → borrow-check → MIR/const-time → **codegen**
— is ported, and Phase D self-hosting is achieved.

## What the scout found — the shape of the problem (read this first)

The handover flagged codegen as a "genuine design call" because Sentinel has no LLVM FFI.
The back-half scout + verification + a hand-written `.ll` probe (this session) settle the
shape — and three findings make the stage **smaller and more tractable than its 8263 lines
suggest**:

- **No phi nodes — codegen uses `alloca` + `load`/`store`, merging branches through memory
  cells.** It emits at `OptimizationLevel::None` (codegen lib.rs:1020) and relies on mem2reg
  *only as a later optimisation* (off at `-O0`), so the unoptimised object is correct as-is.
  **Consequence:** the textual `.ll` needs value-numbering only for instruction-result
  temporaries, **not** for SSA merge points — there are none. Codegen is *simpler* in the SSA
  dimension than the MIR just built in (7/N): the `var_defs` snapshot / VarId-sorted
  merge-param muscle (ADR 0044 D4/D5, the central new muscle there) is **not needed** here.
  (Probe-confirmed — see "the probe" below.)

- **Secret codegen is a no-op.** `secret T` lowers identically to `T` (codegen lib.rs:1594–
  1601, explicit: "Constant-time codegen … is deferred to a follow-on ADR per ADR 0019 D12").
  `WidenToSecret`/`Declassify` are identity lowerings. The constant-time *guarantee* is the
  source-level rejections (ADR 0019: `SecretBranch`/`SecretIndex`/`SecretDivisor`/
  `SecretInRefDeref`) **plus the D5 verifier already ported in 7/N** (`ctverify.sentinel`).
  So there is **no constant-time-emission machinery to port** — secrets need only distinct
  *mangling* for mono keys (a Bar-B concern; the selfhost sources use no secrets at all).

- **The bootstrap subset is small.** The selfhost compiler sources (lexer/parser/resolve/
  types/effects/borrow/mir/ctverify, ~12.7k lines) declare **424 fns, 3 structs, 14 enums —
  and 0 traits, 0 impls, 0 classes, 0 effects, 0 generic user fns** (verified), and never
  *use* `handle`/`perform`/`spawn`/`scope concurrent`/`.await`/`declassify`/`secret`/`?T`
  (nullable). So the **fixed-point needs only the non-exotic core**. The ~1500-line
  handler/kont machinery, the ~765-line handler shape-detection, and the task/scope/concurrency
  + class/trait/impl + generic-mono + nullable lowering are needed for **full-corpus parity**
  but are **NOT bootstrap-critical**. (This is the basis of the two-bar scope, D7.)

The Rust `compile_to_object` is a **3-pass** walk (confirmed):
- **Pass 0 (lib.rs:213–316)** — declare LLVM struct/class **type** decls (opaque first, then
  set bodies, for forward/recursive refs).
- **Pass 1 (lib.rs:318–761)** — declare every **function** + the **runtime ABI symbols** (the
  `sentinel_*` externs).
- **Pass 2 (lib.rs:763–1001)** — emit **function bodies** (`compile_fn` → `lower_block` →
  `lower_expr`; the giant `lower_expr` dispatch is lib.rs:5278–5882).
It reads `hir.program()` (TypedProgram) + `hir.drop_plan()` (DropPlan) (lib.rs:168–170) and
ends by `target_machine.write_to_file(&module, FileType::Object, …)` (lib.rs:1027) — **there
is no textual-`.ll` path in `snc` today** (no `print_to_string`; verified). The object is
then linked: `cc <object> libsentinel_runtime.a -o <exe>` (driver `link`, main.rs:765).

The runtime ABI surface is **~22 `sentinel_*` symbols** (alloc/free/realloc/panic_oob;
str_eq; read_file/write_file/print_bytes; arena_enter/alloc/exit; perform_op/kont_resume/
kont_pure/kont_consume_pure/kont_push; task_spawn/await; scope_enter/register/exit; print).
The **bootstrap subset uses only**: alloc/free/realloc/panic_oob, str_eq, read_file/
write_file/print_bytes, and the u8↔i64 + Vec builtins (len/push/pop/vec_to_array, inlined or
runtime-backed). The handler/task/scope/arena symbols are Bar-B.

**The probe (`.ll` emission validated, evidence-grounded).** A hand-written `.ll` in the
*exact* codegen style — `alloca` + `load`/`store`, a branch whose two arms `store` into a
shared memory cell and a merge block that `load`s it (**no phi**), calling a `sentinel_*`
runtime symbol — compiles **and runs correctly at `-O0`** via BOTH `clang -O0 probe.ll
libsentinel_runtime.a -o probe` (the one-step path) AND `llc -O0 -filetype=obj` → `cc`
(snc's two-step style); both print the runtime call's output and exit with the expected code.
Toolchain present: brew LLVM 18.1.8 (`clang`/`llc`/`opt`), default target
`arm64-apple-darwin`. **So the emission target is viable, not assumed.** (One nit observed:
clang warns on a module triple mismatch — the port hardcodes a canonical triple for
determinism rather than relying on host inference; D2.)

## Owner-chosen decisions (this session)

Settled with the owner (as 5/N–7/N were), recommendations grounded in the scout + probe:

1. **Emission target = textual LLVM IR (`.ll`)** — via `write_file` → external `clang`/`llc`
   → object → link `libsentinel_runtime.a`. (Over emit-C / emit-asm — D2; a C backend for
   legacy-systems portability is BACKLOGGED + research-gated, BACKLOG.md §9.4, not rejected.)
2. **Oracle = canonical `.ll` byte-parity + behavioural run-parity** — a NEW Rust `snc llvm`
   (`llvm_dump.rs`) emits a canonical `.ll` to a spec **we** define (syntax-directed over the
   TypedProgram + DropPlan, reusing the existing mangling/type-mapping — NOT inkwell's
   `print_to_string`); the Sentinel stage reproduces it byte-for-byte over the corpus (the
   port's signature differential), AND every emitted `.ll` is `clang`-compiled + run to assert
   behavioural parity vs the fixture's expected exit/stdout; capstone = the bootstrap
   fixed-point. (Over behavioural-only / inkwell-parity — D3.)
3. **Completion bar = fixed-point first, then full corpus** — port the non-exotic core the
   selfhost sources use and **reach the bootstrap fixed-point** (Bar A, the headline); then
   extend to effects/handlers/concurrency/classes/generics/nullable for full 123-fixture
   parity (Bar B). (D7.)

## Amendments

- **A1 — (8a-i) the `snc llvm` ORACLE + the canonical `.ll` spec LANDED + behaviourally
  validated; the D4 reuse settled (fused `mode 4`, grounded in the mode-2 precedent).**
  - **(8a-i) the oracle** (`1931496`) — `run_llvm` + `crates/sentinel-driver/src/llvm_dump.rs`
    (mirroring `mir_dump.rs`): parse → resolve → check → emit canonical `.ll`. Partial-by-Err
    (a not-yet-ported construct → `Err` → `run_llvm` exits nonzero → the corpus differential
    skips it, exactly as it skips upstream rejects; the subset grows per sub-slice). Validated
    in `tests/llvm.rs` across **three layers (D3)**: (1) **goldens** pin the spec (const+trunc,
    params+call, cmp+unary+bool); (2) a **0-panics corpus sweep** — `snc llvm` over all 141
    fixtures never crashes (**16 emit, 125 cleanly Err**); (3) **behavioural parity** — every
    emitted `.ll`, compiled by `cc` + run, behaves identically (exit + stdout) to `snc build`
    (inkwell) — **16/16** over the straight-line subset (so the textual backend is *correct*,
    not just matching). Probe-grounded: a hand-written `.ll` in this exact style compiles +
    runs exit-correct at `-O0` via `cc`/`clang`/`llc`-18.
  - **THE AS-BUILT canonical `.ll` spec (8a straight-line; supersedes the D2 sketch where they
    differ):** module preamble `target triple = "arm64-apple-darwin"` (hardcoded byte-target).
    One `define` per top-level user fn in **FnId order**; `main` → `i32` (the C-ABI entry, its
    `i64` body `trunc`'d), others → their declared type. **No phi** (the scout finding): every
    param + `let` is an `alloca` slot (`%vN`, a per-fn counter), reads `load`, writes `store`;
    instruction temporaries share the `%vN` counter; params arrive as `%argN`. Ops:
    `add`/`sub`/`mul`/`sdiv`/`udiv` (`u8`→unsigned, ADR 0033) / `and`/`or`/`xor`; `icmp
    <pred>` (signed `slt..` / unsigned `ult..`, result `i1`); `sub <ty> 0, x` (neg); `xor i1
    x, 1` (not); `call <ret> @<name>(<ty> <arg>, …)`; `zext i8..i64` / `trunc i64..i8` (the u8
    width builtins). Operands are literals (`42`, `0`/`1`) or `%vN`. Every value is explicitly
    named (params `%argN`, the rest `%vN`) so there is **no implicit LLVM numbering** to match
    — the byte-determinism the differential needs.
  - **D4 SETTLED → fused `mode 4`** (grounded by reading `types.sentinel`'s `mode 2`): codegen
    emission rides the **6/N `types::run`-with-`mode` template** as a new **`mode 4`**, fused
    into the pass-2 `dump_texpr` walk exactly as MIR `mode 2` is — guarded by a `cg_on(c)` (=
    `mode == 4`) so modes 0–3 stay byte-identical *by construction* (the 0044 D3 discipline).
    The 1:1 mapping from the proven mode-2 machinery: `lastval` → an **operand-threading field**
    (`cglast`: a register number, plus an is-literal flag + literal value, since a `.ll` operand
    is either `%vN` or an integer literal); `mvdv`/`mvdl` (var_defs) → a **VarId→slot
    append-only pool** (the resolve scope idiom); a `mirout` buffer → a **`cgout` `Vec<u8>`**;
    plus a per-fn **value counter**. `type_fn` emits the `define` header + entry + param allocas
    (entry) and the return + `}` (teardown), as it opens/dumps the MIR fn under mode 2. The
    **hybrid** (the `.ll`-emit helpers in a separate `codegen.sentinel` module) is **deferred**
    — fuse first (consistent with mode 2/3; the monolith concern is a Bar-B-scale problem, not
    an 8a one). **The probe is LOW-RISK:** mode 2/3 proved the fused-walk-reuse + flat-pool +
    threaded-value pattern at full corpus scale (123/123); straight-line `.ll` is a *subset*
    (no branch-forking — that's 8b). The re-verify-modes-0–3-byte-identical gate stands when
    the mode-4 code lands.
  - ⚠ **Behavioural-test scaling note:** layer-3 rebuilds every emitted fixture twice (inkwell
    + textual) — fine at 16, but it will dominate the suite as the subset grows; **sample or
    cache** it past a few dozen fixtures (revisit at ~8d/8e).
  - **NEXT = (8a-ii):** add `mode 4` straight-line to `types.sentinel` + a thin
    `selfhost/codegen.sentinel` (`use types::run; run(inp, 4, …)`) + the differential
    (`sentinel_codegen_matches_oracle_on_corpus`: byte-for-byte vs `snc llvm` over the
    straight-line subset + behavioural + leak-free), re-verifying modes 0–3 byte-identical.

## Decision

### D1. Goal.

Port **`compile_to_object`** (`crates/sentinel-codegen`, 8263 lines; the real transform —
ADR 0044's reframing) to Sentinel as **`selfhost/codegen.sentinel`** (the 8th and final
stage), emitting **textual LLVM IR** that an external `clang`/`llc` lowers to an object and
links into a runnable binary behaviourally identical to `snc build`'s. The port reproduces
the Rust backend's **3-pass structure** (D6) over the **TypedProgram + DropPlan** (reached
via the 6/N reuse, D4). HIR is skipped (it is a no-op identity bundle, ADR 0044 reframing);
the inkwell backend **stays** as `snc`'s production path (D9) — the textual path is the new
differential oracle + the thing being ported.

The Rust backend reads:
`compile_to_object(hir: &HirProgram, output: &Path)` → `hir.program()` (TypedProgram:
`fns`/`structs`/`classes`/`traits`/`impls`/`generic_instances`/`refs`/`secrets`/… interner
tables) + `hir.drop_plan()` (DropPlan: scope-exit free sites + `moved_sources`).

### D2. Emission target — textual LLVM IR (`.ll`). [owner-chosen; probe-validated]

`selfhost/codegen.sentinel` builds the `.ll` text in a `Vec<u8>` and `write_file`s it; the
driver-equivalent then runs `clang -O0 <file>.ll libsentinel_runtime.a -o <exe>` (or the
`llc -filetype=obj` → `cc` two-step — both probe-validated). **Why `.ll`** (over the
alternatives):
  - **emit-C** — for the self-host port + the fixed-point, `.ll` wins: emit-C abandons the
    LLVM-IR differential path and the `abi-v1` byte-layout control (struct packing, the
    kont/Vec/array layouts the runtime ABI freezes, ADR 0029 D4) the reproducible-build
    fixed-point rests on, and routing `secret` through C risks the C optimizer reintroducing
    the timing variation the constant-time story forbids. **But a C backend is BACKLOGGED, not
    rejected** (owner, this session) — as a *legacy-systems / portability* target (platforms
    LLVM does not serve; the C-as-portable-assembly route taken by Nim/V/mrustc), a parallel
    emission backend reusing this same TypedProgram + DropPlan walk. **Research-gated:** is it
    valuable / required, which concrete legacy targets demand it, and does `abi-v1` +
    constant-time `secret` survive translation through C + its optimizer? See BACKLOG.md §9.4.
  - **emit-asm** — needs instruction selection + register allocation ported into Sentinel
    (a second backend); unrealistic for this stage.

**`.ll` emission specifics** (settled where the probe + scout make them clear; golden-pinned
details are D3's call):
  - **Canonical, hardcoded target triple** (e.g. `arm64-apple-darwin`) for determinism — not
    host inference (the probe's triple-override warning) — so the fixed-point is reproducible.
  - **No phi, no metadata, no function attributes** (the scout confirmed none are emitted) —
    the surface is: a `target triple`, struct **type defs** (`%Name = type { … }`), runtime
    symbol **declares** (`declare i64 @sentinel_alloc(i64)`), function **defs** with numbered
    temporaries (`%1 = …`) and **explicit types on every operand** (textual IR is not
    type-inferred like inkwell's builder), basic blocks with explicit terminators,
    `alloca`/`load`/`store`/`getelementptr`/`call`/`icmp`/`br`/`switch`/int-arith/bitwise/
    `insertvalue`/`extractvalue`/int-casts/constants.
  - **Opaque pointers** (`ptr`, LLVM 18 default) — the probe used them.

### D3. The oracle — a canonical `.ll` dump (`snc llvm <file>`) + behavioural + fixed-point.

Three layers, in increasing strength (the owner's pick keeps the port's byte-for-byte method
while validating correctness, and sidesteps inkwell's brittleness):

1. **Byte-for-byte canonical-`.ll` differential (the port's signature method).** Add a NEW
   Rust **`run_llvm` + `crates/sentinel-driver/src/llvm_dump.rs`** (mirroring `mir_dump.rs` /
   `borrow_dump.rs`): parse → resolve → check → borrow → **emit a canonical `.ll` text**,
   syntax-directed over the TypedProgram + DropPlan, **reusing the existing `mangle_*` +
   type-mapping helpers** (factored out of codegen as needed). This canonical `.ll` is a spec
   **we own** (deliberately simple + deterministic + clang-valid) — **NOT** inkwell's
   `print_to_string()`. `selfhost/codegen.sentinel` reproduces it **byte-for-byte** over the
   corpus (`sentinel_codegen_matches_oracle_on_corpus`, the phase-go). This is exactly the
   established per-stage pattern: *write the oracle in Rust first (`llvm_dump.rs`), then port
   it to Sentinel* — the dump-in-Rust IS the thing being ported, and a readable textual `.ll`
   backend has independent value.
2. **Behavioural run-parity (correctness — proves the canonical `.ll` is right, not just
   matching).** Every canonical `.ll` (from either side) is `clang`-compiled + linked + run;
   assert its exit code + stdout equal the fixture's expectation (= what `snc build` produces;
   `tests/pass` already pins these). This guarantees the new textual backend is a *correct*
   backend, not merely a parser-pleaser, and closes the gap between the inkwell object and the
   canonical-`.ll` object.
3. **The bootstrap fixed-point (the capstone — D8).** The Sentinel compiler emits its own
   `.ll`, clang builds it, and it reproduces — self-consistency over the whole pipeline.

⚠ **Why not inkwell-parity (the owner-declined option):** matching inkwell's exact textual
output (its value-numbering, `target datalayout` string, type quoting `%"…"`, whitespace) by
hand is brittle. A canonical spec we control is byte-matchable by construction (both Rust and
Sentinel sides emit to the same rule), the way every prior stage's dump was. ⚠ **Two
ground-truths, no conflict:** the *fixtures'* exit/stdout are the **correctness** oracle
(layer 2); the *canonical-`.ll` text* is the **port** oracle (layer 1, Sentinel matches the
Rust `llvm_dump`). The inkwell backend stays `snc`'s production path and is not the oracle.

### D4. How the Sentinel stage obtains the typed program + drop plan (the reuse — probe-gated).

Codegen needs, at every node: its **`Type`** (→ LLVM type + the `secret`-mangling bit), each
`Var`'s **`VarId`** (→ the alloca slot), each `Call`'s **callee** (→ the mangled symbol), the
struct/enum/class **interner tables** (→ layouts + field/variant indices), and the
**DropPlan** (→ scope-exit `sentinel_free` sites). These are *exactly* what `types.sentinel`
computes (and 6/N's `mode 1` already surfaces the move/drop side — DropPlan). So the reuse is
the **6/N template — `types::run(src, mode, result)`** with a new **`mode 4`** that
emits-`.ll` (mode 2 = MIR dump, mode 3 = ctverify, per 7/N). `selfhost/codegen.sentinel`
chains codegen→types→parser (a D.6 chain).

⚠ **THE (8a) REUSE PROBE — fused `mode 4` vs a separate emission module.** Unlike MIR (a
~1134-line side-build that fused cleanly into `types.sentinel` as `mode 2`), codegen is **the
biggest stage** — fusing thousands of `.ll`-emission lines into `types.sentinel` (already
6547 lines) risks an unmanageable monolith.
  - **(a) Fused `mode 4`** (the 7/N precedent): emission lives in `types.sentinel`, guarded by
    `mode == 4`, reusing the pass-2 walk's type/VarId/scope. PRO: zero re-derivation, byte-
    clean by construction for modes 0–3. CON: `types.sentinel` swells to ~10k lines.
  - **(b) Separate `codegen.sentinel` module** that drives its own walk over the typed program
    `types` exposes. PRO: keeps the monolith bounded; the emission code is its own file. CON:
    the back-half finding was that the `TyCtx` can't be cheaply returned by value across a
    module boundary (why 6/N chose `run(src, mode, result)` doing the work *inside* types) —
    so (b) needs either a richer `types` surface (expose the tables + a per-fn walk hook) or a
    partial re-derivation.
  - **Lean: a hybrid** — the *driving walk* is a `mode 4` inside `types.sentinel` (reusing
    type/VarId/scope, byte-clean for modes 0–3 by guarding), but the **`.ll`-text emission
    helpers** (the `emit_*` family building the `Vec<u8>`) live in a `codegen.sentinel` module
    that `types` `use`s — so the bulk of the new code is in its own file while the walk stays
    where the typed context is. The (8a) probe settles this in miniature (emit a one-fn `.ll`
    both ways) and **re-verifies `snc types`/`snc borrow`/`snc mir`/`snc ctverify` stay
    byte-identical** (modes 0–3 — the 0043 A1 / 0044 D3 discipline: this change touches FOUR
    accepted stages) BEFORE the bulk emission.

### D5. The Sentinel data model — the `.ll` text buffer + value-numbering (no SSA merge).

Reproduce the emission in the established **flat `Vec<u8>` text buffer + integer counters**
idiom (the parser/types `out`-buffer pattern):
  - **The `.ll` is built as text** into a `Vec<u8>` (per-fn or whole-module), via `append_str`/
    `append_int`/`append_slice` (the existing helpers) — `write_file`d at the end. Render into
    a **local buffer + `push`-fold into a ctx field**; never pass `&mut (*c).field` to a user
    fn (the ADR 0044 A2 rule).
  - **Value numbering = a per-fn `i64` counter** (`next_val`) bumped on each instruction that
    defines a temporary — because there are **no phi nodes** (D-scout finding), variables are
    `alloca` slots referenced by name/number, so numbering is a simple sequential counter, NOT
    the SSA `var_defs` snapshot/merge machinery 7/N needed. **This is the key simplification
    vs MIR.**
  - **Block labels** = a per-fn counter (`bbN`); branches/loops reference them.
  - **Type rendering** = a `ty_to_ll(handle) -> [u8]` over the types interner (`i64`/`i1`/`i8`/
    `ptr`/`%Struct.N`/`{…}`), reused across passes.
  - ⚠ **PRECISION OBLIGATIONS (the byte-for-byte discipline):** temporary numbers (sequential
    emit order), block-label order, instruction order, struct-field GEP indices, the runtime
    symbol declare order, and `abi-v1` mangled names must match the Rust `llvm_dump` EXACTLY.
    The Sentinel emitter mirrors the Rust dump's emission sequence node-for-node (as every
    prior stage mirrored its oracle).

### D6. The emission structure — the 3 passes (the codegen rehearsal realised).

Mirror `compile_to_object`'s passes, restricted to the Bar-A subset (D7), extended in Bar B:
  - **Pass 0 — type decls.** For each struct: `%Struct.N = type { <field-ll-types> }` (enums:
    the `{ i64 tag, ptr payload }` box-free layout per ADR 0032 D.1; the 14 selfhost enums are
    the compiler's core data model). Opaque-first then bodies for recursion (the Rust order).
  - **Pass 1 — fn + runtime-symbol decls.** `declare`s for the Bar-A `sentinel_*` symbols
    (alloc/free/realloc/panic_oob, str_eq, read_file/write_file/print_bytes) + a `define` head
    per user fn with `abi-v1`-mangled name + explicit param/return types.
  - **Pass 2 — bodies.** `lower_block` (stmts for effect + scope-exit drops from the DropPlan;
    tail for value) + `lower_expr`:
    - scalars (`ConstInt`/`ConstBool`/char/`u8`); `Var` → `load` its alloca; `let`/assign →
      `alloca` + `store`; operators (int arith / `udiv` for `u8` / bitwise / `icmp` / unary);
      short-circuit `&&`/`||` (branch + memory merge); `Deref`/`Index`/field → `getelementptr`
      + `load` (Index with the `sentinel_panic_oob` bounds check);
    - **control flow:** `if`/`else` → `br` + a memory-cell merge (no phi); **`while` + `break`/
      `continue` → real loop CFG with a back-edge** (⚠ unlike 7/N's MIR, which lowered a
      `while` body once — codegen emits the genuine loop: cond/body/after blocks + a back-edge
      `br`, the loop-target stack for break/continue, the ADR 0036 entry-block alloca hoist);
    - **aggregates:** struct-lit (`alloca` + field `store`s, or `insertvalue`), array-lit,
      enum-construct (tag + heap payload via `sentinel_alloc`), **`match`** (`switch` on the
      tag + per-arm payload `load` + the arm bodies);
    - **`Vec<i64>`/`Vec<u8>`** (vec_new/push/pop/len/vec_to_array — the `{len,cap,ptr}` layout,
      `sentinel_realloc` growth, ADR 0034) + str-lit/`str_eq`/u8↔i64;
    - **calls** (direct `call` to the mangled symbol; recursion; the merged multi-module
      program's already-qualified names — D8);
    - **drops** (the DropPlan: `sentinel_free` for un-moved heap bindings at scope exit; the
      box-free recursive-enum drop; Vec/`[u8]` drop).

### D7. Scope — the two bars (owner-chosen: fixed-point first).

**Bar A — the bootstrap subset (reach the fixed-point).** Exactly the features the selfhost
sources use (verified, "What the scout found"): i64/bool/u8, `[u8]`/strings, the 3 structs,
the 14 recursive enums + `match`, arrays + index, `Vec<i64>`/`Vec<u8>`, refs (`&`/`&mut`/`*`),
the operator ladder, `if`/`else`, `while`/`break`/`continue`, `let`/assign, direct + recursive
+ cross-module calls, the Bar-A runtime builtins (D-scout), and DropPlan drops. **This is the
headline** — Bar A + D8 = a self-hosting Sentinel compiler.

**Bar B — full-corpus parity (after the fixed-point).** Extend codegen to the exotic lowering
the corpus exercises but the compiler does not: **generics** (mono instances + generic-struct
mangling), **nullable** (`?T`, `unwrap_or`/`is_some`), **classes/traits/impls/delegates**
(method dispatch, witness tables, init), **effects/handlers** (perform/handle/resume-kont —
the kont ABI + the ~765-line shape-detection + the ~1500-line handler machinery), **structured
concurrency** (spawn/scope/await — the task/scope runtime symbols), **arena routing** (the
non-moved-primitive-array optimisation), `print`/`declassify`. Closing Bar B = `snc llvm`
byte-parity over the full ~123 type-clean corpus → ADR 0045 ACCEPTED.

### D8. The bootstrap fixed-point (the capstone).

The endgame self-hosting demands. Defined in increasing strength:
  - **(i) Per-fixture behavioural parity** (the running differential, D3 layer 2) — each
    fixture's Sentinel-emitted `.ll` → clang → binary behaves identically to `snc build`'s.
    Achievable per Bar-A slice.
  - **(ii) The bootstrap fixed-point** — the Sentinel compiler compiles **itself**. This needs
    a thin **Sentinel-side orchestration** composing the 8 ported stages end-to-end (lex →
    parse → resolve → types → effect → borrow → ctverify → codegen → `.ll` → link). The stages
    already chain via `use`/`run`; the orchestrator is the new glue. ⚠ **Sub-decision (settled
    at the fixed-point slice):** whether self-compilation runs over the **merged** single
    `Program` (reusing the existing `merge_modules` qualification, the simplest path — the
    selfhost sources are multi-file via `use`) or a Sentinel-side module merge; and whether the
    orchestrator is a new `selfhost/snc.sentinel` or an extension of the `types::run` mode
    family. The **reproducibility substrate is in place** (ADR 0029: `abi-v1` frozen +
    deterministic emission, `repro.rs`-guarded) — so the fixed-point is *self-consistency*: the
    Sentinel-emitted `.ll`/object, used to recompile the compiler, reproduces the same output.
    (Byte-identity to `snc`'s *inkwell* object is NOT the bar — a different backend; the bar is
    the Sentinel backend reproducing its **own** output, the meaningful fixed point.)

### D9. Out of scope.

  - **The inkwell backend stays `snc`'s production path** — we ADD a textual path (`run_llvm`
    + `codegen.sentinel`), we do not replace `compile_to_object`. (Two Rust backends that must
    agree *behaviourally*; the fixtures are the arbiter.)
  - **Diagnostic / span parity** (codegen errors — `CodegenError`) — out of scope, as every
    prior stage (D7 of 0040–0044). Codegen lowering is effectively total over type-clean input.
  - **Post-1.0 optimisation** (the textual `.ll` is `-O0`; mem2reg/opts are clang's job if ever
    wanted) — out of scope.
  - **The salsa-tracked codegen query / incremental build** — the textual path is direct.
  - **Constant-time *emission*** (branch-free select, speculation barriers — ADR 0026 D4) —
    already deferred post-1.0 in the Rust (codegen lib.rs:1594); secrets stay strip-to-inner.

### D10. Sub-slicing (the 0040–0044 cadence; merge/split as the build reveals).

**Bar A — reach the fixed-point:**
  - **(8a)** the `snc llvm` oracle (`run_llvm` + `llvm_dump.rs`) skeleton + the canonical `.ll`
    spec (golden-pinned) + the **reuse probe** (fused `mode 4` vs hybrid, D4; re-verify modes
    0–3 byte-identical) + the **behavioural harness** (clang-compile + run + diff) +
    **straight-line** (a fn: const/var/arith/`let`/return → a clang-valid, exit-correct `.ll`).
    The de-risk gate — proves emit→clang→run→diff end-to-end on the simplest program.
  - **(8b)** control flow — `if`/`else` (br + memory merge), `while`/`break`/`continue` (the
    real loop CFG + back-edge + alloca hoist), short-circuit `&&`/`||`.
  - **(8c)** aggregates — structs (lit + field GEP), arrays (lit + index + bounds check),
    `[u8]`/string literals.
  - **(8d)** `Vec<i64>`/`Vec<u8>` (vec_new/push/pop/len/vec_to_array + realloc) + the runtime
    builtins (str_eq, u8↔i64, read_file/write_file/print_bytes) + heap drops (DropPlan).
  - **(8e)** enums + `match` (the `{tag,ptr}` layout, construct, `switch` + payload load) — the
    compiler's core data model.
  - **(8f)** calls + recursion + the merged multi-module program (`abi-v1` mangled names) — the
    full Bar-A subset matches `snc llvm` over the selfhost-shaped corpus subset, behaviourally.
  - **(8g)** **THE BOOTSTRAP FIXED-POINT** (D8(ii)) — the Sentinel-side orchestrator + the
    self-compilation + self-reproduction. **The headline milestone.**

**Bar B — full-corpus parity (each its own slice, merge/split as revealed):**
  - **(8h)** generics (mono + generic-struct mangling) + nullable (`?T`).
  - **(8i)** classes / traits / impls / delegates (dispatch + witness + init).
  - **(8j)** effects / handlers (perform / handle / resume-kont — kont ABI + shape-detection).
  - **(8k)** structured concurrency (spawn / scope / await) + arena routing.
  - **(8l)** the full-corpus phase-go (D11) → **ADR 0045 ACCEPTED**.

### D11. Phase-go.

`sentinel_codegen_matches_oracle_on_corpus` (mirroring the borrow/mir phase-go): build the
Sentinel codegen stage, sweep `tests/pass` + `tests/ui`, skip upstream-rejected fixtures,
assert **byte-equal `snc llvm` dumps** over the in-scope corpus AND **behavioural run-parity**
(clang-compile each `.ll` + run + diff exit/stdout). Bar A green over the selfhost subset +
the fixed-point (8g) is the headline; Bar B green over the full ~123 type-clean corpus flips
ADR 0045 → ACCEPTED, **closing the self-host port**.

## Reasoning

Codegen is the **bootstrap-critical transform** and the finale: porting it is what makes
Sentinel self-hosting (ADR 0031/0038). Three things make it tractable despite its 8263 lines:
(1) **the no-phi alloca/load-store style** means the textual `.ll` is a *syntax-directed text
emission*, not an SSA construction — the hardest muscle of 7/N (`var_defs` snapshot + merge
params) is **not needed**, and the probe proved a hand-written `.ll` in this style
clang-compiles + runs at `-O0`; (2) **secret codegen is a no-op** (the guarantee lives in the
source rejections + the already-ported D5 verifier) — no constant-time-emission machinery; (3)
**the fixed-point needs only the bootstrap subset** (the selfhost sources use no
effects/concurrency/classes/generics/nullable), so Bar A — the headline — is a fraction of the
8263 lines, and the exotic ~2300-line handler/concurrency machinery is sequenced after the
milestone (Bar B), where it is least valuable (runtime-heavy, behaviourally testable, unused by
the compiler). The oracle keeps the port's byte-for-byte differential (a Rust `llvm_dump`
canonical `.ll` the Sentinel side reproduces — the established write-oracle-in-Rust-then-port
pattern) while *adding* behavioural run-parity (so the new textual backend is provably
*correct*, not just matching) and the self-reproduction capstone. The reuse rides the 6/N
`types::run`-with-`mode` foundation (a `mode 4`); the (8a) probe settles fused-vs-hybrid and
guards the four already-accepted modes 0–3.

## Consequences

### Positive
- Achieves **Phase D self-hosting** — the Sentinel compiler compiles itself (the fixed-point).
- The whole `snc` pipeline is ported (lexer → … → codegen), each differentially validated.
- A NEW textual-`.ll` backend (independent value: readable IR, a second oracle on the inkwell
  backend, a Phase-D asset) — and behavioural run-parity proves it *correct*, not just matching.
- Fixed-point-first delivers the headline early; Bar B is clean, decoupled completeness.
- The no-phi + secret-no-op findings make the finale far smaller than 8263 lines suggests.

### Negative
- ⚠ **The biggest stage** — even Bar A spans many slices; Bar B's handler/concurrency/class
  lowering (~2300 lines) is the hairiest code in the compiler. Honestly the longest stage.
- ⚠ **The oracle costs a second Rust backend** (`llvm_dump.rs`) — the canonical `.ll` emitter —
  on top of the Sentinel port (mitigated: it IS the thing being ported, Rust-first per pattern).
- ⚠ **`abi-v1` byte-exactness** — struct/Vec/enum layouts + mangled names + GEP indices must
  match the runtime ABI and the oracle exactly (the byte-for-byte discipline at object scale).
- ⚠ **`mode 4` touches FOUR accepted stages** (types/borrow/mir/ctverify byte-identity) — the
  (8a) probe must re-verify all four before bulk emission (the 0044 D3 gate, widened).
- The external `clang`/`llc` dependency (LLVM 18) is now load-bearing for the self-host path
  (already true for `snc`'s production link; the toolchain is the working norm).

### Neutral
- Diagnostic/span parity stays deferred (D9), as every prior stage.
- The inkwell backend stays `snc`'s production path (D9) — two backends, fixtures arbitrate.
- Constant-time emission stays deferred post-1.0 (D9) — secrets strip-to-inner, as the Rust.

## Revisit

- **D4 (reuse shape)**: probe fused `mode 4` vs hybrid; re-verify `snc types`/`borrow`/`mir`/
  `ctverify` byte-identical BEFORE bulk emission. Record as A1 (the 0044 A1 pattern).
- **D3 (canonical `.ll` spec + the behavioural harness)**: the oracle's call — golden-pinned,
  as the prior stages' dumps; refine the spec if a corpus shape forces it.
- **D8 (the fixed-point composition)**: the Sentinel-side orchestrator + single-merged vs
  multi-module self-compilation — settle at (8g).
- **D7/D10 (the two bars + slice boundaries)**: merge/split as the build reveals; Bar B may be
  re-scoped (e.g. effects/concurrency declared a deferred follow-on if the corpus subset green
  + the fixed-point are judged the meaningful close).

## Context

ADR 0044's reframing established that codegen is the **real transform** (HIR a no-op, MIR an
analysis side-branch reading the typed program — and codegen reads it directly too). The
genuinely new design question it deferred here — Sentinel has no LLVM/inkwell FFI — is settled
by the owner this session: **emit textual `.ll`** (probe-validated) with a **canonical-`.ll`
byte-parity + behavioural oracle**, **fixed-point first**. This stage keeps the lexer → … →
ctverify differential-oracle method and the 6/N `types::run`-with-`mode` reuse, and closes the
port at the **bootstrap fixed-point** — the reproducible-build substrate (ADR 0029 `abi-v1` +
deterministic emission) is *why* C5 froze the ABI. See ADR 0038 for the port's spine, ADR 0026
for the HIR/MIR + constant-time-codegen design (the D4 escape hatch = why secret codegen is a
no-op), ADR 0029 for `abi-v1` + reproducible builds (the fixed-point substrate), ADR 0032/0033/
0034/0036 for the enum/`[u8]`/`Vec`/loop layouts codegen emits, ADR 0043/0044 for the reuse
template, `docs/agent-protocol.md` for the probe discipline, and the auto-memory
`sentinel_selfhost_port` for the running record.
