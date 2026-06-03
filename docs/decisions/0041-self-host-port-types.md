# ADR 0041: Phase D self-host port — (4/N) types-in-Sentinel

Status: **ACCEPTED-WITH-AMENDMENTS** — the fourth sub-phase of the self-host port
(ADR 0031 D5 / ADR 0038 D9), after the lexer (1/N), the parser (2/N, ADR 0039 →
ACCEPTED), and resolve (3/N, ADR 0040 → ACCEPTED). **(4a) has landed: the `snc types`
oracle + the two design probes (A1) are settled + empirically verified.** Ports the
**types** stage to Sentinel: the
name-resolved program → a **type-checked program** (every expression node carrying
its inferred `Type`, the parser/resolver's remaining syntactic ambiguity resolved
by type — method dispatch — and the implicit coercions made explicit), differentially
validated against a Rust `snc types` oracle over the `tests/pass` + `tests/ui`
corpus. It is the **biggest, hardest stage so far** (the Rust
`crates/sentinel-types/src/lib.rs` is **10,891 lines** — ~1.8× resolve's 6,034) and
is **heavily sub-sliced** (D8), flipping to ACCEPTED-WITH-AMENDMENTS as the slices
land (the cadence of ADR 0039 / 0040). See ## Decision for the sub-slice plan,
## Reasoning for the two central data-model calls (the **`Type` representation**, D4,
and **machinery-sharing with resolve**, D3), and ## Revisit for the probe gates.

⚠ **The two design questions were PROBE-GATED + are now SETTLED (A1).** **D4** (the
`Type` representation) is CONFIRMED as **integer type-handles into a flat hash-consed
interner**; **D3** (machinery-sharing) is CONFIRMED as **lean (a) — `types.sentinel`
`use`s `resolve.sentinel` as a D.6 module**. Both empirically verified leak-free (the
ADR-0040 A1 discipline — `docs/agent-protocol.md`). See ## Amendments A1.

## Amendments

