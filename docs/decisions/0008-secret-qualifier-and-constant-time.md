# ADR 0008: Secret qualifier and constant-time check

Status: ACCEPTED (D1-D7 confirmed by B4.0 + B4.1; D8 implicit via structural recursion in free-var collection)
Date: 2026-05-25
Related: 0004 (row representation), 0005 (effect inference judgment), 0006 (default-close row variables), 0007 (effect handlers)

## Context

B3 landed effect handlers end-to-end: surface, typing, runtime per ADR 0007 D5 (commits 821b16a, febf379, e7958e1, bdda217, a9cefb1, 8e3de20). The handler runtime and the secret qualifier share no machinery; B4 opens fresh territory.

HANDOVER §5 lists three validation demos for Phase B: a capability supply-chain demo (B2/B3), an async-as-effect demo (B3), and a constant-time password verification demo "that fails to compile if you try to branch on the comparison result" (B4). The third demo is the entire deliverable of this ADR.

B4 introduces two related things to Sentinel-Mini: a `secret T` qualifier on the type grammar, and a static check that rejects programs whose secret data flows into operations with data-dependent timing on real hardware. The tree-walking interpreter has no inherent notion of timing — `eval` is determined by AST shape and Value content — so the constant-time check is purely static. The prototype proves the type-system shape; the actual constant-time guarantee will live in MIR-level passes in Phase C codegen, outside this prototype.

This ADR pins down where secret lives in the type grammar, what operations are forbidden on secrets, the surface syntax for `secret T` and `declassify(e)`, the interaction with effect rows and handlers, and the phase breakdown for B4.0/B4.1/B4.2.

## Decisions

### D1. Secret as a type constructor

Three shapes were considered for where the qualifier lives in the type grammar:

(a) A `SecretQualifier::{Public, Secret}` field on every `Ty`. Every type carries the qualifier alongside its structural part.

(b) A type constructor `Ty::Secret(Box<Ty>)`. `Secret<Int>` and `Int` are structurally distinct types unified through explicit declassification.

(c) A separate qualifier lattice threaded through inference in parallel to `Ty` and `Row`, analogous to how B2.3a threaded effect rows. Every `infer` arm returns `(Subst, Ty, Row, SecLevel)` with explicit join rules per construct.

Decision: shape (b). `Ty` gains a `Secret(Box<Ty>)` variant.

Rationale: shape (b) falls out of HM unification with zero new machinery. `unify(Secret(a), Secret(b))` recurses on the inner types; `unify(Secret(a), b)` for non-secret `b` fails with a dedicated `TypeError::SecretFlow`. Public-to-secret is similarly a unification failure. Composes orthogonally with `Row` the same way `Fun` does. The parser surface `secret T` (D6) reads directly as a type-constructor application, matching the AST shape without impedance.

Shape (a) was rejected because qualifiers are a lattice, not free variables; every unification site would need an ad-hoc qualifier rule. The qualifier field would inflate every `Ty` node regardless of whether secrecy is in play. Shape (c) was rejected for B4 on the cost-vs-payoff grounds ADR 0007 D5 establishes for prototype work: HANDOVER §5.2's password-verify demo does not need information-flow tracking, and shape (c) is the more powerful system. Shape (c) remains the migration path if a future demo requires implicit flow taint (e.g., "result of branching on secret is itself secret"); the migration is mechanical, `Ty::Secret(t)` becoming `Ty` with a `SecLevel` annotation.

Smart constructor. `Ty::secret(t)` is idempotent: `Ty::secret(Ty::Secret(inner))` collapses to `Ty::Secret(inner)`. Types can arrive from substitution and unification, not just from the parser; the idempotency at the construction site keeps every other site from worrying about flattening. Parser-level rejection of literal `secret secret T` is also performed (a `ParseError::DoubleSecret` or similar) so the surface complains early, but the inference layer never sees a doubly-wrapped secret regardless.

### D2. Qualifier polymorphism and the no-α-leak rule

Shape (b) under unmodified HM allows `id : ∀a. a → a` to instantiate at `a = Secret<Int>` for free, because `Secret<Int>` is just another type. This is good for `id` specifically. It is bad in general: a function `leak : ∀a. a → Int` would also instantiate at `a = Secret<Int>` and produce an `Int` result, laundering a secret to a public type without declassification.

