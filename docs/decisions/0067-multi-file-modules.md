# ADR 0067: Multi-file modules + explicit module declarations (self-host modularization)

Status: **PROPOSED** — the four model D-points (D2 surface, D3 multi-file
mechanism, D4 visibility, D6 byte-identical bar) were **confirmed with the
maintainer 2026-06-29**; two implementation sub-points are **flagged for
ratification at implementation** (the entry-exemption scope of "mandatory
declarations", D2; parts-manifest vs a `read_dir` builtin, D3). This is an
**oracle-moving** change (the discovery + merge layer in BOTH compilers), so it
follows the both-bootstrap-fixed-points rhythm: Rust `snc` + fixtures → re-bless
the parse/ast/resolve differential → mirror into `selfhost/` → both fixed points
byte-identical (snc == scg) → ACCEPTED-WITH-AMENDMENTS as the split lands.
**D9 step 1 (the Rust `snc` mechanism) is IMPLEMENTED and four-check green** — see
the *Implementation status* section.

Date: 2026-06-29

## Related

- **0037** (modules / file-as-module + separate compilation — PROPOSED, MVP
  complete) — the model this **extends, not replaces**. ADR 0037 is **one file =
  one module**, the module path **implicit from the file path**, with per-item
  `use a::b::Item;`. This ADR adds **explicit `module` declarations + multi-file
  modules** ON TOP, keeping 0037's path-based single-file modules as the
  unchanged common case (every existing single-file selfhost module + all of
  `std` keep working with no restructuring).
- **0041** (the self-host `types` stage) — `selfhost/types.sentinel` is the
  **13,718-line** file (62% of the self-host, ~5× the next file) this ADR exists
  to make splittable. It holds the type interner + generic-fn inference +
  borrow-move analysis + the `cg` text emitter + the MIR dump, with **2 of its
  313 fns `pub`**.
- **0045** (self-hosting the D.6 merge — path (a)) — `selfhost/merge.sentinel`,
  where the selfhost MIRROR of this change lands. Its **per-file self-contained
  rename map** (each module processed one-at-a-time, its own top-level names
  qualified by its `$`-prefix) is precisely the assumption multi-file modules
  must **widen to module scope**.
- **0029** (stable `abi-v1`, FROZEN) — mangling / symbols. The split must not
  change observable ABI: module **identity stays `types`**, so the `types$*`
  module-qualified symbols are UNCHANGED. FnId numbering MAY shift but **only
  identically on the Rust `snc` and the self-hosted `scg`** (the maintainer's
  "loose" bar, D6). **No new runtime `sentinel_*` symbol is added** — a
  deliberate choice (see D3 reasoning).
- **0066** (threading + multi-processing roadmap — ACCEPTED) — the concurrency
  track this is prioritized **ahead of** (maintainer 2026-06-29, "maintainability
  is biting now"; the rest of 0066 is PAUSED). The M1.2 channels work surfaced a
  **base-shift bug hidden in a threshold buried in `types.sentinel`** — the
  cautionary tale motivating this whole effort, and the reason D3 **avoids adding
  a builtin** (which would re-shift the FnId base in lockstep across both
  compilers).
- **0008** (secret / constant-time) — the guarantee a front-end/merge change must
  not disturb (see the Security note).

## Context

`selfhost/types.sentinel` is the self-host's chief maintainability liability:
**13,718 lines**, 62% of the self-host, ~5× the next-largest file. It bundles the
type interner, generic-fn inference, borrow-move analysis, the codegen (`cg`) text
emitter, AND the per-fn MIR dump into one file. Every oracle-moving change touches
it in many scattered places — the M1.2 user-fn-base-shift bug hid in a threshold
comparison buried inside it. The maintainer flagged it for splitting (2026-06-29).

**The blocker is the module model.** ADR 0037 is one-file-one-module with
path-implicit identity and per-item `use`. Splitting `types.sentinel` under that
model is possible but ugly:

- Only **2 of 313 fns are `pub`** (`run`, `dump_moves`). Splitting into N modules
  under 0037 would force **pub-ifying ~311 internal helpers** — exposing the
  typer's internals across module boundaries (an encapsulation regression), the
  opposite of what the split is for.
- It would **fragment the `types::` namespace** that importers
  (`borrow`/`codegen`/`ctverify`/`mir` all `use types::run;`) depend on.
- The one shared mutable `TyCtx` struct couples the typer / cg / mir — but a
  **struct crosses module boundaries fine** (it is layout-imported, ADR 0037 D4);
  the real obstacle is purely the 311 private helpers + the shared namespace.
  (Classes are NOT involved — `types.sentinel` uses none, BACKLOG §11.7.)

The clean enabler: **explicit module declarations + multi-file modules** — let
several files form ONE logical module (the Rust `mod` / C++-namespace model), so
`types.sentinel` splits into focused files that TOGETHER are module `types`,
internals staying module-private and the public API + `types::` namespace
unchanged.

## Decision

### D1. Goal

Let **several source files form one logical module**. `selfhost/types.sentinel`
splits into focused files (interner / infer / borrow / cg / mir) that together
ARE module `types`: each file's non-`pub` helpers stay **module-private** (visible
across the module's files, not exported), and the **public API** (`pub fn run`,
`pub fn dump_moves`) plus the **`types::` namespace** importers use are
**UNCHANGED**. Keep ADR 0037's path-based single-file modules as the unchanged
default.

### D2. Surface — the `module` declaration (mandatory in library files)

- A new top-level **`module <path>;`** declaration — a `#[token("module")]`
  parallel to `use`, parsed like a `UseDecl` (path + `;`), appearing **before**
  the `use`s. `<path>` is the `::`-separated module path (`module types;`;
  `module a::b;` for a nested module).
- **Mandatory** in every file reached as a **library module** (via a `use` edge or
  as a multi-file **part**, D3). The declared path is **checked against the file's
  location** — a mismatch is a focused diagnostic (`ModuleDeclMismatch`). This
  realizes the maintainer's "declarations everywhere": every library file states
  its module, and each part of a multi-file module **explicitly claims membership**
  (`module types;`), so the directory→module association is explicit + verified,
  never inferred.
- **The build ENTRY is exempt.** The file passed to `snc build` (carrying `main`,
  never a `use`/`part` target) needs no declaration — its identity is its file
  stem, and forcing a decl on every standalone fixture / example / differential
  seed would be pure ceremony. A file with no `module` decl keeps today's
  path-derived identity (ADR 0037 unchanged), which is the entry / single-file
  case. **[FLAGGED]** the maintainer chose "mandatory everywhere"; this
  entry-exemption is the practical scope (it keeps the hundreds of single-file
  `tests/pass` / `examples` programs valid) — for confirmation at implementation.

### D3. Multi-file modules — a directory + a parts manifest

The maintainer chose **"directory = module"**. Realized so it needs **no
filesystem enumeration** (see reasoning):

- A module `a::b` is rooted at the existing **`<root>/a/b.sentinel`** — discovery
  reads this for `use a::b::Item` exactly as today (**zero discovery change** for
  the single-file case).
- The root MAY declare **`part <name>;`** directives. Each names an additional
  source file **`<root>/a/b/<name>.sentinel`** (in a sibling directory named for
  the module's leaf), which also declares `module a::b;`. The root + all parts
  **together form the one logical module** `a::b`.
- **No `part` decls ⇒ single-file module, exactly as ADR 0037** (backward
  compatible).
- Merged order = the **root's items, then each part in `part`-declaration order**
  (deterministic, explicit; no filename-sort dependence).

So the `types` split lands as:

```
selfhost/
  types.sentinel          # module types;  +  part interner; part infer; part borrow; part cg; part mir;
                          #   (the module "front page": the manifest + shared `use parser::…` + pub fn run/dump_moves)
  types/
    interner.sentinel     # module types;   (the type interner + tables — module-private)
    infer.sentinel        # module types;   (typed dump + generic-fn inference + widening)
    borrow.sentinel       # module types;   (borrow-check move analysis — `dump_moves`)
    cg.sentinel           # module types;   (the `cg` text emitter)
    mir.sentinel          # module types;   (the per-fn MIR dump)
  codegen.sentinel        # use types::run;   ← UNCHANGED
```

**Why a manifest, not filesystem auto-listing.** The maintainer's literal
preview ("read all `*.sentinel` in `types/`, sorted by name") needs directory
enumeration. The **self-hosted** `merge.sentinel` does its OWN discovery (so `scg`
can compile the multi-module selfhost source for the fixed point) and reads files
**by constructed path via `read_file`** — there is **no directory-listing
builtin**. Adding one (`read_dir`) is a **new runtime symbol + a new builtin
FnId**, which **shifts the user-fn base in lockstep across both compilers** — the
exact buried-threshold hazard (M1.2) this maintainability work exists to reduce.
The manifest reuses the existing `read_file`-by-path machinery, identical on both
sides, with **zero new ABI**. **[FLAGGED ALTERNATIVE]** add a `read_dir` builtin
for true auto-listing (drop files in the dir, no manifest) — matches the literal
preview but is heavier and re-triggers the FnId-base-shift rhythm; recommend
deferring unless the manifest proves too noisy across many multi-file modules.

*(Aesthetic variant, bikeshed for review: place the root inside the directory as
`types/mod.sentinel` so the dir holds everything. Costs a file-vs-dir discovery
branch; the form above keeps discovery's `a::b → a/b.sentinel` map unchanged and
is recommended.)*

### D4. Visibility — module-wide private (two levels, unchanged `pub`)

- **`pub`** = exported OUT of the module (cross-module), **exactly as ADR 0037
  today**.
- **Everything not `pub`** = **module-private**: visible across ALL the module's
  files (root + every part), NOT exported. A non-`pub` helper in
  `types/interner.sentinel` is callable from `types/cg.sentinel` with no `pub`.
  There is **no file-level privacy** — the file is invisible to the visibility
  model (the Rust/C++ namespace model).
- The public **`types::` namespace is unchanged**: still exactly `run` +
  `dump_moves`. Importers (`use types::run;`) are untouched.

### D5. Resolve / merge — the rename scope becomes the MODULE

The core mechanism change, in BOTH the Rust `merge_modules` and
`selfhost/merge.sentinel`:

- **Today:** each `ModuleUnit` (= one file) builds a **self-contained rename map**
  (its own top-level names → `module$name`) and is processed independently; a
  reference resolves only against its own file's names + its `use` imports.
- **Multi-file:** the rename map / namespace is built **per MODULE** — the **union
  of all the module's files' top-level items** → `module$name` — and shared across
  the module's files. A cross-file reference within the module (cg → interner's
  private helper) resolves through the shared map to `types$helper`.
- **Rust (`merge_modules`):** group discovered units by module path; for a
  multi-file module build ONE rename map from the union of all its parts' decls;
  emit all parts under that map; the module occupies **one position** in the
  merged-`Program` order (its parts concatenated root-then-`part`-order). The
  `module$name` qualification is unchanged → `types$run` etc. byte-identical.
- **Selfhost (`merge.sentinel`):** the current one-module-at-a-time emit with a
  per-file self-contained rename map must, for a multi-file module, **first scan
  all the module's parts' top-level decls into the rename map** (a pre-pass over
  the parts), **then emit each part's bodies** under that combined map. This
  widens 0045's "self-contained per-file" enabler to "self-contained per-module".

### D6. FnId / symbols — the byte-identical bar (loose: snc == scg)

- Module **identity is unchanged** (`types`), so all `types$*` mangled symbols are
  UNCHANGED across the split.
- FnId numbering = the merged-`Program` item order. The split MAY shift it (e.g.
  if parts are grouped semantically rather than in the original interleaved
  source order). The maintainer's chosen bar: require only that **`snc` and `scg`
  produce identical IR for the split source** (the bootstrap fixed point), NOT
  that it match pre-split. Both compilers run the **same** multi-file merge, so
  they agree.
- `__spawn_wrapper_<id>` embeds a FnId of the *compiled program*; the selfhost
  compiler does not `spawn`, so its own wrappers are N/A — but the principle
  (FnId-embedding symbols shift together on both sides) holds for any program it
  compiles.
- **Sanity aid (not a gate):** where a split step happens to preserve source
  order, diff post-split `snc` IR against pre-split `snc` IR to localize any
  unintended change. Convenience under the "loose" bar, not a requirement.

### D7. Oracle-moving surface — minimized

The `module` / `part` decls are **consumed at discovery/resolve** (identity
directives), NOT carried into the typed program. Therefore:

- **parse / ast / resolve dumps** gain the `module` / `part` nodes →
  **oracle-moving** → re-bless that differential + mirror into the selfhost
  parser/merge.
- **types / mir / borrow / effects / codegen / llvm dumps**: **UNCHANGED** (the
  decls never reach them; module identity + symbols are unchanged). The heavy
  differentials stay green — including the `secret_leak` and borrow differentials
  (Security note).

### D8. Out of scope (deferred)

- `read_dir`-based auto-listing (the D3 flagged alternative).
- Nested submodules INSIDE a multi-file-module directory (a multi-file dir is a
  **leaf** module; `a/b/c.sentinel` nested modules continue to use the existing
  path model and are not mixed into a multi-file `a::b`).
- Re-exports (`pub use`), finer visibility than `pub` / module-private, glob /
  group `use` (ADR 0037 D8 still governs `use` ergonomics).
- Mandatory `module` decls on standalone ENTRIES (fixtures / examples) — the entry
  stays exempt (D2).

### D9. Migration sequence (staged; both fixed points byte-identical at each step)

1. **Mechanism in Rust `snc`.** The `module` token + parse (UseDecl-shaped) + the
   `part` directive; directory/parts discovery in `discover_module_graph`; the
   **module-scoped rename** in `merge_modules` + per-unit resolve grouping. Emit
   `module`/`part` into the ast/parse/resolve dumps. UI fixtures for the new
   rejections (`ModuleDeclMismatch`, `ModuleConflict` [a file AND a `part` claim],
   a missing decl on a library file). Four-check green.
2. **Re-bless** the parse / ast / resolve differential (oracle moved); confirm the
   heavier stage differentials are untouched.
3. **Mirror into `selfhost/`** — `parser.sentinel` (`module`/`part` parse + dumps)
   and `merge.sentinel` (module-scoped rename + parts discovery). Then a
   **byte-identical PREP**: add `module <name>;` to every selfhost **library** file
   (`parser`/`resolve`/`types`/`effects`/`merge`/`codegen`/`borrow`/`mir`/
   `ctverify`). Since the declared name == the path-derived name, the merged
   `Program` + emitted IR are **byte-identical** (only parse/ast/resolve dumps
   move) — this verifies the decl is wired with **no** restructuring yet, and gets
   both fixed points green on the still-single-file `types`.
4. **Split `types.sentinel`** into `types.sentinel` (manifest + front page) +
   `types/{interner,infer,borrow,cg,mir}.sentinel`, **one part per commit**,
   running the **FULL self-host differential green** (snc == scg) at each step.
   Sweep `module X;` into `std` library files as touched (the "everywhere"
   end-state), each byte-identical.

### D10. Phase-go + fixtures

- A multi-file **phase-go**: a small module split into a root + 2 parts where a
  **private** helper in part A is called from part B, `use`d from an entry →
  builds + runs to a computed exit code (proves cross-file module-private
  visibility + the manifest discovery + the module-scoped merge).
- **UI fixtures:** `ModuleDeclMismatch` (decl ≠ location), `ModuleConflict`,
  a non-`pub` item still rejected when imported cross-module (module-private ≠
  exported), and a cross-file private reference within a module accepted.
- The real validation: the `types.sentinel` split with the full differential
  green at every commit.

## Reasoning

- **Why directory + manifest (not auto-listing).** The selfhost has no
  directory-enumeration builtin; adding one is a new runtime symbol + a builtin
  FnId that shifts the user-fn base in lockstep across both compilers — the exact
  buried-threshold hazard (M1.2) this work exists to reduce. The manifest reuses
  `read_file`-by-path, identical on both sides, with zero new ABI.
- **Why module-wide private.** Only 2 of 313 fns are `pub`; pub-ifying the other
  311 to split the file would expose the typer's internals cross-module — an
  encapsulation regression, exactly the outcome to avoid.
- **Why mandatory declarations (maintainer's call).** Uniformity: every library
  file states its module, and a multi-file module's parts each explicitly claim
  membership, so the directory→module association is explicit and checkable rather
  than inferred from a scan.
- **Why the loose byte-identical bar (maintainer's call).** It frees the split to
  group code semantically instead of preserving the original interleaved order.
  The fixed point (snc == scg) is the real correctness property, and both
  compilers run the same merge, so they agree regardless of grouping.

## Consequences

**Positive.** `types.sentinel` splits into ~6 focused files; the typer's 311
helpers stay encapsulated; importers are unchanged; the largest maintainability
liability in the self-host is removed — without weakening any guarantee.

**Negative.** A genuine oracle-moving change to discovery + merge in BOTH
compilers (parse/ast/resolve re-bless + selfhost mirror); a new `module` / `part`
surface; the selfhost merge's per-file rename widens to per-module (a real but
contained pre-pass).

**Neutral.** No new runtime `sentinel_*` symbol; no FnId-base shift (the
deliberate avoidance). Module identity + `abi-v1` mangling unchanged. Codegen, the
borrow checker, and the constant-time check are untouched (a front-end / discovery
/ merge concern only).

## Security note

Modularization is a front-end / discovery / merge concern. It does **not** touch
the MIR `secret_leak` taint oracle, the lexical borrow checker, the FFI/process
secret fence, or codegen. Module boundaries carry **declarations**, not values, so
there is **no new secret sink** and the constant-time guarantee is unweakened. The
migration plan (D7/D9) keeps the `secret_leak` and borrow differentials
byte-identical throughout — confirm them green at each step. A change that altered
either is out of scope here and would be a security regression to report
privately, not a modularization step.

## Implementation status

**D9 step 1 — the Rust `snc` mechanism — landed** (this commit), four-check green
(self-host differential 23/23; only the documented pre-existing Windows failures
remain — `separate_*` link/nm, `c5d4_file_io`'s `/tmp`). What shipped:

- **Surface:** `module <path>;` and `part <name>;` as hard keyword tokens
  (`#[token]`, uniform with `use`; no identifier collisions in the tree), parsed
  into `Program.module: Option<ModuleDecl>` + `Program.parts: Vec<PartDecl>`. A
  pure-manifest root (module/parts, no items) is a valid program. Duplicate
  `module` → `ParseError::DuplicateModuleDecl`.
- **Discovery (`discover_module_graph`):** a module's root is read at
  `<root>/a/b.sentinel` as before; its `part`s are read by path from
  `<root>/a/b/<part>.sentinel` (no directory enumeration). Each file's optional
  `module` decl is checked against its location (`ModuleDeclMismatch`); a missing
  part file is a focused error. The entry may itself be a multi-file root.
- **Merge (`merge_modules`):** the qualify/rewrite rename map is now built **per
  module path** — the union of every file (root + parts) sharing it — so a
  non-`pub` helper in one part is visible from another and rewrites to the same
  `module$helper` symbol. Single-file modules are byte-identical to before (the
  union is just that file), which is why the full self-host differential stays
  green. `resolve_imports` searches every file of a module for an imported item.
- **Dumps:** `module`/`part` emit into the AST/parse dump (`ast_dump`) as
  `(module a b)` / `(part name)`, the oracle format the self-host parser will
  mirror (step 3). Conditional, so decl-less programs dump byte-identically.
- **Tests:** parser unit tests (parse, nested path, pure manifest, duplicate,
  missing semicolons) + 4 `modules.rs` integration tests (the phase-go — a module
  spanning a root + part with cross-file module-private visibility; an entry that
  is itself a multi-file root; the missing-part and decl-mismatch rejections).

**Refinements vs the literal D9-step-1 list (for ratification):**

1. **Resolve dump unchanged.** `module`/`part` are consumed at discovery and the
   `ResolvedProgram` does not carry them; no corpus fixture exercises them. They
   appear in the AST/parse dump only (where `use` lives) — consistent with
   "consumed at discovery". (Cheaper, and nothing in the differential needs it.)
2. **No `ModuleConflict` rejection.** In the manifest design the root is *always*
   the module file and `part`s are explicit, so there is no file-vs-directory
   ambiguity to reject; the discovery rejections are **missing-part** and
   **decl-mismatch** (plus parse-time **duplicate-module**).
3. **"Mandatory decl on a library file" is not yet ENFORCED.** Enforcing it now is
   a flag-day that would break every existing (decl-less) selfhost/`std` module
   before the step-3 sweep adds declarations. For now a missing `module` decl
   keeps the path-derived identity (ADR 0037, lenient); the **mandatory check
   flips on after the decl sweep** (step 3 prep). This is the only deviation from
   the maintainer's "mandatory everywhere" call and is a staging necessity, not a
   change of decision.

**Known wrinkle (entry-as-root only):** for `snc build X.sentinel` where
`X.sentinel` is itself a multi-file root, the default output name `X` collides
with its parts directory `X/` (an LNK error). The self-host never hits this (a
multi-file root like `types` is a *library*, not the entry; the entry's output is
named for the entry). A follow-up could detect the collision and pick a distinct
default; for now pass an explicit `-o`.

**Out of step-1 scope (follow-ups):** `--separate` over multi-file modules (the
merge path is primary and the only one the self-host uses); the self-host mirror
(step 3); the byte-identical decl sweep + mandatory-enforcement flip.

## Revisit

PROPOSED until D9 step 1 (the Rust mechanism) lands and the maintainer confirms
the two flagged sub-points (the entry-exemption scope of "mandatory declarations",
D2; parts-manifest vs a `read_dir` builtin, D3). Then **ACCEPTED-WITH-AMENDMENTS**
as the split proceeds. Triggers:

- If the manifest proves too noisy across many multi-file modules, reconsider the
  `read_dir` builtin (D3 alternative), accepting the one-time FnId-base-shift cost.
- If the per-module rename in the selfhost merge is too costly one-module-at-a-time,
  consider a whole-graph pre-pass that collects every module's decls first.
- If "directory = module" wants the root inside the directory (`types/mod.sentinel`)
  for tidiness, adopt the D3 aesthetic variant (a file-vs-dir discovery branch).
