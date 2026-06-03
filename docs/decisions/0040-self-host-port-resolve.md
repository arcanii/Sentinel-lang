# ADR 0040: Phase D self-host port — (3/N) resolve-in-Sentinel

Status: **PROPOSED** — the third sub-phase of the self-host port (ADR 0031 D5 /
ADR 0038 D9), after the lexer (1/N) and the parser (2/N, ADR 0039 → ACCEPTED).
Ports the **resolve** stage to Sentinel: the parsed AST → a name-resolved program
(every identifier reference bound to an integer ID), differentially validated
against a Rust `snc` oracle over the `tests/pass` + `tests/ui` corpus. Flips to
ACCEPTED-WITH-AMENDMENTS as the slices land (the same cadence as ADR 0039). See
## Decision for the sub-slice plan, ## Reasoning for the data-model call, and the
key Sentinel-language risk (the scope snapshot/restore) flagged in D5.

⚠ **Amended PRE-BUILD by agent-driven probes (A1, 2026-06-03):** the proposed
**cons-list symbol tables + scopes (D4/D5) are UNUSABLE** in Sentinel and are
**corrected to flat parallel-`Vec` arrays** (the parser's token-array idiom). The
oracle (3a) has landed; the Sentinel side builds on the corrected model. See
## Amendments.

## Amendments

- **A1 — agent-driven de-risking probes (pre-build): D3 confirmed; D4 + D5
  CORRECTED to flat parallel-`Vec`.** Three parallel throwaway probes (per
  `docs/agent-protocol.md`) settled the open design questions before the build —
  and overturned the cons-list data-model the Decision proposed.
  - **D3 (parse-sharing): CONFIRMED.** A D.6 module can `pub enum Ast {…}`
    (self-referential, e.g. `Bin(Ast, Ast)`) + a `pub fn` returning it; another
    module `use`s both, calls it, and **recursively consuming-`match`es the AST by
    value across the module boundary**, leak-free. So `selfhost/resolve.sentinel`
    will `use parser::Expr` + a `pub` parse-returning-AST entry from a refactored
    `parser.sentinel` (no self-contained AST copy).
  - **D4 + D5 (symbol tables + scope): CORRECTED — flat parallel-`Vec`, NOT
    cons-lists.** Three hard Sentinel constraints make cons-list tables/scopes
    unusable: **(i)** you **cannot `match` on a `&enum`** (`match_scrutinee_not_enum`)
    → a read-only cons-list walk is impossible, every traversal must *consume*;
    **(ii)** **`match *t` on a `&`-ref recursive enum deep-copies the cell and the
    copy *aliases* the original's heap** → a *reused* table double-frees on the
    second lookup (SIGABRT); **(iii)** partial-consume (`truncate`) of cons cells
    built in a different stack frame **leaks** their boxes (consistent with the
    known D.1b box-free-only recursive-enum-drop limitation — bind-and-ignore /
    partial consume). **Corrected model (proven leak-clean over a 100-arm loop):**
    every symbol table AND the local scope is **flat parallel `Vec<i64>` arrays +
    a packed-name `[u8]` blob** (or `src` slice `(start,end)` indices — zero copy),
    **integer-indexed**, with the live **count/length threaded as an explicit
    `i64`** (never `len` on a ref — `len` rejects a `&Vec`). This is exactly
    `parser.sentinel`'s token-array idiom. **Scope snapshot/restore =
    length-truncation:** O(1) snapshot (record the count), O(added) restore (`pop`
    the arm's bindings off the parallel `Vec`s) — D5 mechanism (a) on `Vec`s. The
    (3a) resolve mini-pipeline (flat fn-table + flat scope + a parallel `RExpr`
    enum + consuming dump) was prototyped end-to-end and emits the exact expected
    dump leak-free.
  - **Reusable disposal rule (bites every later stage):** an owned `[u8]` is freed
    ONLY by pushing its bytes into a `Vec` reclaimed via `vec_to_array` (the
    `print_bytes` / kept-binding chain) — a consuming `match` does NOT free a
    `[u8]` payload, and `len` / indexing don't either. So resolve stores names as
    `src` slice indices, never an owned `[u8]` per binding. (`scope` is also a
    reserved keyword — don't name a binding `scope`.)
  - **(3a) ORACLE landed** (`eba2fb4`): `snc resolve` + `resolve_dump.rs`. The
    Sentinel side ((3a) skeleton, then 3b–3e) builds on the corrected flat-`Vec`
    model; the ADR flips to ACCEPTED-WITH-AMENDMENTS when (3a) lands.

## Context

The lexer (1/N) and parser (2/N) proved the **differential-oracle method** end to
end: a canonical `snc <stage>` dump the Sentinel stage reproduces byte-for-byte,
diffed over the corpus. Resolve is the next stage in the D5 order (lexer →
parser → **resolve** → types → …). What resolve does (Rust
`crates/sentinel-resolve/src/lib.rs`, ~6034 lines; entry `resolve(&Program) ->
Result<ResolvedProgram, ResolveError>` at lib.rs:1950):

1. **Build the top-level symbol tables** — one `HashMap<String, Id>` per item
   kind, IDs assigned in source order, with uniqueness / cross-kind-collision
   validation: effects (lib.rs:1966), structs (2055), classes (2095), traits
   (2112), enums (2186), impls (2236; default impls keyed by `(TraitId,
   ImplTarget)`, named by name). Type / enum / class / trait names share one
   namespace.
2. **Pre-register 14 runtime builtins** as `FnId(0..=13)` (`print` … `print_bytes`,
   lib.rs:2353–2530), then **collect user fns** as `FnId(14..)` with arity
   `FnSignature`s; require `main` exists (lib.rs:2532–2557).
3. **Resolve every body** (fns, then class / impl / delegate methods,
   lib.rs:2559–2827): walk each expression, binding **local variables** (params,
   `let`s, `match`-arm payloads, handler-arm params, synthetic `self`) to `VarId`s
   from a global monotonic `next_var_id` counter, tracked in a **flat per-fn**
   `vars: HashMap<String, VarId>`.

The output is a parallel **`ResolvedProgram`** (lib.rs:231) — `ResolvedExprKind`
(lib.rs:621) mirrors `ExprKind` but with names replaced by IDs **and the parser's
syntactic ambiguity resolved**: the parser emits a uniform `QualifiedCall` /
`ClassInit` for `Name::method(args)` / `Name::init(args)` / bare `Enum::Variant`,
and resolve splits them (lib.rs:3763–3899) into:

- **`Call { id: FnId }`** — a free-function call; *but* if the callee name is an
  in-scope `VarId`, it becomes **`ResumeKont { kont: VarId }`** (a
  continuation-resume `k(arg)`; locals shadow fns — lib.rs:3509–3574).
- **`EnumConstruct { enum_id }`** — when the leading name is in the enum table
  (checked **first**, lib.rs:3768 / 3825).
- **`ClassInit { id: ClassId }`** — when the leading name is a class.
- **`QualifiedCall { impl_id, method_index }`** — a named impl; `method_index` is
  the position of `method` in the impl's trait's method-name list (a linear
  search, lib.rs:3880).
- **`StructLit { id: StructId }`**, **`Perform { effect_id, op_index }`** —
  likewise ID-bound.

Two facts shape the port, exactly as ADR 0039's two facts shaped the parser:

1. **The Sentinel parser (2/N) DUMPS; it does not return an AST.** Resolve needs
   to *walk* an AST, so (3/N) opens by having the parse machinery **produce AST
   values a resolve pass consumes** (mirroring ADR 0039 D3, where the parser
   needed the lexer to *return* tokens). See D3.
2. **Resolve is HashMap-pervasive and clones scopes.** Every symbol table is a
   `HashMap<String, Id>`; child scopes snapshot via `vars.clone()` (match arms
   lib.rs:4021, handler arms 4147, `while` bodies 3276). Sentinel has **no
   hashmaps**, **`Vec<[u8]>` / `Vec<struct>` is unsupported** (ADR 0039 A3), and
   user-enum values are **Move-owned with no clone**. The symbol-table + scope
   representation is therefore the central design problem — see D4 + D5.

## Decision

### D1. Goal.

Port resolve to Sentinel as the third compiler stage, emitting a **canonical
resolved-program dump** byte-identical to a Rust `snc resolve` oracle over the
clean-resolving corpus. Like the parser, it is heavily sub-sliced (D8). Types and
later stages remain their own ADRs.

### D2. The oracle — a canonical resolved dump (`snc resolve <file>`).

Add `snc resolve <file>` (driver `run_resolve` + a `resolve_dump.rs` module,
mirroring `run_ast` + `ast_dump.rs`). It runs the Rust resolve and emits a
**regular, fully-tagged S-expression** dump of the `ResolvedProgram` — the
**`snc ast` form extended with the resolved IDs + the disambiguated node kinds**,
e.g.

```
(fn #14 add ((param #0 a i64) (param #1 b i64)) i64 (block (binop + (var #0) (var #1))))
(fn #15 main () i64 (block (call #14 (int 1) (int 2))))
```

Node kinds that change vs `snc ast`: `(var #N)` (VarId), `(call #N …)` (FnId),
`(resume-kont #N …)`, `(struct-lit #N Name …)` (StructId), `(class-init #N Name
…)` (ClassId), `(qcall-impl #I <method_index> ImplName method …)`,
`(enum-construct #E EnumName Variant …)`, `(perform #E <op_index> Eff op …)`; decl
heads gain their IDs (`(fn #N …)`, `(struct #N …)`, `(enum #N …)`, …). Pure
literals / operators / `if` / blocks / `match` arms are unchanged from `snc ast`
except for nested IDs. Like `snc lex` / `snc ast`, it is a **dev/validation
surface, not `abi-v1`** — freely amendable; pinned by a golden test. The numeric
IDs are the *Rust* assignment order (builtins 0–13, user fns 14+, others in
source order) — the Sentinel side must reproduce that order exactly (it is the
oracle's ground truth, just as kind/node *names* were for the lexer/parser).

### D3. Parse → AST → resolve (opens (3a)).

`selfhost/resolve.sentinel` reuses the parser's `parse_*` machinery to build the
recursive `Expr` / decl AST **in memory**, then walks it with new `resolve_*`
functions producing a resolved form, then dumps that. Concretely: keep the parser
tokenizer + `parse_program`-shape entry, but instead of `dump_item` walk the
parsed items with `resolve_*` + `dump_resolved_*`. Whether the parse machinery is
physically **shared via a D.6 module import** of `selfhost/parser.sentinel` or
**re-included** in `resolve.sentinel` is settled at (3a) by a probe (lean: a D.6
module import, dogfooding modules — the parser would expose its `Expr` enum + a
`parse_program`-returning-AST entry; fall back to a self-contained copy if the
module path fights the borrow/Move rules). Either way the AST node definitions are
the single source of truth.

### D4. Symbol tables in Sentinel — cons-lists + linear scan.

Each `HashMap<String, Id>` becomes a **cons-list of `(name: [u8], id: i64)`** with
a **linear-scan `lookup(table, name) -> i64`** (`-1` = absent) — the same
"correctness over speed; `Vec<non-primitive>` is unsupported so use a recursive
enum cons-list" trade the parser made for arg lists (ADR 0039 A3/A5). One table
per kind (fn / struct / enum / class / trait / effect / named-impl); the
default-impl `(TraitId, ImplTarget)` table is a cons-list of `(trait_id, target_tag,
target_id, impl_id)` scanned linearly. IDs are assigned by a per-kind counter
following the Rust order (builtins pre-seed the fn table at 0–13). Uniqueness /
collision checks are linear scans of the already-built tables (the corpus is
small; O(N²) is fine — and resolve, like the lexer/parser, targets **happy-path**
production first, D7).

### D5. Local scopes + the snapshot/restore RISK (the key probe).

The flat per-fn `vars` map becomes a **cons-list of `(name: [u8], var_id: i64)`**,
prepended as bindings appear; `next_var_id` is a `&mut i64` counter threaded
through the walk (the cursor pattern from ADR 0039). Lookups scan newest-first, so
inner bindings shadow outer — matching the Rust inner-scope-wins rule (D-context).

⚠ **The hard part is the snapshot/restore the Rust code does with `vars.clone()`**
for `match` / handler / `while` arm bodies (a binding scoped to one arm must not
leak past it). Sentinel enum values are **Move-owned with no built-in clone**, so
the port cannot hold a pre-arm copy and a grown copy simultaneously. Candidate
mechanisms, to be **settled by a probe before any scoped-body slice is built**
(the same probe-first discipline as ADR 0039 A3/A5/A11):

  - **(a) length-truncation:** record the scope list's *length* (an `i64`) before
    the arm; after, drop the prepended head cells back to that length (a small
    consuming helper). Works because bindings are prepended (newest = head).
  - **(b) value-in/value-out threading:** `resolve_block` takes the scope **by
    value** and **returns** the (possibly grown) scope; the caller discards the
    returned scope and keeps its own pre-call binding to restore. Needs the scope
    to be cheaply reconstructable.
  - **(c) explicit `copy_scope`:** a manual deep-copy of the cons-list (rebuild),
    used as the snapshot.

(a) is the lean choice (no copy; O(depth) restore). **The early slices avoid the
problem entirely:** a flat fn body with no `match` / `while` / handler sub-region
needs only an append-only scope (no restore), so the snapshot mechanism is
deferred to the slice that first resolves a scoped body (see D8), where the probe
runs first.

### D6. Disambiguation + `Call` resolution.

Mirror lib.rs:3509–3574 / 3763–3899 exactly: for a `Call`, scan `vars` first
(found → `ResumeKont`), else the fn table (→ `Call { FnId }`, with an arity check
against the signature list). For a parsed `QualifiedCall` / `ClassInit`: scan the
enum table **first** (→ `EnumConstruct`), else the class table (`ClassInit`) /
named-impl table (`QualifiedCall`, computing `method_index` by a linear scan of
the impl's trait's method names). The Sentinel `Expr` enum gains the resolved
variants (or a parallel `RExpr` enum is introduced — settled at (3b); lean: a
parallel `RExpr` so the parse AST stays the parser's, and resolve maps `Expr →
RExpr`).

### D7. Out of scope (happy-path first, as with lexer/parser).

Resolve **error/diagnostic parity** (the ~36 `ResolveError` variants,
lib.rs:900–1394) — happy-path resolved-AST production first; the Sentinel stage is
only run where the oracle resolves cleanly (parse-error / resolve-error fixtures
are skipped, exactly as the parser corpus test skips the oracle's failures).
**Cross-module resolve** (`use` / `resolve_imports` / `merge_modules`,
lib.rs:1416–1668) — single-file programs first (the Rust resolve itself rejects a
non-empty `uses` with `UseDeclNotYet` today, so `use`-bearing fixtures are
naturally excluded). Types and later stages (own ADRs); performance.

### D8. Sub-slicing (resolve is big — staged, each oracle-validated).

| Slice | Scope                                                                    |
|-------|--------------------------------------------------------------------------|
| (3a)  | `snc resolve` oracle + the parse→AST→resolve→dump skeleton + the fn      |
|       | table (builtins 0–13, user fns 14+) + a flat append-only `vars` scope    |
|       | (params + `let`) → `(var #N)` / `(call #N …)` for paramful fns + free    |
|       | calls + arithmetic. A seed diff.                                         |
| (3b)  | the rest of the **expression** disambiguation — `ResumeKont`,            |
|       | struct-lit / field, the `::` paths split into `EnumConstruct` /          |
|       | `ClassInit` / `QualifiedCall`, `perform`, array / index / postfix.       |
|       | (Needs the struct / enum / class / trait / impl / effect tables.)        |
| (3c)  | the **scoped bodies** — `match` (arm-pattern bindings), `while`, `handle`|
|       | (handler-arm params + the return arm) — the **snapshot/restore probe**   |
|       | (D5) runs first.                                                         |
| (3d)  | **decls** — `struct` / `enum` / `effect` / `trait` / `impl` / `class`    |
|       | (+ delegate synthesis) resolved heads with their IDs.                    |
| (3e)  | converge to the **full corpus** (the clean-resolving `tests/pass` +      |
|       | `tests/ui` set); a corpus differential test, the phase-go.               |

### D9. Phase-go.

`selfhost/resolve.sentinel` (compiled by `snc`) emits a canonical resolved dump
**byte-identical** to `snc resolve` for every clean-resolving fixture in
`tests/pass` + `tests/ui` (a differential test mirroring
`tests/selfhost_parse.rs`'s corpus test), leak-clean under `leaks --atExit`.

## Reasoning

**Why a fresh `snc resolve` dump (not reuse `snc ast`).** Resolve's whole job is
binding names to IDs + disambiguating the four `::` forms; the dump must *show*
those IDs and the resolved node kinds, which `snc ast` (by design) does not. A
regular ID-bearing extension of the `snc ast` form is both the validation target
and trivially reproducible from Sentinel — the same call ADR 0038/0039 made.

**Why cons-list symbol tables + linear scan.** It is the only representation
available (no hashmaps; `Vec<[u8]>` unsupported), it needs no new language
feature, and the corpus is small enough that linear scan is irrelevant to
correctness. It directly reuses the proven cons-list machinery from ADR 0039.

**Why flag the scope snapshot/restore as a probe.** It is the one place resolve
relies on a capability (cheap scope cloning) Sentinel lacks; discovering a dead
end *after* building the expression walker would be expensive. A small probe
(build a scope cons-list, grow it in a sub-region, restore it, check `leaks`)
de-risks the whole sub-phase — exactly the discipline that de-risked the parser's
`Vec<non-primitive>` and `&mut Ret` unknowns.

## Consequences

### Positive
- The third compiler stage in Sentinel, oracle-validated — the port crosses from
  *syntax* (lex/parse) into *semantics* (name binding).
- A canonical `snc resolve` dump is a reusable tool + the substrate for the types
  stage's oracle later.
- Forces the first real symbol-table + scope machinery in Sentinel — signal for
  the remaining stages (types, effects, codegen all carry environments).

### Negative
- Larger and harder than the parser: symbol tables, ID-order fidelity, the
  scope snapshot/restore risk, and the four-way disambiguation.
- The Rust→Sentinel ID-order contract is rigid (the dump pins exact integer IDs);
  any divergence in assignment order is a mismatch.

### Neutral
- The Rust `snc` stays the production compiler + oracle throughout (ADR 0031 D6).
  `snc resolve` adds a dev surface, not ABI.

## Revisit

PROPOSED until (3a) lands, then ACCEPTED-WITH-AMENDMENTS as slices close.
Triggers:
- **D3 parse-sharing**: if a D.6 module import of the parser fights Move/borrow
  rules, fall back to a self-contained re-include (record as an amendment).
- **D5 scope snapshot/restore**: if length-truncation (a) doesn't hold leak-/
  correctness-wise, adopt value-threading (b) or `copy_scope` (c); if none works,
  surface the language gap (a possible `clone`/persistent-structure need).
- **D2 dump format**: refine the canonical resolved S-expr if it proves awkward to
  emit from Sentinel (a dev contract, freely amendable).
- **D6 `RExpr` vs reusing `Expr`**: choose the parallel-enum vs extend-in-place
  representation at (3b) from whichever the consuming dump drops leak-free.

Date: 2026-06-03
Related:
  - **0039** (self-host port (2/N) parser): the immediately preceding stage —
    establishes the recursive-AST-by-value + consuming-dump + cons-list +
    `(*r)[i]` deref-index + `&mut i64` cursor idioms this stage reuses, and the
    corpus differential-test shape. Resolve consumes the AST the parser produces.
  - **0038** (self-host port kickoff + lexer): the differential-oracle method +
    the `selfhost/` tree + the A2 Sentinel-language workarounds (flat per-fn var
    namespace; deep-`if` tail-borrow; single-helper dispatch).
  - **0031** (Phase D kickoff): D5 stage order lexer → parser → **resolve** → … .
  - **0032** (sum types + `match`): the resolved AST is modelled as Sentinel
    recursive enums; `match`-arm binding scopes are the D5 snapshot/restore site.
  - **0009 §6**: the Rust resolve is a single-pass-structured multi-table walk;
    the Sentinel port mirrors it (cons-list tables + a threaded `next_var_id`).
