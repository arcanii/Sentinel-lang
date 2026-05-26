# ADR 0011: Phase C1 kickoff — type system, name resolution, Salsa retrofit

Status: PROPOSED — D1 (Salsa), D4 (sentinel-resolve crate lift),
and D5 (sentinel-types::check() real) fully exercised at C1.0a-c,
C1.1.1-2, and C1.2.1-4 respectively, with the intentional scope-cut
that codegen stays out of the salsa query graph until C1.2+ (the
typed-HIR rewrite per ADR 0012 lands codegen against TypedProgram,
still as a non-tracked downstream stage). The ADR remains PROPOSED
overall because D2 (explicit annotations at boundaries — partially
exercised; full activation at C1.3), D3 (multi-primitive types —
arrives at C1.3), D6 (sub-phase split — still in flight),
D7/D8/D9/D10 (concrete C1 surface, fixture rewrite, etc. —
exercised at C1.2 via ADR 0012), D11 (CodegenCtx layout shrink —
done at C1.1.2), D12 (perf discipline — deferred per the ADR) all
cover the rest of C1 (type system widening, structs, nullability,
arrays, generics) which haven't landed yet.
Date: 2026-05-25
Last touched: 2026-05-26 (C1.2 landed; D5 status updated)
Related: 0001 (staged validation), 0009 (Phase C kickoff; D1 deferred
Salsa to C1, D7 deferred sentinel-types and sentinel-resolve stubs
to C1), 0010 (concrete C0 surface; D5 reserved `->` for C1
annotations)

## Context

Phase C0 completed at `ca8143b`. The bootstrap compiler can lex,
parse, name-resolve (inside codegen for now), and lower a `fn`-
based program to a runnable binary via LLVM. **Everything is
i64.** There is no real type system — codegen treats every value
as i64 and the compiler accepts programs that would fail in any
language with nominal type safety:

    fn frobnicate(x) { x + 1 }
    fn main() { frobnicate(some_bool) }   // no error today

Phase C1 brings up the type system. HANDOVER §6.2 lists structs,
basic generics, references, `?T` optional, and bounds-checked
arrays as the C1 scope. The handover's budget is 3 months; this
ADR's honest estimate is more like 6, captured in D6.

Three architectural threads cut across C1 and need to land in
coordination:

  1. **Salsa retrofit.** ADR 0009 D1 deferred Salsa adoption to
     C1+ with a discipline rider (D1a) that C0 pipeline stages
     stay pure functions. C0 held the discipline — `lex`,
     `parse`, `compile_to_object` are all `(input) ->
     (output, diagnostics)`. C1 wraps each stage in
     `#[salsa::tracked]` so subsequent type-system work has
     incremental recompilation from day one.
  2. **Name resolution lifts.** Name resolution currently lives
     in `sentinel-codegen::CodegenCtx` (the `vars` and `fns`
     hashmaps). ADR 0009 D7 deferred `sentinel-resolve` to C1.
     C1 extracts this into a dedicated crate that runs before
     type checking.
  3. **`sentinel-types` becomes real.** ADR 0009 D7 deferred the
     `check()` stub to C1 because C0's arithmetic + i64-only
     program shape had no type semantics worth checking. C1
     introduces real types (i32, i64, bool, ?T, struct …) and
     the checker validates programs against them.

These three lifts are the *infrastructure* of C1. Once they're in
place, the per-feature additions (structs, references,
nullability, arrays, generics) sit on the same infrastructure.

## Decision

Twelve D-numbered sub-decisions.

### D1. Adopt Salsa at C1.0.

ADR 0009 D1 deferred Salsa to C1+; C1.0 is when. The retrofit
wraps each existing pipeline stage as a Salsa query:

  - `#[salsa::input] SourceFile` — input
  - `#[salsa::tracked] fn lex_query(db, file) -> Vec<Spanned<TokenKind>>`
  - `#[salsa::tracked] fn parse_query(db, file) -> Option<Program>`
  - … through codegen.

The `salsa = "0.18"` workspace dep is already pinned. C1.0 either
confirms the version or migrates (rust-analyzer's fork is the
alternative). Diagnostic queries use `salsa::accumulator` so
they don't break invalidation. The C0 pure-function discipline
makes the retrofit largely mechanical: each fn gets a `#[tracked]`
attribute and its first arg becomes `db: &dyn SentinelDb`.