The standard escape is qualifier polymorphism (shape (c)). B4 takes the cheaper escape: a unification restriction. Specifically, `unify` rejects `Ty::Var(α) ~ Ty::Secret(t)` for any unbound type variable `α`. A bare type variable cannot bind to a secret. Concretely:

- `id : ∀a. a → a` applied to `Secret<Int>` fails to unify the argument position.
- To use `id` on a secret, the user writes a secret-aware variant or declassifies first.

This rejects too much by construction. The cost is that polymorphic library functions become two-flavored. The benefit is correctness-by-rejection: there is no path through inference by which a secret value reaches a non-secret type without traversing `declassify`. New error: `TypeError::SecretEscapesPolymorphism { var, span }` (final name TBD during B4.1).

A future ADR can promote this restriction to full qualifier polymorphism (shape (c)) if the prototype's demos grow to require it. The restriction is forward-compatible: any program that types under the restriction also types under qualifier polymorphism.

### D3. The constant-time static check

Constant-time at the eval level was considered as a runtime hook parallel to `EvalError::UnhandledOpAtTopLevel` (ADR 0007 D5 implementation note). Rejected: a tree-walking interpreter is several orders of magnitude removed from the host CPU's timing-side-channel surface, and the prototype's `Value` carries no type tag (by B0 design). Stamping `Value::Secret` at runtime would be theater. The constant-time check is purely static; `eval` runs normally on any program that passes inference.

Four static rejections fire in B4.1, each with a dedicated `TypeError` variant:

`SecretBranch { span }`. If `cond` in `if cond then _ else _` has type `Ty::Secret(_)`, reject. The condition's secret type may itself be `Secret<Bool>` produced by D4's comparison rule, or any other secret type that reached the condition position.

`SecretDivisor { span }`. In `BinOp(Div | Mod, _, e)` where `infer(e) = Ty::Secret(_)`, reject. Variable-time division on hardware is the canonical CT footgun.

`SecretFlow { from: Ty, to: Ty, span }`. Raised by `unify` when a `Ty::Secret(_)` meets a non-secret non-variable type, or vice versa. This is the public/secret unification failure (D1).

`SecretEscapesPolymorphism { var: TyVar, span }`. Raised by `unify` per D2 when a bare type variable would bind to `Ty::Secret(_)`.

The following are out of scope for B4: memory-access-at-secret-index (Sentinel-Mini has no arrays or memory primitives); secret-controlled loop counters (the prototype has no loops, only let-rec); speculative-execution barriers (codegen-level, Phase C); constant-time selection primitives (Phase C standard library).

### D4. Comparisons on secrets produce `Secret<Bool>`

`BinOp(Eq | Lt | Gt, a, b)` where either `a` or `b` types as `Ty::Secret(_)` produces a result type of `Ty::Secret(Bool)`, with the other operand unified against the inner type of the secret operand (per D1 unification rules, the other operand must therefore also be `Ty::Secret(_)` of the same inner type, or unification fails with `SecretFlow`).

Branching on the resulting `Secret<Bool>` is then rejected by `SecretBranch` (D3). This is the chain that makes the password-verify demo fail to compile in the natural way:

    let verify = fn(stored) => fn(provided) =>
      if stored == provided then "ok" else "fail"

types `stored == provided` as `Ty::Secret(Bool)` (both operands being `Secret<Bytes>` or similar), then rejects the surrounding `if`. The user's fix is to introduce a constant-time equality primitive that returns a declassified bool by construction. That primitive is left for B4.2 (D7) — under D3 the program above is already rejected, which is the demo deliverable.

Alternative considered: forbid `==`/`<`/`>` on secrets outright. Rejected for B4 because it requires a builtin `constant_time_eq` to exist before any demo can type, and Sentinel-Mini has no builtin-function mechanism today. The cheap rule (comparisons produce `Secret<Bool>`) makes the demo work without introducing builtins; the strict rule remains available as a tightening in a future ADR.

### D5. Declassification as a special form

`declassify(e)` has typing rule `e : Ty::Secret(T) ⊢ declassify(e) : T`. It is the only construct that removes a secret qualifier without unification failure.

