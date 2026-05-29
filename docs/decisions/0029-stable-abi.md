# ADR 0029: Stable ABI (D7) — define, document, freeze, and test `abi-v1`

Status: PROPOSED — the next C5 productionization sub-phase ADR under ADR
0025 (Phase C5 kickoff) D7, mirroring how ADRs 0026/0027/0028 detailed
earlier C5 sub-phases. Flips to ACCEPTED(-WITH-AMENDMENTS) at D7 (2/N),
recording deviations as numbered amendments.

**D7 (1/N) update (2026-05-29) — DELIVERED.** Per the D11 split: the spec
doc + the runtime-struct / mangling / symbol-set stability tests shipped.
`docs/abi-v1.md` documents + freezes the ABI (D2–D6), each item
cross-linked to its bootstrap source. Tests: `abi_v1_struct_layouts_are_stable`
+ `abi_v1_runtime_symbol_set` (sentinel-runtime) and
`abi_v1_mangling_is_stable` (sentinel-codegen). No emitted bytes change
(documents/tests existing behaviour); +3 tests (1230), four-check green.
**Stays PROPOSED** — the `Type`-layout DataLayout assertions (D7) and the
status flip are **D7 (2/N)**.

**Numbering note.** ADR 0025 D14 penciled "ADR 0029 (stable ABI +
reproducible builds — D7+D8)" under the original numbering; the bitwise
detour (ADR 0027) then shifted broker to 0028, so by creation order the
*next* free number is again **0029** — taken here by stable ABI. Actors
(ADR 0025 D5, surface deferred) and the post-1.0 workstreams (D6
cross-process, D9 modules, D10 LSP) take later numbers when they land.
"Numbers are indicative, not binding" (ADR 0025 D14).

**Reproducible builds (D8) are already satisfied** (ADR 0025 D8 resolved
at C5.0: the C0–C4 build is byte-identical across independent `snc`
processes, guarded by `crates/sentinel-driver/tests/repro.rs`). This ADR
*folds D8 in* as one ABI property — "deterministic emission" — already
guarded; no new reproducibility work is required, only its restatement
as part of the frozen `abi-v1` contract.

Date: 2026-05-29
Related:
  - **0025** (Phase C5 kickoff — PROPOSED): D7 (stable ABI) is the
    workstream detailed here; D8 (reproducible builds) is already done and
    folded in; D9 (modules / separate compilation) is post-1.0, so this
    ADR defines the ABI *contract* without yet shipping a separate-
    compilation linker/module surface.
  - **0015** (arrays / nullable layout): `[T]` = `{ i64 len, ptr data }`;
    `?Struct` = `{ i1 valid, ptr payload }`; `?primitive` = `{ i1, T }`.
  - **0017** (regions / refs): `&T`/`&mut T` lower to opaque pointers
    (second-class; D11). RAII drop + the C5.4 scope arenas do not change
    layouts.
  - **0020** (handler runtime): the `*SentinelKont` ABI for effecting fns
    + the `#[repr(C)]` `SentinelKont`/`SentinelFrame` layouts (size-asserted).
  - **0022/0023** (classes/traits): class instances = named LLVM structs;
    method/init/impl mangling (`Name__init`, `Name__method`,
    `Name__Type__Trait__method`).
  - **0024** (structured concurrency): the `#[repr(C)]` `SentinelTask`
    (32/8) + `SentinelScopeCtx` layouts; `Task` lowers to `*SentinelTask`.
  - **0028** (broker integration): the C5.4 `sentinel_arena_*` runtime
    symbols join the runtime-symbol contract.

## Context

