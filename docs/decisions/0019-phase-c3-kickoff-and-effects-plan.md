# ADR 0019: Phase C3 kickoff — effect rows, secrets, handlers (effect-system integration from Phase B)

Status: **ACCEPTED-WITH-AMENDMENTS** at C3.3 close-out. The
typing-layer minimum of Phase C3 ships per the original sub-
phase split: D1-D7 + D10-D13 fully exercised; D8 (handler
runtime) deferred to follow-on ADR 0020; D9 (async-as-effect)
deferred indefinitely. Two amendments captured below in the
"Amendments at C3.3 close" section: A1 the
SecretEscapesPolymorphism rejection from D7 turned out to be
subsumed under Sentinel's monomorphic generics + the
SecretFlow-via-Mismatch path — no separate variant needed; A2
runtime builtins (print / unwrap_or / is_some / len) are
declared effect-free at C3.2 rather than carrying Io, to
preserve backward-compat with existing programs.

D-decision progress: **D1 + D2 + D3 + D4 + D5 + D6 + D7 (3 of
5 explicit rejections; SecretFlow + SecretEscapesPolymorphism
both subsumed per A1) + D10 + D11 + D12 + D13 all exercised**
across C3.0(a) (53c26f7) + C3.0(b) (1a951ee) + C3.1 (ae9483f)
+ C3.1b (2b36b0f) + C3.2(a) (e95966a) + C3.2(b) (82e859e) +
C3.3 (this session). Coverage map:

  - **D1** (`! { Op1, Op2 }` postfix syntax): C3.0 parses, C3.2
    interns as `TypedFnSignature.effect_row: Vec<EffectId>`.
  - **D2** (full inference, annotation as constraint): C3.2(b)'s
    `effect_check` does the fixed-point inference + checks each
    annotated fn's inferred row ⊆ annotation, surfacing
    `EffectAnnotationMismatch` when the body exceeds.
  - **D3** (interned row representation): `RowId` was deferred
    in favour of `Vec<EffectId>` per-signature at C3.2 — at the
    minimum-viable surface a separate RowId interner adds
    bookkeeping without compensating gains. The full row
    interner is a follow-on when `perform` / handlers land
    (ADR 0020) and rows need to flow as first-class types.
  - **D4** (`effect E { ... }` declarations): C3.0 parses, C3.2(a)
    interns as `TypedEffectDecl` with op-param + return-type
    resolved against the struct table. Empty effects + multi-
    op effects + ops with/without `-> RetT` all work.
  - **D5** (interned `Type::Secret(SecretId)`): C3.1.
  - **D6** (`declassify(e)` special form): C3.1.
  - **D7** (5 CT rejections): 3 explicit (SecretBranch,
    SecretDivisor, SecretInRefDeref); SecretFlow subsumed by
    `Type::Mismatch` (mixed public+secret operators fail set
    equality); SecretEscapesPolymorphism subsumed by Sentinel's
    monomorphic generics per A1.
  - **D10** (lexer): C3.0(a) added 6 keywords. `!` repurposed
    positionally for postfix effect annotation.
  - **D11** (pipeline shape): C3.2(b) wires
    `effect_check_query` between `check_query` and
    `borrow_check_query`. `borrow_check_query` depends on it so
    the transitive `accumulated::<Diagnostic>` set includes
    effect-check diagnostics.
  - **D12** (out-of-scope items): handler runtime + async +
    capability enforcement + CT codegen + row-polymorphism
    syntax + effect-on-effect-declarations all deferred per the
    ADR. No code paths bypass these.
  - **D13** (`fn main` effect-free): C3.2(b)'s effect_check
    enforces this. Surfaces `EffectError::UnhandledEffect` on
    any effect bubbling to main since handlers are deferred.