`declassify` is a special form, not a function. The typing rule cannot be expressed as a Sentinel-Mini function type (`∀a. Secret<a> → a` would be a perfect laundering tool — instantiate at `a = α` and the D2 restriction prevents it, but more fundamentally a first-class declassify defeats the audit-point property that declassification sites are syntactically distinguishable in source).

AST: `ExprKind::Declassify { inner: Box<Expr>, span: Span }`. Parser surface: prefix keyword form `declassify(e)` parsed at atom precedence, mirroring how `do Label(arg)` parses. New token `Declassify`, new globally reserved keyword `declassify`.

Eval semantics: `Declassify(e)` evaluates `e` and propagates the resulting `Value` unchanged. The qualifier is purely type-level; the value representation is identical for `Int` and `Secret<Int>`.

### D6. Surface syntax

Type-level: `secret T` as a prefix-keyword type form. Mirrors `&mut T` in Rust grammatically. `secret` becomes a globally reserved keyword. Lexer adds `Token::Secret`. The `TyExpr` parser gains a prefix arm consuming `secret` and recursing on the body `TyExpr`, producing `TyExpr::Secret(Box<TyExpr>)`. Parser rejects `secret secret T` with `ParseError::DoubleSecret` (per D1 smart-constructor note).

Expression-level: `declassify(e)` as described in D5. Lexer adds `Token::Declassify`. `declassify` becomes a globally reserved keyword. Parsed at atom precedence: `atom := ... | "declassify" "(" expr ")"`.

The parens around `declassify`'s argument are mandatory, paralleling `do Label(arg)`'s mandatory parens. This avoids precedence ambiguity if `declassify` ever needs to scope over a larger expression and keeps the audit-point property syntactically obvious.

Alternatives considered. Postfix `T secret` was rejected as unconventional and confusing to Rust-trained readers. Constructor syntax `Secret<T>` was rejected because it forces a generic-type bracket decision the surface does not yet need; that decision should be made on its own merits when the language adds first-class generics. Lowercase `Secret(T)` constructor-call form was rejected for the same reason.

### D7. Interaction with effects and handlers

Effect signatures may mention `Secret`. `effect ReadKey : Unit -> secret Bytes` is the natural way to declare a capability that yields secret data; the parser and `infer_program`'s effect-environment population already handle arbitrary `TyExpr` in effect declarations, so this falls out for free once `TyExpr::Secret` parses.

Handler arm bodies type-check normally against secrets. The arm-body restriction (no branching on secret bools, etc.) is the same rule as everywhere else; no handler-specific machinery. Whether a handler arm can `declassify` its operation argument is decided by ordinary typing — `x` has whatever type the effect signature gave it, and `declassify` applies if it is secret.

The row qualifier and the secret qualifier are orthogonal axes on `Ty::Fun`. A function can be both secret-returning and effectful; nothing in `Fun(arg, ret, row)` looks at whether `arg` or `ret` is `Secret`. `Row::Cons`'s `arg`/`ret` payload types can themselves be `Secret` types, again orthogonally.

Open sub-question deferred to implementation: should `Continuation` resumption arguments be allowed to be secret? Per ADR 0007 D5 the continuation `k : B -> T2 ! rho_outer` where `B` is the effect's return type. If `B = Secret<Foo>` the user invokes `k(some_secret_value)`. Nothing about the runtime cares (secrets are typed-only, values are unchanged). The typing rule unifies as usual. The expected answer is "yes, falls out, no special rule needed." Confirm during B4.1 implementation and document inline if any rough edge appears.

### D8. Generalization

`Ty::Secret(_)` participates in generalization the same way every other type constructor does. Free type variables inside `Ty::Secret(α)` are generalized at let boundaries; the D2 restriction prevents those generalized variables from later instantiating across the secret boundary.

`Scheme.row_vars` (ADR 0005 D9 / decision 18) is unchanged. There is no `secret_vars` field; the qualifier is structural, not parametric, under shape (b).

### D9. Phase breakdown and completion markers