**Status — C1.0a (09dc8c3): foundation crate `sentinel-base`
landed with SourceFile / SentinelDb / Diagnostic accumulator.
C1.0b (557cc60): lex_query and parse_query land in sentinel-syntax;
driver instantiates concrete SentinelDatabase. C1.0c: codegen-
salsa decision committed — see "C1.0c codegen-salsa decision"
addendum below. The Salsa retrofit is now complete for the scope
this ADR committed to in spirit; codegen wrapping is intentionally
out of D1's scope until typed HIR exists.**

**Tracked-struct return types collapse to success-only payloads.**
The original D1 sketch had each query return `(Output, Vec<Error>)`
in a tracked struct. C1.0a discovered this fails because
`miette::SourceSpan` does not derive Hash, and tracked-struct
fields require their inner types to be Hash. C1.0b's resolution:
tracked-function return values carry ONLY success payloads
(`Vec<Spanned<TokenKind>>` for lex, `Option<Program>` for parse —
None on failure); errors get converted to
`sentinel_base::Diagnostic` (a Hash-friendly struct) and pushed
via the accumulator. Driver collects via
`parse_query::accumulated::<Diagnostic>(db, file)`. The conversion
drops per-variant help/label text from the miette-derived error
enums; refining that is a separate concern from the retrofit
pattern.