**D8 (handler runtime + perform) deferred to ADR 0020** per the
original D8 call. Handle / with / perform keywords are reserved
in the lexer at C3.0; the surface parses but rejects at
type-check (handle/perform expressions would surface as the
existing Mismatch path or similar — they're parsed-but-
unusable at C3.3). **D9 (async-as-effect) deferred indefinitely**
— treated as just another effect surface in Phase B but the
runtime work (futures, schedulers, I/O loop) is wholly new and
unrelated to the typing-layer minimum. **D14 (phase-go program)
exercised at C3.3** via `tests/pass/c33_go_no_go.sentinel` —
combines effect_decl + annotated fn + secret typing +
declassify-before-branching in one program; stdout "42",
exit 0.

**D5 amendment A3 (2026-10-28)**, a third amendment, after the
*Revisit* section, closes register **D158**: a `secret T` drops
as its inner type does, in all three back ends. inkwell always
did; the oracle (`snc llvm`) and `scg` dropped nothing for one.

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

What a `secret T` drops is set by D5 amendment A3 (2026-10-28,
below): exactly what a `T` drops, in every back end.

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
| C3.0 | Lexer additions (`effect`, `secret`, `declassify`, `handle`,   | 1 session    | DONE   |
|      | `with`, `perform`); AST + parser for effect_decl, effect       |              | 53c26f7|
|      | annotations on fns, secret type prefix, declassify expr.       |              | +      |
|      |                                                                |              | 1a951ee|
| C3.1 | Secret qualifier landing in `sentinel-types`: `Type::Secret    | 1-2 sessions | DONE   |
|      | (SecretId)` + SecretData + intern_secret; resolve passes       |              | ae9483f|
|      | through; check_query computes Secret types via unification     |              | +      |
|      | (a Secret wrapper unifies with bare T iff explicit declassify  |              | 2b36b0f|
|      | strips it); five CT rejections per D7. Codegen lowers          |              |        |
|      | Secret(T) identically to T at C3.1 minimum (the constant-      |              |        |
|      | time guarantees come in a follow-on codegen ADR per D12).      |              |        |
| C3.2 | Effect rows + effect_check_query salsa pass; effect            | 2-3 sessions | DONE   |
|      | inference (full inference per D2) + annotation check; D13's    |              | e95966a|
|      | "main must be effect-free" rejection.                          |              | +      |
|      |                                                                |              | 82e859e|
| C3.3 | Polish + phase-go programs + STATE.md / HANDOVER refresh +     | 0-1 sessions | DONE   |
|      | ADR 0019 PROPOSED → ACCEPTED flip. Phase C3 close-out (at      |              | this   |
|      | the typing-layer minimum per this ADR's D8 deferral).          |              | session|
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

## Amendments at C3.3 close

Two amendments to the original ADR, recorded as the ADR
flipped from PROPOSED to ACCEPTED-WITH-AMENDMENTS:

**A1. SecretEscapesPolymorphism subsumed by monomorphic
generics + SecretFlow.** Phase B's ADR 0008 D4 listed five
constant-time rejections, the fifth being "a generic fn
instantiated with a Secret type AND used in a non-secret-
preserving way." Sentinel's generics are monomorphic per ADR
0016 D7c: each generic fn produces a concrete instance for
each call-site's type-args. A generic fn `fn id<T>(x: T) -> T`
instantiated with T = `secret i64` produces a monomorphic
instance `id_secret_i64(x: secret i64) -> secret i64` whose
secret-flow is checked exactly the same way as any concrete
signature — if its body would leak the secret (e.g., by passing
the secret value to `print` which takes `i64`), the standard
Mismatch path fires. No separate SecretEscapesPolymorphism
variant needed. The ADR 0019 D7 status header lists "3 of 5"
explicit rejections at C3.1/C3.1b; the remaining 2 (SecretFlow
+ SecretEscapesPolymorphism) are both subsumed.

**A2. Runtime builtins (print / unwrap_or / is_some / len)
declared effect-free at C3.2 rather than carrying Io.** A
naive reading of "effect-aware signatures" would suggest
`print` carries `! { Io }`. C3.2 declares them effect-free
because (a) every existing Sentinel program that uses `print`
would otherwise break with EffectAnnotationMismatch /
UnhandledEffect at main, and (b) `print` is a thin runtime
fixture, not a user-facing fn — promoting it to carry effects
would force every program to handle Io even for "Hello, world."
A future ADR may revisit when handler runtime lands (then we
can have `handle main with { Io.print(x, k) => libc_print(x);
k(()) }` and the surface becomes ergonomic again). Until then,
runtime builtins are an implicit exemption.

**Implementation gap closed at C3.3**: D14 (phase-go program)
was originally specified at C3.2 close; the actual go/no-go
fixture combining all of effect_decl + annotated fn + secret
typing + declassify landed at C3.3 (this session) via
`tests/pass/c33_go_no_go.sentinel`. stdout `42`, exit 0.

## Retrospective at C3.3 close

Six sub-phases shipped across ~5 effective sessions (split
into eight commits because C3.0 was bundled as C3.0(a) + C3.0(b)
and C3.1/C3.2 each had an a/b split for coherence). The
original estimate was 4-6 sessions at C3 minimum (without the
deferred D8 handler runtime); came in at the upper end of the
"at C3 minimum" range, which is exactly what the ADR predicted.

What worked:

- **Phase B reference impl shortcuts the design work.** The
  `sentinel-effects-proto` interpreter validated typing rules,
  inference algorithm, and diagnostic surface across 226 tests.
  C3 was mostly mechanical adaptation: parallel-tree
  integration, salsa pipeline plumbing, secret-as-Type-variant
  via the existing interner pattern. Substantial typing-rule
  research was already done.
- **The interner pattern**. `Type::Secret(SecretId)` is the
  sixth in a row (after `Nullable`, `Array`, `GenericInstance`,
  `Ref` at C2). `Type: Copy + Hash` held across the entire
  Phase C type-system stack — no clone-cascades or per-Type
  hashing diverged. Same precedent for ADR 0019 D3's RowId
  deferral: we DIDN'T use a RowId interner at C3.2 because
  `Vec<EffectId>` on each `TypedFnSignature` was the minimum-
  viable shape, but the interner is a known-shape upgrade path
  if profiling demands.
- **`effect_check_query` as a new salsa pass**. The
  `effect_check_query` chains on `check_query` and
  `borrow_check_query` chains on `effect_check_query`. The
  pipeline shape didn't change for any existing user (codegen
  still just consumes `TypedProgram` + `DropPlan` as before);
  the new pass is purely additive. Diagnostics flow through
  the transitive `accumulated::<Diagnostic>` set without
  needing manual collection.

