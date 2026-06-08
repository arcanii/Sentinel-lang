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

- **A2 — (8a-ii) `selfhost/codegen.sentinel` LANDED; (8a) is COMPLETE.** The EIGHTH and
  final Sentinel stage (`2ed426a`) emits textual `.ll` for the straight-line subset,
  matching `snc llvm` byte-for-byte over the corpus (`sentinel_codegen_matches_oracle_on_corpus`
  + `_on_seeds`, **16/16 emitted**), **leak-free** (`leaks --atExit` 0/0 over the subset),
  with modes 0–3 (`snc lex`/`parse`/`resolve`/`types`/`effects`/`borrow`/`mir`/`ctverify`)
  **byte-identical** (all 8 corpus differentials green). The D4 fused `mode 4` executed as
  A1 designed — a 1:1 mirror of the MIR `mode 2`:
  - **TyCtx gained ~13 cg fields** (reset per-fn in `cg_reset`; `cgout` accumulates all
    fns): `cgnext` (value counter), `cglk`/`cglv` (the threaded operand — kind 0 register
    `%vN` / 1 literal — the analog of `lastval`), `cgsv`/`cgsr` (the VarId→slot append-only
    pool — the analog of `mvdv`/`mvdl`), `cgpty` (param types for the header), `cgmain`,
    `cgak`/`cgav`/`cgat` (the call-arg stacks — the analog of `margs`), `cg_collecting`,
    `cg_suppress` (assign-target place walk), `cg_lastvid`.
  - **`mir_on` narrowed to `mode 2/3`** + a new **`cg_on` = `mode 4`**, so the two
    side-builds never co-fire; every cg emit is `cg_on`-guarded → modes 0–3 dead-code by
    construction (the re-verify gate confirmed it).
  - ⚠⚠ **THE KEY SENTINEL FINDING (reusable):** the A2-rule "never pass `&mut (*c).field`
    to a USER fn" is sidestepped by **direct-to-`cgout` helpers** (`cgo_str`/`cgo_int`/
    `cgo_ty`/`cgo_operand`) that take `&mut TyCtx` and push onto `(*c).cgout` via the
    **builtin `push`** (the one thing allowed to take `&mut (*c).field`), each **consuming
    any `[u8]` arg by value** (so a string literal drops at the helper's exit). This is
    simpler than the MIR teardown's render-to-local-buffer-then-push-fold — no local buffer
    needed, the walk emits text in order. (MIR built pools + rendered at teardown because
    its dump groups out-of-order blocks; straight-line `.ll` is one in-order block, so
    direct emission is clean. 8b's branches may revisit this.)
  - `type_fn` emits the `define` header + param allocas (`cg_emit_fn_header`) before the
    body walk and the return + `}` (`cg_emit_fn_footer`; `main` → `trunc i64 … to i32` +
    `ret i32`, detected by a byte-compare `cg_is_main`); `emit_tparams` reserves each
    param's alloca slot + records its type; `dump_targs` collects each call arg's operand
    (`cg_collect`, gated). **NO phi** — the (7/N) `var_defs`-snapshot/merge muscle is not
    used; alloca/load/store carries everything.
  - **The bind-inner-first rule bit once:** `cgo_ty(c, (*c).cgat[i])` re-borrows `c` while
    indexing a field — bind the field reads to locals first (the 4a-probe gotcha).
  - **NEXT = (8b) control flow** — `if`/`else` (br + a memory-cell merge, NO phi),
    `while`/`break`/`continue` (the real loop CFG + back-edge + the ADR 0036 alloca hoist),
    short-circuit `&&`/`||`. ⚠ The behavioural-test scaling note (A1) still stands.

- **A3 — (8b) CONTROL FLOW COMPLETE** (`c76db27` 8b-1 hoist + if/else; `7b33d49` 8b-2
  while + `&&`/`||`). Codegen now lowers the full non-aggregate control-flow grammar,
  byte-identical to `snc llvm` over the corpus (`sentinel_codegen_matches_oracle_on_corpus`,
  **26/26 emitted**) + 6 control-flow seeds + 2 new goldens (if/else, while-CFG),
  behaviourally correct (`cc`-run == inkwell) and leak-free; modes 0–3 byte-identical.
  - **THE ALLOCA HOIST (8b-1) — the foundational refactor.** Every alloca (params,
    `let`s, if-results) is HOISTED to the entry block. This solves two problems at once:
    (a) the if-result slot's type is known only AFTER its then-branch is walked (the parser
    AST has no precomputed types, unlike the Rust `then_branch.ty`), yet the slot must
    dominate both arm stores + the merge load — so it can't be emitted inline; (b) a
    loop-body `let`'s alloca must not re-run per iteration (ADR 0036). **Oracle:** `Emit`
    gained an `allocas` buffer + a `block` counter; `alloca()` emits there; `dump_fn`
    assembles header + entry + allocas + body. **codegen.sentinel:** the body is buffered
    in a new `cgbody` field; allocas are recorded as `(slot,type)` pairs
    (`cgalloca_sl`/`cgalloca_ty`); a **`cg_putc` router** (`cg_to_body`) sends emission to
    cgbody during the walk and to cgout at teardown; `cg_emit_fn` assembles header + hoisted
    allocas + folded body + ret. The (8a) helpers' 29 cgout pushes were routed through
    `cg_putc` (a `perl` replace_all + the new router). ⚠ Named values (`%vN`) may be defined
    out of numeric order across blocks (a hoisted `%v5` alloca in `entry` while `%v3`/`%v4`
    are in a later block) — valid LLVM (names, not implicit numbering); `cc` accepts it.
  - **if/else (8b-1):** a conditional `br` into then/else blocks (labels `bbN` via the
    block counter), each storing its value into the hoisted result slot, then a merge block
    that loads it. The result slot is reserved AFTER the then walk (type `tt` known), so
    both sides number it identically. **NO phi** — the 7/N MIR `var_defs`-merge muscle is
    not used; memory cells carry the merge.
  - **while/break/continue (8b-2):** the loop CFG — `br` to the cond block (the back-edge
    target), cond branches to body/after, the body branches back. `break`→the innermost
    loop's after block, `continue`→its cond block, via a loop-target stack
    (`cg_loop_cond`/`cg_loop_after`, pushed around the body walk). After a break/continue
    `br`, a fresh **dead block** is opened so trailing source-block code has a home (LLVM
    discards it — no live preds). Loop-body allocas are hoisted by the 8b-1 mechanism (no
    per-iteration growth).
  - **`&&`/`||` (8b-2):** short-circuit, no phi — branch on the left BEFORE the right walk
    so the right operand emits INTO the rhs block (the MIR mode-2 fork structure); the rhs
    block stores the right value, the short block stores the constant (false `&&` / true
    `||`), the merge loads the i1. Lives in the Binary arm (bop 14/15), split across a
    pre-rhs fork hook + a post-rhs merge hook (the cg block locals span both). The Rust
    inkwell `lower_logic_and/or` use `phi`; the canonical spec uses the memory-cell form
    (consistent with if; behaviourally identical).
  - **NEXT = (8c) aggregates** — structs (lit + field GEP), arrays (lit + index + bounds
    check), `[u8]`/string literals. Then (8d) Vec + builtins + drops, (8e) enums/match,
    (8f) calls/recursion/multi-module, **(8g) the bootstrap fixed-point**. ⚠ The
    behavioural test rebuilds each emitted fixture twice (~25s at 26) — sample/cache it
    around (8d)/(8e) as the subset grows.

- **A4 — (8c-1) STRUCTS LANDED** (the first aggregate; opens slice (8c)). Codegen lowers
  struct **type declarations**, **literals**, and **field reads**, byte-identical to `snc
  llvm` over the corpus (`sentinel_codegen_matches_oracle_on_corpus`, **32/32 emitted** —
  the five C1.4-shaped pure-struct fixtures now light up) + 4 struct seeds + 1 new golden
  (`llvm_struct_decl_lit_and_field`), behaviourally correct (`cc`-run == inkwell) and
  leak-free; modes 0–3 (types/borrow/mir/ctverify) + effects byte-identical (the 0044 D3
  gate, four stages).
  - **A struct is a first-class SSA VALUE, not memory.** The canonical choice (matching the
    inkwell rvalue path, D6) is **`insertvalue`/`extractvalue`** over an aggregate `%vN`,
    NOT alloca/GEP for the struct itself — so a struct flows as ONE operand and
    `let`/`Var`/param/return/call already carry it through the EXISTING alloca/store/load the
    moment the type renderer learns structs. No GEP, no new memory model; the slice is
    small.
  - **Pass 0 — named type decls.** A new pre-fns pass emits `%Struct.N = type { <field-ll-
    types> }` per struct in StructId order (empty → `type {}`). **Oracle** (`dump`): a loop
    over `program.structs`; `llvm_ty` extended `Type::Struct(id)` → `%Struct.{id}`.
    **codegen.sentinel** (`cg_pass0`, wired into the mode-4 preamble right after the target
    triple): iterates the flat struct/field tables (`sts`/`fldo`/`fldty`) via a
    buffer-targeted `ll_type_to` (the `cgo_ty` mapping written to `result` — Pass 0 writes
    the module preamble, not the cg body buffer); `cgo_ty` gained the struct case through the
    existing `struct_of_handle` (kind-6 handle → StructId).
  - **struct-lit = `insertvalue` chain from `undef`, COLLECT-then-emit.** The oracle was
    switched interleave→**collect** (lower ALL field operands first, then emit the chain) so
    both backends agree even when a field VALUE emits instructions — which lets
    codegen.sentinel **reuse the call-arg machinery verbatim** (`cg_collecting` + a `snap` of
    the shared `cgak`/`cgav`/`cgat` stacks; `dump_sfields` `cg_collect`s each field; a new
    `cg_emit_structlit` folds `[snap..]` into the chain, then pops back). Nested struct lits
    fall out (each gets its own snap). For literal/nested-lit fields (the whole corpus)
    collect and interleave are byte-identical, so the golden is unchanged.
  - **field read = `extractvalue`.** The Field arm captures the target aggregate operand
    after the receiver walk and emits `extractvalue %Struct.N <agg>, <fidx>` (a new
    `cg_extract`); a chained `o.inner.x` nests as the inner result feeds the outer. Guarded
    to plain structs (kind 6) — a class/generic-instance target makes the oracle Err (the
    fixture is skipped, so its bytes are never compared).
  - **GENERIC structs are 8h/Bar B.** The oracle skips them in Pass 0 (`type_params`
    non-empty); no oracle-emitting fixture declares one, so the Sentinel side iterating ALL
    structs matches byte-for-byte on the Bar-A subset (the skip + a `Type::GenericInstance`
    renderer land together at 8h). Field ASSIGNMENT (`p.x = …`) stays deferred (the oracle's
    non-Var-lvalue limit) — the struct fixtures construct + read only.
  - **NEXT = (8c-2) arrays** (lit + index + the `sentinel_panic_oob` bounds check) →
    `[u8]`/string literals (closing 8c) → (8d) Vec + builtins + drops. ⚠ The behavioural
    test is now ~66s (rebuilds each emitted fixture twice; 32 fixtures) — sample/cache at
    (8d)/(8e).

- **A5 — (8c-2) ARRAYS LANDED** (the second aggregate). Codegen lowers array literals,
  indexing, and `len`, byte-identical to `snc llvm` over the corpus
  (`sentinel_codegen_matches_oracle_on_corpus`, **42/42 emitted** — the 10 C1.6/C2.3 array
  fixtures light up) + 3 array seeds + 1 new golden (`llvm_array_lit_index_and_len`),
  behavioural (`cc`-run == inkwell) and leak-free; modes 0–3 + effects byte-identical.
  - **`[T]` is the abi-v1 `{ i64 len, ptr data }`** (ADR 0029 §2) — ONE inline literal struct
    type for every element type (the data is an opaque heap pointer), so it needs NO Pass-0
    name (unlike a struct's `%Struct.N`); `let`/`Var`/param/return/call carry it through the
    EXISTING alloca/store/load the moment the type renderer learns `Type::Array` → `{ i64,
    ptr }`. The element type matters only for the GEP stride (carried by the ArrayLit/Index
    nodes).
  - **array literal = heap alloc + GEP-stores + insertvalue.** `n * sizeof(elem)` via the
    **GEP-sizeof constant idiom** (`getelementptr T, ptr null, i64 n` then `ptrtoint`) —
    correct for ANY element type incl. structs (with padding), without replicating layout
    sizing — then `sentinel_alloc`, a per-element `getelementptr`+`store`, and the `{len,ptr}`
    `insertvalue`. Element operands are COLLECTED first (reusing the call-arg stacks +
    `cg_emit_arraylit`).
  - **`a[i]` = bounds-check + GEP + load.** Extract `len`(0)/`data`(1); `icmp sge i, 0` + `icmp
    slt i, len` + `and`; `br` to ok/oob; OOB = `call void @sentinel_panic_oob(i, len)` +
    `unreachable`; OK = `getelementptr elem, data, i` + `load` (the OK block continues; the
    Index arm captures both operands then `cg_emit_index`, reusing `cg_fresh_block` — blocks
    oob-first/ok-second). `len(arr)` = `extractvalue 0`.
  - **The first runtime-symbol declares.** A program emits `declare ptr @sentinel_alloc(i64)` /
    `declare void @sentinel_panic_oob(i64, i64)` ONLY for the symbols it actually uses (per-
    symbol `used_alloc`/`used_panic` flags set during emission; the oracle buffers the fns so
    the declares precede them, the Sentinel side reads the flags in the mode-4 preamble) — so a
    heap/bounds-free program (all of 8a–8c-1) stays byte-identical. `c16_empty_array` declares
    only `sentinel_alloc` (a `let xs = []` + `len`, no index) — proving the per-symbol split.
  - ⚠ **The `len`-arg collect gap (a debugging find).** `len` (FnId 3) is a GENERIC builtin →
    routes through `dump_gcall`/`dump_args_capture_first`, NOT `dump_targs`, and that path walked
    the first arg WITHOUT `cg_collect`ing it (the same gap `dump_array_elems` had) → an empty
    `cgak` → `cg_emit_call` indexed out of bounds (the compiler SIGABRT'd on the 3 `len`
    fixtures). Fixed by collecting the first arg in both `dump_args_capture_first` and
    `dump_array_elems`. (Also: the flat per-fn namespace bit once — `let d` in the new `len`
    branch clashed with `cg_emit_call`'s existing `d`; renamed.)
  - **NEXT = (8c-3) `[u8]`/string literals** (heap-copied byte arrays — the `lower_string_lit`
    path: an `i8` array of constant byte stores) → closes (8c) → (8d) Vec + builtins + drops.
    ⚠ The behavioural test is now ~71s (42 fixtures rebuilt twice) — sample/cache at (8d).

- **A6 — (8c-3) `[u8]`/STRING LITERALS LANDED — slice (8c) aggregates is COMPLETE.** Codegen
  lowers string literals (and the char-literal cg operand), byte-identical to `snc llvm` over the
  corpus (`sentinel_codegen_matches_oracle_on_corpus`, **43/43 emitted** — `c5d5_break_continue`
  joins, its `len("tok")=3` / `len("word")=4` driving the exit) + 2 seeds + 1 new golden
  (`llvm_string_literal_is_a_u8_array`), behavioural (`cc`-run == inkwell) and leak-free; modes
  0–3 + effects byte-identical.
  - **A string literal IS a `[u8]`** (ADR 0033 D2/D3) — the decoded bytes heap-copied
    (`sentinel_alloc` + N constant `i8` stores) into a fresh `{ i64, ptr }`, EXACTLY a u8 array
    literal of byte constants. So it reuses the array machinery wholesale: the oracle factored the
    array-buffer scaffold (GEP-sizeof + alloc + GEP-stores + `insertvalue {len,ptr}`) into
    `emit_array_buffer`, shared by the ArrayLit arm (lowered element operands) and the StringLit
    arm (constant `i8` byte operands) so the two cannot drift; the Sentinel `Str` arm pushes each
    decoded byte as an `i8` literal operand then calls the existing `cg_emit_arraylit` (reading
    `sb` BEFORE `sink_name` consumes it). `sizeof(u8) = 1` so the GEP-sizeof buffer is exactly N
    bytes — the same idiom, no special case.
  - **Char-literal cg operand (a closed latent gap).** The Sentinel `Char` arm did not set the cg
    operand (no emitting fixture exercised a char before); it now sets `cglk=1`/`cglv=cv` like the
    `Int` arm — a `u8` constant, the same family as a string byte, needed by the string phase-go
    (`c5d2_strings`, which still Errs on `str_eq`/`print_bytes` → 8d). The oracle already handled
    `CharLit` since 8a.
  - **NEXT = (8d) `Vec<i64>`/`Vec<u8>`** (`vec_new`/`push`/`pop`/`len`/`vec_to_array` + the
    `{len,cap,ptr}` layout + `sentinel_realloc`) + the runtime builtins (`str_eq`, `read_file`/
    `write_file`/`print_bytes`) + **heap drops** (the DropPlan — `sentinel_free` at scope exit; the
    first slice where the emitted program's *runtime* leaks matter, vs the compiler's). ⚠⚠ The
    behavioural test is now ~55–71s (43 fixtures rebuilt twice) — **sample/cache it at (8d)** (the
    `snc build` ground-truth + the `cc` compile per fixture is the cost).

- **A7 — (8d, runtime builtins) LANDED** — the byte-array runtime builtins, done FIRST within (8d)
  (ahead of Vec/refs/drops) because they're the simplest + most impactful. Codegen lowers `str_eq`,
  `print_bytes`, `read_file`, `write_file`, byte-identical to `snc llvm` over the corpus
  (`sentinel_codegen_matches_oracle_on_corpus`, **45/45 emitted** — **`c5d2_strings`** (the D.2
  strings phase-go, via `str_eq`) AND **`c5d4_file_io`** (the D.4 file-IO phase-go, via
  `read_file`/`write_file`/`print_bytes` — REAL file I/O) join) + 2 seeds + 1 new golden
  (`llvm_str_eq_runtime_builtin`), behavioural (`cc`-run == inkwell) and leak-free; modes 0–3 +
  effects byte-identical.
  - **Each builtin decomposes its `[u8]` into (ptr, len)** and calls the `sentinel_*` symbol: the
    `{ i64 len, ptr data }` is `extractvalue`'d into `len`(0) + `ptr`(1), passed as the C ABI's
    `(ptr, i64, …)`. `str_eq(a,b)` → `i1`; `print_bytes`/`write_file` → `i64`; `read_file(path)`
    calls with a HOISTED `i64` out-len slot then reassembles the owned `[u8]` `{ out_len, data }`.
    All are non-generic builtins, so they already route through `dump_targs` (args collected) on the
    Sentinel side; the oracle handles them in `lower_call` like `len`/`zext`/`trunc`.
  - **Refactor — `RuntimeSyms`.** The per-symbol declare bools (`used_alloc`/`used_panic`, 8c-2)
    became a `RuntimeSyms` struct (`alloc`/`panic_oob`/`str_eq`/`read_file`/`write_file`/
    `print_bytes`) with `merge` + `emit_declares`, so the fixed-order declare block scales as
    symbols are added (`realloc` joins at the Vec slice). The Sentinel side mirrors it with
    `cg_used_*` ctx flags + the preamble emitting the SAME fixed order — `c5d2_strings` declares
    `alloc`+`panic_oob`+`str_eq`; `c5d4_file_io` declares the file symbols. The order is canonical:
    alloc, panic_oob, str_eq, read_file, write_file, print_bytes.
  - **`cg_lenptr` (Sentinel)** — a helper emitting the two `extractvalue`s for one `[u8]` operand,
    returning the len reg (ptr reg = len+1, the next number). Keeps the 4 builtin arms compact.
  - **NEXT = (8d rest) Vec + heap drops.** Vec (`vec_new`/`push`/`pop`/`len`/`vec_to_array` +
    `{len,cap,ptr}` + `sentinel_realloc`) needs **ref support** first — `push`/`pop` take `&mut Vec`
    (in-place field GEP + a realloc grow CFG), so `&v`/`&mut v`/`*p`/`*p = x` (ADR D7 Bar-A refs)
    come with it. Then **heap drops** (the DropPlan — `sentinel_free` for un-moved heap bindings at
    scope exit; the box-free recursive-enum drop; Vec/`[u8]` drop) — drops are byte-parity-neutral
    for behaviour (exit/stdout unaffected by leaks) but needed for a clean fixed-point. ⚠⚠ The
    behavioural test (~57s at 45) should be **sampled/cached** here.

- **A8 — (8d-refs) REFERENCES LANDED** — `&`/`&mut`/`*`/`*p = x`, the prerequisite for Vec
  (`push`/`pop` take `&mut Vec`). Byte-identical to `snc llvm` over the corpus
  (`sentinel_codegen_matches_oracle_on_corpus`, **53/53 emitted** — the 8 C2 ref fixtures light
  up) + 2 seeds + 1 new golden (`llvm_refs_address_of_and_deref`), behavioural (`cc`-run ==
  inkwell) and leak-free; modes 0–3 + effects byte-identical.
  - **A reference `&T`/`&mut T` is an opaque `ptr`** (LLVM ignores mutability; the pointee type is
    recovered from `program.refs` at the deref). A ref param/let is a `ptr` slot.
  - **The lvalue/rvalue model.** `&v`/`&mut v` → the lvalue pointer: a `Var`'s alloca slot (the
    slot IS the pointer — NO instruction); `&*r` (reborrow) → r's value. `*r` (rvalue) → `load
    <pointee>, ptr <r-value>`. `*r = x` → `store <pointee> x, ptr <r-value>`. ⚠ **Order:** the
    Sentinel `SAssign` walks **target-then-value**, and a deref target DOES emit (load r) — so the
    oracle's `Assign` was restructured to lower the deref-target pointer BEFORE the value (for a
    `Var` target, which emits nothing, value-then-store still matches).
  - **Oracle** (`llvm_dump.rs`): a `lower_lvalue_ptr` helper (`Var`→slot, `Deref`→`lower_expr`
    inner); the `Unary(Ref/RefMut)`→lvalue-ptr / `Unary(Deref)`→load arms.
  - **Sentinel** (mode 4): **reuses the existing `cg_suppress`** (the assign-target machinery) — for
    `&`/`&mut` it suppresses the inner Var's load and reads its slot (`cg_slot_get`); for `*` it
    loads through r, or (as an assign place) leaves r's pointer + signals `cg_lastvid = -1`; `&*r`
    keeps the inner deref-place's pointer (`un_vid = -1` → don't re-slot). `SAssign` stores through
    that pointer (`cg_store` renders the pointer reg as `ptr %v<n>`). No new ctx fields. ⚠ The `&*r`
    reborrow was a 1-byte miss first (`cg_slot_get(-1)` → `%v-1`) — fixed by the `un_vid >= 0` guard.
  - **NEXT = (8d-Vec)** — `vec_new` (a constant `{ i64 0, i64 0, ptr null }`), `push(&mut v, x)`
    (the `len==cap` `sentinel_realloc` GROW CFG, in-place field GEP/store through the `&mut Vec`),
    `pop`/`len`/`vec_to_array`, the `{ i64 len, i64 cap, ptr data }` layout (data = FIELD 2). Then
    **heap drops** (DropPlan). ⚠⚠ behavioural test ~80s at 53 — **sample/cache it now**.

- **A9 — (8d-Vec-1) Vec IN-PLACE OPS LANDED** — `vec_new`/`push`/`pop`/`len`/`v[i]`, the
  growable-collection MVP minus the bridge. A `Vec<T>` is `{ i64 len, i64 cap, ptr data }`
  (ADR 0034; data = FIELD 2, vs `[T]`'s field 1). Byte-identical to `snc llvm` over the corpus
  (`sentinel_codegen_matches_oracle_on_corpus`, **54/54 emitted** — `c5d5_loops` joins) + 1 seed +
  1 new golden (`llvm_vec_new_push_and_len`), behavioural (`cc`-run == inkwell) and leak-free;
  modes 0–3 + effects byte-identical. (`vec_to_array` — the `Vec`→`[T]` memcpy bridge — is 8d-Vec-2;
  `c5d3_collections` needs it.)
  - **vec_new** → the constant `{ i64 0, i64 0, ptr null }` — a NEW operand kind (the Sentinel
    `cgo_operand` gained kind 2); the same value for every element type.
  - **push(&mut v, x)** → load len/cap through the `&mut Vec`'s field GEPs; if `len == cap` grow
    (`sentinel_realloc` to `max(1, cap*2) * sizeof`, via `select` + the GEP-sizeof idiom) and store
    cap/data back; then `data[len] = x`, `len++`. A grow/cont CFG, **no phi**. Returns i64 0. ⚠ The
    oracle lowers BOTH push args before the field GEPs (matching the Sentinel's collect-both-first),
    so a side-effecting push element matches on both backends (the lexer pushes computed bytes).
  - **pop(&mut v)** → empty-check (reuse the OOB trap), decrement len, return `data[len-1]`.
  - **len / `v[i]`** → use the arg's ACTUAL aggregate type: `{i64,ptr}` (array) vs `{i64,i64,ptr}`
    (Vec); `cg_emit_index` gained the target type + keys the data field on `cg_is_vec` (2 vs 1).
  - **Mechanics.** The Vec builtins are GENERIC (route through `dump_gcall` → the first arg is
    collected, as `len` already needed), so `cg_emit_call` reads the collected `&mut`-Vec pointer +
    element operands; the element type is recovered from the `&mut Vec<T>` arg (`strip_ref` +
    `vec_elem_of`). New `sentinel_realloc` declare (`RuntimeSyms` gained `realloc`; canonical order
    alloc/realloc/panic_oob/str_eq/read_file/write_file/print_bytes). **The grow-CFG matched the
    differential on the FIRST try** — the value-numbering discipline holds at this complexity.
  - **NEXT = (8d-Vec-2)** — `vec_to_array(v)` (extract len/data, `sentinel_alloc(len*sizeof)`,
    `llvm.memcpy` the live prefix, build the `[T]`) → `c5d3_collections` (the D.3 phase-go) emits.
    Then **heap drops** (DropPlan). ⚠⚠ behavioural test ~83s at 54 — sample/cache imminent.

- **A10 — (8d-Vec-2) `vec_to_array` LANDED — the `Vec` → `[T]` memcpy bridge:** `vec_to_array(v:
  Vec<T>) -> [T]` copies the live `len` elements of `v` into a fresh heap buffer and builds an
  owned `[T]` `{ i64 len, ptr data }`. Byte-identical to `snc llvm` over the corpus
  (`sentinel_codegen_matches_oracle_on_corpus`, **55/55 emitted** — `c5d3_collections`, the D.3
  Vec phase-go, joins) + 2 seeds + 1 new golden (`llvm_vec_to_array_bridge`), behavioural (`cc`-run
  == inkwell) and leak-free; modes 0–3 + effects byte-identical. Mirrors the production
  `lower_vec_to_array` (codegen lib.rs:5043).
  - **The bridge.** The Vec is passed by VALUE (its aggregate `{ i64, i64, ptr }`), so `extractvalue
    0` = `len`, `extractvalue 2` = the source `data` pointer (field 2, vs `[T]`'s field 1). Size =
    `len * sizeof(T)` via the GEP-sizeof idiom (`getelementptr T, ptr null, i64 %len` + `ptrtoint`
    — correct for any element type, the same idiom `emit_array_buffer` uses); `sentinel_alloc` the
    dest; `call void @llvm.memcpy.p0.p0.i64(ptr %dest, ptr %src, i64 %size, i1 false)` the live
    prefix (align 1 implicit; a 0-length copy on an empty `Vec` is a no-op); then `insertvalue` the
    `[T]` `{ len, dest }`. **NON-consuming** — the array is an independent copy, so `v` keeps its own
    buffer and both drop at scope exit (leak-free once heap drops land).
  - **The intrinsic.** `llvm.memcpy` is a NEW declare — `declare void @llvm.memcpy.p0.p0.i64(ptr,
    ptr, i64, i1)`. As an `@llvm.*` intrinsic (not a `sentinel_*` runtime symbol) it declares LAST,
    after the runtime-symbol group: `RuntimeSyms` (oracle) / the `cg_used_*` chain (Sentinel) both
    gained a `memcpy` flag, set as the call is emitted, so a program not using `vec_to_array` stays
    byte-identical to A9. (Probe-validated first: a hand-written `.ll` in this exact shape compiles +
    runs correct under clang-18.)
  - **Mechanics.** `vec_to_array` is a GENERIC builtin (FnId 10) routing through `dump_gcall` (the
    arg is collected, as `len`/`push` already needed; the typer returns `mk_array(targ)` = `[T]`), so
    `cg_emit_call`'s new `fid == 10` arm reads the collected Vec operand (`cgak/cgav/cgat[snap]`) and
    recovers the element type via `vec_elem_of` (no `strip_ref` — the arg is a bare `Vec`, not `&mut
    Vec`). **The bridge matched the differential on the FIRST try** + leak-free (the arm allocates
    nothing — reads the collect-stacks, emits to `cgout`).
  - **DO-FIRST honoured:** the behavioural test (~83s at 55) was SAMPLED — the targeted inkwell-vs-cc
    check ran on the one fixture whose `.ll` changed (`c5d3` Err→emit, exit 55 both backends) + the 2
    new seeds (Vec<u8>→2, Vec<i64>→40); the other 54 are provably unchanged (the corpus differential
    confirms byte-identical `.ll`). The full behavioural suite was then run ONCE as the final gate.
  - **NEXT = heap drops** (the DropPlan — `sentinel_free` at scope exit; byte-parity-NEUTRAL for
    behaviour, but needed for a clean fixed-point: the textual `.ll` path now allocs `Vec`/array
    buffers it never frees) → **(8e) enums/match** → **(8f) calls/recursion/multi-module** → **(8g)
    THE BOOTSTRAP FIXED-POINT.**

- **A11 — (8d-drops-1) SCOPE-EXIT HEAP DROPS LANDED — array / `Vec` / `[u8]` `sentinel_free`:** at
  each block's exit the un-moved heap-backed bindings are freed in reverse declaration order, so
  the generated programs stop leaking. Byte-identical to `snc llvm` over the corpus
  (`sentinel_codegen_matches_oracle_on_corpus`, still **55/55 emitted**, now WITH drops) + 2 new
  seeds + 5 updated goldens + 1 new golden (`llvm_scope_drops_moved_and_nested`), behavioural
  (`cc`-run == inkwell — drops don't change exit/stdout) and **leak-free under `leaks --atExit`**
  (13 of the 15 heap fixtures; the other two are the next two slices). Modes 0–3 + effects
  byte-identical; 1466 tests, four-check green. Mirrors the production `emit_frame_drops` /
  `emit_drop_for_binding` (codegen lib.rs:3666/3742).
  - **Two key design calls that SIMPLIFY the port** (both validated by the leaks sweep):
    - **Per-binding `sentinel_free`, NO arena routing.** Production routes primitive array literals
      (`[i64]`/`[i32]`/`[bool]`) into a per-scope arena bulk-freed by `sentinel_arena_exit`
      (`compute_arena_routed`, lib.rs:7574) — a perf optimization, NOT a correctness requirement. The
      oracle is OUR canonical spec, and the behavioural test compares *behaviour* not `.ll` text, so a
      plain per-binding free is equally leak-free, far simpler, and byte-parity-clean. Arenas →
      deferred (Bar B). `c54_scope_arena` (nested-block arrays) is leak-free via per-binding frees.
    - **Skip `moved_sources` ALONE — no separate tail-returned guard.** Production skips
      `moved ∪ {tail-returned}`; but the body tail is walked in a CONSUMING context, so a returned
      `Var` is already recorded as a move → `{tail-returned} ⊆ {moved}`. Skipping `moved` alone gives
      the identical drop set with no `&enum`-peek (which Sentinel can't do). The Sentinel's move set
      (`mvf`/`mvv`) was already proven == the oracle's `DropPlan` in the (6/N) borrow slice, so
      `cg_is_moved` ⟺ `moved_sources_for`. Validated: `c24_moved_array_no_double_free` /
      `c23_array_move` (recursive move) → exit-correct, 0 leaks, no double-free.
  - **Two-frame fn structure.** A fn has scope-0 (params) + scope-1 (the body block); body-frame
    drops fire first, then param-frame drops, before `ret` — so a `[u8]` param of a non-consuming
    callee (e.g. `show(b: [u8])`) is freed by the callee, and `reverse(all) = reverse(body) ++
    reverse(params)`. The oracle's `dump_fn` opens both frames explicitly; the Sentinel's
    `type_fn` opens the param frame (mark before `emit_tparams`) and the Block arm opens the body
    frame — frames nest LIFO via the recursive Block walk, so the mark is a local (no stack). ⚠ The
    param-frame drop must fire BEFORE `curfn` is cleared (`cg_is_moved` keys on it).
  - **Plumbing.** Oracle: `run_llvm` runs `sentinel_borrow_check::borrow_check(&typed)` and threads
    the `DropPlan` into `dump`; `Emit` gained `drop_plan` / `current_fn` / `var_ty` / `scopes`.
    Sentinel: a `cgdv`/`cgdt`/`cgds` scope-drop pool (VarId/type/slot) + `cg_drop_record` (at each
    param + `let`), `cg_drop_frame` (reverse-free [mark..] + truncate), `cg_emit_drop` (Array→free
    field 1, Vec→free field 2), `cg_is_moved`. `sentinel_free` is a new declare on both sides
    (`RuntimeSyms.free` / `cg_used_free`; declares right after `realloc`).
  - **REMAINING (the next two slices):** `c16_array_in_struct` needs **(8d-drops-2)** recursive
    struct-field drop (its `.ll` emits no free yet — the `leaks` 0 is a conservative false negative;
    inkwell genuinely frees it). `c5d5_break_continue` needs **(8d-drops-3)** loop-exit drops (the
    per-iteration `[u8]` on the `break`/`continue` paths — 5 leaks today). Then → **(8e) enums/match**.

- **A12 — (8d-drops-2) RECURSIVE STRUCT-FIELD DROP LANDED:** dropping a struct binding now GEPs
  into each heap-backed field (declaration order) and frees it recursively, so a struct with an
  array/`Vec`/struct field no longer leaks. Byte-identical to `snc llvm`
  (`sentinel_codegen_matches_oracle_on_corpus`, **55/55** — `c16_array_in_struct` now emits its
  field free) + 1 new seed + 1 new golden (`llvm_struct_field_recursive_drop`), behavioural
  (`cc`-run == inkwell, exit 12) and **leak-free** (`c16_array_in_struct` 0 leaks — genuinely now, a
  real `sentinel_free`); modes 0–3 + effects byte-identical; 1467 tests, four-check green; scg
  compiler leak-free. Mirrors the production `emit_drop_struct_fields` (codegen lib.rs:3934).
  - **The shape.** `emit_drop_for_binding`'s `ptr_reg` arg is uniformly a `%vN` — an alloca slot for
    a top-level binding, OR a `getelementptr %Struct.K, ptr <ptr_reg>, i32 0, i32 <idx>` field
    register when recursing. A struct arm GEPs each field whose type `needs_drop` and recurses; the
    GEP `idx` is the field's DECLARATION position (a counter over ALL fields), not the count of
    drop-needing fields. `needs_drop`: array / `Vec` → true; struct → true iff some field does
    (recursive, so a struct of only scalars GEPs nothing); the Bar-B shapes → false (don't emit).
  - **Sentinel.** A `cg_needs_drop` predicate (mirrors `needs_drop`) + a struct branch in
    `cg_emit_drop` that scans the flat `fldo`/`fldty` field table for `owner == sid` (declaration
    order, like `cg_pass0`), tracking `fidx` to emit the GEP index. `struct_of_handle` recovers the
    StructId; recursion reuses `cg_emit_drop`. Only `c16_array_in_struct` changed (the only emitting
    fixture with a heap-backed struct field; a struct of scalars like `Point` GEPs nothing). Array
    of struct (`c16_array_of_struct`) stays a plain array drop — per-element recursion is deferred,
    but its `Point` elements own no heap so it's already leak-free.
  - **NEXT = (8d-drops-3)** — loop-exit drops: `c5d5_break_continue`'s per-iteration `[u8]` leaks on
    the `break`/`continue` paths (5 leaks) because branching to `loop_after`/`loop_cond` skips the
    body block's end-of-iteration drop. Drain the loop-body frame(s) before the branch (production
    `emit_loop_exit_drops`, lib.rs:3723). Then → **(8e) enums/match**.

- **A13 — (8d-drops-3) LOOP-EXIT DROPS LANDED — heap drops COMPLETE:** a `break`/`continue` now
  drains the open scope frame(s) from the top down to the loop-body frame BEFORE branching, so a
  per-iteration heap binding is freed on the early-exit path (not just the fall-through). Byte-identical
  to `snc llvm` (`sentinel_codegen_matches_oracle_on_corpus`, **55/55**) + 2 new seeds (break +
  continue) + 1 new golden (`llvm_loop_exit_drops_on_break`), behavioural (`cc`-run == inkwell, exit
  unchanged) and **leak-free**; modes 0–3 + effects byte-identical; 1468 tests, four-check green; scg
  compiler leak-free. **🎯 ALL 15 heap fixtures are now leak-free under `leaks --atExit`** — the
  generated programs match inkwell's leak-freedom; heap drops (drops-1/2/3) are DONE. Mirrors the
  production `emit_loop_exit_drops` (codegen lib.rs:3723).
  - **The mechanism.** Each loop records a `scope_floor` = the scope depth at loop-body entry
    (captured BEFORE the body block pushes its frame). On `break`/`continue`, drain every frame
    `>= scope_floor` (the body + any nested `if`/block frames open at the branch), innermost first,
    WITHOUT popping — the now-dead remainder of each block still pops + re-drops via its own block
    exit (into an unreachable block), so each runtime path frees a given binding exactly once
    (early-exit drain XOR fall-through body-end drop — mutually exclusive blocks). The fn-return
    early-exit shape (ADR 0017), reused for loops.
  - **Oracle.** `loops: Vec<(u32, u32)>` → `Vec<(u32, u32, usize)>` (+ `scope_floor`); the while arm
    captures `self.scopes.len()` at the `loops.push`; `emit_scope_drops` factored into
    `emit_frame_drops(idx)` + `emit_loop_exit_drops(floor)` (drain `[floor..len)` reverse); the
    break/continue arm drains before the branch.
  - **Sentinel.** A `cg_loop_floor` stack (parallel to `cg_loop_cond`/`cg_loop_after`) records
    `len(cgdv)` at loop-body entry; a `cg_drop_range(floor)` (drop-only, NO truncate — `cg_drop_frame`
    minus the pop loop) drains `cgdv[floor..]` reverse; SBreak/SContinue call it before the branch.
    🔑 **Reversing the flat `cgdv[floor..]` == the oracle's top-down per-frame reverse** — the flat
    pool concatenates frames in push order, so reversing the whole range reverses both the frame order
    AND each frame's bindings, exactly matching `for i in (floor..len).rev() { frame[i].rev() }`.
  - **NEXT = (8e) enums/match** → **(8f) calls/recursion/multi-module** → **(8g) THE BOOTSTRAP
    FIXED-POINT.**

- **A14 — (8e-1) ENUM TYPE + ENUM-CONSTRUCT + ENUM DROP LANDED:** an enum lowers to the abi-v1
  `{ i32 tag, ptr payload }` (ADR 0032 D4); construction builds `{ tag, heap-boxed-payload-or-null }`;
  the scope-exit drop null-checks + frees the payload box. Byte-identical to `snc llvm`
  (`sentinel_codegen_matches_oracle_on_corpus`, still **55/55** — `c5d1_enum` needs `match`, the 8e-2
  slice, so the corpus is unchanged) + 2 new seeds + 1 new golden (`llvm_enum_construct_and_drop`),
  behavioural (`cc`-run == inkwell) and leak-free (the enum seed + scg compiler); modes 0–3 + effects
  byte-identical; 1469 tests, four-check green. Mirrors the production `lower_enum_construct` (codegen
  lib.rs:4085) + the enum drop arm (lib.rs:3876) + the `{i32,ptr}` layout (lib.rs:1708).
  - **enum-construct.** A unit variant → a null payload. A payload variant → build the payload struct
    `{ <field tys> }` from the args (`insertvalue` chain from `undef`), heap-box it (GEP-sizeof +
    `sentinel_alloc` + `store`), and point the enum at the box; then `{ i32 tag, ptr payload }` (tag =
    variant index, source order). Args are lowered FIRST (the Sentinel collects them via `dump_cargs`
    before `cg_emit_enum_construct`), so a side-effecting arg lands before the payload chain on both.
  - **enum drop.** `load { i32, ptr }`, `extractvalue 1` (the payload), `icmp eq ptr …, null`, branch
    to a free block (`sentinel_free`) or the after block. **Box-free-only** — a recursive enum's
    nested boxes are NOT walked (the production's measured D.1b limit); `needs_drop(Enum)` is true iff
    some variant carries a payload (a pure-unit enum boxes nothing).
  - **`{ i32, ptr }` is inline** (like `[T]`/`Vec`) — no Pass-0 name; the payload struct type
    `{ … }` is recovered per variant from `enum_data().variants[v].payloads` (oracle) /
    `varpay[varps[j]..]` (Sentinel, via `variant_flat`). The Sentinel: `cgo_ty`/`ll_type_to` learn
    `enum_of_handle ≥ 0 → { i32, ptr }`; a `cg_emit_pstruct` helper emits the payload struct literal;
    `cg_emit_enum_construct` mirrors the oracle; the Qcall enum arm wraps `dump_cargs` in
    `cg_collecting` then emits; `cg_emit_drop`/`cg_needs_drop` gain the enum arm.
  - **NEXT = (8e-2) match** — `switch` on the tag to per-variant blocks (bind payloads via GEP+load
    into slots), a memory-cell result merge (no phi, like `if`), wildcard/`unreachable` default. Lights
    up `c5d1_enum` (and the recursive-enum `selfhost_ast_drop`). Mirror `lower_match` (lib.rs:4144).

- **A15 — (8e-2) MATCH LANDED — (8e) ENUMS COMPLETE:** `match` lowers to an IF-ELSE CHAIN over the
  variant arms (NOT a `switch`), the memory-cell merge, and per-arm payload binding. Byte-identical to
  `snc llvm` (`sentinel_codegen_matches_oracle_on_corpus`, **57/57 emitted** — `c5d1_enum` AND the
  recursive-enum `selfhost_ast_drop` join) + 2 new seeds + 1 new golden (`llvm_match_if_else_chain`),
  behavioural (`cc`-run == inkwell: c5d1→42, selfhost_ast_drop→11) and leak-free; modes 0–3 + effects
  byte-identical; 1470 tests, four-check green; scg compiler leak-free. **The most complex emission in
  the port — byte-identical on the FIRST try** (the value/block-numbering discipline holds at switch
  scale). Behaviourally mirrors the production `lower_match` (codegen lib.rs:4144) + `bind_pattern_payloads`
  (lib.rs:4238).
  - **🔑 IF-ELSE CHAIN, NOT `switch`.** Behaviourally identical for disjoint variants (the variants are
    a partition; the wildcard is the catch-all), but it lowers in a SINGLE PASS — the Sentinel walks the
    arm cons-list CONSUMING, with no slice to pre-scan for a switch's up-front case→block table (a switch
    would force a temp buffer for the arm bodies). The oracle (a slice) matches the same shape. Per arm:
    `icmp eq i32 %tag, <vidx>` → `br` to the arm block (bind + body + `store` result + `br merge`) or the
    next-check block; after the arms `unreachable` (exhaustive — no wildcard in the emitting set; a `_`
    body would be the final else, deferred); the merge `load`s the result (the if-merge memory cell).
  - **Result slot type.** The oracle alloca's `result` BEFORE the arms with `expr.ty`; the Sentinel uses
    `exp` (the match's expectation == its type for a USED match — both emitting fixtures are fn-body
    tails). Tag/payload are `extractvalue 0`/`1` of the scrutinee `{i32,ptr}`.
  - **Payload bind** (`bind_pattern_payloads`): per binding, `getelementptr <pstruct>, ptr %payload, i32
    0, i32 i` + `load` into a fresh HOISTED slot keyed by the binding's VarId (so the arm body's `Var`
    reads it); a `_` binding still gets a slot. NOT drop-recorded — a heap binding aliases the box's
    buffer (owned + freed by the scrutinee's enum drop), so dropping it would double-free.
  - **Sentinel threading.** 6 `cg_m_*` fields: `cg_m_tag`/`cg_m_payload`/`cg_m_result`/`cg_m_merge` SET
    in the `Expr::Match` arm and SAVED+RESTORED around `dump_tarms` (nested matches); `cg_m_armnext`
    (the arm's next-check block, set in `dump_tpat`, CAPTURED into a `dump_tarms` local before the body
    so a nested match can't clobber it) and `cg_m_pj` (the variant flat index, for `cg_emit_pstruct` in
    the bind). The prologue (icmp/br/arm-label) + bind emit in `dump_tpat`/`dump_tbinds`; the epilogue
    (store/br merge/next-label) in `dump_tarms`. ⚠ `selfhost_ast_drop` is a recursive enum — its
    nested-box leak is box-free-only on BOTH backends (the D.1b limit), consistent + `leaks`-clean.
  - **NEXT = (8f) calls/recursion/multi-module** → **(8g) THE BOOTSTRAP FIXED-POINT.** The Bar-A
    construct set is now essentially complete (scalars, control flow, structs, arrays/strings, refs,
    Vec, builtins, heap drops, enums+match) — what remains is the whole-program plumbing for the
    fixed-point.

- **A16 — (8f-1) THE SELFHOST FRONT-END STAGES SELF-HOST:** the Sentinel codegen (`scg`) emits its OWN
  `selfhost/lexer.sentinel` (390 lines → 4378 `.ll` lines) and `selfhost/parser.sentinel` (2590 lines →
  21606 `.ll` lines) **byte-identically to `snc llvm`**, AND the `cc`-compiled `.ll` runs identically to
  inkwell (`snc build`) — a step toward the bootstrap fixed-point: the compiler compiling its own source.
  This was achieved with **NO codegen change** — the Bar-A construct set (closed at 8e) already suffices;
  the lexer + parser exercise it at real scale (20×–500× the largest corpus fixture). Locked in by a new
  differential test (`sentinel_codegen_matches_oracle_on_selfhost_stages`). 1471 tests, four-check green.
  - **The self-contained stages** (`lexer`/`parser`, no `use`) lower through the existing single-file
    pipeline; the **multi-module** stages (`types`/`resolve`/`effects`/`codegen`, which `use` each other)
    need the merged path — `snc llvm` currently rejects `use` ("not yet wired"), while `snc build` already
    discovers the module graph + `merge_modules` → `run_build_merged`.
  - **NEXT = (8f-2)** — wire `snc llvm` to the merged multi-module path (mirror `run_build`'s discovery +
    `merge_modules`, emit `.ll` instead of an object) so the oracle can lower the FULL merged compiler
    (`types`/`codegen`). Then **(8g) the fixed-point** — the harder half: `scg` is single-file (reads one
    `input.sentinel`), so self-compiling the multi-module compiler needs `scg` to merge too (port the
    driver's discovery + `merge_modules` + `Renamer` to Sentinel) OR a pre-merged single source. The
    capstone: `scg` emits the compiler's `.ll`; `cc` → `scg'`; assert `scg'` emits == `scg` (byte-for-byte
    self-compilation — why C5 shipped `abi-v1` + reproducible builds).

- **A17 — (8f-2/8f-3) `snc llvm` LOWERS THE FULL SELF-HOSTING COMPILER:** with the multi-module path
  wired + the last two lvalue stragglers ported, **`snc llvm` emits every selfhost stage**, including
  the merged `codegen.sentinel` (the 3-deep parser→types→codegen chain, **~83k `.ll` lines**), and the
  `cc`-compiled `.ll` runs identically to inkwell. The Sentinel side handles the same lvalue forms
  (seed-validated). 1472 tests (a field-place seed + a merged-compiler emission guard), four-check green.
  - **(8f-2) multi-module `snc llvm`.** `run_llvm` gained the D.6 dispatch (mirroring `run_build`):
    `discover_module_graph` → `merge_modules` → a new `run_llvm_merged` that runs resolve → check →
    borrow-check → `llvm_dump::dump` over the merged `Program` (bypassing Salsa, like `run_build_merged`).
    The single-file path is unchanged. This is what lets the oracle lower the multi-module stages.
  - **(8f-3) lvalue field places.** The selfhost sources use exactly two address-of forms — `&mut V`
    (already handled) and **`&mut (*c).f`** (236×) — plus the symmetric `(*c).f = x`. Added the
    `FieldAccess` arm to the oracle's `lower_lvalue_ptr` (GEP into the target's lvalue pointer:
    `getelementptr %Struct.N, ptr <target>, i32 0, i32 field_index`) + the `FieldAccess` assign target.
    🔑 **The Sentinel needed only ONE change** — a `FieldAccess`-under-`cg_suppress` branch emitting the
    GEP + signalling `cg_lastvid = -1`; the existing `&mut`/`SAssign` machinery already treats
    `cg_lastvid = -1` as "the operand IS the place address", so address-of-field and assign-to-field both
    fell out. Validated by a single-file seed (`&mut (*b).items` + `(*b).n = …`): scg == oracle, cc ==
    inkwell, leak-free. (Element-ref `&a[i]` is unported — the selfhost sources don't use it.)
  - **The Bar-A construct coverage is PROVEN COMPLETE on the real compiler** — the merged `codegen`
    (the whole front end + analysis + the codegen itself) lowers with zero stragglers.
  - **NEXT = (8g) THE FIXED-POINT.** The oracle now lowers the full compiler; the remaining half is the
    Sentinel side: `scg` is single-file, so self-compiling the multi-module compiler needs `scg` to MERGE
    (port `discover_module_graph` + `merge_modules` + `Renamer` to Sentinel — a sizable feature) OR a
    pre-merged single source fed to `scg`. Then: `scg` emits the compiler's `.ll`; `cc` → `scg'`; assert
    `scg'` emits == `scg` — byte-for-byte self-compilation.

- **A18 — (8g) THE BOOTSTRAP FIXED POINT — the self-host capstone (path b, merge-to-source):** the
  Sentinel codegen `scg` (`snc build` via inkwell) lowers the WHOLE multi-module self-hosting compiler
  and REPRODUCES it. **(1) Self-compilation:** `scg` reads the MERGED compiler source (`snc merge`) and
  emits `.ll` **byte-identical to the `snc llvm` oracle** (83,536 `.ll` lines). **(2) Fixed point:** `cc`
  that `.ll` → a fresh compiler `scg'`, which re-emits the **same** `.ll` byte-for-byte — the compiler
  reproduces its own output (why C5 shipped `abi-v1` + reproducible builds, ADR 0029). Validated by
  `sentinel_codegen_reaches_the_bootstrap_fixed_point` (the (8g) capstone test); 1473 tests, modes 0–4
  byte-identical, `scg` leak-free (`leaks --atExit`: 0 leaks lowering the merged compiler), four-check
  green. **The headline self-host milestone: the Sentinel compiler compiles itself.**
  - **🔑 PATH (b), MERGE-TO-SOURCE [owner-chosen, D8(ii)].** Rather than port the D.6 driver merge to
    Sentinel (path a — `discover_module_graph` + `merge_modules` + the `Renamer`, ~550 intricate lines),
    the Rust driver merges the multi-file compiler into one `Program` (the existing `merge_modules`) and
    prints it back to a single `.sentinel` file fed to the unchanged single-file `scg`. Every compiler
    STAGE (lex → resolve → types → effect → borrow → ctverify → codegen) runs inside `scg`; only the
    module-merge PRE-PASS stays in Rust (driver plumbing, ADR-sanctioned). Path (a) — self-hosting the
    merge itself — is a strictly-stronger, separable follow-on (ideally with its own `snc merge`-AST
    oracle).
  - **`$`-in-identifier lexer extension.** `merge_modules` qualifies every top-level name by module path
    (`util$add`, `parser$Expr`). `$` was unused in Sentinel syntax and absent from every corpus fixture,
    so admitting it as an identifier-CONTINUATION char (Rust lexer regex `[A-Za-z_][A-Za-z0-9_$]*` +
    `selfhost/parser.sentinel`'s `is_ident_cont` + `lexer.sentinel` for port faithfulness) is
    byte-neutral for all existing source and lets the `$`-qualified merged names round-trip through
    re-parse.
  - **`source_dump.rs` + `snc merge`.** A faithful `Program → .sentinel` un-parser over the Bar-A subset
    (fns/structs/enums + every stmt/expr/type/pattern/`match`), Err-loud on exotic kinds. **Fidelity by
    construction:** parens are AST-transparent (no `Paren` node, ast/lib.rs:147) → every compound expr is
    wrapped `( … )`, preserving precedence AND sidestepping positional ambiguity (struct-lit-in-head etc.)
    with no `allow_struct_lit` re-derivation; decls emit per kind in vector order (the parser buckets by
    kind → intra-kind order fixes the IDs); string/char bytes re-encode via `\xHH`. `snc merge` =
    `discover_module_graph` → `merge_modules` → `source_dump`. **Round-trip proven** before the capstone:
    `snc llvm <merged-source>` == `snc llvm <multi-module-entry>` byte-for-byte over the selfhost `types`
    and `codegen` (the Rust-only fidelity gate).
  - **TWO (8g)-revealed cg gaps in `types.sentinel` mode-4, fixed to match the oracle.** Both involve
    constructs that appear ONLY in the merged `types`/`codegen` (never in the corpus or the self-contained
    lexer/parser) — which is exactly why (8g), the first time `scg` lowers the full compiler, surfaced
    them. The Rust oracle (`llvm_dump.rs`) already handled both (it had to, to lower the full compiler at
    A17); these are the Sentinel port catching up — no oracle change.
    - **Field-place GEP base (28×).** `&mut c.f` / `c.f = x` on a LOCAL struct (`c` a `Var`, the cg's
      pervasive ctx pools) must GEP into the var's **alloca slot** (`cg_slot_get`); the A17 `cg_suppress`
      mechanism used the stale threaded operand (correct only for a `*c` Deref target, where it holds the
      loaded pointer), so a field-place after a store/branch rendered `ptr 0` or a wrong register. Now:
      capture the target's `cg_lastvid`; `>=0` (a `Var` target) → slot, else (Deref/nested) → operand —
      mirroring the oracle's recursive `lower_lvalue_ptr` (Var→slot, Deref→value, FieldAccess→inner GEP).
    - **`match` wildcard (1×).** A `_` arm (always last in the selfhost sources) emits its body + `store`
      result + `br merge` as the FINAL ELSE — not the A15-deferred `unreachable`. A new save/restored
      `cg_m_wild` flag (nesting-safe like the other `cg_m_*` fields) tells the match epilogue to skip
      `unreachable`. Matches the oracle's `lower_match` wildcard branch.
  - **The fixed-point consistency subtlety** (a good illustration): the merged source IS the compiler's
    source, so the capstone must merge the CURRENT `types.sentinel` — a STALE merge made `scg` (fixed
    logic) and `scg'` (cc'd from `scg`'s output, which EXECUTES the merged source's logic) disagree by
    exactly the two fixed bugs. Fresh merge → convergence.
  - **REMAINING for ADR 0045 → ACCEPTED:** Bar B (generics + nullable + classes/traits/impls +
    effects/handlers + concurrency) for full ~123-corpus parity (D7/D10). The owner may instead re-scope
    Bar B as a deferred follow-on and declare the port closed at the fixed-point (Revisit, below) — the
    headline (compiler compiles itself) is reached.

- **A19 — PATH (a), (a-1)+(a-1b): THE SINGLE-MODULE SENTINEL UN-PARSER.** The owner chose **path (a)**
  (self-host the D.6 module merge so `scg` discovers + merges + emits with NO Rust pre-pass — the "true
  full self-host") over Bar B / closing the port. Its foundation is a Sentinel un-parser:
  **`selfhost/merge.sentinel`**, a parsed program re-emitted as re-parseable source (the Sentinel port of
  the Rust `source_dump.rs`), reusing the parser's AST enums + `tokenize`/`parse_block`/`parse_type`/
  `parse_params` + the **consuming-walk structure of the parser's own `dump_*`** (which already walk + emit
  a [different] text from the AST). It handles the **full selfhost subset** — fns + structs + enums + every
  expr/stmt/type/pattern. **Round-trip PROVEN byte-identical** on the real single-module stages: `snc llvm`
  on the un-parsed source == `snc llvm` on the original — `lexer.sentinel` (4,390 `.ll` lines) +
  `parser.sentinel` (21,618 `.ll` lines); leak-free (`leaks --atExit` 0 un-parsing `parser.sentinel`);
  guarded by `sentinel_merge_unparser_round_trips_single_module_stages`; 1474 tests, four-check green.
  - **🔑 EMISSION RULES (from `source_dump.rs`).** Parens are AST-transparent (no `Paren` node) → wrap
    EVERY compound expr in `( … )` (preserves precedence + sidesteps positional ambiguity, no
    `allow_struct_lit` re-derivation), **EXCEPT `if`/`while`/fn-body blocks** which must stay literal
    `{ … }` (emitted raw via `emit_block_raw` — the parser requires a brace block there, not `(`).
    Op-codes → symbols (mirroring the parser's `bsym`/`usym`: Binary 1-15, Unary 1-5); string/char bytes
    re-encode via `\xHH` (or a bare printable byte); decls + struct fields / enum variants emit with a
    trailing comma (re-parse accepts it).
  - **DATA-MODEL spine proven** (probe): `scg` can read multiple files, build paths at runtime, tokenize
    each sequentially, and accumulate into one `Vec<u8>` — with NO `Vec<Program>` held (each module is
    parsed, used, dropped before the next), because each module's rename map is self-contained. ⚠ An
    earlier "heap crash" blocker was a FALSE ALARM — a probe's exit 139 was its correct return value
    (`53+48+19+19`), misread as SIGSEGV (`128+11`); `lldb` confirmed a clean exit. No bug.
  - **NEXT:** (a-2) the per-module rename map (flat parallel pools, the resolve/types idiom) + the rewrite
    FUSED into the un-parser (look up each name-emit site: Call callee / StructLit-ClassInit-Qcall names /
    type Idents / Pattern enum / Handle-Perform effects; never `Var`/locals/field/method/variant/op names);
    (a-3) BFS discovery (read entry, follow `use` edges, build `<root>/a/b.sentinel` paths, read each);
    (a-4) wire `discover_and_merge` into `codegen.sentinel`'s `main` (then `types::run(merged, 4, …)`) +
    the capstone (`scg` discovers+merges+emits == the `snc llvm` oracle, `cc`→`scg'` self-reproduces).

- **A20 — PATH (a) COMPLETE: THE SELF-HOSTED MERGE — THE TRUE FULL SELF-HOST.** `scg`
  (`codegen.sentinel`, which now `use`s `merge.sentinel`) **DISCOVERS + MERGES + EMITS the whole
  multi-module compiler ITSELF — with NO Rust merge pre-pass.** It reads the multi-module entry, follows
  the `use` edges, merges in Sentinel, and lowers the merged program to `.ll` **byte-identical to the `snc
  llvm` oracle** (94,390 `.ll` lines for the full `codegen`+`merge`+`types`+`parser` graph); `cc` that
  `.ll` → `scg'`, which re-emits the same `.ll` byte-for-byte (the fixed point). Leak-free; 1476 tests,
  four-check green. (a-2)+(a-3) feat `ef56104`, (a-4) feat `d33397c`.
  - **(a-2)+(a-3) THE SELF-HOSTED MERGE.** BFS discovery over `use` edges + a per-module rename map + the
    rewrite fused into the un-parser. **🔑 The enabler: each module's rename map is SELF-CONTAINED** (its
    own top-level names qualified by its `$`-prefix + each `use a::b::Item` → `a$b$Item`, computed from the
    use path alone — `merge_modules` never cross-references other modules to build a map), so modules
    process ONE AT A TIME (re-parse per module — NO `Vec<Program>`, NO HashMap; flat parallel pools, the
    resolve/types idiom). The rewrite fuses into `emit_expr` via `emit_name` at the qualify sites (Call
    callee / StructLit-ClassInit-Qcall names / type Idents / Pattern enum / Handle-Perform effects; never
    `Var`/locals/field/method/variant/op — mirroring the Rust `rewrite_expr`); `use`s are stripped; the
    entry's `main` keeps its symbol. **Matches the Rust short-circuit:** a genuinely single-file entry (no
    `use`) is a PASSTHROUGH (no qualify), gated on `has_use` of the entry — so the existing single-file
    `scg` tests (corpus/seeds/selfhost-stages/fixed-point) hold unchanged. Proven: the merged `types`
    (83,458 `.ll`) and `codegen` (83,536) match the oracle byte-for-byte.
  - **(a-4) `scg` SELF-MERGES.** `merge.sentinel`'s body is a `pub fn merge_source() -> [u8]`;
    `codegen.sentinel`'s `main` is `merge_source()` → `types::run(merged, 4, …)` — so ONE binary does the
    whole pipeline (discover → merge → lex → … → codegen). Tests:
    `sentinel_merge_matches_oracle_on_multi_module_stages` + `sentinel_merge_unparser_round_trips_single_
    module_stages` (path-(a) un-parser/merge) and `sentinel_codegen_self_merges_the_compiler_and_reaches_
    fixed_point` (the capstone).
  - **⚠ FALSE-ALARM, recorded:** the data-model probe's "heap crash" was a misread — a probe's exit 139
    was its correct return value (`53+48+19+19`), not SIGSEGV (`128+11`); `lldb` showed a clean exit. No
    bug; the spine was sound all along.
  - **BOTH fixed-point paths are now reached:** path (b) (8g, A18 — Rust merge-to-source feeding
    single-file `scg`) AND path (a) (A20 — `scg` self-merges). The headline self-host milestone is
    doubly-secured. **REMAINING for ADR 0045 → ACCEPTED:** Bar B (generics + nullable + classes/traits/
    impls + effects/handlers + concurrency) for full ~123-corpus parity — or re-scope it deferred and
    declare the port closed (Revisit D7/D10).

- **A21 — BAR B OPENED (full-corpus parity → ADR 0045 ACCEPTED); the `print` builtin landed.** Owner
  chose to pursue Bar B. The method is the established per-slice lock-step: extend the oracle
  (`llvm_dump.rs`) + the Sentinel mode-4 cg (`types.sentinel`) for each construct the corpus uses but the
  selfhost compiler doesn't, **mirroring the inkwell backend** (`crates/sentinel-codegen/src/lib.rs`) for
  the layout/lowering; validate = the codegen differential (`sentinel_codegen_matches_oracle_on_corpus`,
  byte + behavioural) + modes 0–3 byte-identical + leaks + four-check; one feat per construct, docs batched.
  - **✅ `print` (FnId 0)** — `print(i64) -> i64` → `call i64 @sentinel_print(i64 x)` (oracle: a
    `RuntimeSyms.print` flag + the FnId-0 case in `lower_call` + the declare before print_bytes; Sentinel:
    `cg_used_print` + the `fid==0` arm in `cg_emit_call`). The emitting set grew **57 → 73** (the 16 print
    fixtures). Feat `391ae58`.
  - **🔑 THE REMAINING BAR-B BREAKDOWN** (from `snc llvm` over the corpus — the count is fixtures whose
    FIRST un-portable construct is this; some overlap):
    - **effects/handlers — 18 effecting fns + perform/handle/return-arm exprs** (c35/c36/c37). The gate is
      `dump_fn`'s `if !sig.effect_row.is_empty()` Err (llvm_dump.rs:200) + `lower_expr`'s Perform/Handle.
      THE HAIRIEST (~2300 lines — kont ABI + the ~765-line shape-detection + the handler machinery).
      Mirror inkwell's Handle/Perform/ResumeKont lowering. Likely the last + biggest slice.
    - **nullable `?T` — 4 type + 3 expr** (c14/c15: null lit, widen-to-nullable, `unwrap_or`/`is_some`
      FnId 1/2). `llvm_ty` needs the `Nullable` layout (inner-dependent: scalar vs heap-payload struct —
      see inkwell `llvm_basic_type` lib.rs:1623); the Sentinel interner already has nullable kind 4 +
      `mk_nullable`/`nullable_inner_of`. No table-threading. A clean self-contained next slice.
    - **secret `secret T` — 6 type + declassify** — STRIP-TO-INNER (a no-op; the value IS the inner). The
      oracle's `llvm_ty` (a free fn, 31 call sites) needs the secrets table to strip `Type::Secret(id)` →
      `secrets[id].inner` (`Type::strip_secret(&[SecretData])` exists, sentinel-types lib.rs:793; inkwell
      strips at llvm_basic_type entry, lib.rs:1599). Cleanest: an `Emit::lty(&self, ty)` that strips via
      `self.program.secrets` then calls `llvm_ty`, replacing the Emit-context `llvm_ty(` calls; strip
      inline at the struct-decl loop + `dump_fn` ret. `declassify(e)` → lower `e` (no instruction). The
      Sentinel `cgo_ty` strips kind-3 → inner similarly.
    - **generics — 5 generic fns + 2 GenericInstance** (c16/c17) — monomorphization (mono instances +
      `GenericInstance` struct layout + generic-struct mangling). Gate: `dump_fn`'s
      `if !sig.type_params.is_empty()` Err (lib.rs:198) + `llvm_ty`'s GenericInstance + `lower_call`'s
      "generic call" Err. Mirror inkwell's `generic_instances`/mono.
    - **classes/traits/impls/delegates** (c41/c42/c43/c4_named_impl) — MethodCall + ClassInit +
      QualifiedCall lowering (dispatch + witness/vtable + init). Mirror inkwell's class/impl codegen.
    - **concurrency — spawn/scope/await** (c4_go_no_go) + arena routing — the task/scope runtime symbols.
  - **NEXT (recommended order, easiest→hardest):** nullable → secret(+declassify) → generics → classes →
    effects/handlers → concurrency → the full-corpus phase-go (8l) → ADR 0045 ACCEPTED.

- **A22 — BAR B: the nullable `?T` slice LANDED (feat `8150ccc`).** Ported the nullable lowering to BOTH
  backends in lock-step, mirroring inkwell (`lib.rs` `llvm_basic_type` :1623 / `lower_is_some` /
  `lower_unwrap_or` / NullLit / WidenToNullable / the Cmp nullable arm). The codegen differential's
  emitting set grew **73 → 80** — the 6 `c15_*` fixtures (`null_literal` / `widen` / `eq_null` /
  `nullable_struct_field` / `maybe_compose` / `go_no_go`) **+ `c16_linked_list_node`** (which reaches
  `?Struct` via `null`). (c17_generic_nullable stays Err'd — it needs mono, the generics slice.)
  - **The layout** (oracle `llvm_ty` + Sentinel `cgo_ty`/`ll_type_to`): `?T` = `{ i1 valid, <payload> }`,
    inner-dependent — `?primitive` inline (`{ i1, T }`), `?Struct`/`?GenericInstance` heap-indirect
    (`{ i1, ptr }`, which is what makes `struct Node { next: ?Node }` representable — the cycle goes
    through a pointer-sized field). `llvm_ty` is self-contained (no table-threading; the layout is fully
    determined by `NullableInner`). Pass-0 struct decls + fn param/return types pick it up for free.
  - **The exprs:** `null` → the `{ i1 0, <zero> }` **constant operand** (no instruction; `?Struct` →
    `ptr null`) — Sentinel models it as a new operand **kind 3** (`cgo_operand`), keyed by the inner type
    handle. `T → ?T` widening → the `{ i1 1, T }` **insertvalue** chain — Sentinel adds **`cg_widen`** (the
    codegen counterpart to `mir_widen`), called at every widen site (Int/Bool/Var/binop/…); a `secret`
    widen is a no-op (the value IS the inner). `x == null` (ADR 0014 D7) → extract each i1 valid bit
    (field 0) + `icmp` those (`cg_cmp_null`). `is_some` (FnId 2) → `extractvalue 0`; `unwrap_or` (FnId 1)
    → extract valid+payload + `select` (both operands pre-evaluated, so no control flow).
  - **`?Struct` heap-box WidenToNullable is DEFERRED** (Err) — unexercised (the corpus only widens
    primitives + reaches `?Struct` via `null`). Drop stays a no-op for nullable (`needs_drop`/`cg_needs_drop`
    false) — correct for the corpus (no heap-boxed nullable is ever bound-then-dropped); pairs with the
    deferred heap-box. ⚠ FIX in-flight: `cg_extract` returned `cg_reg`'s `0`, not its dest register — now
    returns `d` (its natural API; pre-existing callers ignored the return).
  - **Validation:** the 7 fixtures byte-identical (`scg` == `snc llvm`) + behaviourally equal to inkwell
    (exit/stdout) + leak-free (`leaks --atExit`: 0 leaks — nullable is inline / a null pointer, no heap).
    Modes 0–3 byte-identical; BOTH bootstrap fixed-point paths preserved (pure-additive — the selfhost
    sources use no nullable). Four-check green (1476 tests). **NEXT: secret + declassify** (strip-to-inner).

- **A23 — BAR B: the secret + declassify slice LANDED (feat `7b471a9`).** `secret T` lowers IDENTICALLY to
  its inner T — strip-to-inner, a no-op at the value level (ADR 0019 D5/D12; the constant-time guarantee is
  the source-level rejections + the 7/N D5 verifier, NOT a distinct runtime representation). The emitting
  set grew **80 → 85** — `c31_secret_typing`, `c31_go_no_go` (`secret i64` + `secret bool` +
  `secret==secret→secret bool`), `c52_secret_ct`, `c53_ct_eq`, + the `c52_secret_leak` ui fixture.
  (`c5_go_no_go`/`c33_go_no_go` stay Err'd — they need effects/handlers + classes, not secret.)
  - **The strip** mirrors inkwell's `llvm_basic_type` ENTRY strip (lib.rs:1598). Oracle: the free `llvm_ty`
    has no secrets table, so a new **`Emit::lty(&self, ty)`** strips a top-level `Type::Secret(id)` via
    `ty.strip_secret(&self.program.secrets)` (sentinel-types lib.rs:793) then calls `llvm_ty`; every
    Emit-context `llvm_ty(` call became `self.lty(` (scoped prefix-replace, ~34 sites), and the 4 non-Emit
    sites (Pass-0 struct field + `dump_fn` ret/params) strip inline via `program.secrets`. Sentinel:
    `cgo_ty`/`ll_type_to` strip secret (interner **kind 3**) at their ENTRY — a stripped primitive
    (`secret bool` → `i1`) must route to the scalar arms, so strip BEFORE the dispatch (the prior `i64`
    fallback was wrong for `secret bool`/`secret i32`).
  - **The exprs are already identity:** `declassify(e)` lowers `e` (oracle `WidenToSecret|Declassify =>
    lower(inner)`; Sentinel's Declassify arm flows the inner operand untouched); the `T → secret T` widen is
    a no-op (`cg_widen` only wraps nullable, kind 4). Drop stays no-op for secret scalars (`needs_drop`/
    `cg_needs_drop` false — secret scalars own no heap; a hypothetical `secret Struct` drop is deferred,
    unexercised).
  - **Validation:** the 5 fixtures byte-identical (`scg` == `snc llvm`) + behaviourally equal to inkwell +
    leak-free (no heap). Modes 0–3 byte-identical; BOTH fixed-point paths preserved. Four-check green (1476
    tests). **NEXT: generics (mono)** — `dump_fn` type_params Err (lib.rs:198), `llvm_ty` GenericInstance,
    `lower_call` generic Err; mirror inkwell's `generic_instances`/mono.

- **A24 — BAR B: generics, sub-slice (a) — generic STRUCT instances LANDED (feat `d3be39b`).** The generics
  slice splits into **(a) generic struct instances** (this) + **(b) generic fns / monomorphization** (next).
  A concrete `Decl<args>` (`Box<i64>`, `Pair<i64,i64>`, `Holder<[i64]>`) lowers like a plain struct, but
  its LLVM type is named **STRUCTURALLY** (`%Box_i64`, `%Holder_arr_i64`) — NOT by interner id, because the
  Rust and Sentinel type-checkers may intern instances in different orders, so a structural name (mirroring
  inkwell `mangle_generic_struct_name`, lib.rs:2198) is order-independent. The emitting set grew **85 → 87**
  (`c17_box`, `c25_generic_struct_array_drop`). (The other c17 fixtures need generic FNS / mono → (b).)
  - **Oracle:** `llvm_ty` now takes `&TypedProgram` (threaded — this UNIFIED the A23 secret strip with the
    new `GenericInstance` arm; `Emit::lty` forwards, the 4 dump/dump_fn sites pass `program`). `mangle_type`
    / `mangle_instance` (local ports). Pass 0 emits `%<mangled> = type { <substituted field types> }` for
    each concrete instance (`Type::substitute` on the decl fields by the instance args; abstract
    TypeParam-bearing instances skipped via `type_has_typeparam`). `needs_drop` /
    `emit_drop_for_binding` gain GenericInstance arms (substitute fields, recurse) → `Holder<[i64]>` frees
    its `[i64]` field.
  - **Sentinel mode-4:** `cgo_ty`/`ll_type_to` render kind-10 via `cg_mangle_to` (a structural mangle to a
    buffer); `cg_has_typeparam` / `cg_struct_is_generic` (a generic decl = a TypeParam-bearing field — the
    Sentinel doesn't track a struct type-param count) skip generic decls + abstract instances; `cg_pass0`
    iterates the type interner (kind 10) for concrete instances; `cg_needs_drop`/`cg_emit_drop` + the
    field-access arm handle generic-instance targets. ⚠⚠ **All new helpers BIND `(*c).ta[idx]` etc. to a
    local before a recursive `&mut c` call** — the nested-`&mut`-ctx quirk yields a WRONG value otherwise
    (this cost a debug cycle: `cg_has_typeparam(c, (*c).fldty[i])` silently read garbage → generic decls
    weren't skipped + field subst gave `i64`).
  - **⚠ THE UN-PARSER (`merge.sentinel`) had to become generics-aware.** `scg` = `merge_source` (un-parse)
    + `types::run(4)`; `merge_source` ALWAYS re-emits via `emit_module` (no raw passthrough even for a
    single-file no-`use` entry). `emit_struct_decl` did `skip_type_params` — DROPPING `<T>` — so a re-emitted
    `struct Box<T>` became `struct Box`, and the re-parse scanned `value: T` as an unbound `i64` (the merge
    was built generics-unaware — the selfhost sources use none). Fixed: a new **`emit_type_params`** emits
    `<T0, T1, …>` verbatim (type-param names are local, never module-renamed). `emit_type` already preserved
    generic ANNOTATIONS (`Box<i64>`). Non-generic decls are byte-unchanged → the fixed point holds. (The fn
    `emit_fn_decl` `<T>` fix is bundled with (b), where generic fns are exercised.)
  - **Validation:** c17_box + c25 byte-identical (`scg` == `snc llvm`) + behaviourally equal to inkwell
    (42 / 66); `scg` leak-free on c25 (the `Holder<[i64]>` drop). Modes 0–3 byte-identical; BOTH fixed-point
    paths preserved (the un-parser change is generic-only). **NEXT: (b) generic fns / mono** — the
    discovery worklist (`collect_mono_instantiations`) + substituted mono defines (`TypedFnDef::substitute`)
    + `mangle_mono_name` (`id__i64`) + `lower_call` mono dispatch (oracle); a Sentinel mono pass that
    re-walks each generic fn body with the type-param scope bound to concrete args; + `emit_fn_decl`'s `<T>`.

- **A25 — BAR B: generics, sub-slice (b) — generic FNS / MONOMORPHIZATION LANDED (feat `170a13a`); the
  GENERICS slice is COMPLETE.** A generic fn is emitted ONCE PER concrete instantiation under a mangled
  symbol (`id__i64`, `__`-separated — vs the single-`_` generic-struct mangle); each call resolves to its
  instance's symbol. The emitting set grew **87 → 92** (c17_id, c17_two_instantiations [`pick__i64` +
  `pick__bool` — two monomorphizations of one fn], c17_generic_nullable, c17_generic_array, c17_go_no_go).
  - **Oracle:** REUSES the inkwell backend's `collect_mono_instantiations` (made `pub` — pure logic, so the
    oracle discovers the same set in the same worklist order). The main fn loop FILTERS generic defs; a mono
    loop substitutes each generic def → a concrete `TypedFnDef` (`TypedFnDef::substitute`) and emits it via a
    new `dump_fn_named` under `mangle_mono_name`. `lower_call` threads `type_args` → a generic callee uses
    the mangled symbol. (Insertion order; the corpus has no transitive generic-from-generic calls.)
  - **Sentinel mode-4 (the hard half — a 2nd pass over the mode-4 flow):** the pass-2 dispatch SKIPS generic
    fns in mode 4 (recording each `fn`-token position, `cggfn_id`/`pos`); `dump_generic_call` records the
    instance (`cg_mono_record`, dedup) + stashes the inferred type-args (`cg_targs`). A **MONO PASS** (after
    the main walk, before the declares) RE-WALKS each instance's body via `type_fn` with `cg_mono_on` +
    `cg_mono_args`, so `type_of_typeexpr` resolves `T` → the concrete arg and `cg_emit_fn` mangles the
    symbol. `cg_emit_call` mangles a generic callee from `cg_targs`. ⚠ `dump_args_capture_all` had to
    `cg_collect` the generic-call args (was mir-only — else the mangled call had no args).
  - **Un-parser:** `emit_fn_decl` now PRESERVES the generic `<T>` (capture to a temp, emit after the name) —
    else the re-parsed fn lost its type-params and mono never triggered (`uftp` stayed 0). Non-generic fns
    byte-unchanged → the fixed point holds.
  - **Validation:** the 5 fixtures byte-identical (`scg` == `snc llvm`) + behaviourally equal to inkwell;
    `scg` leak-free on c17_go_no_go. Modes 0–3 byte-identical; BOTH fixed-point paths preserved (the generic
    machinery is inert for the generics-free selfhost sources — `nmono`=0, no fns skipped). **NEXT: classes/
    traits/impls** (MethodCall / ClassInit / QualifiedCall dispatch + witness/init).

- **A26 — BAR B: classes / traits / impls / delegates LANDED (feat `a1a3341`).** A class is a Pass-0 named
  aggregate `%Class.N` (N = ClassId, like `%Struct.N`) held BY VALUE; on top of it a **pointer ABI** (ADR
  0022 D9, mirroring the inkwell backend lib.rs:643–740 + 2362–2520 + 5671–5771): `init` = `void
  @Class__init(ptr out, params)` (writes through the out-ptr); a method = `<ret> @Class__method(ptr self,
  params)`; an impl method = `<prefix>__<Type>__<Trait>__<method>` (`prefix` = the impl name, or `default`).
  The emitting set grew **92 → 98** (c41_class_basic, c41_go_no_go, c42_trait_basic, c42_go_no_go,
  c43_go_no_go, c4_named_impl). 🔑 `self` binds DIRECTLY to the first param `%arg0` (NO alloca, so writes
  persist to the caller): `Var(self)` LOADS the whole `%Class.N`, `self.f`'s lvalue GEPs `%arg0`, a method
  receiver passes `%arg0`/the field GEP. Everything else composes through the existing FieldAccess (an
  aggregate `extractvalue` read / a GEP-into-pointer write — generalised from struct to class) + call paths.
  - **The 4 lowering forms.** `ClassInit{id,args}` → alloca `%Class.N`, `call void @Name__init(ptr <slot>,
    args)`, then `load` (the alloca is reserved BEFORE the args, matching inkwell's order). `MethodCall` →
    `lower_lvalue_ptr(target)` for `self`, then `call @Class__method(ptr self, args)`. `ImplMethodCall`
    (receiver-typed dispatch, Path 1) → same self-ptr ABI but the impl-method mangle, always the `default`
    impl. `QualifiedCall` (Path 2) → `args[0]` IS the receiver (a ref lowered to a `ptr`), passed as `self`;
    `args[1..]` are the declared params. **Delegates need NO special codegen** — the type layer (4f-delegate,
    resolve Pass 4.5) already synthesised each `delegate f: T to Tr;` into an ORDINARY `impl as Tr for C`
    whose body is `self.f.m(args)`; codegen emits it like any impl (GEP the inline field → a `ptr` to it →
    dispatch to the field type's default impl, e.g. `default__Logger__Writer__write` GEPs `self.writer` and
    calls `default__FileSink__Writer__write`).
  - **Oracle (`crates/sentinel-driver/src/llvm_dump.rs`):** `Type::Class` → `%Class.N` in `llvm_ty`; a
    Pass-0 class-type loop; a `dump_method` (the self-ptr ABI — `self` = `ptr %arg0`, declared params
    `%arg1..`, `ret void` + tail-ignored for init) over `program.class_decls` (init then methods) then
    `program.impl_decls` (incl. the delegate-synth impls, ImplId order — so the order is all fns, then all
    class methods, then all impl methods); `mangle_impl_method`; an `Emit::self_var` + the `self` special-case
    in the `Var` rvalue (`load … %arg0`) + `lower_lvalue_ptr` (`%arg0`). Placeholder `current_fn = FnId(MAX)`
    (no DropPlan entry → empty moved-set; corpus methods own no heap anyway).
  - **Un-parser (`source_dump.rs` + `selfhost/merge.sentinel`) — the gotcha-flagged half.** Both REJECTED
    class/trait/impl DECLARATIONS (out-of-Bar-A); now both emit them so a re-parse yields the
    STRUCTURALLY-IDENTICAL program (same ClassId / field+method indices / delegate ImplIds — all fixed by
    intra-kind source order, preserved). `source_dump.rs` is AST-driven; `merge.sentinel` is TOKEN-driven
    (re-walks the parser's token stream — mirrors `parser.sentinel`'s class/trait/impl walk; `parse_self_kind`
    + `is_kw_init` exposed `pub`). The corpus differential routes through `scg` (= merge + types mode-4), so
    BOTH the un-parse AND the cg must be right; validated by `snc llvm <orig>` == `snc llvm <(snc merge orig)>`
    (Rust) + `scg` == `snc llvm` (Sentinel).
  - **Sentinel mode-4 (`selfhost/types.sentinel`):** `%Class.N` in `cgo_ty`/`ll_type_to`/`cg_pass0`; operand
    **kind 4** = `%arg0`; a **`cgcls` buffer** for class/impl/delegate DEFINEs, appended after `cgout` (the
    fns + mono) — the group-ordered pass-2 walk (classes → impls → delegate-synth) already lands them in the
    oracle's order, so ONE buffer suffices; `cg_self_var` + `cg_arg_base` (=1 for methods, so `emit_tparams`
    stores `%arg{i+1}`); the `Var`/`Field` `self`+class arms; ClassInit/Method/Qcall cg; `cg_emit_method`
    (the define assembler, into cgcls) + `cg_emit_method_sym` (built from `cgm_*` context fields set by
    type_class/type_impl/synth_delegate_impl) + the call emitters (`cg_emit_class_mcall`/`cg_emit_impl_mcall`/
    `cg_emit_qcall`, names from the flat class/trait method tables in `snb`); `synth_forward` emits the
    delegate forwarding dispatch. ⚠ Bound `(*c).field[i]` to a local before any `&mut c` helper call (the
    nested-`&mut`-ctx quirk).
  - **Validation:** the 6 fixtures byte-identical (`scg` == `snc llvm`) + behaviourally equal to inkwell
    (exit 7/42/42/42/42/42) + leak-free (`leaks --atExit`: 0 leaks — corpus classes are stack-only, no heap
    fields). Modes 0–3 byte-identical (no regression — the cg is `cg_on`-gated); BOTH bootstrap fixed-point
    paths preserved (the selfhost sources declare no classes → `cgcls` stays empty, the un-parser's
    class/trait/impl emitters are never reached). 1476 tests, four-check green. **NEXT: effects/handlers**
    (18 effecting fns + perform/handle/return-arm — the hairiest ~2300 lines: the kont ABI + the ~765-line
    shape-detection + the handler machinery) → concurrency (spawn/scope/await) → the full-corpus phase-go →
    **ADR 0045 ACCEPTED**.

- **A27 — BAR B: effects/handlers, sub-slice c35a — inline perform/handle/resume LANDED (feat `29e3027`).**
  The first + simplest of the production's C3.5(a)–(e)+C3.6 handler sub-phases: the RESTRICTED case where a
  `handle` body is a DIRECT `perform` (no effecting fn, no captured frames). `handle perform Eff.op(a) with {
  Eff.op([msg,] k) => k(…) }`, lowered in lock-step (oracle + both un-parsers + the Sentinel mode-4 cg). The
  emitting set grew **98 → 101**: `c35_handle_inline_perform`, `c35_handle_log_returns_msg`, + the type-clean
  NEGATIVE `c37_perform_outside_handle` (the codegen oracle emits it — it doesn't run effect-check, which is
  what rejects `unhandled effect Io in main`; analogous to `c52_secret_leak` being a type-clean CT-negative in
  the emitting set; a tests/ui fixture, behaviourally skipped). (The other c35/c36/c37 fixtures stay Err'd —
  they need the effecting-fn ABI / resumers / return-arm / nesting of the later sub-slices.)
  - **The kont ABI** (ADR 0020, `sentinel-runtime`, mirrored): `SentinelKont { i32 op_id@0, i32 _pad, i64
    arg@8, i8 consumed@16, [7 x i8], ptr frames_head@24 }`; `encode_op_id = (eid<<16)|(op&0xFFFF)` (=
    `eid*65536+op` in the Sentinel, no `<<`/`|`); `PURE_RETURN_OP_ID = u32::MAX` (emits `i32 4294967295`,
    `cc`-accepted). Symbols: `perform_op(i32,i64)->ptr`, `kont_resume(ptr,i64)->ptr` (a pure-or-bubble result
    kont), `kont_consume_pure(ptr)->i64`. The runtime owns the kont memory (perform allocs; resume +
    consume_pure free) → leak-free with NO codegen-side drops.
  - **The lowering.** `perform Eff.op(a)` → `call ptr @sentinel_perform_op(i32 op_id, i64 a|0)` (the kont*).
    `handle` → a dispatch LOOP over a `current_kont_slot`: load the kont, read its `op_id` (offset 0), then an
    **if-else CHAIN** — one `icmp op_id, <encode(eid,op)>` + branch per arm, then a final `icmp op_id, PURE`
    → `consume_pure` (the body-was-pure path), `default` `unreachable` — merged through a **result memory
    cell (NO phi)**. A handler arm binds its op-param (`kont.arg` via i8-GEP @8 into an i64 slot) + the
    continuation (a slot holding the kont ptr). `k(v)` (a `ResumeKont` — in the Sentinel, a `Call` whose
    callee resolves to an in-scope var) → `kont_resume(kont, v)` then `icmp result.op_id, PURE` → pure
    (`consume_pure` = the resumed value) vs bubble (store the new kont to the slot + branch back to the loop).
  - **⚠⚠ THE IF-ELSE CHAIN, NOT A `switch` — in BOTH backends.** The Sentinel's `HArms` is a single-
    consumption cons-list (you can't iterate the arms twice — once for a `switch`'s case list, once for the
    bodies — and `match &enum` / `Vec<Expr>` are both unsupported), so the dispatch MUST chain (mirroring the
    match cg). The oracle was first prototyped with a `switch`, found unmirror­able, and rewritten to the
    chain so the two are byte-identical. (This was the scout's key finding, ADR-recorded in HANDOVER, now
    realized.) ⚠ The kont load in `k(v)` is emitted BEFORE the resume arg is lowered (the oracle's order) —
    the Sentinel splits its resume helper (`cg_emit_resume_load` before the arg walk, `cg_emit_resume_tail`
    after) so the register numbering matches.
  - **Oracle (`llvm_dump.rs`):** the `Perform`/`Handle`/`ResumeKont` `lower_expr` arms + `lower_handle` (the
    chain + result cell) + `lower_resume_kont` + `bind_handler_arm_params` + `encode_op_id` + a `handle_stack`
    on `Emit` + the 3 kont `RuntimeSyms` declares.
  - **Un-parsers (`source_dump.rs` + `merge.sentinel`):** emit effect DECLs (`effect Name { op(p:T) -> R; }`)
    — both REJECTED them before; `merge.sentinel`'s `emit_expr` already had `perform`/`handle`. The corpus
    differential routes through `scg` (merge + types mode-4), so both must round-trip.
  - **Sentinel mode-4 (`types.sentinel`):** the `Perform`/`Handle`/`resume-kont`(Call) cg woven through the
    shared 5-mode `dump_tharms` (the if-else chain) + the `dump_thparams` VarId-range param binding; a
    `cg_alloca_ptr` (raw `alloca ptr` via a `-1` type sentinel in the hoist pool); the handle dispatch state
    (`cg_h_opid`/`ck`/`loop`/`cks`/`rslot`/`merge`, save/restored for nesting) + 3 `cg_used_*` flags on
    `TyCtx`; the kont declares in `run` mode 4.
  - **Validation:** both c35a fixtures `scg` == `snc llvm` byte-for-byte + behaviourally == inkwell (exit 42)
    + leak-free (`leaks --atExit`). Modes 0–3 byte-identical (no regression — the cg is `cg_on`-gated); BOTH
    bootstrap fixed-point paths preserved (selfhost uses no effects). 1476 tests, four-check green. **NEXT:
    c35b** (the effecting-fn ABI — a `!{E}` fn returns `Kont*` — + pure-return wrap [`kont_pure`] + handle-of-
    fn-call body + multi-arm) → c35c (let-bound perform + per-let resumer fns + `kont_push`) → c35d/c35e/c36a/
    c36b → the full-corpus phase-go → ADR 0045 ACCEPTED.

- **A28 — BAR B: effects/handlers, sub-slice c35b — the effecting-fn Kont\* ABI + pure-return LANDED (feat
  `02891fd`).** The second handler sub-phase: a fn with a non-`Async` effect row returns a continuation
  pointer (`Kont*`) instead of its declared type, so a `handle` whose body is a **CALL to an effecting fn**
  (not only an inline `perform`) dispatches on the kont that call returns. The emitting set grew **101 → 107**
  — SIX fixtures flip `Err → Ok` (byte-identical `scg` == `snc llvm` + behaviourally == inkwell + leak-free):
  `c35b_handle_fn_call_body` / `c35b_handle_multi_arm` / `c35b_handle_pure_return` (the targets), plus
  `c32_go_no_go` + `c33_go_no_go` (effecting fns with pure / call-to-effecting bodies, **no `handle`** — a
  natural consequence of the ABI: `check_inner() !{Io} { 0 }`, `maybe_log() !{Io} { x+1 }`, `check_outer()
  !{Io} { check_inner() }`), plus **`c5_go_no_go`** (the C5 phase-go — its simple `handle recv_client_share()
  with { Net.recv(req,k) => k(5) }` + classes + secret all now lower). The remaining c35c/d/e/c36/c37
  fixtures stay `Err`'d (let-bound / embedded / chained perform, return arm, nested handle).
  - **The ABI** (mirror inkwell `uses_kont_abi` / the C3.5(b) shape): an **effecting fn** = a non-empty
    effect row that contains some effect OTHER than the built-in `Async` (a direct-runtime marker —
    spawn/await, never `handle` — so an `Async`-only fn keeps the plain value ABI). Such a fn returns `ptr`.
    Its tail is either **kont-producing** (`produces_kont` — a direct `perform`, a call to another effecting
    fn, or a block whose tail is, with no performing statement) → return the `ptr` as-is; or **pure** (no
    `perform` anywhere) → wrap the i64 via `sentinel_kont_pure(i64) -> ptr` (a PURE_RETURN-tagged kont) so the
    caller's `handle` sees a uniform `Kont*` and matches the PURE_RETURN case.
  - **Oracle (`llvm_dump.rs`):** lift the `dump_fn` effect-row Err gate; gate emission with the new
    `uses_kont_abi` + `validate_effecting_fn_body` (`stmt_performs` / `expr_performs` — defer a body with a
    performing statement, or a tail that mixes a `perform` into pure context like `perform Op()+1` /
    `f(perform Op())`, to c35c+). `ret_ll = "ptr"` for an effecting fn; the return wraps a pure tail via the
    new `kont_pure` `RuntimeSym`. `lower_call` returns `ptr` for an effecting callee; `lower_handle`'s body
    gate is the extended `produces_kont(body, program)` (multi-arm was already the if-else chain). The c35a
    dispatch loop (if-else chain + result memory cell, NO phi) is unchanged.
  - **Un-parser (`merge.sentinel`):** `emit_fn_decl` now **re-emits** the `! { E }` effect row (new
    `emit_effect_row`, effect names via `emit_name_slice` so they rename in a multi-module merge) instead of
    `skip_effect_row` dropping it — else the merged source loses the row, the re-parse never sees the
    effecting fn, and it lowers as a plain `i64`-returning fn (`ufeff` stays 0). **This is the A24 generic-
    `<T>` preservation analog** — `merge_source` always re-emits (no raw passthrough; `merge_mode` only gates
    the rename map), so a dropped row is silently lost. (`source_dump.rs` already round-trips the row; it is
    deliberately Bar-A-scoped and rejects `c33`/`c5` `declassify`, which is harmless — no corpus-wide
    `source_dump` round-trip test exists; the corpus differential routes through `scg` = `merge.sentinel` +
    `types.sentinel`.)
  - **Sentinel mode-4 (`types.sentinel`):** a per-FnId effecting table **`ufeff`** recorded in `scan_fn_sig`
    (pass-1, peeking the row via `eff_row_is_kont` / `cg_is_async` for any non-`Async` name); `cg_emit_fn`
    returns `ptr` + wraps a pure tail via `sentinel_kont_pure` (gated on the per-fn `cg_eff` = `ufeff[fnidx]`
    + `cg_tailk`); `cg_emit_call` returns `ptr` for an effecting callee + sets `cg_tailk`; `cg_emit_perform`
    sets `cg_tailk`; the `cg_used_kontpure` declare in `run` mode 4. ⚠ `cg_tailk` is a dynamic flag (reset in
    `cg_reset`, set by the last `perform` / effecting-call) — correct for the emitted set (no emitted
    effecting fn has a let-bound effecting call + pure tail, the one staleness case validate would otherwise
    let through). The **handle body lowering needed NO change** (it already stores the body operand as the
    initial kont `ptr`, whatever produced it — c35a's direct-perform or c35b's effecting-call).
  - **Validation:** 107/107 emitted fixtures `scg` == `snc llvm` byte-for-byte (`sentinel_codegen_matches_
    oracle_on_corpus`); the 6 behaviourally == inkwell (`llvm_behaviour_matches_inkwell_over_emitted_subset`,
    exit 42/0) + leak-free (`leaks --atExit`). Modes 0–3 stay byte-identical (the cg is `cg_on`-gated; `ufeff`
    is an inert pass-1 table); BOTH bootstrap fixed-point paths preserved (the selfhost compiler declares no
    effects → `ufeff` all 0, `cg_eff` never set). 1476 tests, four-check green. **NEXT: c35c** (let-bound
    perform — per-let resumer fns + `sentinel_kont_push`, the captured-frame chain) → c35d/c35e/c36a/c36b →
    the full-corpus phase-go → ADR 0045 ACCEPTED.

- **A29 — BAR B: effects/handlers, sub-slice c35c — let-bound perform + the captured frame LANDED (feat
  `96c54b9`).** The third handler sub-phase (the production's C3.5(c)): an effecting fn whose body is a
  **let-bound perform in non-tail position** — `fn f(..) !{E} { let v: i64 = perform Op(..); <pure tail> }` —
  where the perform is reified into a runtime **captured evaluation frame** so the kont's resume replays the
  let's tail. The **first sub-slice that emits TWO `define`s per source fn** + the first use of
  `sentinel_kont_push`. The emitting set grew **107 → 110** — THREE fixtures flip `Err → Ok` (byte-identical
  `scg` == `snc llvm` + behaviourally == inkwell + leak-free): `c35c_let_bound_perform` (no capture —
  `do_work() { let v = perform Io.read(); v + 1 }`), `c35c_let_bound_perform_with_capture` (captures the param
  `offset`; `v + offset`), and **`c35c_go_no_go`'s analog `c37_go_no_go`** (the C3.7 phase-go — a
  perform-WITH-arg `perform Io.log(x)`, captured `x`, tail `x + logged`, + `print(result)` → stdout `85`,
  exit `0`; a natural consequence of the shape). The other c35d/c35e/c36/c37-negative fixtures stay `Err`'d
  (embedded / chained perform, return arm, nested handle).
  - **The two-`define` lowering** (mirror inkwell `compile_effecting_fn_with_let`):
    1. the **PARENT** `@<name>` (the c35b Kont\* ABI, returns `ptr`): bind params, allocate + fill the
       **captured-state struct** (`i64[N]` via `sentinel_alloc`, one field per captured var at byte offsets
       `0,8,…`, i8-GEP-stored; a **null ptr** when nothing is captured), lower the let's effecting RHS to a
       Kont\*, **`sentinel_kont_push(kont, @__resume_<name>, captured)`** a frame, `ret ptr` the kont.
    2. the **RESUMER** `@__resume_<name>(i64 %arg0, ptr %arg1)`: bind the let var to the resumed value
       `%arg0` (a `resumed_value` alloca) + each captured var to its struct field (i8-GEP-loaded from
       `%arg1` into a fresh alloca), lower the **pure tail**, wrap it via `sentinel_kont_pure`, `ret ptr`.
    The two defines share **NO register counter** (each starts `%v0` fresh — LLVM `%vN` are NAMED locals, so
    non-contiguous numbering across the pair is legal). The runtime owns the kont / frame / captured memory
    (`sentinel_kont_resume` frees them as it drains the chain) → **leak-free with NO codegen-side drops**.
  - **The resumer ABI** (`sentinel-runtime` `SentinelFrame`): `resumer(value: i64, captured: *mut u8) -> *mut
    SentinelKont`. At c35c the resumer is **non-performing** (it wraps its result via `sentinel_kont_pure`);
    the runtime's `sentinel_kont_resume` already bubbles on a non-pure-return resumer result (the c35e
    chained-let groundwork). The captured struct is the tail's free vars (minus the let var) at i64 offsets.
  - **Oracle (`llvm_dump.rs`):** `detect_let_shape` (an effecting non-main fn, body = a single i64 `let`
    whose RHS `produces_kont` + a pure tail) routes `dump_fn_named` to the new **`dump_let_shape_fn`** BEFORE
    `validate_effecting_fn_body` (which still defers embedded / chained / mixed-tail performs to c35d+);
    `collect_captured_vars` + `walk_collect_var_refs` walk the tail for free VarIds (first-reference order =
    the struct field layout) minus the let var; a new **`kont_push` `RuntimeSym`** (`declare void
    @sentinel_kont_push(ptr, ptr, ptr)`, in the kont group after `kont_pure`).
  - **Un-parsers: NO change.** `source_dump.rs` round-trips the let-bound-perform body + the `!{E}` effect
    row byte-identically (verified: `snc llvm <merged>` == `snc llvm <orig>` for all three); `merge.sentinel`
    already emits `let` / `perform` / `handle` / the effect row (c35a/c35b). c35c introduces NO new syntax —
    just a new combination of already-supported constructs.
  - **Sentinel mode-4 (`types.sentinel`):** an effecting fn (`cg_eff`, mode-4 only) routes through the new
    **`cg_emit_fn_eff`**, which detects the let-shape **structurally** — a single `SLet` statement (every
    EMITTED effecting fn with a performing statement IS a let-shape, since the oracle's
    `validate_effecting_fn_body` defers all other performing bodies → they never reach `scg`; so the Sentinel
    needs no i64 / pure-tail re-check) — and emits the parent + resumer via **`cg_letshape_emit`**, else falls
    through to **`cg_eff_normal`** (the c35b straight-line path: `dump_texpr` + `cg_drop_frame` + `cg_emit_fn`).
    The PARENT reuses the already-set-up param state (`emit_tparams` ran — cg slots + the register counter at
    `#params`); the RESUMER `cg_reset`s to a fresh counter, manually binds the let var (`nextvid++` +
    `bind_name` + a slot) + rebinds each captured param's slot (loaded from `%arg1`; its TYPE binding persists
    in the env, so the tail's `Var` resolves), walks the tail, wraps via `sentinel_kont_pure`. Both defines
    hand-assemble their header + the shared `cg_flush_allocas_body` (hoisted allocas + cgbody fold) + a
    custom `ret ptr`. The **capture set is the param VarId range `[cg_pv0, cg_pvn)`** (captured in `type_fn`
    around `emit_tparams`) = the oracle's first-reference order for the c35c corpus (**≤1 param per let-shape
    fn, always used in the tail**); multi-param first-reference ordering is a c35d+ refinement. ⚠ The Sentinel
    match grammar has NO bind-the-whole-value pattern (only `Enum::Variant(..)` + `_`), so the non-let-shape
    branches re-wrap the moved-out `Stmts`/`Stmt` parts (both fully enumerated) and fall through; the body's
    `_` arm is the unreachable non-`Block` case (a fn body is always a `Block`). New: `cg_used_kontpush` (+ its
    mode-4 declare).
  - **Validation:** 110/110 emitted fixtures `scg` == `snc llvm` byte-for-byte (`sentinel_codegen_matches_
    oracle_on_corpus`); the 3 behaviourally == inkwell (`llvm_behaviour_matches_inkwell_over_emitted_subset`,
    exit 42 / stdout 85) + leak-free (`leaks --atExit`: 0 leaks — the runtime frees kont/frame/captured).
    Modes 0–3 stay byte-identical (the let-shape routing is `cg_on`+`cg_eff`-gated, mode-4 only); BOTH
    bootstrap fixed-point paths preserved (the selfhost compiler declares no effects → `cg_eff` never set →
    `cg_emit_fn_eff` never reached). 1476 tests, four-check green. **NEXT: c35d** (embedded perform — a tail
    that mixes a perform into pure context, `perform Op()+1` / `f(perform Op())`; inkwell
    `detect_embedded_perform_shape` + the placeholder-substituted resumer) → c35e (chained effecting lets —
    `detect_chained_effecting_lets_shape` + N resumers, the resumer-can-perform bubble) → c36a (return arm) →
    c36b (nested handle) → the full-corpus phase-go → ADR 0045 ACCEPTED.

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