B4.0 — surface. Lexer adds `Token::Secret`, `Token::Declassify`. Parser adds `TyExpr::Secret` prefix form and `ExprKind::Declassify` atom form. `Ty::Secret(Box<Ty>)` variant added with smart-constructor `Ty::secret`. Placeholder `TypeError::SecretsNotYetSupported` keeps inference whole on programs that contain `secret` or `declassify` (no inference work yet). Eval is unchanged — `Declassify` evaluates its inner expression and returns the value, which already works under the existing eval shape because `Value` is qualifier-blind. Mirrors B3.0 (handler surface, 821b16a).

B4.1 — typing. `unify` extended for `Ty::Secret` cases per D1 and D2. `infer` arm for `Declassify` (unwraps `Ty::Secret`, errors if inner is not secret). `infer` arms for `If`, `BinOp(Div | Mod, ...)`, `BinOp(Eq | Lt | Gt, ...)` extended with the D3/D4 rejections and the D4 result-typing rule. New `TypeError` variants: `SecretBranch`, `SecretDivisor`, `SecretFlow`, `SecretEscapesPolymorphism`. `TypeError::SecretsNotYetSupported` removed. Mirrors B3.1 (handler typing, febf379 + e7958e1).

B4.2 — demo. The HANDOVER §5.2 password-verify demo lands as an integration test asserting that the rejecting program is rejected with the `SecretBranch` diagnostic. If the demo also wants to show a passing rewrite that uses constant-time equality, a built-in `constant_time_eq : secret Bytes -> secret Bytes -> Bool` is introduced — but the built-in mechanism does not yet exist in the prototype, so a simpler rewrite using `declassify` after some explicit CT step (or just omitting the passing case from the demo) is the fallback. Decide during implementation based on what the demo requires; the rejection deliverable is the load-bearing one.

B4 is complete when:

- `Ty::Secret(Box<Ty>)` is in `types.rs`, used by inference, and round-trips through `Display`.
- `Ty::secret` smart constructor enforces idempotency.
- The four D3 `TypeError` variants exist and fire from inference on the matching surface programs.
- `TypeError::SecretsNotYetSupported` is removed from `lib.rs` parallel to how `TypeError::EffectNotYetSupported` was removed in B2.3b2-b and how the eval-side placeholders were removed in B3.2b.
- The password-verify demo lives in `tests/integration.rs` as a rejecting program asserting the rendered diagnostic.
- 169 + N lib tests pass with N covering the four rejections, declassify typing, and Secret round-trip; 14 + 1-or-2 integration tests pass with the new password demo(s).

## Considered and rejected (for B4)

Shape (a) qualifier-field-on-every-`Ty`. Per D1: ad-hoc unification rules per construct, inflates every `Ty` node, does not reuse `unify`. Wrong shape for HM.

Shape (c) qualifier-lattice-threaded-through-inference. Per D1: the more powerful information-flow-typing system (Volpano-Smith-Irvine and successors), forward-compatible from shape (b) when needed, deferred until a demo requires implicit flow taint. ADR 0007 D5's "build only what the demos need" cost discipline applies.

Runtime CT enforcement via `Value::Secret(Box<Value>)`. Per D3: tree-walking interpreter is wrong layer for timing concerns; `Value::Secret` would be theater. Codegen-level CT verification is Phase C MIR territory per HANDOVER §6.1.

Forbidding `==`/`<`/`>` on secrets outright (strict comparison rule). Per D4: requires `constant_time_eq` builtin before any demo can type, and the prototype has no builtin mechanism. The cheap rule (D4 produces `Secret<Bool>`) makes the password demo fail in exactly the way HANDOVER §5.2 specifies, without introducing builtins. Strict rule remains available as a future tightening.

Full qualifier polymorphism in B4. Per D2: the no-α-leak unification restriction is correct-by-rejection without the inference machinery cost. Forward-compatible.

First-class `declassify` as a function. Per D5: defeats the audit-point property that declassification sites are syntactically distinguishable. `declassify` is a special form, parser-rejected at non-call positions.

Memory-access-at-secret-index check. Per D3: Sentinel-Mini has no arrays or memory primitives in B4. Defer to whatever phase introduces them.

Speculative-execution barriers / `lfence` insertion. Per D3 and HANDOVER §6.1: codegen-level, Phase C.