What surprised us:

- **D3 (RowId interner) deferred at C3.2**. The ADR specified
  interned `RowId` analogous to `GenericInstanceId`. In
  practice, `Vec<EffectId>` per signature was simpler and
  carried the same set-equality semantics. RowId becomes useful
  when rows flow as first-class types (e.g., when handler
  runtime arrives and `kont: Continuation<Row>` needs to carry
  a row by id). Deferred to ADR 0020.
- **Runtime builtins as effect-free** (A2). The ADR didn't
  explicitly call this out; we discovered the backward-compat
  issue at C3.2 sig-construction and made the call inline.
  Documented as A2 above.
- **D7 only needed 3 explicit rejections** (A1). The fourth
  (SecretFlow) trivially routes through the existing Mismatch
  path; the fifth (SecretEscapesPolymorphism) is subsumed by
  monomorphic generics. Five-rejection enumeration in the ADR
  was overcautious; three suffice for the C3.1 surface.

What's deferred to ADR 0020 (handler runtime):

- **D8: `handle e with { ... }` + `perform Op(args)`**. The
  surface parses (lexer keywords reserved); type-check stops
  short of actually implementing handler dispatch. The
  substantive design call (free-monad reification vs CPS vs
  stack-saved continuations) is its own ADR.
- **Async-as-effect** (D9). Treated as just another effect at
  the surface, but the runtime piece (futures, schedulers, I/O
  loop) is wholly new. Deferred indefinitely.

What's deferred to a follow-on capability-or-module ADR:

- **Capability enforcement at module boundaries** (HANDOVER
  §6.2 calls this out as C3 work). Sentinel doesn't have a
  module system yet — D12 explicitly deferred. Lands when
  modules ship.
