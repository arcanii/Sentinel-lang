# ADR 0037: Phase D.6 — modules / multi-file (file-as-module + separate compilation)

Status: PROPOSED — **D.6 (1/N) NON-GENERIC per-unit separate compilation is FUNCTIONALLY
COMPLETE + WORKING** (`snc build --separate`): module-qualified D7 mangling + the per-unit
ID model (D5.1) + the codegen-per-unit boundary (D5.2) are PINNED **and REALISED** — each
module compiles to its OWN object independently; cross-module **fn calls** resolve at LINK
time via the module-qualified `abi-v1` symbol, and cross-module **types** (`pub struct` /
`pub enum`, incl. in fn signatures) are LAYOUT-imported (D4 — re-materialized per unit, no
link symbol). Green phase-gos: cross-module fn (`add` → exit 42), struct (`Point` local use
→ 42), enum (`Shape` w/ payloads, matched → 42), and the type-in-signature case
(`sum(p: Point) -> i64`, a struct crossing by value → 42); + `ModuleNotFound`/`PrivateItem`
rejections. Built ADDITIVELY (opt-in `--separate`; the Path A merge + both bootstrap fixed
points untouched). **(2/N) is now FUNCTIONALLY COMPLETE — EVERY pub item kind crosses a
`--separate` boundary:** generics (INLINED + monomorphized locally per importer), trait/impl
(inlined trait, importer-local impl), effect decls (inlined), and **cross-UNIT effect
perform/handle** (the last hard case — a library performs, the entry handles; `b43b7d6`+`4c3a28b`):
the OVERLOADED `EffectId` (op-id basis AND `effect_decls[]` index) is DECOUPLED by a build-wide
**op-id base map** (effect NAME → graph-stable index) consulted in codegen, with an empty-map
delegation to `encode_op_id` so single-file / merge / corpus stay BYTE-IDENTICAL (oracle untouched);
effecting externs carry `effect_row_names` re-resolved in `check_module`. ▶ REMAINING: (2/N)
`linkonce_odr` dedup (an OPTIMIZATION — share generic/method instances across importers; needs the
module-qualified type-tag fix below); (3/N) incremental caching. The multi-file
SURFACE shipped earlier via the interim Path A merge. The surface shipped via
the lower-risk Path A merge — owner-chosen: whole-graph front-end + merge
into one `Program` → existing pipeline; true per-unit separate-compilation back
end deferred to the (1/N)/(2/N)/(3/N) sub-phases below (D9). Landed green: `use` front-end, module-graph discovery, top-level
`pub`, import resolution + visibility, the merge (`merge_modules`), and
**cross-module items of every kind** — `merge_modules` qualifies fn + struct +
enum + trait + effect + class + named-impl names by module path and rewrites
every reference (call callees, type annotations / signatures, struct literals,
enum construction/patterns, `impl as Trait for Type` heads, `perform`/`handle`
effect names, effect rows, delegate trait names) via a per-module `Renamer`.
Verified: a cross-module `pub fn` (exit 5), same-named privates coexist
(exit 41, 0 leaks), a cross-module `pub struct` (exit 42), a cross-module `pub
enum` + `match` (exit 52), same-named structs coexist (exit 42), a 3-deep
cross-module struct field (exit 42), a cross-module `pub trait` impl'd +
dispatched (exit 42), same-named classes coexist (exit 42), a cross-module `pub
effect` performed + handled through the handler runtime (exit 42), and
cross-module GENERICS (a `Box<i64>` struct, an `id<T>` fn, `Pair`/`make_pair`/`fst`
— instantiated in a different module than defined, exit 42; they work for free
because Path A's whole-program mono runs over the merged graph), and effect-check
parity (the merged path now rejects a multi-file `main` with an unhandled effect).
FOLLOW-UPS: per-unit objects + module-qualified `abi-v1` mangling + multi-object
link (the true separate-comp back end, incl. per-unit `linkonce_odr` generics);
span-accurate multi-source diagnostics. See ## Implementation notes. The sixth and **last** Phase D language prerequisite
under ADR 0031 (Phase D kickoff) D4 item 5, before the self-host port (D5). After
sum types (D.1), strings + a byte type (D.2), growable collections (D.3), file I/O
(D.4), and loops (D.5), the surface has been **single-file by design** since 1.0
(ADR 0025 D9) — but a compiler is many files. D.6 makes Sentinel multi-file. Flips
to ACCEPTED-WITH-AMENDMENTS when the MVP sub-phases land. The 4 OPEN DESIGN POINTS
are SETTLED (owner-confirmed): allow import cycles; AMEND `abi-v1` (not `abi-v2`);
source root = the entry file's directory; `use a::b::c` = item `c` in module `a::b`.

**▶ THE PER-UNIT BACK END (this revision).** Path A gave Sentinel the multi-file
*surface* + semantics but compiles the whole graph as ONE object (no per-unit `.o`,
no incremental rebuilds) — it is NOT separate compilation. This revision settles the
true back end the ADR always committed to (D1/D5): each module → its own `.o`,
cross-module refs resolved at LINK time via module-qualified `abi-v1` symbols, and
`linkonce_odr` cross-unit generic dedup. The frozen-ABI pieces are PINNED here before
any code: **D7** (the exact module-qualified mangling), the **per-unit ID model**
(the extern-fn-in-FnId-space model, D5.1), and the **codegen-per-unit boundary** (the
3 whole-program assumptions to break, D5.2). It is built **additively**: the Path A
merge + `snc merge` (the self-host fixed-point path) + both bootstrap fixed points
stay green throughout; the per-unit path lands behind an opt-in until it reaches Path A
parity, then becomes the default for `snc build` (D9). See **SETTLED DESIGN POINTS**.

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
    single-`"sentinel"`-module path becomes the degenerate one-module case. Each unit's
    object is emitted **independently** (inkwell `TargetMachine::write_to_file` per
    `Module`) — **NOT** `llvm-link`-merged into one IR module before object emission:
    that IR-merge would be the Path A whole-program model wearing a modular front-end
    (no independent reproducibility, no incremental rebuild). Cross-unit references are
    LLVM **external** symbols resolved by the **native linker**, which is what keeps the
    units independently reproducible + (3/N) incrementally rebuildable.
  - **Cross-module calls → external symbols.** A call to an imported `a::foo` lowers
    to a call to `foo`'s **module-qualified symbol** (D7), **declared external**
    (declaration only) in the calling unit; A's unit **defines + exports** it. The
    whole-program `fns: HashMap<FnId, FunctionValue>` becomes per-unit, plus an
    **extern-declaration path** for imported callees (the breaking change called out
    in the readiness scan).
  - **Linking.** `cc` links **all** unit `.o` files + `libsentinel_runtime.a`; the
    **entry module** owns `main` (the C entry). Link order is deterministic (sorted
    by module path) for reproducibility.

### D5.1. The per-unit ID model — extern-fn-in-FnId-space (PINNED).

Each module resolves into its **own** ID space (`FnId`/`StructId`/… restart per
module: builtins `FnId(0..=13)` shared + fixed, then imported externs, then own items
— D3). The representation that costs the **least cascade** through types/codegen
(settled in the impl notes, elevated here): a cross-module call stays an ordinary
`Call { id: FnId }` — **no new expr variant**. An imported `pub fn` is registered in
*this* module's `FnId` space (after builtins) as an **extern**: its `FnSignature`
carries the imported signature (param/return types from the exports table) **plus its
module-qualified `link_symbol`** (D7) and **no body**. So `fn_signatures` per unit =
builtins ∪ imported externs ∪ own fns; a call to an imported name resolves to its
extern `FnId` and type-checks against the imported signature like any other call.
Imported `pub` **types** (struct/enum) are registered analogously in this unit's
`StructId`/`EnumId` space with their **layout** from the exports table (a type carries
*no* link symbol — units agree on layout, D4). A new `resolve_module(program, imports)`
generalizes `resolve()` (the `imports == []` case = today's single-file resolve,
preserved bit-for-bit, so the single-file/Path-A paths are byte-unchanged).

### D5.2. The codegen-per-unit boundary — the 3 whole-program assumptions to break.

`compile_to_object` is whole-program today; the readiness scan named three assumptions
that must become per-unit. Each is pinned here:

  1. **`collect_mono_instantiations` (whole-program generic discovery).** Walks every
     non-generic fn body in the *whole* program to find `(FnId, type_args)` instances.
     → Becomes **per-unit**: each unit discovers the instances *it* uses (including of
     *imported* generics — so a `pub` generic fn's **body** crosses the boundary, unlike
     a monomorphic fn, D6) and emits each with **`linkonce_odr`** linkage so units that
     share an instance dedup at link. (2/N — 1/N is non-generic, so 1/N keeps the pass
     per-entry-module and emits nothing cross-unit from it.)
  2. **The single `fns: HashMap<FnId, FunctionValue>`.** One map over one global FnId
     space, every entry a *definition*, every free fn named by its **bare source name**
     (`add_function(&signature.name, …)`). → Becomes **per-unit**, built from this
     unit's `fn_signatures`: a **local** fn is *defined* under its module-qualified
     symbol (D7); an **extern** fn (D5.1) is *declared* (external linkage, its
     `link_symbol`) — a declaration, not a definition. `main` keeps local linkage + the
     bare `main` symbol.
  3. **`self.fns.get(&id)` call resolution.** → **Mechanically unchanged** — the FnId
     still keys the map. The map simply returns a *declaration* for an imported callee,
     so the emitted `call` references the external module-qualified symbol and the
     linker binds it to the defining unit's definition. This is what makes the
     extern-fn-in-FnId-space model (D5.1) low-cascade: the call path never learns about
     modules at all.

The driver drives **per module**: `resolve_module → check → effect/borrow/ct → hir →
compile_to_object` (one `.o` each, the module's path threaded in for mangling), then
`cc` all `.o` + `libsentinel_runtime.a` in **path-sorted** order (deterministic; entry
module owns `main`). `compile_to_object(hir, output)` gains the module path (empty =
single-file = today's bare-name path, unchanged).

### D6. Cross-module generics (the hard piece) — `(2/N)`.

`collect_mono_instantiations` is **whole-program**: it walks the entire program to
discover which `(FnId, type_args)` generic instances are used, then emits each once.
Separate compilation breaks this — module B may instantiate `A::id<i64>` without A
knowing. The standard answer (the C++ template model): the **using** unit instantiates
the generic it needs and emits it with **`linkonce_odr`** linkage, so multiple units
that instantiate the same `A::id<i64>` **dedup at link time**. This needs: (a)
per-unit mono discovery (the imported generic's *body* is available for instantiation
— so a `pub` generic fn's body **does** cross the boundary, unlike a monomorphic fn),
and (b) `linkonce_odr` + a mangling that makes `A::id<i64>` identical across units, and
(c) **module-qualified type tags** in the mono key (D7's last bullet) so
`id<a::b::Point>` and `id<c::d::Point>` are not `linkonce_odr`-deduped as one.
This is the single largest mechanic and is **deferred to (2/N)** so (1/N) can land
non-generic separate compilation first.

### D7. `abi-v1` amendment — module-qualified, collision-free mangling (PINNED).

Cross-unit linking *is* symbol resolution, so every cross-module symbol must encode
its module path **unambiguously**. The frozen scheme (settled — open point 2 = AMEND,
not `abi-v2`):

  - **The unit of mangling is `(module_path, item)`** where `module_path` is the
    module's path segments (`[util, math]` for `util/math.sentinel`; **empty** for a
    single-file program) and `item` is the item's **existing intra-module mangled
    name** — the C5 `abi-v1` §4 symbol, unchanged: a free fn is its source name
    (`double`), a mono instance is `mangle_mono_name` (`id__i64`), a class init/method
    is `Point__init` / `Counter__inc`, an impl method is `add__Point__Display__show`.
  - **Empty module path → the bare `item`, byte-for-byte.** A single-file program is
    the empty-module-path case, so **its every symbol is exactly what `abi-v1` emits
    today** — this is why this is an *amendment*, not an `abi-v2` bump: no existing
    single-file artifact's observable ABI changes. (`abi_v1_mangling_is_stable` stays
    green unchanged; a NEW multi-module golden test pins the rest.)
  - **Non-empty module path → `_S` + a length-prefixed segment per module path
    segment + a length-prefixed `item`**, each segment `<decimal-byte-len><bytes>`
    (Itanium-ish source-name encoding). `item` is wrapped as **one** length-prefixed
    blob (its internal `__` structure is the existing §4 scheme, untouched — the
    amendment fixes the *module* collision surface, not the intra-module one). Examples:
    `util::math` fn `add` → `_S4util4math3add`; `lex::token` class-method `Token::new`
    (item `Token__new`) → `_S3lex5token10Token__new`; `parse` class-method `Token::new`
    → `_S5parse10Token__new` (distinct module prefix — the two never collide, the D7
    requirement). The encoding is a prefix-free code over the segment sequence
    `[m1, …, mk, item]`, so distinct `(module, item)` pairs always yield distinct
    symbols; decoding is unambiguous because no Sentinel identifier (nor any §4 type
    tag) starts with a digit, so the greedy length read terminates exactly at the
    segment bytes. Convention (for `llvm-nm` readability, not parsing): the **last**
    length-prefixed segment is the item, the rest are the module path.
  - **Exceptions (unchanged):** the entry module's `main` stays `main` (the one C entry
    point); the `sentinel_*` runtime symbols (§5) and the inlined builtins are never
    mangled. `_S` is **reserved** for Sentinel-emitted symbols (like `sentinel_*`) —
    documented in `abi-v1.md` so user code does not rely on a `_S*` name surviving.
  - **Cross-module type tags (a 2/N soundness note).** `mangle_type` (§4) renders a
    `Struct`/`Class`/`Enum` by its **bare** name today. That is sound for 1/N
    (non-generic: no type appears in a mono key) but **unsound once generics cross
    units**: `id<a::b::Point>` and `id<c::d::Point>` would both mangle `id__Point` and
    `linkonce_odr` would wrongly dedup them. So **2/N must module-qualify the type tag**
    of a cross-module `Struct`/`Class`/`Enum` in a mono key (e.g. its defining module
    path, length-prefixed) — tracked as a 2/N decision, not a 1/N concern.

This is a **frozen-ABI change** (ADR 0029 D8): the mangling golden tests + the spec
doc (`docs/abi-v1.md` §4 gains the module-qualified scheme, and §8 drops the
"separate-compilation linker" + "length-prefixed mangling" out-of-scope bullets)
update **in the same commit** as the codegen mangling change.

### D8. Out of scope (deferred).

`use` globs (`use a::*`) / groups (`use a::{X, Y}`) / aliases (`use a::X as Y`);
module-alias imports (`use a::b;` naming the module); re-exports (`pub use`); nested
in-file `mod { }` blocks (file-as-module only); a package/manifest system + external
dependencies (the root is just a directory); conditional compilation; visibility
finer than `pub` / private (no `pub(crate)`); and **incremental caching** of unchanged
units (D10 — a Salsa-backed follow-on, not the correctness MVP).

### D9. Pipeline / sub-phase split.

The multi-file **surface** (the `use` token + parse, top-level `pub`, the resolve
module graph + discovery, visibility) **already shipped via the interim Path A merge**.
The sub-phases below are the **per-unit back end** — true separate compilation — built
**additively** on top of that surface:

| Sub        | Title                                                            | Risk |
|------------|------------------------------------------------------------------|------|
| D.6 (1/N)  | **Non-generic** per-unit separate compilation: `resolve_module`  | high |
|            | (per-unit ID spaces + imported externs, D5.1) + per-unit type-   |      |
|            | check against imported signatures + per-unit codegen (the 3      |      |
|            | broken assumptions, D5.2) + module-qualified mangling (D7) +      |      |
|            | extern-symbol cross-module **fn** calls + cross-module **types**  |      |
|            | (layout import) + deterministic multi-object link.               |      |
| D.6 (2/N)  | **Cross-module generics** (per-unit instantiation + `linkonce_odr` | high |
|            | dedup; `pub` generic bodies cross the boundary; module-qualified  |      |
|            | type tags, D7) + cross-module trait/impl **methods** + effects +  |      |
|            | visibility edge cases.                                           |      |
| D.6 (3/N)  | **Incremental caching** of unchanged units (Salsa-backed) + the   | med  |
|            | per-unit `.o` reproducibility discipline (extend `repro.rs`).     |      |

**Additive discipline (keep-green).** The per-unit back end lands **alongside** the
Path A merge, not in place of it: `merge_modules` + `run_build_merged` + `snc merge` +
the **self-host fixed-point paths** (which compile the merged compiler to one unit) +
both bootstrap fixed points stay green throughout. The per-unit pipeline is reached via
an **opt-in** (an internal `--separate` flag / env switch on `snc build`, exercised by
the D10 phase-go) until it reaches Path A parity at the end of (2/N); then it becomes
the **default** for `snc build` multi-file and the merge is retired for that path (the
merge + `source_dump` remain for `snc merge` + the self-host port, which are unaffected
by this track).

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

**Path A realization of cross-module items (landed, `3571ec2` types + `7fd7817`
traits/effects/classes).** Because Path A merges the graph into one `Program`
rather than compiling units independently, cross-module references are realized
by **name-qualification + reference-rewrite in `merge_modules`**, not by D5's
per-unit layout imports + extern symbols. **Every** top-level item's
*declaration* name is qualified by module path — fn, struct, enum, trait,
effect, class, and named impl (`geo::Point` → `geo$Point`, the same
collision-free `$` scheme as fns) — and a per-module `Renamer` (the
name→qualified map plus the in-scope type-parameter set, which is never
qualified) rewrites **every** reference:
- call callees;
- all signature `TypeExpr`s (fn params/returns, struct fields, enum variant
  payloads, trait/effect op sigs, class fields/methods/init, impl method sigs +
  the impl's for-type, delegate types), `let` annotations, struct literals, enum
  construction (`QualifiedCall`/`ClassInit` heads), and `match` variant patterns;
- `impl … as Trait for Type` heads (both names), `perform`/`handle` effect names,
  fn/method/trait/impl **effect rows**, delegate `to Trait` names, and named-impl
  `QualifiedCall` heads.

Op names and method names stay unqualified (scoped within their effect / trait,
like enum variants). The walks are exhaustive (no wildcard arm), so a new
`ExprKind` / `TypeExprKind` / `Pattern` variant is a compile error rather than a
silently un-rewritten reference. **So a `pub` item of any kind crosses a module
boundary and same-named items coexist.** Cross-module **generics** also work for
free (verified, `53a9aba`): `collect_mono_instantiations` runs whole-program over
the merged graph, so a generic instantiated in a different module than its
definition is monomorphised + emitted like any single-file instance (the
`Renamer` rewrites `TypeExprKind::Generic` heads + args; type params are never
qualified). **Effect-check parity** is also done (`7af1dce`): `run_build_merged`
calls the pure `sentinel_effect_check::effect_check(&typed)` between type-check and
borrow-check — the single-file path gets this from `borrow_check_query` chaining
on `effect_check_query`, but the merged path calls the pure passes directly, so it
must invoke effect-check itself — so a multi-file `main` with an unhandled effect
is rejected, not miscompiled. Still outstanding under Path A: span-accurate
multi-source diagnostics (the merged path reports by message; spans point into
per-module sources). The true per-unit back end (D5) will re-derive these symbols
through the module-qualified `abi-v1` mangling (D7) instead of the `$` scheme, and
add the per-unit `linkonce_odr` generic-dedup story (ADR 0037 (2/N)).

## Revisit

PROPOSED until the per-unit back end's D.6 (1/N) lands (the multi-file surface already
shipped via Path A), then ACCEPTED-WITH-AMENDMENTS as the sub-phases close. Triggers:
- **D6 generics**: if `linkonce_odr` per-unit instantiation proves too costly
  (code bloat / link time), consider a whole-program mono pass that still emits
  per-unit objects (a hybrid), or an explicit-instantiation surface.
- **D7 mangling**: if preserving single-file bare names as the empty-module case is
  awkward, bump to `abi-v2` instead of amending `abi-v1`.
- **D8**: `use` ergonomics (globs / groups / aliases / re-exports) once a self-host
  unit finds single-item `use` too noisy.

## SETTLED DESIGN POINTS

The four original open points are **settled** (owner-confirmed):

1. **Import cycles → ALLOW.** Separate compilation resolves cross-unit references at
   link time, so a `use` cycle is not a layering problem; discovery uses a `visited`
   set. (Realized in `discover_module_graph`.)
2. **`abi-v1` AMENDMENT, not `abi-v2`.** Module-qualified mangling preserves every
   single-file symbol as the empty-module-path case (D7), so no existing artifact's
   observable ABI changes — an amendment.
3. **Source root → the entry file's directory.** `snc build src/main.sentinel` → root
   `src/`; an explicit `--root` / manifest is deferred (D8).
4. **`use a::b::c` → item `c` in module `a::b`.** Last segment is the item; module-path
   imports are deferred (D8).

Settled in **this revision** (the per-unit back end):

5. **Mangling scheme (D7) → PINNED:** `_S` + length-prefixed module segments +
   length-prefixed item; empty module path → the bare item (single-file unchanged);
   `main` / `sentinel_*` exempt; `_S` reserved.
6. **Per-unit ID model (D5.1) → the extern-fn-in-FnId-space model** (an imported fn =
   an extern `FnSignature` + `link_symbol`, no body; no new expr variant; imported
   types are layout-only, no symbol).
7. **Additive gating (D9) → opt-in `--separate` until (2/N) parity, then default;** the
   Path A merge + the self-host fixed-point paths stay green throughout.

Deferred to (2/N) (flagged here so it is not forgotten):

8. **Cross-module type tags in mono keys** must be module-qualified (D7 last bullet) —
   else same-named cross-module types `linkonce_odr`-dedup unsoundly.

Realised in (2/N):

9. **Cross-UNIT effect op ids → a build-wide op-id BASE map** (effect NAME → a
   graph-stable sorted index), NOT a shared global `EffectId`. The `EffectId` is
   overloaded (the `(eid<<16)|op` op-id basis AND the `effect_decls[]` index), so a
   shared effect can't take a global id without the table index going out of bounds.
   Codegen's `encode_op_id_ctx` consults the map and DELEGATES to the standalone
   `encode_op_id` on a miss, so an empty map (single-file / merge / corpus) is
   byte-identical — an amendment, not an ABI break, and the `snc llvm` oracle copy of
   `encode_op_id` is untouched. The map is keyed by NAME for the MVP; same-named
   cross-module effects would collide — an **origin-qualified key is the robust
   upgrade** (the same shape as the point-8 type-tag fix). Effecting externs carry
   `effect_row_names`, re-resolved to the importer's `EffectId`s in `check_module`
   (the effect analogue of param-type re-resolution); the Kont* ABI follows via
   `uses_kont_abi`.
