# ADR 0063: Consuming a pre-built Sentinel library — a signed interface descriptor

Status: **PROPOSED (design).** The **other half** of the pre-built-libraries rock (HANDOVER
§0 NEXT item 1): consume a *compiled* Sentinel library (`snc build --lib` → `.a`/`.lib`)
instead of re-`use`-ing its source, with the ADR 0061 trust model applied. ADR 0061 built
the signing/trust machinery and noted it "signs such a descriptor"; this ADR specifies the
descriptor and the consumption path. Design-only; pins the model + the one real fork before
code, per the ADR-first norm.

Date: 2026-06-28
Related: **0037** (separate compilation — the extern-fn-in-FnId-space import model D5.1, the
exports table `extract_exports`, module-qualified `abi-v1` symbols D7; this reuses all of
it), **0059** (`snc build --lib` + `--emit-header` — the C header is the C-only analogue;
this is its Sentinel-shaped sibling), **0061** (the signed artifact + the build-time trust
gate + capability bounding — the descriptor is the thing signed/verified), **0029**
(`abi-v1` — the symbols the consumer links against).

## Context

`snc build --lib` already emits a compiled object + a C header (ADR 0059). A C header is not
enough for a **Sentinel** consumer: to type-check a call, resolve dispatch, and
capability-bound against a binary it has no source for, a consumer needs the **Sentinel**
signatures — param/return types, struct/enum layouts, **effect rows**, generic bodies — none
of which a C header carries.

The machinery to consume a module's *public interface as externs* already exists: ADR 0037's
`--separate` path computes an **exports table** (`extract_exports` → `ExportedItem`) and
resolves importers' `use`s against it via the **extern-fn-in-FnId-space** model (D5.1: an
imported non-generic `pub fn` becomes an extern `FnSignature` + its module-qualified
`abi-v1` link symbol, *no body*; a `pub` type is layout re-materialized; a `pub` generic fn's
*body* crosses to be inlined). Crucially, **`ExportedItem` holds AST declarations** (`FnDef`,
`StructDecl`, `EnumDecl`, signatures as `TypeExpr`, effect-row names) — *not* interned
`Type`s — so the interface is fully expressible as data/text without a type-interner
serialization problem.

So the gap is narrow: today that exports table is built by **parsing the dependency's
source**. For a *pre-built* library, the consumer has no source — it has the compiled object.
This ADR persists the exports table into a **descriptor file** shipped with the object, and
teaches the consumer to resolve against the descriptor + link the object, gated by ADR 0061.

## Decision (proposed)

### D1. A pre-built Sentinel library = three artifacts (a "package").

`snc build --lib foo.sentinel` (multi-module via ADR 0059 A8) gains an interface emission:

- **`libfoo.a` / `foo.lib`** — the compiled object (today's `--lib` output), exporting the
  module-qualified `abi-v1` symbols (ADR 0037 D7).
- **`foo.sif`** — the **Sentinel interface descriptor**: the serialized public interface (D2).
- **`foo.sif.sig`** — the ADR 0061 detached signature over `foo.sif` (D4).

The descriptor is the unit of *identity + trust*; the object is what the linker consumes. (A
consumer that has only the object + descriptor never sees the library's source.)

### D2. The descriptor = the serialized public interface (the exports table).

`foo.sif` carries, per `pub` item, exactly what the ADR 0037 importer needs:

- **non-generic `pub fn`** → name, param `TypeExpr`s, return `TypeExpr`, effect-row names,
  and its **module-qualified link symbol** (D7). No body.
- **`pub struct` / `pub enum`** → the full decl (layout; the importer re-materializes it,
  no link symbol — ADR 0037 D4).
- **`pub` generic fn** → the full `FnDef` **with body** (the importer inlines + monomorphizes
  locally — ADR 0037 D6). (This does ship that generic's *source body* — an accepted
  consequence of the C++-template model; non-generic bodies stay compiled-only.)
- **`pub trait` / `pub effect`** → the decl (re-materialized; ADR 0037 D6).
- a **header**: descriptor-format version, the source `abi-v1` version, the library module
  path(s), and the compiler version (for compatibility checks).

**▶ THE ONE REAL FORK — the descriptor's concrete format (D2a):**

- **(a) A structured descriptor** (TOML/JSON or a small custom grammar) read by a **dedicated
  descriptor reader**, never the main lexer/parser. **NOT oracle-moving** (the
  `snc lex`/`ast` dumps + the self-host fixed point are untouched). Cost: serialize the
  `ExportedItem` AST decls to the format (hand-rolled, or serde derives on the AST). The AST
  is already `Clone + Hash`; the `TypeExpr`/decl shapes are small. **Recommended** — same
  reasoning as ADR 0062's file-level choice (avoid the oracle tax unless it buys something).
- **(b) A Sentinel-source descriptor** with a new **`pub extern fn name(...) -> T;`**
  declaration form (a Sentinel-ABI signature-only item, vs `extern "C"`'s C-ABI), parsed by
  the **main parser** + resolved through the existing import path. Most uniform (the
  descriptor *is* Sentinel; one parser), and a nice language surface. But it adds syntax →
  **oracle-moving** (mirror `selfhost/`, re-bless, both fixed points). Heavier.

Recommendation: **(a)** for v1; revisit (b) if a first-class `pub extern fn` surface proves
worth the oracle tax later.

### D3. Consumption — resolve against the descriptor, link the object.

A consumer declares a pre-built dependency (a small **`[lib]` block in the consumer manifest**
— reusing/extending the ADR 0061 `sentinel-trust.toml`, or a sibling `sentinel-deps.toml`):
the library name → its `.sif` + object path (or a search dir holding `name.sif` +
`name.a`/`.lib`). Then:

- `use foo::item` resolves `foo` to its **descriptor** (not a `.sentinel` source file): the
  driver reads `foo.sif`, builds the same `ExportedItem` table `extract_exports` produces,
  and feeds it to the **existing** ADR 0037 importer path — `resolve_module(program,
  imports)` registers each item as an extern (`FnId` + link symbol) / layout type. **No new
  resolve/codegen model** — the descriptor just replaces "parse the source" as the table's
  origin.
- At link, the driver adds `foo.a`/`foo.lib` to the link line (the symbols the externs
  reference resolve there).
- Module discovery (`discover_module_graph`, ADR 0037 + 0062) tries a source file first, then
  a declared pre-built lib — so a dependency is consumed from source *or* compiled,
  interchangeably.

### D4. Trust (ADR 0061) — the descriptor is the signed artifact.

The `.sif` is signed; the build-time gate (`--require-signatures`, D7 of ADR 0061) verifies
`foo.sif.sig` over `foo.sif`, resolves the key against the trust manifest, and **capability-
bounds** the library by the descriptor's declared capabilities (the effect rows + `ffi`/…)
against the key's grants (ADR 0061 D6). So a pre-built dependency that is unsigned, signed by
an untrusted key, tampered, or over-reaching its grants **fails to compile** under strict —
exactly the AI_TOOLING §7.1 contract, now for *binary* dependencies (the supply-chain
sharp end: you're trusting a binary you can't read, so the signed descriptor is what you
review + pin). The object's integrity rides along: the descriptor commits to the object's
hash (a field in `foo.sif`), so a swapped `.a` fails verification too.

### D5. Scope (v1) + phasing.

Mirror the `--separate` maturity (ADR 0037): **v1 = non-generic `pub fn`s + `pub struct`/
`pub enum`** (the cases `--separate` fully supports cross-unit), the descriptor emission, the
descriptor-backed resolution, the link, and the trust gate over the `.sif`. **Deferred:**
generic `pub fn` bodies, traits/effects across a pre-built boundary (follow the ADR 0037
(2/N) tail), `--shared`/`dlopen` consumption, and a real package/registry (the dep is still
a local path).

## Reasoning

- **Reuse over reinvention.** The hard parts — the extern import model, module-qualified
  `abi-v1` symbols, the layout re-materialization, the trust gate — already exist (ADR 0037 +
  0061). This ADR adds only *persist the exports table* + *read it back as a table*. The
  AST-based exports table (not interned types) is what makes the descriptor cheap.
- **Structured over Sentinel-source (D2a).** A dedicated reader keeps the change off the
  self-host oracle; the descriptor is a *build artifact*, not a language surface, so it
  doesn't need to be Sentinel syntax. (b) is a tempting elegance but the oracle tax isn't
  worth it for v1.
- **The descriptor, not the object, is the trust unit.** You can't review a `.a`; you *can*
  review a `.sif` (it's the signatures + capabilities). Signing it — and committing to the
  object's hash within it — is the reviewable, pinnable artifact.

## Self-host

**Not oracle-moving under the recommended (a).** The descriptor is read by a dedicated
reader; no `selfhost` file is a `.sif`; the lexer/parser/AST dumps + both fixed points are
untouched. (Choosing (b) *would* be oracle-moving — the `pub extern fn` syntax — and is
explicitly the heavier path.) The link/resolve reuse changes which *table source* feeds the
existing importer, not the emitted IR for a given resolved program.

## Constant-time guarantee

**Untouched.** A consumed library was CT-checked when it was built (`--lib` runs the full
verification, ADR 0059); the descriptor carries its public signatures, and the FFI fence +
capability bounds gate what the consumer can do with it. The consumer's own code is
CT-checked as always.

## Non-goals (v1)

- Cross-`pub`-boundary **generics/traits/effects** for pre-built libs (follow ADR 0037 (2/N)).
- A package **registry** / fetch / version solver — the dependency is a local path + a
  manifest entry.
- **`--shared`/`dlopen`** consumption (the static object first).
- Consuming a **non-Sentinel** object as if it were a Sentinel lib (that is the `extern "C"`
  FFI path, ADR 0057).

## Open questions

- **D2a format** — structured (recommended, non-oracle-moving) vs `pub extern fn` (oracle-
  moving). The decision gates everything else.
- **Descriptor ↔ object binding** — the `.sif` commits to the object's SHA-512 (recommended,
  so a swapped object fails); confirm the object path/name convention.
- **Manifest home** — extend `sentinel-trust.toml` with a `[lib]`/`[[lib]]` block vs a sibling
  `sentinel-deps.toml`. Recommendation: one file (`sentinel-trust.toml` already holds the
  trusted keys a pre-built lib is verified against).
- **ABI-version compatibility** — the `.sif` header pins the `abi-v1` + compiler version;
  define the mismatch policy (refuse vs warn).

## Phasing

1. **Descriptor emission** — `snc build --lib --emit-interface foo.sif` (serialize the exports
   table; commit to the object hash). Producer side; testable by round-tripping the reader.
2. **Descriptor-backed resolution + link** — the `[lib]` manifest, `use` against a `.sif`,
   the extern import + link. The end-to-end consume path (build a lib, consume it compiled).
3. **Trust gate over the `.sif`** — wire the ADR 0061 gate + capability bounding to the
   pre-built path (the `.sif` is the signed/verified artifact).