- **Constant-time codegen** (branch-free `select` / `cmov`
  for secret comparisons; speculation barriers). Separate
  codegen-side ADR. The C3.1 static check covers the typing-
  layer guarantees; codegen-layer guarantees are a separate
  concern.

Pipeline at C3.3 close:

    parse_query → resolve_query → check_query
                → effect_check_query → borrow_check_query → codegen

The five-pass front-end is mature. Codegen still consumes
`TypedProgram` + `DropPlan` only; effect-check's
`EffectCheckedProgram` flows through the salsa graph for
future use (LSP / handler runtime) but doesn't change codegen's
inputs.

## Revisit

This ADR is **ACCEPTED-WITH-AMENDMENTS** after C3's sub-phases
landed. Per-D revisit triggers:

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

## D5 amendment A3 (2026-10-28) — a `secret`-qualified value drops as its inner type does, in all three back ends (register D158)

D5 makes `secret T` a type of its own, `Type::Secret(SecretId)`, whose `SecretData` carries the
inner `T`, and C3.1 lowers it as `T` (the *Appendix*: "Secret(T) lowers identically to T"). D5
never said what it drops. inkwell has answered since C3.1: `emit_drop_for_binding` and
`field_type_needs_drop_inner` strip one `secret` layer before they pick a drop, so a `secret T`
is freed or released as a `T` is. This amendment makes that rule part of D5 and holds the two
text back ends to it: **a value of type `secret T` drops exactly as a value of type `T` does —
at the same drop sites, behind the same moved flags, by the same walk — in inkwell, the oracle
(`snc llvm`) and `scg`.** One layer is all there is: `secret secret T` does not parse.

The text back ends did not follow it. The oracle's `needs_drop` answered `false` for every
`secret T` — its arm, written down when `needs_drop` became an exhaustive match (`61bcac3`; the
`_ => false` before it gave the same answer), said that a `secret` wraps only word scalars, so a
droppable inner was unrepresentable — and its `emit_drop_for_binding` had no arm for one.
`scg`'s `cg_needs_drop` and `cg_emit_drop` dispatched on the qualified type without stripping
it; the self-host port had left "a hypothetical `secret Struct` drop" deferred ([ADR
0045](0045-self-host-port-codegen.md), its secret slice). The premise did not hold:
`secret [i64]`, `secret [u8]`, `secret Vec<i64>` and a `secret`-qualified struct, enum or class
all type-check, as does a struct with a `secret`-qualified field; only a `secret`-qualified
`Shared` or `Mutex` is refused (ADR 0071's `SecretHandle`). So in the oracle's and `scg`'s
output such a binding or parameter was dropped nowhere — not at a scope exit, a `return`,
`break` or `continue` drain, a handler arm's exit, an assignment ([ADR
0077](0077-drop-flags-for-maybe-moved-bindings.md) A2), a generic instance's parameter frame or
an effecting fn's frame-pushing parent (ADR 0077 A1) — and a struct's or a generic instance's
drop walked past a `secret`-qualified field: always a leak, never a release too many.
`[secret T]`, an array of secrets, is an `Array` and was always freed, and ADR 0071 A4's class
drop already stripped the qualifier for a class's fields.

Now the oracle's `needs_drop` follows the inner type of a `secret T`, its `emit_drop_for_binding`
strips the qualifier (`unsecret`) before it dispatches, and `scg`'s `cg_needs_drop` and
`cg_emit_drop` strip it (`strip_secret`) first. inkwell does not change. Because `needs_drop` also
decides which moved bindings and fields get a flag (ADR 0077 D3, D5), and an exit or an assignment
skips a binding whose move was walked without one, the text back ends now give a moved flag, as
inkwell does, to a `secret`-qualified binding moved on one path (by `declassify` or by a call) and
to a `secret`-qualified field moved out of a struct, and drop each behind its flag.

