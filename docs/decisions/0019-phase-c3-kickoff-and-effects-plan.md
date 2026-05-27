# ADR 0019: Phase C3 kickoff — effect rows, secrets, handlers (effect-system integration from Phase B)

Status: PROPOSED — to flip to ACCEPTED (or ACCEPTED-WITH-AMENDMENTS)
as the C3 sub-phases land. C3 is the second-hardest single
Phase C sub-phase per HANDOVER §6.2 (C2 was the hardest);
this ADR is intentionally written before any C3 code so the
substantive design decisions — effect-row representation,
secret qualifier, handler runtime — can be challenged on
paper.

Date: 2026-05-28
Related:
  - **0011** (Phase C1 kickoff — ACCEPTED): the parallel-tree
    pattern + ADR-first norm + interned-instance trick all
    inherited here. ADR 0019 continues the streak.
  - **0017** (Phase C2 kickoff — ACCEPTED-WITH-AMENDMENTS):
    the lexical-first-then-Polonius-later precedent applies
    here too — Sentinel's pattern for "ship the typing layer
    minimum, defer the runtime / precision layer." C3 mirrors:
    ship effect *typing* + secret *typing* at minimum; defer
    handler *runtime* + constant-time codegen + async runtime
    to follow-on ADRs.
  - **0002-0008** (Phase B effect-system design — all
    ACCEPTED): the research-grade interpreter `sentinel-effects-
    proto` validated this design across ~226 tests + three
    HANDOVER §5.2 validation demos. C3 absorbs the *design*;
    rebuilds the *implementation* against the production
    compiler shape per the Phase B crate's "throwaway by intent"
    framing.

## Context

Phase C2 closed with the type system covering primitive scalars
+ nominal structs + nullable types + heap-backed arrays +
witness-table generics + references + mutability + lexical
borrow check + RAII drop. The C1 + C2 universe is `{ I64, I32,
Bool, Struct, Nullable, Array, TypeParam, GenericInstance, Ref }`
— preserving `Type: Copy + Hash` across five interner-table
ADRs (0014 + 0015 + 0016 + 0017 + this one).

C3 introduces the effect-system layer per HANDOVER §6.2's
month 9-12 budget: "Integrate the lessons from Phase B. Effect
inference, effect handlers, async-as-effect, capability
enforcement at the module boundary. Add the `secret` qualifier
with constant-time operations and the speculation-barrier
insertion in codegen."

The Phase B research-grade interpreter (`sentinel-effects-
proto`) validated three substantive design points:

  - **Effect rows** (ADR 0004): Remy-style records with open vs
    closed tails. The judgment becomes `(Subst, Ty, Row)`. Each
    fn arrow carries a row; inference flows effect contributions
    through expressions. Default-close at top level per ADR
    0006: unconstrained row variables in `infer_top`'s return
    become `Row::Empty`, preserving polymorphism inside let-
    bindings but closing the outermost surface.

  - **Effect handlers** (ADR 0007): `handle e with { L(x, k) =>
    body, return v => ret }` — deep, one-shot continuations via
    free-monad-style frame reification. The runtime returns
    `Step::Value(v)` or `Step::Op { label, arg, kont }`; each
    evaluation frame prepends itself to `kont` and re-raises on
    `Step::Op`. Handlers match by label; resumption wraps the
    handler around the continuation's tail so handling is deep.

  - **Secret qualifier** (ADR 0008): `Ty::Secret(Box<Ty>)` as a
    type constructor + `declassify(e)` as a special form. Four
    static constant-time rejections: `SecretBranch` (if-condition
    secret), `SecretDivisor` (div/mod by secret),
    `SecretFlow` (public/secret unification failure),
    `SecretEscapesPolymorphism` (TyVar bound to a Secret). No
    runtime CT enforcement in Phase B — tree-walking interpreter
    is the wrong layer.

What the bootstrap compiler must build vs what Phase B already
proved:

  - **What Phase B proved**: the typing rules are sound + the
    inference algorithm + the diagnostic surface all work. The
    research-grade interpreter is the reference implementation.
  - **What C3 must build new**: production-shape integration
    with `sentinel-types::Type` (interner pattern), salsa-
    pipeline plumbing (new `effect_check_query`), codegen for
    secret (constant-time guarantees) + handlers (CFG-level
    lowering of continuations). Most of this is mechanical
    given the typed reference impl.

The C2.5 retrospective noted that C2's "1-session per sub-
phase" rhythm didn't fully carry — the borrow checker was
genuinely novel machinery. C3 has a different shape: the
typing-layer pieces (effect rows, secrets) port from a working
reference impl; the runtime-layer pieces (handlers, CT codegen)
are genuinely new. The split mirrors the lexical-first /
Polonius-later precedent from ADR 0017 D6.

