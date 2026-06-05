# ADR 0044: Phase D self-host port — (7/N) MIR + the constant-time verifier in Sentinel

Status: **PROPOSED** — the seventh sub-phase of the self-host port (ADR 0031 D5 /
ADR 0038 D9), after the lexer (1/N), parser (2/N, ADR 0039), resolve (3/N, ADR 0040),
types (4/N, ADR 0041), effect-check (5/N, ADR 0042), and borrow-check (6/N, ADR 0043)
— all ACCEPTED. Ports the **MIR lowering** (`lower_to_mir`: the type-checked program →
a minimal SSA/CFG mid-level IR) **+ the constant-time verifier** (`verify_constant_time`:
the secret-leak gate) to Sentinel, differentially validated against a Rust `snc mir`
oracle over the `tests/pass` + `tests/ui` corpus. The SECOND stage to REUSE the typed
program (building on 6/N's `types::run` foundation).

## The reframing — what the scout found (read this first)

The handover framed (7/N) as a single **"HIR/MIR → codegen"** transform pipeline. The
back-half scout showed the architecture is different — three very different things, not a
sequence:

- **HIR is a no-op.** `crates/sentinel-hir` (101 lines) is an *identity bundle*
  (`HirProgram { program: &TypedProgram, drop_plan: &DropPlan }`). No desugaring at 1.0
  (the ADR 0026 D1/D3 escape hatch). Nothing to port as a stage.
- **MIR is an analysis SIDE-BRANCH, not on the codegen path.** `lower_to_mir(typed) ->
  MirProgram` (SSA/CFG) exists **only** to feed `verify_constant_time() -> Vec<SecretLeak>`
  (the secret-leak gate). Confirmed: `compile_to_object(hir)` reads `hir.program()` /
  `hir.drop_plan()` (codegen lib.rs:168–170) — **codegen consumes the TypedProgram
  directly and ignores MIR entirely.** MIR is "analysis-only at 1.0" (mir lib.rs:9–12).
- **Codegen is the real transform** (`TypedProgram + DropPlan → LLVM IR → object`,
  8263 lines, inkwell/LLVM-coupled) and the bootstrap-critical path.

So the pipeline is:

```
… → borrow-check ─→ DropPlan ─┐
                              ├─→ lower_to_mir → verify_constant_time   [ANALYSIS gate — dead-ends here]
                              └─→ compile_to_object(typed, dropplan) → object   [the transform → 8/N]
```

**(7/N) = MIR + verifier is therefore the LAST analysis pass** (the fourth and final gate
the Rust `snc` runs over the typed program: types → effect-check → borrow-check →
**constant-time**), NOT a step toward the object. It keeps the proven differential-dump
cadence; **codegen is a separate (8/N)** with its own kickoff ADR (the emission-target
question — Sentinel has no LLVM FFI — is deferred there; see Context).

**Owner-chosen as (7/N)** over codegen-first (this session, after the scout): finish all
four analysis gates on the low-risk cadence, isolate codegen's hard emission/oracle
question into its own focused ADR, and use MIR as a **⅛-scale rehearsal** of codegen's
exact transform muscles (linearise expressions into an SSA value/instruction stream,
basic-block CFG, lower control flow to branch terminators). The owner picked the full
variant — **the constant-time verifier is IN scope** (not the "drop verifier" option).

## Amendments

- **A1 — (7a) the `snc mir` ORACLE + the SSA data-model PROBE LANDED; D2 + D4 settled, the
  (7b) branch-merge de-risked early.**
  - **(7a-i) the oracle** (`ce29b1e`) — `run_mir` + `mir_dump.rs` + 8 goldens; the AS-BUILT
    grammar is recorded in D2 [LANDED]. Robust: **0 panics over the corpus; accepts exactly
    the 123 type-clean fixtures** (= the types/borrow phase-go set), rejects the 18
    type-rejected. Lowering is total → `snc mir` does not gate on the verifier.
  - **(7a-ii) the SSA data-model PROBE — D4 CONFIRMED + the (7b) merge de-risked.** A
    standalone Sentinel program hand-lowers `dbl` (straight-line) + **`g`** (the
    `let mut x = 1; if c { x = 2; 0 } else { 0 }; x` merge case) by directly driving the
    construction primitives, and reproduces `snc mir`'s dump for BOTH **byte-for-byte,
    leak-free** (`leaks --atExit`: 0 leaks). **The whole IR is flat append-only
    parallel-`Vec<i64>` pools tagged by block** — values (`vty`), instructions
    (`in_blk`/`in_dest`/`in_op`/…), block params (`bp_blk`/`bp_val`), terminators
    (`tm_blk`/`tm_kind`/…), jump-args (`tjargs`) — with **NO `Vec` index-assign anywhere**
    (terminators/insts created out of order still group per block in the dump via a
    tagged-pool scan). `var_defs` = append-only `(var, val)` pairs (`vd_var`/`vd_val`) with
    newest-first lookup + length-snapshot/`pop`-truncate (the resolve scope idiom). **The
    branch-merge** (nominally 7b) is validated: snapshot the `var_defs` depth at the branch,
    walk the then-arm, **SAVE its rebindings into a side `(var,val)` list**, truncate, walk
    the else-arm, then **iterate the VarId range (naturally sorted)** comparing each live
    var's then-value (side-list else snapshot) vs else-value (`vd[s..e]` else snapshot) →
    add a merge param + jump args + rebind on divergence. `g` produces the exact 2-param
    merge (`v5` result + `v6` diverged `x`) with the right jump args. So D4's "the
    `var_defs` index-assign is the #1 risk" is **retired** — the resolve append-only idiom
    carries it, and the hardest slice (7b) is proven ahead of schedule.
  - **D3 (reuse shape) RECOMMENDATION → fused-mode-guarded** (confirming the D3 lean, now
    grounded by reading `types.sentinel`'s `run`/`dump_texpr`): `run(src, mode, result)`
    already builds the type dump in `out` AND the `mvf`/`mvv` move side-table during the
    pass-2 `dump_texpr` walk (the 6/N borrow fuse); add a **`mode 2`** that builds the MIR
    pools (new TyCtx fields) as a guarded side-effect of that SAME walk and dumps them via
    `dump_mir` (mode 2 discards `out`, exactly as mode 1 does). The walk already computes
    each node's TYPE + resolves each `Var`'s real VarId via `sc_lookup` — so `value_tys` and
    the merge's VarId-sort match the oracle **for free, with no scope replay**. The MirValue
    threads up via a `lastval` ctx field (set on each emit, read by parents with the
    save-between-children pattern the type walk already uses); the If/`&&`/`||` arms
    fork blocks under mode 2 (7b). The MIR side-build is strictly `if (*c).mode == 2`-guarded
    and never touches the type-dump-writing, so **modes 0/1 stay byte-identical by
    construction** — re-verify `snc types` + `snc borrow` 123/123 before+after the wiring
    (the 0043 A1 discipline). The alternative (a separate twin walk) is safe-by-construction
    but would duplicate type-derivation + scope/VarId management (heavier for the full
    stage's Opaque/generic forms). **NEXT in (7a): the straight-line INTEGRATION** — MIR
    pools in `TyCtx` + a `mode 2` `lower_fn` + the fused emits on the linear arms
    (Int/Bool/Var/Unary/Binary/Cmp/Call/Block/Let) + `dump_mir` + a thin `mir.sentinel`
    (`use types::run; run(inp, 2, …)`) + a straight-line differential test.

- **A2 — (7a) the straight-line INTEGRATION LANDED; (7a) is COMPLETE.** `selfhost/mir.sentinel`
  (the 7th Sentinel stage, `dc20dd8`) lowers the straight-line grammar (const_int /
  const_bool / var / unary neg+not / binop / cmp / `let` → one block + Return) and matches
  `snc mir` byte-for-byte on 8 seeds (`sentinel_mir_matches_oracle_on_seeds`); **modes 0/1
  (`snc types` / `snc borrow`) stay 123/123 byte-identical** (the mode-2 emits are dead under
  modes 0/1 — the fuse is byte-clean by construction); **leak-free** (`leaks --atExit` 0/0).
  The fuse executed as designed: `TyCtx` gained the ~22 MIR pool fields; `run` sets
  `(*c).mode`; `type_fn` (mode 2) resets the pools + opens an entry block + dumps the fn on
  exit; `emit_tparams` adds each param as an entry SSA param + binds `var_defs`; the linear
  `dump_texpr` arms emit a guarded `if (*c).mode == 2` inst and thread the MirValue via a
  `lastval` ctx field (save-between-children for `Binary`); `SLet` binds `var_defs` to the
  RHS's `lastval`. ⚠ **THE ONE SENTINEL SNAG (a reusable rule):** passing a ctx-field `&mut
  (*c).mirout` to a USER fn (`append_str`/`append_int`) **re-borrows `c`** ("cannot take
  `&mut c` while already borrowed mutably") — only BUILTINS (`push`/`pop`) may take `&mut
  (*c).field`. So the `mir_dump_*` helpers take a **LOCAL `out: &mut Vec<u8>` buffer**
  (distinct from `c`), and `type_fn` folds it into the `mirout` FIELD via a `push` loop.
  **RULE: render into a local buffer + `push`-fold into a ctx Vec field; never pass `&mut
  (*c).field` to a user fn.** **NEXT = (7b)** control flow (`if`/`&&`/`||` → branch + merge —
  the probe-proven algorithm) → **(7c)** the `Opaque` catch-all + `Load` + calls → **(7e)**
  the full-corpus phase-go.

- **A4 — (7c) the LOWERING is COMPLETE + (7e) the full-corpus PHASE-GO is GREEN.**
  `selfhost/mir.sentinel` matches `snc mir` **byte-for-byte over the ENTIRE clean-lowering
  corpus — 123/123 fixtures** (`sentinel_mir_matches_oracle_on_corpus`, the 7e phase-go),
  modes 0/1 (`snc types`/`snc borrow`) stay 123/123 byte-identical, leak-free (full sweep
  over deref/class/enum/effect/handle/generic/trait/named-impl). Two feat commits closed
  (7c): **`bd4ca96`** (the common forms → 87/123) + **`a0a5a3c`** (the effect/class/enum
  forms → 123/123). The mechanisms (all fused, mode-2-guarded):
  - **A `margs` operand STACK** + `mir_emit_va` (op 6 `call` / op 7 `opaque`): the arg-list
    walkers (`dump_targs`/`dump_sfields`/`dump_array_elems`/`dump_args_capture_*`/`dump_cargs`
    →`dump_targs`/`dump_tarms`) push each child's MirValue while a `mir_collecting` flag is
    set (so a shared walker in a non-collecting context can't pollute the stack); a variadic
    arm snapshots `len(margs)`, walks, then `emit_va` copies `margs[snap..]` into the flat
    `miargs` pool and truncates. Receivers/scrutinees are pushed manually (first), then the
    args collect. Nesting works (each `emit_va` truncates its own args).
  - **Op 5 `load`** (Index + `*`-deref, base + optional index), **op 8 `declassify`**,
    and **the widen-Opaque** (`mir_widen`: `WidenToSecret`/`WidenToNullable` are dump-time
    wrappers here but `Opaque([inner])` VALUES in the Rust MIR — emit one at each widen site).
  - **`mir_suppress`** — a place the IR doesn't lower as a value (an assign target, a
    `handle`'s arms/return) walks for the dump but emits NOTHING (the emit helpers early-out).
    A field/`*`-deref/index **store** → `Opaque([value])` (the target suppressed); a plain-Var
    target still rebinds `var_defs` (detected via `mir_lastvid`, which the place-wrapper arms
    reset to −1). A `handle` lowers ONLY its body → `Opaque([body])`.
  - **An UNBOUND Var** (a match/handler payload binding the IR doesn't model) → a fresh
    `Opaque([])`, matching the Rust `lookup_var`. ⚠ Flat-namespace catch: the `Expr::Call`
    arm's two branches (kont vs fn-call) needed distinct `let` names (`kmc`/`kd`) for their
    emit blocks (a per-arm flat namespace; only match ARMS are independent scopes). **NEXT =
    (7d)** the const-time verifier (`verify_constant_time`) + leaking seeds — the last piece
    (the owner's full-version pick); its corpus differential is near-empty (only
    `c52_secret_leak` is a true positive), so the leaking seeds are load-bearing.

- **A3 — (7b) CONTROL FLOW LANDED** (`c15ce6d`): the branch-merge in the `If` and Logic
  (`&&`/`||`) `dump_texpr` arms under mode 2 — the probe-proven algorithm, executed as
  designed. **`If`:** branch on the cond into fresh then/else blocks (snapshot `var_defs`
  depth), walk each arm, then a merge block whose first param is the if-result + one param
  per variable that **diverged** across the arms (VarId-sorted — iterate `0..nextvid`, keep
  those live at the branch), reconciled via the arms' jumps. The then-arm rebindings are
  saved to a side `(var,val)` list, `var_defs` truncated to the branch depth, then the else
  arm walks and each live var's then-value (side-list / branch snapshot) vs else-value
  (`var_defs[snap..]` / snapshot) is compared. **`&&`/`||`:** a short-circuit branch (And:
  true→rhs, false→short-false; Or: true→short-true, false→rhs) merging the rhs value with
  the short constant. New helpers: `mir_term_branch`/`mir_term_jump` + `var_defs`
  prefix/range lookups + truncate + a side-list lookup. ⚠ **ONE MISSING HOOK found by the
  `g` merge seed:** the merge needs **assignments** (not just `let`s) to update `var_defs` —
  7a only hooked `SLet`. Added an `SAssign` rebind: `mir_lastvid` (the VarId the most-recent
  `Var` node resolved to, set in the Var arm) gives a plain-Var target's VarId → rebind
  `var_defs` to the RHS's `lastval`. (A field/`*`-deref store also routes its receiver Var
  through `mir_lastvid`, so the Var-only rebind is exercised by the corpus only once the
  `Opaque` store lands — 7c.) Matches `snc mir` byte-for-byte on **+8 control-flow seeds**
  (`if`, `&&`, `||`, `secret &&`, a 1-var + a 2-var merge, nested `if`s, a logic chain);
  **modes 0/1 stay 123/123 byte-identical**; **leak-free**. **NEXT = (7c)** the `Opaque`
  catch-all + `Load` (index/deref) + calls (with the `mir_args` stack) → **(7e)** the
  full-corpus phase-go.

## Decision

### D1. Goal.

Port two functions from `crates/sentinel-mir` (1134 lines):

1. **`lower_to_mir(program: &TypedProgram) -> MirProgram`** (mir lib.rs:208) — lower each
   **top-level** function's type-checked body into SSA/CFG form. Sentinel has only
   *structured* control flow and no loop back-edges in the IR, so the CFG is a DAG and SSA
   falls out of one structured walk (no dominance-frontier phi placement). `if` / `&&` /
   `||` lower to `Branch` + a merge block whose SSA `params` are the phi-equivalent.
2. **`verify_constant_time(program: &MirProgram) -> Vec<SecretLeak>`** (mir lib.rs:775) —
   the secret-leak gate: no `secret` value may reach a conditional-branch condition, a load
   index or base, or a division divisor. Taint is read **straight off each value's `Type`**
   (`is_secret(v) = Type::Secret(_)`) — the type checker already computed the fixpoint, so
   there is NO separate def-use propagation (mir lib.rs:761–774).

The MIR data model (the Sentinel port must reproduce its shape + numbering):
`MirProgram { functions }`; `MirFunction { name, value_tys: Vec<Type>, blocks, entry,
ret_ty }`; `MirBlock { params: Vec<MirValue>, insts, term }`; `MirInst { dest, op, span }`;
`MirValue(u32)` = index into `value_tys`; `MirBlockId(u32)` = index into `blocks`. **9
`MirOp`s** (`ConstInt`, `ConstBool`, `Unary`, `Binary`, `Compare`, `Declassify`, `Load
{base, index?}`, `Call {callee, args}`, `Opaque(args)`) + **4 `MirTerminator`s** (`Jump
{target, args}`, `Branch {cond, then/else_blk, then/else_args, span}`, `Return(value?)`,
`Unreachable`). `SinkKind` = `Branch`/`MemoryIndex`/`MemoryAddress`/`Division`.

### D2. The oracle — a canonical MIR dump (`snc mir <file>`). [LANDED — the load-bearing differential]

`run_mir` + a new `crates/sentinel-driver/src/mir_dump.rs` (mirroring `borrow_dump.rs` /
`effects_dump.rs`): parse → resolve → check → **`lower_to_mir`** → pretty-print the
`MirProgram`. **The dump is the load-bearing differential surface** — it exercises the
whole transform. (The *verifier's* output is near-empty on clean fixtures — see D6 — so it
cannot carry the differential; the lowering dump must.)

**Proposed dump grammar** (an S-expr, the family style; golden-pinned, the oracle's call):
```
(fn <name> (params v0 v1 …) -> <ret-type>
  (block 0 (params …) (v2 = <op>) … (term <terminator>))
  (block 1 …))
```
where SSA values render as `vK`, types via the types-stage structural `type_display`
(`secret [u8]`, `Vec<i64>`, `Point<i64>` — **no interner IDs**), ops/terminators as nested
S-exprs (`(v3 = binop Mul v0 v1)`, `(v4 = load v3 v2)`, `(term branch v5 -> b1(v6) b2(v7))`,
`(term jump b3(v8))`, `(term return v9)`). **NO raw spans in the dump** — spans are byte
offsets used only by the leak diagnostic (out of scope, D7); rendering them would make the
dump whitespace-fragile. One `(fn …)` per **top-level** fn in `program.fns`, in FnId order.

⚠ **Lowering is TOTAL** (it never rejects — `Unreachable`/`Opaque` absorb unmodelled
forms). So `snc mir` does **NOT** gate on the verifier; it dumps the lowered form even for
a program the verifier would reject. The corpus differential skips only the fixtures the
**upstream** pipeline rejects (parse/resolve/type errors), exactly as borrow/effects did →
the phase-go set is the ~123 type-clean corpus (D9). The verifier is validated separately
(D6). Golden-tested (`tests/mir.rs`). ⚠ Interesting case: `c52_secret_leak` (secret `&&`)
is type-clean (in the 123) but MIR-verifier-rejected — its lowered dump shows a
secret-conditional `Branch`, so the LOWERING differential covers it (123), while the
VERIFIER differential (D6) is where it shows up as the one true positive.

**[LANDED]** `run_mir` + `crates/sentinel-driver/src/mir_dump.rs` + 8 goldens (`tests/mir.rs`).
**AS-BUILT grammar (supersedes the sketch above):** `(fn <name> <ret-ty> <block>…)`; each
block `(block b<N> (params <def>…) <inst>… (term <t>))`; a value DEF (block param / inst
dest) = `v<N>:<ty>` (type via `type_display`, inline colon — may contain spaces, e.g.
`v0:secret bool`), a USE = the bare `v<N>`; inst = `(v<dest>:<ty> <opname> <operands>)` —
`const_int <n>` / `const_bool <true|false>` / `unary <sym> v` / `binop <sym> v v` / `cmp
<sym> v v` / `declassify v` / `load v[ v]` (base then optional index) / `call <callee> v…` /
`opaque v…`; term = `(term return[ v])` / `(term jump b<t> v…)` / `(term branch <cond>
(b<then> v…) (b<else> v…))` / `(term unreachable)`; operators via `.symbol()` (the `snc ast`
form); **NO spans**. Fns in FnId order (= source order). **Robust: 0 panics over the corpus;
accepts EXACTLY the 123 type-clean fixtures** (= the types/borrow phase-go set — lowering is
TOTAL, so every type-clean fixture lowers), rejects the 18 type-rejected. The
`c52_secret_leak` lowered dump shows the secret `Branch` (covered by the lowering
differential). This grammar is now the byte-target the Sentinel `mir.sentinel` reproduces.

### D3. How the Sentinel stage obtains the typed program (the reuse — PROBE-GATED).

MIR lowering needs, at every `TypedExpr` node: (a) its **`Type`** (→ `value_tys` +
`is_secret`), (b) for a `Var`, its **`VarId`** (→ `var_defs`), (c) for a `Call`, the
**callee signature name**. These are *exactly* what `types.sentinel`'s pass-2 walk already
computes per node (the env `VarId → type handle`, the name-blob scope `sc_lookup`, the
fn-sig table). So the reuse is the **6/N template — `types::run(src, mode, result)`** with
a new **`mode 2`** that lowers-to-MIR-and-dumps; `selfhost/mir.sentinel` is then ~10 lines
(`use types::run; run(inp, 2, &mut result)`), a D.6 chain mir→types→parser.

⚠ **THE PROBE (the central call): twin walk vs fused walk.**
  - **(a) Twin walk** — a `lower_*` family inside `types.sentinel` mirroring `dump_t*`,
    re-deriving each node's type as it builds MIR, gated by `mode 2`. PRO: clean separation
    (doesn't touch `dump_texpr`, so mode 0/1 are untouched by construction). CON: duplicates
    the per-node type-derivation logic (~the size of `dump_texpr` again).
  - **(b) Fused walk** — extend `dump_texpr` to BUILD MIR as a `mode 2` side-build,
    REUSING the type handle + VarId it already computes at each node (the 6/N move-recording
    pattern), emitting nothing to `out` under mode 2 and dumping the built MIR after the
    walk. PRO: no type-logic duplication. CON: more invasive than borrow's fuse — borrow's
    was a **pure side-flag** (`consuming: bool`), but MIR **restructures control flow**
    (`if`/`&&`/`||` must FORK blocks), so `dump_texpr` grows block-management under mode 2.

  **Lean (b) fused** — the typer already threads type + VarId + scope through every node
  (exactly MIR's inputs), so the only genuinely new work is the SSA bookkeeping +
  block-forking, which can't drift from the type computation. But the block-forking is the
  real risk → **the (7a) probe builds the SSA construction in miniature first** (D4) and
  confirms mode 2 leaves `snc types` (mode 0) + `snc borrow` (mode 1) **byte-identical**
  (guarded mode-2 code paths) before the full walk. ⚠ The `types.sentinel` change touches
  TWO accepted stages (types 123/123 + borrow 123/123) — re-run both corpora + the leak
  sweep after the refactor, BEFORE adding lowering logic (the 0043 A1 discipline).

### D4. The Sentinel data model — parallel-Vec SSA (the rehearsal; the #1 risk).

Reproduce the IR in the established **flat parallel-`Vec<i64>` + name-blob, integer-indexed**
idiom (`Vec<non-primitive>` unsupported):
  - **`value_tys`** = `Vec<i64>` of **type handles** — REUSE the types-stage interner
    (`tk`/`ta`/`tb`); the handles are already in hand during the walk. `is_secret(v)` =
    the handle at `value_tys[v]` is `Secret`-kind (reuse types' `is_secret`).
  - **blocks / insts / terminators** = flat parallel-Vec pools + **arg slices** (start/len
    into a flat `args` pool — the resolve cons-list-free idiom), NOT `Vec<struct>`.
  - ⚠⚠ **`var_defs` (`VarId → current MirValue`) is THE risk.** The Rust uses
    `var_defs.insert(id, v)` — an **index-assign**, which is **FORBIDDEN in Sentinel** (ADR
    0017 D12). Reproduce it with the **resolve SCOPE idiom**: append-only `(varid, mirvalue)`
    push-pairs + **newest-first lookup** (last push wins) + **length-snapshot**. A `let`/
    assign/param rebind = push a new pair; the `lower_if` `var_defs.clone()` snapshot =
    record the length; per-arm restore = the resolve `truncate` pop. The merge's diverged-var
    scan enumerates the distinct VarIds live at the branch in **VarId-sorted order** (the
    Rust uses a `BTreeMap` precisely for this determinism — `defs_at_branch.keys()`).
  - ⚠ **PRECISION OBLIGATIONS (the byte-for-byte discipline — the analog of resolve's
    VarId order):** `MirValue` numbers (sequential `new_value` alloc order), `MirBlockId`
    numbers (`new_block` order), and merge-param order (VarId-sorted) must match the Rust
    walk EXACTLY. The Sentinel lowering mirrors the Rust `emit`/`add_param`/`new_block`
    sequence node-for-node.

### D5. The lowering walk (the transform shape — the codegen rehearsal).

Per fn in `program.fns` (**top-level only** — class/impl/init method bodies live in
`class_decls`/`impl_decls`, NOT `fns`, and the Rust defers lowering them, D7; generic fn
defs lower as-is with `TypeParam` flowing through inert — never secret). entry block params
= the fn params (each `add_param` + bind in `var_defs`); body via `lower_block` (stmts for
effect, tail for value); the exit block `Return(Some(tail))`.
  - **`lower_stmt`**: `Let` → lower value, bind; `Assign` → lower value, `Var` target
    rebinds (`var_defs`), else `Opaque([v])` (no `Store` op — mutable indexing is out of
    scope, ADR 0017 D12); `While` → lower cond + body **once** (no loop structure — a
    `while` cond is non-secret, and the loop adds no taint path a single pass misses,
    ADR 0036); `Break`/`Continue` → nothing; `Expr` → lower.
  - **`lower_expr` → MirValue**: const → `ConstInt`/`ConstBool`; char/str → `Opaque([])`
    (public constants); `Var` → `lookup_var`; `Unary` (`Deref` → `Load{base, None}` so a
    secret pointer is visible, else `Unary`); `Binary` → `Binary` (a secret `Div` divisor is
    the leak); `Cmp` → `Compare`; `Logic` → **`lower_logic`** (short-circuit branch); `Declassify`
    → `Declassify` (the one taint sink); `Widen*` / `NullLit` → `Opaque`; `Block` → `lower_block`;
    `If` → **`lower_if`** (branch + merge); `Call` → `Call{callee = signature(id).name, args}`;
    `Index` → `Load{base, index: Some}` (a secret index is the leak); **everything else**
    (struct-lit, field-access, array-lit, method/impl-method/qualified-call, class-init,
    perform, resume-kont, handle, scope, spawn, await, enum-construct, match) → **`Opaque`**
    carrying its operands so taint can't vanish.
  - **`lower_if` + `lower_logic` = the central new muscle** (the codegen rehearsal): fork
    `then`/`else` blocks from the `var_defs` snapshot at the branch, walk each arm, then a
    merge block whose first SSA param is the result + one param per **diverged** variable
    (VarId-sorted); arms `Jump` to the merge with matching args. `&&`/`||` are control flow
    (not a binary op) so the verifier can see a secret-dependent short-circuit (`secret bool
    && _` type-checks — `SecretBranch` only rejects `if`).

### D6. The verifier (`verify_constant_time`) — IN scope (owner's pick).

Port it as a pass over the built MIR: for each inst, `Load` secret `index` → `MemoryIndex`,
`Load` secret `base` → `MemoryAddress`, `Binary(Div, _, divisor)` secret `divisor` →
`Division`; for each block, `Branch` secret `cond` → `Branch`. Taint = `is_secret` off the
value's type handle (D4) — **no def-use propagation** (the type carries the fixpoint).
Output = the leak set.

⚠ **DIFFERENTIAL NOTE — the verifier's output is near-empty on the clean corpus.** A
*leaking* program is `snc build`-rejected, so it is not in the clean corpus → the verifier
returns **0 leaks for almost every clean fixture**. Its corpus differential is therefore a
**no-false-positive** check (it must NOT flag the secret-but-constant-time fixtures
`c31_secret_typing` / `c52_secret_ct` / `c53_ct_eq`, where secrets flow through
`Opaque`/`Compare`/non-`Div` `Binary`). The ONE true positive in the type-clean set is
`c52_secret_leak` (secret `&&` → a secret `Branch`). So the verifier differential surface =
a `snc ctverify <file>` (or `snc mir --verify`) leak-set dump over the type-clean corpus
(empty for ~122, `(leak Branch)` for `c52_secret_leak`) + hand-crafted **leaking seeds**
(secret branch / index / div) compared directly against the Rust verifier (the positive
tests). The exact verifier-oracle surface (a sibling subcommand vs a `--verify` flag vs seed
goldens) is a D6 sub-decision, golden-pinned; the lowering dump (D2) is the headline.

### D7. Out of scope.

Codegen (→ 8/N, its own ADR); **class / impl / init method-body** MIR lowering (the Rust
defers it — `lower_to_mir` walks only `program.fns`; revisit only if a fixture's *secret*
path needs a method body, which the corpus does not); the **post-1.0 standalone forward
taint propagation** (only needed once MIR is lowered from *post-optimisation* code — mir
lib.rs:767–771); MIR-level **diagnostic/span** rendering (the `secret_leak` what/why/how +
spans — diagnostic parity has never been ported); the salsa-tracked `mir_query`.

### D8. Sub-slicing (the 0040–0043 cadence; merge/split as the build reveals).

  - **(7a)** the `snc mir` oracle + the **SSA data-model PROBE** (append-only `var_defs` +
    block/value pools + an `if`-merge, over a hand-typed mini-AST — settles D3 twin-vs-fused
    + D4 + confirms mode 2 keeps types/borrow byte-identical) + **straight-line lowering**
    (const / var / unary / binary / cmp / call → one block + `Return`).
  - **(7b)** control flow: `if`/`Block` → `Branch` + merge (the `var_defs` snapshot/merge,
    VarId-sorted params) + `Logic` `&&`/`||` → short-circuit branch. **The hard slice.**
  - **(7c)** the `Opaque` catch-all forms + `Load` (struct/array/field/method/impl-method/
    qcall/class-init/perform/resume-kont/handle/scope/spawn/await/enum-construct/match;
    `Index` → `Load`, `Deref` → `Load`; `Widen`/`Null`/char/str → `Opaque`).
  - **(7d)** the verifier + the leaking seeds (the positive CT tests).
  - **(7e)** the full-corpus phase-go (D9) → ADR 0044 ACCEPTED.

### D9. Phase-go.

`sentinel_mir_matches_oracle_on_corpus` (mirroring the borrow/effects phase-go): build the
Sentinel MIR stage, sweep `tests/pass` + `tests/ui`, skip upstream-rejected fixtures,
assert byte-equal `snc mir` dumps over the type-clean corpus (~123 — lowering is total, so
every type-clean fixture lowers). Green flips ADR 0044 → ACCEPTED.

## Reasoning

MIR + the verifier is the **last analysis pass** — porting it completes every gate the
Rust `snc` runs over the typed program (types → effect-check → borrow-check →
constant-time), all on the proven differential-dump cadence. Two things make it the right
(7/N): (1) it is a **⅛-scale rehearsal** of codegen's exact transform muscles (SSA value/
instruction streams, basic-block CFG, branch-merge for control flow) — at 1134 vs codegen's
8263 lines, so the IR-building patterns are proven before the grand finale; (2) it cleanly
**isolates codegen's hard emission-target + oracle question** (Sentinel has no LLVM FFI)
into a dedicated (8/N) ADR rather than rushing it. The reuse rides the 6/N `types::run`-with-
`mode` foundation; the one genuinely new muscle (SSA construction + branch-merge over an
**append-only** `var_defs`, since index-assign is forbidden) is the de-risk focus of (7a).
The lowering DUMP is the load-bearing differential because the verifier's output is empty on
clean fixtures — a richer surface than borrow's tiny VarId set.

## Consequences

### Positive
- Completes all four analysis gates; the whole front end + every typed-program gate ported.
- A ⅛-scale rehearsal of codegen's SSA/branch-merge transform before (8/N).
- Clean reuse via the established `types::run`-with-`mode` template (mir.sentinel is thin).
- The lowering dump is a rich differential (the full IR shape), unlike borrow's VarId set.

### Negative
- ⚠ `var_defs`'s no-index-assign forces the append-only scope idiom (a real port challenge
  — probe-gated, D4); MIR value/block numbering is a NEW byte-for-byte ordering obligation.
- The fuse (D3 lean b) is **heavier than borrow's** — block-forking, not a pure side-flag —
  so mode 2 risks the types/borrow byte-identity (guarded paths + re-verify, the 0043 gate).
- The verifier's corpus differential is near-empty (mitigated by positive leaking seeds).

### Neutral
- Diagnostic/span parity stays deferred (D7), as every prior stage.
- Class/impl-method-body lowering deferred (the Rust `lower_to_mir` defers it too, D7).
- Codegen is the separate (8/N) — the bootstrap-critical transform + the fixed-point.

## Revisit

- **D3 (reuse shape)**: probe twin-walk vs fused-walk under `mode 2`; confirm mode 2 keeps
  `snc types` + `snc borrow` byte-identical BEFORE lowering logic. Record as A1 (the
  0041 A3 / 0042 A1 / 0043 A1 pattern).
- **D4 (`var_defs`)**: probe the append-only snapshot/merge in miniature (the resolve scope
  idiom at SSA scale) before (7b).
- **D2 dump format + D6 verifier-oracle surface**: the oracle's call — golden-pinned, as the
  prior stages' dumps were.
- **D8 slice boundaries**: merge/split as the build reveals.

## Context

The handover's "HIR/MIR → codegen" was three-in-one; the back-half scout split it (the
reframing above): HIR is a no-op, MIR is an analysis side-branch (codegen reads the typed
program directly), and codegen is the real transform. So (7/N) is the last analysis pass
(this ADR), and **codegen is (8/N)** — where the genuinely new design question lives:
Sentinel has no LLVM/inkwell FFI (only `read_file`/`write_file`/`print_bytes`), so a
self-hosted codegen must emit a **textual** form — most plausibly LLVM IR (`.ll`) via
`write_file` → external `llc`/`clang` link — and its oracle likely shifts from byte-dump
parity to **behavioural run-parity** + the bootstrap fixed-point (flagged for the 8/N ADR,
not settled here). This stage keeps the lexer → … → borrow-check differential-oracle method
and the 6/N typed-program reuse. See ADR 0038 for the port's spine, ADR 0026 for the
HIR/MIR pipeline + constant-time-verification design, ADR 0043 for the reuse template (the
`types::run`-with-`mode` foundation), `docs/agent-protocol.md` for the probe discipline,
and the auto-memory `sentinel_selfhost_port` for the running record.