**What it frees, and how.** What inkwell's drop frees, with the same calls: an array's, a string's
or a `Vec`'s buffer and an enum's box with `sentinel_free`, which is libc `free`; a `Shared` or
`Mutex` held in a `secret`-qualified struct or class with `sentinel_shared_release` /
`sentinel_mutex_release`. (inkwell routes only a public array literal into a scope arena, never a
`secret` one, so where the public twin is bulk-freed by `sentinel_arena_exit`, a `secret` array is
freed by `sentinel_free` at the same exit.) The qualifier adds no drop action, in any back end: a
`secret` buffer is neither zeroed nor mlocked, as in inkwell's output all along, so its bytes may
stay in the freed block, and in an allocation that reuses it, until something overwrites them;
before, the text back ends' output left them in a block never freed. The memory Sentinel mlocks at
allocation and zeroes at the last release is a `Shared` or `Mutex` cell whose payload is a `secret`
scalar (ADR 0071, `sentinel_shared_new_secret` / `sentinel_mutex_new_secret`). Wiping a heap
`secret` buffer is not designed; inbound request R1
([`inbound-requests.md`](../inbound-requests.md)) asks for it for `[secret u8]` / `Vec<secret u8>`,
and this amendment does not provide it.

For a program `snc build` builds, it is sound for these reasons:

- **It is inkwell's rule, unchanged.** `snc build` has dropped a `secret T` as `T` since C3.1,
  so the corpus and every program built since run it. The change moves no inkwell object (all 305
  merge and 280 `--separate` objects of the sweep below are byte-identical), and for ten of the
  fifteen probes in the table below the oracle now emits, byte for byte, the IR of the probe's twin
  without the qualifier.
- **Ownership already looked through the qualifier.** The borrow checker's `is_copy_type` and
  `scg`'s `is_move_type` strip `secret`, so a `secret T` binding is moved as a `T` binding is, and a
  `secret`-qualified field of a struct is partially moved ([ADR
  0046](0046-partial-move-field-soundness.md)) as any other field is. The text back ends' move
  bookkeeping reads the type only through `needs_drop` (a class's fields through `holds_handle`,
  which already looked through it), and `needs_drop` now gives such a binding or field the moved
  flag (ADR 0077) inkwell gives it. A drop the change adds is the drop a `T` binding gets at the
  same site; on a path that moved the value, it sits behind the flag that move set.
- **Few operations reach a `secret` aggregate.** The type checker refuses a field access, an
  index or a method call on a `secret` value, so one can only be bound, moved, widened,
  borrowed, declassified or assigned; `declassify`, a widening and a by-value argument move
  their operand. The probes cover each of these.
- **The handles it releases were counted.** The type checker refuses a `secret`-qualified `Shared`
  or `Mutex` (`SecretHandle`), so no program `snc build` builds holds one, and a handle held in a
  `secret`-qualified struct or class was counted into it as into the same value without the
  qualifier (ADR 0071 A1, A4, A6), so its release gives back that owner's own unit.

Measured on macOS as the peak resident set size (`/usr/bin/time -l`, decimal MB, the second of
two runs), with debug builds of `snc` and `scg` from HEAD (`113f559`, ADR 0077 A2, without this
amendment) and from the change, each linked against its own build's runtime (the two are
byte-identical). Every run answers 42, plain and again under `MallocScribble`,
`MallocGuardEdges` and `MallocErrorAbort`, and `scg`'s IR is byte-identical to the oracle's on
every probe, before and after.