Secret memory lifecycle integration with the broker. The broker has `SecretStrategy` (Phase A7); Sentinel-Mini's `Value` representation does not currently allocate from the broker at all. Broker-as-value-heap integration is the "B?" backlog item in STATE.md §B.1's phase tracker. The `secret T` qualifier in B4 is purely type-level and does not require any allocator coordination. When broker-as-value-heap lands, secret-typed values should route to a `SecretStrategy`-wrapped arena, but that integration is its own ADR.

## D9 amendment (2026-05-25, B4.0 landed)

B4.0 closed in three commits:

- **B4.0a** (1693b8c) — surface AST + `SecretsNotYetSupported` placeholder. `Ty::Secret(Box<Ty>)` + idempotent `Ty::secret` smart constructor (D1); `TyExpr::Secret(Box<TyExpr>, Span)` + `ExprKind::Declassify { inner, span }` (D5/D6 surface); `Subst::apply` recurses via `Ty::secret`; `unify` adds `(Secret, Secret)` structural arm. `eval` `Declassify` arm delegates to `inner.eval` — unreachable in B4.0 via the full pipeline because inference rejects first, but exists so eval is total over `ExprKind`. Placeholder fires from `infer`'s `Declassify` arm and from `infer_program`'s effect-decl walker (`tyexpr_find_secret_span`).
- **B4.0b** (63cd57b) — lexer `Token::Secret` + `Token::Declassify`, both globally reserved.
- **B4.0c** (0b6b2ce) — parser surface. `secret T` prefix on type atoms binding tighter than `->` (precedence chosen to make single-arrow effect signatures `Int -> secret Bytes` paren-free); `declassify(e)` atom-precedence expression with mandatory parens (D5 audit-point property). `ParseError::DoubleSecret` rejects literal `secret secret T` and `secret (secret T)` — the smart constructor still collapses, this is the human-source early complaint.

D1 (shape (b) over (a)/(c)), D5 (declassify as special form, parser-rejected at non-call positions), and D6 (surface syntax) survived first contact with the lexer/parser/AST and are confirmed.

D7 (effect signatures may mention `secret`) is parser-confirmed (the parser accepts `effect ReadKey : Int -> secret Int ;`) but inference-rejected with `SecretsNotYetSupported` in B4.0. Full D7 confirmation lands when B4.1 removes the placeholder and exercises an effect-decl-with-secret end to end.

D2 (no-α-leak unification restriction), D3 (the four CT rejections: `SecretBranch`, `SecretDivisor`, `SecretFlow`, `SecretEscapesPolymorphism`), D4 (comparisons on secrets produce `Secret<Bool>`), D8 (generalization participation) are pending B4.1. The current `unify` `Var` arm will happily bind a TyVar to a `Ty::Secret(_)` — D2 enforcement is B4.1 — but this is unobservable in B4.0 because every entry point that introduces `Ty::Secret` into inference is gated by the placeholder.

Test coverage: 209 (192 lib + 17 integration), net +26 from B3.2's 183 (192 - 169 = +23 lib, +3 integration). Of the +23 lib: 5 `b40_*` in types.rs (Display/idempotency/recursion), 3 `b40a_*` placeholder tests in infer.rs (build AST directly because lexer doesn't recognise the keywords until B4.0b), 6 `b40b_*` lexer tests (keyword tokenisation + reserved-status), 9 `b40c_*` parser tests (precedence, paren behavior, DoubleSecret, declassify atom). Integration: declassify-rejected-at-infer, effect-decl-with-secret-rejected-at-infer, double-secret-rejected-at-parse.

## D9 amendment (2026-05-25, B4.1 landed)

B4.1 closed in two commits:

- **B4.1a** (e760d57) — foundation. Four new `TypeError` variants land: `SecretFlow { from, to, span }` (the public/secret unification failure, raised by the catch-all arm of `unify` when one side is `Ty::Secret(_)` and the other is non-secret-non-Var); `SecretEscapesPolymorphism { var, span }` (D2 no-α-leak, raised by the Var arm of `unify` when a bare TyVar would bind to `Ty::Secret(_)`); `SecretBranch { span }` (declared in B4.1a, fires in B4.1b); `SecretDivisor { span }` (declared in B4.1a, fires in B4.1b). The `unify` Var arm gains the D2 short-circuit; the catch-all Mismatch arm splits into SecretFlow vs Mismatch. The Declassify infer arm replaces its B4.0a placeholder with the real D5 rule (unify against `Ty::Secret(α)` and return `s.apply(α)`).

