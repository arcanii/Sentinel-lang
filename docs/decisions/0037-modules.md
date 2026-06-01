# ADR 0037: Phase D.6 — modules / multi-file (file-as-module + separate compilation)

Status: PROPOSED — **D.6 (1/N) IN PROGRESS; multi-file now COMPILES + RUNS**
(via the lower-risk Path A merge — owner-chosen: whole-graph front-end + merge
into one `Program` → existing pipeline; true per-unit separate-compilation back
end deferred). Landed green: `use` front-end, module-graph discovery, top-level
`pub`, import resolution + visibility, and the merge (`merge_modules`) — a
cross-module `pub fn` call compiles + runs (exit 5), same-named privates across
modules coexist (exit 41, 0 leaks). FOLLOW-UPS: per-unit objects +
module-qualified `abi-v1` mangling + multi-object link (the true separate-comp
back end); cross-module types/generics; effect-check parity. See ## Implementation
notes. The sixth and **last** Phase D language prerequisite
under ADR 0031 (Phase D kickoff) D4 item 5, before the self-host port (D5). After
sum types (D.1), strings + a byte type (D.2), growable collections (D.3), file I/O
(D.4), and loops (D.5), the surface has been **single-file by design** since 1.0
(ADR 0025 D9) — but a compiler is many files. D.6 makes Sentinel multi-file. Flips
to ACCEPTED-WITH-AMENDMENTS when the MVP sub-phases land. The 4 OPEN DESIGN POINTS
are SETTLED (owner-confirmed): allow import cycles; AMEND `abi-v1` (not `abi-v2`);
source root = the entry file's directory; `use a::b::c` = item `c` in module `a::b`.

Date: 2026-06-01
Related:
  - **0031** (Phase D kickoff): D4 item 5 — "Modules / multi-file (ADR 0025 D9,
    deferred from 1.0) — `mod`/`use` + a resolve-layer module graph +
    separate-compilation units keyed to `abi-v1`. A compiler is many files." This
    ADR is that per-feature ADR; it refines the surface to **file-as-module** and
    commits to **true separate compilation** (vs. a whole-program multi-file merge).
  - **0025 D9** (modules deferral): modules were the biggest scope swing in C5 and
    were explicitly deferred post-1.0 (a single large file sufficed for the go/no-go).
    This ADR un-defers them.
  - **0029** (stable `abi-v1`): the mangling scheme + the ~23 `sentinel_*` runtime
    symbols are frozen + test-enforced. Cross-unit symbols become part of the ABI
    surface, so module-qualified mangling is an **`abi-v1` amendment** (D7) — the
    current bare-source-name scheme is single-file-only and not collision-free
    across modules.
  - **0016** (generics / monomorphization): `collect_mono_instantiations` is
    **whole-program** today; cross-module generics are the hardest piece of separate
    compilation (D6, deferred to (2/N)).
  - **0009 §6.1** ("name resolution, module graph"): the architecture always
    anticipated a module graph in resolve + a Salsa query engine — D.6 cashes it in.
  - **0022 / 0023** (classes / traits + impls): method + impl mangling
    (`<Class>__<method>`, `<Name>__<Type>__<Trait>__<method>`) gains a module prefix.

## Context

Of the ADR 0031 D4 prerequisites, modules is the **last** and the most
architectural. The 1.0 + D.1–D.5 language is **single-file**: `snc build
<one-file>` reads exactly one source, parses it to one `Program`, resolves all
top-level names into a **single flat namespace** (string-keyed `fn_table` /
`struct_table` / …), assigns **sequential global IDs** (`FnId`/`StructId`/… in
source order, builtins first), and lowers everything into **one LLVM module** →
**one object** → `cc`-linked with `libsentinel_runtime.a`. A self-hosted compiler —
a lexer, a parser, a resolver, a dozen codegen passes — is **many files**, so the
single-file assumption is the gate.

