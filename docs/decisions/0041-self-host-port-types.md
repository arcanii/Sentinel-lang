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
  `types.sentinel` skeleton (the scalar grammar) on the A1-confirmed `TyCtx` interner.
- **A3 — (4b) milestone-1: `selfhost/types.sentinel`, the FOURTH Sentinel stage (the
  scalar skeleton).** The first Sentinel-side increment. Type-checks paramful `fn`s
  over the **scalar grammar** — int/bool literals, var refs, unary, binop/cmp/logic,
  `if`, blocks, `let` (annotated + inferred), and calls (user fns + scalar builtins) —
  emitting the `snc types` dump byte-for-byte. **Matches the oracle on 32 corpus
  fixtures** (every pure-scalar one) **+ 18 seeds** (`sentinel_typer_matches_oracle_on_seeds`),
  leak-free; compiled FIRST TRY.
  - **⚠ D3 REFINED → self-contained (NOT module-reuse).** The ADR's lean (a)
    (`types.sentinel` `use`s `resolve.sentinel`'s machinery) was reconsidered at the
    build and **dropped in favour of self-contained** (D3 option (b), inline): the
    types stage needs a type interner + a VarId→type env, and **resolve's `RCtx`
    cannot be extended with type fields across a module boundary** (Sentinel has no
    struct-spread / inheritance), so reuse would force **two-context threading**
    (`&mut RCtx` for names + `&mut TCtx` for types) through the whole deep walk —
    clunky and error-prone. Self-contained gives ONE clean `TyCtx` (resolve's
    name-blob scope + fn table + the type interner + the env). The duplication
    (resolve's pass-1/lookup idioms, re-expressed) is mechanical and resolve is frozen
    (ACCEPTED), so the maintenance risk is low. The Revisit's D3 trigger explicitly
    sanctions this (“if reusing resolve's helpers … fights … fall to inline
    re-resolve (b)”) — the fight here is the struct-extension limit, not Move/borrow.
    `types.sentinel` `use`s only the **parser** (the AST enums + `tokenize` /
    `parse_block` / `parse_type` / `parse_params` / `skip_*` / `append_*`).
  - **The `TyCtx` bundle + the verified D4 interner.** One `struct TyCtx` of `Vec<i64>`
    fields: the hash-consed type interner (`tk`/`ta`/`tb` — A1's exact verified
    idiom, scalars 0–3 below `BASE=100`, compounds interned); the **append-only**
    VarId→type env (`env`, pushed in VarId order by `bind_src`/`bind_name` — never
    index-assigned); resolve's name-blob scope (`nb`/`scs`/`sce`/`scv`/`base`); the
    fn table (`ufs`/`ufe` name slices + **`ufret`** return-type handles). `render_type`
    walks handles to the structural text; `type_of_typeexpr` interns a parsed `TypeE`
    → handle (all compound arms implemented, only scalars exercised at m-1).
  - **The key structural delta from resolve: `dump_texpr` RETURNS the node's type
    handle** (resolve's `dump_r*` returned nothing) and `close_ty` appends ` :<type>`
    before the node's `)`, so a parent uses its children's types (a binop's type =
    its operands'; cmp/logic → bool; a block → its tail; a call → the callee's
    return). The `let` shows its inferred type (resolve's `_`), bound DURING-WALK
    (value first → its type → the let's VarId/type).
  - **Forward-ref-safe calls:** pass 1 (the brace-depth fn scan) also **pre-scans each
    fn's return-type handle** into `ufret` (`scan_fn_ret` skips the header to the
    `-> ret`), so a call to a later-defined fn types correctly. Next: **(4c)**
    compound types — the interner's compound arms exercised (struct-lit + field-index,
    arrays + index, `Vec`, nullable + `WidenToNullable`) — then (4d) secret … (4i) the
    full-corpus phase-go.
- **A4 — (4c-1) structs + (4c-2) arrays: the first compound types.** Exercises the
  A1-verified interner's compound arms end to end. **Matches the oracle on 54 corpus
  fixtures** (40 after structs, 54 after arrays — up from 32), leak-free.
  - **The 3-pass restructure.** `main` now scans top-level NAMES first (pass 1: `fn` →
    `ufs`/`ufe`; `struct` → names copied into a **struct-name blob `snb`** + `sts`/`ste`;
    `itempos`/`itemkind`), THEN resolves signatures + field types (pass 1.5: fn returns
    `ufret`; struct fields → the field table `fldo`/`flds`/`flde`/`fldty`) — names-first
    so a field/return type referencing ANY struct resolves — THEN emits in source order
    (pass 2), dispatching via a single **`type_item`** helper (re-passing the `&mut`
    params — sibling-`if`-tail `&mut`-of-local borrows conflict, ADR 0038 A2). Structs
    consume no VarId, so VarIds still flow only through fns (source order; no group-order
    needed until classes/impls at 4f).
  - **The `Struct` type (interner kind 6).** Rendered via the struct-name blob `snb`
    (so `render_type` needs no `src` threading — the names live in `c`). `type_of_typeexpr`
    resolves a non-scalar `TIdent` to `mk_struct(StructId)`. **⚠ bug fixed in-flight:**
    the struct/field tables key on `snb`, but the lookups first reused `blob_eq` (which
    reads the SCOPE blob `nb`) → an `snb`-offset read into the small/empty `nb`
    out-of-bounded (`idx=0 len=0`); added a dedicated **`snb_eq`**. (Reusable rule: a
    blob comparator is hardwired to ONE blob — the scope blob and the decl-name blob
    need separate comparators.)
  - **struct-lit + field-access.** `(struct-lit #sid Name <v0> <v1>… :Name)` — positional
    VALUES in **declaration order** (the corpus writes fields in decl order, so source
    order = decl order; a name-keyed reorder is a follow-up if a fixture needs it).
    `(field <target :Struct> name <field_index> :fty)` — the field's decl index + type,
    looked up in the field table.
  - **(4c-2) arrays:** `(array <e…> :[T])` (T from the first element; empty-array
    context-typing is a follow-up) + `(index <t :[T]> <i :i64> :T)`. **+ the generic-
    builtin `(targs T)` mechanism:** `len`/`unwrap_or`/`is_some` are generic over their
    element/inner; since `(targs T)` PRECEDES the args in the output but T is inferred
    FROM the args, the args dump to a temp buffer first, then `(targs T)` + the spliced
    args (`unwrap_or`'s result type IS its inferred T). Next: **(4c-3)** nullable
    (`?T` + null + `WidenToNullable` — needs expected-type threading) then (4d) secret.
- **A5 — (4c-3) nullable ATTEMPTED + REVERTED; a Sentinel leak gotcha found (the
  LOGIC works, the code shape leaks).** The expected-type-threading logic was
  correct — `null`'s type taken from the expectation, `(widen-null …)` inserted where
  a `T` value meets a `?T` expectation (let value / struct field / fn return /
  if-branch), the cmp-operand sharing its type so `x == null` infers null — and it
  **matched the oracle on 61 corpus fixtures** (+7: every c15 nullable incl.
  `maybe_compose`'s if-branch-under-`?T` threading, `eq_null`'s cmp-null, and
  `linked_list_node`'s recursive `?Node`). **But it LEAKED** (208 bytes — the consumed
  `If` Expr AST tree not dropped). **⚠ ROOT CAUSE (a reusable Sentinel finding): a
  SEPARATE recursive dump fn with an EXTRA param leaks the Move-enum it consumes.**
  The attempt added a `dump_exp(out, ex, exp, src, c, gb)` (6 params, threading the
  expected type) ALONGSIDE `dump_texpr` (5 params); `dump_texpr`'s recursion is
  leak-free, but `dump_exp`'s recursion leaked the `Expr` tree it destructured
  (confirmed — the leaked blocks form the If→Block→Null tree; a non-nullable `if`
  routed through `dump_texpr` is clean). Workarounds (unconditional single-consume; a
  top-level `if exp_nullable { match ex } else { dump_texpr(ex) }` delegation) fixed
  the scalar leaf-temp leak but not the nullable-`if` recursion. **The fix (deferred
  to the next session): thread `exp` THROUGH `dump_texpr` ITSELF** (add the param to
  the proven-leak-free walker + use it in the Null/If/Block/Binary-right arms + a
  widen at the let/field/return boundaries) — NOT a separate recursive fn. (Or first
  probe why a 6-param recursive fn leaks its consumed enum where the 5-param one does
  not — a possible Sentinel drop-glue/param-count bug worth a minimal repro.) Reverted
  to the clean (4c-2) state; (4c-1)/(4c-2) stay committed. The WIP is preserved
  out-of-tree for the next attempt.
  - **Update (a 2nd attempt, also reverted; the simple hypotheses were RED HERRINGS).**
    Restructuring `dump_exp` to a straight `match ex` (no both-arms move; the `_` arm
    single-consumes a temp) was byte-correct but **still leaked**. **Five minimal
    probes ruled out every quick cause** — a 6-param recursive consuming-dump is clean
    (NOT the param count), a `match`-with-temp-leaf is clean, an If-cond-via-a-separate-fn
    is clean, and **mutual recursion is clean**. The leak's stack is `main → type_item →
    parse_block`: the parsed body `Expr` tree is not freed when routed through
    `dump_exp` (but IS through `dump_texpr` — 4c-2 clean). So it's a Sentinel drop-glue
    bug specific to the **full file's larger mutual-recursion group**, not minimally
    reproducible. **Next-session options:** (a) thread `exp` through `dump_texpr` ITSELF
    (one self-recursive walker, no new mutually-recursive fn) — the widen-before-emit
    then needs **per-arm** handling on the widen-able leaves (you can't wrap after
    emit); or (b) grow the probe toward the real file (add the ctx params, more
    mutual-recursion edges) until it leaks, isolate the trigger, and **escalate as a
    Sentinel compiler bug**.
- **A6 — (4c-3) nullable LANDED via path (a); the A5 leak is RESOLVED.** Took
  option (a): **threaded the expected-type `exp` through `dump_texpr` ITSELF** (a 6th
  param, `-1` = no expectation) and **deleted the separate `dump_exp`** — so the whole
  walk is ONE self-recursive fn (the proven-leak-free 4c-2 shape), never a second
  mutually-recursive consumer. **Matches `snc types` byte-for-byte on 61 corpus
  fixtures** (up from 54 — the +7 are every clean nullable fixture: `c15_null_literal`
  / `c15_widen` / `c15_eq_null` / `c15_nullable_struct_field` / `c15_go_no_go` /
  `c15_maybe_compose` / `c16_linked_list_node`) **+ 30 seeds** (+6 nullable), **leak-free
  across all 61** (a full `leaks --atExit` sweep), **zero regressions** vs the 4c-2
  baseline (a build-both-and-diff-the-match-sets check). Four-check green (1422 tests).
  - **The widen mechanism (the "can't wrap after emit" constraint).** A `(widen-null
    … :?T)` wrapper PRECEDES its inner node, but a node's natural type is sometimes
    known only AFTER its children synthesize — so there are two emit shapes, both
    keyed on `want_widen(exp, nt)` (true iff `exp` is `?T` and `nt` is its inner):
    **(i) inline** for leaves whose type is known up front (`Int`=i64, `Bool`=bool,
    `Var`=its env type, `SynthZero`=i64): `widen_pre` emits the prefix, the node emits,
    `widen_post` appends ` :?T)` and returns the widened type; **(ii) temp-splice** for
    `Binary` (result type = the operands'): build the binop into a temp `Vec<u8>`, then
    `widen_splice` prepends the wrapper iff needed. `null` is neither — its type simply
    IS `exp`. `If`/`Block` THREAD `exp` (to both branches / the tail); every other child
    recursion passes `-1`.
  - **The threading points** (where a non-`-1` `exp` originates): the **fn return** →
    body `Block` tail (`type_fn`); the **`let` annotation** (`tyopt_exp` — taken BEFORE
    the value is walked, replacing `type_of_tyopt`, so the value sees the `?T`
    expectation; the let's type is the annotation if present else the inferred value
    type); each **struct field** (its declared type, looked up by name in `dump_sfields`
    — which now takes the `StructId`); the **`==`/`!=` right operand** (inherits the
    left operand's type, so `x == null` types the `null` as `x`'s `?T`, ADR 0014 D7).
  - **⚠ The reusable Sentinel finding (supersedes A5's open question).** A recursive
    consuming-dump that THREADS an extra `&mut`-carried param is leak-free **as the file's
    single self-recursive walker** but LEAKS its consumed Move-enum **as a second
    fn mutually-recursive with the first** (the A5 `dump_exp` ↔ `dump_texpr` pair) — a
    Sentinel drop-glue bug that resisted minimisation (A5's five probes). **The robust
    idiom: never add a parallel recursive Expr-consumer; extend the existing walker's
    signature instead.** (The escalation in A5 option (b) is unnecessary now that (a)
    works, but the bug itself remains real — worth a dev note if a future stage is
    forced into a second mutually-recursive consumer.) Call-arg widening is NOT
    exercised by the corpus (every nullable call passes a `?T` var, never a bare `T`
    literal) and is left unimplemented; add it (needs the callee's param types) only if
    a fixture demands it. **Next: (4d) secret** (`WidenToSecret` + the secret-preserving
    operator rules + `declassify`) — reuses this exact widen-threading machinery for the
    `T → secret T` coercion.
- **A7 — (4d) secret typing LANDED; the widen machinery generalised + the
  secret-preserving operators + `declassify`.** **Matches `snc types` byte-for-byte on
  66 corpus fixtures** (up from 61 — the +5 are every clean secret fixture:
  `c31_secret_typing` / `c31_go_no_go` / `c52_secret_ct` / `c53_ct_eq` /
  `c52_secret_leak` — the last exercising secret `&&`), **+ 35 seeds** (+5 secret),
  **leak-free across all 66**, **zero regressions** vs the 4c-3 baseline. As foreseen,
  it reused the 4c-3 expected-type-threading wholesale — the `secret T` annotation
  already rendered (kind 3, like `?T` rendered pre-4c-3), and `WidenToSecret` is the
  same coercion shape as `WidenToNullable`.
  - **The widen generalised over BOTH coercions.** `want_widen`/`exp_nullable`
    collapsed into one **`widen_kind(exp, nt) → 0|1|2`** (0 none / 1 `widen-null`,
    `?T` kind 4 / 2 `widen-secret`, `secret T` kind 3 — both store the inner `T` in
    payload A). `widen_pre` emits the matching prefix; **`widen_post`/`widen_splice`
    were untouched** — `close_ty` renders the ` :?T` / ` :secret T` suffix from `exp`
    itself, so the suffix needs no kind. So every threading point (`let` / field / fn
    return / `if`-branch / cmp-operand) now does secret widening for free; in the
    corpus secret only widens at `let` values (`let pw: secret i64 = 42`).
  - **The secret-preserving operators (D5b).** A key simplification fell out of ADR
    0019's rule that **mixed public+secret operands are a `Type::Mismatch`** (rejected
    upstream → oracle-skipped): in a clean fixture both operands share secrecy, so
    **arithmetic needs NO change** — `resty = lt` already carries the secret (`secret
    i64 + secret i64` → `lt` = `secret i64`). Only **cmp/logic** changed: `secret
    operand → secret bool` (`mk_secret(2)` when `is_secret(lt)`, else `bool`). So
    `secret == secret : secret bool`, `secret && secret : secret bool`. New helpers
    `is_secret` + `strip_secret`.
  - **`declassify` (D6).** A new `Declassify` arm: `(declassify <inner :secret T> :T)`
    — synthesize the inner with no expectation, the node's type = `strip_secret(inner)`.
    In the corpus `declassify`'s result never lands in a widen position, so it is
    direct-emit (no wrapper). **The C3.1b CT-rejections** (`SecretBranch`,
    `SecretInRefDeref`, `SecretDivisor`) are type ERRORS → oracle-rejected → out of
    scope (D7), exactly as planned. **Next: (4e) enum/match** (variant-construction
    typing + match-arm payload binding + the `variant_index`).
- **A8 — (4e) enums + match LANDED.** The first decl kind whose *bodies* bind VarIds
  (match-arm payloads) and the first new interner kind since `Struct`. **Matches `snc
  types` byte-for-byte on 67 corpus fixtures** (up from 66 — the +1 is `c5d1_enum`, the
  D.1 phase-go: unit + tuple-payload variants, constructed + `match`ed) **+ 40 seeds**
  (+5 enum/match incl. the wildcard `_` and a `bool`-payload variant), **leak-free
  across all 67** (the `Match`/`Arms`/`Pattern`/`Binds` Move-enums all consumed
  cleanly), **zero regressions** vs the 4d baseline.
  - **The enum + variant tables** (extending the struct-table idiom). `ets`/`ete` hold
    enum names in the shared decl-name blob `snb` (EnumId = index); a **flat variant
    table** `varo` (owner EnumId) / `varns`/`varne` (variant name in `snb`) /
    `varps`/`varpe` (a slice into `varpay`, a flat `Vec<i64>` of payload-type handles).
    Built in the existing **pass-1** (enum NAMES, token kind 53 — alongside `fn`/`struct`)
    + **pass-1.5** (`scan_enum_variants` → `scan_payloads`, interning payload types now
    that all names are registered, so a recursive `?Enum` payload resolves). The **Enum
    interner kind 7** renders its name from `snb` (no `src` threading, exactly like
    `Struct` kind 6). `type_of_typeexpr` now resolves a non-scalar / non-struct `TIdent`
    to `mk_enum` (scalar → struct → enum → `i64` default).
  - **enum-construct** (the `Qcall` split, mirroring resolve). When the base name is an
    enum: `(enum-construct #eid <variant_index> Enum Variant <typed-args> :Enum)` — the
    `variant_index` (decl order) + the node's `:Enum` type are the type-stage additions;
    args dump with no expectation (the corpus never widens a payload arg). A non-enum
    `::` (a named-impl qcall, 4f) emits a **non-leaking placeholder** (`(qcall-impl …)`
    with the args consumed via `dump_targs`) so the match stays exhaustive without
    leaking the unhandled `Qcall`'s payloads — unreachable by the matching corpus.
  - **match.** `(match <scrut :Enum> #eid (arm (pat <vidx> V (bind #vid name ty)…)
    <body :T>)… :T)`: the scrutinee's type → its `EnumId` (`enum_of_handle`, emitted as
    `#eid`); resolve's uniform `(pat Enum V #vid…)` becomes `(pat <variant_index> V
    (bind #vid name <payload-type>)…)` — the enum name is dropped for the index, and
    each payload binding now carries its **declared payload type** (from `varpay` via
    `variant_flat`). Arm payloads are **scoped per-arm** (`truncate_scope` pops the name
    blob back after each body; VarIds stay monotonic, the env append-only — the resolve
    3c idiom). The match's expectation `exp` threads to each arm **body** (so a
    `?T`/`secret` match widens at the bodies); the match's type is the shared arm-body
    type. ⚠ The bind name is emitted from `nb` *after* `bind_name` copies it there
    (`bind_name` consumes the AST `[u8]`, so it can't also be `append_str`'d).
    **Next: (4f) classes/traits/impls** — the method-dispatch split (`MethodCall` vs
    `ImplMethodCall`), `ClassInit`, `self` typing, the `QualifiedCall` check.
- **A9 — (4f) classes + traits + impls LANDED (the CORE; delegate synthesis deferred).**
  The largest single slice — the **receiver-typed method-dispatch split** + a
  **group-order VarId restructure**. **Matches `snc types` byte-for-byte on 84 corpus
  fixtures** (up from 67 — **+17**: `c41` ×2, `c42` ×2, `c4_named_impl`, and — as a
  BONUS — the entire ref/borrow set `c20`–`c22` ×11 + `c25_go_no_go`, unlocked by the
  ref-typing fix below) **+ 44 seeds** (+4 class/trait/impl/ref), **leak-free across all
  84**, **zero regressions**.
  - **The group-order restructure (the architectural change).** The Rust resolver
    assigns ALL fn-body VarIds, THEN class-body, THEN impl-body (lib.rs:2559), but the
    dump is source order. So pass-2 now types each item into a shared `itembuf` in
    GROUP order — A (fns/structs/enums/traits) → B (classes) → C (impls) — tagged with
    its source index (`rsrc`/`rbs`/`rbe`), then splices source-order. Each region's
    VarId base is the prior region's final `nextvid` (the resolve idiom, no count
    precomputed). **Behavior-preserving on the prior corpus** (groups B/C empty there →
    the 67 stayed byte-identical, verified before adding any class code).
  - **Classes.** The class + class-field + class-method tables (names in the shared
    `snb`; **Class interner kind 8**, rendered like Struct/Enum). A bucketed decl
    emitter (`type_class` → fields / init / methods buffers, mirroring resolve) with
    **typed bodies** — `self` bound (synthetic for init, the receiver token for
    methods) typed as the class; the method body's expectation is its return type.
    `type_of_typeexpr` now resolves a `TIdent` through scalar→struct→enum→**class**.
    `Name::init(args)` → `(class-init #cid Name <args> :Class)`. Class field access
    extends the `Field` arm (`strip_ref` then struct-or-class).
  - **Traits + impls.** The trait (+ trait-method-return-type) + impl (→ trait-id /
    class-id, resolved via `src`-vs-`snb` slice lookups in pass-1.5) tables. `type_trait`
    emits the sig heads (no VarIds — `emit_trait_params`); `type_impl` emits the impl
    head + **typed method bodies** (reusing `tmethod`, `self` typed as the impl's class).
  - **The dispatch split (the headline).** `target.m(args)` → the receiver's class (via
    `class_of_handle(strip_ref(ty))`): its OWN method → `(method #cid <idx> <target> m
    <args> :ret)`; else a default-impl method (`impl_for_class` finds the impl of that
    class whose trait has `m`) → `(impl-method #iid <idx> <target> m <args> :ret)`. The
    `#cid`/`#iid` precede the target, so the target builds in a temp first (the `Binary`
    idiom). Named-impl qualified calls `Impl::m(args)` (the 4e `Qcall` placeholder's else
    branch) → `(qcall-impl #iid <idx> Impl m <args> :ret)`.
  - **Ref typing (the bonus).** `&`/`&mut`/`*` unary now build/strip ref types
    (`mk_ref(0/1, inner)` / `strip_ref`) instead of passing the operand type through —
    which is all the `c20`/`c21`/`c22` ref+borrow fixtures + `c25` needed to type-clean
    (a +12 windfall beyond the class fixtures). `&mut c : &mut Counter` also makes the
    qcall-impl arg type right.
  - **⚠ Deferred: delegate-impl synthesis (`c43_go_no_go`).** A `class C { delegate f: T
    to Tr; }` makes the compiler SYNTHESISE a forwarding `impl _ as Tr for C` whose every
    method is `fn m(self, p…) { self.f.m(p…) }` (ADR 0040 A12). This needs: the delegate
    field added to the class field table; a delegate record; a **group D** that
    synthesises the forwarding impls AFTER user impls (continuing the impl VarId region,
    recorded at the class's source index); and a **typed forwarding body** — a
    method-dispatch on `self.f` (typed as the field type, itself splitting method vs
    impl-method). It is ~150 lines of SYNTHESISED emission (no source to diff-guide), so
    it is a self-contained **(4f-delegate)** follow-up. The current build SKIPS delegates
    and degrades `c43` gracefully (a leak-free mismatch). **Next: (4f-delegate)** then
    **(4g) effects/handlers**, (4h) generics, (4i) the full-corpus phase-go.
- **A10 — (4f-delegate) forwarding-impl synthesis LANDED; (4f) is COMPLETE.** Matches
  `snc types` on **85 corpus fixtures** (+1 `c43_go_no_go`, the delegation phase-go) **+
  45 seeds** (+1 delegate), **leak-free across all 85**, **zero regressions**. The
  delegate (`delegate f: T to Tr;`) now: **(1)** adds a class FIELD in pass-1.5 (so
  `self.f` resolves in the init + synth bodies) + records the delegate (owner class,
  field name/index/type, trait); **(2)** registers the synth impls in the impl table
  BEFORE pass-2 (ImplId continues after user impls — an unnamed `impl _` → a 0,0 name
  slot — so the `c.m(…)` dispatch in a fn body finds the synth impl via
  `impl_for_class`); **(3)** emits the delegate field in the class decl (a `dfbuf`
  bucket, after explicit fields); **(4) group D** (after group C) synthesises each
  forwarding impl into `itembuf`, **recorded at the owning class's source index** (so it
  splices right after the class), VarIds continuing the impl region. Each method
  (re-walked from the trait's sigs via the stored `trpos`) forwards to `self.f.m(params)`
  — a method-dispatch on the delegate field built DIRECTLY (the same own/impl split;
  `self`+params get fresh group-D VarIds; the forwarding args read their types from the
  env). ⚠ The method-name `[u8]` (built from a `src` slice for the dispatch lookups)
  must be `sink_name`d after the lookups (an owned `[u8]` merely indexed/borrowed leaks).
  **(4f) classes/traits/impls — including delegation — is COMPLETE. Next: (4g)
  effects/handlers** (`Perform` op-return typing, `Handle` + `Kont` interning), (4h)
  generics, (4i) the full-corpus phase-go.

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