The C2.5 lexer state at the start of C3:

  - **Keywords**: `let, fn, if, else, true, false, struct,
    null, mut`
  - **Punctuation**: `+ - * / = ( ) { } [ ] , ; : . ? & ->`
    `== != < <= > >= && || !`
  - **`!` already exists as the logical-not unary prefix**;
    C3's effect-row syntax `-> T ! { Op1, Op2 }` needs positional
    disambiguation (postfix after `->T`, distinct from prefix
    `!cond`).

## Decision

Fourteen D-numbered sub-decisions covering effect-row syntax
(D1), inference vs annotation (D2), type representation (D3),
effect declarations (D4), secret qualifier (D5), declassify
form (D6), constant-time check (D7), effect handlers — deferred
at C3 minimum (D8), async-as-effect — deferred (D9), lexer
additions (D10), pipeline shape (D11), out-of-scope items
(D12), `fn main` invariant (D13), and the phase-go program
(D14).

### D1. Effect annotations on fn signatures: `fn name(...) -> T ! { Op1, Op2 }`.

Optional postfix annotation following the return type:

    fn_def       = 'fn' name type_params? '(' params ')'
                   '->' type ('!' '{' effect_list '}')? block
    effect_list  = ident (',' ident)*

Examples:

    fn pure_compute(x: i64) -> i64 { ... }              // no effects
    fn read_file(path: i64) -> i64 ! { Io } { ... }     // one effect
    fn launch_missiles(x: i64) -> i64 ! { Io, Nuke } { ... }