Two decisions were settled with the language owner before this ADR:

  1. **Module surface = file-as-module + `use`.** Each source file *is* a module;
     its path relative to the source root *is* its module path. `use a::b::Item;`
     imports a `pub` item across files. No explicit `mod { … }` blocks (the file is
     the module — the Go / Python shape, not Rust's in-file module tree). `pub`
     (already parsed, a no-op since C4.1) becomes the cross-module visibility gate.
  2. **Compilation model = true separate compilation** (not a whole-program
     multi-file merge). Each module compiles to **its own object file**,
     independently, with cross-module references resolved at **link time** via
     stable `abi-v1`-keyed symbols — the model ADR 0031 D4 names. This is the
     ambitious choice: it breaks three whole-program assumptions in codegen (below)
     and turns cross-unit symbols into ABI surface. It is also what makes a
     self-hosted compiler's build incremental and what keeps each unit's `.o`
     independently reproducible.

**Honest sizing (the ADR 0031 norm).** True separate compilation is a multi-stage
effort on the order of a small C-phase, not a single sub-phase. The genuinely hard
pieces — **cross-module generics** (whole-program `collect_mono_instantiations` must
become per-unit + link-time dedup) and the **module-qualified mangling ABI change**
— are sequenced explicitly (D9) rather than pretended to be free.

## Decision

### D1. Goal.

Make Sentinel **multi-file** with **file-as-module** semantics and **true separate
compilation**: `snc build <entry>` discovers the module graph by following `use`
edges from the entry file, compiles **each module to its own object independently**,
and links the units + the runtime into one binary. `pub` gates cross-module
visibility. Enough that the self-host port (D5) can be written as many `.sentinel`
files compiled + linked separately, each independently reproducible and (later)
incrementally cached.

### D2. Surface syntax — file-as-module + `use`.

```sentinel
// src/lex/token.sentinel  — module `lex::token`
pub enum Token { Ident, Int, Plus }          // exported
fn helper() -> i64 { 0 }                       // module-private (no `pub`)

// src/main.sentinel  — the entry module (contains `main`)
use lex::token::Token;                          // import a pub item

fn main() -> i64 {
    let t: Token = Token::Int;
    0
}
```

  - **A file is a module.** Its module path is its path **relative to the source
    root** with the extension dropped and separators as `::`: `src/lex/token.sentinel`
    → module `lex::token`. The **source root** is the directory of the entry file
    passed to `snc build` (e.g. `snc build src/main.sentinel` → root `src/`).
  - **`use <module-path>::<item>;`** brings one `pub` item into the current module's
    scope under its short name. A new **`use` keyword** (lexer token) at the top of a
    file. `::` (already lexed, used for `Type::method` / `Enum::Variant`) extends to
    module paths.
  - **`pub`** on a top-level item (`pub fn` / `pub struct` / `pub enum` / `pub trait`
    / `pub effect`) exports it; un-`pub` items are **module-private** (visible only
    within their own file). The C4.1 `pub` on class fields/methods is unchanged.
  - **MVP `use` granularity:** one item per `use` (no globs `*`, groups `{A, B}`, or
    `as` aliases — D8 deferred). A module path with no item (`use lex::token;` to name
    the module itself) is **deferred**; the MVP imports items, not module aliases.

### D3. Resolve — the module graph + per-unit namespaces.

  - **Discovery.** Starting from the entry file, resolve builds a **module graph**:
    parse the entry, collect its `use` edges, map each referenced module path to a
    file (`a::b` → `<root>/a/b.sentinel`), parse + recurse. The graph is the set of
    reachable modules. A missing file for a `use`d path is a focused diagnostic
    (`ModuleNotFound`). **Import cycles are allowed** (A `use`s B, B `use`s A) — true
    separate compilation resolves cross-unit references at link time, so cycles are
    not a layering problem (unlike a header-include model). Flagged as a design point
    (P1) in case we want to forbid them for simplicity in (1/N).
  - **Per-unit namespaces.** Each module resolves into **its own** name tables and
    **its own ID space** (`FnId`/`StructId`/… restart per module). A name lookup
    first checks the module's own items + its `use`-imported names, then builtins. A
    cross-module reference resolves to an **(ModuleId, item)** pair (an *external*
    reference), not a local ID — this is what lets a unit be type-checked + lowered
    without the defining unit's internals.
  - **Visibility.** A `use` of a non-`pub` item, or a reference to a non-`pub` item,
    is rejected (`PrivateItem`). `pub` is enforced here (the C4.1 parse becomes
    load-bearing).

### D4. Types / borrow / effect checks — per unit, against imported signatures.

Each module is type-checked **independently**, against the **signatures** (not
bodies) of its imported items. A `pub fn`'s signature, a `pub struct`'s field
layout, a `pub enum`'s variants, a `pub trait`'s methods cross the unit boundary as
**imported declarations**; their bodies do not. This mirrors how the checks already
consume `FnSignature` / `TypedStructDecl` tables — the tables are now assembled
**per unit** from (own items ∪ imported `pub` signatures) rather than from one flat
program. Types carry no symbol, only layout, so a `pub struct` from module A used in
B needs only A's *declaration* imported (B agrees on layout) — no link symbol.

### D5. Codegen — one object per module + external symbols.

  - **One LLVM module per unit.** Codegen runs **per module**, emitting that module's
    `pub` + private items into its own LLVM module → its own `.o`. The current
    single-`"sentinel"`-module path becomes the degenerate one-module case.
  - **Cross-module calls → external symbols.** A call to an imported `a::foo` lowers
    to a call to `foo`'s **module-qualified symbol** (D7), **declared external**
    (declaration only) in the calling unit; A's unit **defines + exports** it. The
    whole-program `fns: HashMap<FnId, FunctionValue>` becomes per-unit, plus an
    **extern-declaration path** for imported callees (the breaking change called out
    in the readiness scan).
  - **Linking.** `cc` links **all** unit `.o` files + `libsentinel_runtime.a`; the
    **entry module** owns `main` (the C entry). Link order is deterministic (sorted
    by module path) for reproducibility.

### D6. Cross-module generics (the hard piece) — `(2/N)`.

`collect_mono_instantiations` is **whole-program**: it walks the entire program to
discover which `(FnId, type_args)` generic instances are used, then emits each once.
Separate compilation breaks this — module B may instantiate `A::id<i64>` without A
knowing. The standard answer (the C++ template model): the **using** unit instantiates
the generic it needs and emits it with **`linkonce_odr`** linkage, so multiple units
that instantiate the same `A::id<i64>` **dedup at link time**. This needs: (a)
per-unit mono discovery (the imported generic's *body* is available for instantiation
— so a `pub` generic fn's body **does** cross the boundary, unlike a monomorphic fn),
and (b) `linkonce_odr` + a mangling that makes `A::id<i64>` identical across units.
This is the single largest mechanic and is **deferred to (2/N)** so (1/N) can land
non-generic separate compilation first.

### D7. `abi-v1` amendment — module-qualified, collision-free mangling.

Today a free fn is its **bare source name** (`fn double` → symbol `double`) — fine
single-file, but **not collision-free across modules** (two modules' `foo`) and not
length-prefixed (ADR 0029's noted soft-spot: `a::b` vs `a_b` could collide). Separate
compilation makes every `pub` symbol cross-unit, so the mangling must encode the
module path **unambiguously**. Proposed: a **length-prefixed, module-qualified**
scheme, e.g. `_S` + per-segment `<len><segment>` for the module path + the item
(Itanium-ish), so `lex::token::Token::new` and `parse::Token::new` never collide.
`main` stays `main` (the C entry); `sentinel_*` runtime symbols are unchanged. This
is a **frozen-ABI change** (ADR 0029 D8): the mangling golden-string tests + the
spec doc (`docs/abi-v1.md`) update **in the same commit**. Whether this is an
`abi-v1` **amendment** or an `abi-v2` bump is a design point (P2) — leaning amendment,
since no existing single-file program's *observable* ABI changes (a single-file
program is one module; its bare names can be preserved as the empty-module-path case).

### D8. Out of scope (deferred).

`use` globs (`use a::*`) / groups (`use a::{X, Y}`) / aliases (`use a::X as Y`);
module-alias imports (`use a::b;` naming the module); re-exports (`pub use`); nested
in-file `mod { }` blocks (file-as-module only); a package/manifest system + external
dependencies (the root is just a directory); conditional compilation; visibility
finer than `pub` / private (no `pub(crate)`); and **incremental caching** of unchanged
units (D10 — a Salsa-backed follow-on, not the correctness MVP).

### D9. Pipeline / sub-phase split.

| Sub        | Title                                                            | Risk |
|------------|------------------------------------------------------------------|------|
| D.6 (1/N)  | File-as-module surface (`use` token + parse, top-level `pub`) +  | high |
|            | the resolve module graph + per-unit namespaces + visibility +    |      |
|            | per-unit type-check against imported signatures + **non-generic** |      |
|            | separate compilation (per-unit `.o`, module-qualified mangling,   |      |
|            | extern-symbol cross-module calls + types, deterministic link).    |      |
| D.6 (2/N)  | **Cross-module generics** (per-unit instantiation + `linkonce_odr` | high |
|            | dedup; `pub` generic bodies cross the boundary) + cross-module    |      |
|            | trait/impl methods + visibility edge cases.                      |      |
| D.6 (3/N)  | **Incremental caching** of unchanged units (Salsa-backed) + the   | med  |
|            | per-unit `.o` reproducibility discipline (extend `repro.rs`).     |      |

### D10. Phase-go + fixtures.

A multi-file phase-go: an entry `main.sentinel` that `use`s a `pub` item (a fn + a
type) from a second file in a subdirectory (`util/math.sentinel` → module
`util::math`), compiled to **two objects** + linked, returning a computed exit code;
verified the two `.o`s are emitted independently and the link resolves the
cross-module symbol. Plus UI fixtures for the new rejections (`ModuleNotFound`,
`PrivateItem` — a `use` of a non-`pub` item). (2/N) adds a cross-module generic
fixture; (3/N) adds a "touch one unit, only it recompiles" check.

## Reasoning

**Why file-as-module (not `mod` blocks).** A compiler is naturally one-file-per-pass;
file-as-module needs no module-tree ceremony and maps cleanly to the OS filesystem
(the discovery the build already does). It is the smaller, more orthogonal surface —
`use` + `pub` + a path→file rule — and it is what the self-host port wants.

**Why true separate compilation (not whole-program merge).** A whole-program merge
(parse all files, concatenate into one `Program`, resolve together) would be far
smaller and is a tempting MVP — but it is a dead end for the two things modules exist
to give a real compiler: **independent, reproducible units** and **incremental
rebuilds**. Committing to separate compilation now means the ID model, the mangling,
and the codegen-per-unit boundary are right from the start, rather than retrofitted.
The cost is honestly large (D6 + D7), which is why the hard pieces are sub-phased.

**Why the mangling change is unavoidable.** Cross-unit linking *is* symbol
resolution; the current bare-source-name scheme cannot name two modules' `foo`
distinctly. Module-qualified, length-prefixed mangling is the minimum that makes
separate compilation sound — and it must be frozen + tested like the rest of
`abi-v1`.

## Consequences

### Positive
- Sentinel becomes multi-file — the **last** ADR 0031 D4 prerequisite — unblocking
  the self-host port (D5): the compiler can be written as many `.sentinel` units.
- Independently reproducible, separately compiled units (the `repro.rs` discipline
  extends per-unit); incremental rebuilds become possible (D.6 3/N).
- `pub` finally does something; the `::` path syntax + the Salsa query engine
  (ADR 0009 §6.1) are cashed in.

### Negative
- The biggest architectural change since the core pipeline: per-unit ID spaces, a
  resolve module graph, codegen-per-unit + extern symbols, a frozen-ABI mangling
  change, and (2/N) cross-unit generic instantiation with `linkonce_odr`. High risk;
  multi-sub-phase.
- More moving parts in the build (N objects + a link graph) and in `repro` (per-unit
  byte-identity + deterministic link order).

### Neutral
- No new runtime `sentinel_*` symbols (modules are a front-end + linking concern, not
  a runtime one). The mangling change touches the ABI doc + tests but adds no
  runtime surface.

## Implementation notes — D.6 (1/N) core (the worked-out design)

D.6 (1/N) is being built in green increments. **Landed:** the `use` front-end +
top-level `pub` (parser) + the driver's `discover_module_graph` (follows `use`
edges → files; ModuleNotFound; cycles allowed). **Remaining = the per-unit core,**
designed below after reading the resolve/codegen internals. It is a **cohesive
change** (resolve + types + codegen + link together) — `use` does not compile until
all four land — so it is one focused effort, not a stack of independently-green
steps. Sequencing within it:

1. **Expose the graph + a global pub-signature pass.** `discover_module_graph`
   returns the parsed `Program` per module. A pre-pass collects each module's
   `pub` item **signatures** + their module-qualified symbols into an exports
   table keyed by `(module_path, item_name)`. (Cheap — signatures, not bodies.)

2. **Per-module resolve against imports — the extern-fn-in-FnId-space model.** Add
   `resolve_module(program, imports) -> ResolvedProgram` (`resolve()` becomes the
   `imports == []` case, preserving single-file). The least-cascade representation:
   a cross-module call stays an ordinary `ResolvedExprKind::Call { id: FnId }`. Each
   imported fn is registered in **this module's** FnId space (after builtins) with
   its signature from the exports table, but marked **extern** (a
   `link_symbol: String` + `origin: Vec<String>` on the resolved fn / signature; no
   body). The module's own fns follow. So `fn_signatures[id]` covers builtins +
   imported externs + own fns; a call to an imported name resolves to its extern
   FnId — **no new expr variant, no expr-cascade**. Visibility (`pub`, now parsed)
   is enforced here: importing a non-`pub` item → `PrivateItem`; a `use` of an
   absent item → `UnknownImport`.

3. **Per-module type-check** runs unchanged against the per-module
   `fn_signatures` (externs included) — extern fns are just signatures, so
   cross-module calls type-check normally.

4. **Per-module codegen + module-qualified mangling (the `abi-v1` D7 amendment).**
   Codegen's `fns: HashMap<FnId, FunctionValue>` works **uniformly**: an extern
   FnId becomes a *declaration* (external linkage, the module-qualified
   `link_symbol`) instead of a definition; a local `pub` fn is *defined* under its
   module-qualified symbol so other units' externs resolve at link. `main` stays
   `main`; private fns keep local linkage. Calls via FnId are unchanged. The
   mangling is **length-prefixed + module-qualified** (Itanium-ish:
   `_S<len><seg>…<len><item>`) so `a::b::f` and `a_b::f` can't collide — frozen
   per `abi-v1`, so the `abi_v1_mangling_is_stable` golden test +
   `docs/abi-v1.md` update **in the same commit**.

5. **Driver orchestration + link.** Drive per-module resolve→types→codegen (one
   `.o` each), then `cc` all `.o` + `libsentinel_runtime.a` (entry module owns
   `main`; deterministic, path-sorted link order for repro). Replace the current
   multi-module gate with this pipeline.

**Scope the first vertical slice to cross-module FN calls** (a `pub fn` in module
B called from module A → two `.o` + link → runs). Cross-module **types**
(`struct`/`enum` imports — layout-only, no symbol) and cross-module
**trait/impl/effect** follow within (1/N); **generics** are (2/N) (`linkonce_odr`,
D6); incremental per-unit caching is (3/N). The whole-program
`collect_mono_instantiations` only needs touching once generics cross units (2/N),
so (1/N) can keep it per-entry-module for the non-generic slice.

## Revisit

PROPOSED until D.6 (1/N) lands, then ACCEPTED-WITH-AMENDMENTS as sub-phases close.
Triggers:
- **D6 generics**: if `linkonce_odr` per-unit instantiation proves too costly
  (code bloat / link time), consider a whole-program mono pass that still emits
  per-unit objects (a hybrid), or an explicit-instantiation surface.
- **D7 mangling**: if preserving single-file bare names as the empty-module case is
  awkward, bump to `abi-v2` instead of amending `abi-v1`.
- **D8**: `use` ergonomics (globs / groups / aliases / re-exports) once a self-host
  unit finds single-item `use` too noisy.

## OPEN DESIGN POINTS (to settle at D.6 (1/N))

1. **Import cycles.** Allow (separate compilation resolves them at link) vs. forbid
   for (1/N) simplicity (a topological module order). → leaning **allow**.
2. **`abi-v1` amendment vs. `abi-v2`.** Module-qualified mangling as an amendment
   (preserve single-file bare names as the empty-module case) vs. a clean `abi-v2`
   bump. → leaning **amendment**.
3. **Source root.** The entry file's directory (`snc build src/main.sentinel` → root
   `src/`) vs. an explicit `--root` / a project manifest. → leaning **entry-file
   directory** for the MVP, manifest deferred (D8).
4. **`use` path resolution.** `use a::b::c` = item `c` in module `a::b` (file
   `a/b.sentinel`). Confirm the last-segment-is-the-item rule (vs. allowing
   module-path imports). → leaning **last segment is the item**.