| 2,000,000 iterations | before: inkwell / oracle / `scg` (MB) | after (MB) |
|---|---|---|
| a `secret [i64]` local | 1.9 / 66.1 / 66.1 | 1.9 / 1.9 / 1.8 |
| a `secret [u8]` (a string) local | 1.8 / 34.0 / 33.9 | 1.8 / 1.9 / 1.8 |
| a `secret Vec<i64>` local | 1.9 / 34.0 / 34.0 | 1.9 |
| a `secret` struct holding an array | 1.8 / 34.0 / 34.0 | 1.8 / 1.9 / 1.9 |
| a `secret` struct holding a `Shared` | 1.9 / 66.2 / 66.2 | 2.0 |
| a `secret` struct holding a `Mutex` | 2.2 / 66.4 / 66.4 | 2.2 |
| a `secret` class holding a `Shared` | 2.0 / 66.2 / 66.2 | 2.0 |
| a `secret` struct holding a class | 2.0 / 66.2 / 66.2 | 2.0 / 1.9 / 1.9 |
| a public struct with a `secret [i64]` field | 1.9 / 34.0 / 34.0 | 1.9 / 1.8 / 1.9 |
| a public struct with a `secret` class field | 2.0 / 66.2 / 66.2 | 2.0 / 1.9 / 1.9 |
| a `secret` struct parameter holding an array | 1.9 / 34.0 / 34.0 | 1.9 |
| a `secret` class parameter holding a `Shared` | 1.9 / 66.2 / 66.2 | 2.0 |
| a `secret` binding declassified on every other call, dropped behind its flag on the rest | 1.9 / 18.0 / 18.0 | 1.9 / 1.8 / 1.9 |
| a `secret [i64]` assigned in a loop | 1.9 / 66.1 / 66.1 | 1.8 / 1.9 / 1.8 |
| a `secret` struct holding a `Shared`, assigned in a loop | 1.9 / 66.2 / 66.2 | 2.0 / 2.0 / 1.9 |

Each probe's twin without the qualifier measures 1.8 to 2.5 MB in every back end, before and after.
The probes holding a `Shared`, a `Mutex` or a class, but the one assigned in a loop, drop the value
in an inner block or a callee and then read or relock the cell, so a release too many would answer
wrong. After the change `leaks --atExit` finds no leak in any probe's or twin's executables. One
more probe and its twin, an `init` parameter holding a `Shared` that the `init` only reads, stay at
66.2 MB through inkwell, `secret` or not, which is register D154; there the `secret` probe's oracle
and `scg` output fall from 66.2 to 2.0 and 1.9, what the twin's already measured, and leak nothing.

A matched `snc llvm` sweep of the 485 `.sentinel` files, other than the new fixture, in
`tests/pass`, `tests/ui`, `examples` and `sentinel_library` changes no verdict: 339 byte-identical,
144 refused by both with identical messages, and 2 changed, `tests/pass/c19_widen_arm_family`'s
`main` and `tests/pass/c71_class_field_handles`'s `SK__init`, each by a moved flag and a drop behind
it, of the `secret [i64]` that `declassify(from_array)` moves and of the `secret K` parameter that
`self.k = k` moves. Neither drop runs, since the move comes first on every path; with SSA names and
labels normalized both diffs are insertions only, and inkwell's IR has the same flag. Of
`demos/win32`'s three files one is identical and two are refused by both. A matched `snc build` and
run of 305 programs (the 237 in `tests/pass` and the 68 examples with a `main`, merged, the examples
also `--separate`; the seven socket examples built, not run), plain and under the malloc checks, and
of the oracle's IR for the 285 it emits, changes no build result, exit code or output, and every
merge and `--separate` object is byte-identical. The oracle's IR of the ten self-hosted module roots
does not move, since those sources declare no `secret`-typed binding, parameter, field or return;
the `cg.sentinel` change itself moves the five roots that include it, in `cg_needs_drop` and
`cg_emit_drop` only, and `scg` built from them emits the oracle's bytes for the merged `codegen`
root. `scg` equals the oracle on 341 of the 342 corpus files the oracle emits (the exception,
`examples/lang/delegation`, is the known named-impl qualification gap, the same before) and on all
116 codegen seeds; the two seeds that hold a `secret` struct, one moved out of a class and one
assigned, move in both alike.