**C1.0c codegen-salsa decision (option 2: defer until typed HIR
exists at C1.2+).** The original D1 ellipsis ("… through codegen")
implied that the Salsa retrofit would eventually wrap
`compile_to_object` as a tracked query. C1.0c reconsiders that
commitment and decides against it for now. Three options were
weighed at the C1.0c session opening (per HANDOVER §0.2 sketches):

  1. **Wrap a bitcode-producing query.** `compile_module(db, file)
     -> Vec<u8>` returns serialised LLVM bitcode (via
     `module.write_bitcode_to_memory()`); driver writes the object
     file + invokes cc outside the query graph. Sidesteps the
     LLVM `'ctx` lifetime by keeping Context/Module local to the
     query body. Cost: bitcode roundtrip per cache miss, plus
     bitcode-as-cache-key for change detection.

  2. **Don't wrap codegen at all.** Driver continues calling
     `compile_to_object` directly. Codegen sits outside the salsa
     query graph until at least C1.2 (when typed HIR replaces raw
     AST as codegen's input).

  3. **Wrap a `lower_to_ir` step only.** Split codegen into in-
     memory IR build (tracked) and object emit (untracked). Mostly
     an organisational win over option 1.

**Decision: option 2.** Reasoning:

  - Codegen gets rewritten at C1.2 anyway. The current
    `sentinel-codegen` lowers raw AST → LLVM IR; at C1.2 it will
    lower TypedProgram → LLVM IR with the type information
    informing instruction selection. Investing in a salsa wrapper
    for the pre-types codegen pays off for a few weeks at most
    and then has to be redone against the new input shape.
  - The LLVM lifetime story is non-trivial. `inkwell::Context`,
    `Module<'ctx>`, `Builder<'ctx>`, and `FunctionValue<'ctx>`
    all carry the same `'ctx` lifetime; making them survive
    across a tracked-query boundary requires either bitcode
    serialisation (option 1) or refactoring codegen to be a
    single function call that builds + emits in one go (which it
    almost already is). The design pressure to get this right is
    real but not load-bearing for any user-visible feature today.
  - The C1.0b front-end retrofit gave us the incremental-rebuild
    shape that matters for LSP / `cargo check`-style tooling.
    Those tools want types-but-not-codegen; they short-circuit
    after parse/types and never touch the codegen query. So
    codegen incremental rebuild has near-zero practical value
    until end-to-end incremental builds become a measurable
    bottleneck — far beyond C1.

  - The ADR 0009 D1a discipline (pipeline stages are pure
    functions) keeps codegen retrofittable at any later point.
    `compile_to_object(program, path) -> Result<(), CodegenError>`
    is already a pure-function-with-side-effect; wrapping it in
    salsa whenever we decide to is mechanical.

The cost of option 2 is a small piece of architectural debt: the
driver still has a direct function call from query-graph output
(parse_query's Program) into a non-salsa stage (codegen). This is
explicit, documented, and revisited automatically at C1.2 because
the codegen rewrite for typed HIR will touch the call site.

Status flips D1 to "fully exercised at C1.0c" — the Salsa retrofit
is complete for the front-end and intentionally bounded.
**Re-opens** at C1.2 with the typed-HIR codegen rewrite, when the
salsa-wrapping cost vs benefit can be reassessed against a more
complete query graph.

### D2. Type system shape: explicit annotations at fn boundaries; monomorphic in C1.

Function signatures require explicit type annotations:

    fn add(a: i64, b: i64) -> i64 { a + b }

ADR 0010 D5 reserved the `->` token specifically for this. Inside
fn bodies, simple `let` bindings either:

  - Carry an annotation: `let x: i64 = expr;`
  - Or have the type inferred from the RHS: `let x = expr;` —
    essentially bidirectional checking inside one fn

No type inference across fn boundaries in C1. No Hindley-Milner.
Sentinel 1.0 may add boundary inference later but C1's
commitment is explicit fn signatures.

### D3. Basic types in C1: `i32`, `i64`, `bool`, `?T`, `struct`.

Initial type universe:

  - `i32`, `i64` — sized integers
  - `bool` — `true` / `false` literals; ADR 0010 D9's C-style
    truthy `if cond` gets replaced with `cond: bool` at C1.3
  - `?T` — nullable per HANDOVER §6.2 / SENTINEL_DESIGN2 §4.3
  - `struct Name { field: Type, … }` — basic structs at C1.4

References (`&T`, `&mut T`) wait for C2 (regions). Generics wait
for C1.7.

### D4. sentinel-resolve lifts to its own crate at C1.1.

The name-resolution code currently in `sentinel-codegen` (the
`vars` and `fns` hashmaps inside `CodegenCtx`) moves to
`sentinel-resolve` at C1.1. The crate exports a function:

    pub fn resolve(program: &Program) -> Result<ResolvedProgram,
        ResolveError>

`ResolvedProgram` is the input AST with name references replaced
by stable identifiers (`VarId`, `FnId`) — no more string lookups
in codegen. The codegen pass becomes pure-structural against
`ResolvedProgram`.

**Status — ACCEPTED at C1.1.** C1.1.1 (438dd16) populated the
crate: VarId/FnId/FnSignature, parallel-tree resolved AST,
ResolveError with the 6 name-resolution variants migrated from
CodegenError, pure `resolve()` + `#[salsa::tracked]` `resolve_query`
chaining on parse_query. C1.1.2 (9374edf) rewired codegen to
consume `&ResolvedProgram` and updated the driver pipeline to
chain parse_query → resolve_query → codegen. The 22 C0 pass-test
fixtures still run end-to-end through the new path.

**Representation choice: parallel tree.** Three options were
weighed (side-table, generic-AST `ExprKind<R>`, parallel tree).
Parallel tree wins on debuggability — concrete variants, no
generics to chase — at the cost of keeping two type hierarchies
in sync. See STATE.md C.3 decision 38 for the full weigh-up.

### D5. sentinel-types implements check() at C1.2.

After C1.1's resolve crate exists, `sentinel-types::check
(resolved_program) -> Result<TypedProgram, TypeError>` runs
between resolve and codegen:

  - Parses type annotations (`i64`, `bool`, `?T`, `Foo` for
    struct names — the annotation grammar lands as part of C1.2)
  - Validates fn signatures (return type matches body, param
    types annotate the body)
  - Type-checks expressions against expected types
  - Surfaces `TypeError::Mismatch`, `TypeError::ArgCountMismatch`
    (replaces codegen-level `ArityMismatch`), etc.

C0's `CodegenError::ArityMismatch` and similar shift upstream;
codegen at C1.2+ should be diagnostic-free if `check()` passed.

**Status — ACCEPTED at C1.2** (C1.2.1 lexer `:` token at af16655;
C1.2.2 AST/parser annotation grammar + 22 fixture rewrite at
90965a5; C1.2.3 sentinel-types scaffold at ded07bc; C1.2.4
codegen+driver consume TypedProgram at c9a21ff). The check()
function takes &ResolvedProgram and returns a TypedProgram
parallel tree with `Type` on every expression. At C1.2 the
universe is just `I64` so only `TypeError::UnknownType` fires;
`Mismatch`, `ReturnTypeMismatch`, and `CallArgMismatch` exist
but are dormant until C1.3 introduces multi-type expressions.
`ArityMismatch` (the old codegen variant from D5's "and similar")
already moved to `ResolveError` at C1.1.1 since arity-checking
is name-resolution territory; sentinel-types didn't need to
duplicate it.

### D6. C1 sub-phase split — eight sub-phases.

| Sub  | Title                                                    | Estimate |
|------|----------------------------------------------------------|----------|
| C1.0 | Salsa retrofit; pipeline becomes queries                 | 2 wks    |
| C1.1 | sentinel-resolve crate; name resolution moves out of codegen | 2-3 wks |
| C1.2 | sentinel-types: annotation grammar + basic type check    | 4 wks    |
| C1.3 | Multiple primitive types (i32, i64, bool); replace C-style truthy | 2 wks |
| C1.4 | Struct definitions + field access                        | 3-4 wks  |
| C1.5 | `?T` nullability + null-checks                            | 2-3 wks  |
| C1.6 | Arrays + bounds checking                                  | 3-4 wks  |
| C1.7 | Witness-table generics (basic)                            | 4-6 wks  |

Honest total: ~22-28 weeks (~5-6 months). HANDOVER §6.2's
3-month budget for C1 was optimistic; ADR 0011 acknowledges this
up front rather than discovering it sub-phase by sub-phase. The
budget pressure is real (Phase C1 + C2 + C3 + C4 + C5 has to
fit in the remaining ~12 months of HANDOVER §6's 12-18 month
phase budget) — flagged here so the schedule is renegotiated
ahead of time rather than mid-C1.

### D7. ADR 0012 (concrete C1 surface) lands before C1.2.

Following ADR 0010's pattern: C1.2 introduces the concrete
surface for type annotations (`fn f(x: T) -> T`,
`let x: T = …`), `true`/`false` literals, struct declaration
syntax, field-access syntax (probably `.field`), and the
syntax for `?T` (probably `?T` with a postfix question mark or
prefix `?`). The concrete decisions live in ADR 0012, written
after C1.1 ships so the parser has a real AST to point at.

### D8. Backwards-compat break at C1.2.

C0.5 fixtures have no type annotations. C1.2 requires `fn f(x:
i64) -> i64`. Existing 22 pass-test fixtures get a mechanical
annotation rewrite at C1.2 (similar to the C0.5 fn-wrap pass).
This is the second hard break and the last large-scale fixture
rewrite for a while.

### D9. Test harness expansion.

`tests/ui/` grows type-error fixtures (`type_mismatch_add_bool
.sentinel`, `type_mismatch_assign.sentinel`, `arity_mismatch
.sentinel`, `null_deref.sentinel`, etc.) as each C1.X
sub-phase lands new diagnostics. `tests/pass/` grows with
`c1X_*` per-sub-phase acceptance fixtures.

### D10. C0.4 C-style truthy retires at C1.3.

When `bool` lands in C1.3, `if cond { … }` requires `cond:
bool`. C-style `if 5 { … }` becomes a `TypeError::Mismatch`.
The seven if-using C0 fixtures (c04_if_*, c05_go_no_go) get
their conditions updated — typically via comparison operators
(also C1.3): `if x != 0 { … }`, `if cond_fn(x) { … }`, etc.

### D11. CodegenCtx layout shrinks.

Once name resolution moves out of codegen (D4), `CodegenCtx`
loses the `vars` and `fns` hashmaps. It keeps the LLVM bits
(context/builder/i64_type — but the type is now whatever the
TypedProgram says) and gains type-aware lowering for the new
primitives. The two-pass codegen pattern stays (signatures then
bodies). For C1.3+, the per-fn vars map comes back but keyed by
the resolve-assigned `VarId` rather than `String`.

### D12. Performance discipline (deferred to C1.7).

HANDOVER §6.5 targets (clean build 10K lines in 30s, incremental
in 1s, LSP go-to-definition p95 50ms) become measurable once
Salsa is in (C1.0) but only meaningful once there's enough
language for non-trivial programs (C1.7+). C1.7's exit checks
benchmarks against these targets. If they fail, C2 starts with
a performance sub-phase before regions work begins.

## Reasoning

The C1 decision space clusters around three themes:

**Infrastructure-first ordering.** D1 (Salsa), D4 (resolve crate
lift), D5 (types crate lift) all land before per-feature work.
The cost of doing per-feature work first and retrofitting Salsa
later is the exact mistake ADR 0009 D1 avoided by deferring
Salsa from C0 to C1 — pay it once now, before the type system
makes Salsa's query keys complicated to design.

**Explicit over inferred.** D2's explicit-annotations-at-fn-
boundaries call buys two things: (a) alignment with Sentinel 1.0
per SENTINEL_DESIGN2 §4 (Rust-style boundary annotations), (b)
simpler initial type-checker — no HM unification, no row
polymorphism, no occurs check. The Phase B Sentinel-Mini work
showed how complex HM-with-effects gets; C1's job is not to
relearn those lessons but to ship a working monomorphic checker.
Inference inside fn bodies (let-binding type inferred from RHS)
is the smallest concession to ergonomics.

**Honest budget.** D6 documents 5-6 months, not HANDOVER's 3.
The estimate is per sub-phase, not aspirational. Better to write
this down now than discover it mid-C1.4.

## Consequences

### Positive

- C1.0-C1.2 (the infrastructure trio) unblocks every later
  sub-phase. Once Salsa + resolve + types-stub-becomes-real are
  in, the per-feature work (structs, nullability, arrays,
  generics) is "just" adding type rules and codegen lowering.
- D8's hard break at C1.2 is the predictable second mass-fixture
  rewrite. After that, the surface should be stable enough that
  fixtures don't need wholesale rewriting again until 1.0.
- The C0 rhythm (ADR first, feat commits per sub-phase, docs
  commits at sub-phase boundaries, STATE.md backfill with
  hashes) transfers directly to C1.
- Going into C1 with a credible Salsa adoption means LSP polish
  (C5) inherits incremental recompilation for free.

### Negative

- 5-6 month estimate for C1 alone. With C2 (regions), C3
  (effects integration from Phase B), C4 (classes/traits), C5
  (broker integration/LSP) still ahead, HANDOVER §6's 12-18
  month phase budget is going to slip. This ADR makes the slip
  visible early.
- D8's annotation pass at C1.2 means the 22 fixtures get
  rewritten again. Mechanical, but boring.
- D11's "vars and fns leave the ctx" requires codegen to
  rebuild a per-fn local map keyed by `VarId` — small shift in
  the codegen shape.

### Neutral

- D12's perf-targets-deferred is honest about when the
  numbers become measurable. They're aspirational until then.

## Alternatives considered

- **Skip Salsa for C1 entirely; defer to C5 or later.** Rejected
  because ADR 0009 D1 already committed to C1; pushing further
  makes the eventual retrofit larger. The "C0 pipeline stages
  stay pure functions" discipline (ADR 0009 D1a) was held
  specifically to keep the C1 retrofit cheap; now is when that
  pays off.
- **HM type inference instead of explicit annotations.**
  Rejected: Sentinel 1.0 surface (SENTINEL_DESIGN2 §15.1) reads
  as Rust-style with explicit boundaries. Inference inside fn
  bodies (D2's "let binding inferred from RHS") is the
  pragmatic middle ground.
- **Merge sentinel-resolve into sentinel-types.** One crate
  doing both. Rejected: per-pass crates align with the
  per-pass Salsa queries; LSP/refactoring tooling later wants
  per-pass query keys. Also: name resolution is a thing in
  every compiler; making it its own crate matches the field.
- **Skip nullability in C1; defer to C2.** Rejected: HANDOVER
  §6.2 lists `?T` as a C1 deliverable; deferring would push the
  "obvious memory-safety violations" criterion (HANDOVER §6.2:
  "At the end of C1 the compiler should reject all the
  'obvious' memory-safety violations") past C1.

## Revisit

D6's per-sub-phase estimates are aspirational. If C1.2 takes 6
weeks instead of 4, the C1.4+ schedule slides; that's normal
for compiler work. Revisit the estimates at each sub-phase
boundary; STATE.md captures actuals.

D2's "explicit annotations, monomorphic" may turn out to be too
restrictive. If C1.5's nullability work surfaces a need for
inference (e.g., `let x = if cond { 1 } else { null };` needs
to infer `?i64`), revisit D2 — bidirectional checking might
extend to this case without going to full HM.

D6's C1.7 (generics) is the longest single sub-phase and the
one most likely to slip. If the witness-table choice (HANDOVER
§14.1) turns out painful, C1.7 may split or move to early C2.

D11 (CodegenCtx loses vars/fns hashmaps) may need different
state. If C1.6 arrays need codegen-side metadata (e.g., per-
array length tracking), the ctx shape revisits.

D12's perf measurements may surface before C1.7 if Salsa's
query graph evolves in unexpected directions. If C1.4 (structs)
already takes the incremental rebuild past 1s, C1.4's exit
includes a perf check.
