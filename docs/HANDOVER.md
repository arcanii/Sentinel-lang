# HANDOVER.md — Sentinel Bootstrap Compiler Implementation

This document is the practical handover for starting work on the Sentinel
bootstrap compiler in Rust. It assumes you have read SENTINEL_DESIGN.md
and SENTINEL_DESIGN2.md and have decided to proceed with the staged
validation approach described in Section 16.3 of the design document.

Read this top to bottom once before writing any code. Then use it as a
reference as you work through the milestones.

---

## 0. Current Implementation Status

> This section is the canonical "where the codebase is right now"
> pointer. For per-crate detail and design decisions, read
> docs/STATE.md and the ADRs under docs/decisions/.

**Phase A — sentinel-broker — complete.** Generational arenas
(bump + slab), scoped budgets, diagnostics, recording mode,
secret-memory policy. 69 active tests + 1 doctest. See STATE.md
Section A. ADR 0001 (staged validation) is the umbrella.

**Phase B — sentinel-effects-proto (Sentinel-Mini) — complete.**
Research-grade tree-walking interpreter validating Sentinel's
effect-system design before the production compiler commits.
226 tests (203 lib + 23 integration). All three HANDOVER §5.2
validation demos landed (supply-chain, async-as-effect,
password-verify). The crate is explicitly throwaway per
HANDOVER §5; deletion-eligible once C3 absorbs its lessons.
ADRs 0002-0008 are authoritative. See STATE.md Section B.

**Phase C0 — bootstrap compiler MVP — complete.** The new
production-shape crates (sentinel-syntax, sentinel-ast,
sentinel-codegen, sentinel-driver, sentinel-runtime) now ship a
full lex → parse → AST → two-pass LLVM IR → object → cc-linked
executable pipeline via the `snc` binary. The ADR 0010 appendix
go/no-go program runs:

    fn double(x) { x * 2 }
    fn pick(cond, a, b) { if cond { a } else { b } }
    fn main() {
        let x = 5;
        let y = pick(x, double(x), 0);
        print(y)
    }
    // stdout: "10\n", exit 0

Six sub-phases C0.0-C0.5 shipped across twelve feat+docs commits.
22 pass-test fixtures cover the full surface. ADRs 0009 (Phase C
kickoff) and 0010 (concrete C0 surface) are ACCEPTED. Everything
is `i64` per ADR 0009 ("no type system in C0"); `bool` arrives at
C1.3. See STATE.md Section C.