- **A1 — the two design probes (D3 + D4), settled + EMPIRICALLY VERIFIED PRE-BUILD.**
  Two parallel throwaway probes (per `docs/agent-protocol.md`) plus the orchestrator's
  cheap re-verification (the agents' sandboxes denied `snc` exec, so the orchestrator
  compiled + ran + `leaks`-swept the minimal idioms itself) settled both open
  questions before the Sentinel consumer is built.
  - **D4 (the `Type` representation): CONFIRMED — integer type-handles into a flat
    hash-consed interner.** Verified end-to-end (`snc build` + run + `leaks --atExit`
    → **0 leaks**, all assertions pass): a `TyCtx` struct of parallel `Vec<i64>`
    arrays (`tk` kind-tag / `ta` / `tb`), scalars as fixed codes below a `BASE`
    (compounds = `BASE + interner_index`); `intern_type` linear-scans for `(kind,a,b)`
    and returns the existing handle else appends — so **two separately-built
    structurally-equal types get the same handle and equality is integer `==`**
    (confirmed: `secret [i64]` built twice → equal; `&mut i64` → distinct);
    `render_type` recursively walks handles to the structural text (`secret [i64]`,
    `&mut i64`, `?i64` printed correctly); `subst_type` rebuilds + re-interns (a
    `secret [T]`→`secret [bool]` substitution is idempotent under hash-consing). The
    struct-of-`Vec`s interner **auto-drops cleanly** (no explicit teardown — like
    resolve's RCtx).
    - **⚠ THE KEY CORRECTION (source-verified): the VarId→type env must be
      APPEND-ONLY.** `env[vid] = h` is a hard compile error
      (`sentinel::types::IndexAssignNotSupported`, ADR 0017 D12 — mutable indexing is
      unsupported; LHS must be a bare ident / `*ref` / `expr.field`). So the env is a
      `Vec<i64>` of handles **`push`ed in VarId order** (the `vid`-th push *is*
      `env[vid]`, exactly resolve's `scv` idiom); indexed **read** `(*c).env[vid]` is
      fully supported. Never design a pass that overwrites a set env slot.
    - **Three reusable interner-idiom gotchas** (caught by the orchestrator's
      re-verification — each would have been written into the real stage): **(i)**
      pushing a byte into a `Vec<u8>` needs a **char/`u8` literal** (`push(out, '[')`)
      or `i64_to_u8(n)` — a bare int literal trips `type_arg_inference_conflict`
      (`T` bound `u8` then `i64`); **(ii)** **nested `&mut ctx` in one call
      expression** (`f(g(&mut c), &mut c)`) is a `borrow_conflict` — bind the inner
      result to a `let` first; **(iii)** `print_bytes(vec_to_array(v))` **inline
      leaks** (the builtin borrows its `[u8]`) — bind `let arr = vec_to_array(v);`
      first (the resolve idiom). **Secondary:** generic-instance hash-consing must key
      on the **arg-handle CONTENT** (not a packed `(start,end)` slice of a side
      `argv` array, which differs per build) — a documented (4h) follow-up; the core
      5 capabilities need no generics.
  - **D3 (machinery-sharing): CONFIRMED — lean (a).** Verified (`snc build` + run +
    `leaks` → **0 leaks**, exit as designed) a **3-deep D.6 module chain** (`a ← b ←
    main`, mirroring `parser ← resolve ← types`) with a **DIAMOND import** (module
    `a`'s `pub struct S { Vec<i64> fields }` + `pub fn build`/`lookup` taking
    `&mut S` are imported by **both** `b` and the entry `main`): a downstream module
    **constructs the imported struct** (`S { .. vec_new() .. }`), **reuses the
    imported helpers through `&mut S` across the module boundary** (`push`/`len`/index
    a field via `(*c).f`), and drops it cleanly (no `Vec` field moved out). The
    diamond (same `a::` items pulled along two `use` paths) was the one shape the
    existing `resolve ← parser` chain hadn't exercised — it works. So
    `types.sentinel` will `use` `resolve.sentinel`'s `RCtx` + pass-1 table-builders +
    lookups (made `pub`) and drive its own pass-2 typing walk, sharing the symbol
    tables rather than re-resolving inline.
- **A2 — (4a) the `snc types` oracle landed.** `run_types` (driver, parse → resolve →
  `check` → dump) + `types_dump.rs` (the `snc resolve` S-expr form **extended with
  each expression node's inferred `Type`** as a trailing ` :<type>` via
  `type_display`, **+ the type-resolved disambiguations**: the resolver's uniform
  `(method …)` split by receiver type into `(method #classid <idx> …)` /
  `(impl-method …)`; the synthesized `(widen-null …)` / `(widen-secret …)`; the
  `let`'s inferred type replacing resolve's `_`; the computed `field_index` /
  `variant_index` / generic `(targs …)`). **Robust: 0 panics across all 141 corpus
  fixtures (123 type cleanly, 18 oracle-rejected → skipped by the differential, as
  resolve)**; 5 goldens (`tests/types.rs`) pin params/call, let-inference,
  struct-lit+field-index, nullable-widen, and method-dispatch. A dev surface, not
  `abi-v1`. **ADR flips to ACCEPTED-WITH-AMENDMENTS.** Next: (4b) the Sentinel
  `types.sentinel` skeleton (the scalar grammar) on the A1-confirmed `TyCtx` interner
  + the resolve-as-a-module reuse.

## Context

The lexer (1/N), parser (2/N), and resolve (3/N) proved the **differential-oracle
method**: a canonical `snc <stage>` dump the Sentinel stage reproduces byte-for-byte,
diffed over the corpus. Types is the next stage in the D5 order (lexer → parser →
resolve → **types** → HIR/MIR → codegen). What types does (Rust
`crates/sentinel-types/src/lib.rs`; entry `check(&ResolvedProgram) -> Result<
TypedProgram, TypeError>` at lib.rs:3292):

1. **Build the type name-tables + interner tables** (lib.rs:3293–3320): struct /
   class / enum name → ID maps; per-struct type-param counts; and the empty interner
   `Vec`s (`generic_instances`, `refs`, `secrets`, later `konts` / `tasks`) that
   grow as compound types are encountered. Then **resolve every decl's
   type-expressions** to concrete `Type`s (enum-variant payloads 0.5, struct fields
   1, struct-cycle detection 2, effect ops, trait/impl/class method signatures) via
   `resolve_type_expr_with_scope` (lib.rs:1052) — a `TypeExpr` (surface syntax) →
   `Type` walk against the name-tables + a per-item type-param scope.

2. **Type-check every body** (`check_fn` 4561 → `check_block` 4767 → `check_stmt`
   4835 → `check_expr` 5763 — `check_expr` alone is ~1,100 lines): a recursive
   **bottom-up synthesis + bidirectional checking** walk. Each node gets a `ty`
   (`TypedExpr.ty`, lib.rs:2181); `let`/param bindings populate a flat per-fn
   **`VarTypeEnv`** (VarId → `Type`, lib.rs:4686); calls check arg types against
   signatures; the generic ones run **bidirectional inference** (`check_call` 5520
   + `unify_one` 5426 + `try_substitute` 5306 + `contains_type_param` 5368 — the
   HM-ish core, binding `TypeParam`s from arg types).

3. **Resolve the remaining ambiguity + insert coercions** the parser/resolver left
   open *because they need types*:
   - **Method dispatch** — resolve emits a uniform `(method target m args)`
     (`ResolvedExprKind::MethodCall`, no dispatch — it lacks types); types splits it
     by **receiver type** into `TypedExprKind::MethodCall` (a class's own method,
     `check_method_call_expr` 7261) or `ImplMethodCall` (a default-impl method when
     the class has no such method, ADR 0023 D5 Path 1). (`QualifiedCall` /
     `ClassInit` / `EnumConstruct` / `StructLit` / `Perform` are already split by
     resolve — syntactic; types only *type-checks* them, computing `field_index` /
     `variant_index` / reordered struct fields.)
   - **Implicit widening** — `WidenToNullable` (`T → ?T`, ADR 0014 D3) and
     `WidenToSecret` (`T → secret T`, ADR 0019 D5) nodes are **synthesized** around
     expressions whose context expects the wider type (`coerce_to_expected` 5256).
   - **Struct-field reordering** — `StructLit.fields` are reordered source → decl
     order so codegen lowers by index.

The output is `TypedProgram` (lib.rs:1328); `TypedExprKind` (lib.rs:2185) mirrors
`ResolvedExprKind` but **every node carries a `Type`** + the dispatch split + the
widening nodes. The `Type` enum (lib.rs:56) has **15 variants**, several interned to
preserve `Copy + Hash`: `Ref(RefId)`, `Secret(SecretId)`, `GenericInstance(
GenericInstanceId)`, `Kont(KontId)`, `Task(TaskId)` index into `TypedProgram`'s
side-tables; the rest are scalar (`I64`/`I32`/`U8`/`Bool`) or carry a decl ID
(`Struct`/`Class`/`Enum`) or a flat element tag (`Array(ArrayElem)`/`Vec(VecElem)`/
`Nullable(NullableInner)`). `type_display` (lib.rs:896) renders a `Type`
**structurally** (`&mut i64`, `secret [u8]`, `Vec<i64>`, `Point<i64>`) — crucially,
**the interner IDs never appear in the rendering** (only the structure does).

Three facts shape the port, as two facts each shaped the parser (ADR 0039) and
resolve (ADR 0040):

1. **Types must walk a *resolved* program.** Resolve (`resolve.sentinel`) currently
   **resolves-and-dumps in one consuming pass** — it never builds a reusable
   resolved-AST *value*. Types needs the resolved info (VarIds, FnIds, the
   `::`-splits, the symbol tables). So (4/N) opens, exactly as (3/N) opened by
   needing the parser to *return* an AST, by deciding **how types obtains resolved
   input** — re-resolve inline over the parser AST, or reuse resolve's machinery.
   See **D3**.

2. **`Type` is `Copy`, hash-consed-by-interner, and recursively rendered/unified.**
   Sentinel has **no `Copy` enums** (user enums are Move-owned, no clone), **no
   hashmaps**, and **`Vec<non-primitive>` is unsupported** (ADR 0039 A3). A recursive
   Sentinel `Type` enum would hit the *exact* dead-ends resolve's cons-list tables
   hit (can't `match` a `&enum`; a reused `&`-ref enum `match` aliases the heap →
   double-free; partial-consume leaks — ADR 0040 A1). So `Type` cannot be a Move
   enum threaded by value through the env + unify + render. The representation is the
   central design problem — see **D4**.

3. **The interner IDs are invisible in the dump.** Because `type_display` renders
   structurally (D4 below), the Sentinel side need **not** reproduce the Rust
   `RefId`/`SecretId`/`GenericInstanceId` *assignment order* — only the structural
   rendering. This is a major de-risk vs resolve (whose VarIds, shown in the dump,
   pinned an exact assignment order). VarIds still show (the dump extends resolve's),
   but those are already settled by the resolve stage.

## Decision

### D1. Goal.

Port the types stage to Sentinel as the fourth compiler stage, emitting a
**canonical typed-program dump** byte-identical to a Rust `snc types` oracle over the
clean-typing corpus. Heavily sub-sliced (D8). **Effect-check is OUT OF SCOPE** — it
is a *separate crate* (`sentinel-effect-check`, run as its own query after
`check_query`); the types stage is `sentinel_types::check` alone (it computes the
inferred effect rows on `Perform`/`Handle` nodes but does not *discharge* or *check*
them). HIR/MIR and codegen remain their own later ADRs.

### D2. The oracle — a canonical typed dump (`snc types <file>`).

Add `snc types <file>` (driver `run_types` + a `types_dump.rs` module, mirroring
`run_resolve` + `resolve_dump.rs`; `run_types` = parse → resolve → **`check`** →
dump, the `run_resolve` chain plus one `sentinel_types::check` call). It emits the
**`snc resolve` S-expr form extended with each expression node's inferred `Type`**
(via `type_display`) **+ the type-resolved disambiguations**:

- **Every expression node ends with a trailing ` :<type>`** (the node's `ty`,
  rendered structurally by `type_display`), e.g.

  ```
  (fn #14 add ((param #0 a i64) (param #1 b i64)) i64 (block (binop + (var #0 :i64) (var #1 :i64) :i64) :i64))
  (fn #15 main () i64 (block (call #14 (int 1 :i64) (int 2 :i64) :i64) :i64))
  ```

  Full regularity (even `(int 1 :i64)`, where the type is redundant) is the point:
  one emit rule (every typed expr appends `:ty`), and the redundancy *is* the
  validation — it pins that the Sentinel side synthesized the identical type at every
  node. Decl heads (`(fn …)`, `(struct …)`, signatures) are unchanged from `snc
  resolve` (their types are annotations, already shown).

- **The method-dispatch split** (resolve's uniform `(method …)` → one of):
  `(method #classid <idx> target m args :T)` for a class's own method;
  `(impl-method #implid <idx> target m args :T)` for a default-impl method. (The
  already-split `(qcall-impl …)` / `(class-init …)` / `(enum-construct …)` /
  `(struct-lit …)` / `(perform …)` gain only their `:T` + computed indices.)

- **The synthesized coercion nodes**: `(widen-null inner :?T)` and
  `(widen-secret inner :secret T)` wrap their inner expression.

Like `snc lex` / `snc ast` / `snc resolve`, it is a **dev/validation surface, not
`abi-v1`** — freely amendable, pinned by a golden test. The exact punctuation (`:`
marker, the dispatch/widen node names) is settled at (4a) from whatever emits
cleanest from Sentinel; the *principle* (every node typed; dispatch split; coercions
explicit) is the contract. Unlike the resolve VarIds, the structural type rendering
carries **no interner-ID order obligation** (D-context fact 3).

### D3. How types obtains resolved input (opens (4a) — PROBE-GATED).

`types.sentinel` needs (a) the parsed/resolved AST to walk, and (b) the symbol
tables (struct fields, enum variants, class/impl methods, effect ops, fn sigs) +
the `VarId`/`FnId`/`::`-splits resolve computed. Three candidate structures, **a
probe settles which before the (4b) consumer build** (ADR 0040 A1 discipline):

- **(a) reuse resolve's machinery via a D.6 module [LEAN].** `resolve.sentinel`
  `pub`-exposes its `RCtx` symbol-table bundle + the pass-1 table builders + the
  lookups (`fn_lookup` / `struct_lookup` / `sc_lookup` / `enum_lookup` / …) + the
  `::`-disambiguation; `types.sentinel` `use`s them (and the parser, transitively)
  and writes its **own pass-2 walk** that resolves names *and* infers types in one
  go, reusing the tables. Pass 1 is **identical** between resolve and types (the same
  tables), so sharing it removes the largest duplication; pass 2 diverges (resolve
  dumps IDs, types dumps IDs+types+dispatch) and is re-written. This is "share the
  machinery, not the output."

- **(b) re-resolve inline over the parser AST.** `types.sentinel` `use`s only the
  *parser* and re-implements resolve's table-building + binding + `::`-splits as part
  of its own walk. Maximum duplication but zero coupling to resolve's internals
  (robust if resolve's helpers prove too dump-entangled to share).

- **(c) resolve returns a resolved-AST value.** Refactor `resolve.sentinel` to build
  a parallel `RExpr` value tree (as the parser was refactored to return `Expr`);
  `types.sentinel` consumes it. Cleanest layering but most expensive — Move-only
  enums make a build-then-reconsume tree costly (the very reason resolve dumps
  inline), and it forces a large resolve rewrite.

**Lean: (a).** The probe question: *can `resolve.sentinel`'s `RCtx` + table-builders
+ lookups be `pub`-exposed and driven by a second module's walk, leak-free, without
the `Vec`-field-move-out SIGTRAP* (ADR 0040 A4)? If the helpers are too entangled
with resolve's emit, fall back to (b).

### D4. The `Type` representation in Sentinel (THE central problem — PROBE-GATED).

The Rust `Type` is a `Copy + Hash` 15-variant enum with five interner side-tables.
Sentinel can't have a `Copy` recursive enum (D-context fact 2). **Lean: a `Type` is
an integer "type-handle"** — the direct extension of resolve's proven flat-`Vec`
model:

- **Scalars are fixed reserved codes** (e.g. `0=i64, 1=i32, 2=bool, 3=u8`) — handles
  below a `BASE`. **Decl types** carry their ID in a small range (or are interned
  like compounds). **Compound types** (`Array`/`Vec`/`Nullable`/`Ref`/`Secret`/
  `GenericInstance`/`Kont`/`Task`/`TypeParam`/`Struct`/`Class`/`Enum`) are
  **interned** into flat parallel `Vec<i64>` arrays in a `TCtx` (extending `RCtx`):
  a `tk` (kind-tag) array + payload arrays (`ta` = inner-handle / elem-handle /
  struct-id; `tb` = a second field — `Ref`'s mutable-flag, `GenericInstance`'s
  arg-list head, `Kont`'s ret-handle). A handle ≥ `BASE` indexes
  `tk[h-BASE]`/`ta[h-BASE]`/`tb[h-BASE]`. Generic-instance arg lists (variadic) are
  a flat `(start,end)` slice into a side `Vec<i64>` of arg-handles (the resolve
  op-list idiom), since `Vec<Vec>` is unsupported.

- **Hash-consing makes equality = integer compare.** `intern_type(kind, a, b)` scans
  the existing interner for a structural match and returns its handle, else appends —
  so two structurally-equal types get the *same* handle and `unify_one`'s `p == a`
  fast path is a plain `==`. (The corpus is small; the linear scan is irrelevant to
  correctness, as with resolve's tables.) Alternative if hash-consing fights Sentinel:
  structural compare by walking both handles — settled by the probe.

- **`type_display` → a recursive `render_type(out, handle, tctx)`** that walks the
  interner by handle (scalars → literal; `Ref` → `&`/`&mut` + inner; `Secret` →
  `secret ` + inner; `Array` → `[` + inner + `]`; `GenericInstance` → name + `<args>`;
  …) — a direct port of lib.rs:896. **No interner ID appears** (D-context fact 3).

- **The env is a flat `Vec<i64>`** of type-handles indexed by VarId (VarIds are
  global integers from resolve) — O(1) `env[varid]`, no scope cloning (types' env
  never snapshots; it's monotonic per fn, like resolve's scope but keyed by VarId).

- **`unify_one` / `try_substitute` / `contains_type_param`** become recursive
  walks over handles building new handles (substitution interns the substituted
  type) — direct ports, leak-free because handles are `i64` (nothing to drop).

**The probe question** (before the (4c) compound-type slice): *represent types as
integer handles into a flat hash-consed `TCtx` interner; intern + compare + render +
substitute a nested type (e.g. `secret [?i64]`, `Vec<Point<i64>>`); store handles in
a VarId-indexed env; all leak-free.* If hash-consing or the variadic generic-arg
slice fights the borrow/Move rules, fall to structural-compare / an alternate arg
encoding (record as an amendment, the ADR-0040-D5 pattern).

### D5. Inference scope — bottom-up synthesis + bidirectional check.

The corpus is largely **annotation-driven + monomorphic** (most fns declare
param/return types; inference is mostly synthesize-bottom-up + check-against-expected
via `coerce_to_expected`). The genuinely-inferred part is **generic-fn calls**
(`check_call` runs `unify_one` to bind `TypeParam`s from arg types) + a few builtins
(`vec_new` / `push` infer their element from context). **Port the synthesis +
bidirectional check first** (the bulk — every scalar/compound expression form), then
the generic-call unification as a dedicated later slice (4h). The `coerce_to_expected`
widening-insertion logic (the `WidenToNullable`/`WidenToSecret` synthesis) ports with
the slice that introduces each widened type (nullable 4c, secret 4d).

### D6. The type-resolved disambiguation + coercions.

Mirror the Rust splits exactly: **method dispatch** (`check_method_call_expr` 7261 —
class method vs default-impl method by receiver type → `MethodCall` /
`ImplMethodCall`); **field/variant index computation** (`field_index` from the struct
decl, `variant_index` from the enum decl); **struct-field reordering** (source →
decl order); **widening insertion** (`coerce_to_expected`). The `Binary`→`Cmp`/`Logic`
categorisation is **already** in the resolve dump (the parser tags op-codes by
category) — types only adds the `:bool` on `Cmp`/`Logic` and the operand types.

### D7. Out of scope (happy-path first, as with lexer/parser/resolve).

Type **error/diagnostic parity** (the **57** `TypeError` variants, lib.rs:2496–3290)
— happy-path typed-AST production first; the Sentinel stage is run only where the
oracle type-checks cleanly (parse-error / resolve-error / **type-error** fixtures are
skipped, exactly as the resolve corpus test skips the oracle's failures).
**Effect-check** (the separate `sentinel-effect-check` crate — D1). **Cross-module**
(`use`-bearing fixtures — resolve already rejects them today, so naturally excluded).
**Full generic-inference edge cases** (ambiguous-null retry loops, deep nested
`TypeParam` conflicts) — ported only as the corpus demands. HIR/MIR/codegen (own
ADRs); performance.

### D8. Sub-slicing (types is ~1.8× resolve — staged, each oracle-validated).

| Slice | Scope                                                                    |
|-------|--------------------------------------------------------------------------|
| (4a)  | `snc types` oracle (`run_types` + `types_dump.rs`) + goldens + the       |
|       | **D3/D4 probes** (A1) + the parse→resolve→type→dump skeleton + the        |
|       | scalar `Type` codes + the simplest fixtures (arithmetic / comparison /    |
|       | `let` with annotations / var refs → fully typed). A seed diff.            |
| (4b)  | the **scalar/primitive expression grammar** fully typed — `if` / block /  |
|       | unary / logic / non-generic `call` (arg-type checks) — over the flat      |
|       | `VarTypeEnv` (VarId→handle).                                              |
| (4c)  | **compound types** — the `TCtx` interner (the D4 model): structs          |
|       | (struct-lit field-reorder + field-access `field_index`), arrays (lit +    |
|       | index), `Vec`, nullable (`?T` + null + `WidenToNullable`).                |
| (4d)  | **secret typing** — `WidenToSecret`, the operator-secret-preserving       |
|       | rules (C3.1b), `declassify`.                                              |
| (4e)  | **enums + match** — variant-construction typing (`variant_index`),        |
|       | match-arm payload binding types + exhaustiveness (reuses D.1).            |
| (4f)  | **classes / traits / impls** — the **method-dispatch split** (`MethodCall`|
|       | vs `ImplMethodCall`), `ClassInit`, `self` typing, `QualifiedCall` check.  |
| (4g)  | **effects / handlers** — `Perform` op-return typing, `Handle` + `Kont`    |
|       | interning, `ResumeKont`.                                                  |
| (4h)  | **generics** — generic-fn calls (`unify_one` bidirectional inference),    |
|       | generic structs (`GenericInstance` interning), the `type_args` on calls.  |
| (4i)  | converge to the **full clean-typing corpus**; a corpus differential       |
|       | test, the phase-go (D9).                                                  |

(Slices may merge/split as built — resolve's 3a–3e grew amendments A2–A12; types is
larger, so expect more.)

### D9. Phase-go.

`selfhost/types.sentinel` (compiled by `snc`) emits a canonical typed dump
**byte-identical** to `snc types` for every clean-typing fixture in `tests/pass` +
`tests/ui` (a differential test mirroring
`sentinel_resolver_matches_oracle_on_corpus`), leak-clean under `leaks --atExit`.

## Reasoning

**Why a fresh `snc types` dump (not reuse `snc resolve`).** Types' whole job is
attaching an inferred `Type` to every node + resolving dispatch + making coercions
explicit; the dump must *show* those, which `snc resolve` (by design) does not. A
regular type-annotated extension of the resolve form is both the validation target
and trivially reproducible from Sentinel — the call ADR 0038/0039/0040 each made.

**Why integer type-handles (D4), not a recursive `Type` enum.** A Move-only `Type`
enum hits resolve's exact cons-list dead-ends (can't `match` a `&enum`; reused
`&`-ref `match` aliases the heap → double-free; partial-consume leaks — ADR 0040
A1), and `Type` must be stored in an env, compared in `unify`, and substituted in
generic instantiation — all of which need cheap copies a Move enum can't give.
Integer handles into a flat hash-consed interner are `Copy` (an `i64`), give
equality-as-`==`, render structurally, and reuse resolve's proven flat-`Vec`
machinery. **And the dump never shows interner IDs** (`type_display` is structural),
so unlike resolve's VarIds the handle *assignment order* carries no obligation — the
representation's one rigid risk (ID-order fidelity) is absent here.

**Why share resolve's machinery (D3 lean (a)).** Pass 1 (the symbol tables) is
identical between resolve and types; re-implementing it inline (b) doubles the
largest, most-tested chunk of resolve. Sharing it via a D.6 module dogfoods modules
(as resolve `use`d the parser) and keeps the tables single-sourced. The risk —
resolve's helpers being dump-entangled — is exactly what a probe is for.

**Why flag both D3 and D4 as probes.** They are the two places types relies on
capabilities the Sentinel surface doesn't obviously give (cross-module machinery
reuse; a `Copy`, comparable, renderable, substitutable type value). Discovering a
dead end *after* building the ~1,100-line expression walker would be the most
expensive possible failure — the same probe-first discipline that de-risked the
parser's `Vec<non-primitive>` (ADR 0039 A3) and overturned resolve's cons-list tables
*before* the build (ADR 0040 A1).

## Consequences

### Positive
- The fourth compiler stage in Sentinel, oracle-validated — the port crosses from
  *name binding* (resolve) into *inference* (the semantic heart of a type system).
- A canonical `snc types` dump is a reusable tool + the substrate for the HIR/MIR
  stage's oracle later.
- Forces the first real **type representation + inference engine** in Sentinel (an
  interner, unification, substitution) — signal for HIR/MIR/codegen, which all carry
  typed values.

### Negative
- The largest, hardest stage: ~1.8× resolve, with the `Type` interner, unification,
  method dispatch, secret propagation, and exhaustiveness all to port. Expect the
  most sub-slices + amendments of any stage so far.
- The `coerce_to_expected` / `unify_one` bidirectional logic is subtle; a divergence
  from the Rust order of synthesis-vs-check produces a different typed tree (a
  mismatch), even where both would type-check.

### Neutral
- The Rust `snc` stays the production compiler + oracle throughout (ADR 0031 D6).
  `snc types` adds a dev surface, not ABI. Effect-check stays a separate stage,
  un-ported here (its own future ADR if/when the port reaches it).

## Revisit

PROPOSED until (4a) lands (oracle + the D3/D4 probes), then
ACCEPTED-WITH-AMENDMENTS as slices close. Triggers:
- **D4 `Type` representation**: if integer-handles + hash-consing fight the
  borrow/Move rules, adopt structural-compare or an alternate compound encoding; if
  no flat representation works, surface the language gap (a possible `Copy`-enum /
  persistent-structure need) — the ADR-0040-D5 escalation pattern.
- **D3 machinery-sharing**: if reusing `resolve.sentinel`'s helpers across a module
  boundary fights Move/borrow, fall to inline re-resolve (b) or the RExpr-value
  refactor (c); record as an amendment.
- **D2 dump format**: refine the type-annotation punctuation / dispatch-node names if
  awkward to emit from Sentinel (a dev contract, freely amendable).
- **D8 slice boundaries**: merge/split as the build reveals (resolve did, A2–A12).

Date: 2026-06-03
Related:
  - **0040** (self-host port (3/N) resolve): the immediately preceding stage —
    establishes the **flat parallel-`Vec` symbol tables + packed-name blob +
    integer-indexed lookups + the `&mut RCtx` bundle + GROUP-ORDER resolution**
    this stage extends (the `TCtx` type interner is the same flat-`Vec` idiom; the
    `VarId`→handle env reuses the scope machinery). Types consumes resolve's output;
    D3 decides how.
  - **0039** (self-host port (2/N) parser): the recursive-AST-by-value +
    consuming-dump + cons-list + `(*r)[i]` deref-index + `&mut i64` cursor idioms,
    and the corpus differential-test shape.
  - **0038** (self-host port kickoff + lexer): the differential-oracle method + the
    `selfhost/` tree + the A2 Sentinel-language workarounds.
  - **0031** (Phase D kickoff): D5 stage order lexer → parser → resolve → **types**
    → HIR/MIR → codegen.
  - **0016** (witness-table generics): `unify_one` / `try_substitute` /
    `GenericInstance` — the generic-inference core ported at (4h).
  - **0019** (secret typing) / **0014** (nullable) / **0033** (strings/`u8`): the
    `WidenToSecret` / `WidenToNullable` coercions + the secret-preserving operator
    rules ported at (4c)/(4d).
  - **0032** (sum types + `match`): enum-variant typing + match exhaustiveness
    ported at (4e).
  - **0023** (traits + impls) / **0022** (classes): the method-dispatch split
    (`MethodCall` / `ImplMethodCall` / `QualifiedCall`) ported at (4f).