The ABI exists today but is **ad-hoc and implicit** — scattered across
`llvm_basic_type` (the `Type` → LLVM layout map), `mangle_type` /
`mangle_mono_name` / the class+impl name builders, the ~18 `sentinel_*`
extern declarations in codegen, and the `#[repr(C)]` runtime structs.
Nothing *documents* it as a contract, nothing *versions* it, and only the
two runtime structs (`SentinelKont`, `SentinelTask`) have stability
asserts. 1.0 needs the ABI **defined, frozen, and tested** because:

  - **Separate-compilation units must link.** Even though the 1.0
    go/no-go is single-file (ADR 0025 D9 → modules post-1.0), the runtime
    crate is *already* a separately-compiled unit linked against generated
    objects; its symbol + struct contract is a real ABI boundary today.
  - **The runtime must be independently versionable.** A runtime built at
    one version must agree with codegen's emitted layouts/symbols; an
    accidental drift (e.g. a reordered struct field, a renamed symbol,
    a changed `SentinelKont` size) is a silent miscompile.
  - **Phase D self-hosting needs a stable target.** The self-hosted
    compiler emits to the same ABI; it cannot chase an undocumented,
    moving target.

This is a **no-codegen-hazard** sub-phase: it documents and *pins what
codegen already emits*, adding a spec doc + a layout-stability test
suite. It changes no emitted bytes (the `repro.rs` / behaviour-preservation
bar holds trivially), which is a deliberate contrast to the UAF-sensitive
C5.4 codegen change that precedes it.

## Decision

### D1. Goal.

Define, **document**, **freeze at `abi-v1`**, and **test** the ABI of
compiled Sentinel artifacts: the calling convention, the in-memory layout
of every `Type` constructor, the name-mangling scheme, and the
runtime-symbol contract (`sentinel_*` + the `#[repr(C)]` runtime structs).
Deliver a spec document plus a layout-stability test suite that fails on
any accidental drift. No emitted bytes change.

### D2. The ABI spec document (`docs/abi-v1.md`).

A new spec doc is the human-readable contract, frozen at `abi-v1`,
covering D3–D6. It is the source of truth a Phase-D self-hosted backend
or an independent runtime build targets. It records, per item, *where in
the bootstrap it is realised* (file + symbol) so the doc and the code
stay cross-checkable.

### D3. Calling convention.

  - **Platform ABI**: the native C ABI via LLVM target lowering —
    SysV AMD64 on x86-64, AAPCS64 on aarch64 (the two 1.0 targets per
    ADR 0025 D12). Sentinel adds no calling convention of its own; small
    structs are returned/passed per the platform ABI (LLVM decides
    register vs. by-pointer).
  - **`main`** returns `i32` (the C `main` exit code = the tail value
    truncated; ADR 0012 D11).
  - **Ordinary fns** return their declared `Type` by value.
  - **Effecting fns** (`uses_kont_abi`) return `*SentinelKont` (the
    free-monad ABI; ADR 0020 D7), *not* their surface type — the surface
    value is delivered via the kont.
  - **Class init** takes the instance `out_ptr` as its first parameter
    and writes through it (ADR 0022 D9); **methods** take `self_ptr`
    first.

### D4. `Type` data layout (the layout catalog).

Freeze the `llvm_basic_type` mapping (all layouts LLVM-default-aligned,
non-packed):

| `Type` | `abi-v1` layout |
|--------|-----------------|
| `bool` | `i1` |
| `i32` / `i64` | `i32` / `i64` |
| `Struct` / `GenericInstance` / `Class` | named LLVM struct, **fields in declaration order** |
| `[T]` | `{ i64 len, ptr data }` (ADR 0015 D1) |
| `?primitive` | `{ i1 valid, <prim> }` inline |
| `?Struct` / `?GenericInstance` | `{ i1 valid, ptr payload }`, payload heap-allocated (ADR 0015 D11) |
| `&T` / `&mut T` | opaque `ptr` (ADR 0017 D11) |
| `secret T` | **identical to `T`** (no constant-time layout change; ADR 0019 D5 / D12) |
| `Kont` | opaque `ptr` to `SentinelKont` |
| `Task` | opaque `ptr` to `SentinelTask` |

`TypeParam` / `TraitSelf` never reach codegen (monomorphised /
substituted away); `Secret` is stripped at layout entry. These are
documented as "not in the data ABI."

### D5. Name mangling.