The `!` token is already lexed (C1.3's logical-not). The parser
disambiguates by position: postfix `!` immediately following a
type expression at fn-signature position parses as the effect-
annotation arm; prefix `!` at expression position parses as
logical-not. Same precedent as C2's positional disambiguation
of `*` (deref vs multiply) and `&` (borrow-take vs reference-
type-prefix).

The effect list is a comma-separated list of effect identifiers
in braces. Empty braces (`! { }`) is **disallowed at parse**:
write the no-effect case as no annotation at all (don't write
`! { }`). Single-effect case uses `! { Op }`.

### D2. Effect inference: full inference; annotations checked against inference.

Phase B's full-inference design carries forward. The type
checker computes the effect row of every fn body by recursing
through call sites + `perform` (when handlers land at D8) +
runtime-builtin effects. **If the fn has an annotation**, the
inferred row is checked against the annotated row (set
equality, modulo ordering): an annotation is a *contract* the
fn declares to its callers, not a free declaration. **If the
fn has no annotation**, the inferred row is the row.

This is the bidirectional shape: inference is the source of
truth; annotations are checked, not requested. The Phase B
interpreter never had user-written row annotations on arrow
types — C3 introduces them for the surface but treats them as
constraints, not declarations.

Two error variants land at the C3.2 sub-phase:

  - `EffectError::EffectAnnotationMismatch { fn_name, declared,
    inferred }` — fn declared `! { Io }` but body inferred
    `! { Io, Net }`. Suggests either (a) widen the annotation,
    or (b) wrap the offending call site in a handler.
  - `EffectError::UnhandledEffect { fn_name, effect, source }`
    — main has unhandled effects at C3 minimum (D13). Other
    sites with unhandled effects surface this once handlers
    land (D8).

### D3. Type representation: interned `Type::Effect` row entries; rows separate from types.

Phase B's row representation was structural — `Row::{Empty, Var,
Cons}` with `Box`. Translating this directly to `sentinel-types`
would break `Type: Copy + Hash` (the `Cons` variant's `Box<Ty>`
payload precludes it). The interner pattern Sentinel has used
across five ADRs (0014/0015/0016/0017 plus this one) extends
naturally:

  - **`RowId(u32)`** — Copy + Hash newtype; indexes into a
    program-level `Vec<RowData>`.
  - **`RowData { effects: Vec<EffectId>, open: Option<RowVarId> }`**
    — the resolved row at the end of inference. The structural
    form (`Empty | Var | Cons`) lives inside the type-checker's
    working `unify_row` machinery; the *interned form* is what
    flows through the parallel-tree TypedProgram.
  - **`EffectId(u32)`** — names the effect (e.g., `Io = 0`,
    `Net = 1`). Looked up via `TypedProgram::effect_decl(id)`.
  - **`RowVarId(u32)`** — fresh row variables produced during
    inference; closed-over by default-close per ADR 0006 D1
    before the row reaches `TypedProgram`.

This is *not* `Type::Row` as a Type variant. Effect rows attach
to **fn signatures**, not to arbitrary types — Sentinel doesn't
have first-class fns yet, so rows only appear in
`TypedFnSignature.effect_row: RowId`. If first-class fns arrive
later, `Type::Fun(...)` will gain a row payload (probably also
interned).

The `Type` universe stays `{ I64, I32, Bool, Struct, Nullable,
Array, TypeParam, GenericInstance, Ref, Secret }` (Secret added
per D5; Row stays out of `Type`).

### D4. Effect declarations: `effect Io { read() -> i64; write(x: i64); }`.

Top-level effect declarations introduce a labeled operation
set:

    program       = (fn_def | struct_decl | effect_decl)*
    effect_decl   = 'effect' Ident '{' op_decl (';' op_decl)* ';'? '}'
    op_decl       = Ident '(' params ')' ('->' type)?

Each declared `effect E { ... }` introduces an `EffectId`. Each
`op_decl` inside introduces an operation under that effect with
its own signature. At C3 minimum (no handlers per D8),
operations are **declared but not invocable**: `perform E.read()`
parses but type-checks as `EffectError::PerformDeferred` until
D8 lands. Effects can still be **named** in fn signatures (D1)
and runtime-builtins can be declared to carry them (e.g.,
`print: !{ Io }`).

This is intentional: C3 minimum ships the *machinery* (typing,
inference, signatures, secret) but defers the *runtime* (handler
dispatch, continuation reification). Compare ADR 0017's
"lexical borrow checker first; Polonius later" call.

Effect declarations don't have type parameters at C3 minimum.
Effect polymorphism (Koka's `effect E<T>`) is deferred to a
follow-on ADR.

### D5. Secret qualifier: `Type::Secret(SecretId)` interned.

Sentinel adopts Phase B's `Ty::Secret(Box<Ty>)` design with the
interner adaptation:

  - **`Type::Secret(SecretId)`** joins the Type universe.
  - **`SecretId(u32)`** indexes `TypedProgram.secrets:
    Vec<SecretData>`.
  - **`SecretData { inner: Type }`** carries the wrapped type.

Surface syntax (D6's lexer adds the `secret` keyword):

    type         = base_type | '?' type | '[' type ']'
                 | '&' 'mut'? type
                 | 'secret' type
                 | Ident type_args?

`secret secret T` is rejected at parse with `ParseError::DoubleSecret`
(matches Phase B B4.0c).

`secret` composes with the existing depth-1-ish constructors:

  - `secret &T` — secret reference (the *value* of the ref is
    secret; the ref itself is public). Allowed.
  - `& secret T` — reference to a secret. Allowed.
  - `?secret T` — nullable secret. Allowed.
  - `secret ?T` — secret nullable. Allowed; the optionality is
    secret-flavored.
  - `[secret T]` — array of secrets. Allowed.
  - `secret [T]` — secret array. The array's length + data are
    secret; reading any element produces `secret T`. Allowed.

The `Type: Copy` invariant survives — `SecretId` is `u32`. The
interner table lives alongside `refs:`, `generic_instances:`,
`structs:` on `TypedProgram`.

### D6. Declassify: `declassify(e)` as a special form.

The escape hatch from secrecy: `declassify(e)` strips the
secret wrapper:

    declassify_expr = 'declassify' '(' expr ')'

Typing rule: `e : Type::Secret(SecretId)` where `secret.inner
= T` produces an expression of type `T`. Lexer adds the
`declassify` keyword (D10). Mandatory parens (matches Phase B
B4.0c).

Declassification is a deliberately-noisy operation: it surfaces
in code review + IDE highlighting. Sentinel does NOT auto-
declassify on any operation. The only path from `secret T` to
`T` is through explicit `declassify`. This is the design call
from ADR 0008 D5.

### D7. Constant-time check: five static rejections.

Port Phase B's four CT rejections + add one for the C2 ref
layer:

  1. **`SecretBranch`** — `if cond { ... } else { ... }` where
     `cond : secret bool`. Conditional branching on secret data
     leaks via timing. Reject.
  2. **`SecretDivisor`** — `a / b` or `a % b` where `b :
     secret i64`. Variable-time division on secret leaks via
     timing. Reject.
  3. **`SecretFlow`** — public/secret unification failure.
     Fn declared `fn(x: i64) -> i64` called with `secret i64`
     fails to unify; explicit declassify required.
  4. **`SecretEscapesPolymorphism`** — generic fn instantiated
     with a Secret type AND used in a non-secret-preserving way
     (operator returns the bare type). Phase B's variant.
  5. **`SecretInRefDeref`** — `*r` where `r : &secret T` is
     OK (produces `secret T`); but `*r` where `r : secret &T`
     deref'ing a secret reference is rejected because the
     dereference target depends on the secret pointer (cache-
     side-channel risk). The pointer itself being secret is
     fishy. C3 minimum rejects; future ADR may relax with a
     CT-aware codegen path.

These are static; no runtime CT enforcement at C3 minimum.
Branch-free codegen for secret comparisons (the `select` /
`cmov` lowering) is a follow-on ADR alongside the speculation-
barrier insertion HANDOVER §6.2 calls for.

### D8. Effect handlers: deferred to C3.last or follow-on ADR.

`handle e with { Op(x, k) => body, return v => ret }` from
Phase B's ADR 0007 is a substantive runtime piece. At C3
minimum:

  - The **surface** (handler grammar + perform expression) is
    declared but **rejected at type-check**: any `handle` /
    `perform` site surfaces `EffectError::HandlersNotYet`.
  - The **typing rules** (effect-row discharge by handlers) are
    not yet implemented; effect rows on fn signatures are
    declared by D1 + D2 but only flow through call sites — there
    is no way to "discharge" an effect at C3 minimum, so any
    use site of an effect-bearing fn propagates the effect up to
    `fn main` where it surfaces as `UnhandledEffect`.

The lowering strategy (free-monad reification per Phase B vs
CPS transform vs stack-saved continuations) is a substantial
design call that justifies its own ADR. C3 ships the typing
layer; handler runtime lands in C3.last or a separate ADR
(provisionally 0020).

Reasoning: C2 split lexical-borrow-check from Polonius for
exactly this kind of risk-management. The effect-typing layer
is useful on its own (secret + constant-time + effect-aware
signatures + main-must-be-pure rejection) without the handler
runtime. Bundling them risks overshooting the per-sub-phase
budget the way C2.2 + C2.3 + C2.4 each used a full session.

### D9. Async-as-effect: deferred.

Async is one of Phase B's three validation demos (HANDOVER §5.2).
The Phase B impl modeled async as just another effect; the
runtime piece (futures, schedulers, the I/O loop) is wholly
new infrastructure. Defer to post-C3.

### D10. Lexer additions at C3.

Five new keywords + one repurposed token + one already-
present token:

| Token / Keyword | Notes                                          |
| --------------- | ---------------------------------------------- |
| `effect`        | Keyword — top-level effect declaration         |
| `secret`        | Keyword — secret type prefix                   |
| `declassify`    | Keyword — declassify special form              |
| `handle`        | Keyword — handler expression (reserved at C3;  |
|                 | usable at C3.last / 0020)                      |
| `with`          | Keyword — pairs with `handle`                  |
| `perform`       | Keyword — perform operation (reserved at C3)   |
| `!`             | Already lexed (C1.3 logical-not); reused for   |
|                 | postfix effect-row annotation per D1           |
| `;`             | Already lexed; reused for operation separator  |
|                 | in `effect E { ... }` blocks                   |

`return` is **NOT** added at C3. Phase B used `return v =>
ret_body` in handler arms; C3.last (D8 closure) will revisit.
`return` may stay implicit (the last arm without a label
pattern is the return arm) or pick up the keyword then.

### D11. Pipeline shape: effect_check_query between check_query and borrow_check_query.

C3 adds a new salsa-tracked query:

    parse_query → resolve_query → check_query
        → effect_check_query → borrow_check_query → codegen

`effect_check_query` consumes `TypedProgram` + returns
`EffectCheckedProgram` (parallel-tree adding effect-row +
secret-flow metadata) or `Vec<EffectError>` via the Diagnostic
accumulator. Diagnostics still propagate transitively.

The position matters: effects influence typing's call-site
inference (call to `read_file()` propagates `! { Io }` to the
caller's row); secret typing fits naturally inside or just
after the regular type-check. Sentinel's choice is **a
separate query**, run after `check_query`, so that:

  - The type-checker stays focused on `Type` unification
    without effect-row baggage.
  - Effect-check has full access to typed bodies + can produce
    its own diagnostic surface.
  - Future LSP / tooling can opt out of effect-check (e.g., a
    "show inferred types" mode that doesn't care about effect
    inference).

This is the same shape as `borrow_check_query` from C2 — a
new pass slotted into the existing pipeline.

### D12. Out of scope at C3.

The following are explicitly deferred:

  - **Effect handler runtime** (`handle ... with { ... }` +
    `perform Op` evaluation). D8 covers the deferral. Lowering
    strategy (free-monad reification / CPS / stack-saved) is
    its own ADR.
  - **Async runtime** (futures, schedulers, I/O loop). D9.
  - **Capability enforcement at module boundaries**. HANDOVER
    §6.2 calls this out as C3 work; ADR 0019 D12 defers
    because Sentinel doesn't have a module system yet. Lands
    in the module-system ADR.
  - **Constant-time codegen** (branch-free `select` / `cmov`
    for secret comparisons; speculation-barrier insertion).
    Separate codegen-side ADR.
  - **Row-polymorphism syntax at the surface** (e.g.,
    `fn foo<ρ : Effect>(x: i64) -> i64 ! { ρ }`). Default-close
    per ADR 0006 D1 hides this at the type checker; surface
    syntax is deferred.
  - **Effect polymorphism via type parameters on `effect`
    declarations** (Koka-style `effect E<T>`). Deferred.
  - **User-defined `return` arms in handlers** with non-
    identity transformations. Deferred alongside D8.
  - **Operations with multi-arg signatures**. Phase B handlers
    used single-arg ops (`L(x, k) => body`); generalizing to
    `L(x, y, k) => body` is mechanical but adds parser
    complexity. Deferred until D8.
  - **Multi-shot continuations**. Phase B used one-shot
    continuations; multi-shot needs deep-copy machinery the
    bootstrap compiler doesn't have. Phase D territory.
  - **Effect aliasing / hierarchy** (`Net <: Io` style). Deferred.

### D13. `fn main` invariant tightens: main must be effect-free.

`fn main() -> i64` must have effect row `{}` (empty). Any
effect that propagates to main without being discharged
surfaces as `EffectError::UnhandledEffect { fn: "main",
effect, source_span }`. Since handlers are deferred at C3
minimum (D8), this means **at C3 minimum, calling an effect-
bearing fn from main rejects the program** until either (a)
the call is wrapped in a `handle` (D8 closure) or (b) the
called fn's effect annotation is removed (which makes it a
lie, also rejected by D2's annotation check).

This is the C3 minimum's main constraint. It's what makes the
phase-go program (D14) ship-able without handlers: secret
typing + effect-typing-rejection-at-main.

### D14. Phase-go program spec.

At `tests/pass/c30_go_no_go.sentinel`. Exercises D1-D7 + D13
without needing handlers (D8 deferred):

    effect Io {
        log(msg: i64) -> i64;
    }

    fn double_secret(x: secret i64) -> secret i64 {
        x + x
    }

    fn check_password(stored: secret i64, attempt: secret i64) -> secret bool {
        stored == attempt
    }

    fn main() -> i64 {
        let pw_stored: secret i64 = secret(42);
        let pw_attempt: secret i64 = secret(42);
        let matched: secret bool = check_password(pw_stored, pw_attempt);
        let public: bool = declassify(matched);
        if public { 1 } else { 0 }
    }

Expected: stdout `1\n`, exit 0. The `secret(...)` constructor
form is itself a Phase B-style intrinsic (D5+ implementation
detail — may be `secret i64 = 42 as secret i64` or a literal
form; ADR 0008 D2 had the constructor question; C3 picks). The
program exercises secret typing (D5), declassify (D6), constant-
time check passing (D7 — the if-condition is bool, not secret
bool — declassify happened first), and effect annotation absence
(D1: no `!` on signatures).

Negative companion at `tests/ui/c30_secret_branch.sentinel`:

    fn main() -> i64 {
        let secret_flag: secret bool = secret(true);
        if secret_flag { 1 } else { 0 }   // ERROR: SecretBranch
    }

Expected: snc rejects with `sentinel::effect::secret_branch`
+ exit code 1.

A second phase-go at `c30_effect_propagation.sentinel`:

    effect Io { log(msg: i64) -> i64; }

    fn maybe_log(x: i64) -> i64 ! { Io } {
        x + 1
    }

    fn main() -> i64 {
        maybe_log(5)   // ERROR: UnhandledEffect at main
    }

Expected: snc rejects with `sentinel::effect::unhandled_effect`
+ exit code 1. Confirms D13 — effects bubble up to main and
get rejected since D8 (handlers) isn't there to discharge them.

## Sub-phase split

A rough split into 4-5 sub-phases. Each sub-phase ADR-first
per the C1 + C2 rhythm; concrete D-decisions refined in the
sub-phase ADRs (mostly mechanical; expect minor amendments).

| Sub  | Title                                                          | Estimate     | Status |
|------|----------------------------------------------------------------|--------------|--------|
| C3.0 | Lexer additions (`effect`, `secret`, `declassify`, `handle`,   | 1 session    | next   |
|      | `with`, `perform`); AST + parser for effect_decl, effect       |              |        |
|      | annotations on fns, secret type prefix, declassify expr.       |              |        |
| C3.1 | Secret qualifier landing in `sentinel-types`: `Type::Secret    | 1-2 sessions |        |
|      | (SecretId)` + SecretData + intern_secret; resolve passes       |              |        |
|      | through; check_query computes Secret types via unification     |              |        |
|      | (a Secret wrapper unifies with bare T iff explicit declassify  |              |        |
|      | strips it); five CT rejections per D7. Codegen lowers          |              |        |
|      | Secret(T) identically to T at C3.1 minimum (the constant-      |              |        |
|      | time guarantees come in a follow-on codegen ADR per D12).      |              |        |
| C3.2 | Effect rows + effect_check_query salsa pass; effect            | 2-3 sessions |        |
|      | inference (full inference per D2) + annotation check; D13's    |              |        |
|      | "main must be effect-free" rejection.                          |              |        |
| C3.3 | Polish + phase-go programs + STATE.md / HANDOVER refresh +     | 0-1 sessions |        |
|      | ADR 0019 PROPOSED → ACCEPTED flip. Phase C3 close-out (at      |              |        |
|      | the typing-layer minimum per this ADR's D8 deferral).          |              |        |
| C3.4 | (DEFERRED to follow-on ADR 0020): Effect handler runtime —     | 3-5 sessions |        |
|      | `handle ... with { ... }` lowering + perform reification +     |              |        |
|      | continuation runtime. Substantial; justifies its own ADR.      |              |        |

Honest total at C3 minimum (without handler runtime):
**~4-6 sessions across 4 sub-phases**. Lower bound than C2's
6-13 estimate because:

  - The reference impl (`sentinel-effects-proto`) gives us
    working typing rules + inference algorithm + diagnostic
    surface. Most of the C3.1 + C3.2 work is mechanical
    porting + parallel-tree integration.
  - The substantive new piece (handler runtime + CT codegen)
    is explicitly deferred to follow-on ADRs.

Including the C3.4 handler runtime would push to 7-11 sessions
total; ADR 0020 will refine. The C2.5 retrospective's
"infrastructure compounded on surrounding pieces; substantive
work was concentrated in the borrow checker" pattern likely
applies again: the lexer + AST + parser deltas are trivial;
the substantive work is in `sentinel-types` (secret/effect
unification) + the new `sentinel-effect-check` crate.

## Reasoning

The decisions cluster around five themes.

**Minimum-viable C3 surface.** D1-D7 ship the typing layer:
effect rows + secret typing + the five static CT rejections.
D8-D9 defer the runtime / async layers. D12 documents what's
deliberately punted. The C3 win is "the language gains effect-
aware signatures + a sound secret-flow check"; the long-term
handler-runtime + async story follows.

**Lexical-first / Polonius-later precedent.** ADR 0017 D6
shipped the borrow checker with the simplest-correct-enough
formulation (lexical) and explicitly deferred the precise
formulation (Polonius) to a follow-on ADR. C3 mirrors: ship
effect *typing* + secret *typing* at minimum; defer effect
*runtime* (handlers) + constant-time *codegen* to follow-on
ADRs. The pattern is "ship the typing layer first; iterate
the runtime layer in a separate sub-phase or ADR."

**Continue the C1 + C2 design patterns.** D3 + D5 (interned
`RowId` / `SecretId`) continue the five-ADR streak of
preserving `Type: Copy` via internment. The effect-check pass
slots into the existing salsa pipeline as a new query between
check_query and borrow_check_query (D11). Phase-go fixtures
continue the cNN_go_no_go.sentinel naming (c30 for C3.0).

**Phase B is the reference impl, not the production code.**
The `sentinel-effects-proto` crate has 226 tests + three
validation demos. It validated the design. The production
compiler rebuilds *the design* against the C1 + C2
infrastructure. The crate stays in the workspace as a
regression / reference through C3; deletion-eligible after
C3.3 close.

**Capability enforcement is module-system territory.** HANDOVER
§6.2 lists "capability enforcement at the module boundary" as
C3 work, but Sentinel doesn't have a module system yet. The
deferral is structural, not an avoidance: it lands in the
module-system ADR (post-C3, probably C5).

## Consequences

### Positive

- The language gains effect-aware signatures + a sound secret-
  flow check. This is the core of Sentinel's safety story at
  the language layer (after C2's borrow checker).
- The `secret T` qualifier ports cleanly from Phase B with the
  proven interner pattern. Five static CT rejections cover the
  primary side-channel categories.
- Effects on fn signatures are visible: a fn that does I/O has
  to say so in its signature (or in inference produces the
  same conclusion). This is the "honest signatures" benefit
  Sentinel advertises.
- The C1 + C2 invariants survive — `Type: Copy + Hash`, the
  parallel-tree pattern, the salsa pipeline, ADR-first per
  sub-phase. C3 inherits all of them.
- `effect_check_query` is a NEW pass between check_query and
  borrow_check_query. Pipeline becomes:
  `parse_query → resolve_query → check_query → effect_check_query →
  borrow_check_query → codegen`. LSP / tooling benefits from
  the additional query.
- The D8 deferral lets C3 ship in 4-6 sessions vs the
  combined 7-11 if handlers were bundled. Sentinel still gets
  to handlers — it just gets there in a separate ADR with
  C3's surface stable.
- The interner pattern continues. No new design risk on the
  type-system side.

### Negative

- Without handlers (D8), effects "land at main and die." Any
  effect-bearing fn called from main rejects the program. This
  is the lexical-first analog from ADR 0017 — useful but
  limited; users will hit it routinely and need to wait for the
  handler ADR.
- The "effect-bearing fn called from main rejects" property
  means C3 minimum can't ship many useful programs without
  resorting to `print` (the runtime builtin) being declared
  effect-free. That's a fudge — we explicitly mark `print` as
  carrying no effect even though it's I/O — but it's the only
  way to keep main usable at C3 minimum. ADR closure expected
  at D8.
- The Phase B reference impl validates typing soundness but
  doesn't cover the production-compiler-shape coupling (salsa
  queries, parallel-tree, diagnostic accumulator routing).
  Some surprises in C3.1 + C3.2 likely.
- `Type::Secret` adds the sixth interner table to TypedProgram
  (after fn_signatures, generic_instances, refs, structs,
  effect-related tables for D4). Profile-driven HashMap
  interning becomes more relevant as the type table grows.
- The C3.4 handler runtime ADR (0020) will be substantive.
  The lowering strategy is genuinely undecided — Sentinel's
  bootstrap-compiler design has no precedent for delimited
  continuations. Bigger risk than the typing layer.

### Neutral

- D8's deferral matches the C2 precedent + risk profile. Not
  a regression from C2.
- D10's "no new tokens for the !-postfix annotation" reuses
  existing punctuation; the parser disambiguation is the same
  pattern as C2's `*` (deref vs multiply).

## Alternatives considered

- **Ship handlers at C3.** Rejected per D8. Lowering strategy
  is its own design call; bundling overshoots the per-sub-
  phase budget by 3-5 sessions. Same shape as ADR 0017's
  "ship Polonius at C2" rejection.

- **Mandatory effect annotations on all fns** (no inference).
  Rejected per D2. Phase B proved full inference works; mandatory
  annotations are a usability tax with no compensating safety
  win at the bootstrap stage. Optional annotations as constraints
  is the calibrated middle ground.

- **Ship constant-time codegen at C3.1.** Rejected per D7 + D12.
  CT codegen is a separate concern (branch-free lowering of
  `select` / `cmov`; speculation barriers; cache-side-channel
  resistant memory access patterns). The static check shipped
  at C3.1 covers the typing-layer guarantees; codegen-layer
  guarantees deserve their own ADR.

- **Use a `Row` Type variant** (`Type::Row(...)`) instead of
  interning rows on `TypedFnSignature`. Rejected per D3. Rows
  only appear on fn arrows; making them a first-class type
  payload complicates `Type::substitute` + `unify_one` without
  a compensating win until first-class fns arrive.

- **Make `Secret` a type qualifier separate from the Type
  enum** (e.g., a `Qualified<Type>` newtype). Rejected per D5.
  The interner-pattern adaptation works cleanly with `Secret`
  as just another Type variant; the type-checker's unification
  rules are simpler when Secret IS a Type rather than a wrapper
  around one.

- **Keep `sentinel-effects-proto` running as a "validation"
  layer alongside the production compiler indefinitely.**
  Rejected. The Phase B crate is explicitly throwaway per its
  lib.rs framing + STATE.md §B. C3 absorbs the design; the
  reference impl gets archived after C3.3 (kept for git history
  + ADRs 0002-0008 + possibly a follow-on retrospective).
  Deletion-eligibility is at C3.3 close.

- **Use Phase B's structural `Row::{Empty, Var, Cons}`
  directly** without interning. Rejected per D3. Breaks
  `Type: Copy + Hash` indirectly (the row would need to live
  somewhere; if on TypedFnSignature directly, signatures stop
  being Copy / Hash and downstream code needs `.clone()` /
  manual hashing). Interner is the proven path.

## Revisit

This ADR is **PROPOSED** until C3's sub-phases land. Per-D
revisit triggers:

- **D1** (effect-annotation syntax): revisit if `! { Op1, Op2 }`
  postfix syntax surfaces as confusing in user testing. Koka
  uses `<Op1, Op2>` postfix; some languages use `: Op1 + Op2`.
- **D2** (inference vs annotation): revisit if users prefer
  mandatory-annotations-for-public-fns. C3 minimum stays
  inference-with-checked-annotations.
- **D3** (interner pattern): revisit if profiling shows the
  linear-search interner becomes a bottleneck. HashMap upgrade
  is mechanical (same as ADR 0016 D6a's GenericInstance).
- **D5** (Secret as Type variant vs qualifier): revisit if the
  unification rules surface coupling we didn't predict.
- **D7** (five CT rejections): revisit as new categories
  surface. The fifth (`SecretInRefDeref`) is the C2-ref-specific
  one; future ref layers may add more.
- **D8** (handler deferral): the explicit revisit trigger.
  C3.4 / ADR 0020 (handler runtime) is the planned post-C3
  work.
- **D11** (pipeline shape): revisit if effect-check + borrow-
  check turn out to have circular dependencies we didn't
  predict.
- **D12** (out-of-scope list): each item gets its own future
  ADR. The handler-runtime ADR (0020) is the next-largest
  single chunk.
- **D13** (main effect-free): revisit at the handler ADR
  (effects can finally be discharged) — may relax to "main has
  to handle all effects" rather than "main can't have any
  effects."

## Appendix: estimated implementation footprint

For session-budget planning. Numbers are rough; the actual
C3 sub-phase commits will be larger if the unification
machinery + diagnostic surface surface unanticipated coupling.

  - **C3.0** (lexer + AST + parser, ~400-600 LOC):
    - sentinel-syntax (lexer): +30-50 (six keywords + the
      positional `!` disambiguation)
    - sentinel-ast: +80-120 (EffectDecl + OpDecl +
      effect_row field on FnDef + TypeExprKind::Secret +
      ExprKind::Declassify + ExprKind::Perform +
      ExprKind::Handle placeholders)
    - sentinel-syntax (parser): +150-250 (parse effect_decl,
      parse `! { ... }` postfix, parse `secret T` prefix,
      parse `declassify(e)`, parse `handle ... with { ... }`
      with-handler-stub)
    - sentinel-resolve: +30-50 (pass-through; resolve
      doesn't compute effects)
    - tests: +30-50

  - **C3.1** (secret typing, ~400-700 LOC):
    - sentinel-types: +300-500 (Type::Secret + SecretId +
      SecretData + intern_secret + unify_secret + five CT
      rejections + bidirectional declassify check)
    - sentinel-codegen: +20-40 (Secret(T) lowers identically
      to T)
    - tests: +25-40 fixtures + unit tests

  - **C3.2** (effect rows + effect_check_query, ~600-1000 LOC):
    - sentinel-effect-check (new crate): +400-600 (row
      representation, inference, annotation check, D13
      main-effect-free rejection, EffectError variants)
    - sentinel-types: +100-200 (TypedFnSignature.effect_row,
      EffectId, RowId, RowData)
    - sentinel-driver: +20 (wire effect_check_query into the
      pipeline)
    - tests/pass + tests/ui: +10-15 fixtures

  - **C3.3** (polish + close-out, ~100-200 LOC):
    - phase-go fixtures (c30_*)
    - STATE.md + HANDOVER §0 close-out
    - ADR 0019 PROPOSED → ACCEPTED flip
    - tests: +5 driver pass-tests for phase-go

  - **C3.4** (handler runtime — DEFERRED to ADR 0020,
    ~800-1500 LOC estimated):
    - sentinel-effect-check: +200-400 (handler typing rules,
      effect discharge, perform inference)
    - sentinel-codegen: +500-1000 (continuation lowering;
      free-monad reification or CPS transform — TBD by ADR
      0020)
    - sentinel-runtime: +100-200 (continuation runtime
      symbols)
    - tests: +20-30 fixtures

  - **Total at C3 minimum (without 3.4)**: ~1500-2500 LOC
    across crates. Smaller than C2's ~2100-3700 LOC because
    the reference impl shortcuts much of the design work.

  - **Estimated session budget at C3 minimum**: 4-6 sessions
    across 4 sub-phases. Compare ADR 0017 D9's "6-13 sessions
    across 5-6 sub-phases" for C2 (actual ~6 at low end);
    C3 minimum should land closer to 5.

After C3 minimum: **ADR 0020 (handler runtime)** opens the
substantial design call deferred at D8. Then **Phase C4
(traits + structured concurrency)** per HANDOVER §6.2. The
language story at the typing layer is essentially complete by
the end of C3 minimum; the runtime + structural pieces are
the remaining bulk.