- **B4.1b** (52acc0a) — CT-specific rejections + cleanup. The If arm rejects secret cond with `SecretBranch` before the Bool unify (dedicated diagnostic, not the generic SecretFlow). The BinOp arm rejects Div with a secret divisor with `SecretDivisor`. The BinOp Eq/Lt/Gt arms gain the D4 rule: when either operand is `Ty::Secret(_)`, unwrap that side, unify the other against the inner, and produce `Ty::Secret(Bool)`. For Lt/Gt the inner additionally unifies against `Int` (existing binop_signature constraint, post-D4-unwrap). The `infer_program` `tyexpr_find_secret_span` walker + `SecretsNotYetSupported` variant are deleted -- D2/D3/D4 collectively ensure secret values from `do L(arg)` have nowhere unsafe to flow.

After B4.1: D1 (shape), D2 (no-α-leak), D3 (the four CT rejections), D4 (Secret<Bool> comparisons), D5 (Declassify special form + typing rule), D6 (surface syntax), and D7 (effect signatures may mention secret) are all confirmed by tests. D8 (generalization) is implicit via B4.0a's structural recursion of `collect_free_vars` and `collect_free_row_vars` into `Ty::Secret(_)`; no dedicated test added because no surface program exposes the generalization boundary with a polymorphic secret-typed value (D2 prevents that by construction).

Test coverage: 222 (203 lib + 19 integration), net +13 from B4.0's 209. B4.1a: +5 lib net (1 rename of the B4.0a placeholder Declassify test from "rejected with placeholder" to "is secret flow"; 5 new tests covering D2 direct, Declassify-positive via synthetic env, SecretFlow via catch-all, Secret-Secret recursion sanity, Secret-Int-vs-Secret-Bool mismatch on inners). B4.1b: +8 lib net (2 in-place rewrites of the B4.0a effect-decl-rejection tests as positive type-checks; 6 new tests covering SecretBranch, SecretDivisor, D4 on three comparison shapes, D4-Lt-Bool-rejects-on-inner) and +2 integration net (1 in-place rewrite of the effect-decl-rejection test as a positive type-check; 1 password-verify chain rejects with SecretBranch — the HANDOVER §5.2 deliverable; 1 secret-in-arithmetic SecretFlow end-to-end).

A positive end-to-end test for declassify-on-Secret cannot be written because D5 intentionally omits a `classify` primitive (the Secret-introduction dual). The Secret-introducing path is restricted to typing of `do L(arg)` for effect-decls naming secret; resuming a continuation requires producing a Secret value, and the surface has no form that does so. Positive D5 coverage lives in lib's `b41a_declassify_on_secret_unwraps_the_inner_type` via a synthetic env. The integration tests file carries an inline note explaining the gap.

## D9 amendment (2026-05-25, B4.2 not yet started)

B4.2 is the demo-polish phase. Scope per ADR 0008 D9 + HANDOVER §5.2:

1. The HANDOVER §5.2 password-verify demo already lives as `pipeline_password_verify_naive_rejects_with_secret_branch` (B4.1b). B4.2 may add a more polished version with realistic naming and a comment block explaining the CT rationale, possibly elevated to a featured example.

2. If a "passing rewrite" of the demo is desired (a version that type-checks using a constant-time equality primitive), Sentinel-Mini needs a builtin-function mechanism, which the prototype does not have. ADR 0008 D9 calls this out as a fallback: "a simpler rewrite using `declassify` after some explicit CT step (or just omitting the passing case from the demo) is the fallback." The rejection deliverable is the load-bearing one and is complete.

3. Documentation work: README / example files / inline lints may reference the new surface; any such cleanup belongs here.

4. Optional: revisit the gap that no positive end-to-end `declassify` test exists. If a tiny built-in `classify : T -> secret T` is added (with a strong audit comment that it would not exist in real Sentinel), a positive test becomes possible. This is a B4.2 design call.

If B4.2 only does (1) and (3), it can be a small docs-and-naming commit.