Freeze the existing scheme:
  - free fns: the bare source name;
  - monomorphic instances: `base__<tag>__<tag>…` (`mangle_mono_name`,
    `__`-separated type-arg tags);
  - type tags (`mangle_type`): `i64`/`i32`/`bool`, struct/class by name,
    `opt_<T>`, `arr_<T>`, `ref_<T>`/`refmut_<T>`, generic instance
    `Base_<arg>_<arg>`, `sec_<T>`;
  - class init / method: `Name__init` / `Name__method`;
  - impl method: `Name__Type__Trait__method`.

`abi-v1` documents these as the stable external symbol names. (A future
revision may adopt a length-prefixed / collision-proof scheme; flagged in
D8 as the one mangling soft-spot — see Reasoning.)

### D6. Runtime-symbol contract.

Freeze the `sentinel_*` symbol set with signatures, grouped by
subsystem: heap (`sentinel_alloc`/`_free`/`_panic_oob`), scope arenas
(`sentinel_arena_enter`/`_alloc`/`_exit`; ADR 0028), handlers
(`sentinel_perform_op`/`_kont_resume`/`_kont_pure`/`_kont_consume_pure`/
`_kont_push`; ADR 0020), concurrency (`sentinel_task_spawn`/`_await`,
`sentinel_scope_enter`/`_register`/`_exit`; ADR 0024), and I/O
(`sentinel_print`). Plus the `#[repr(C)]` struct layouts the ABI exposes:
`SentinelKont` (32 B / align 8), `SentinelFrame`, `SentinelTask`
(32 B / align 8), `SentinelScopeCtx`.

### D7. Layout-stability test suite.

The machine-checkable half of the freeze — a test fails the moment an
emitted layout, mangling, or symbol drifts:
  - **Runtime structs**: extend the existing `size_of`/`align_of` asserts
    (`SentinelKont`, `SentinelTask`) to cover `SentinelFrame` +
    `SentinelScopeCtx`, and assert key field offsets via `offset_of!`.
  - **`Type` layouts**: a codegen-level test that, for each `Type`
    constructor, queries the lowered LLVM type's size/alignment (and
    struct field offsets) through the target `DataLayout` and asserts the
    `abi-v1` values — so an accidental reorder/repack/width change fails.
  - **Mangling**: golden-string asserts on `mangle_mono_name` /
    `mangle_type` / the class+impl builders for a representative matrix.
  - **Symbol set**: assert the generated module declares exactly the
    `abi-v1` `sentinel_*` symbol set with the documented prototypes (and
    that the runtime crate *defines* them — a cross-crate contract check).
  - **Determinism**: `repro.rs` (compile-twice + diff) already guards
    emission determinism (D8); reaffirmed as an `abi-v1` property.

### D8. Versioning + evolution policy.

Freeze at **`abi-v1`**. Pre-1.0 the ABI may still change, but **every
change updates the spec doc + the stability tests in the same commit** —
the tests exist precisely to force that discipline (a silent drift
becomes a red test, not a latent miscompile). Post-1.0, any
layout/mangling/symbol change is a **breaking bump to `abi-v2`**. The
mangling scheme's lack of length-prefixing (D5) is the one known
soft-spot (theoretical collisions like `a__b` vs `a_ _b`); recorded as a
candidate `abi-v2` hardening, not blocking 1.0 (no current collision in
the surface, which is single-file with user-chosen identifiers).

### D9. Out of scope (post-1.0 / later).

  - The **separate-compilation linker + module surface** (`mod`/`use`,
    module graph, per-unit objects keyed to `abi-v1`) — ADR 0025 D9,
    post-1.0. `abi-v1` defines the *contract* such units would link
    against; it does not ship the units.
  - **Cross-architecture beyond x86-64/aarch64** (ADR 0025 D12).
  - An **ABI-compatibility checker / C-header generator** for external
    FFI consumers.
  - ABI **migration tooling** (`abi-v1`→`v2` shims).
  - Constant-time `secret` *layout* changes — secrets are register/stack
    scalars and lower identically to their inner type; an arrays-of-
    secrets surface (which would add layout) is a deferred follow-on.

### D10. Phase-go + fixtures.

  - **Layout-stability suite green** (D7) — the sub-phase's machine-
    checkable bar; a deliberately-introduced reorder/rename must turn it
    red (verified once during development, then reverted).
  - **`repro.rs` byte-identical** (D8) — unchanged emission.
  - **No `tests/pass` exit/stdout changes** — this sub-phase emits no new
    bytes, so the whole existing suite is the behaviour-preservation guard.