**Phase C1.0 — Salsa retrofit — complete.**
**Phase C1.1 — sentinel-resolve crate lift — complete.**
**Phase C1.2 — annotation grammar + sentinel-types::check() — complete.**
**Phase C1.3 — bool + i32 + comparison + logical operators; ADR 0010 D9 retired — complete.**
**Phase C1.4 — struct definitions + field access + struct literals — complete.**
**Phase C1.5 — nullable types `?T` + null literal + unwrap_or / is_some builtins — complete (D10 deferred).**
Phase C1 (type system, regions, effects per HANDOVER §6.2) is in
flight per ADR 0011 (PROPOSED, 8 sub-phases, honest 5-6 month
estimate vs HANDOVER's 3-month budget). The C1.0 + C1.1 + C1.2 +
C1.3 + C1.4 + C1.5 sub-phases have all landed:

  - **C1.0a** (09dc8c3): foundation crate `sentinel-base` hosting
    the `#[salsa::db]` SentinelDb trait, `#[salsa::input]`
    SourceFile, and `#[salsa::accumulator]` Diagnostic. Salsa 0.18
    is in the dep graph.
  - **C1.0b** (557cc60): `sentinel-syntax::query` exposes
    `lex_query` and `parse_query` as `#[salsa::tracked]` queries.
    Errors route through the Diagnostic accumulator. sentinel-driver
    instantiates a concrete `SentinelDatabase`.
  - **C1.0c** (8b58644, decision-only commit): codegen stays
    outside the salsa query graph through Phase C1.0. Three options
    weighed in the ADR 0011 D1 amendment; chosen option is "don't
    wrap codegen at all" because (a) it gets rewritten at C1.2 for
    typed HIR, (b) LLVM `'ctx` lifetimes don't trivially fit
    salsa's query model, (c) LSP/check-only tooling that wants
    incremental rebuild exits after types-not-codegen anyway.
  - **C1.1.1** (438dd16): sentinel-resolve crate populated.
    VarId(u32) / FnId(u32) stable identifiers; FnSignature lookup
    table; parallel-tree resolved AST mirroring sentinel-ast's
    Program shape with name references replaced by IDs;
    ResolveError with the 6 name-resolution variants migrated from
    CodegenError; pure `resolve(program)` entry point;
    `resolve_query(db, file)` `#[salsa::tracked]` wrapper chaining
    on parse_query. 21 unit tests (positive paths + each error
    variant + 4 salsa query smoke).
  - **C1.1.2** (9374edf): codegen consumes &ResolvedProgram;
    driver pipeline becomes parse_query → resolve_query → codegen.
    Codegen loses ~200 lines (name resolution is gone).
  - **ADR 0012** (6ab3661, PROPOSED): concrete C1 surface syntax
    — annotation grammar (D1-D4 for C1.2), bool/comparison/logical
    operators (D5-D8 for C1.3), lexer additions (D9), hard-break
    fixture rewrite plan (D10).
  - **C1.2.1** (af16655): lexer `:` token landed per ADR 0012 D9.
  - **C1.2.2** (90965a5): AST gains `TypeExpr` (Spanned<Ident at
    C1.2>), `FnDef.return_type`, `Param.ty`, `StmtKind::Let.ty_annot`
    (Option); parser wires `fn name(p: T) -> T` and optional
    `let x: T = ...`; resolve carries TypeExpr through;
    22 pass-test fixtures + 1 UI fixture mechanically rewritten
    via committed Python script per ADR 0012 D10.
  - **C1.2.3** (ded07bc): sentinel-types crate populated.
    `Type::I64` universe (C1.3 widens), parallel-tree TypedProgram
    with `ty: Type` on every TypedExpr, 4 TypeError variants
    (UnknownType active at C1.2; Mismatch / ReturnTypeMismatch /
    CallArgMismatch dormant until C1.3 multi-type expressions),
    pure `check()` + `#[salsa::tracked]` `check_query`.
  - **C1.2.4** (c9a21ff): codegen consumes &TypedProgram; driver
    pipeline becomes parse_query → resolve_query → check_query
    → codegen. `check_query::accumulated::<Diagnostic>` picks up
    lex / parse / resolve / types diagnostics transitively — full
    four-stage front-end is now query-shaped.
  - **C1.3.1** (2801a81): lexer adds the 11 C1.3 tokens per ADR
    0012 D9 — `true` and `false` keywords, six comparison ops
    (`== != < <= > >=`), three logical ops (`&& || !`). logos's
    longest-match handles the precedence-aware lexing
    (`==` beats `=`, `!=` beats `!`, `<=` beats `<`, `>=` beats
    `>`). 9 new lexer tests.
  - **C1.3.2-4** (cd1c0d4): the bool + comparison + logical
    surface lands end-to-end as a single coordinated commit
    because the AST / resolve / types / codegen parallel-tree
    enums need their exhaustive matches updated together.
    AST gains `ExprKind::BoolLit(bool)`, `ExprKind::Cmp(CmpOp,
    l, r)`, `ExprKind::Logic(LogicOp, l, r)`, `UnaryOp::Not`,
    new `CmpOp` and `LogicOp` enums. Parser inserts the
    or → and → cmp precedence levels per ADR 0012 D7;
    comparisons are non-associative per D6 (chained cmp surfaces
    as `ParseError::ChainedComparison`). Resolve passes the new
    variants through unchanged. Types widens its universe to
    `{ I64, I32, Bool }`; operator-typing rules per ADR 0012 (arith
    rejects bool; cmp same → Bool; logic Bool, Bool → Bool;
    unary `!` Bool → Bool). Codegen drops `i64_type` from the
    ctx in favour of an `llvm_int_type(Type)` helper that picks
    between `i1` / `i32` / `i64`; vars HashMap stores
    `(PointerValue, Type)`; comparisons lower via
    `build_int_compare` with the right `IntPredicate`; logicals
    lower as PHI-based short-circuit; unary `!` is `xor x, 1`.
    Activates the dormant `Mismatch` / `ReturnTypeMismatch` /
    `CallArgMismatch` variants from C1.2. +49 unit tests
    (+7 ast / +21 parser / +18 types / +10 codegen) — clean
    pipeline change with no behavior regressions.
  - **C1.3.5** (ba5fd9d): retires ADR 0010 D9's C-style truthy.
    Type checker now requires `cond.ty == Bool` for `if`;
    codegen drops the legacy compare-NE-zero path (debug_assert
    pins the invariant). Six C0 if-using fixtures rewritten
    mechanically: `if 1` → `if true`, `if 0` → `if false`,
    `if x` (x: i64) → `if x != 0`; c05_go_no_go restructured to
    use `is_positive(x): bool` + `pick(cond: bool, ...)` per
    the ADR 0012 appendix's C1.3 phase-go shape. Seven new
    c13_* pass-test fixtures land (bool_literal, comparison,
    logical_and/or, unary_not, short_circuit_and/or). The
    short-circuit fixtures specifically pin the PHI-based
    codegen — if a future change ever regresses to eager
    evaluation, the side effect of the skipped `print(99)` will
    surface in stdout and the test fails. +8 tests over step 2
    (+1 types if_condition_rejects_non_bool + 7 c13 fixtures).
  - **ADR 0013** (e93635b, PROPOSED→ACCEPTED): concrete C1.4
    surface — struct declaration grammar (D1), postfix field
    access (D2), struct literal grammar (D3) with parser-
    disambiguation D3a (no struct lit in if-cond, parens
    escape), struct types in type position (D4) extending ADR
    0012 D3's "primitives are identifiers" pattern, nominal
    type equality (D5), struct == deferred to C1.5+ (D6),
    recursive-struct detection at type-check time (D7), lexer
    additions (D8: `struct` keyword + `.` token), tuple /
    unit / derives / methods / generics all out of scope at
    C1.4 (D9, D10), `fn main() -> i64` invariant stays (D11),
    phase-go program spec (D12).
  - **C1.4.1** (f34b401): lexer adds `struct` keyword + `.`
    token per ADR 0013 D8. Two new TokenKind variants. logos
    longest-match not relevant — no `..` / `.=` neighbours
    until ranges arrive at C2+. +4 new lexer tests including
    the `structure` / `structured` ident-prefix-vs-keyword
    regression.
  - **C1.4.2-6** (aa8f252): the struct surface lands end-to-end
    as a single coordinated commit because the parallel-tree
    pattern requires it. AST gains `StructDecl` + `StructField`
    at top-level; `Program` gains `structs: Vec<StructDecl>`
    alongside `fns`. New `ExprKind` variants: `StructLit`
    (Rust-style `Name { field: expr, ... }`), `FieldAccess`
    (postfix `expr.field`). Parser handles all of it with a
    new `allow_struct_lit: bool` mode flag for D3a (forbids
    bare struct lit in if-cond; parens escape). The parser also
    gains a `parse_postfix()` wrapper that consumes `.field`
    chains after any atom — sets up the pattern for C1.6
    arrays' `[index]` and C4 methods' `.method()`. Resolve
    adds `StructId(u32)`, struct table built in pass 0 (before
    fn signatures), `ResolveError::RedefinedStruct` +
    `UndefinedStruct`. Types widens the universe to
    `{ I64, I32, Bool, Struct(StructId) }`; `resolve_type_expr`
    looks up Idents against the struct table; check_expr handles
    StructLit (validates field set matches decl, reorders to
    decl order so codegen iterates by index) and FieldAccess
    (validates target is struct, looks up field index); cycle
    detection emits `TypeError::RecursiveStruct` for direct
    + mutual cycles; four new error variants in total
    (FieldAccessOnNonStruct, UnknownField, MissingField,
    RecursiveStruct). **Codegen's value type widens from
    `IntValue<'ctx>` to `BasicValueEnum<'ctx>`** so struct
    values flow through the same machinery; new pass 0 declares
    LLVM struct types; new `llvm_basic_type` helper replaces
    `llvm_int_type` for the storage-type path (int helper
    retained for arithmetic operand coercion); StructLit lowers
    via `build_insert_value` chain from `get_undef`; FieldAccess
    lowers via `build_extract_value`. +63 tests across all
    crates. Five new c14_* pass-test fixtures land
    (struct_basic, struct_nested, struct_in_if,
    struct_bool_field, c14_go_no_go).

  - **ADR 0014** (3cb1238, PROPOSED→ACCEPTED-WITH-AMENDMENTS):
    concrete C1.5 surface — postfix `?T` type syntax (D1),
    `null` keyword literal (D2) with bidirectional context
    inference, implicit `T → ?T` widening at expression position
    (D3), `Type::Nullable(NullableInner)` flat-subset
    representation (D4 — amended from Box<Type> for Copy
    preservation), bidirectional checking infrastructure (D5),
    no-nested-nullables (D6), `==` / `!=` against `null` (D7),
    lexer additions (D8: `null` keyword + `?` token), generic
    builtins `unwrap_or` / `is_some` (D9), recursive-struct
    cycle-check relaxation (D10 — DEFERRED to C1.6+ because
    `?T = { i1, T }` flat representation can't actually break
    cycles without heap), out-of-scope list (D11: pattern
    matching, force-unwrap `x!`, optional chaining `?.`,
    null-coalesce `??`, `?` propagation, flow typing).
  - **C1.5.1** (dff8642): lexer adds `null` keyword + `?` token
    per ADR 0014 D8. Two new TokenKind variants. The `?` token
    is reserved for type-position only at C1.5; the
    expression-position uses (`?.`, `??`, `x!`, `?`
    propagation) are deferred per D11. +6 new lexer tests.
  - **C1.5.2-6** (1d0adae): the nullable surface lands end-to-end.
    AST gains `ExprKind::NullLit` + `TypeExprKind::Nullable`.
    Parser handles the optional `?` prefix in parse_type with
    `ParseError::NestedNullable` rejection for `??T`; null
    literal recognition in parse_atom. Resolve adds
    `ResolvedExprKind::NullLit` and pre-registers the two
    generic builtins (`unwrap_or` at FnId(1), `is_some` at
    FnId(2)); user fns now start at FnId(3). Types widens the
    universe to `{ I64, I32, Bool, Struct, Nullable }` via the
    flat NullableInner subset enum (D4 amendment — keeps Type
    Copy); adds bidirectional `check_expr(expr, expected:
    Option<Type>, ...)` infrastructure; `coerce_to_expected`
    inserts `TypedExprKind::WidenToNullable` wrappers for the
    implicit T→?T widening; `TypeError::AmbiguousNull` for bare
    `let x = null;`; Cmp rule extended for `x == null` /
    `null == x` comparing discriminator bits; unwrap_or /
    is_some special-cased at the Call typing arm with
    type-from-arg inference. Codegen lowers `?T` as LLVM
    `{ i1, T }`; null lit as const `{ i1 false, T zero }`;
    WidenToNullable via build_insert_value; unwrap_or via
    build_select; is_some via build_extract_value(0); Cmp on
    nullable extracts valid bits and compares. Pass 0 splits
    into "declare opaque struct types" then "set bodies" to
    handle forward references through `?Other` fields. +42
    tests across all crates. Six new c15_* pass-test fixtures
    land (null_literal, widen, eq_null, nullable_struct_field,
    maybe_compose, c15_go_no_go).

**C1 next: C1.6** — arrays + bounds checking per ADR 0011 D6.
Estimated 3-4 weeks. ADRs 0013 + 0014 are ACCEPTED (0014 with
amendments); ADR 0015 (concrete C1.6 surface — array literal
syntax probably `[1, 2, 3]`, array type `[T; N]` or `[T]`,
indexing `a[i]`, bounds-check semantics) lands before the
first C1.6 feat commit. C1.6 will likely also introduce the
heap-allocation primitive that unlocks ADR 0014 D10's deferred
recursive-struct relaxation.

**Workspace test count**: 686 active across all crates (+54 over
C1.4: +3 AST, +15 syntax (6 lexer + 9 parser), +4 resolve, +18
types, +8 codegen, +6 pass-test fixtures). All four check-suite
checks green (cargo build --workspace, cargo clippy --workspace
--all-targets -D warnings, cargo test --workspace, cargo test
--workspace --doc). c05 go/no-go (C1.3 bool flow) still runs
end-to-end: stdout "10", exit 0. c14 go/no-go (C1.4 struct flow)
runs: stdout "7", exit 0. c15 go/no-go (C1.5 nullable flow) runs:
stdout "142", exit 0. See STATE.md "Conventions" for the per-crate
breakdown.

**ADR status**:

  - 0001 staged-validation                       ACCEPTED
  - 0002 effect-rows-in-mini                     ACCEPTED
  - 0003 b1-retrospective                        ACCEPTED
  - 0004 row-representation-and-effect-surface   ACCEPTED
  - 0005 effect-inference-judgment               ACCEPTED
  - 0006 default-close-row-variables             ACCEPTED
  - 0007 effect-handlers                         ACCEPTED
  - 0008 secret-qualifier-and-constant-time      ACCEPTED
  - 0009 phase-c-kickoff-and-c0-plan             ACCEPTED (all C0
                                                 sub-phases done)
  - 0010 concrete-c0-surface-syntax              ACCEPTED (all
                                                 D-decisions exercised)
  - 0011 phase-c1-kickoff-and-type-system-plan   PROPOSED — D1 +
                                                 D2 + D3 + D4 + D5 +
                                                 D7 + D10 + D11 all
                                                 fully exercised across
                                                 C1.0a-c (Salsa), C1.1
                                                 (resolve lift), C1.2
                                                 (annotation grammar +
                                                 check), C1.3 (primitive
                                                 widening + truthy
                                                 retirement), C1.4
                                                 (structs + ADR 0013),
                                                 C1.5 (nullables + ADR
                                                 0014); ADR remains
                                                 PROPOSED because D6
                                                 (sub-phase split —
                                                 C1.6+ in flight:
                                                 arrays + heap +
                                                 generics) and D12
                                                 (perf discipline —
                                                 deferred per the ADR)
                                                 are still ahead
  - 0012 concrete-c1-surface-syntax              ACCEPTED — every
                                                 D-decision exercised
                                                 across C1.2-3
  - 0013 concrete-c1-4-struct-syntax             ACCEPTED — every
                                                 D-decision exercised
                                                 across C1.4
  - 0014 concrete-c1-5-nullable-syntax           ACCEPTED-WITH-
                                                 AMENDMENTS — D1-D3 +
                                                 D5-D9 + D11 all
                                                 fully exercised; D4
                                                 amended from
                                                 Box<Type> to a flat
                                                 NullableInner subset
                                                 enum (preserves Type
                                                 Copy); D10 deferred
                                                 to C1.6+ because the
                                                 `?T = { i1, T }`
                                                 flat representation
                                                 can't break cycles
                                                 without heap
                                                 indirection

### 0.1 Working norms (carry forward into Phase C1)

Original Phase-A norms, augmented with Phase-B and Phase-C
lessons:

- **Trust STATE.md, not the git log.** Commit messages are dense
  and miss design rationale. Always read docs/STATE.md and this
  file before doing anything; never infer state from git log alone.

- **Terminal quirk: nested heredocs break.** This developer's
  terminal mangles `<<EOF ... <<INNER ... INNER ... EOF`-style
  scripts. Use one of: (a) base64-encoded python3 -c blocks,
  (b) write a script to /tmp/ via a single non-nested heredoc and
  then execute it, or (c) single non-nested heredocs only.

- **Small patches, build between each.** The session that built
  Phase A7 took four diagnostic/fix iterations because the
  initial patch was too ambitious. Better practice: land the
  type/trait changes first, build, then add the implementations,
  build, then add the tests. Same lesson held for Phase C0:
  small sub-phase commits + cargo test after each beats one big
  commit.

- **Honest disclosure beats confident-but-wrong.** This developer
  values being told when something is uncertain or guessed at
  ("I'm not sure if BudgetScope::within_budget emits BudgetClosed
  on rejection, so I included an assertion to find out") over
  patches presented as definitely-correct that turn out not to be.
  The C1.0b pause is an example: rather than land a half-working
  retrofit, the session committed the working sentinel-base alone
  and documented C1.0b's path forward.

- **Minimal ceremony.** "go", "proceed", short replies are the
  norm. Long preambles are unwelcome.

- **Examples held to -D warnings.** Don't allow lint debt in
  examples; they're educational artifacts. Same for tests/pass/
  fixtures (Phase C).

- **Check before overwriting docs.** When patching documentation
  files via Python, always check `p.exists()` and read existing
  content first. Prefer merge/append patterns for docs/. Phase A
  hard-learned lesson on BACKLOG.md.

New norms learned during Phase B and Phase C:

- **ADR-first per phase boundary.** ADR 0002 was the Phase B
  kickoff; ADR 0009 was Phase C kickoff; ADR 0011 was Phase C1
  kickoff. Each landed PROPOSED before the first feat commit,
  became ACCEPTED at sub-phase completion. Continue the pattern
  for Phase D and beyond.

- **feat + docs commit pairs per sub-phase.** Each sub-phase
  ships as a feat commit (code + tests) followed by a docs
  commit (STATE.md refresh + ADR status updates). The docs
  commit also backfills the hash that the feat commit produced.
  See the C0.0-C0.5 history for the rhythm.

- **The pure-function pipeline discipline (ADR 0009 D1a).**
  C0 held it — `lex`, `parse`, `compile_to_object` are all
  `(input) -> (output, diagnostics)`. C1.0a starts cashing in
  the payoff by retrofitting Salsa. Keep new pipeline stages
  pure-function until salsa wrapping happens at a known
  sub-phase.

- **cargo clippy --workspace --all-targets -D warnings** is
  part of the standard four-check suite alongside build / test
  / test --doc. Don't let clippy debt accumulate; it has caused
  full re-sweep commits before (4182ff6 cleared pre-B4.0 lints).

- **No pushes from the assistant.** Commits land locally; the
  dev pushes via GitHub Desktop in batches. Never run `git push`.

- **macOS-only assumption.** `.cargo/config.toml` hardcodes brew
  paths (LLVM_SYS_180_PREFIX=/opt/homebrew/opt/llvm@18, link
  search at /opt/homebrew/lib + /usr/local/lib). Cross-platform
  is a future concern; right now LLVM 18 must be `brew install`-ed.

- **Mode B working conventions** (from Phase B onward): paste-
  direct zsh; anchor-guarded Python patches via `/tmp/`; no
  nested heredocs; when a Python script needs multi-line Rust
  text put it in a separate `cat > /tmp/foo.txt <<'RSEOF' …`
  block and `read_text()` it from Python (triple-quoted Python
  strings inside a bash heredoc can mangle terminals); cargo
  test -p <crate> after each patch.

### 0.2 Next session opening (C1.6 — arrays + bounds checking + heap indirection)

Resume at **C1.6** per ADR 0011 D6. The C1 primitive surface
+ first compound type (structs) + non-recursive nullables are
complete; C1.6 introduces arrays and the heap-allocation
primitive that unlocks ADR 0014 D10's deferred recursive-struct
relaxation.

**Pre-flight work before any code**: write ADR 0015 *PROPOSED*
covering the concrete C1.6 surface decisions. Patterns to argue:

  - **Array type syntax**: `[T; N]` for fixed-size arrays?
    `[T]` for dynamically-sized? Or both, with different
    semantics? Rust uses `[T; N]` for stack arrays and `Vec<T>`
    for heap. Sentinel might want a simpler "always-heap-
    backed" model at C1.6 with `[T]` syntax — defer the size-
    in-type story to a later ADR.
  - **Array literal syntax**: `[1, 2, 3]` (Rust-style) is the
    obvious choice. No new lexer tokens needed (uses existing
    `[` and `]`).
  - **Lexer additions**: `[` and `]` brackets. Two new tokens.
  - **Indexing syntax**: postfix `a[i]` — extends parse_postfix
    alongside `.field` (precedent from C1.4). Returns `T` for
    array `[T]` indexed by `i64`.
  - **Bounds checking semantics**: per HANDOVER §6.2's
    "obvious memory-safety violations" criterion, out-of-bounds
    indices must be runtime-detected. Options: (a) every index
    becomes `if i < len { a[i] } else { panic }`, (b) compiler
    inserts a check that traps on OOB, (c) bounds-check elision
    when statically provable. C1.6 ships (a) or (b); (c) is C2+.
  - **Heap allocation primitive**: needed both for the array
    storage backing AND for ADR 0014 D10's deferred recursive-
    struct unlock. Likely a `box T` operator or builtin that
    moves a T into heap-allocated storage and returns a pointer.
    The pointer's representation in the type system: a new
    `Type::Heap(Box<Type>)` variant, OR fold it into the
    nullable / array representations directly. TBD in ADR.
  - **Recursive-struct unlock**: once heap is available, the
    cycle detector's D10 relaxation becomes implementable —
    nullable edges that point to heap-allocated structs break
    cycles because heap pointers have fixed size.

Sub-steps once ADR 0015 is in (rough sketch — refine in the
ADR):

1. **Lexer**: `[` and `]` tokens. Possibly a `box` keyword for
   heap allocation. ~2-3 tokens.
2. **AST + parser**: `TypeExprKind::Array(Box<TypeExpr>)`;
   `ExprKind::ArrayLit(Vec<Expr>)`; `ExprKind::Index(target,
   index)`. parse_postfix extends with `[expr]`. parse_type
   extends with `[T]`.
3. **Resolve**: passes through.
4. **Types**: `Type::Array(Box<Type>)` (or flat subset like
   C1.5's NullableInner). Array literal typing requires
   all elements to be the same T. Index typing requires
   target to be array, index to be i64; result is T.
5. **Codegen**: arrays as LLVM pointers + length pairs?
   Or fat pointers `{ ptr, len }`? Needs malloc/free
   runtime additions in sentinel-runtime.

**Estimated effort for C1.6**: 3-4 sessions per ADR 0011 D6.
Larger than C1.5 because heap allocation is genuinely new
runtime territory (malloc/free wiring through
sentinel-runtime); bounds-checked indexing requires a new
codegen pattern (cmp + branch + trap or panic call); array
ABI / by-value vs by-reference is a real design question.

After C1.6: **C1.7** (generics) per ADR 0011 D6. The
remaining C1 budget per ADR 0011 D6 is ~4-6 weeks (just
C1.7) if estimates hold.

C1.5 retrospective (estimate vs actual): ADR 0011 D6 estimated
"2-3 weeks" for C1.5; the actual was ~1 session across 3
commits. Faster than estimated. The pieces that took the most
thought were: (a) the bidirectional checking infrastructure
(threading `expected: Option<Type>` through check_expr while
preserving the more-specific ReturnTypeMismatch /
CallArgMismatch error variants for non-nullable contexts), (b)
the D4 amendment from `Box<Type>` to `Type::Nullable(NullableInner)`
flat subset (keeps Type Copy), (c) the D10 deferral discovery
— the `?T = { i1, T }` flat representation makes recursive
nullable structs infinite-sized in LLVM, so the
recursive-struct unlock waits for C1.6's heap. Notes captured
in STATE.md decisions 65-73.

C1.4 retrospective (kept for reference): ADR 0011 D6 estimated
"3-4 weeks" for C1.4; the actual was ~1 session across 3
commits. The codegen value-type widening from `IntValue<'ctx>`
to `BasicValueEnum<'ctx>` and the D3a parser disambiguation
were the highest-thought-cost pieces. Notes in STATE.md
decisions 54-64.

C1.3 retrospective (kept for reference): ADR 0011 D6 estimated
"2 weeks" for C1.3; the actual was ~1 session across 3
commits. Notes in STATE.md decisions 46-53.

### 0.3 Quick-status block for session start

For pasting into a fresh chat to bootstrap context:

    Continuing Sentinel-lang work. Repo: https://github.com/arcanii/Sentinel-lang
    Local HEAD: <docs-commit> (docs: C1.5 landed; HANDOVER §0 + STATE.md + ADR refresh).
    Branch in sync with origin/main (verify with `git status` at session start).
    Working tree clean.

    Phase A (broker) + Phase B (effects-proto) + Phase C0 (bootstrap
    compiler MVP) + Phase C1.0 (salsa retrofit) + Phase C1.1
    (sentinel-resolve crate lift) + Phase C1.2 (annotation grammar
    + sentinel-types::check) + Phase C1.3 (bool, i32, comparison +
    logical operators; ADR 0010 D9 C-style truthy retired) +
    Phase C1.4 (structs + field access + struct literals; ADR 0013
    ACCEPTED) + Phase C1.5 (`?T` nullables + null literal +
    unwrap_or / is_some builtins + bidirectional checking; ADR
    0014 ACCEPTED-WITH-AMENDMENTS) complete. 686 active workspace
    tests. Three go/no-go programs run end-to-end:
    tests/pass/c05_go_no_go.sentinel (C1.3 bool flow): stdout
    "10", exit 0; tests/pass/c14_go_no_go.sentinel (C1.4 struct
    flow): stdout "7", exit 0; tests/pass/c15_go_no_go.sentinel
    (C1.5 nullable flow): stdout "142", exit 0. Pipeline is
    parse_query → resolve_query → check_query → codegen with
    diagnostics transitively accumulated; codegen value type is
    BasicValueEnum<'ctx>. ADRs 0001-0010 + 0012 + 0013 ACCEPTED;
    ADR 0014 ACCEPTED-WITH-AMENDMENTS (D4 representation amended
    to flat NullableInner subset; D10 deferred to C1.6+); ADR 0011
    PROPOSED with D1 + D2 + D3 + D4 + D5 + D7 + D10 + D11 fully
    exercised. C1 type system covers primitive scalars + nominal
    structs + non-recursive nullables; arrays + heap + generics
    remain.

    Phase C1 in flight per ADR 0011 (PROPOSED, 8 sub-phases).
    C1.0 + C1.1 + C1.2 + C1.3 + C1.4 + C1.5 all landed. Next:
    start C1.6 (arrays + bounds checking + heap indirection) per
    ADR 0011 D6. Begin with ADR 0015 PROPOSED for the concrete
    C1.6 surface (array type/literal syntax, `[i]` indexing,
    bounds-check semantics, heap-allocation primitive). C1.6's
    heap arrival also unlocks ADR 0014 D10's deferred recursive-
    struct relaxation. ~3-4 sessions estimated; see HANDOVER §0.2
    for the rough plan.

    Read docs/HANDOVER.md §0 in full, then docs/STATE.md, then
    ADR 0011 D6 sub-phase budget. ADRs 0009-0010 are historical
    context for C0; 0012 + 0013 are closed-ACCEPTED; 0014 is
    ACCEPTED-WITH-AMENDMENTS. Resume at C1.6 per HANDOVER §0.2.

---

## 1. Scope of This Document

This is not a specification of Sentinel. The design documents are the
specification, and they are still partly under-specified by intent. This
document covers how to *build* the bootstrap compiler: environment,
tooling, architecture, milestones, testing, and the order in which to
attack the work.

The audience is a senior engineer or small team (one to three people)
with Rust experience and some compiler or PL background. If you have not
written a compiler before, plan to spend the first two weeks reading
"Crafting Interpreters" and the rustc dev guide before starting on
milestone zero.

---

## 2. Strategic Approach

Do not start by building the full Sentinel compiler. The design document
explicitly recommends a staged validation: prove the broker idea works
as a Rust library, prove the effects system works as a research
prototype, and only then commit to building the full language. This
handover document follows that staging.

The milestones below are organized as four phases. Phase A and Phase B
are the validation prototypes. Phase C is the bootstrap compiler proper.
Phase D is the path to self-hosting. Each phase has a clear go/no-go
decision point at the end. Do not skip the decision points. If Phase A
produces a broker that nobody wants to use, building Phase C is wasted
effort.

The expected calendar time, with a small focused team, is roughly:
Phase A six months, Phase B nine months (overlapping the second half of
Phase A), Phase C twelve to eighteen months, Phase D another nine to
twelve months. This is honest, not optimistic. Most language projects
underestimate by 2-3x; budget accordingly.

---

## 3. Environment Setup (macOS)

### 3.1 Toolchain

Install Rust via rustup. Use the stable channel for the compiler itself
and pin the version in `rust-toolchain.toml` so contributors get
reproducible builds.

    curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
    rustup default stable
    rustup component add rustfmt clippy rust-analyzer
    rustup target add aarch64-apple-darwin x86_64-apple-darwin

Install LLVM via Homebrew. Pin to a specific major version because
`inkwell` and LLVM's C API are version-coupled.

    brew install llvm@18
    echo 'export PATH="/opt/homebrew/opt/llvm@18/bin:$PATH"' >> ~/.zshrc
    echo 'export LLVM_SYS_180_PREFIX=/opt/homebrew/opt/llvm@18' >> ~/.zshrc

Install supporting tools:

    brew install cmake ninja just ripgrep fd jq
    cargo install cargo-nextest cargo-insta cargo-deny mdbook

`just` is used as the command runner instead of `make`. `cargo-nextest`
is significantly faster than the default test runner for compiler test
suites. `cargo-insta` is used for snapshot testing of compiler output.

### 3.2 Repository Layout

Create a single workspace repository. Do not split into multiple repos
yet; the dependency churn early on will make multi-repo intolerable.

    sentinel/
    ├── Cargo.toml              # workspace manifest
    ├── rust-toolchain.toml
    ├── justfile
    ├── .github/workflows/
    ├── docs/
    │   ├── SENTINEL_DESIGN.md
    │   ├── SENTINEL_DESIGN2.md
    │   └── HANDOVER.md         # this file
    ├── crates/
    │   ├── sentinel-broker/        # Phase A deliverable
    │   ├── sentinel-effects-proto/ # Phase B deliverable
    │   ├── sentinel-syntax/        # lexer + parser + CST
    │   ├── sentinel-ast/           # AST types
    │   ├── sentinel-resolve/       # name resolution
    │   ├── sentinel-types/         # type/region/effect checking
    │   ├── sentinel-hir/           # typed HIR
    │   ├── sentinel-mir/           # SSA-form IR
    │   ├── sentinel-codegen/       # LLVM lowering
    │   ├── sentinel-driver/        # the `snc` binary
    │   ├── sentinel-runtime/       # the broker as runtime library
    │   └── sentinel-lsp/           # language server
    ├── tests/
    │   ├── ui/                     # compile-error tests
    │   ├── pass/                   # programs that should compile and run
    │   └── snapshots/              # insta snapshots
    └── examples/

### 3.3 Initial Workspace Manifest

The top-level `Cargo.toml` declares the workspace and pins dependency
versions centrally. Every member crate inherits from `[workspace.deps]`
rather than declaring its own versions. This avoids version drift, which
is the most common source of pain in multi-crate Rust projects.

Key dependencies to pin from day one: `logos` for lexing, `chumsky` or
hand-written recursive descent for parsing (recommend hand-written for
better error messages), `salsa` for the query engine, `inkwell` for
LLVM, `cranelift` for the debug backend, `bumpalo` and `typed-arena`
for AST allocation, `indexmap`, `rustc-hash`, `smallvec`, `tracing`,
`thiserror`, `miette` for diagnostics, `insta` for snapshot tests.

### 3.4 Build Commands

Define common commands in a `justfile`:

    default: build

    build:
        cargo build --workspace

    test:
        cargo nextest run --workspace

    fmt:
        cargo fmt --all

    lint:
        cargo clippy --workspace --all-targets -- -D warnings

    snc *args:
        cargo run --bin snc -- {{args}}

    check-all: fmt lint test

    bless:
        INSTA_UPDATE=always cargo nextest run --workspace

---

## 4. Phase A — The Broker Prototype

**Goal**: build the memory broker as a standalone Rust crate, ship it,
get real users, learn whether the API actually proves out.

**Duration**: three to six months.

**Go/no-go criterion**: at least three real Rust projects (yours or
external) have adopted the broker for non-trivial work and the API has
stabilized through their feedback. If after six months no one wants to
use it, the broker idea is wrong or the API is wrong, and Sentinel
should pause.

### 4.1 What to Build

The broker crate exposes a `Broker` struct that owns allocation policy
for a process. It provides:

  - Generational arenas with O(1) bulk free and safe dangling-handle
    detection. Handles are `(arena_id, slot_index, generation)` triples.
    Access through a handle checks the generation atomically.
  - Programmable allocation strategies as trait objects. The default
    strategies are bump, slab, and system-malloc, but users can plug in
    their own.
  - Memory budgets with structured-scope semantics. The Rust API uses a
    builder pattern since Rust does not have effect handlers.
  - Statistics queries (live bytes, peak, fragmentation, allocation
    counts by tag).
  - A recording mode that captures every allocation event into a ring
    buffer for deterministic replay.
  - Secret-memory policy with mlock, no-core-dump exclusion, and
    zero-on-free. On macOS this uses `mlock(2)`, `madvise(MADV_NOCORE)`
    where available, and explicit zero with a barrier on free.

What to *defer*: cross-process shared memory (hard, do it in Phase C
when you have the full language to express it). Memory-hard secret
storage (research, post-1.0). Argon2id integration (use the `argon2`
crate as a separate library, not part of the broker yet).

### 4.2 API Sketch

    use sentinel_broker::{Broker, Arena, Budget, Handle, Secret};

    let broker = Broker::new();
    let arena = broker.create_arena("request", 4 * 1024 * 1024);
    let handle: Handle<Request> = arena.alloc(Request::new());

    let req: &Request = handle.get()?; // returns Err if invalidated
    arena.drop(); // all handles into this arena now return Err

    let budget = Budget::new(8 * 1024 * 1024);
    budget.scope(|alloc| {
        let v: Vec<u8, _> = Vec::with_capacity_in(1000, alloc);
        // ...
    }).map_err(|over| /* graceful fallback */)?;

    let key: Secret<[u8; 32]> = broker.alloc_secret([0u8; 32]);
    // key is mlock'd, zeroed on drop, excluded from Debug output

### 4.3 Validation

Write three example programs that use the broker for real work:

  - A small HTTP server with per-request arenas.
  - A parser combinator library that uses bump allocation for AST nodes.
  - A key-value store with the budget API enforcing memory limits.

If writing these is awkward, the API is wrong. Iterate.

Publish the crate to crates.io as `sentinel-broker` once the API feels
stable. Watch what users do with it. The point is to discover whether
the broker concept survives contact with real code.

---

## 5. Phase B — The Effects Prototype

**Goal**: build a small interpreted language with algebraic effects and
the `secret` qualifier, just enough to learn whether the effects-as-
capabilities story works in practice.

**Duration**: six to nine months, can start at month three of Phase A.

**Go/no-go criterion**: you can write a small program that demonstrates
supply-chain capability enforcement, async-as-effect, and constant-time
operations on `secret` data, and the ergonomics feel reasonable.

### 5.1 What to Build

A tree-walking interpreter for a tiny language — call it Sentinel-Mini.
No classes, no regions, no broker integration. The point is to validate
the effect system in isolation.

Required features:

  - Hindley-Milner-style type inference with effect rows.
  - Effect declarations: `pure`, `io`, `network`, `throw`, `await`.
  - Effect handlers with `handle expr with { ... }` syntax.
  - The `secret T` qualifier with constant-time equality and a
    "no branching on secret" check.
  - A capability check: importing a module restricts its effects.

What to *defer*: anything not directly testing the effect system.
Performance is irrelevant; this is a research artifact.

### 5.2 Validation

Write three example programs:

  - A "supply chain attack" demo where importing a JSON parser fails
    because it declares the `network` effect.
  - An async demo where the same function runs synchronously in tests
    and asynchronously in production by swapping the effect handler.
  - A constant-time password verification demo that fails to compile
    if you try to branch on the comparison result.

Publish a short paper or technical report at the end of Phase B
documenting what worked and what did not. This is genuinely useful
output even if Sentinel never proceeds.

---

## 6. Phase C — The Bootstrap Compiler

**Goal**: build the production Sentinel compiler in Rust, targeting the
1.0 subset defined in SENTINEL_DESIGN2.md Section 15.

**Duration**: twelve to eighteen months.

**Go/no-go criterion**: the compiler can compile a non-trivial Sentinel
program (target: a TLS handshake implementation, or an HTTP server)
that exercises all 1.0 features.

### 6.1 Architecture

The compiler is a query-based pipeline built on Salsa. Every phase is
expressed as a memoized query over inputs; incremental recompilation
is foundational, not retrofitted.

The pipeline:

    source file
      -> [sentinel-syntax]    lexer + parser -> CST
      -> [sentinel-ast]       CST -> AST lowering
      -> [sentinel-resolve]   name resolution, module graph
      -> [sentinel-types]     type, region, nullability, secrecy,
                              effect inference and checking
      -> [sentinel-hir]       typed HIR with all qualifiers resolved
      -> [sentinel-mir]       SSA lowering, escape analysis,
                              bounds-check elision, constant-time
                              verification on secret data
      -> [sentinel-codegen]   LLVM IR via inkwell, or Cranelift for
                              fast debug builds

The driver crate (`snc`) wires the queries together and exposes the
command-line interface. The LSP crate (`sentinel-lsp`) reuses the
exact same query engine, which is the entire point of using Salsa.

### 6.2 Implementation Order Within Phase C

Build the pipeline end-to-end for the smallest possible language
subset first, then expand. This is the rustc approach and it works.

**C0 (month 1-3)**: lexer, parser, AST for a subset with only `let`,
arithmetic, `if`, and function calls. End-to-end compilation to LLVM
that produces a runnable binary. No type system yet; everything is
i64. The goal is to prove the pipeline plumbing works.

**C1 (month 3-6)**: bring up the type system. Add `struct`, basic
generics, and references. Implement non-nullable types and the `?T`
optional. Bounds-checked array access. At the end of C1 the compiler
should reject all the "obvious" memory-safety violations.

**C2 (month 6-9)**: regions and ownership. Named regions, second-class
references by default, move semantics, `&` and `&mut` borrows. This is
the hardest single phase; budget pessimistically. Use the Polonius
formulation of borrow checking; it generalizes more cleanly than the
NLL formulation when you add regions.

**C3 (month 9-12)**: effects. Integrate the lessons from Phase B.
Effect inference, effect handlers, async-as-effect, capability
enforcement at the module boundary. Add the `secret` qualifier with
constant-time operations and the speculation-barrier insertion in
codegen.

**C4 (month 12-15)**: classes, traits with named implementations,
delegation, structured concurrency, actors. Most of this is
"reasonable language design plumbing" rather than novel work, but the
volume is significant.

**C5 (month 15-18)**: broker integration, cross-process safety,
reproducible-build guarantees, stable ABI definition, LSP and tooling
polish.

### 6.3 Diagnostics

Diagnostic quality is not optional. The borrow checker, region
checker, and effect checker will produce confusing errors by default,
and Sentinel's whole pitch depends on these errors being
comprehensible. Use `miette` for rich diagnostics from day one.
Allocate at least 15% of compiler engineering time to error message
quality. Steal Elm's and Rust's diagnostic conventions shamelessly.

Every error should answer three questions: what is wrong, why is it
wrong, and what should I do about it. Test diagnostics with snapshot
tests so regressions are visible in PRs.

### 6.4 Testing Strategy

Three layers:

  - **Unit tests** in each crate for individual functions and types.
    Standard Rust practice.
  - **UI tests** in `tests/ui/`. Each is a Sentinel program plus an
    expected stderr. Modeled on rustc's UI test suite. These catch
    regressions in diagnostics and in what the compiler accepts or
    rejects.
  - **Execution tests** in `tests/pass/`. Each is a Sentinel program
    plus expected stdout. The test runner compiles and runs the
    program and compares output.

Use `cargo-insta` for snapshot management. Every PR runs the full
suite via `cargo nextest`. CI fails on any unblessed snapshot
difference.

### 6.5 Performance Targets

Compile time is part of the value proposition. Set targets early and
measure continuously:

  - Clean build of a 10K-line program: under 30 seconds.
  - Incremental build after a one-line change: under 1 second.
  - LSP "go to definition" latency: under 50ms p95.

These are aspirational but they shape architecture decisions. If you
hit a fork in the road, take the path that preserves these targets.

---

## 7. Phase D — Self-Hosting

**Goal**: rewrite the compiler in Sentinel, reach the four-stage
fixed point described in SENTINEL_DESIGN.md.

**Duration**: nine to twelve months after Phase C completes.

**Go/no-go criterion**: stage-three compiler compiles its own source
to a binary that, fed its own source, produces a byte-identical
binary.

### 7.1 Staging

Follow the four-stage plan from the design document exactly. Do not
attempt to self-host all at once; the half-and-half configurations
(Sentinel parser feeding Rust type checker, etc.) are what surface
language ergonomics problems.

Stage one is the easiest and most informative: port the lexer and
parser. If writing the parser in Sentinel is unpleasant, the
language is wrong, and you find out cheaply.

### 7.2 Keep the Rust Bootstrap Alive

Do not delete the Rust bootstrap when self-hosting succeeds. Maintain
it indefinitely as a reproducibility anchor and a defense against
trusting-trust attacks. Pin which Sentinel version the self-hosted
compiler is written in, separately from which Sentinel version it
implements. Every Sentinel release should be buildable from the Rust
bootstrap.

---

## 8. Open Questions to Resolve Early

These are listed in design document Section 18 but they need
*decisions* before Phase C, not just acknowledgment.

**Effects with traits**: can trait methods declare effects
polymorphically? If yes, design the row-polymorphism story now. If no,
document the workaround. This decision shapes the entire type system
and must be made before C3.

**Region inference vs explicit regions**: the design says "named,
visible regions" but practical ergonomics likely require some
inference. Decide where the line is before C2. Recommend: regions are
inferable within a function body but must be explicit at function
boundaries when more than one region is involved.

**Async runtime**: even with effects-as-async, you need a default
scheduler. Will Sentinel ship its own, or wrap an existing one (Tokio
via FFI)? Decide before C3. Recommend: ship a minimal scheduler in
the standard library, allow user-defined schedulers via effect
handlers.

**Stable ABI scope**: a stable ABI for the whole language is
extremely ambitious. Restrict to `extern "sentinel-stable"`
declarations explicitly, like Swift did with `@frozen`. Decide the
exact subset before C5.

**Generic dispatch default**: witness tables vs monomorphization.
The design says witness tables by default, but measure both on
realistic code before committing. Decide before C1.

Document each decision in `docs/decisions/NNNN-title.md` using the
Architecture Decision Record format. Future contributors will need
the reasoning, not just the outcome.

---

## 9. Team and Process

### 9.1 Minimum Viable Team

A realistic minimum is two senior engineers with compiler experience
plus one engineer doing tooling, build infrastructure, and developer
experience. A single person can do Phase A and start Phase B but
cannot realistically complete Phase C alone in any reasonable
timeframe.

If you only have one person, do Phase A, do a reduced Phase B, and
write a thorough postmortem. That alone is a significant contribution.

### 9.2 Process

Use a monorepo. Use trunk-based development with short-lived feature
branches. Require PRs to pass `just check-all` before merge. Require
ADRs for any decision that touches language semantics. Hold a weekly
design review focused on the open questions in Section 8.

Do not chase contributors aggressively in the first year. A small
focused team makes faster progress than a large unfocused one, and
language projects are particularly vulnerable to bikeshedding when
contributors arrive before the core design is stable.

### 9.3 Communication

Maintain a public design log as `mdbook` in `docs/`. Every significant
decision lands as a chapter. Publish quarterly progress reports. This
discipline forces clarity on what you actually built versus what you
planned, and it builds the credibility needed when you eventually
want adopters.

---

## 10. Day One Checklist

When you sit down to actually start:

  - Clone or create the `sentinel` repository with the layout in
    Section 3.2.
  - Install the toolchain from Section 3.1.
  - Copy SENTINEL_DESIGN.md, SENTINEL_DESIGN2.md, and HANDOVER.md
    into `docs/`.
  - Initialize the workspace with empty crates matching Section 3.2.
  - Set up CI to run `just check-all` on every PR.
  - Create `docs/decisions/0001-staged-validation.md` recording the
    decision to follow the Phase A through D plan.
  - Start Phase A milestone one: scaffold `sentinel-broker` with
    the `Broker::new()` constructor, the simplest possible arena, and
    a test that allocates and frees a value.

Ship something on day one. The hardest part of starting a multi-year
project is starting; the rest is iteration.

---

## 11. What to Do When Stuck

You will get stuck. Specific places it tends to happen:

  - **Borrow checker design** in C2. Read the Polonius papers, read
    the rustc dev guide chapter on NLL, look at how Hylo handles
    second-class references. Allocate four weeks of design time
    before writing code.
  - **Effect inference** in C3. Read the Koka and Effekt papers. The
    row polymorphism formulation is the standard one; implement it
    even though it is harder than the alternatives, because the
    alternatives do not compose.
  - **LLVM integration** anywhere. `inkwell` papers over most of the
    pain but not all. When in doubt, write the LLVM IR by hand first,
    confirm it does what you want, then figure out how to generate it
    from `inkwell`.
  - **Diagnostics that confuse users**. Find five people unfamiliar
    with Sentinel, show them the error, ask them to explain it. Their
    confusion is more informative than any internal review.

The general rule: when stuck for more than three days, write the
problem down as a design document, share it for review, and timebox
the resolution. Languages die from indecision more often than from
bad decisions.

---

## 12. Closing

Sentinel is an ambitious project, and the honest assessment in
SENTINEL_DESIGN2.md applies: most language projects at this level of
ambition do not reach widespread adoption. That is not a reason not to
build it. The ideas — programmable runtime, regions, effects-as-
capabilities, the `secret` qualifier — are worth exploring even if the
full language never ships at scale. Each phase produces value
independently. Each phase has a clear go/no-go decision. Each phase
teaches you something the next phase needs.

Build Phase A. See what happens. Decide from there.

Good luck.

*End of document.*