The probes built each program through inkwell, the oracle's IR and `scg`'s IR with both builds, and
ran it under the malloc checks (the first set plain as well). One set crosses 25 kinds of
`secret`-qualified value (top-level `secret` arrays, strings, `Vec`s, structs, enums, classes and
generic instances, and public structs, generic instances and enum payloads with a `secret`-qualified
part) with up to 132 ways to move or drop one (3,174 programs; the 18 `declassify` and widening ways
apply only to the 18 kinds that are themselves `secret`), beside 31 hand-written programs and one
cross-module program, nearly all 2,000 rounds each: of the 3,024 programs `snc build` accepts, every
run answers 42 before and after; the oracle lowers 3,013 of the 3,023 single-file ones, as it did
before, its IR moves in 2,906 of them and `scg`'s equals it in all; ADR 0077's fail-closed refusal
never fires, and no back end refuses anything new. The other set generates 3,786 programs from
earlier drop-path generators with bindings, parameters, fields and payloads made `secret`-qualified,
3,484 of which `snc build` accepts: none crashes, aborts or answers differently where it did not
before, gains a refusal or a new `scg`/oracle difference, and the oracle's IR moves in every one.
What HEAD already did stays as it was: 92 stop at the runtime's ADR 0075 D6 abort (a handler arm
resumed into another operation) through inkwell and the oracle, `scg` refuses 730 (D159: a captured
parameter is not a word), and in four of the 92 its output runs on instead. Counted by `leaks
--atExit` on a sample of 392 of them, the heap blocks the oracle's output leaves live at exit fall
from 6,843,269 to 596,586 (inkwell's: 1,271,039): fewer in 381 programs and the same in 11, never
more than before and never more than inkwell's. What the oracle's output still leaves, inkwell's
leaves too: an enum payload's heap, a class's heap fields and the old value of a field assignment.

No corpus program's behaviour changes. Before the change the codegen seeds caught a revert of the
drop dispatch (through the two seeds that hold a `secret` struct) but not of `needs_drop`, and a
revert in both text back ends together left them agreeing, so no existing test saw it; the pins are
what hold it. Pinned by `tests/pass/c31_secret_aggregate_drops` (twelve shapes of `secret`-qualified
arrays, strings, `Vec`s, enums, structs and classes: moved on one path, as parameters, assigned, as
a struct's fields, into an init's and a method's parameters, across effecting fns and handler arms,
through `declassify` and widening, in loops, at generic instances; 2,000 rounds with freed cells
reused), its oracle-IR run
(`selfhost_codegen::oracle_ir_of_the_secret_aggregate_drop_program_runs`), the oracle's IR count
test `llvm_a_secret_qualified_value_drops_as_its_inner_type` (frees, releases and flag-guarded drops
in 23 functions), and four codegen seeds beside the existing `secret H` assignment seed. Only the
count test sees the change reverted in both text back ends, which then agree in dropping nothing.
The seeds and the codegen differential see either half reverted in either back end alone. A drop the
change added, if duplicated or run on the path that moved its value, aborts the fixture or changes
its answer wherever it runs: 154 of 238 such IR mutants; the other 84 never take effect on any path
the fixture runs (a drop where its value was always moved, or a guard whose flag is always false
there).

It moves the IR the oracle and `scg` emit wherever a `secret`-qualified value's inner type needs
a drop, and not inkwell's, so it is at least a minor version (ADR 0076 D2); it goes out with the
bump to 0.3.0 that ends the agreed batch.

What stays open:

- **A `secret T` drops no further than `T`.** What `T`'s drop leaves, its `secret` form leaves
  too, in every back end: an enum payload's own heap (register D138; a `secret` enum with an
  array payload leaks 999 blocks over 1,000 rounds in all three back ends after the change, as
  the public enum does, where the text back ends leaked 1,998 before), a class's heap fields
  (D137), the old value of an assignment into a field (D120's field half), and in inkwell the
  handles a method's or an init's parameter frame keeps (D154).
- **No zeroing.** A `secret` buffer is freed as a public one is (above); scrubbing one is
  undesigned (inbound request R1 asks for it for `[secret u8]`).
- **`scg`'s let and embedded shapes** still refuse an effecting fn with a `secret`-qualified
  aggregate parameter (register D159), as they refuse any parameter that is not a word; the
  oracle lowers such a fn and now drops the parameter in the frame-pushing parent, as inkwell
  does.

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