### D11. Sub-phase split.

| Sub        | Title                                                          | Risk | Est.      |
|------------|----------------------------------------------------------------|------|-----------|
| D7 (1/N)   | `docs/abi-v1.md` spec (D2–D6) + the runtime-struct + mangling  | low  | 1 session |
|            | + symbol-set stability tests (D7), each cross-linked to code.  |      |           |
| D7 (2/N)   | The `Type`-layout DataLayout assertions (D7) + the negative    | low  | ≤1 session|
|            | "drift turns it red" check; ADR flip. (May merge into 1/N.)    |      |           |

Low risk throughout: documentation + tests over unchanged emission. No
codegen change, so the `c51` behaviour-preservation bar holds by
construction.

## Reasoning

**Why now, after C5.4.** The ABI is a *prerequisite* for the two things
that follow it — the TLS go/no-go links generated code against the
runtime (an ABI boundary), and Phase D self-hosting emits to this ABI —
so freezing it first de-risks both. It is also the natural low-risk
counterpart to the just-shipped UAF-sensitive C5.4 codegen: it changes no
bytes and is verified by construction.

**Why document-and-pin rather than redesign.** The existing layouts /
mangling / symbols already work across the whole C0–C4 surface and are
deterministic (D8). 1.0's need is *stability*, not a better ABI; the
cheapest stability is to write down what is, freeze it, and add tests
that fail on drift. A redesign (length-prefixed mangling, a richer
calling convention) is `abi-v2` territory and post-1.0.

**Why tests are the load-bearing deliverable.** A spec doc alone rots;
the layout-stability suite is what *enforces* the freeze — it converts
any future accidental ABI drift from a silent miscompile into a failing
test, which is the actual 1.0 guarantee.

## Consequences

### Positive
- The ABI becomes an explicit, versioned, test-enforced contract — a
  precondition for separate compilation, an independently-versioned
  runtime, and Phase D self-hosting.
- Zero regression risk: no emitted bytes change; the entire existing
  suite + `repro.rs` are the guard.

### Negative
- The frozen mangling scheme is not collision-proof (D5/D8); accepted for
  1.0 (no collision in the single-file surface), flagged for `abi-v2`.
- A documented ABI is a commitment: future layout changes now cost a spec
  + test update (by design — that is the discipline being bought).

### Neutral
- No surface-syntax change; programs behave identically.
- Separate compilation / modules remain post-1.0 (ADR 0025 D9); `abi-v1`
  defines the contract they will later use.

## Alternatives considered

- **Defer the ABI to Phase D.** Rejected: the runtime↔codegen boundary is
  already a live ABI, and an undocumented one risks silent drift now;
  Phase D needs it stable *before* self-hosting, not during.
- **Redesign the ABI for 1.0** (length-prefixed mangling, richer
  convention). Rejected: 1.0 needs stability, not a better ABI; redesign
  is `abi-v2`, post-1.0.
- **Spec doc with no tests.** Rejected: a doc without enforcement rots;
  the stability suite is the actual guarantee.

## Revisit

PROPOSED until the sub-phase closes. Triggers:
- **D5/D8**: if the single-file→modules step (post-1.0) surfaces a real
  mangling collision, harden to a length-prefixed scheme as `abi-v2`.
- **D3/D4**: if a later 1.0 surface (e.g. arrays-of-secrets, or a richer
  `Task<T>`) adds a new layout, extend `abi-v1` (a non-breaking addition)
  + its tests in the same commit.

## Appendix: estimated implementation footprint

| Workstream                                              | LOC estimate |
|---------------------------------------------------------|--------------|
| `docs/abi-v1.md` spec (D2–D6)                           | doc, ~300-450 lines |
| runtime struct + offset stability asserts (D7)          | ~60-100      |
| `Type`-layout DataLayout assertions (D7)                | ~120-200     |
| mangling + symbol-set golden tests (D7)                 | ~80-150      |
| **D7 total**                                            | **~260-450 code + doc** |
