# STATE.md — Sentinel Implementation Status

This document tracks what is actually built. When it disagrees with
HANDOVER.md, STATE.md is the source of truth. New contributors (or
new chat sessions) should be able to read this file and understand
the current state of the workspace without re-reading every commit.

The workspace has the complete Phase A broker (production-shape memory
subsystem) + the Phase B sentinel-effects-proto (Sentinel-Mini,
research-grade interpreter) + the **complete Phase C bootstrap compiler**:
every sentinel-* crate per ADR 0009 is populated (syntax/ast/resolve/
types/borrow-check/effect-check/hir/mir/codegen/runtime/driver), lowering
the full Sentinel language to native code via LLVM 18. **Phase C closed at
Sentinel 1.0 (2026-05-30).** sentinel-lsp remains a stub (post-1.0); the
next phase is **D (self-hosting)**.

Last updated: **Phase D movement 2 — the SELF-HOST PORT — (4/N) TYPES is COMPLETE;
ADR 0041 → ACCEPTED. (4a) oracle + probes + (4b) m-1 SCALAR + (4c) STRUCTS/ARRAYS/
NULLABLE + (4d) SECRET + (4e) ENUM/MATCH + (4f) CLASSES/TRAITS/IMPLS+DELEGATES + (4g)
EFFECTS/HANDLERS + (4h) GENERICS + (4i) CHAR/STRING + Vec<T> + CONCURRENCY + THE
FULL-CORPUS PHASE-GO (A1–A13) ALL LANDED.** `selfhost/types.sentinel` matches `snc
types` byte-for-byte over the ENTIRE clean-typing corpus (**123/123 fixtures**,
`sentinel_typer_matches_oracle_on_corpus`), leak-free, 0 regressions — the 4th Sentinel
stage is DONE. **(5/N) EFFECT-CHECK is COMPLETE — ADR 0042 → ACCEPTED** (owner-chosen
over borrow-check/HIR-MIR/codegen; the smallest stage since the lexer, 758 lines):
the `snc effects` oracle (`run_effects` + `effects_dump.rs`) + `selfhost/effects.sentinel`
(the 5th Sentinel stage — self-contained; effect rows as i64 BITMASKS, a precompute walk
recording call-graph edges + own-effect masks, the rebuild-per-sweep fixed-point via
recursion, handler/scope discharge) match `snc effects` byte-for-byte over the ENTIRE
clean-effect corpus (**122/122 fixtures**, `sentinel_effect_checker_matches_oracle_on_corpus`),
leak-free, 0 regressions; (5a)+(5b) converged in one build. The port now has **lexer +
parser + resolve + types + effect-check** all done. **(6/N) BORROW-CHECK is COMPLETE —
ADR 0043 → ACCEPTED** (owner-chosen over the transform half; ⚠ the BACK-HALF INFLECTION —
borrow-check needs the FULL typed program for Copy/Move move-analysis, so the owner chose to
REUSE `types.sentinel`): the `snc borrow` oracle (`DropPlan.moved_sources` per fn) +
`selfhost/borrow.sentinel` (the 6th Sentinel stage + the FIRST to REUSE a prior one — the
move analysis is FUSED into the typer's pass-2 walk via a `consuming` side-flag, and
`borrow.sentinel` reuses it through a D.6 chain borrow→types→parser calling `types::run`
in mode 1) match `snc borrow` byte-for-byte over the ENTIRE clean-borrow corpus
(**123/123 fixtures**, `sentinel_borrow_checker_matches_oracle_on_corpus`), leak-free,
`snc types` byte-identical. ⚠ builtin args are non-consuming + a match scrutinee is not a
move (oracle-revealed). The port now has **lexer+parser+resolve+types+effect-check+
borrow-check** all done — the whole FRONT END + both analysis passes. **▶ (7/N) MIR + the
CONSTANT-TIME VERIFIER OPENED — ADR 0044 PROPOSED (owner-chosen over codegen-first).** The
back-half scout REFRAMED the handover's "HIR/MIR → codegen": **HIR is a no-op** (a 101-line
identity bundle), **MIR is an analysis SIDE-BRANCH** (feeds only `verify_constant_time`;
codegen reads the `TypedProgram` directly via `hir.program()`), and **codegen is the real
transform** (→ a separate **8/N**, where Sentinel's lack of LLVM FFI forces an
emission-target call). So **(7/N) = the LAST analysis pass** (the 4th typed-program gate:
types→effect→borrow→**const-time**): port `lower_to_mir` (→ a minimal SSA/CFG IR) +
`verify_constant_time` (the secret-leak gate); oracle = a new `snc mir` lowered-form dump
(the load-bearing differential — the verifier is near-empty on clean fixtures); reuses the
6/N `types::run`-with-`mode` template (a new `mode 2`); a ⅛-scale rehearsal of codegen's
SSA/branch-merge transform. ⚠ the data-model risk = `var_defs` (`VarId→MirValue`) is
index-assigned in Rust → use the resolve append-only-scope idiom (D4). NEXT-after = **(8/N)
codegen + the bootstrap fixed-point.** Types is the **biggest/hardest
port stage** (the Rust `sentinel-types` is **10,891 lines, ~1.8× resolve**: HM-ish
inference, method/trait dispatch, secret-type propagation, match exhaustiveness;
effect-check is a SEPARATE crate, out of scope). **`selfhost/types.sentinel`, the
FOURTH Sentinel stage**, type-checks the SCALAR grammar (m-1) + **structs (4c-1)**
(decl tables + struct-lit + field-access `field_index`, the `Struct` interner kind
rendered via a struct-name blob) + **arrays (4c-2)** (lit + index + the
generic-builtin `(targs T)` mechanism for `len`) + **nullable (4c-3)** (`?T` + `null`
+ the implicit `T → ?T` widening, `widen-null`) + **secret (4d)** (`secret T` +
`declassify` + the `T → secret T` widening `widen-secret` + the secret-preserving
operators — `secret == secret → secret bool`) + **enum/match (4e)** (enum decls + the
`Enum::Variant` → `enum-construct` split with `variant_index` + `match` with payload
`(bind #vid name ty)` binding + the wildcard) + **classes/traits/impls (4f core)** (the
receiver-typed method-dispatch split `(method #cid …)`/`(impl-method #iid …)`, class-init,
`self` typing, named-impl `qcall-impl`, `&`/`&mut`/`*` ref typing, **+ delegate-impl
synthesis**) + **effects/handlers (4g)** (effect decls, `perform`, `handle` arms +
resume-kont, the effect-op-param VarId offset) emitting the `snc types` dump
byte-for-byte; **matches the oracle on 108 corpus fixtures + 48 seeds**
(`sentinel_typer_matches_oracle_on_seeds`), leak-free. ⚠ **D3 REFINED →
self-contained** (NOT module-reuse of resolve): resolve's `RCtx` can't be extended
with type fields across a module boundary, so `types.sentinel` is one clean `TyCtx`
(resolve's name-blob scope + fn table + the verified D4 type interner + the
append-only env), importing only the **parser**. KEY: `dump_texpr` RETURNS each
node's type handle + `close_ty` appends ` :<type>`; a 3-pass `main`
(names → sigs/fields → emit) so forward struct/fn refs resolve; ⚠ the struct/field
tables key on a separate `snb` blob (NOT the scope blob `nb` — a dedicated `snb_eq`).
**(4c-3) NULLABLE resolved the ADR 0041 A5 leak via path (a): the expected type
`exp` is threaded through `dump_texpr` ITSELF** (one self-recursive walker — a 6th
param, `-1` = none — never a SEPARATE mutually-recursive consumer, which leaked the
Move-enum it destructured; the reusable Sentinel finding in A6). Leaves widen inline
(`widen_pre`/`widen_post`); `Binary` builds in a temp then `widen_splice`s the
wrapper; `null` types from `exp`; `If`/`Block` thread `exp`; threading points = fn
return / `let` annotation (`tyopt_exp`) / struct field / `==`/`!=` operand. **(4d)
SECRET (A7) reused that machinery wholesale** — the widen generalised over `?T` (kind
4) + `secret T` (kind 3) via one `widen_kind` (the ` :T` suffix needs no kind — it
renders from `exp`); `declassify` strips a secret layer (`strip_secret`); cmp/logic
yield `secret bool` on a secret operand (arithmetic propagates FOR FREE via `resty =
lt`; mixed public+secret is a `Mismatch`, oracle-rejected). **(4e) ENUM/MATCH (A8)**
added the enum + flat variant tables (names in `snb`, payload types in a `varpay`
slice; Enum interner kind 7), the `Qcall`→`enum-construct` split (`variant_index` +
`:Enum`), and `match` (scrutinee `EnumId`, per-arm `variant_index` + payload-typed
`(bind …)` scoped via `truncate_scope`, the wildcard, `exp` threaded to arm bodies).
**(4f) CLASSES/TRAITS/IMPLS (A9)** — the largest slice — added a **group-order VarId
restructure** (pass-2 types items into a shared `itembuf` in fns→classes→impls order,
then splices source-order) + the class/trait/impl tables (Class interner kind 8) + the
**method-dispatch split** (own method `(method #cid …)` vs default-impl `(impl-method
#iid …)` vs named `(qcall-impl …)`) + `self`/class-field typing + `&`/`&mut`/`*` ref
typing (a BONUS that unlocked the c20–c22 ref+borrow set + c25). **(4f-delegate, A10)**
closed it — `delegate f: T to Tr` synthesises a forwarding `impl _ as Tr for C`
(group D, after user impls; the field added to the class table; each method forwards
to `self.f.m(args)` — a dispatch on the delegate field). **(4f) is COMPLETE.** **(4g)
EFFECTS/HANDLERS (A11)** — effect-decl emit + effect tables (op return types) + `perform`
(→ op return type) + `handle` (body type; arms bind op-params+`k`; the `return v` arm) +
resume-kont (a Call whose callee is an in-scope var) + ⚠ **the effect-op-param VarId
offset** (the fn region starts at `voff`; the env gets `voff` phantom slots, else
`env[voff]` is OOB). +23 → 108 fixtures. **(4h) GENERICS (A12)** — the LAST types
feature (the HM-ish inference engine): two new interner kinds (**`TypeParam`** kind 9,
rendered `<T#idx>` + **`GenericInstance`** kind 10, rendered `Decl<a, b>`, hash-consed
on the StructId + arg CONTENT) + a per-decl **type-param scope** (`tp_setup`/`tp_lookup`/
`tp_reset`, a `tpb` blob, reset per decl) + the fn-signature table (`scan_fn_sig` records
`uftp`/`ufps`/`ufpe`/`ufret`) + **`unify_one` bidirectional inference** at a generic-fn
call (`dump_generic_call` dumps args to a temp + captures types, unifies declared-vs-arg
structurally, emits `(targs …)`, returns `subst_type` of the declared return) + generic
struct-lit/field-access **substitution** (the field type substituted by the instance's
type-args). +7 → **115 corpus fixtures** (the six c17 + a bonus `c25_generic_struct_array_drop`),
leak-free, 0 regressions. **(4i) THE FULL-CORPUS PHASE-GO (A13)** closed the stage —
three feat increments fixing the last 8 diffs: **char/string literals** (the `Char`→`u8`
+ `Str`→`[u8]` `dump_texpr` arms; +c5d2/c5d5_break_continue → 117), **`Vec<T>` typing**
(interner kind 11 + `vec_new`/`push`/`pop`/index/`len`/`vec_to_array` + the `String` =
`Vec<u8>` alias + `read_file`→`[u8]`; +c5d3/c5d5_loops/c5d4/selfhost_ast_drop → 121),
and **concurrency + a delegate-field-order fix** (kind 12 `Task<T>` + `scope`/`spawn`/
`.await`; the oracle buckets regular fields before delegate fields → `register_delegate_fields`
appends delegate fields after `scan_cmem_loop`; +c44/c4 → 123). Wired
`sentinel_typer_matches_oracle_on_corpus` (the D9 phase-go) → **123/123 clean-typing
fixtures match, leak-free. ADR 0041 → ACCEPTED; the TYPES stage is COMPLETE.** NEXT =
**(5/N)** the next self-host stage (ADR 0038 D5: HIR/MIR → codegen; borrow-check /
effect-check are separate crates — its own kickoff ADR, per the 0039/0040/0041 cadence).
**(4a) shipped the ORACLE + de-risked the Sentinel build:** `snc types <file>`
(driver `run_types` + `types_dump.rs`, parse → resolve → `check` → dump) emits the
`snc resolve` S-expr form **extended with each expression node's inferred `Type`** (a
trailing ` :<type>` via `type_display`) **+ the type-resolved disambiguations** — the
resolver's uniform `(method …)` split by receiver type into `(method #classid <idx>
…)` / `(impl-method …)`, the synthesized `(widen-null …)`/`(widen-secret …)`
coercions, the `let`'s inferred type (replacing resolve's `_`), and the computed
`field_index`/`variant_index`/generic `(targs …)`. **Robust: 0 panics across all 141
corpus fixtures** (123 type cleanly, 18 oracle-rejected → skipped by the differential,
as resolve); 5 goldens (`tests/types.rs`) pin params/call, let-inference,
struct-lit+field-index, nullable-widen, method-dispatch. **Two parallel design probes
(A1) settled + EMPIRICALLY VERIFIED the Sentinel-side model PRE-BUILD** (orchestrator
re-ran each `snc build`+run+`leaks` → **0 leaks** apiece): **D4** — a `Type` is an
**integer type-handle into a flat hash-consed interner** (parallel `Vec<i64>` arrays;
structural equality = integer `==`; recursive render/subst), with the VarId→type env
**APPEND-ONLY** (`env[vid]=h` is a hard `IndexAssignNotSupported`; index-READ is fine)
+ 3 reusable interner gotchas (byte-literal push needs a char lit; nested `&mut ctx`
in one call → bind inner first; inline `print_bytes(vec_to_array(v))` leaks → bind
`let arr`); **D3** — module-reuse of `resolve.sentinel` was probe-VERIFIED viable (a
3-deep chain + diamond import) but **refined at the (4b) build to self-contained**
(A3): resolve's `RCtx` can't be extended with type fields across a module boundary, so
`types.sentinel` is one clean `TyCtx` importing only the parser. **Four-check green
(1422 tests).** The prior banner (the (3/N) RESOLVE, COMPLETE, ADR 0040 → ACCEPTED)
stands below.

Prior banner: **(3/N) RESOLVE is
COMPLETE; ADR 0040 → ACCEPTED** (A2–A12): `selfhost/resolve.sentinel`, the third
compiler stage in Sentinel, name-resolves the **entire expression + decl grammar**
— every decl kind + every `::`-path form (struct-lit / enum-construct / perform /
qcall-impl / class-init / resume-kont, method/init bodies binding `self`) **+ the
SCOPED bodies (3c)** (`match` / `while` / `handle`) via a name-blob scope +
during-walk binding + D5 length-truncation — **+ GROUP-ORDER resolution** (resolve
all fns → `classvid = fnvid` → all classes → `implvid = classvid` → all impls; emit
per-item buffers in source order) **+ delegate-impl synthesis** (a `delegate` → a
class field + a synthesised forwarding `impl` whose ImplIds/VarIds continue the
impl region). **`sentinel_resolver_matches_oracle_on_corpus` matches `snc resolve`
byte-for-byte over the ENTIRE clean-resolving `tests/pass`+`tests/ui` corpus (132
fixtures, the D9 phase-go), leak-free.** The one cross-stage edit: the parser
STORES handler-arm params (`snc ast` byte-unchanged). NEXT was the **types** stage
(now open — see the top banner). The (2/N) PARSER (COMPLETE, ADR 0039 → ACCEPTED,
A1–A22) detail follows.

Earlier: **The (3a) resolve oracle LANDED:**
`snc resolve <file>` (`run_resolve` + `resolve_dump.rs`) dumps the
`ResolvedProgram` as the `snc ast` form extended with the resolved IDs
(`(var #N)` / `(call #14 …)` / `(struct-lit #N …)` / `(qcall-impl …)` /
`(enum-construct …)`…; builtins are FnId 0–13, user fns #14+; the built-in
`Async` effect emits deterministically last) — robust over the corpus (0 panics
/ 141), 4 goldens. **Then 3 parallel AGENT PROBES (ADR 0040 A1; see
`docs/agent-protocol.md`) settled the design PRE-BUILD** — D3 parse-sharing
CONFIRMED (resolve.sentinel can `use` the parser's AST via a D.6 module), and
**D4/D5 CORRECTED: the cons-list symbol tables/scopes are unusable** (can't
`match` a `&enum`; a `&`-ref enum `match` aliases the heap → double-free on
reuse; partial cons-cell `truncate` leaks) — replaced by **flat parallel-`Vec`
arrays + a packed-name byte blob, integer-indexed, length-truncation restore**
(the parser's token-array idiom; prototyped leak-clean over a 100-arm loop).
**(3a) milestone-1 has LANDED** — `selfhost/resolve.sentinel` (`06f9241`), the
**THIRD compiler stage in Sentinel**, `use`s `parser.sentinel` as a **D.6 module**
(the A1-confirmed parse-sharing: import the parser's `Expr` + parse fns, walk the
AST by value), name-resolves it, and matches `snc resolve` for paramful fns +
free calls + arithmetic + comparisons + unary + var refs. A 2-pass driver (pass 1
= flat-`Vec` user-fn table, builtins #0–13 + user fns #14+; pass 2 per-fn = parse
header → `(param #varid …)` + scope, `parse_block` body → 1-phase consuming
`dump_rexpr`, then the `(effect #N Async)` tail) on A1's corrected flat-`Vec`
model (src-slice keys, varid = scope index, names sunk to a reclaimed `garbage`).
**Compiled first try.** **(3a) m-2 (A3) then added `let`** — the binding set is
**pre-scanned** from the body tokens into the immutable scope (a `let` binds
during the walk, but the leak-free design wants the scope frozen first), so a
`let` resolves its own VarId by name; `(let #vid [mut] <ty> <value>)`, VarIds
continuing after the params. **17 differential seeds match `snc resolve`,
leak-free; (3a) is feature-complete for the fn-body grammar.** **(3b-1) (A4) then
opened the decl walk** — both passes now handle top-level decls (pass 1 a
brace-depth scan so method `fn`s inside trait/impl/class bodies are not counted as
top-level fns; pass 2 a source-order dispatch, `resolve_item`), the symbol tables
are bundled in an **`&mut RCtx`** struct (A4 probe — a Sentinel struct of `Vec`
fields supports `push`/`len`/index through the ref + cross-field read+write,
leak-free; always `(*c).field`; no `&mut`→`&` reborrow; never move a `Vec` field
out), and the first decl kind lands end to end: `(struct #id Name (field …))`
heads (source order, interleaved with fns) + struct-lit `#id` (a struct consumes
no VarId). **(3b-2) (A5) added the enum table** — `(enum #id Name (variant V
<payloads>…)…)` heads (EnumId is its OWN source-order namespace, coexisting with
struct ids) + the `Enum::Variant` → `(enum-construct #E Enum Variant args)` split
(the enum table checked FIRST in BOTH the parser's uniform `Qcall` and `ClassInit`
arms; purely additive — one more `RCtx` table, zero new params). **(3b-3) (A6)
added the effect table** — `(effect #id Name (op name (params) ret)…)` heads (user
effects in source order, `Async` last) + `perform` → `(perform #E op_index Eff op
args)` (op_index = the op's position in the effect's op list). ⚠ **effect op
params consume GLOBAL VarIds** in the Rust resolver (before any fn body), so every
fn's VarIds are OFFSET by the total effect-op-param count regardless of source
order — reproduced with **phantom scope slots** (one dummy entry per op param,
below every fn's `base`, never looked up). **39 differential seeds match `snc
resolve`, leak-free.** **(3b-4) (A7) added trait + impl** — `(trait #id …)` heads
(sigs, no bodies) + `(impl #id name #trait_id Trait struct#tid Type (method
#selfvid …))` heads with method BODIES (a synthetic `self` VarId, then params,
then body lets) + the `Qcall` else-branch → `(qcall-impl #I method_index ImplName
method args)` (method_index = the method's position in the impl's trait's methods).
⚠ **method-body VarIds are GROUP-ordered** (the Rust resolver resolves ALL fn
bodies before ANY impl method, lib.rs:2559), so `varid ≠ scope-array index` — the
scope now stores EXPLICIT varids (`scv`), the fn region counts from `voff` and the
impl region from `voff + total-fn-bindings` (the (3b-4a) rearchitecture retired the
(3b-3) phantom slots). **(3b-5) (A8) added class + closed (3b)** — `(class #id Name
(field …) (init #selfvid (params) <block>)? (method #selfvid …)…)` heads (BUCKETED
fields/init/methods, init+method bodies binding `self` — the init's a synthetic
`-1` sentinel matched by name) + `(class-init #C Name args)` + `resume-kont` (a
`Call` whose callee is an in-scope var, scope checked BEFORE the fn table). The
class group resolves BEFORE impls, so `classvid` counts from `voff +
total-fn-bindings` and `implvid` from `+ total-class-bindings`. **52 differential
seeds match `snc resolve`, leak-free. (3b) is COMPLETE.** **(3c) (A9/A10) then
closed the SCOPED bodies** — `match` arm payloads, `while`, `handle` (handler-arm
params + the return arm). Two model corrections (probe-confirmed): bindings are
DURING-WALK in source order (a `let b = match …{E::A(x)=>x}` binds `x` to the lower
VarId, then `b`), and the scope is a **NAME BLOB** (body bindings come from the AST
as owned `[u8]`, not token slices — their bytes are pushed into `ctx.nb`). Arm
scopes use **D5 length-truncation** (`truncate_scope` pops the scope to a pre-arm
mark; VarIds stay sequential). `handle` needed one cross-stage parser edit (the
parser now STORES handler-arm params as a `Binds`; `snc ast` byte-unchanged). **61
differential seeds match `snc resolve`, leak-free. (3c) is COMPLETE — the full
grammar resolves.** **(3e) (A11) then landed GROUP-ORDER resolution + the corpus
phase-go** — the class/impl VarId base was wrongly token-counted (missing the 3c
match/handler/return fn-region bindings; c5_go_no_go: a `handle` in a fn → off by
2); fixed by resolving in three groups (fns → classes → impls; each region's base
= the prior's FINAL counter) + emitting per-item buffers in source order. **The
corpus differential matches `snc resolve` over 130 fixtures, leak-free.** **(A12)
then closed delegate-impl synthesis** — a `class C { delegate f: T to Tr; }` →
`(field f T)` (a delegate-field bucket after the explicit fields) + a SYNTHESISED
forwarding `(impl _ as Tr for C { fn m(self,…) { self.f.m(…) } })` per trait method
(re-walked from the trait sigs; ImplIds + VarIds continue the impl region; emitted
at the class's source index). **The corpus differential now covers the ENTIRE
clean-resolving corpus (132 fixtures); ADR 0040 → ACCEPTED. (3/N) RESOLVE is
COMPLETE.** **NEXT = the types stage (its own ADR — write PROPOSED first).**
Movement 1 (the
language/stdlib build-out,
ADR 0031 D2) is **complete** — D.1 sum types + `match`, D.2 strings + `u8`, D.3
`Vec<T>`, D.4 file I/O, D.5 loops, D.6 modules — so **the language gate for
self-hosting is cleared**. Movement 2 ports `snc` to Sentinel stage by stage, each
**differentially validated against the Rust `snc` oracle** (ADR 0031 D5). **(1/N)
the LEXER has landed:** `snc lex <file>` (the oracle — a canonical token dump) +
`selfhost/lexer.sentinel` (the FIRST compiler stage in Sentinel, reproducing all 69
`TokenKind`s) + a corpus-wide differential test asserting the Sentinel lexer's dump
equals `snc lex` for **every clean-lexing fixture (139/139** in `tests/pass` +
`tests/ui`; the lone deliberate lex-error fixture is excluded — error parity is a
follow-on). It dogfoods the post-1.0 language (`[u8]`/`u8`, `Vec`, `read_file`/
`print_bytes`, `while`). The Rust `snc` stays the production compiler + oracle until
the bootstrap fixed-point bakes. NEXT: **(2/N) the parser — ADR 0039 → ACCEPTED-WITH-AMENDMENTS; (2a) LANDED.**
The first parser slice is in Sentinel + oracle-validated: **(i)** the **`snc ast`
canonical-dump oracle** (`run_ast` + `ast_dump.rs`, golden-tested) — a complete
regular S-expr form, e.g. `(fn main () i64 (block (binop + (int 1) (binop * (int
2) (int 3)))))` (a fresh dump, since `snc parse`'s `Display` omits
enums/traits/impls/classes); **(ii)** **`selfhost/parser.sentinel`** — the SECOND
compiler stage in Sentinel: tokenizes, recursive-descent parses each paramless
`fn NAME() -> TYPE { <expr> }` into a recursive `Expr` AST, and dumps it
byte-identically to `snc ast` for integer arithmetic (`+ - * /` precedence,
parens, left-assoc, multi-fn) — verified by `tests/selfhost_parse.rs` (5 seeds),
leak-free. The structure (ADR 0039 A3, proven by probes): recursive `Expr` enum
returned by value + consuming-dumped (no `Vec<Expr>`); helpers share the token
arrays + `src` via `(*r)[i]` deref-index + a `&mut i64` cursor; left-assoc folds
via recursion (a param accumulator — a loop one trips the moved-in-loop rule).
Also locked: the recursive-AST drop gate (`selfhost_ast_drop.sentinel`, 0 leaks).
**(2b) is now UNDERWAY** — D6's "full expressions" slice is itself sub-sliced
(~28 `ExprKind` variants); **increments 1 + 2 have LANDED.** **(2b) increment-1
(ADR 0039 A4):** the **complete operator-precedence ladder** mirroring the Rust
parser (`expr → or → and → cmp → bitor → bitxor → bitand → add → mul → unary →
postfix → atom`) — logical `|| &&`, the six non-associative comparisons
`== != < <= > >=`, bitwise `| ^ &`, additive `+ -`, multiplicative `* /`, prefix
unary `- !`, parens — plus the scalar atom leaves (integer / `true` / `false` /
`null` literals + variable refs). **(2b) increment-2 (ADR 0039 A5):** function
calls `f(args)` → `(call …)` (an atom case; the callee is a name) + the POSTFIX
chain — field `t.field` → `(field …)`, index `t[i]` → `(index …)`, method
`t.m(args)` → `(method …)` — applied left-to-right via a new `parse_postfix`
layer. **(2b) increment-3 (ADR 0039 A6):** the identifier-prefixed `::` paths in
`parse_atom` — qualified call `A::b(args)` → `(qcall …)`, class init
`Name::init(args)` → `(class-init …)`, and paren-less `Enum::Variant` (a qcall with
empty args; enum-vs-impl is a resolve concern) — plus array literals
`[e1, e2, …]` → `(array …)` (atom-position `[`, distinct from the postfix index
`[`). `Expr` gained `Bool`/`Null`/`Var([u8])`/`Unary` + a unified
`Binary(op-code, …)` (inc-1), then `Call`/`Method`/`Field`/`Index` (inc-2), then
`Qcall`/`ClassInit`/`Array` (inc-3), then `If`/`BlockE` (inc-4), then `Match` +
the `Arms`/`Pattern`/`Binds` enums (inc-5); argument + element lists are a **second
mutually-recursive cons-list enum** `Args = End | Cell(Expr, Args)` (since
`Vec<non-primitive>` is unsupported — de-risked by a probe), `parse_args` generalised
with a terminator-tag param. **(2b) increment-4 (ADR 0039 A7):** `if … { … } else
{ … }` → `(if cond (block …) (block …))` — dispatched at the TOP of `parse_expr`
(never an operator operand), `else` mandatory, `else if` chains — plus brace blocks
`{ <expr> }` → `(block …)` (statement-FREE for now). **(2b) increment-5 (ADR 0039
A8):** `match <scrut> { pat => body, … }` → `(match scrut (arm pat body)…)` — also
dispatched at the top of `parse_expr`; patterns are `_` → `(pat _)` or a qualified
variant `Enum::Variant(b1, b2)` → `(pat Enum Variant b1 b2)`; the **deepest mutual
recursion yet** (four enums: `Expr → Arms → {Pattern → Binds, Expr}`, de-risked by a
probe). **(2b) increment-6 (ADR 0039 A9):** struct literals `Name { f: v, … }` →
`(struct-lit Name (field f v)…)` — disambiguated from an `if`/`match` head's block by
a **context-free `{ Ident :` lookahead** (a block never starts with a single-colon
`Ident :`), avoiding the Rust parser's threaded `allow_struct_lit` flag; `Expr`
gained `StructLit` + a `Fields` cons-list (empty `Name {}` deferred). **(2b)
increment-7 (ADR 0039 A10):** the effect / concurrency leaf forms — `declassify(e)`
→ `(declassify e)`, `perform Eff.op(args)` → `(perform Eff op args…)`,
`scope concurrent { block }` → `(scope (block …))`, `spawn <call>` → `(spawn …)`,
and the `.await` **postfix** → `(await target)`; `Expr` gained
`Declassify`/`Perform`/`Scope`/`Spawn`/`Await` + five keyword tags (scope/while
bodies stay statement-free until (2c)). **(2b) increment-8 (ADR 0039 A11):**
`handle <body> with { Eff.op(params) => arm, … return v => arm }` →
`(handle body (arm Eff op armbody)… (return v body))` — handler-arm params parsed
but NOT dumped; the optional `return` arm kept SEPARATE (a `&mut Ret` out-param,
the first non-primitive `&mut` assignment in the port — de-risked by a probe) so it
dumps LAST regardless of source order; `Expr` gained `Handle` + the `HArms`/`Ret`
enums. **This CLOSES the (2b) expression grammar** — every `ExprKind` the oracle
emits now parses. **(2c-1) (ADR 0039 A12):** a block is now a real `{ <stmt>*
<tail> }` → `(block <stmt>… <tail>)` — `let [mut] name = e` → `(let [mut] name _ e)`
(`_` = the type annotation, added at (2c-2)), `target = e` → `(assign …)`, `while
cond { body }` → `(while …)`, `break`/`continue` → `(break)`/`(continue)`, an
expr-statement → `(expr e)`; the fn body is now a real block. The block tail is a
`&mut Expr` out-param defaulting to a **nullary** `SynthZero` that dumps `(int 0)`
(the synth unit tail for a statement-only `while` body — a boxed `Int(0)` default
leaked, since `*tail = e` doesn't free the old enum). `Expr` gained `Block(Stmts,
Expr)`/`SynthZero` + new `Stmts`/`Stmt` enums; new tokens `;` + let/mut/while/break/
continue. **(2c-2) (ADR 0039 A13):** the optional `: type` on a `let` → `(let [mut]
name <type> e)` (vs `_`), via a `parse_type` mirroring the Rust one — `secret T`,
`&T`/`&mut T` → `(ref …)`/`(refmut …)`, `?T` → `(opt …)`, `[T]` → `(arr …)`, `Ident`,
`Ident<args>` → `(generic name args…)`; nested generics close without a `>>` split
(the tokenizer has no `>>`, so `Vec<Box<i64>>` lexes two `Gt`). New recursive
`TypeE` enum + `TyArgs` cons-list + a `TyOpt`; new tokens `?` (50) + `secret` (51).
The diff corpus is **135 seeds**, all matching `snc ast`, leak-free. **(2c-3) (ADR
0039 A14):** full `fn` definitions — `fn name <T>? ( [mut] p: T, … ) -> RET ! { eff
}? { body }` → `(fn name ((param [mut] p <type>) …) <ret> <block>)`; the return type
now routes through `parse_type` (so `[u8]`/`?T`/`Vec<T>` returns dump right); generic
type-params + the effect row are parsed-and-SKIPPED (the dump emits neither). New
`Params` enum. **This CLOSES the (2c) fn-level grammar** — every
`Stmt`/`Expr`/`TypeExpr`/`Pattern` + the fn header now parse. The diff corpus is
**148 seeds**, all matching `snc ast`, leak-free. **(2d) is now UNDERWAY** — the
last parser slice, the top-level decls, landed one kind per increment. **(2d-1)
(ADR 0039 A15):** `use a::b::Item;` → `(use a b Item)` + the top-level-decl
dispatch. The oracle now dumps every decl in **source order** (the parsed
`Program` is kind-bucketed, so `dump` re-sorts the decls by span start — the
order the Sentinel parser emits them as it scans); `main`'s fn-only loop becomes
a `dump_item` dispatcher on the leading token (an optional `pub` is
parsed-and-skipped). **(2d-2) (A16):** `struct Name { f: T, … }` →
`(struct Name (field f <type>) …)` (empty → `(struct Name)`; field types via
`parse_type`); a recursive `dump_struct_fields` parses + dumps each `name : type`
inline. **(2d-3) (A17):** `enum Name { V1, V2(T), … }` →
`(enum Name (variant V1) (variant V2 <type>) …)` (unit vs positional-payload
variants; empty → `(enum Name)`; non-generic). **(2d-4) (A18):** `effect Name {
op(p: T) -> R; … }` → `(effect Name (op op (<params>) <ret>) …)` (a missing op
return dumps `_`, reusing `TyOpt`; empty → `(effect Name)`). **(2d-5) (A19):**
`trait Name { fn m(self: &Self, …) -> R; … }` → `(trait Name (method m
<shared|exclusive> (<params>) <ret>) …)` (sigs have no body; `self` dumps as its
kind word; introduces `parse_self_kind` + `dump_method_head`, shared by
impl/class). **(2d-6) (A20):** `impl Name? as Trait for Type { … }` →
`(impl <name-or-_> Trait Type (method m <self> (<params>) <ret> <block>) …)` (a
default impl dumps `_`; methods carry a body). **(2d-7) (A21):** `class Name {
let f: T; init(…){…} fn m(self…){…} delegate g: T to Tr; }` → `(class Name (field
f <type>)… (init …)? (method …)… (delegate …)…)` — ⚠ class items **bucket**
(fields/init/methods/delegates), so they dump grouped (NOT source order); the
Sentinel parser scans once into 4 per-kind `Vec<u8>` buffers then concatenates.
**Every top-level decl kind now parses.** **(2d-8) (A22) CLOSES the parser
stage:** the self-contained tokenizer/parser gained `//` line comments, the
prefix `*`/`&`/`&mut` unary operators, and char/string literals (`'c'` →
`(char N)`, `"s"` → `(str b…)`, escapes decoded per ADR 0033 D2), and a leak in
the new string-dump arm was fixed (consume the `[u8]` by value) — so the parser
now matches `snc ast` over the **full `tests/pass` + `tests/ui` corpus
(139/139)**, leak-free, with a corpus differential test
(`sentinel_parser_matches_oracle_on_corpus`) as the phase-go. **The (2/N) PARSER
stage is COMPLETE → next port stage = resolve.** Recap of the movement-1 close:
after D.1 (sum types), D.2 (strings + `u8`), D.3 (growable `Vec<T>`), and D.4
(file I/O), the surface had been **recursion-only by design**; **D.5 adds loops** — a
compiler's iteration-heavy passes (scan a byte buffer, drain a token `Vec`) want
bounded, stack-safe iteration with early exit. **(1/N)** landed `while <cond> {
<body> }`: **a loop is a STATEMENT** (`StmtKind::While`, not an expression — no
loop value; Sentinel has no unit type), lowering to the **first backward CFG
branch** (loop_cond → loop_body → loop_after, body→cond back-edge); the body is a
`lower_block` scope, so its bindings **drop per-iteration** (a body that allocates
each pass is leak-free); the **loop-carried move rule** rejects moving an *outer*
Move-typed binding inside the cond/body (`MovedInLoopBody`); body allocas are
**hoisted to the entry block** inside loops (a `loop_depth` flag — a 2M-iteration
body-`let` loop is stack-safe; non-loop codegen byte-identical, the c51 bar holds).
**(2/N)** lands **`break` / `continue`** — payload-free **statements**
(`StmtKind::Break`/`Continue`, new `break`/`continue` lexer keywords) branching to
the innermost enclosing loop's `loop_after` / `loop_cond` via a **loop-target
stack** (`CodegenCtx::loop_targets`, the innermost loop = top; no labels). Load-
bearing (2/N) call: a `break`/`continue` branches out of the *middle* of the body,
skipping `lower_block`'s end-of-body drops, so codegen **drains every scope frame
down to the loop body BEFORE branching** (`emit_loop_exit_drops` over a
`scope_floor` — the body scope + any nested `if`/block scopes), keeping a heap
binding live at the branch leak-free (the fn-return early-exit shape, ADR 0017).
Since Sentinel has no early `return`, this is the **first mid-block divergence** —
the dead remainder is parked on an `after_loopctl` block. **Out of a loop**,
`break`/`continue` is rejected (`LoopControlOutsideLoop`) via a `loop_depth` on the
type env. Ergonomic note: a *conditional* break uses the tail idiom `if c { break;
0 } else { 0 };` (`if` requires `else` + a tail — pre-existing, not break's fault).
**No new `Type`, no cascade, no FnId-shift.** Phase-gos: `c5d5_loops.sentinel`
(while baseline) at **exit 67**; `c5d5_break_continue.sentinel` (break-terminated
sum + continue-filtered sum + two loops that break / continue with a `[u8]` live)
at **exit 115** — both **0 leaks** (`leaks --atExit`; verified incl. nested loops).
DEFERRED (ADR 0036 D8): `for` / ranges / iterators, labeled break,
`break`-with-value / loop-as-expression, a termination check. **1361 tests,
four-check green. Phase D.5 COMPLETE.** Now in progress: **#5 modules
(Phase D.6, ADR 0037)** — file-as-module + `use`, the last ADR 0031 D4
prerequisite before the self-host port. **Multi-file Sentinel now compiles
+ runs** (cross-module `pub fn` calls, collision-free module-qualified
names, `use`/`pub` visibility) via the lower-risk Path A merge; per-unit
separate-compilation back end (objects + mangling + link) is the follow-up.
**D.6 (1/N) — MULTI-FILE COMPILES + RUNS (green), via the lower-risk
PATH A** (whole-graph front-end + merge → existing pipeline; true per-unit
resolve/codegen deferred — owner-chosen). 7 increments: (1) `use` front-end;
(2) **module-graph discovery** (driver follows `use` edges to files —
source root = entry's dir, `use a::b::Item` → `a/b.sentinel`,
ModuleNotFound); (3) **top-level `pub`**; (4) **import resolution +
visibility** (`PrivateItem`/`UnknownImport`/`ModuleNotFound`); (5) **the
merge** (`merge_modules`: qualify fn names by module path `util::add` →
`util$add`, rewrite cross-module + own call refs per module scope, keep the
entry's `main`) → the EXISTING pipeline compiles the merged program → one
object → link; (6) **cross-module DATA TYPES** (`merge_modules` also
qualifies struct + enum names + rewrites EVERY type reference —
params/returns/fields/variant payloads/`let` annotations + struct literals
+ enum construction/patterns — via a per-module `Renamer` that skips
in-scope type parameters); (7) **cross-module TRAITS / EFFECTS / CLASSES**
(qualifies trait/effect/class/named-impl names too + rewrites their refs:
`impl as Trait for Type` heads, `perform`/`handle` effect names,
fn/method effect rows, delegate trait names) — so EVERY top-level item kind
is module-qualified and same-named items coexist; (8) **effect-check parity**
for the merged path (`run_build_merged` now calls the pure
`sentinel_effect_check::effect_check` between type-check and borrow-check —
the salsa chain's `borrow_check_query`→`effect_check_query` order — so a
multi-file `main` with an unhandled effect is rejected, not miscompiled).
**Verified end-to-end:** a cross-module `pub fn` (exit 5); same-named private
`helper`s coexist (exit 41, 0 leaks); private import → PrivateItem; a
cross-module `pub struct` (exit 42); a cross-module `pub enum` + `match`
(exit 52); same-named `struct Item`s coexist (exit 42); a 3-deep
cross-module struct field (exit 42); a cross-module `pub trait` impl'd +
dispatched (exit 42); same-named `class`es coexist (exit 42); a cross-module
`pub effect` performed + handled through the handler runtime (exit 42); an
UNHANDLED cross-module effect in `main` → rejected (UnhandledEffect); and
**cross-module GENERICS** (a `Box<i64>` struct, an `id<T>` fn, and
`Pair`/`make_pair`/`fst` — all instantiated in a different module than
their definition, exit 42) — they work for free because Path A's
whole-program `collect_mono_instantiations` runs over the merged graph.
Single-file unaffected. The merged path is **complete + sound**; its two
remaining follow-ups are now **independent deferred tracks, not blockers** for
self-hosting: the per-unit separate-compilation back end (per-unit objects +
module-qualified `abi-v1` mangling + multi-object link + the per-unit
`linkonce_odr` generic story, ADR 0037 (2/N)) and span-accurate multi-source
diagnostics (the merged path reports by message). With D.6 done, **movement 1's
language gate is cleared** and **movement 2 (the self-host port) opens — ADR 0038
PROPOSED** (first sub-phase: lexer-in-Sentinel; the port is back-end-agnostic, so
it builds on the Path A merge and does not wait on the per-unit back end).

Pre-D.5 context: **Phase D.4 — file I/O via a minimal stdlib — MVP COMPLETE
(1/N + 2/N; ADR 0035 → ACCEPTED-WITH-AMENDMENTS).** The fourth
prerequisite — **file I/O** — now compiles + runs end to end, leak-free: a
self-hosting compiler can read its source + write its artifact. **File I/O =
runtime builtins** (like `print`), **NOT** the algebraic-effect/handler machinery
(ADR 0035 D2 — effects are resumable user computations; OS I/O is irreversible
side effects, and the effect-check forbids an effectful `main`; this amends
ADR 0031 D4's "effects + handlers" framing). **(1/N)** landed
**`read_file([u8]) -> [u8]`** (read a whole file into an owned byte array;
`std::fs::read` + copy into a `sentinel_alloc`'d buffer freed at scope-exit drop;
count via out-param) and **`write_file([u8], [u8]) -> i64`** (create/truncate +
write). **(2/N)** lands **`print_bytes([u8]) -> i64`** (write a `[u8]` to stdout —
the byte companion to `print`; exact bytes, no newline, flushed). All three are
non-generic `[u8]` runtime builtins (the `str_eq` template), **panic-on-failure**
(D5), backed by `std::fs`-based `sentinel_read_file` / `sentinel_write_file` /
`sentinel_print_bytes` (abi-v1 now 23 symbols). Builtins are FnId 0..=13
(read_file=11, write_file=12, print_bytes=13; main → 14). Phase-go
`tests/pass/c5d4_file_io.sentinel` round-trips a file (write → read → `str_eq` + a
`back[i]` check + `len`) AND `print_bytes` the content → **exit 5, stdout
"hello", 0 leaks** (`leaks --atExit`); a missing file aborts (exit 134). DEFERRED
(ADR 0035 D8): a recoverable error model (`?[u8]`/`Result`), the `Io` effect-row
promotion, streaming / file handles / `seek`, `read_stdin`, directories / `stat`,
append, `secret` I/O. **1341 tests, four-check green. Phase D.4 MVP closes.**

Pre-D.4 context: **Phase D.4 (1/N)** landed `read_file` + `write_file`; the
fourth prerequisite. After D.1 (sum types), D.2
(strings + `u8`), and D.3 (growable `Vec<T>`), the fourth prerequisite — **file
I/O** — opens: a self-hosting compiler must read its source + write its
artifact. **File I/O = runtime builtins** (like `print`), **NOT** the
algebraic-effect/handler machinery (ADR 0035 D2 — effects are resumable user
computations; OS I/O is irreversible side effects, and the effect-check forbids
an effectful `main`; this amends ADR 0031 D4's "effects + handlers" framing).
**(1/N)** lands **`read_file(path: [u8]) -> [u8]`** (read a whole file into an
owned byte array — the runtime `std::fs::read`s it, copies into a
`sentinel_alloc`'d buffer freed at scope-exit drop, returns the count via an
out-param) and **`write_file(path: [u8], data: [u8]) -> i64`** (create/truncate
+ write; args borrowed). Both are non-generic `[u8]` runtime builtins (the
`str_eq` template), **panic-on-failure** (ADR 0035 D5; abort like OOB/bad-alloc),
backed by new libc-/`std::fs`-based `sentinel_read_file` / `sentinel_write_file`
symbols (abi-v1 now 22 symbols). Builtins are now FnId 0..=12 (read_file=11,
write_file=12; main → 13). Phase-go `tests/pass/c5d4_file_io.sentinel` round-trips
"hello" (write → read → `str_eq` + a `back[i]` byte check + `len`) at **exit 5, 0
leaks** (`leaks --atExit`); the missing-file abort exits 134 with a clear
message. DEFERRED (ADR 0035 D8): a recoverable error model (`?[u8]`/`Result`),
the `Io` effect-row promotion, streaming / file handles / `seek`, `read_stdin`,
directories / `stat`, append, `secret` I/O. **1339 tests, four-check green. Phase
D.4 (1/N) lands** (`print_bytes` + the ADR flip are (2/N)).

Pre-D.4 context: **Phase D.3 — growable collections (`Vec<T>`) — MVP COMPLETE
(1/N + 2/N; ADR 0034 → ACCEPTED-WITH-AMENDMENTS).** The third
prerequisite — **growable collections** — now compiles + runs end to end,
leak-free. A `Vec<T>` IS `[T]` + a capacity field + mutation
(`Type::Vec(VecElem)` mirrors `Type::Array(ArrayElem)`; `{ i64 len, i64 cap, ptr
data }`, data is field 2). **(1/N)** landed `vec_new`/`push`/`len` + the
`sentinel_realloc` growth runtime + `&mut Vec` mutable-borrow + primitive-element
drop. **(2/N)** lands `v[i]` (reusing the C1.6 bounds-checked Index — `lower_index`
keys the data field on the target type), `pop<T>(&mut Vec<T>) -> T` (decrement
`len`, trap on empty), the **`Vec<u8>`→`[u8]` bridge** `vec_to_array<T>(Vec<T>) ->
[T]` (non-consuming `memcpy` so a built string can be `str_eq`'d against a
keyword), and **`String` = `Vec<u8>`** (Amendment A1). `vec_new`/`push`/`pop`/
`vec_to_array` all type through the uniform generic-call path; builtins are now
FnId 0..=10 (main → 11). Phase-go `tests/pass/c5d3_collections.sentinel`
(`Vec<i64>` push/index/pop/len + escape; `String` "let" built/indexed/bridged/
`str_eq`'d) runs at **exit 55, 0 leaks** (`leaks --atExit`). DEFERRED (ADR 0034
D8): a `Map`/`HashMap`, droppable-element `Vec` drop (`Vec<Struct>`/`Vec<[u8]>`),
`Vec`-in-generic-fns, `with_capacity`/`insert`/`remove`/slicing/iterators/`for`,
`secret Vec`, broker-backing. **1334 tests, four-check green. Phase D.3 MVP
closes.**

Pre-2/N context: **D.3 (1/N)** landed `Type::Vec(VecElem)` + the cascade +
`vec_new`/`push`/`len` end to end (the growable Vec foundation). After D.1 (sum types +
`match`) and D.2 (strings + `u8`), the third prerequisite — **growable
collections** (a lexer's token buffer + a parser's node lists need growth the
fixed `[T]` array can't express) — opens. **D.3 (1/N)** lands a growable,
owned, mutable **`Vec<T>`** end to end: a `Vec<T>` **IS `[T]` plus a capacity
field + mutation** (`Type::Vec(VecElem)` mirrors `Type::Array(ArrayElem)`,
reusing the array element-typing / move / drop machinery), lowering to abi-v1
`{ i64 len, i64 cap, ptr data }` (the data pointer is **field 2**). Builtins:
**`vec_new<T>() -> Vec<T>`** (empty `{0,0,null}`; element inferred from the
binding / return-type annotation, like `null`'s `?T`), **`push<T>(&mut Vec<T>,
T) -> i64`** (append + grow via the new **`sentinel_realloc`** runtime symbol to
`max(1, cap*2)*sizeof(T)` when `len==cap` — the first heap-mutation primitive,
taking `&mut`), and **`len`** overloaded over `[T]` and `Vec<T>`. `vec_new` /
`push` type through the **uniform generic-call path** (an explicit `(Vec,Vec)`
arm in `unify_one` binds the element); the cascade adds `Type::Vec` across every
exhaustive `Type` match; `Vec` is **Move** + needs-drop (frees the field-2
buffer, null-safe). A `&mut Vec` builtin arg registers a **mutable borrow**
(extends ADR 0033 A3); a non-`mut` `Vec` push is rejected. Builtins shift the
FnId base (vec_new=7, push=8; main 7→9). **Amendments (ADR 0034):** A1
`String`=`Vec<u8>` deferred to (2/N) with the bridge; A2 return-type pushdown
extended to `Vec`; A3 `len` overload is a contained `check_call` special-case;
A4 `sentinel_realloc` (not a Vec-specific grow fn); A5 `VecElementNotSupported`
rejects non-flat elements; arena routing needs no change (a `Vec` init is a
`Call`, not an `ArrayLit`). Phase-go `tests/pass/c5d3_collections.sentinel`
(multi-growth `Vec<i64>`, char-pushed `Vec<u8>`, a `Vec` moved out of a helper)
runs at **exit 67, 0 leaks** (`leaks --atExit`). DEFERRED to (2/N): `v[i]` /
`pop` / the `Vec<u8>`→`[u8]` bridge / the `String` alias; to (3/N): the phase-go
close + ADR flip. **1324 tests, four-check green. Phase D.3 (1/N) lands.**

Pre-D.3 context: **Phase D.2 — strings + a byte (`u8`) type MVP — COMPLETE
(1/N–4/N; ADR 0033 → ACCEPTED-WITH-AMENDMENTS).** After
D.1 (sum types + `match`), the second prerequisite — **strings + a `u8` byte
type** (a compiler's input is text) — now compiles + runs end to end,
leak-free. A string **IS a `[u8]`** (byte array — full reuse of the C1.6 array
machinery); `u8` is an integer-scalar primitive (→ LLVM `i8`, unsigned ops);
char literals `'a'` (→ `u8`), string literals `"…"` (→ `[u8]`, heap-copied +
owned), byte indexing, a `str_eq` byte-matcher, and `u8`↔`i64` conversions.
**(1/N)** lexer (escape-aware `StringLit`/`CharLit`); **(2/N)** AST + parser
(`ExprKind::CharLit`/`StringLit` + the parse-time escape decoder; resolve
rejected `NotYet`); **(3/N)** the type layer (`Type::U8` + the cascade +
`ArrayElem::U8`; char→`u8`, string→`[u8]`; `str_eq`/conversion builtins typed;
op-generic pipeline absorbs `u8` via one `is_int` change; codegen rejected the
literals + builtins until 4/N); **(4/N)** codegen + runtime (`i8` char const;
string heap-copy via direct byte-stores; `udiv`/unsigned compares; `zext`/
`trunc` conversions; `sentinel_str_eq` runtime; `abi-v1` `u8` entry;
`c5d2_strings` phase-go at exit 42, 0 leaks). Amendments: **A1** string heap
copy via byte-stores not a global (no `&Module` in `CodegenCtx`); **A2** inline
string-literal args to borrowing builtins inherit the pre-existing
temporary-drop gap (bound vars are leak-free); **A3** `str_eq` args are
borrowed not consumed. 1311 tests, four-check green. **Phase D.2 MVP closes.**

Pre-D.2 context: **Phase D.1 — sum types (`enum`) + pattern matching
(`match`) MVP — COMPLETE (4/N; ADR 0032 → ACCEPTED-WITH-AMENDMENTS).** The
first self-hosting prerequisite — **sum types + `match`**
(an AST is a sum type, the biggest self-hosting blocker) — now compiles +
runs end to end. (1/N) lexer; (2/N) AST + parser; (3/N) the type layer
(`Type::Enum(EnumId)` — the 11th interner-style variant — + `EnumData`/
`VariantData` + `TypedProgram.enums`; `ResolvedExprKind`/`TypedExprKind`
`EnumConstruct`+`Match`; resolve disambiguation of `Name::Variant(args)`
construction from impl-method/class-init + match-pattern binding scoping;
construction + `match` type-check with **exhaustiveness** — 5 new
`TypeError`s; enum names usable in type position; **directly-recursive
enums type-check** — the AST enabler). **(4/N) lands codegen:** construction
lowers to the abi-v1 `{ i32 tag, ptr payload }` (heap-boxed payload via
`sentinel_alloc`, `null` for unit variants); `match` → an LLVM `switch` on
the tag into per-arm blocks (GEP/load payload bindings into locals; arm
results reconcile at a merge block; `_` = default, else `unreachable` —
exhaustiveness is type-checked); scope-exit drop frees the payload box (if
non-null). Bare `Enum::Variant` unit construction parses (matches ADR D2 +
the pattern surface). abi-v1 §2 gains the enum layout entry + a stability
assertion. Phase-go `tests/pass/c5d1_enum.sentinel` (Shape constructed +
`match`ed, exit 42). **Amendments:** A1 — drop is **box-free only** (a
recursive enum / heap-typed payload leaks its nested boxes; leak-safe, NO
UAF). A (5/N) follow-up investigation proved the naive recursive fix —
synthesized per-enum drop *fns* alone — **double-frees** (a `match`-bound
payload is moved into a consumer while the scrutinee is also dropped → the
box freed twice; a `Tree` aborts at exit 133, empirically). The correct fix
is the **payload-ownership model** (`match` consumes the scrutinee, bindings
own the payload fields, the `match` frees the box, bindings are
drop-plan-registered + dropped at arm-scope exit, then drop fns are sound
for non-match paths) — a coordinated borrow-check + drop-plan + codegen
change, deferred (recorded in ADR 0032 A1 follow-up; code stays at box-free).
A2 — inline-small-enum optimisation deferred. A3 — generic enums
(`Option`/`Result`) → **D.1b**. +29 tests across 3/N+4/N (1268), four-check
green; c51 bar + `repro.rs` hold. **Phase D.1 (sum types + `match`) MVP
closes.**

**Next: Phase D.2 — strings + a byte (`u8`) type (ADR 0033 PROPOSED).** Per
the ADR 0031 D4 roadmap (sum types → **strings + byte type** → collections →
…), and because a self-hosted lexer's input is text. **ADR 0033 designs it:**
a string **IS a `[u8]`** (byte array — maximal reuse of the C1.6 array
machinery: `len`/index/drop/move/escape/arena all apply unchanged), plus a
`u8` integer-scalar primitive, char literals `'a'` (→ `u8`, with escapes),
string literals `"…"` (→ `[u8]`, heap-copied from a global constant so they
drop uniformly), and the lexer's core ops (`s[i]`, `len`, a `str_eq`
builtin, `u8`↔`i64` conversions). 4-sub-phase split (lexer → AST/parser →
`Type::U8` + the cascade → codegen/runtime). **D.2 (1/N) shipped:** the lexer
recognises char + string literals (escape-aware; `u8` lexes as an `Ident`,
no keyword token) — additive, +7 tests (1275), four-check green. **D.2 (2/N)
shipped (`f310f15`):** the AST + parser. `ExprKind` gains `CharLit(u8)` +
`StringLit(Vec<u8>)` carrying the **decoded** bytes (a string IS a `[u8]`),
with s-expr `Display` arms; the parser decodes the span at parse time (strip
quotes, process escapes `\n \t \r \0 \\ \' \"` + `\xHH` → bytes; non-escape
bytes incl. multi-byte UTF-8 pass through), validating a char is **exactly
one byte** (`''`/`'ab'`/multi-byte rejected) and rejecting a bad escape (2 new
`ParseError`s). `u8` needed **no type-parser change** (already a `TypeExpr`
`Ident`). Resolve **rejects** char/string literals (`CharStringLitNotYet`)
until 3/N — so `ResolvedExprKind` gains **no new variant** and the typed-tree
crates (types/codegen/mir/borrow/effect) are untouched; blast radius stays in
**ast + syntax + resolve**. Additive, +21 tests (1296), four-check green.
**D.2 (3/N) shipped (`56f69f0`):** the type layer. `Type::U8` (a primitive,
no interner) cascades across every exhaustive `Type` match (types + codegen
`i8`/`mangle` + borrow `is_copy` + the drop groups); `ArrayElem::U8` is the
one new array element (`[u8]` IS the string). char → `u8`, string → `[u8]`;
`ResolvedExprKind`/`TypedExprKind` gain `CharLit`/`StringLit`. `u8` reuses the
op-generic `Binary`/`Cmp`/bitwise + secret pipelines with one change
(`is_int += U8`); mixed-width `u8 + i64` stays a `Mismatch`; `secret u8`
inherits the secret rules. Three builtins typed — `str_eq([u8],[u8]) → bool`,
`u8_to_i64`/`i64_to_u8` (`FnId(4..=6)`; user fns shift +3). Codegen lowers
`u8` → `i8` (a real `u8` fn body compiles) but **rejects char/string literals
+ the three builtin calls** until (4/N) (the enum-(3/N) codegen-rejects
discipline; verified clean — exit 1, no panic). Additive, +13 tests (1309),
four-check green. **D.2 (4/N) shipped (`891ec98`) — Phase D.2 MVP closes,
ADR 0033 ACCEPTED-WITH-AMENDMENTS:** codegen + runtime. A char literal is an
`i8` const; a string literal heap-copies its bytes (`sentinel_alloc` + N `i8`
stores — A1: direct stores, not a global, since `CodegenCtx` has no `&Module`)
into an owned `[u8]` that drops via the array path; `u8` lowers to `i8` with
**unsigned** ops (`udiv` + unsigned `icmp`); `u8_to_i64`=`zext`,
`i64_to_u8`=`trunc`; `str_eq` calls the new runtime `sentinel_str_eq`
(`[u8]` args borrowed, not consumed — A3). `abi-v1` gains the `u8` entry +
`sentinel_str_eq` (19 symbols), pinned by tests. Verified via `leaks --atExit`:
`c5d2_strings` (parse "42"→42) + `c5d2_u8_unsigned` (unsigned div/compare) both
exit 42, **0 leaks**; the c51 repro bar holds. A2: inline string-literal args
to borrowing builtins inherit the pre-existing temporary-drop gap (bound vars
are leak-free). +2 pass fixtures (**1311**), four-check green.
**Next: Phase D.3 — growable collections (ADR 0034 PROPOSED).** Per ADR 0031
D4 item 3. The load-bearing lever: **a `Vec<T>` is `[T]` plus capacity +
mutation** — `Type::Vec(VecElem)` mirrors `Type::Array(ArrayElem)` (the same
flat element subset, `{i64 len, i64 cap, ptr data}`, element-generic builtins),
so it reuses the array index/bounds-check/move/drop machinery with **no new
generics + no lexer/parser change**; **`String` = `Vec<u8>`** (a growable byte
buffer — the 0033 "a string is its bytes" lever). New: capacity + growth
(`realloc`), `push(&mut v, x)` (the first heap-mutation primitive, over the
existing `&mut` + borrow check), `vec_new`/`len`/`v[i]`. 3-sub-phase split
(1/N type+codegen+runtime+drop for primitive-element Vecs → 2/N `v[i]`/`pop`/
the `Vec<u8>`→`[u8]` bridge → 3/N close). Out of scope: a `Map`, droppable-
element `Vec` drop, `Vec`-in-generics, iterators/`for`, broker-backing (the
bump arena can't `realloc`). After D.3: file I/O →
modules → loops (ADR 0031 D4), then the self-host port (lexer → parser → … in
Sentinel, differentially validated against the Rust `snc` oracle). (D.1b
generic enums + the enum payload-ownership/leak-completeness fix remain
available follow-ons.)

Pre-D.1(1/N) context: **🎉 SENTINEL 1.0 (2026-05-30) — Phase C5 + Phase C close.**
The 1.0 go/no-go (`tests/pass/c5_go_no_go.sentinel`, a constant-time
TLS-1.3-handshake-shaped program) runs and **passes the D5 constant-time
verification** — the close bar (ADR 0030 D8) is met — so 1.0 is declared:
**ADR 0025 (Phase C5 kickoff) and ADR 0030 (go/no-go) → ACCEPTED-WITH-AMENDMENTS.**
The 1.0 language: the full type system + witness-table generics, references
+ lexical borrow check + RAII drop, `secret` typing + effect rows + the
algebraic-effect handler runtime, classes/traits/impls/delegation +
structured concurrency, **machine-verified constant-time `secret`** (the
headline guarantee — the MIR D5 pass rejects a `secret` at a branch /
index|address / divisor), bitwise `& | ^`, broker-backed scope arenas, and
a frozen, layout-tested **`abi-v1`**. Single-process, single-file,
loop-free-by-design (recursion substitutes). 1232 tests, four-check green.
**Scoped out of 1.0** (post-1.0 follow-ons, all analysed): constant-time
*emission* (cmov/speculation barriers — branch-free code already passes
D5; ADR 0026 D4), bitwise shifts `<< >> ~` (ADR 0027 A1), actors (ADR 0030
D3), LSP/tooling (ADR 0025 D10), `[secret T]` arrays, modules/multi-file,
cross-process, a `u8`/byte type, loops, the full escape analysis. **Next:
Phase D — self-hosting (ADR 0031);** ⚠ self-hosting is a *major* multi-stage
effort — the 1.0 language has no strings / file I/O / growable collections
/ modules, all of which a compiler-in-Sentinel needs first, so Phase D
opens with a language/stdlib build-out, not lexer-in-Sentinel (see ADR 0031).

Pre-1.0 context: **C5 D7: the stable ABI — `abi-v1` defined, frozen, and
tested; ADR 0029 → ACCEPTED-WITH-AMENDMENTS. Phase C5 D7 closes.**
`docs/abi-v1.md` documents + **freezes** the ABI codegen already emits —
the C calling convention (`main`→i32; effecting fns→`*SentinelKont`; class
init→`out_ptr`), the `Type`→LLVM **layout catalog** (`[T]`=`{i64,ptr}`,
`?Struct`=`{i1,ptr}`, `?primitive`=`{i1,T}`, struct fields in decl order,
ref/kont/task=opaque ptr, `secret T`≡`T`), the `#[repr(C)]` runtime struct
layouts (`SentinelKont`/`Frame`/`Task`/`ScopeCtx`), the name-mangling
scheme, and the ~18 `sentinel_*` runtime-symbol contract — each
cross-linked to where it is realised. **Layout-stability tests pin it so a
drift turns a test red, not a silent miscompile** (1/N): runtime struct
size/align/`offset_of!` asserts + a symbol-set address test (`abi_v1_*` in
sentinel-runtime) + codegen mangling golden strings; (2/N) the
`Type`-layout DataLayout assertions (`abi_v1_type_layouts_via_datalayout`)
that lower each `Type` via `llvm_basic_type` and assert size / align /
field offsets **+ field types** (the latter pins field *order*, which
equal-sized offsets cannot) — verified to go red on a deliberate reorder,
then reverted. **No emitted bytes change** (documents + tests existing
behaviour) → the c51 bar + `repro.rs` hold by construction; reproducible
builds (D8) fold in. Amendments: A1 (field-type asserts strengthen the
"offsets" wording), A2 (named-struct layout pinned via a representative
`{i64,i64}` struct, since lowering a real `Struct(id)` needs codegen's
pass-0 cache). +4 tests (1231). Four-check green. **Next: the 1.0 go/no-go
(ADR 0030 PROPOSED).** A readiness/scoping pass found the TLS-handshake-
shaped close-bar program is an *assembly of proven patterns* (`c53_ct_eq`
constant-time verify, `c4` class/trait/delegation, `c37` effects,
recursion for iteration — all green) + modelling choices, **not** new
machinery; **actors are descoped** from 1.0 (ADR 0030 D3, a deviation from
C5.0 — a sequential handshake needs no mailbox), and bit shifts are a
conditional JIT prerequisite (ADR 0027 A1, only if a reduced primitive
needs them). **go/no-go (1/N + 2/N) are DONE — the close bar is MET.**
`tests/pass/c5_go_no_go.sentinel` is now the full handshake program with
real **constant-time** crypto over `secret` scalars — a Montgomery-ladder
step + branch-free `cswap`, an HKDF-expand-shaped mix via the `Kdf` trait,
and the `c53_ct_eq` `Finished` verify — and it **passes the D5
constant-time check** (`verify_constant_time` gates the build) + runs to
exit 42. The headline 1.0 capability (express + *prove* constant-time
crypto) is exercised end-to-end. Ergonomic finding: C3.1b makes a mixed
secret/public op a type error, so constant-time code lifts public
constants via a `sec` widening helper. **Resume at go/no-go (3/N): the
formal 1.0 declaration** — flip ADR 0025 → ACCEPTED + declare Sentinel
1.0. That call is **intentionally left to the developer** (a momentous,
milestone decision); the substantive close bar (runs + passes D5) is met.

Pre-D7(1/N) context: **C5.4 (2/N): the scope→arena codegen; ADR 0028 →
ACCEPTED-WITH-AMENDMENTS.** Codegen routes a scope's non-escaping
primitive array-literal heap buffers into a broker bump arena and replaces
that scope's per-binding `sentinel_free`s with one `sentinel_arena_exit`.
The routed set = *exactly* `emit_scope_drops`'s free set (`∉ moved ∧
≠ tail_returned`, narrowed to `let x = [primitive array literal]` in
non-generic non-effecting fns); one `HashSet<VarId>` drives both routing +
free-skip; the per-scope arena handle lives in a new `ScopeFrame`, lazily
created. UAF-safe by reasoning (routed ⊆ proven-non-escaping free set),
verified by disassembly + the c24/c25 guards. (Amends ADR 0028's "UAF
hole": a tail-returned array IS in `moved_sources`, so `∉ moved` alone
already excludes returned arrays; both checks kept to mirror
`emit_scope_drops`.) +1 fixture (`c54_scope_arena`, exit 42); 1227 tests.

Pre-C5.4(2/N) context: **C5.4 (1/N): the broker-arena substrate (ADR
0028).** The Phase A broker backs a scope-arena C-ABI in the runtime.
**Finding that shaped it:** the broker is a safe *handle* allocator
(bump bulk-frees / slab fixed-size) with no public raw pointer, so a
drop-in `sentinel_alloc` doesn't fit; instead a public raw-bytes API was
added (`Arena::alloc_bytes` → `NonNull<u8>`, exposing the strategy's
`alloc_raw`), and the runtime gained `sentinel_arena_enter` (create a
bump arena) / `sentinel_arena_alloc` (16-byte-aligned bump alloc) /
`sentinel_arena_exit` (destroy → `BumpStrategy::drop` frees the buffer),
on a process-wide lazy `Broker`. Additive + c51-safe: codegen was
untouched (+5 tests, 1226). C5.4 (2/N) above wired codegen to call them.

Pre-C5.4(1/N) context: **C5.3 (2/N): bitwise `& | ^` end-to-end; ADR 0027 →
ACCEPTED-WITH-AMENDMENTS.** The bitwise operators now compile and run.
The surface was *small* because the `Binary` pipeline is op-generic:
resolve passes `BinOp` through, the type checker's Binary handler is
op-agnostic (only `Div` is special-cased for `SecretDivisor`), and
`lower_to_mir` / the D5 pass treat `Binary` generically — so only AST +
parser + codegen changed. **AST:** `BinOp` gains `BitAnd`/`BitOr`/`BitXor`
(no new `ExprKind`/`TypedExprKind`/`MirOp` variants). **Parser:** three
left-assoc levels `parse_bitor`→`parse_bitxor`→`parse_bitand` between
`parse_cmp` and `parse_add` (Rust order `&`>`^`>`|`); infix `&` is
bit-and, prefix `&` is borrow (positional, unambiguous). **Types:** no
change — bitwise inherits the C3.1b secret-preserving integer rule
(`secret op secret → secret`; mixed → `Mismatch`; `bool` rejected), and
**no new `SecretXxx` rejection** since bitwise is the *sanctioned*
constant-time secret computation. **Codegen:** LLVM `and`/`or`/`xor`.
**MIR + D5:** unchanged (bitwise is a non-sink). Fixtures: `c53_bitwise`
(`5 & 6 ^ 3 | 8` == 15, pins precedence); **`c53_ct_eq`** — a real
constant-time equality over secrets (XOR-accumulate + OR-reduce +
`declassify`) that compiles, runs, and **passes** D5 (the go/no-go's
`Finished` MAC-verify shape, replacing the C5.2b arithmetic stand-in).
**ADR 0027 → ACCEPTED-WITH-AMENDMENTS** (A1: the `<< >> ~` wave / C5.4 is
a deferred follow-on — the constant-time *compare* needs only `^`/`|`).
+9 tests (1221). Four-check green. **Next: C5.4 — broker integration
(D4), per ADR 0028 (PROPOSED).** (Chosen by the developer over shifts /
go/no-go assembly.) Reading the broker + runtime showed the broker is an
*arena* allocator — bump bulk-free / slab fixed-size / typed handles —
that does **not** fit a drop-in `sentinel_alloc` (arbitrary-size,
individual-free, raw `*u8`); ADR 0028 instead maps Sentinel scopes →
arenas via the borrow-check `DropPlan` (no new escape analysis), shipping
a **runtime-only broker foundation first** (C5.4 (1/N) — c51-safe, since
codegen is untouched and objects stay byte-identical) then the
scope→arena codegen (C5.4 (2/N), may defer post-1.0). Numbering: the
bitwise *shift* wave `<< >> ~` (ADR 0027 A1) is an *unnumbered* deferred
follow-on (not C5.4); ADR 0025 D14's "ADR 0027 = broker" is superseded
(0027 = bitwise; broker = 0028).

Pre-C5.3(2/N) context: **C5.3 (1/N): lexer — bitwise `|` (`Pipe`) + `^`
(`Caret`) tokens (ADR 0027 D2).** The first wave of the bitwise-operator
surface (the prerequisite for the go/no-go's constant-time MAC verify):
two new
logos tokens (`|` → `Pipe`, `^` → `Caret`); longest-match keeps `||` →
`PipePipe`, and the infix bitwise-and **reuses** the existing `&` (`Amp`)
token, disambiguated from the borrow prefix by the parser's operand
position at 2/N. No `<<`/`>>` tokens yet (C5.4 — they need the
`>>`-vs-nested-generic-close split). +4 lexer tests (additive — the
parser doesn't consume the tokens yet); four-check green (1212). **Next:
C5.3 (2/N)** — the `& | ^` surface end-to-end: extend `BinOp`; parser
precedence `&`>`^`>`|` between cmp and add; secret-preserving integer
typing (mirroring C3.1b arithmetic); LLVM and/or/xor codegen (MIR + the
D5 pass already cover bitwise as non-sinks) + `c53_bitwise` / `c53_ct_eq`
fixtures; ADR 0027 flip.

Pre-C5.3(1/N) context: **C5.2b (2/N): the D5 constant-time verification is
wired into `snc` (ADR 0026 D5/D9).** `snc build` now runs the
constant-time check: after `check_query`, the driver lowers the typed
program to MIR
(`lower_to_mir` — now a real pipeline consumer) and runs
`verify_constant_time`; a `secret` reaching a conditional branch, a load
index/address, or a division divisor is a `sentinel::mir::secret_leak`
compile error (exit 1) that gates codegen. (Codegen still consumes the
typed program via the HIR seam per the D3 escape hatch; MIR stays
analysis-only.) Two phase-go fixtures (D9): **`c52_secret_leak`** (UI) —
`secret bool && secret bool` type-checks (`SecretBranch` only rejects
`if`) but lowers to a secret short-circuit `Branch`, rejected with an
`insta` snapshot of the rendered diagnostic; **`c52_secret_ct`** (pass) —
a branch-free masked select over secrets (`c*a + (1-c)*b`) compiles,
runs, **passes** D5, exits 42. The **c51 behaviour-preservation bar
holds**: every existing pass/ui fixture is unchanged and the
`tests/repro.rs` objects stay byte-identical (D5 runs before, and gates,
an unchanged codegen). +2 tests (1208). Four-check green. **Next: C5.3** —
bitwise operators (`& | ^`, then `<< >> ~`) per **ADR 0027 (PROPOSED)**,
the prerequisite surface for the go/no-go's constant-time `Finished` MAC
verify (an XOR-accumulate compare, which needs `^`/`|`). The C5.2a/D4
emission open question is resolved by **deferring D4**: a branch-free
*arithmetic* primitive already passes D5 on the existing codegen, and the
constant-time idioms are bitwise, so the bitwise surface comes first and
D4's branch-free-select/barriers are pushed behind it (likely post-1.0).
ADR 0026 stays PROPOSED.

Pre-C5.2b(2/N) context: **C5.2b (1/N): the D5 constant-time verification
pass (ADR 0026 D5).** `verify_constant_time(&MirProgram) -> Vec<SecretLeak>` is the
first consumer of `lower_to_mir` and the machine-checkable form of ADR
0008's guarantee: it rejects any `secret` value that reaches a
conditional-branch condition, a load index/address, or a division
divisor, emitting a `sentinel::mir::secret_leak` what/why/how diagnostic
(a `SinkKind` names which sink). **Taint oracle:** each SSA value carries
its `Type`, and the type checker's operator-secret-preserving rules
already computed the taint fixpoint (`declassify` clears it; fn-signature
boundaries respected), so the pass reads taint straight off the type
(`is_secret`) and inspects each sink — no separate def-use propagation is
needed at the typed-program level (the ADR's forward propagation is only
needed once MIR is lowered from *post-optimisation* code — post-1.0; a D5
amendment). The one leak this catches over the C3.1 source rejections is
`secret bool && secret bool` (a secret short-circuit `Branch`), which
type-checks because `SecretBranch` only rejects `if`. The MIR data model
gains a `span` on `MirInst` + `MirTerminator::Branch` (the sink-bearing
nodes), threaded in `lower_to_mir`, so the diagnostic points at source.
**Additive** — not yet wired into the driver (C5.2b (2/N) does that + the
`c52_*` fixtures); zero regression risk. +4 verify tests (1206).
Four-check green. **Next: C5.2b (2/N)** — wire D5 into the driver (a real
`secret_leak` compile error) + `c52_secret_leak` (UI) + `c52_secret_ct`
(a branch-free masked select, pass) fixtures.

Pre-C5.2b(1/N) context: **C5.1b (2/N): `lower_to_mir` — typed fn bodies → MIR SSA
(ADR 0026 D2).** `sentinel-mir` now lowers each free function's
type-checked body into a `MirFunction` in SSA/CFG form. Sentinel has only
*structured* control flow and no loops, so the CFG is a DAG and SSA falls
out of one structured walk (no dominance-frontier phi placement): `if` /
`&&` / `||` become `MirTerminator::Branch` edges into fresh blocks that
reconcile at a merge block via SSA block-params (the phi-equivalent), and
a variable reassigned on one arm is threaded through a merge param
(deterministic, `VarId`-sorted via a `BTreeMap` env). `&&`/`||` are
control flow because `secret bool && secret bool` type-checks
(`SecretBranch` only rejects `if`) — a short-circuit branch on a secret is
a leak the C5.2 D5 pass must see. The three D5 sinks lower precisely (an
`if`/short-circuit condition → `Branch`; `a[i]` / `*p` → `MirOp::Load` so
a secret index *or* address is visible; `a / b` → `Binary(Div)`) and
`declassify(e)` → `MirOp::Declassify` (the one taint sink); everything
else → `MirOp::Opaque` carrying its operands so taint can't vanish.
Scope: top-level fns only (class/impl/init method bodies are a mechanical
follow-on); generic defs lowered as-is (`TypeParam` is never secret); no
monomorphisation (MIR is analysis-only per the D3 escape hatch).
**Additive** — nothing consumes MIR yet (the D5 verification at C5.2 is
its first consumer; the driver will call `lower_to_mir(hir.program())`
then); zero regression risk; codegen stays on the typed program. +7
lowering tests (1202). Four-check green via `cargo nextest run
--workspace` (1202) + `cargo test --doc`. **Next: C5.2** — the D5
constant-time verification pass over this MIR
(`sentinel::mir::secret_leak`) + D4 constant-time emission (a codegen
pass) — the 1.0 headline.

Pre-C5.1b(2/N) context: **C5.1b (1/N): the MIR data model lands (ADR 0026
D2).** `sentinel-mir` is no longer a stub — it defines a minimal SSA/CFG
IR (`MirProgram` / `MirFunction` / `MirBlock` with SSA block parameters
as the phi-equivalent / `MirInst` / `MirOp` / `MirTerminator` /
`MirValue`) built to host the C5.2 D5 constant-time verification. Each
SSA value carries its `Type`, so secrecy reads straight off
`Type::Secret(_)` (`MirFunction::is_secret` — the taint seed); the three
D5 sinks are representable (a `Branch` condition, a `Load` index, a
`Binary` Div/Rem operand), and every non-secret-relevant construct
funnels through `MirOp::Opaque` / `MirTerminator::Unreachable` carrying
its operands so taint stays sound. **Data model only that increment** —
additive, nothing consumed MIR yet. Test-count-neutral (the
`sentinel-mir` stub smoke test replaced by a hand-built secret-branch SSA
test). Four-check green (1195).

Pre-C5.1b context: **C5.1a (1/N): the HIR pipeline stage is introduced
(ADR 0026 D1+D3).** Per ADR 0009 §6.1 the pipeline is `types → hir →
mir → codegen`; C0–C4 short-circuited `types → codegen`. This increment
stands up `sentinel-hir` as a real stage between the type checker and
codegen: a pure `lower_to_hir(&TypedProgram, &DropPlan) -> HirProgram`
the driver calls after borrow-check, with `compile_to_object` now
consuming `&HirProgram`. At this increment the HIR is a thin borrowing
**bundle** of the typed program + drop plan (no desugaring yet), so the
migration is **behaviour-preserving by construction**: all 1195 tests
pass and every `tests/repro.rs` object stays byte-identical (the
ADR 0026 D9 `c51` bar). `lower_to_hir` stays a pure fn (not
salsa-tracked), matching codegen's non-salsa status (ADR 0011 C1.0c).
Test-count-neutral (the `sentinel-hir` stub's smoke test replaced by a
`lower_to_hir` round-trip). Four-check green via `cargo nextest run
--workspace` (1195) + `cargo test --doc`.

**C5.1 escape hatch INVOKED (ADR 0026 D1/D3 amendment, decided with the
developer).** Codegen couples to the typed tree at ~295 `TypedExprKind`
/ 342 `TypedExpr` refs across 90 signatures (28 expr variants) —
migrating it to a *thick* HIR is a multi-session, high-risk rewrite of
nearly the whole backend, and it is **not required** for the 1.0
constant-time-`secret` capability. So codegen STAYS on the typed program
(reached via this seam, `HirProgram::program()`); the thick HIR desugar
(dispatch resolution, monomorphisation, explicit drops) + the
codegen-consumes-HIR migration are **post-1.0** (still Phase-D-valuable).
**C5.1a closes at the seam.** **Next: C5.1b** — `sentinel-mir` +
`mir_query`, an SSA/CFG lowered from the typed program (via the seam)
that hosts the C5.2 D5 constant-time verification; then C5.2 =
constant-time emission (D4, a codegen pass) + the D5 verification.

Pre-C5.1a context: **C5.0 (complete) + ADR 0026 PROPOSED — Phase C5
kickoff; the 1.0 go/no-go program is CHOSEN.** Per ADR 0025 D1/D13 the Phase C /
Sentinel-1.0 close bar is a **single-process, single-file TLS 1.3
handshake** (server-flavoured: ECDHE → HKDF key schedule →
constant-time `Finished` MAC verify). This pins the two scoping-
open questions: **D6 (cross-process) → post-1.0** and **D9 (module
system) → post-1.0** — the handshake is self-contained and fits one
file, so `@shared`/cross-process and `mod`/`use`/separate-compilation
both leave the 1.0 path (the largest scope reduction in C5).
Rationale: TLS is secret-dense, so it forces the headline 1.0
capability — constant-time `secret` codegen (D3, the C5.2 security
core) — which an HTTP server would leave unexercised; it still
exercises classes/traits/delegation, effects+handlers, structured
concurrency + a connection actor (D5), generics, and the broker
(D4). See ADR 0025 `## C5.0 resolution`. **D11 test infra DONE:**
`cargo nextest` adopted (`.config/nextest.toml`, default + ci
profiles) + the 15 driver UI rejections (c34/c37/c41/c42/c43/c44)
migrated from ad-hoc `stderr.contains(code)` to `insta` blessed
full-diagnostic snapshots (`crates/sentinel-driver/tests/ui.rs` — snc
is run from the workspace root with a relative path, so the snapshots
are portable with zero normalization; a snapshot pins the entire
what/why/how, not just the presence of a code). The 2 pure-syntax UI
snapshots (lex/parse) remain in `sentinel-syntax/tests/ui.rs`.
**D8 reproducible-build audit DONE:** empirically verified the entire
C0–C4 codegen surface is byte-identical across independent `snc`
processes (full C4 program, generics monomorphization, handler/capture
fixtures, nested handles, RAII drop) — codegen's `std::HashMap`s are
all lookup tables (emission walks source-ordered `Vec`s), mach-O
objects carry no timestamp, and LLVM lowers identical IR identically,
so the ADR's proposed HashMap→BTreeMap conversion is unnecessary at the
current surface (the std maps stay; the test is the guard). Locked in
with `crates/sentinel-driver/tests/repro.rs` (compile-twice, diff the
object; 5 fixtures) — a regression guard for the C5.1 HIR/MIR stages.
**C5.0 COMPLETE** (D1/D13 + D6/D9 decision, D11 test infra, D8 repro
audit). Next: C5.1 = HIR/MIR stages (D2), C5.2 = constant-time secret
codegen (D3). **ADR 0026 PROPOSED is drafted** (C5.1/C5.2 HIR/MIR
pipeline + constant-time secret codegen — 10 D-decisions, 4-sub-phase
split C5.1a→C5.2b: codegen re-targets `TypedProgram`→a desugared HIR,
MIR is the SSA substrate for the constant-time verification, escape
hatch documented). Next: **C5.1a** — `sentinel-hir` + `hir_query`
desugar + migrate codegen to HIR. Four-check suite green
via `cargo nextest run --workspace` + `cargo test --doc` (1195 tests:
the C4-close ~1190 + 5 repro; D11 was test-count-neutral).

Pre-C5.0 context: **C4.5: Phase C4 close-out — ADR 0021 flips
PROPOSED → ACCEPTED-WITH-AMENDMENTS; PHASE C4 CLOSES.** All six
sub-phases shipped (C4.0 lexer, C4.1 classes, C4.2 traits+impls,
C4.3 delegation, C4.4 structured concurrency, C4.5 close-out).
C4.5 added the combined full-surface phase-go
`tests/pass/c4_go_no_go.sentinel` (class + `&mut Self`/`&Self`
methods + init + trait + impl + delegation + scope/spawn/await
in one program; exit 42) + `tests/pass/c4_named_impl.sentinel`
(two named impls of one (trait,type) co-existing via qualified
calls; exit 42). ADR 0021 D13's `spawn lb.write(42)` was adapted
to spawn a free fn (spawn is fn-call-only per ADR 0024 D2 —
amendment A2); ADR 0021 A1 records async-as-effect (D9)
superseded by ADR 0024's direct-runtime API (surface identical).
**Next: Phase C5** (broker integration + cross-process + actors
+ stable ABI + tooling per HANDOVER §6.2; Sentinel 1.0 at C5
close). Four-check suite green (~1191 active workspace).

The substantive structured-concurrency landing was **C4.4 (2/N)**
per ADR 0024 D4+D5+D8 — `scope concurrent { spawn fn(args);
expr.await }` compiles + runs end-to-end on a thread-per-spawn
runtime. Pieces:
- **Types**: `Type::Task(TaskId)` (tenth interner variant) +
  `TaskData { result_ty }` + `intern_task` + `TypedProgram.tasks`;
  `TypedExprKind::Scope/Spawn/Await`; spawn validates a Call
  target returning `i64` (Task<i64>-only per D7); await requires
  a `Type::Task` receiver. Three `TypeError`s: SpawnMustBeCall /
  SpawnResultMustBeI64 / AwaitOnNonTask.
- **Resolve**: scope/spawn/await pass-through (NotYet dropped);
  the built-in **`Async`** effect is auto-registered (appended
  after user effects, so user EffectIds stay stable — a
  deviation from ADR 0024 D5's "EffectId(0)" wording).
- **Effect-check**: spawn/await contribute `Async` to a fn's
  inferred row; `scope concurrent` **discharges** `Async` (like
  a handler discharges its handled effects) so a scoped program
  keeps `main` effect-free, while a spawn/await outside a scope
  bubbles `Async` to `main` and is rejected (D5 discipline,
  shipped not deferred).
- **Codegen**: 5 runtime externs + a per-spawn-target wrapper
  synthesized in a `compile_to_object` pre-walk (before
  CodegenCtx exists — it lacks `&Module`); lower scope
  (enter/exit + `current_scope` save/restore), spawn (pack i64
  args + task_spawn + scope_register), await (task_await). An
  Async-only fn keeps the value-returning ABI rather than the
  C3 handler `Kont*` ABI (`uses_kont_abi` excludes Async).
- **Runtime fix**: the `_pad` field becomes an `owned` flag so
  an explicit `.await` inside a scope is safe against the
  scope's exit-time auto-await (scope owns + frees the Task;
  await only joins+reads when owned). Closes a documented UAF
  in the C4.4 (2/N runtime) symbols.

Deferred (ADR 0024 amendments): work-stealing scheduler (A1,
thread-per-spawn shipped); cancellation on early scope exit
(A2); `Task<T>` for T≠i64 + spawn args beyond i64 (A3); explicit
`Task<T>` type-position annotations (use inference — A5).

Phase-go `tests/pass/c44_go_no_go.sentinel` (`scope concurrent
{ let t = spawn double(21); t.await }`) exits 42; +3 UI fixtures
pin the typing rejections. Full four-check suite green.

**Runtime additions** (sentinel-runtime/src/lib.rs):
- `SentinelTask { result: i64, done: u32, owned: u32,
  join_handle_ptr: *JoinHandleBox, args_free_ptr: *u8 }` —
  32 bytes / 8-byte aligned (the C4.4 (2/N) close repurposed
  the former `_pad` slot as `owned`: 1 = a scope owns + frees
  the Task, so `.await` only joins+reads). Stable-layout test
  pins the size for ABI compat with codegen.
- `SentinelScopeCtx { registry_ptr: *ScopeRegistry }` +
  `ScopeRegistry { tasks: Vec<*mut SentinelTask> }`.
- Five `#[no_mangle] pub extern "C"` fns per ADR 0024 D7:
  - `sentinel_task_spawn(wrapper, args_storage, args_size)
    -> *mut SentinelTask` allocates task + spawns OS thread.
  - `sentinel_task_await(*mut SentinelTask) -> i64` joins
    the thread + frees the task.
  - `sentinel_scope_enter() -> *mut SentinelScopeCtx`
    allocates a per-scope task registry.
  - `sentinel_scope_register(*mut SentinelScopeCtx, *mut
    SentinelTask)` registers a task in the scope.
  - `sentinel_scope_exit(*mut SentinelScopeCtx)` walks the
    registry + auto-awaits + frees the scope.
- `JoinHandleBox { handle: Option<std::thread::JoinHandle<()>> }`
  internal struct holding the OS thread handle behind a
  pointer the FFI side stashes opaquely.

**No types / codegen / phase-go yet**: the runtime substrate
ships standalone. Resolve still rejects scope/spawn/await
with the C4.4 (1/N) NotYet diagnostics; the typing + codegen
wiring + c44_go_no_go phase-go land in the next iteration.

**Why ship runtime alone**: the prior attempt at the full
C4.4 (2/N) (types + codegen + phase-go in one push) ran into
codegen wrapper-synthesis complexity — synthesizing per-fn
wrapper LLVM IR from inside CodegenCtx is awkward because
CodegenCtx doesn't hold a `&Module` reference. The cleanest
fix is to pre-walk the typed program in compile_to_object
BEFORE CodegenCtx is created. The runtime substrate is
isolated + tested + correct; landing it solo lets the
follow-on focus entirely on the types + codegen wiring with
a clean working tree. See HANDOVER §0.2 for detailed prep.

Workspace test delta from C4.4 (1/N) close: +6 (1182 total)
— +6 sentinel-runtime tests (spawn-then-await round trip /
spawn with i64 arg / scope enter+exit round trip / scope
register + auto-await / Task layout stability + smoke).
**Phase C4.4 (2/N runtime) closes here.** Next: C4.4 (2/N
types + codegen) — types layer (Type::Task interner +
spawn/await/scope typing + 3 new TypeError variants),
codegen (5 runtime-fn externs + pre-walk wrapper synthesis +
lower Spawn/Await/Scope), c44_go_no_go phase-go, ADR 0024 →
ACCEPTED-WITH-AMENDMENTS flip.

Pre-C4.4(2/N runtime) context: **C4.4 (1/N): scope / spawn /
await AST + parser landed per ADR 0024 D1+D2+D3.** ADR 0024
stays PROPOSED. First of the C4.4 sub-iterations: structured-
concurrency surface parses end-to-end at AST + parser.
Downstream resolve rejects with three new "not yet"
diagnostics — ScopeNotYet / SpawnNotYet / AwaitNotYet — until
C4.4 (2/N) brings up the typing layer + runtime (5 new
symbols: sentinel_task_spawn, _await, sentinel_scope_enter,
_exit, _register) + codegen per ADR 0024 D4-D9.

**AST additions**:
- `ExprKind::Scope { mode: ScopeMode, body: Box<Block> }` +
  `ScopeMode::Concurrent` (only mode at C4.4 minimum per ADR
  0024 D1; sequential / race / other modes reserved).
- `ExprKind::Spawn { call_expr: Box<Expr> }` — prefix unary.
  Restricted to fn-call shape at the typing layer per ADR
  0024 D2; parser accepts any expression.
- `ExprKind::Await { task_expr: Box<Expr> }` — postfix
  `task.await` form.

**Parser additions**:
- `parse_scope_expr` invoked from `parse_atom` on
  `TokenKind::Scope`. Requires positional `concurrent` Ident
  (the C4.0 lexer keeps it as a plain Ident per the smallest-
  surface principle) + a block body.
- `parse_spawn_expr` invoked from `parse_atom` on
  `TokenKind::Spawn`. Uses `parse_postfix` to consume the
  call-shaped inner expression.
- Postfix `.await` dispatch in `parse_postfix`'s Dot arm.
  `TokenKind::Await` is reserved at C4.0, distinct from a
  regular Ident.
- Precedence: prefix `spawn` binds looser than postfix
  `.await`, so `spawn fn(x).await` parses as `spawn
  (fn(x).await)`. Explicit parens or a let-binding give the
  `(spawn fn(x)).await` shape. Mirrors Rust's await-precedence
  rule.

**Resolve pass-through**: `ResolvedExprKind` gains
`Scope` / `Spawn` / `Await` variants. Three new
`ResolveError` variants: `ScopeNotYet`, `SpawnNotYet`,
`AwaitNotYet` — all wired to clear help text pointing at
C4.4 (2/N). Mirrors the C4.2 (1/N) pattern.

**Types pass-through**: the new `ResolvedExprKind` variants
are unreachable in practice at C4.4 (1/N) (resolve surfaces
NotYet first). The arm panics if hit; C4.4 (2/N) replaces
with real typing logic.

Workspace test delta from C4.3 close: +9 (1177 total) — +6
parser tests (scope block / scope-missing-concurrent / spawn
fn-call / await postfix / await-on-parenthesized-spawn /
scope-spawn-await-combined) + +3 resolve rejection tests.
**Phase C4.4 (1/N) closes here.** Next: C4.4 (2/N) — types
(Type::Task interner + Async built-in effect + spawn/await
typing) + sentinel-runtime (5 new symbols + Task + ScopeCtx
structs) + codegen (per-spawn wrapper synthesis + scope-enter/
exit lowering) + c44_go_no_go phase-go.

Pre-C4.4(1/N) context: **ADR 0024 PROPOSED: C4.4 structured
concurrency surface + Async effect + runtime scheduler.**
Twelve D-decisions covering the surface (D1-D3:
scope/spawn/await grammar), typing (D4: Type::Task interner;
D5: Async built-in effect), runtime (D6: thread-per-spawn at
minimum; D7: five new runtime symbols), lowering (D8 + D9:
scope-bounded auto-await on exit), out-of-scope deferrals
(D10), lexer state (D11: no new tokens), phase-go (D12), and
the sub-iteration split (D13).

Key amendment to ADR 0021 D9 documented in ADR 0024 D6: at
C4.4 minimum we ship a **direct runtime API** rather than
async-as-effect via the C3 handler runtime — multi-shot
continuations (ADR 0020 D2 deferred) would be required for
the latter. The user-facing surface is identical; only the
lowering strategy differs. Other key choices: thread-per-spawn
(no work-stealing scheduler at minimum); fn-call-only spawn;
Task<i64>-only at minimum. Estimated footprint ~1500 LOC
across all layers.

Pre-ADR-0024 context: **C4.3: delegation auto-forwarders per
ADR 0021 D6.** `delegate field: T to Trait;` inside a class
body synthesizes a default impl of `Trait` for the class that
auto-forwards every trait method to
`self.field.method(args)`. The synthesis happens entirely at
the resolve layer — types + codegen flow through the existing
per-impl machinery (no changes). c43_go_no_go phase-go runs
at exit 42. No detail ADR — C4.3 lands under ADR 0021 D6 +
the existing `delegate` lexer reservation from C4.0 D11.

**AST additions**:
- `DelegateDecl { visibility, field_name, field_name_span, ty,
  trait_name, trait_name_span, span }` — the surface form.
- `ClassDecl.delegates: Vec<DelegateDecl>` alongside fields /
  init / methods.

**Parser additions**: `parse_delegate_decl` invoked from
`parse_class_decl`'s loop on `TokenKind::Delegate`. Grammar:
`('pub')? 'delegate' Ident ':' type 'to' Ident ';'`. `to` is a
positional Ident (the lexer keeps it as a plain Ident per C4.0
D11). Three new rejections fire via UnexpectedToken with clear
"expected" text: missing `to`, missing trait name, missing `;`.

**Resolve additions**: Pass 0e extension registers
delegate-synthesized impls alongside user impls in
`impl_default_table` — coherence collision between a delegate
and a user `impl as Trait for Class` fires the existing
DuplicateDefaultImpl rejection. Pass 3 (`resolve_class_decl`)
extends the field-uniqueness loop to also walk delegates,
adding a synthesized `ResolvedClassField` per delegate (with
DuplicateClassField on collision against an explicit `let`
field). Pass 4.5 (NEW) synthesizes the per-delegate
`ResolvedImplDecl` — one ResolvedImplMethodDef per trait
method, body = `self.field.method(args)` constructed as a
`ResolvedExprKind::MethodCall` on a `FieldAccess` on `Var(self)`.
Each synthesized method allocates a new self VarId + per-param
VarIds via the shared `next_var_id` counter.

**No types / codegen changes**: the synthesized
ResolvedImplDecl flows through Pass 0d/3c/3d/Pass 6 (types)
and Pass 1/compile_impl (codegen) without modification. The
synthesized impl is indistinguishable from a user-written
`impl as Trait for Class` from those layers' perspective.

**Two new C4.3 fixtures + two UI fixtures**:
- `c43_go_no_go.sentinel` (ADR 0021 D6 phase-go): Logger class
  with `delegate writer: FileSink to Writer`; FileSink has the
  default Writer impl. `l.write(42)` routes via the synthesized
  forwarder to `self.writer.write(42)` → FileSink's Writer
  impl → count = 42. Exit 42.
- `c43_delegate_collides_with_impl.sentinel` (UI): Logger has
  both `delegate writer: FileSink to Writer` AND
  `impl as Writer for Logger` →
  `sentinel::resolve::duplicate_default_impl`.
- `c43_delegate_undefined_trait.sentinel` (UI): `delegate
  field: T to Missing;` where Missing isn't declared →
  `sentinel::resolve::undefined_trait_for_impl`.

Workspace test delta from C4.2 close: +14 (1168 total) — +7
parser tests (one-delegate / pub-delegate / two-delegates /
missing-to / missing-trait / missing-semi / full-surface-with-
delegate) + +4 resolve tests (synthesizes-impl-and-field /
collides-with-user-impl / undefined-trait / field-collides-with-
let) + +3 driver tests (c43_go_no_go + 2 UI rejections).
**Phase C4.3 closes here.** Next: **Phase C4.4** — structured
concurrency (scope/spawn/await + Async effect + work-stealing
scheduler) per ADR 0021 D8 + D14. The scheduler warrants its
own detail ADR (ADR 0024) at sub-phase open per the per-sub-
phase ADR norm.

Pre-C4.3 context: **C4.2 (2/N): resolve / types / codegen
wired up for traits + impls; `s.write(10)` +
`Doubling::write(&mut s, 16)` ship end-to-end.** ADR 0023 →
ACCEPTED-WITH-AMENDMENTS. Second of the C4.2 sub-iterations:
trait + impl declarations flow through the full pipeline.
Receiver-typed dispatch (Path 1) and qualified-named dispatch
(Path 2) run end-to-end. Path 3 bounded-generic dispatch is
the amendment — DEFERRED pending `<W: Writer>` bounded-generic
surface.

**Resolve additions**: `TraitId(u32)` + `ImplId(u32)` interners.
`ResolvedTraitDecl` / `ResolvedTraitMethodSig` /
`ResolvedTraitParam` (sig-only params, no VarIds) /
`ResolvedImplDecl` / `ResolvedImplMethodDef` parallel-tree
types. `ImplTarget { Class(ClassId), Struct(StructId) }` for
the impl's `for Type` clause. `ResolvedProgram.traits:
Vec<ResolvedTraitDecl>` + `.impls: Vec<ResolvedImplDecl>`. Pass
0d collects + resolves trait method sigs (DuplicateTraitMethod
on collision). Pass 0e collects impl decls + enforces scope-
local coherence — default impls keyed by `(TraitId, ImplTarget)`
in `impl_default_table` with DuplicateDefaultImpl rejection;
named impls keyed by name in `impl_named_table` with
DuplicateImplName rejection. Pass 4 (new) resolves impl method
bodies — binds synthetic `self` VarId + params. The three
"not yet" rejections from C4.2 (1/N) are dropped; the
QualifiedCall arm now routes through `impl_named_table` →
`impl_to_trait_methods` to assign `(ImplId, method_index)` at
resolve time. Eight new ResolveError variants: RedefinedTrait,
DuplicateTraitMethod, UndefinedTraitForImpl,
UndefinedTypeForImpl, DuplicateDefaultImpl, DuplicateImplName,
UndefinedImpl, UndefinedTraitMethod.

**Types additions**: `Type::TraitSelf(TraitId)` interner extension
preserving the `Copy + Hash` invariant (ninth interner-table-
style variant; 0014 / 0015 / 0016 / 0017 / 0019 / 0020 / 0022 /
0023). `TraitData` + `TypedTraitMethodSig` / `ImplData` +
`TypedImplMethodDef` on `TypedProgram.trait_decls` /
`.impl_decls`. Pass 3c types trait method signatures — empty
type-param scope (no generic traits at C4.2 minimum per ADR
0023 D10). Pass 3d types impl signatures + verifies
completeness (ImplMissingMethod) + per-method signature match
(self_kind + params + return_type + effect_row —
ImplMethodSignatureMismatch). Pass 6 types impl method bodies
— binds `self` to the impl target's concrete type with mutable
bit from self_kind. The D6 method-call algorithm extends
`check_method_call_expr`: after class-method lookup fails,
search default impls for the receiver type that provide method
X. Exactly one match → ImplMethodCall; multiple →
AmbiguousMethodCall; zero → MethodNotFound. The new
`check_qualified_call_expr` validates the receiver (args[0]) is
the impl target's type or a ref-thereof
(ImplMethodReceiverMismatch) + arg arity/types. Two new
`TypedExprKind` variants (ImplMethodCall + QualifiedCall) with
substitute impls + walk methods updated across
walk_expr_for_mono / count_performs / find_unique_perform /
walk_collect_var_refs / find_var_name_in_expr. Four new
TypeError variants: ImplMissingMethod,
ImplMethodSignatureMismatch, AmbiguousMethodCall,
ImplMethodReceiverMismatch.

**Codegen additions**: per-impl LLVM fn declarations in pass 1
(mangled `default__Type__Trait__method` for default impls or
`Name__Type__Trait__method` for named impls); per-impl body
compilation in pass 2 via the new `compile_impl` method
(mirrors `compile_class`'s method-emission loop). Both
ImplMethodCall and QualifiedCall lower to direct calls into
the impl method's mangled fn. ImplMethodCall uses
`lower_lvalue_ptr` on the receiver target (same as class
MethodCall); QualifiedCall lowers `args[0]` as a value (the
user wrote `&mut s`, which is already a pointer) and passes it
as self_ptr. `impl_method_fns: HashMap<(ImplId, usize),
FunctionValue>` field added to CodegenCtx. Witness-table
machinery from ADR 0023 D9 is **DEFERRED** alongside Path 3:
at C4.2 minimum only direct-call dispatch lands; witness
tables are scaffolding for the bounded-generic dispatch path,
which itself is deferred.

**Borrow-check fix**: existing C4.1 MethodCall arm was walking
the receiver via `walk_expr` (consuming), which would surface a
spurious use-after-move when the same Move-typed receiver is
referenced by multiple method calls. Switched to
`walk_expr_lvalue` (non-consuming, matching FieldAccess +
Index) — auto-ref produces `&target` / `&mut target` which is
a Copy borrow, not a consuming move. The ImplMethodCall arm
follows the same pattern.

**Two new C4.2 pass fixtures + four UI fixtures**:
- `c42_trait_basic.sentinel`: smallest trait + default impl +
  receiver-typed dispatch (`Tally::init().tick(42)` → exit 42).
- `c42_go_no_go.sentinel` (ADR 0023 D12 phase-go): trait Writer
  with one method; class FileSink with init; default + named
  Doubling Writer impls; `s.write(10)` dispatches via the
  default impl (Path 1), `Doubling::write(&mut s, 16)` via the
  named impl (Path 2). count = 10 + 32 = 42. Exit 42.
- `c42_impl_missing_method.sentinel` (UI): impl block missing a
  trait method → `sentinel::types::impl_missing_method`.
- `c42_impl_method_sig_mismatch.sentinel` (UI): impl method's
  param type differs from trait's →
  `sentinel::types::impl_method_signature_mismatch`.
- `c42_duplicate_default_impl.sentinel` (UI): two default impls
  of (Writer, FileSink) →
  `sentinel::resolve::duplicate_default_impl`.
- `c42_duplicate_impl_name.sentinel` (UI): two `Doubling` named
  impls → `sentinel::resolve::duplicate_impl_name`.

**Amendments at C4.2 close**: A1 D5 Path 3 (bounded-generic
dispatch via witness tables) DEFERRED — needs `<W: Writer>`
bounded-generic syntax which is its own substantial surface
(parser + AST + resolve + types + codegen). A2 D9 witness-
table values not emitted — scaffolding for Path 3 which is
deferred. A3 D7's `Type::TraitSelf(TraitId)` interner SHIPPED
but unused at runtime — at C4.2 minimum trait method sigs
don't reference `Self` in their params/returns (only positional
`self: &Self` / `&mut Self` via self_kind, mirroring the C4.1
A2 amendment). The interner is in place for the eventual
`Self`-in-type-position lift.

Workspace test delta from C4.1 close: +16 (1154 total) — +10
resolve unit tests (positive impl/trait id assignment + 9
rejection paths) + +6 driver tests (c42_trait_basic +
c42_go_no_go + 4 UI rejection assertions). **Phase C4.2 (2/N)
closes here.** ADR 0023 flips PROPOSED → ACCEPTED-WITH-
AMENDMENTS. Next: **Phase C4.3** — delegation + auto-forwarder
codegen per ADR 0021 D6, OR Path 3 bounded-generic follow-on
(deferred amendment).

Pre-C4.2(2/N) context: **C4.2 (1/N): trait + impl AST + parser
landed per ADR 0023 D1+D3+D4.** ADR 0023 stays PROPOSED. First
of the C4.2 sub-iterations: trait declarations + impl blocks
(default + named forms) + `ImplName::method(args)` qualified
calls parse end-to-end at the AST + parser layer. Downstream
resolve rejects them with three new "not yet" diagnostics —
TraitDeclNotYet / ImplDeclNotYet / QualifiedCallNotYet — until
C4.2 (2/N) brings up the impl table, dispatch, and codegen
per ADR 0023 D8 + D9.

**AST additions**:
- `TraitDecl { name, methods, span }` + `TraitMethodSig
  { name, self_kind, params, return_type, effect_row, span }`
  alongside `Program.fns` / `.structs` / `.effects` /
  `.classes`.
- `ImplDecl { name: Option<String>, trait_name, type_name,
  methods, span }` + `ImplMethodDef { visibility, name,
  self_kind, params, return_type, effect_row, body, span }`.
  `name: None` for default impls; `Some` for named impls.
- `ExprKind::QualifiedCall { impl_name, impl_name_span,
  method, method_span, args }` — disambiguated from
  `ClassInit` at parse time by checking if the second Ident
  after `::` is `init`.
- `Program.traits: Vec<TraitDecl>` + `Program.impls:
  Vec<ImplDecl>`.

**Parser additions**: `parse_trait_decl` + `parse_trait_method_sig`
(method signature terminated by `;`, no body). `parse_impl_decl`
distinguishing default vs named via the optional `Ident` before
`as`. `parse_impl_method_def` mirrors `parse_method_decl` from
C4.1. The `parse_atom` Ident::method arm in C4.1 — which only
accepted `Init` — now also accepts a generic Ident; the dispatch
distinguishes `ClassInit` (second Ident = `init`) from
`QualifiedCall` (second Ident = method name). Empty trait bodies
(`trait T {}`) and method-bodies-instead-of-`;` get clear
UnexpectedToken diagnostics.

**Resolve pass-through**: at the start of `resolve()`, if
`program.traits` or `program.impls` is non-empty, surface
`TraitDeclNotYet` / `ImplDeclNotYet` (mirroring the C3.0
EffectDeclNotYet pattern). Inside `resolve_expr`,
`ExprKind::QualifiedCall` surfaces `QualifiedCallNotYet`.
Three new ResolveError variants land — all wired to clear
help text pointing at C4.2 (2/N) per ADR 0023 D8.

Workspace test delta from C4.1 close: +14 (1138 total) — +11
parser tests (trait empty / one-method / two-methods /
method-requires-semi / impl default / impl named / impl pub /
impl missing-as / impl missing-for / impl two-methods / full
C4.2 surface) + +3 resolve rejection tests (TraitDeclNotYet,
ImplDeclNotYet, QualifiedCallNotYet) + 1 promoted C4.1 test
(parse_qualified_call_with_non_init_ident replaces the old
parse_class_init_rejects_non_init).
**Phase C4.2 (1/N) closes here.** Next: C4.2 (2/N) — resolve
(TraitId + ImplId interners + per-scope impl table) + types
(`Type::TraitSelf(TraitId)` interner extension + per-impl
signature check + dispatch resolution per ADR 0023 D6) +
codegen (per-impl LLVM fn mangling + witness-table machinery
+ Path 1 / Path 2 lowering) + c42_go_no_go phase-go.

Pre-C4.2(1/N) context: **ADR 0023 PROPOSED: concrete C4.2 trait + impl
surface.** No code changes — this is the C4.2 detail ADR
mirroring ADR 0022 (C4.1 classes) within the larger Phase C4
plan from ADR 0021. Twelve D-decisions cover trait declaration
grammar (D1), method signature shape inside traits (D2),
default impl block grammar `impl as Trait for Type` (D3),
named impl form `impl Name as Trait for Type` per ADR 0021 D5
(D4), three dispatch paths (D5: receiver-typed +
qualified-named + bounded-generic via witness tables),
method-call resolution algorithm (D6), `Self` in trait /
impl contexts via the new `Type::TraitSelf(TraitId)` interner
variant (D7), the typing pipeline (D8: two new resolve passes
+ one new types pass + one new bodies pass), codegen with
per-impl LLVM fn + global witness-table values (D9), the
out-of-scope list (D10: default method bodies, supertraits,
generic traits, `dyn Trait`, bounded-generic + named-impl
pairing, generic-impl, cross-module coherence, associated
types, where clauses), the C4.0 lexer state (D11: no new
tokens — `trait`/`impl`/`as`/`for` reserved at C4.0), and
the c42_go_no_go phase-go fixture (D12: trait Writer with
two impls — default + named `Doubling` — on FileSink class,
demonstrating receiver-typed + qualified-named dispatch
together).

**Key D5 amendment** (bounded-generic + named-impl pairing):
the C4.2 minimum ships Path 3 (`fn use_writer<W: Writer>(w:
W)`) for **default impls only** — the `@`-form turbofish
syntax for picking a named impl at the call site is deferred
to a follow-on. ADR 0021 D5's static-by-default dispatch
holds; named impls participate in Path 1 (receiver-typed)
and Path 2 (qualified-named) only at C4.2.

Workspace test count unchanged (1124 + 1 doctest); no code
shipped in this docs-only commit.

Pre-ADR-0023 context: **C4.1 close-out: ADR 0022 → ACCEPTED-WITH-AMENDMENTS;
Phase C4.1 closes.** Two C4.1 sub-iterations shipped (C4.1
(1/N) AST + parser; C4.1 (2/N) resolve / types / codegen +
method-call + Name::init). All eleven D-decisions exercised
modulo two amendments: A1 the D4 definite-assignment dataflow
is partial (flat any-assigned check; branch-aware merge +
InitFieldReadBeforeAssign deferred); A2 `Self` in general
type position deferred (only positional `self: &Self` via
parse_self_param at C4.1). The phase-go fixture (Point with
manhattan + translate) runs end-to-end at exit 42; the
amendments don't block C4.2.

ADR 0021 stays PROPOSED — flips at C4.5 close after the full
Phase C4 surface (traits + impls + delegation + structured
concurrency) lands.

Workspace test count unchanged from C4.1 (2/N) close (1124 +
1 doctest); this is a docs-only commit pair.

Pre-C4.1-close context: **C4.1 (2/N): resolve / types / codegen wired up for classes;
`Name::init(args)` + postfix `.method(args)` ship end-to-end.**
ADRs 0021 + 0022 stay PROPOSED until C4.1 close. Second of
the C4.1 sub-iterations: class declarations now flow through
the full pipeline. `Point::init(10, 20).translate(3, 9)` works
end-to-end per ADR 0022 D11. Definite-assignment runs at type-
check with a flat any-assigned check (branch-aware merge
deferred); the c41_init_field_unassigned UI fixture pins the
rejection.

**Parser additions**: postfix `.method(args)` extension to
`parse_postfix` (Dot followed by Ident-then-LParen produces
`ExprKind::MethodCall`; Ident-then-anything-else produces
`FieldAccess` as before). `Name::init(args)` extension to
`parse_atom`'s Ident arm (Ident followed by `::` then `init`
then `(args)` produces `ExprKind::ClassInit`). Non-`init`
associated fns (`Name::new(...)`) rejected with a clear
expected-`init` diagnostic; the surface stays minimal at C4.1
per ADR 0022 D5.

**AST additions**: two new `ExprKind` variants
— `MethodCall { target, method, method_span, args }` and
`ClassInit { class_name, class_name_span, args }`. Display
impls follow the existing pattern.

**Resolve additions**: `ClassId(u32)` interner +
`ResolvedClassDecl` / `ResolvedClassField` / `ResolvedInitDef`
/ `ResolvedMethodDef` parallel-tree types +
`ResolvedProgram.classes: Vec<ResolvedClassDecl>`. Pass 0c
builds class_table alongside struct_table; classes share a
namespace with structs (RedefinedClass on collision). Pass 3
resolves init + method bodies; each binds synthetic `self`
VarId so `Var("self")` lookups against the standard vars
HashMap. `ResolvedExprKind` gains `MethodCall` + `ClassInit`
variants (parallel to AST). Five new ResolveError variants:
RedefinedClass, UndefinedClass, DuplicateClassField,
DuplicateClassMethod, SelfOutsideClassContext (fires when
`self` used outside a class context, e.g., `fn main() { self
}`).

**Types additions**: `Type::Class(ClassId)` interner extension
preserving the `Copy + Hash` invariant (the eighth interner-
table ADR running: 0014 / 0015 / 0016 / 0017 / 0019 / 0020 /
0022). `TypedProgram.class_decls: Vec<ClassData>` holds
field types + init signature + per-method signatures with
bodies. Pass 0 builds both struct_table AND class_table so
field/signature type-exprs can reference either. Pass 3.5
(new) populates class signatures with stub bodies before fn-
body type-check so fn bodies can call `Name::init(args)` /
`obj.method(args)` via the typed class_decls table. Pass 5
(new) overwrites stubs with real type-checked bodies. `self`
binds as `Type::Class(class_id)` with the mutable bit
tracking `&Self` (false) vs `&mut Self` (true) — keeps the
existing struct field-access + assignment machinery
applicable without new ref-deref logic. **FieldAccess** in
`check_expr` extends with a `Type::Class` arm (lookup via
`ClassData::field`). **Definite-assignment** (minimal): walk
init body, collect field names assigned via `self.field =
expr` anywhere; reject any declared field not in the set with
InitFieldMaybeUnassigned. Branch-aware merge (if/else
snapshot+restore mirroring the C2 borrow CFG) deferred to a
follow-on iteration. Two new `TypedExprKind` variants
(MethodCall + ClassInit) with `Substitute` impls + walk
methods updated across walk_expr_for_mono / count_performs /
find_unique_perform / walk_collect_var_refs /
find_var_name_in_expr. Six new TypeError variants:
InitFieldMaybeUnassigned, ClassConstructionMustUseInit
(reserved but currently unused — the resolve-layer
UndefinedStruct catches struct-lit on class names),
MethodNotFound, MethodCallOnNonClass,
ClassInitArityMismatch, MethodArityMismatch.

**Codegen additions**: per-class LLVM struct type in pass 0
(declared opaque alongside structs, body set after); per-class
init + per-method LLVM fn declarations in pass 1 (mangled as
`Name__init` / `Name__method`); per-class body compilation in
pass 2 via the new `compile_class` method. Init has the
`out_ptr` ABI per ADR 0022 D9 — first param is a pointer to
caller-provided storage; field writes inside the body GEP into
that storage. Methods take `self_ptr` as the first param.
**Inside init/method bodies** self_var_id binds directly to
the incoming pointer (no extra alloca + store + load); subsequent
`self.field` reads + writes GEP straight into the class
storage. **MethodCall** lowers via `lower_lvalue_ptr` on the
receiver + direct call to the mangled method fn. **ClassInit**
lowers via alloca for the class struct + call to the mangled
init fn (passing alloca as out_ptr) + load of the alloca.
Drop emission for class instances follows the C2.4 recursive
struct field drop machinery (Class arm in
`emit_drop_for_binding`); at C4.1 minimum class fields are
typically primitives so no actual drop work fires for the
phase-go fixture.

**Three new C4.1 fixtures**:
- `c41_class_basic.sentinel`: single-field Point with init +
  one shared-self `get` method; `Point::init(7).get()` exits 7.
- `c41_go_no_go.sentinel` (ADR 0022 D11 phase-go): Point with
  two i64 fields + init + shared-self `manhattan` +
  exclusive-self `translate` (which mutates fields then calls
  `self.manhattan()`); p starts at (10, 20); translate(3, 9)
  updates to (13, 29) and returns 42.
- `c41_init_field_unassigned.sentinel` (UI): Pair declares
  `a` + `b` but init only assigns `a`;
  `sentinel::types::init_field_maybe_unassigned` fires.

**Init body limitation carries forward**: init bodies still
parse with a trailing placeholder `0` per the C4.1 (1/N)
note; codegen's `compile_class` strips this by lowering only
the stmts (not the tail) for init. `Block.tail` becoming
`Option<Expr>` is still queued as a follow-on cleanup.

**Carry-overs from this iteration**:
- Branch-aware definite-assignment dataflow (if/else
  snapshot+merge) — current impl is flat any-assigned. Pin
  with an additional UI fixture when it lands.
- Non-lvalue receiver MethodCall (e.g.,
  `Point::init(1, 2).manhattan()` directly) — current impl
  requires an lvalue receiver. Needs an alloca-store-GEP
  detour at the call site.
- `ClassConstructionMustUseInit` is declared but unreachable
  because resolve catches struct-lit-on-class as
  UndefinedStruct first. A follow-on can promote the
  diagnostic for a clearer error.

Workspace test delta from C4.1 (1/N) close: +17 (1124 total)
— +4 syntax parser (method-call postfix + ClassInit + chains
with field-access + non-`init` rejection), +10 resolve
(positive class-decl id assignment / init-call / method-call /
Self in method + 6 rejection paths), +3 driver
(c41_class_basic + c41_go_no_go + c41_init_field_unassigned
UI). **Phase C4.1 (2/N) closes here.** Next: C4.1 close-out
— phase-go integration + ADR 0022 PROPOSED → ACCEPTED flip
(if all D-decisions exercised cleanly; D6's partial-init
field-lvalue form may want a follow-on note), then
**Phase C4.2** — traits + named impls per ADR 0021 D14.

Pre-C4.1(2/N) context: **C4.1 (1/N): class AST + parser landed per ADR 0022.**
ADRs 0021 + 0022 stay PROPOSED. First of the C4.1 sub-iterations:
class declarations parse end-to-end at the AST + parser layer.
Downstream passes (resolve / types / codegen) leave classes
untouched — `Program.classes` is a fresh Vec they don't yet
iterate. Class declarations parse but go nowhere through the
pipeline; the resolve/types/codegen wiring is the next C4.1
sub-iteration.

**AST additions**:
- `ClassDecl { name, fields, init, methods, span }` alongside
  `Program.fns` / `.structs` / `.effects`.
- `ClassField { visibility, name, ty }`.
- `InitDef { visibility, params, body }`.
- `MethodDef { visibility, name, self_kind, params,
  return_type, effect_row, body }`.
- `Visibility { Private, Public }` parsed but not enforced
  until Phase C5 modules.
- `SelfKind { Shared, Exclusive }` captures
  `self: &Self` vs `self: &mut Self`.

**Lexer addition**: `ColonColon` token for `Name::init(args)`
per ADR 0022 D5.

**Parser additions**: `parse_class_decl` + dispatchers
(`parse_class_field` / `parse_init_decl` / `parse_method_decl`
/ `parse_self_param` / `parse_optional_visibility`). Generic
classes (`class Pair<A, B>`) explicitly rejected per ADR 0022
D1; the typed AST extension is its own follow-on. The new
`DuplicateClassInit` ParseError variant enforces ADR 0022 D4's
at-most-one-init rule.

**`parse_atom`** gains a `SelfVal` arm: `self` is emitted as
`Var("self")` at the AST layer; resolve checks the class-
context constraint per ADR 0022 D8.

**Init body limitation**: the existing `Block` requires a
trailing expression; init bodies temporarily carry a
placeholder `0` until `block.tail` becomes `Option` in a
follow-on. The typing layer will strip the placeholder when
init resolution lands.

**Method-call form** (`obj.method(args)`) is NOT in this
iteration — postfix `.method(args)` parsing arrives alongside
the resolve/types/codegen wiring. Method bodies that
reference other methods use direct-field-access workarounds
in the parser tests.

Workspace test delta from ADR 0022 PROPOSED: +14 (1107 total)
— +3 lexer (ColonColon + colon-vs-coloncolon + init-call
skeleton) + +11 parser (empty class / fields-only / pub field
/ init / methods / full-surface + 3 rejection paths).
**Phase C4.1 (1/N) closes here.** Next: C4.1 second iteration
— resolve / types / codegen wiring + method-call parsing +
typed `Type::Class(ClassId)` interner.

Pre-C4.1(1/N) context: **ADR 0022 PROPOSED: concrete C4.1 class surface.**
No code changes — this is the C4.1 detail ADR mirroring ADR
0013 (C1.4 structs) within the larger Phase C4 plan from ADR
0021. Eleven D-decisions cover the class declaration grammar
(D1), field declarations (D2), method declarations with
`self: &Self` / `self: &mut Self` (D3), `init(args)`
constructor + definite-assignment (D4), class instantiation
via `Name::init(args)` — struct-literal syntax rejected for
classes (D5), field access reusing C1.4 + C2 lvalue
machinery (D6), static method dispatch (D7), `Self` + `self`
resolution rules (D8), codegen lowering classes as LLVM
struct + per-class method namespace + init `out_ptr` ABI
(D9), no new lexer tokens (D10 — all reserved at C4.0), and
the c41_go_no_go phase-go fixture (D11).

**Key amendments to D5**: the `new Name(args)` sugar floated
in ADR 0021 D3 stays rejected at C4.1 — `Name::init(args)`
form composes cleanly with future associated fns + no new
keyword needed. Struct-literal syntax `Name { field: value }`
remains valid for `struct` declarations but rejected for
`class` declarations to enforce the no-half-constructed
invariant.

Workspace test count unchanged (1093 + 1 doctest); no code
shipped in this docs-only commit.

Pre-ADR-0022 context: **C4.0 landed; lexer keywords for Phase C4.**
ADR 0021 stays PROPOSED. Twelve new TokenKind variants reserve
the Phase C4 surface: `class`, `trait`, `impl`, `init`,
`delegate`, `scope`, `spawn`, `await`, `Self`, `self`, `as`,
`for`. `Self` (SelfTy) and `self` (SelfVal) are distinct
case-distinguished tokens per ADR 0021 D2's split between the
implementing type and the receiver value. `to` (inside
`delegate inner: T to Trait;`) and `concurrent` (inside
`scope concurrent`) remain plain Idents — the parser
recognises them positionally per the smallest-surface
principle.

The parser doesn't yet activate any of these — they're
lexer-reserved at C4.0; the parser layer brings them online
sub-phase by sub-phase per ADR 0021 D14:
  - C4.1: class / init / Self / self.
  - C4.2: trait / impl / as / for.
  - C4.3: delegate.
  - C4.4: scope / spawn / await.

Workspace test delta from ADR 0021 PROPOSED: +18 (1093
total) — +18 lexer tests (one per new keyword + an ident-
prefix regression + skeleton sequence tests for class /
trait / impl / delegate / scope+spawn+await surfaces).
**Phase C4.0 closes here.** Next: **C4.1** — class
declarations + methods + init constructor per ADR 0021 D1-
D3 + the matching detail ADR 0022 PROPOSED at sub-phase
open.

Pre-C4.0 context: **ADR 0021 PROPOSED: Phase C4 kickoff (classes + traits + delegation + structured concurrency).**
No code changes — this is the pre-flight design ADR for Phase
C4 per the ADR-first norm. ADR 0021 carries 14 D-decisions:
classes as struct+method+impl+delegation sugar (D1), method
declarations with `self: &Self` per ADR 0017 D6 (D2), `init`
constructors with definite-assignment check (D3), trait decls
+ named implementations (D4 + D5), delegation auto-forwarder
codegen (D6), scope-local coherence (D7), structured
concurrency primitives `scope`/`spawn`/`await` (D8), async-as-
effect via a runtime scheduler — closes ADR 0019 D9 (D9),
deferred items: actors → C5, dynamic dispatch + default
methods + trait inheritance (D10 + D12), ~10 new lexer
keywords (D11), phase-go program (D13), 6-sub-phase split
covering C4.0 lexer → C4.5 close-out (D14).

The ADR commits to **named impls** (instead of Rust's orphan
rule) + **delegation** (instead of class inheritance, per
SENTINEL_DESIGN §7) + **async-as-effect via the ADR 0020
free-monad runtime + a new work-stealing scheduler in C4.4.**
The scheduler is the largest single new runtime component
since Phase A's broker; ADR 0024 will carry that substantive
design call at C4.4 open. Estimated total: 8-12 sessions
across 6 sub-phases.

Workspace test count unchanged (1075 + 1 doctest); no code
shipped in this docs-only commit.

Pre-ADR-0021 context: **C3.7 landed; ADR 0020 → ACCEPTED-WITH-AMENDMENTS; Phase C3 closes.**
ADR 0020 flips PROPOSED → ACCEPTED-WITH-AMENDMENTS with all
eight sub-phases shipped (C3.4 + C3.5(a/b/c/d/e) + C3.6(a/b) +
C3.7). The single amendment: **D2's multi-shot relaxation is
deferred** indefinitely — Phase B's one-shot validation demos
all passed, and the C3 bootstrap minimum doesn't surface a
multi-shot use case. The upgrade path (deep-clone the frame
chain + captured-state on each resume entry) stays mechanical.

**C3.7 ships**:
- `lower_handle`'s body restriction is **fully lifted**. Any
  expression that produces a value is accepted — pure i64
  bodies wrap via `sentinel_kont_pure` at the handle site so
  the dispatch loop's PURE_RETURN switch case fires. The
  previous Perform / Call-to-effecting / Handle gate goes
  away. Empty arms (just a return arm) is permitted; the
  merge phi takes its type from pure_val when arm_results is
  empty.
- The legacy `HandleBodyNotDirectPerform` codegen variant
  remains in the error enum for backward-compat but is no
  longer reachable from codegen.

**Three new C3.7 fixtures**:
- `c37_go_no_go.sentinel` (ADR 0020 D12 phase-go): do_work(42)
  performs Io.log; handler resumes with msg + 1 = 43; do_work
  computes x + logged = 42 + 43 = 85; print outputs "85\n";
  exit 0.
- `c37_handle_return.sentinel`: `handle 42 with { return v =>
  v * 2 }` — pure body wrapped via sentinel_kont_pure; PURE_RETURN
  switch case fires the return arm transform; exit 84.
- `c37_perform_outside_handle.sentinel` (UI): bare `perform
  Io.log(...)` in main; ADR 0019 D13's unhandled_effect
  rejection fires (snc exit 1).

Workspace test delta from C3.6(b) close: +3 (1075 total) — +2
driver pass-tests (c37_go_no_go, c37_handle_return) + 1
driver UI test (c37_perform_outside_handle).

**Phase C3 closes here.** The handler-runtime layer is
complete; the language surface now supports the full
effect-system trifecta from HANDOVER §6.2: secret-typing
(C3.1), effect rows + main-must-be-effect-free (C3.2/C3.3),
and handler runtime with deep-handler one-shot continuations
(C3.4 → C3.7). Twenty ADRs landed across Phases A, B, C0,
C1, C2, C3.

Next: **Phase C4** per HANDOVER §6.2 — traits + structured
concurrency. ADR 0017's polonius migration (ADR 0018) is also
queued as a Phase C2 follow-on, plus the documented Phase C2
soundness gap (partial-move-through-field-projection) closure.

Pre-C3.7 context: **C3.6(b) landed; nested handles per ADR 0020 D3.**
ADR 0020 stays PROPOSED. `lower_handle`'s body restriction
extends to also accept nested `Handle` expressions; an inner
handle (detected via `CodegenCtx.handle_depth > 1`) emits
Kont*-typed merge values instead of i64 so the enclosing outer
handle can dispatch on the result. Switch's default destination
flips from `handle_unreachable` (top-level) to
`handle_propagate` (nested), contributing the un-caught
current_kont to the merge. Inner-handle arm bodies' i64 results
get wrapped via `sentinel_kont_pure` before joining the merge;
the pure-return switch case either passes current_kont through
(no return arm) or consume_pure + apply + re-wrap (with return
arm).

**Nesting detection**: `CodegenCtx.handle_depth: u32` is
incremented at `lower_handle` entry + decremented at exit.
`is_nested = depth > 1`. A thin wrapper around
`lower_handle_inner` ensures the decrement fires on the early-
error path too.

**Two new pass fixtures**:
- `c36b_nested_handle_basic.sentinel`: do_both has chained
  effecting lets {Io.read, Net.fetch}; inner catches Io
  (k(7)), inner's propagate routes Net.fetch to outer, outer
  catches Net (k(35)), 7 + 35 = 42.
- `c36b_nested_handle_inner_full.sentinel`: inner catches Io
  fully (k(15)), wraps via pure_kont, outer's PURE_RETURN
  switch case unwraps; result + 27 = 42.

**Still pending for C3.6(c) / C3.7**: multi-shot continuations
(ADR 0020 D2 relaxation — needs deep-clone-on-resume + sharing
analysis for captured state), non-i64-returning ops, embedded
performs in chained-let RHSes, phase-go fixture per ADR 0020
D12.

Workspace test delta from C3.6(a) close: +2 (1072 total) — +2
driver pass-tests (c36b_*). **Phase C3.6(b) closes here.**
Next: C3.6(c) — multi-shot continuations per ADR 0020 D2, or
skip to C3.7 polish + phase-go fixture.

Pre-C3.6(b) context: **C3.6(a) landed; non-identity return arms per ADR 0020 D4.**
ADR 0020 stays PROPOSED. `lower_handle` now actually uses the
`return_arm` parameter: in the pure-return switch case, bind
the return arm's value VarId to the unwrapped i64 + lower the
body — its result flows to the handle's merge phi. Without a
return arm, the unwrapped value flows directly (default
identity `return v => v`).

**Phase B parity via deep-handler re-wrap**: `HandleContext`
grows a `return_arm: Option<TypedReturnArm>` field (cloned at
`lower_handle` entry). `lower_resume_kont` consults this in
its pure-unwrap path so k(v)'s value is the return-arm'd i64,
not the raw resumed i64 — matching Phase B's `k := \v. handle
(kont.resume v) with H` semantics where every continuation
call re-wraps in the handler (return arm included). Without
this k(v) treatment, the return arm would only fire when the
body produces a value WITHOUT performing (pure-bodied
effecting fn); the re-wrap extension makes the return arm fire
uniformly whenever the resumed computation drains to a value.
The `HandleContext` struct drops its `Copy` derive in favour
of `Clone`; `lower_resume_kont` snapshots once + reuses for
both pure and bubble branches.

**Two new pass fixtures**:
- `c36a_return_arm_transform.sentinel`: pure-bodied effecting
  fn → outer pure-dispatch fires return arm → 21 * 2 = 42.
- `c36a_return_arm_after_resume.sentinel`: handle's body
  performs Io.read, arm body is `k(21)`, return arm is `v =>
  v * 2` → k(21)'s pure-unwrap path applies the return arm →
  21 * 2 = 42.

**Still pending for C3.6 / C3.7**: nested handles (scoped
frame ownership when an inner handle's body emits an op the
inner doesn't catch — needs to propagate the bubble to the
outer handle), multi-shot continuations (ADR 0020 D2
relaxation — needs deep-clone-on-resume + sharing analysis),
non-i64-returning ops, embedded performs in chained-let
RHSes.

Workspace test delta from C3.5(e) close: +2 (1070 total) — +2
driver pass-tests (c36a_*). **Phase C3.6(a) closes here.**
Next: C3.6(b) — nested handles per ADR 0020 D3, then C3.6(c)
— multi-shot continuations per ADR 0020 D2.

Pre-C3.6(a) context: **C3.5(e) landed; chained effecting lets via resumer-can-perform.**
ADR 0020 stays PROPOSED. This sub-phase closes the
resumers-can-perform gap from C3.5(c)/(d) — a `let v: i64 =
perform Op` inside another resumer's body now bubbles its
result kont back to the enclosing handle's dispatch loop
instead of asserting pure-return. Effecting fn bodies of the
shape `let a = perform; let b = perform; ...; pure_tail` (any
N >= 2 chained effecting lets) now compile end-to-end via N
per-let resumer fns, emitted iteratively in
`compile_effecting_fn_with_chained_lets`.

**Runtime change** (`sentinel_kont_resume`): return type widens
from `i64` to `*mut SentinelKont`. The chain-drain loop, on
encountering a resumer's non-PURE_RETURN result kont, splices
the original kont's remaining frames onto the bubble's chain
tail (deep-handler re-wrap per ADR 0020 D3) and returns the
bubble. If the chain drains pure, the final i64 is wrapped via
`sentinel_kont_pure` so the returned kont is uniform. Existing
runtime tests adjusted: callers now unwrap via
`sentinel_kont_consume_pure` instead of reading the i64
directly.

**Codegen change** (`lower_handle` + `lower_resume_kont`): the
handle's switch is now inside a dispatch *loop*. An alloca-backed
`current_kont_slot` holds the kont the switch reads each
iteration; `lower_resume_kont`'s bubble path stores the new
kont into the slot + branches to the loop block. The pure path
calls `sentinel_kont_consume_pure` to recover the resumed i64.
`CodegenCtx` grows a `handle_stack: Vec<HandleContext>` that
`lower_resume_kont` consults at lowering time to find the
enclosing handle's loop block + slot.

**Chained-lets compilation**:
`detect_chained_effecting_lets_shape` matches `stmts.len() >=
2`, every stmt is `let v: i64 = <produces-kont>`, tail is
pure. Pass 1 pre-declares N resumer fns + computes the per-
resumer capture set via `compute_chained_lets_captures` (vars
referenced in `lets[i+1..].rhs + tail`, minus the future-let-
bound ids and the let_i value param).
`compile_effecting_fn_with_chained_lets` emits each resumer
body in turn: bind let-i to the value param + unpack captures,
then either (a) wrap the pure tail (last resumer) or (b) build
captures for resumer-(i+1), lower `lets[i+1].rhs` to a kont,
push the next resumer + return the kont. The parent fn body
mirrors case (b) with fn params instead of unpacked captures.
A new `emit_chained_lets_captures_alloc` helper centralises
the captured-struct alloc + populate step.

**Three new pass fixtures**:
- `c35e_chained_perform.sentinel`: two reads + sum → 42.
- `c35e_chained_perform_with_capture.sentinel`: two reads with
  outer `base: i64` captured forward through both resumers
  → 42.
- `c35e_chained_dependent_perform.sentinel`: second perform's
  arg uses the first let's binding (validates that resumer-0
  correctly captures `a` for use in resumer-1's perform arg)
  → 42.

**Still pending for C3.6 / C3.7**: return arms with non-
identity transforms (ADR 0020 D4 generalisation), nested
handles (scoped frame ownership), multi-shot continuations
(ADR 0020 D2 relaxation), non-i64-returning ops, embedded
performs in chained-let RHSes (e.g.,
`let a = perform Op() + 1`).

Workspace test delta from C3.5(d) close: +3 (1068 total) — +3
driver pass-tests (c35e_*). **Phase C3.5(e) closes here.**
Next: C3.6 — return arms + nested handles + multi-shot
relaxation per ADR 0020 D2/D4.

Pre-C3.5(e) context: **C3.5(d) landed; unified embedded-perform shape.**
ADR 0020 stays PROPOSED. This sub-phase generalises C3.5(c)'s
per-let frame reification: an effecting fn whose tail contains
**exactly one perform anywhere** in its tree (binop, pure fn-
call arg, struct-lit field, field-access target, index, unary
op, etc. — any pure surrounding context) now compiles via the
same machinery. Codegen detects the shape, **substitutes the
perform with a placeholder Var** in a clone of the tail, and
emits a resumer fn that lowers the substituted tail with the
placeholder bound to the resumed value.

**Substitution machinery**: three new walkers cover the typed
AST. `count_performs` returns the number of Perform expressions
anywhere in the subtree (including nested in args). `find_unique_perform`
returns a borrowed reference to the first Perform in pre-order
(unique when count == 1). `substitute_perform_with_var` deep-
clones the tree, replacing the Perform with `Var(placeholder_id)`.
These traverse every TypedExprKind variant (binop, struct-lit,
field-access, index, if, block, handle, ...) so the unified
detection covers the entire pure-surrounding-context space.

**Detection** (`detect_embedded_perform_shape`): the body has
`stmts.len() == 0`, the tail isn't already a direct Perform or
Call-to-effecting-fn (those are handled at C3.5(a)/(b)),
contains exactly one perform whose return type is i64 (MVP
restriction), and is fully effecting (`expr_performs(tail) ==
true`). The shape and let-shape (C3.5(c)) are disjoint —
let-shape requires stmts.len() == 1, embedded-shape requires
stmts.len() == 0.

**Codegen** (`compile_effecting_fn_with_embedded_perform`):
mirrors `compile_effecting_fn_with_let` but the resumer body
lowers the *substituted tail* instead of the original. The
parent fn body locates the unique perform via
`find_unique_perform`, lowers it directly via `lower_perform`,
then pushes a frame referencing the resumer + captured-state
struct. Captured VarIds are collected from the substituted tail
(excluding the placeholder).

**Three new pass fixtures**:
- `c35d_binop_with_perform.sentinel`: `perform Io.read() + 1`
  → resume(41) → 42.
- `c35d_perform_with_capture_and_binop.sentinel`:
  `perform Io.read() * 2 + extra` with `extra` captured from
  fn param → do_work(2) + resume(20) → 42.
- `c35d_perform_in_call_arg.sentinel`:
  `double(perform Io.read())` where `double(x)` is a pure
  user fn → resume(21) → double(21) = 42.

**Still pending for C3.5(e) / C3.6**: chained effecting lets
(`let a = perform Op1(); let b = perform Op2(); ...`) which
require resumers-can-perform — the runtime's resume loop
currently assumes pure-return wraps; nested perform inside a
resumer needs a different bubble-up path. Multi-arg ops (kont
struct's single `arg: i64` slot accommodates one value).
Non-i64-returning ops (placeholder type is hardcoded i64).
Return arms with non-identity transforms (ADR 0020 D4
generalisation). Nested handles + multi-shot continuations
(ADR 0020 D2 relaxation).

Workspace test delta from C3.5(c) close: +3 (1065 total) — +3
driver pass-tests (c35d_*). **Phase C3.5(d) closes here.**
Next: C3.5(e) — chained effecting lets via resumer-can-perform
support (requires runtime resume loop update for non-pure-
return result konts) OR C3.6 polish (return arms / nested
handles / multi-shot).

Pre-C3.5(d) context: **C3.5(c) landed; let-bound perform via frame reification.**
ADR 0020 stays PROPOSED. This sub-phase adds the FIRST piece of
per-evaluation-site frame reification: an effecting fn whose body
is a single let with effecting RHS + pure tail now compiles via
codegen-emitted **per-let resumer fns** + **captured-state
structs**. The fifth runtime symbol `sentinel_kont_push` lands;
`sentinel_kont_resume` extends to **replay the frame chain in
head→tail order** (innermost first) — each frame's resumer is
called with the accumulated value, the result's pure-return arg
becomes the next iteration's value, and the final unwrapped
value is what resume returns.

**Codegen pattern**: at compile time, codegen detects effecting
fns where `body = { let v: T = effecting_rhs; pure_tail }`. For
each match: walks the tail to collect referenced VarIds
(excluding the let-bound), filters to fn params (MVP
restriction), declares a per-let resumer fn `__resume_<fn>_let_<n>`
with signature `ptr (i64 value, ptr captured)`. The resumer's
body allocates stack slots for the let-bound var (filled from
the resumed value) + each captured var (loaded from the
captured-state struct via byte-offset GEP), lowers the tail
expression normally, wraps the i64 result in
`sentinel_kont_pure`, and returns. At the let-site in the
parent fn, codegen allocates the captured struct via
`sentinel_alloc(N*8)`, stores each fn param's value, lowers the
RHS (perform / effecting call) to get a Kont*, calls
`sentinel_kont_push(kont, resumer, captured)`, and returns the
Kont*.

**Runtime extension**: `SentinelKont` grows a `frames_head:
*mut SentinelFrame` field at offset 24 (with 7-byte pad after
the consumed flag for stable 8-byte alignment). New struct size
is 32 bytes. `SentinelFrame` is a linked-list node holding
`{ resumer, captured, next }`. `sentinel_perform_op` initialises
`frames_head` to null. `sentinel_kont_push` allocates a new
Frame, links into the kont's chain at head. `sentinel_kont_resume`
walks the chain: for each frame, calls `resumer(value, captured)`
to get a result Kont (a pure-return wrap at C3.5(c) MVP);
unwraps the result's arg as the next value; frees the result
kont, the captured ptr, and the frame itself; advances to next.
After draining the chain, frees the original kont and returns
the final accumulated value.

**Three new pass fixtures**:
`c35c_let_bound_perform.sentinel` (no captures: `let v = perform
Io.read(); v + 1` → exit 42), and
`c35c_let_bound_perform_with_capture.sentinel` (captures fn
param `offset`: do_work(35) with handler resume(7) → 7 + 35 =
42 → exit 42). The previous C3.5(b)-era UI fixture
`c35b_effecting_fn_let_bound_perform.sentinel` (which asserted
codegen rejection of let-bound performs) was retired since
that shape now compiles + runs.

**Still pending for C3.5(d) / C3.6**: perform-inside-binop
(`perform Op() + 1`), perform-inside-if-cond
(`if perform Op() { ... } else { ... }`), chained effecting
lets (`let a = perform Op1(); let b = perform Op2(); ...`),
return arms with non-identity transforms, nested handles,
multi-shot continuations (ADR 0020 D2 relaxation).

Workspace test delta from C3.5(b) close: +1 (1062 total) — +2
driver pass-tests (c35c_*), -1 driver UI test (the c35b
let-bound-perform rejection assertion was retired since that
shape now succeeds). **Phase C3.5(c) closes here.** Next:
C3.5(d) — extend frame reification to binop / if-cond / chained
lets, then C3.6 (return arms, nested handles, multi-shot
relaxation), then C3.7 (close-out).

Pre-C3.5(c) context: **C3.5(b) landed; effecting fn ABI + handle-of-call.**
ADR 0020 stays PROPOSED. This sub-phase extends the handler
runtime so `handle do_work() with { ... }` (where do_work is
declared `! { Io }` with body that's a direct perform OR a
call to another effecting fn) compiles + runs end-to-end. The
substantive piece: **effecting fns' LLVM ABI returns Kont*
(opaque pointer)** instead of their declared Sentinel return
type. The fn's declared type stays the same at the type-
system layer for effect-row reasoning; only the IR shape
changes. Two new runtime symbols round out the ABI:
`sentinel_kont_pure(value)` wraps a pure value in a kont
tagged with the reserved `PURE_RETURN_OP_ID = u32::MAX`
(used when an effecting fn's body produces a value rather
than performing — ADR 0020 D4's default `return v => v`
expressed at the runtime layer); `sentinel_kont_consume_pure`
unwraps it. **Handle codegen extends** to accept body=Call-
to-effecting-fn (in addition to body=Perform) and emits a
**runtime switch** on the kont's op_id with one case per
arm plus the PURE_RETURN_OP_ID case. The static-arm-pick
of C3.5(a) is gone — every handle now uses the switch
uniformly, simplifying codegen.

**Still pending for C3.5(c) / C3.6**: let-bound performs,
perform-inside-binop, perform-inside-if-cond, and other
forms that need per-evaluation-site **frame reification**
(emitting a resumer fn + captured-state struct at each
intermediate site so the kont can carry "rest of the
computation" back through the call chain). Multi-shot
continuations (ADR 0020 D2) are still one-shot only.

Three new pass-test fixtures land: `c35b_handle_fn_call_body`
(promoted from the C3.4-era tests/ui/c34_*: do_work() body is
a direct perform; handle catches the kont, exit 42),
`c35b_handle_multi_arm` (multi-op effect Io.{read, write}
with the matching arm selected via runtime switch on op_id,
exit 42), and `c35b_handle_pure_return` (effecting fn with
pure body `41 + 1`; sentinel_kont_pure wraps; handle's
PURE_RETURN_OP_ID switch case unwraps; exit 42). Plus one
UI fixture `c35b_effecting_fn_let_bound_perform` asserting
the new `effecting_fn_body_not_direct` codegen diagnostic
when a let-bound perform appears (general case not yet
ready). The previous C3.3 + C3.2 phase-go fixtures (c32, c33)
have effecting fns with pure bodies (annotation-as-constraint
demonstration); these now compile through the
sentinel_kont_pure wrap path — no fixture rewrites needed.

Workspace test delta from C3.5(a) close: +3 (1061 total) —
+4 driver pass-tests (c35b_*) + 1 driver UI test, -2 driver
tests (c34 UI-style assertion removed since the fixture
promotes to tests/pass/, and the c34 codegen-rejection test
was retired). No new sentinel-runtime tests at this
sub-phase (sentinel_kont_pure/consume_pure are exercised
end-to-end through the c35b_handle_pure_return fixture).
**Phase C3.5(b) closes here.** Next: C3.5(c) — per-
evaluation-site frame reification (let-stmts, binops,
if-cond) per ADR 0020 D7's `sentinel_kont_push` machinery.

Pre-C3.5(b) context: **C3.5(a) landed; first end-to-end handler runs.**
ADR 0020 stays PROPOSED. The minimum-viable handler runtime
ships here for the *restricted case* — a `handle` body that is
a direct `perform Op(args)` (no fn-call-that-performs, no
nested handles, no intervening evaluation frames). The
**general case** (frame reification at every evaluation site)
remains pending at C3.5(b) / C3.6. **Three new runtime
symbols** in sentinel-runtime: `sentinel_perform_op(op_id: u32,
arg: i64) -> *mut SentinelKont` allocates the kont +
populates its tag and arg; `sentinel_kont_resume(kont, value)
-> i64` flips the one-shot flag, frees the kont, and returns
the value (no frame replay in restricted case);
`sentinel_kont_panic_resumed() -> never` aborts on a second
resume per ADR 0020 D2. The `SentinelKont` struct
(`{ op_id: u32, _pad: u32, arg: i64, consumed: u8 }`, 24
bytes / 8-byte aligned) is the on-heap payload; codegen reads
its `arg` field via GEP at byte offset 8. **Codegen**:
`lower_perform` emits a `sentinel_perform_op(op_id, arg)`
call (op_id = `(EffectId.0 << 16) | op_index`);
`lower_resume_kont` emits `sentinel_kont_resume(kont, value)`;
`lower_handle` (restricted) statically picks the arm whose
(effect_id, op_index) matches the body's Perform, lowers the
body to get the kont*, binds the arm's op-param VarIds via
GEP into the kont struct, binds the kont VarId, then lowers
the arm body. `llvm_basic_type` for `Type::Kont` now returns
a plain `ptr` (opaque). **Three test fixtures**:
`c35_handle_inline_perform.sentinel` (0-arg op + k(42) →
exit 42) and `c35_handle_log_returns_msg.sentinel` (1-arg op
with msg = 7; arm body k(msg+35) → exit 42) both run
end-to-end via snc. The previous c34 fixture (which uses
do_work() as the handle body) now surfaces the more specific
`handle_body_not_direct_perform` codegen diagnostic — the
driver test was updated to match. Workspace test delta from
C3.4 close: +6 (1058 total) — +4 sentinel-runtime (kont
layout / initialisation / resume / round-trip); +2 driver
pass-tests (c35_*). **Phase C3.5(a) closes here.** Next:
C3.5(b) or C3.6 — frame reification at every evaluation
site so `fn-calls-that-perform` and other non-inline
handle bodies work end-to-end. Per ADR 0020 D9 estimate
2-3 sessions total for the perform+handle codegen; ~1
session already spent on the restricted case here.

Pre-C3.5(a) context: **C3.4 landed; handler runtime typing layer in.**
ADR 0020 stays PROPOSED (still in flight across C3.4-C3.7); the
typing-layer pieces — AST, parser, resolve, type-check, effect-
check — all ship here. Codegen for `handle` / `perform` /
continuation resume surfaces a clean
`CodegenError::HandlersNotYetSupported` diagnostic at C3.4 and
lands at C3.5 (perform) + C3.6 (handle) per ADR 0020 D9.
**Surface additions at C3.4**: `ExprKind::Handle { body, arms,
return_arm }`, `ExprKind::Perform { effect, op, args }`, plus
`HandlerArm` and `ReturnArm` AST structs. The handler-arm
syntax `EffectName.OpName(p1, ..., k) => body` plus the
optional `return v => body` parse via two new lexer tokens
(`FatArrow` for `=>`, `Return` for the contextual keyword).
**Resolve mirrors** Handle + Perform with `(EffectId,
op_index)` references; handler-arm params each get a VarId
(last is the continuation `k`). `Call` resolution gains a
vars-first lookup so `k(arg)` inside an arm body resolves to
the new `ResolvedExprKind::ResumeKont { kont, args }` variant
instead of failing as UndefinedFunction. **Type-check** adds a
seventh interner table: `Type::Kont(KontId)` indexes
`TypedProgram.konts: Vec<KontData { arg_ty, ret_ty }>` — the
kont's arg_ty is the op's return type and ret_ty is the outer
`handle` expression's type. Preserves the load-bearing
`Type: Copy + Hash` invariant (now across 7 ADRs: 0014, 0015,
0016, 0017, 0019, 0020). **Effect discharge** per ADR 0020 D6:
`walk_expr` on a `Handle` node walks the body into a
*temporary* set, subtracts the handled (EffectId, op_index)
pairs, then merges into the outer accumulator — so an
`Io`-bearing computation wrapped in a matching handler
contributes the empty row to its enclosing fn. **New error
variants**: ResolveError gains UndefinedHandlerEffect /
UndefinedHandlerOp / DuplicateHandlerArm; TypeError gains
KontUsedAsValue / HandlerArmTypeMismatch /
OperationArityMismatch / KontArityMismatch. Pipeline at C3.4
close: **unchanged** from C3.3 — parse_query → resolve_query →
check_query → effect_check_query → borrow_check_query →
codegen. Handle/Perform/ResumeKont flow as new
TypedExprKind variants through borrow-check (defensive
recursive walk) and bail at codegen. C3.4 fixture at
`tests/ui/c34_handlers_codegen_not_yet.sentinel` exercises the
full flow — body performs Io.read, handler arm calls k(42),
type-check + effect-check + borrow-check pass; codegen
surfaces `sentinel::codegen::handlers_not_yet_supported`. Five
UI fixtures cover the new error variants. Workspace test
delta from C3.3 close: +44 (1052 total) — +4 ast (Handle /
Perform display tests), +19 syntax (7 lexer + 12 parser),
+8 resolve unit tests, +5 types unit tests, +3 effect-check
unit tests, +5 driver tests (4 UI + 1 codegen-rejection
assertion). **Phase C3.4 closes here.** Next: C3.5 — codegen
for `perform` per ADR 0020 D9 (3 new runtime symbols:
sentinel_perform_op + sentinel_kont_push +
sentinel_kont_panic_resumed; 2-3 sessions). Then C3.6 (handle
codegen + sentinel_kont_resume) + C3.7 (polish + phase-go +
ADR 0020 PROPOSED → ACCEPTED flip).

Pre-C3.4 context: **C3.3 landed; Phase C3 typing-layer closes.**
ADR 0019 flips PROPOSED → ACCEPTED-WITH-AMENDMENTS with
D1+D2+D3+D4+D5+D6+D7+D10+D11+D12+D13+D14 all exercised. **D8
(handler runtime) deferred to follow-on ADR 0020** per the
original design call; **D9 (async-as-effect) deferred
indefinitely**. Six sub-phases shipped: C3.0(a) lexer +
C3.0(b) AST+parser+resolve + C3.1 secret typing + C3.1b
operator-secret-preserving + C3.2(a) effect data model +
C3.2(b) effect_check_query crate + C3.3 close-out (this
session, including the phase-go fixture combining everything).
**Pipeline at C3.3 close**: parse_query → resolve_query →
check_query → **effect_check_query** → borrow_check_query →
codegen. New `sentinel-effect-check` crate (~470 LOC) hosts
the effect-check pass; `borrow_check_query` chains on
`effect_check_query` so diagnostics flow transitively through
the existing `accumulated::<Diagnostic>` set. **Two amendments
at C3.3**: A1 the fifth Phase B SecretEscapesPolymorphism
rejection is subsumed by Sentinel's monomorphic generics +
SecretFlow-via-Mismatch (no separate variant needed); A2
runtime builtins (print/unwrap_or/is_some/len) are declared
effect-free at C3.2 to preserve backward-compat with existing
programs that use `print`. **D3 (RowId interner) deferred to
ADR 0020**: at the minimum-viable surface, `Vec<EffectId>` on
each `TypedFnSignature.effect_row` was simpler than a separate
row interner; the interner is a known-shape upgrade path when
rows flow as first-class types (e.g., when handler runtime
arrives + continuations carry rows). C3.3 phase-go fixture at
`tests/pass/c33_go_no_go.sentinel` combines effect_decl +
annotated fn (not called from main) + secret typing + secret
arithmetic + declassify before branching — produces stdout
`42`, exit 0. Workspace test delta from C3.1 close: +20
(1008 total) — +5 resolve unit tests for effect resolution,
+6 types tests for effect-decl + effect-row signature
threading, +8 effect-check unit tests, +1 driver pass-test
(c32_go_no_go: "42") + 1 driver pass-test (c33_go_no_go:
"42"). Phase C3 typing-layer **CLOSES**. Next: ADR 0020 for
handler runtime (D8 deferral) **OR** Phase C4 traits +
structured concurrency per HANDOVER §6.2. ADRs landed in C3:
0019 (PROPOSED → ACCEPTED-WITH-AMENDMENTS at C3.3).

Pre-C3.3 context: **C3.1 landed; secret typing complete; Phase C3
in flight.** ADR 0019 (PROPOSED) kicked off Phase C3 — effect
rows + secret qualifier + handler runtime per HANDOVER §6.2.
Four sub-phases landed: C3.0(a) lexer (six new keywords:
`effect`, `secret`, `declassify`, `handle`, `with`, `perform`),
C3.0(b) AST + parser + resolve for the new surface, C3.1 the
`Type::Secret(SecretId)` interner + declassify + implicit
`T → secret T` widening at let/arg/return boundaries +
2 of 4 static constant-time rejections (`SecretBranch`,
`SecretInRefDeref`), and C3.1b the operator-secret-preserving
rules + `SecretDivisor` rejection. The fifth Phase B
"SecretEscapesPolymorphism" rejection is subsumed under
Sentinel's monomorphic generics + SecretFlow-via-Mismatch
(a generic fn instantiated with a secret type produces a
monomorphic instance whose flow is checked the same way as any
concrete signature — no separate variant needed). **`Type::Secret`
is the sixth interner-table ADR running** (0014 D4 amendment +
0015 D6 amendment + 0016 D6a + 0017 D11 + 0019 D5) preserving
`Type: Copy + Hash`. `TypedProgram.secrets: Vec<SecretData>`
+ `secret_data(id)` accessor + `intern_secret` helper.
Operator typing (`+ - * /`, `== != < <= > >=`, `&& ||`, unary
`- !`) is **secret-preserving**: `secret T op secret T → secret T`
(or `secret bool` for comparisons); mixed-public-secret operands
surface as Mismatch (SecretFlow). `declassify(e)` strips one
layer of Secret; idempotent on non-secret inputs per Phase B
ADR 0008 D5. Codegen lowers `Type::Secret(T)` identically to
T at C3.1; constant-time codegen (branch-free `select`/`cmov`,
speculation barriers) is deferred per ADR 0019 D12.
`TypedExprKind::WidenToSecret` + `Declassify` lower as identity.
`llvm_basic_type` + borrow-check `is_copy_type` + codegen
`emit_drop_for_binding` all strip secrets at entry (so
`secret i64` is correctly Copy; `secret Bag` is Move; drop
recursion sees through the wrapper). Eleven new TypeError
variants total at C3.1 close (`SecretBranch`, `SecretDivisor`,
`SecretInRefDeref` + the dormant `SecretNotYet` from C3.0; the
others are Mismatch-routed). Effect declarations + effect-row
annotations on fn signatures **parse but reject at resolve**
with `EffectDeclNotYet` / `EffectAnnotationNotYet` — the
effect_check_query salsa pass lands at C3.2. The C3.1 phase-go
program at `tests/pass/c31_go_no_go.sentinel` (secret-typed
check_password-style fn with secret arithmetic + secret
comparison + declassify-before-branching) produces stdout
`100\n`, exit 0. Two additional fixtures: c31_secret_typing
(stdout "50" — implicit widen + declassify-then-arith), and the
prior `tests/pass/c25_*` C2 fixtures still build + run.
Workspace test delta from C2.5 baseline: +54 (989 total) —
+8 lexer (six C3 keywords + ident-prefix regression + surface-
skeleton), +16 parser, +4 resolve (rejection arms), +14 types
(+3 interner / +4 widening + declassify / +5 CT rejections +
operator-preserve / +2 div), +2 driver pass-tests (c31_*).
ADR 0019 stays PROPOSED — only the secret-typing slice (D5 +
D6 + D7 mostly + D13 partially) of the 14 D-decisions is
landed; ADR flips to ACCEPTED at C3.3 close-out alongside the
effect_check_query work + main-must-be-effect-free invariant.
**Phase C3.1 closes here.** Next: C3.2 — effect rows in the
typed AST + new `sentinel-effect-check` crate +
`effect_check_query` salsa pass. Estimated 2-3 sessions per
the ADR 0019 sub-phase split table.

Pre-C3.1 context: **C2.5 landed; Phase C2 closes.** ADR 0017
flips PROPOSED → ACCEPTED-WITH-AMENDMENTS with all 14
D-decisions exercised. Four C2.5 deliverables shipped: (a) the
C2.4 recursive-field-drop gap closed; (b) the Polonius
migration plan landed as standalone ADR 0018 per the C2.5
amendment A2; (c) corner-case + limitations scan empirically
confirmed two known lexical over-rejections (borrow-past-last-
use, field-disjoint borrows) + a soundness gap (partial-move-
through-field-projection + drop ⇒ double-free) documented in
`docs/borrow-check-limitations.md`; (d) this banner +
HANDOVER §0.2 / §0.3 refresh + ADR 0017 flip. Four new fixtures
+ driver tests (c25_struct_field_drop / c25_nested_struct_drop
/ c25_generic_struct_array_drop / c25_go_no_go); the D14 phase-
go program at `tests/pass/c25_go_no_go.sentinel` combines `&Bag`
shared borrow (D1 + D3 + D6 shared), `consume(Bag)` move (D9
across a struct with heap-backed array field), `&mut acc`
exclusive borrow + add_into (D1 + D2 + D6 XOR), and recursive
field drop at scope exit (D8 + C2.5(a)) — produces stdout
`190\n`, exit 0. **C2.5(a) codegen**: `emit_drop_struct_fields`
now takes `&TypedProgram`; iterates
`program.struct_decl(id).fields` for `Type::Struct(id)` or
substitutes through `program.generic_instance(id).args` for
`Type::GenericInstance(id)`; per-field GEP via
`build_struct_gep` then recursive `emit_drop_for_binding`. New
`field_type_needs_drop(ty, program)` helper short-circuits the
recursion for pure-data fields (primitives + refs + ?primitive)
with a cycle guard via `seen: Vec<Type>` so recursive structs
via `?Struct` don't infinite-loop. `emit_scope_drops` +
`emit_drop_for_binding` signatures grow `program:
&TypedProgram` to thread the lookup. Three c25 fixtures
exercise the closure: struct-with-array, nested-non-generic-
struct, generic-struct-with-array. **ADR 0018 PROPOSED**:
documents the lexical → flow-sensitive borrow-check migration
plan. Six D-decisions: D1 (trigger: empirical friction not
principle), D2 (preserved surface: BorrowError variants +
DropPlan + pipeline shape stay), D3 (adopt polonius-engine
0.13 library; vendor-fork fallback if maintenance stalls),
D4 (representation changes: CFG + origins + loans + liveness),
D5 (three-step rollout: fact generator → output lowering →
flip default), D6 (out-of-scope items: field-precise borrows
+ first-class refs + closures + traits). No migration code
ships at C2.5; the ADR records the plan only. **Known
soundness gap documented** in
`docs/borrow-check-limitations.md`: postfix `.field` on a
Move-typed binding is non-consuming per C2.3 design, so
`consume_arr(p.items)` followed by main's drop causes a
double-free. Empirically reproducible (compile clean; exits 0
on macOS; UB under stricter allocators). Highest-priority
post-C2 work on the borrow-check side. Closure: per-(VarId,
FieldPath) move state in a follow-on sub-phase (provisionally
C2.6 or ADR 0019). Workspace test delta: +4 (935 total) — +4
driver pass-test fixtures (c25_*); no new unit tests (the
codegen drop path is exercised end-to-end via fixtures, which
is the cleanest test path for LLVM-context-bearing code).
**Phase C2 closes here.** Six sub-phases shipped (C2.0.1 +
C2.0.2 + C2.1 + C2.2 + C2.3 + C2.4 + C2.5 — seven feat/docs
commits), ~6 effective sessions actual vs ADR 0017 D9
estimate "6-13 sessions across 5-6 sub-phases" — low end of
the range. Phase C3 (effect-system integration from Phase B
Sentinel-Mini per HANDOVER §6.2) opens next.

Pre-C2.5 context: **C2.4 landed; RAII / drop closes the C1.6+
heap leak.** Auto-drop at scope-exit per ADR 0017 D8 — the
long-standing leak from arrays + `?Struct` payloads since C1.6
finally closes. New `sentinel_free(ptr: *mut u8) -> void`
runtime symbol (libc free wrapper, null-guarded). Codegen
emits `sentinel_free` calls at every scope exit for heap-backed
bindings that aren't in the move-source set (the destination
of a move owns the value + drops it). The drop tracking
integrates with C2.3 via a new **DropPlan** artifact from the
borrow checker: `DropPlan { moved_sources: BTreeMap<FnId,
BTreeSet<VarId>> }` — per-fn union of all VarIds that are
sources of moves somewhere in the fn body. The borrow checker
populates this via `FnCtx.moved_sources_union` (accumulates
across branches; never reset by if/else snapshot/restore). The
salsa pipeline becomes parse → resolve → check → borrow_check
(returns Option&lt;DropPlan&gt; on success) → codegen (consumes
DropPlan). FnId / VarId gain PartialOrd + Ord derives for
BTreeMap/BTreeSet ordering. CodegenCtx gains `scope_stack:
Vec<Vec<VarId>>`, `current_fn_id`, `free_fn` (FunctionValue),
and `drop_plan: &'plan DropPlan` (introducing a second lifetime
`<'ctx, 'plan>`). Drop helpers: `emit_scope_drops(tail_returned)`
iterates the current scope's bindings in reverse declaration
order; `emit_drop_for_binding(ptr, ty)` dispatches on type —
`Type::Array` loads + extracts data ptr + calls sentinel_free;
`Type::Nullable(Struct | GenericInstance)` does the same
conditionally on the valid bit; primitives + refs + nullable-of-
primitive are no-ops. `tail_returned_var` helper recognises a
trailing `Var(id)` (the value being moved out via fn / block
return) so we don't drop it. **Known gap at C2.4 v1** (closed
at C2.5(a)): struct + generic-instance recursive field drops
were DEFERRED — `emit_drop_struct_fields` was a no-op stub. A
struct containing an array field (e.g., c16_array_in_struct's
`Bag`) leaked its inner array's heap data. Direct array
bindings + `?Struct` bindings dropped correctly. Documented
for closure during C2.5 polish — now closed.
The C2.4 phase-go program at `tests/pass/c24_go_no_go.sentinel`
(inner-block array + main-level array + move into consume)
produces stdout `160\n`, exit 0. Three additional fixtures:
c24_array_dropped (exit 24 — array dropped at fn return),
c24_moved_array_no_double_free (exit 66 — move skips drop, no
double-free), c24_nested_blocks_drop (exit 10 — inner-block
array dropped at block exit). ADR 0017 D8 now exercised at
C2.4 — D6 / D7 / D9 already covered at C2.1 / C2.2 / C2.3.
D14 (full borrow-checking phase-go) still pending at C2.5.
Workspace test delta: +8 (931 total) — +2 borrow-check unit
tests (DropPlan tracking), +2 runtime sentinel_free unit
tests, +4 driver pass-test fixtures (c24_*). **Phase C2.4
closes here.** Phase C2.5 (polish + Polonius migration plan +
ADR 0017 → ACCEPTED + STATE/HANDOVER close-out for Phase C2)
opens next.

Pre-C2.4 context: **C2.3 landed; move semantics + use-after-move.**
Move detection extends the borrow checker with per-binding move-
state tracking + branch-aware merge at if/else per ADR 0017 D9.
Compound-type bindings (struct, array, generic-instance,
nullable-of-non-Copy, TypeParam) implicitly own their data;
pass-by-value or re-binding MOVES them. Primitives (i64, i32,
bool), references (&T, &mut T), and nullables of Copy types
remain Copy and pass freely. New `BorrowError::UseAfterMove`
variant (three-label miette diagnostic: decl_span, move_span,
use_span) brings the C2.x error count to eight total. **Type
classification** via `is_copy_type(ty)`: Copy = primitives +
refs + ?Copy-inner; Move = everything else. **Consuming
contexts** (Var(x) reads that transition to Moved) are
everything EXCEPT: postfix receivers (`p.field`, `xs[i]` — read
for projection, non-consuming), lvalue operands (`& x`, `&mut
x`, LHS of `=`), and runtime-builtin call args (`len(xs)`,
`is_some(x)`, `unwrap_or(x, d)`, `print(x)` — these are inline-
lowered and semantically borrow). The runtime-builtin
non-consuming rule is the C2.3 special case that keeps
c15_maybe_compose (`is_some(x) ... unwrap_or(x, ...)`) and
c16_go_no_go (`len(a)` in cond + `a[i]` / `sum_from(a, ...)`
in branches) valid without manual rewriting — a future ADR can
promote builtins to real `&T` signatures + trait bounds.
**Branch-aware merge at if/else**: `FnCtx.moved` is snapshotted
before each branch, restored between, and merged after with
"moved in either branch → moved after". This is what makes
`fn pick(c, p) { if c { fst(p) } else { snd(p) } }` accept —
both branches independently move p; the merge declares p Moved
after the if/else; no use follows. **c17_go_no_go fixture
updated** for move semantics — the previous
`pick_int(true, p) + pick_int(false, p)` double-used p (would
fire UseAfterMove); the updated fixture constructs two distinct
Pair values (p1, p2), each moved once. Other c17 fixtures
(c17_id / c17_two_instantiations / c17_generic_nullable /
c17_generic_array / c17_box) didn't need changes — each
binding used at most once per fixture under move semantics. The
C2.3 phase-go program at `tests/pass/c23_go_no_go.sentinel`
(Account struct + transfer with branch-aware move + balance_of)
produces stdout `100\n`, exit 0. Three additional fixtures:
c23_move_struct (exit 7 — single move of Point), c23_branch_isolation
(exit 1 — if/else with both arms moving), c23_array_move (exit
15 — sum_to_end recursion over [i64] showing postfix + builtin
+ user-fn move interaction). Negative cases verified end-to-end
via snc — `consume(p) + consume(p)` surfaces
`sentinel::borrow::use_after_move` with the three-label
diagnostic + exits 1. ADR 0017 D9 is now exercised at C2.3 — D6
fully at C2.1+C2.2, D7 at C2.1, D9 at C2.3. D8 (RAII+drop closing
the C1.6+ heap-leak deferral) + D14 (full borrow-checking
phase-go combining XOR + move + drop) still pending at C2.4 +
C2.5. Workspace test delta: +16 (923 total) — +12 borrow-check
unit tests (positive: primitives-are-copy / struct-moved-once /
field-access-no-move / array-index-no-move / builtin-no-consume /
branch-both-arms-OK / two-distinct-bindings / nullable-of-copy;
negative: double-pass-by-value / rebind-then-use / array-double-
consume / use-after-move-via-let), +4 driver pass-test fixtures
(c23_*). **Phase C2.3 closes here.** Phase C2.4 (RAII / drop +
`sentinel_free` runtime symbol — closes the C1.6+ heap-leak
deferral) opens next.

Pre-C2.3 context: **C2.2 landed; `&mut T` + shared-XOR-mutable rule.**
The "interesting half" of ADR 0017 D6's borrow checker — shared-
only shipped at C2.1, and C2.2 adds the mutable side. Five new
`BorrowError` variants land covering the full XOR rule:
`MutableBorrowOfShared` (& then &mut), `SharedBorrowOfMutable`
(&mut then &), `BorrowConflict` (two &mut), `WriteWhileBorrowed`
(`x = v;` while &x or &mut x active), `ReadWhileMutBorrowed`
(reading `x` while &mut x active). The XOR invariant per ADR
0017 D6: at any point a place P is either (a) borrow-free, (b)
has N ≥ 1 shared borrows, or (c) has exactly one exclusive
borrow. Multiple shared borrows coexist freely; mixing shared
and mutable is rejected; two mutables conflict; the owner can't
mutate the place under any outstanding lend; reading the owner
while &mut active violates exclusivity. **Place-tracking**: per-
source-VarId `PlaceState { shared: Vec<BorrowInstance>,
mut_borrow: Option<BorrowInstance> }` lives in FnCtx. Each
`&x` / `&mut x` site adds a borrow with `BorrowLifetime ∈
{ Transient, UntilScope(depth) }`. **Transient borrows** die at
every statement boundary (`clear_transients()` runs in walk_stmt
after every stmt); **rooted borrows** (those promoted via
`promote_transients(depth)` at ref-typed `let r = &x;` or
equivalent assignment) live until their scope pops. The
transient model is what keeps c20_go_no_go's `add(&a, &b);` +
`increment(&mut a);` valid — the shared borrows from `add` are
transient and die before `increment`'s `&mut a` is taken.
**BorrowSource::Incoming** gained a VarId payload so place-
tracking can route conflicts through the param's place key.
`walk_assign_target` walks LHS lvalues without triggering read-
checks on Var leaves, instead firing `check_write_conflict` to
catch direct `x = v` writes under outstanding borrows.
`walk_expr_lvalue` is the dual for `&` / `&mut` operands — no
read-check on the inner lvalue. The C2.2 phase-go program at
`tests/pass/c22_go_no_go.sentinel` (block-scoped shared multi-
read followed by block-scoped exclusive write) produces stdout
`35\n`, exit 0 — a = 20 from two `&x` shared reads, b = 15
from `*r2 = *r2 + 5` increment under `&mut x`, a + b = 35.
Three additional fixtures: c22_multi_shared (exit 15 —
3-way shared), c22_scoped_mut (exit 34 — scoped mut then read),
c22_transient_then_mut (exit 16 — transient shareds + mut on
consecutive stmts). Negative cases verified end-to-end via snc
on /tmp fixtures (e.g., `let r1 = &x; let r2 = &mut x;`
surfaces `sentinel::borrow::mutable_borrow_of_shared`). ADR
0017 D6 is now **fully exercised** — both the shared-only AND
`&mut`+XOR halves shipped. D7's second-class-refs rule
continues to be enforced via the C2.1 ReturnsLocalRef check.
D8 (RAII+drop closing the C1.6+ heap-leak deferral), D9 (move
semantics + use-after-move), D14 (full borrow-checking phase-
go combining XOR + move + drop) still pending across C2.3 →
C2.5. Workspace test delta: +18 (907 total) — +14 borrow-check
unit tests (positive: multi-shared / single-mut-with-use /
mut-in-inner-scope-then-shared-outside / shared-then-mut-in-
separate-blocks / transient-borrows-die-at-stmt-end / c22-
shape / read-while-shared-ok / shared-in-block-then-mut-
outside; negative: double-mut / shared-then-mut / mut-then-
shared / write-while-shared-borrowed / write-while-mut-
borrowed / read-while-mut-borrowed), +4 driver pass-test
fixtures (c22_*). **Phase C2.2 closes here.** Phase C2.3 (move
semantics + use-after-move per ADR 0017 D9) opens next.

Pre-C2.2 context: **C2.1 landed; shared-only lexical borrow checker.**
New crate `sentinel-borrow-check` slots between check_query and
codegen in the salsa pipeline per ADR 0017 D6. The bootstrap
pipeline is now **parse_query → resolve_query → check_query →
borrow_check_query → codegen** with diagnostics accumulating
transitively. C2.1 ships the shared-only subset — `&T` only;
`&mut` + shared-XOR-mutable lands at C2.2; move semantics at
C2.3; RAII+drop closing the C1.6+ heap-leak deferral at C2.4;
Polonius migration plan at C2.5 with the ADR flipping to
ACCEPTED. **Two error variants at C2.1**: `OutlivesSource`
(canonical use-after-scope, e.g., `let r = { let inner = 5;
&inner }; *r`) and `ReturnsLocalRef` (a fn returns a `&T` whose
ultimate source is a fn-local — `let` or by-value param — both
die at return per ADR 0017 D7's "second-class refs everywhere").
The borrow-source representation is the bounded enum
`BorrowSource ∈ { Local(VarId), Incoming, LocalAnonymous }`.
Local = a binding declared in this fn (let or by-value param);
Incoming = came in via an incoming `&T` param (caller's scope);
LocalAnonymous = fallback for fn-call returns where no ref arg
contributes a source (mostly defensive at C2.1 since only user
fns return refs and they themselves must have borrow-checked).
The analysis is per-fn; inter-procedural reasoning is bounded
to "a call returning a ref inherits the most-restrictive source
among its ref args" — sufficient for the no-`&mut` subset. Inner
blocks (`{ ... }`) push/pop scopes; let-stmts record source from
RHS before declaring the binding; assign-stmts to ref-typed Vars
update the recorded source. The C2.1 phase-go program at
`tests/pass/c21_go_no_go.sentinel` (`sum_two(&a, &b) + triple(&a)
+ triple(&b)` exercising multi-ref fn calls + shared borrows of
multiple locals) produces stdout `168\n`, exit 0. Three additional
fixtures: c21_borrow_local_ok (exit 10 — `&x` used in source's
scope), c21_pass_through_ref (exit 17 — incoming ref returned),
c21_reborrow (exit 21 — `& *r` reborrow propagating source).
Negative cases verified end-to-end via snc on /tmp fixtures —
borrow errors surface as clean miette diagnostics + snc exits
with code 1, blocking codegen. ADR 0017 D6's lexical-first
formulation is now exercised (D7's second-class-refs rule
enforced via the ReturnsLocalRef check); D8 (RAII+drop), D9
(move semantics), D14 (full borrow-checking phase-go) still
pending. Workspace test delta: +19 (889 total) — +15
borrow-check unit tests (positive: no-refs / in-scope-ref /
multi-ref / pass-through / reborrow / c20-subset / inner-block-
ok; negative: return-local / return-by-value-param /
use-after-inner-scope / return-via-call-chain; salsa: succeeds-
valid / emits-on-error / propagates-type-diagnostic), +4 driver
pass-test fixtures (c21_*). **Phase C2.1 closes here.** Phase
C2.2 (`&mut T` + shared-XOR-mutable rule per ADR 0017 D6 — the
largest sub-phase per D9's table) opens next.

Pre-C2.1 context: **C2.0.2 landed; refs + mutability + deref +
assignment end-to-end.** The bundled C2 infrastructure commit per
ADR 0017 D1-D5 + D11 — references (`&T` / `&mut T`), mutable
bindings (`let mut x` + `mut x: T` params), dereference (`*expr`),
borrow-take (`&expr` / `&mut expr`), and assignment statements
(`lhs = rhs;`) all ship together in one feat commit (9516ebb).
**NO borrow checking yet** — that lands at C2.1 / C2.2 as a new
salsa query (`borrow_check_query`) slotted between check_query
and codegen per ADR 0017 D6. C2.0.2 does enforce the *static*
ref rules at type-check time: lvalue / mutability gates,
no-nested-refs, no-refs-in-arrays / struct-fields, deref-of-
non-ref. The bootstrap pipeline is still **parse_query →
resolve_query → check_query → codegen**; refs flow through as
new parallel-tree variants in each crate. Type universe at
C2.0.2 close: `{ I64, I32, Bool, Struct(StructId),
Nullable(NullableInner), Array(ArrayElem), TypeParam(TypeParamId),
GenericInstance(GenericInstanceId), Ref(RefId) }` — `Ref(RefId)`
is the new variant, indexed into a new `refs: Vec<RefData>`
interner table on `TypedProgram` (`RefData { mutable: bool,
inner: Type }`). Same C1.7.4b interner pattern as
`GenericInstance(GenericInstanceId)`, preserving the load-bearing
`Type: Copy` invariant across the fifth ADR running (0014, 0015,
0016, 0017). `NullableInner` gains a `Ref(RefId)` variant for
`?&T`; `ArrayElem` does NOT gain Ref because `[&T]` is rejected
at resolve-type-expr time with `TypeError::RefInArray` per ADR
0017 D1 (refs in array elements need named regions for
soundness, deferred). Refs in struct fields rejected with
`TypeError::RefInStructField` per ADR 0017 D7 / D12 — same
first-class-refs case. Ten new TypeError variants land at C2.0.2:
`NestedRef`, `RefInArray`, `RefInStructField`, `BorrowOfRvalue`,
`AssignToRvalue`, `AssignToImmutable`, `BorrowMutOfImmutable`,
`DerefOfNonRef`, `AssignThroughSharedRef`, `IndexAssignNotSupported`
(the last per D12's "mutable indexing deferred"). Type::substitute
extended to recurse through Ref's inner (mirrors the
GenericInstance arm — clone the data, substitute the inner,
intern the new ref). unify_one extended for `Type::Ref(p) ~
Type::Ref(a)`: mutability must match + inner types unify, which
enables generic+ref inference like `fn f<T>(x: &T)` called with
`&i64` binding `T = I64`. The mutability env (`VarTypeEnv`) is
now `HashMap<VarId, (Type, bool)>`; the second slot tracks
binding mutability for `&mut x` / `x = v;` validation. Codegen
lowers refs as LLVM opaque pointers (LLVM 15+ no-typed-pointer
era); `&x` returns the alloca pointer directly via a new
`lower_lvalue_ptr` helper that also handles `*r`-as-lvalue and
`p.field`-as-lvalue (recursive); `*r` loads from r's pointer
value of the inner type; `x = v;` / `*r = v;` / `p.x = v;`
share the lvalue-ptr machinery for the store target. The C2.0.2
phase-go program at `tests/pass/c20_go_no_go.sentinel`
(the full ADR 0017 D14 program: `add(&a, &b)` + `increment(&mut a)`
+ `let mut a` + print) produces stdout `53\n`, exit 0 — sum=42
(10+32 shared-borrow add) + inc=11 (after exclusive-borrow
deref-assign mutates a 10→11). Three additional fixtures:
c20_ref_basic (exit 42 — bare `add(&a, &b)`), c20_mut_basic
(exit 4 — `let mut + reassignment + arithmetic`), c20_deref_basic
(exit 22 — `&mut a + *x = new_val + tail-read`). ADR 0017
stays **PROPOSED** — only C2.0.2 of six sub-phases has landed;
the ADR flips ACCEPTED at C2.5 close-out. Workspace test delta:
+62 (870 total) — +8 ast (UnaryOp Ref/RefMut/Deref symbols,
TypeExprKind::Ref display, let-mut display, assign display),
+23 syntax (parser tests for `&T` / `&mut T` types + unary
`&` / `&mut` / `*` + `let mut` + `mut` param + assignment
statements), +21 types (ref interner + lvalue rules + new error
variants), +6 codegen (smoke tests for ref/mut/deref/assign
through the lowering path), +4 driver pass-tests (c20_*).
**C2.0.2 closes the C2 infrastructure phase.** Phase C2.1
(shared-only lexical borrow checker per ADR 0017 D6) opens next.

Pre-C2.0.2 context: **C1.7 landed; Phase C1 closes.** Witness-table
generics per ADR 0011 D6 / ADR 0016 — generic fns + generic
structs + codegen monomorphisation — shipped end-to-end across
five commits (e411ded ADR 0016 PROPOSED, the pre-flight design;
c1e5083 AST + parser + resolve scaffolding; d32a9fe types crate
generic-fn typing + builtin re-route; ad7e10d codegen
monomorphization for user generic fns; 2c6c652 generic structs
end-to-end with the ADR 0016 D12 phase-go). The bootstrap
pipeline is still **parse_query → resolve_query → check_query →
codegen**; C1.7 flows through as new variants on the existing
parallel trees plus a new interner table for generic-struct
instances. Type universe at C1.7 close: `{ I64, I32, Bool,
Struct(StructId), Nullable(NullableInner), Array(ArrayElem),
TypeParam(TypeParamId), GenericInstance(GenericInstanceId) }`
where `NullableInner` and `ArrayElem` each gain `TypeParam` and
`GenericInstance` variants (partially closing the ADR 0015 D6
deferral — `?Box<i64>` and `[Box<i64>]` now work; `?[T]` and
`[?T]` stay deferred). `Type` stays `Copy` — the interned-
instance trick (per ADR 0016 D6a) wraps each unique
`(struct_id, args: Vec<Type>)` pair in a `Copy` `u32` newtype,
preserving the load-bearing invariant across four ADRs (0014,
0015, and 0016 amendments). Generic builtins (`unwrap_or`,
`is_some`, `len`) lose their special-cased typing branches in
sentinel-types — their typing now routes through the unified
`check_call` inference path per ADR 0016 D8a — while keeping
their special codegen lowering per D8b (force-unwrap / pattern
matching / runtime-metadata extraction have no Sentinel-1.7
source bodies). Codegen monomorphises each `(FnId, Vec<Type>)`
call site into its own LLVM fn with a mangled name (`pick__i64`,
`make_pair__i64__bool`, etc.); each generic-struct instance gets
its own LLVM struct type (`Pair_i64_i64`). The C1.7 phase-go
program at `tests/pass/c17_go_no_go.sentinel` (the full ADR 0016
D12 program: `struct Pair<A, B>` + `make_pair / fst / snd` +
`pick_int(true, p) + pick_int(false, p)`) produces stdout `42\n`,
exit 0. Plus c17_id (`stdout "42"`), c17_two_instantiations
(`stdout "41"`), c17_generic_nullable (`stdout "100"`),
c17_generic_array (`stdout "6"`), c17_box (`stdout "42"`) — six
fixtures total. ADR 0016 flips PROPOSED → ACCEPTED (no
amendments; all twelve D-decisions exercised as drafted). ADR
0011 flips PROPOSED → ACCEPTED (D6 sub-phase budget closed;
D12 perf discipline measured: sub-second cold builds,
sub-100ms incremental rebuilds). Workspace test delta: +47 (798
total; +1 doctest unchanged at sentinel-broker) — +0 lexer (no
new tokens per ADR 0016 D5; `<` and `>` reused from C1.3
comparisons), +19 syntax parser, +7 resolve, +18 types (12 at
C1.7.4a + 6 at C1.7.4b: generic-decl typecheck, generic-instance
signature, arity mismatch, missing args, args-on-non-generic),
+6 pass-test fixtures (c17_*), +4 driver pass-tests (c17_id /
c17_two_instantiations / c17_generic_nullable / c17_generic_array
already from C1.7.5 + c17_box + c17_go_no_go from C1.7.4b).
**Phase C1 closes here.** Phase C2 (regions, references,
mutability per HANDOVER §6.3) opens next.

Pre-C1.7 context: **C1.6 landed** — arrays + heap-allocation
runtime + `len` builtin + ADR 0014 D10 unlock (recursive structs
via `?T` heap indirection) shipped end-to-end across three feat
commits (8924d38 ADR 0015 PROPOSED; 3cfd49f lexer adds `[` and
`]` tokens; 8c5bbbe bundled runtime + AST + parser + resolve +
types + codegen). The bootstrap pipeline is still **parse_query →
resolve_query → check_query → codegen**; the array surface flows
through as new parallel-tree variants. Type universe at C1.6
close: `{ I64, I32, Bool, Struct(StructId), Nullable(NullableInner),
Array(ArrayElem) }` where `NullableInner` and `ArrayElem` are
parallel flat subset enums (primitives + structs only) — `?[T]`
and `[?T]` are deferred per the C1.6 amendment of ADR 0015 D6.
**ADR 0014 D10 retired**: the C1.5 deferral closes here per ADR
0015 D11. `?Struct` codegen switches from inline `{ i1, T }` to
heap-indirect `{ i1, ptr }`; the cycle detector relaxes — only
direct struct edges contribute to cycles, nullable struct edges
break them. `struct Node { next: ?Node }` now type-checks AND
compiles AND runs. The C1.6 phase-go program at
`tests/pass/c16_go_no_go.sentinel` runs: `sum_from([1,2,3,4,5],
0)` recursive over `[i64]` with `len` + `a[i]` produces stdout
`15\n`, exit 0. Pipeline introduces the first heap allocations
in Sentinel via `sentinel_alloc` / `sentinel_panic_oob` runtime
symbols added to sentinel-runtime (libc malloc wrapper + abort
on OOB). NO `free` at C1.6 — arrays leak; resource management is
C2's region work. ADR 0015 is now ACCEPTED-WITH-AMENDMENTS (D6
flat-depth-1 subset + D11 unlock implemented). ADR 0014 D10
retires (no longer deferred). Workspace test delta: +58 (744
total) — +4 AST, +20 syntax (6 lexer + 14 parser; the lexer
count was 6 new tests for `[`/`]` brackets), +4 resolve, +14
types, +9 codegen, +7 pass-test fixtures (c16_*). Pre-C1.6
context: **C1.5 landed** — nullable types `?T` + `null` literal
+ `unwrap_or` / `is_some` builtins shipped end-to-end across three
commits (3cb1238 ADR 0014 PROPOSED; dff8642 lexer adds `null`
keyword + `?` token; 1d0adae bundled AST + parser + resolve +
types + codegen). The bootstrap pipeline is still **parse_query
→ resolve_query → check_query → codegen**; the nullable surface
flows through as new parallel-tree variants in each crate. Type
universe at C1.5 close: `{ I64, I32, Bool, Struct(StructId),
Nullable(NullableInner) }` where `NullableInner` is a flat subset
enum that keeps `Type` `Copy` (revises ADR 0014 D4's Box<Type>
choice — the subset enum makes ?(?T) structurally unrepresentable
and avoids a 20-30-site `.clone()` refactor). C1.5 ships
bidirectional checking for `?T` contexts: let-annotations push to
RHS, fn return-types push to the body tail (when ?T), call-arg
types push to each arg (when ?T), struct-literal field types push
to each field value. The `null` keyword has no synthesis type and
requires an inferable `?T` context; bare `let x = null;` surfaces
as `TypeError::AmbiguousNull`. Implicit `T → ?T` widening at
expression position per ADR 0014 D3 inserts an explicit
`TypedExprKind::WidenToNullable` wrapper that codegen lowers via
`build_insert_value` into `{ i1 true, T payload }`. Codegen
represents `?T` as the LLVM struct `{ i1 valid, T payload }`;
`unwrap_or` lowers inline as `build_select(valid, payload,
default)`; `is_some` lowers as `build_extract_value(struct, 0)`.
Equality against `null` (`==` / `!=`) compares the valid bits.
**ADR 0014 D10 deferred**: the cycle-check relaxation (recursive
structs via nullable edges) is documented but NOT implemented at
C1.5 because the `{ i1, T }` flat representation makes `struct
Node { next: ?Node }` infinite-sized in LLVM. Proper indirection
needs heap allocation; C1.6+. The C1.5 cycle detector walks
nullable struct edges as if they were direct edges (conservative).
The C1.5 phase-go program at `tests/pass/c15_go_no_go.sentinel`
runs: `find_or(some 42, 0) + find_or(null, 100)` produces stdout
`142\n`, exit 0. ADR 0014 is now ACCEPTED. Workspace test delta:
+54 (686 total) — +3 AST, +15 syntax (6 lexer + 9 parser), +4
resolve, +18 types, +8 codegen, +6 pass-test fixtures (c15_*).
Pre-C1.5 context: **C1.4 landed** — structs + field access + struct
literals shipped end-to-end across three commits (e93635b ADR
0013 PROPOSED; f34b401 lexer adds `struct` keyword + `.` token;
aa8f252 AST + parser + resolve + types + codegen for struct decl,
construction, field access). The bootstrap pipeline is still
**parse_query → resolve_query → check_query → codegen**; the
struct surface flows through as new parallel-tree variants in
each crate. Type universe at C1.4 close: `{ I64, I32, Bool,
Struct(StructId) }` per ADR 0013 D4. The codegen value type
widened from `IntValue<'ctx>` to `BasicValueEnum<'ctx>` so struct
values flow through the same machinery; struct literals lower
via `build_insert_value` chains from `get_undef()`; field access
via `build_extract_value`. Recursive structs (direct + mutual)
are detected at type-check time per ADR 0013 D7 — `struct Node
{ next: Node }` surfaces as `TypeError::RecursiveStruct` with
the full cycle path; lifts at C1.5 when `?T` arrives. The C1.4
phase-go program at `tests/pass/c14_go_no_go.sentinel` runs:
`Point { x: 3, y: 4 }` + `manhattan(p) { p.x + p.y }` produces
stdout `7\n`, exit 0. ADR 0013 is now ACCEPTED. Workspace test
delta: +68 (632 total) — +4 lexer, +6 AST, +21 parser, +7
resolve, +19 types, +6 codegen, +5 pass-test fixtures (c14_*).
Pre-C1.4 context: **C1.3 landed** — the C1 primitive surface is now
complete (bool + comparison + logical operators) across three
feat commits (2801a81 lexer adds the 11 C1.3 tokens; cd1c0d4
AST + parser + resolve + types + codegen handle bool literals,
comparisons, logicals, unary `!`, with i32 added to the type
universe; ba5fd9d retires ADR 0010 D9 C-style truthy and rewrites
the 6 if-using C0 fixtures + adds 7 new c13 pass-test fixtures).
The bootstrap pipeline is still **parse_query → resolve_query →
check_query → codegen**; the new pieces flow Type information
through the existing four-stage shape rather than adding new
stages. Type universe at C1.3 close: `{ I64, I32, Bool }`, all
three recognised by `resolve_type_expr` and all three handled
type-aware in codegen (i1 for Bool, i32 / i64 for the int
widths). Operator-typing rules per ADR 0012:
  * arithmetic `+ - * /` — both operands same int type → same
    int type (Bool rejected)
  * comparisons `== != < <= > >=` — both operands same type →
    Bool (parser-level non-associative per D6)
  * logicals `&& ||` — both operands Bool → Bool, short-circuit
    via PHI-based basic-block control flow in codegen
  * unary `-` — int → same int type
  * unary `!` — Bool → Bool (lowered as xor with i1 const 1)
  * `if cond` — cond must be Bool (ADR 0010 D9 retired)
The dormant `Mismatch` / `ReturnTypeMismatch` / `CallArgMismatch`
variants from C1.2 are now exercised by C1.3's operator + bool
flow. The C0 go/no-go program at `tests/pass/c05_go_no_go.sentinel`
was rewritten per the ADR 0012 appendix's C1.3 phase-go shape
(`is_positive(x)` returns bool, `pick(cond: bool, ...)` consumes
bool, the if-condition is bool) and still produces stdout `10\n`,
exit 0. Workspace test delta: +72 (564 total) — +9 lexer, +28
AST+parser, +18 types, +10 codegen, +7 pass-test fixtures (c13_*).
i32 is in the universe but practically thin without literal
typing (integer literals default to I64), which is a C1.5+
concern. Pre-C1.3 context: **C1.2 landed** — annotation grammar
+ sentinel-types type checker exercised end-to-end across four
feat commits (af16655 lexer `:` token; 90965a5 AST/parser/resolve
annotation grammar + 22 pass-test fixture rewrite; ded07bc
sentinel-types scaffold with Type/TypedProgram/check/check_query;
c9a21ff codegen+driver consume TypedProgram). At C1.2 the type
universe was `I64` only per ADR 0012 D4 — every annotation had
to say `i64`; Mismatch / ReturnTypeMismatch / CallArgMismatch
variants existed but were dormant at C1.2 since everything typed
to I64. ADR 0011 D5 (sentinel-types::check() real) and ADR 0012
D1-D4 (annotation grammar) became exercised at C1.2. Workspace
test delta at C1.2: +24 (492 total: +15 sentinel-types, +9 in
ast/syntax). Pre-C1.2 context: **ADR 0012 PROPOSED** — concrete
C1 surface syntax ADR landed as a docs-only commit pinning the
annotation grammar (D1-D4, for C1.2) and the bool/comparison/
logical operator set (D5-D8, for C1.3) before either sub-phase
begins. Eleven D-numbered decisions: mandatory return-type
annotations on `fn`, optional `let` annotations, primitives as
identifiers (not lexer keywords), `i64` is the C1.2 universe with
`i32` + `bool` arriving at C1.3, lexer additions (`:`, `true`,
`false`, six comparison ops, `&& || !`), non-associative
comparisons matching Rust, retirement of ADR 0010 D9 C-style
truthy at C1.3, hard break for fixture annotation rewrite per
ADR 0011 D8. Pre-ADR-0012 context: **C1.1 landed
(C1.1.1 at 438dd16, C1.1.2 at 9374edf)** — name resolution lifts
out of `sentinel-codegen` into
a populated `sentinel-resolve` crate per ADR 0011 D4. C1.1.1
scaffolds the crate: VarId/FnId/FnSignature, parallel-tree
resolved AST (ResolvedProgram/FnDef/Param/Block/Stmt/Expr),
ResolveError with the 6 variants that used to live in
CodegenError (UndefinedVariable, RedeclaredVariable,
UndefinedFunction, ArityMismatch, RedefinedFunction, MissingMain),
pure `resolve()` + `#[salsa::tracked]` `resolve_query` chaining on
parse_query. C1.1.2 rewires sentinel-codegen and sentinel-driver:
`compile_to_object` now takes `&ResolvedProgram`; codegen's vars
map is keyed by VarId, fns map by FnId; the driver pipeline
becomes parse_query → resolve_query → codegen with diagnostics
transitively accumulated across all three stages. **Phase C1.1
complete**; ADR 0011 D4 ACCEPTED. Workspace test delta: +20
(C1.1.1's resolve unit tests + salsa query smoke) -5 (codegen's
rejects-NAME tests moved to resolve, net -8 deleted + 3 new
positive codegen tests added) = net +15. 468 tests total. All
22 C0 pass-test fixtures still run end-to-end through the new
pipeline. Pre-C1.1 context: **C1.0c decision landed** — the
codegen-salsa question is resolved as "defer until C1.2+ (typed
HIR rewrite)"; ADR 0011 D1 amended with the three-option weigh-up
and rationale. The Salsa retrofit is now complete for the front-
end with codegen intentionally outside the query graph. **Phase
C1.0 is complete**. Pre-C1.0c context: **C1.0b landed at
557cc60** — the lex and parse pipeline stages
now run through `#[salsa::tracked]` queries against `SentinelDb`,
with `sentinel_base::Diagnostic`s flowing through the
`#[salsa::accumulator]` rather than rich error vectors through
tracked-struct fields (the C1.0a pause was caused by
`miette::SourceSpan` not deriving Hash; routing errors through the
accumulator side-steps the Hash bound entirely). AST types
(`Spanned<T>`, `Block`, `Param`, `FnDef`, `Program`, `StmtKind`,
`ExprKind`) gained `#[derive(Hash)]`. `sentinel-syntax::query`
exposes `lex_query` and `parse_query`; the `sentinel-driver`
binary instantiates a concrete `SentinelDatabase`, sets a
`SourceFile`, calls `parse_query`, and collects diagnostics via
`parse_query::accumulated::<Diagnostic>`. All 22 existing pass-test
fixtures still run end-to-end through the new query-based driver
path; the C0 go/no-go program at `tests/pass/c05_go_no_go.sentinel`
still produces stdout `10\n`, exit 0. Codegen is intentionally not
yet salsa-wrapped (deferred to C1.0c per HANDOVER §0.2 step 5);
LLVM context lifetimes may not fit salsa's query model cleanly and
the retrofit/driver wiring was worth landing first. ADR 0011's D1
(Salsa adoption at C1.0) is now exercised end-to-end; ADR 0011
remains PROPOSED until all of C1.0 (incl. codegen) is in. Net +7
workspace tests over C1.0a (4 lex/parse query positive + 1
diagnostic + 2 cache validation).

Phase C0 retrospective (preserved as historical context for what
came before Phase C1): the bootstrap compiler can lex, parse,
name-resolve (still in codegen), and lower fn-based programs with
let, arithmetic, if/else, blocks, and print to runnable binaries
via LLVM. The ADR 0010 appendix go/no-go program
(`double + pick + main with print`) compiles and runs at
`tests/pass/c05_go_no_go.sentinel`: stdout `10\n`, exit 0.
Programs are one-or-more `fn` definitions with an explicit `main`
entry point. Codegen is two-pass (signatures, then bodies); main
returns i32 (the C ABI shape) and other fns return i64. ADR 0009
status records Phase C0 as complete; ADR 0010 status notes all
D-decisions exercised. Workspace test count at Phase C0 close:
445 (+22 over C0.4 — 2 ast Display + 14 parser + 1 codegen net +
5 pass).

Phase C0 retrospective: six sub-phases (C0.0 lexer, C0.1 parser +
AST, C0.2 LLVM codegen + first runnable binaries, C0.3 let +
variables, C0.4 if/else + print + first stdout, C0.5 fn defs +
main) shipped across twelve commits. The compile pipeline source
-> lex -> parse -> AST -> two-pass LLVM IR -> object -> cc-linked
executable handles every C0 feature.

Phase B retrospective (preserved as historical context for what
came before C0): all three HANDOVER §5.2 validation demos landed
(supply-chain, async-as-effect, password-verify), 226 tests passing
in effects-proto. ADRs 0001-0008 ACCEPTED. See ADRs 0003 (B1
retrospective), 0004 (row representation), 0005 (effect-inference
judgment; D9 closed), 0006 (default-close, amended; D4 row
polymorphism implemented), 0007 (effect handlers; status fully
ACCEPTED, D9 fully complete — all phases B3.0 + B3.1 + B3.2 landed),
0008 (secret qualifier and constant-time check; status ACCEPTED, D1
through D7 confirmed by B4.0 + B4.1, D8 implicit via existing
free-var/free-row-var recursion, D9 amended for B4.2 landed). Phase
B is finished; Phase C began with ADR 0009 at 7a04ba1 and C0.0 at
8f37381.

---

## Section A — sentinel-broker

Production-shape crate. The broker provides generational arenas,
two allocation strategies, scoped budgets, secret memory, recording,
and diagnostics. Intended for adoption beyond the Sentinel compiler
itself per HANDOVER §4.

### A.1 Phase Tracker

| Phase | Title                                              | Status | Commit  |
|-------|----------------------------------------------------|--------|---------|
| A0    | Dev dependencies (thiserror, tracing, proptest)    | Done   | 9c7474d |
| A1    | Foundation types (ArenaId, Generation, ...)        | Done   | 9c7474d |
| A2    | Bump arena + generational handles + destroy_arena  | Done   | 9c7474d |
| A3    | Pluggable AllocStrategy (Bump + Slab), builder     | Done   | f606d19 |
| A3.5  | Per-slot generations + slab recycling              | Done   | 37ab02b |
| A4    | Scoped allocation budgets                          | Done   | 493ee7b |
| A5    | Stats, list_arenas, where_is                       | Done   | 15d751c |
| A6    | Recording mode (event log, ring buffer)            | Done   | 2e8fb8b |
| A7    | Secret-memory policy (mlock + zero-on-free)        | Done   | f3170bf |
| A8    | Validation examples / integration demos            | Done   | 683981d |
| A9    | Fallible builders + BrokerError carries OS detail  | Done   | 755e710 |

Test coverage as of A9: 69 tests (62 lib + 5 integration + 2
proptest). The count dropped from 70 → 69 between A8 and A9 because A9
incidentally removed `strategy::slab::tests::slab_free_returns_not_implemented`,
an obsolete test that survived A3.5 (slab recycling) and asserted the
*opposite* of the correct slab behavior — slab DOES support free as of A3.5.
The correctly-named `bump_free_returns_not_implemented` (which matches
invariant #3) is retained.

Doctests: 1 passing + 6 ignored. Clippy clean under `-D warnings`
across crate and examples.

### A.2 Crate Layout

    crates/sentinel-broker/
      Cargo.toml
      benches/arena_bench.rs
      src/
        lib.rs            crate root + re-exports
        arena.rs          Arena (strategy + recorder + counters)
        broker.rs         Broker, ArenaHandle, within_budget
        budget.rs         Budget, BudgetScope, BudgetArenaBuilder
        builder.rs        ArenaBuilder (bump/slab + try_bump/try_slab as of A9)
        error.rs          BrokerError enum (no longer Copy as of A9)
        handle.rs         Handle<T>, HandleRef<'a, T>
        ids.rs            ArenaId, BudgetId, SlotIndex, Generation,
                          SlotGeneration, monotonic counters
        recording.rs      Recorder, Event (A6)
        secret.rs         SecretPolicy, SecretStrategy (A7)
        stats.rs          BrokerStats, ArenaSummary, HandleLocation (A5)
        strategy/
          mod.rs          AllocStrategy trait + AllocOk/SlotPtr/StrategyKind
          bump.rs         BumpStrategy
          slab.rs         SlabStrategy (freelist + per-slot generations)
      examples/
        token_bucket.rs       high-frequency slab allocation
        request_pipeline.rs   bump-per-request under budgets, with recorder
        credential_store.rs   secret slab with STRICT/LENIENT, raw zero-on-free verification
      tests/
        integration.rs    end-to-end API tests
        proptest.rs       property-based isolation/invalidation tests

### A.3 Public API Surface

All re-exports from `sentinel_broker`:

- Core: `ArenaHandle`, `Broker`, `Arena`, `Handle`, `ArenaBuilder`
- IDs: `ArenaId`, `BudgetId`, `Generation`, `SlotGeneration`, `SlotIndex`
- Strategies: `AllocStrategy`, `StrategyKind`
- Budgets (A4): `Budget`, `BudgetScope`, `BudgetArenaBuilder`
- Stats (A5): `ArenaSummary`, `BrokerStats`, `HandleLocation`
- Recording (A6): `Event`, `Recorder`
- Secret (A7): `SecretPolicy`, `SecretStrategy`
- Errors: `BrokerError`

#### A.3.1 Broker

- `Broker::new()`
- `Broker::with_recorder(Arc<Recorder>)` (A6)
- `broker.create_arena(name, capacity) -> ArenaHandle`
- `broker.arena(name).capacity(n).bump() -> ArenaHandle` (panics on
  misuse; see also `try_bump`)
- `broker.arena(name).slab(slot_size, slot_align, slot_count) -> ArenaHandle`
- `broker.arena(name).secret(policy).bump()/.slab(...)` (A7)
- `broker.arena(name).try_bump() -> Result<ArenaHandle, BrokerError>` (A9)
- `broker.arena(name).try_slab(...) -> Result<ArenaHandle, BrokerError>` (A9)
- `broker.destroy_arena(id)` invalidates all handles
- `broker.live_arena_count()`
- `broker.stats() -> BrokerStats` (A5)
- `broker.list_arenas() -> Vec<ArenaSummary>` (A5, sorted by id)
- `broker.where_is(&handle) -> Option<HandleLocation>` (A5)
- `broker.recorder() -> Option<&Arc<Recorder>>` (A6)
- `broker.within_budget(cap, |scope| { ... })?` (A4, nestable)

#### A.3.2 Arena / ArenaHandle

- `arena.alloc(value) -> Result<Handle<T>, BrokerError>`
- `arena.free(&handle)` (slab only; bump returns `NotImplemented`)
- `handle.get() -> Result<&T, BrokerError>`
- `handle.is_live()`, `arena_id()`, `slot()`, `slot_generation()`
- `arena.__raw_slot_bytes_for_diagnostics(slot)` (A8, doc-hidden,
  forensic tooling only)

#### A.3.3 BrokerError variants

`UseAfterFree`, `UseAfterFreeSlot`, `OutOfMemory`, `InvalidSlot`,
`UnknownArena`, `BrokerPoisoned`, `BudgetExceeded`, `NotImplemented`,
`SecretMemory { reason: String, os_errno: Option<i32> }` (A9 shape),
`BuilderMisuse { reason: &'static str }` (A9).

As of A9, `BrokerError` no longer derives `Copy`. `SecretMemory`
carries the underlying OS error number where available (previously
the OS errno was only logged via `tracing::warn!`).

### A.4 Design Invariants

These are properties the test suite enforces; future changes must
preserve them.

1. Arena destruction invalidates every handle. `destroy_arena(id)`
   removes the broker's `Arc<Arena>` from its map and calls
   `Arena::invalidate()`, which advances generation atomically.
   `Handle::get()` then returns `BrokerError::UseAfterFree`.

2. Per-slot generations defeat ABA. Each slab slot has its own
   generation counter. Reusing a slot increments it, so a handle to
   the prior occupant returns `BrokerError::UseAfterFreeSlot`.

3. Bump strategy never recycles. `BumpStrategy::free` returns
   `NotImplemented`; only `SlabStrategy` supports free. Bump's whole
   point is O(1) bulk free via arena destruction.

4. Budgets pre-charge reserved capacity. `arena("a").capacity(N).bump()`
   inside `within_budget(cap, ...)` charges N to the budget chain
   BEFORE the arena exists. Reservation, not usage, is what counts.
   Nested budgets charge both inner and every ancestor.

5. Budget refunds are atomic. If `try_charge` walks the chain and
   exceeds any cap, all prior charges in that walk are refunded
   before returning `BudgetExceeded`.

6. Recording never affects behaviour. If no recorder is attached,
   the hot path is an `Option::None` branch. If recording fails
   (mutex poisoned), the event is dropped silently — recording is
   observation, not enforcement.

7. All counters use `Ordering::Relaxed`. Snapshots from `stats()`
   may show momentary inconsistency across fields under concurrent
   load. This is expected and acceptable.

8. `Broker::with_recorder` is construction-time only. No runtime
   swap, no `AtomicPtr`. The recorder is set once and read on every
   event-emitting path via `&Option<Arc<Recorder>>`.

9. (A9) Panicking and fallible builders coexist. `.bump()` and
   `.slab()` panic on misuse and are kept for tests and demos that
   want construction-time failure surfacing as a panic. `.try_bump()`
   and `.try_slab()` are exact structural twins returning `Result`.
   New code prefers the fallible variants.

### A.5 Known Limitations / Tech Debt

- `Arena::with_strategy` is `#[allow(dead_code)]` — kept as a
  convenience wrapper but only the recorder-aware variant
  `with_strategy_and_recorder` is used.
- `Recorder` uses `Mutex<Vec<Event>>`; under very high concurrent
  allocation it serializes through one mutex. Acceptable for now.
- Bounded-ring `Recorder::record` uses `Vec::remove(0)` (O(n)) on
  overflow. A `VecDeque` would be better for larger caps.
- No benchmark gate in CI. `benches/arena_bench.rs` exists but is
  not exercised on every PR.
- Doctests are ignored to avoid pulling test-only types into the
  public examples. Should be tagged `no_run` and fleshed out before
  publishing.
- (BACKLOG §0.1 remaining) Bump `slot_size_hint` is `None`; the
  `SlotInfo.size` field exists but is dead-code. Either return it
  from a bump-side override or document diagnostics as slab-only.
- (BACKLOG §0.1 remaining) `Event` variants are not `#[non_exhaustive]`.
  Consumers serializing events would see field additions as
  breaking changes.

---

## Section B — sentinel-effects-proto (Sentinel-Mini)

Research-grade tree-walking interpreter. Built to validate
Sentinel's effect-system design before committing to the Phase C
production compiler per HANDOVER §5. The crate is explicitly
expected to be thrown away or rewritten once its lessons are
absorbed.

### B.1 Phase Tracker

| Phase | Title                                              | Status | Commit  |
|-------|----------------------------------------------------|--------|---------|
| B0    | Scaffold: lex + parse + eval, no types or effects  | Done   | d090ca1 |
| B1    | HM type inference, letrec, span-tracked errors     | Done   | e6b06cd |
| B2.0  | Row scaffold: Ty::Fun carries empty Row            | Done   | 2cd81a7 |
| B2.1  | Remy-style row unification                         | Done   | 323ab33 |
| B2.2a | Effect-surface lexer/AST/parser                    | Done   | a3fd3cc |
| B2.2b | Wire Perform through pipeline as type error        | Done   | 62405f2 |
| B2.3a | Effect-inference judgment refactor (no semantics)  | Done   | fd8eef6 |
| B2.3b1 | Row mechanics (Lambda mints ρ, App via arrow_with); default-close residual rows | Done   | f2a17d9 |
| B2.3b2-a | Perform inference + UnknownEffect; eff_env from prog.effects | Done   | 4c69ed7 |
| B2.3b2-b | Row generalization in Scheme; instantiate freshens row vars; drop EffectNotYetSupported | Done   | 47cc5a1 |
| B2    | Effect rows and effect declarations                | Done   |        |
| B3.0  | Handler surface (lexer + parser + AST + placeholders) | Done   | 821b16a |
| B3.1a | row_split + HandlerLabelNotInRow                   | Done   | febf379 |
| B3.1b | Handler typing rule + DuplicateHandlerArm          | Done   | e7958e1 |
| B3.2a | Handler runtime scaffolding (Step/Continuation/Frame)| Done   | bdda217 |
| B3.2b | Handler runtime (Perform reifies, Handle dispatches) | Done   | a9cefb1 |
| B3.2c | Positive runtime coverage (4 integration tests)      | Done   | 8e3de20 |
| B3.2  | Handler runtime (operation reification + dispatch)   | Done   |         |
| B4.0a | Surface AST + SecretsNotYetSupported placeholder     | Done   | 1693b8c |
| B4.0b | Lexer Token::Secret + Token::Declassify              | Done   | 63cd57b |
| B4.0c | Parser secret prefix + declassify atom + DoubleSecret| Done   | 0b6b2ce |
| B4.0  | Secret/declassify surface (B4 phase 0 of 3)          | Done   |         |
| B4.1a | 4 TypeError variants + D2 unify + Declassify typing  | Done   | e760d57 |
| B4.1b | D3 If/Div + D4 comparisons; drop placeholder         | Done   | 52acc0a |
| B4.1  | Secret typing (unify, infer, four CT rejections)     | Done   |         |
| B4.2  | Three Phase B validation demos + README/STATE refresh| Done   | 9541969 |
| B4    | Secret T qualifier and constant-time check           | Done   |         |
| B?    | Broker-as-value-heap integration (bonus)           | Planned |        |

Test coverage as of B1: 95 tests (8 lexer + 11 parser + 11 eval +
4 span + 7 types + 41 infer + 7 diag + 6 integration). Clippy
clean under `-D warnings`. No doctests yet.

Test coverage as of B2.0: 98 tests (B1 carry-over 95 + 3 new in
`types.rs`: `b20_empty_row_renders_as_empty_string`,
`b20_arrow_with_empty_row_is_unchanged_from_b1`,
`b20_rowvar_display_uses_r_prefix`). Clippy clean under
`-D warnings`; `clippy::result_large_err` allowed crate-wide in
`lib.rs` because widening `Ty::Fun` with `Row` pushed
`TypeError::Mismatch` over the 128-byte threshold. See B.5
design decision 12.

Test coverage as of B2.1: 111 tests (B2.0 carry-over 98 + 13 new
in `infer.rs`: ~10 `b21_unify_row_*` cases covering empty rows,
var binding, matching labels, mismatched payloads, label
rewriting, occurs-check, disjoint open tails, and closed-row
failures; ~3 row display tests). Clippy clean under
`-D warnings`. See B.5 design decision 13.

Test coverage as of B2.2: 134 tests (B2.1 carry-over 111 + 23
new: 6 lexer tokens, 11 parser (effect decls, do-perform,
uppercase-label rule, paren ty-expr, missing-semicolon,
arrow-required signature, right-associative arrows, do
inside arithmetic), 3 infer (Perform rejected with
EffectNotYetSupported, span targets label, pure-body
infers), 1 eval (direct-eval defence in depth), 2
integration (pure body evaluates, do is rejected with
rendered caret). Clippy clean under `-D warnings`. See
B.5 design decisions 14 and 15.

Test coverage as of B2.3a: 134 tests (B2.2 carry-over 134 + 0
new). B2.3a is a pure behavior-identical refactor per ADR 0005
D9: the inference judgment changes from `(Subst, Ty)` to
`(Subst, Ty, Row)`, every arm returns `Row::Empty`, every
recursive `infer` call site is threaded with a new `EffectEnv`
parameter that is unused at this phase, `Scheme` gains a
`row_vars: Vec<RowVar>` field (empty by default), and `infer_top`
gains a strict residual-row check that is unreachable in B2.3a.
All 134 existing test assertions pass unchanged; only the four
`Scheme { .. }` struct-literal call sites in tests were updated
to include the new (empty) `row_vars` field. Clippy clean under
`-D warnings`. See B.5 design decision 16 for the ADR 0005 D9
divergence (`TypeError::UnhandledEffects` and the strict
`infer_top` check were pulled forward from the D9 B2.3b list).

B1 landed across five commits: spans + Spanned AST + `let rec`
(abfb3d9), types scaffold (b3589ea), inference driver wired into
`run` (24a3db8), proper HM let-rec typing (72c0996), and
hand-rolled caret diagnostics (e6b06cd).

### B.2 Crate Layout

    crates/sentinel-effects-proto/
      Cargo.toml
      src/
        lib.rs            re-exports + `run()` convenience + `MiniError`
                          (now incl. `MiniError::render(src) -> String`)
        lexer.rs          logos tokeniser, `Token` (incl. `Rec`), `LexError`
        ast.rs            `Expr = Spanned<ExprKind>`, `BinOp`, `LetRec` variant
        parser.rs         hand-written recursive descent, precedence climbing,
                          `ParseError`, span-threading
        eval.rs           tree-walking interpreter, persistent `Env`,
                          `Value`, `EvalError`, `let rec` via `OnceLock`
        span.rs           `Span { start: u32, end: u32 }`, `Spanned<T>` (B1.1)
        types.rs          `Ty`, `TyVar`, `Scheme`, free-var sets (B1.4)
        infer.rs          HM Algorithm W: `Subst`, `unify`, `instantiate`,
                          `generalize`, `TypeEnv`, `infer`, `infer_top`,
                          `TypeError` (B1.4-B1.6)
        diag.rs           `LineCol`, `locate`, `render` -- hand-rolled
                          rustc-style caret diagnostics (B1.7)
      tests/
        integration.rs    end-to-end pipeline tests

### B.3 Language Surface (B0)

Pure expression calculus with HM type inference. Everything is an
expression. No statements, no effects, no `secret` yet.
Recursion is supported via `let rec` (B1.3); types are inferred
with let-polymorphism and let-rec generalization (B1.5/B1.6).

Grammar (informal):

    expr      := let | letrec | if | lambda | compare
    let       := "let" IDENT "=" expr "in" expr
    letrec    := "let" "rec" IDENT "=" lambda "in" expr
    if        := "if" expr "then" expr "else" expr
    lambda    := "fn" "(" IDENT ")" "=>" expr
    compare   := add ( ("==" | "<" | ">") add )?
    add       := mul ( ("+" | "-") mul )*
    mul       := app ( ("*" | "/") app )*
    app       := atom ( "(" expr ")" )*
    atom      := INT | BOOL | IDENT | "(" expr ")"

Comments are `// to end of line`.

Single-parameter lambdas only. Multi-parameter functions are written
curried (`fn(x) => fn(y) => ...`).

### B.4 Public API Surface

All re-exports from `sentinel_effects_proto`:

- AST: `Expr = Spanned<ExprKind>`, `ExprKind`, `BinOp`, `expr` constructor helper
- Spans: `Span`, `Spanned<T>`
- Lexer: `Token`, `LexError`,
  `lex(source) -> Result<Vec<(Token, Span)>, LexError>`
- Parser: `ParseError`,
  `parse(&[(Token, Span)]) -> Result<Expr, ParseError>`
- Eval: `Value`, `EvalError`, `Env`, `Step`, `Continuation`,
  `eval(&Expr, &Env) -> Result<Step, EvalError>` (B3.2a return type;
  `crate::run` bridges `Step::Value` → `Value` and surfaces
  `Step::Op` as `EvalError::UnhandledOpAtTopLevel`)
- Types: `Ty`, `TyVar`, `Scheme`
- Inference: `TypeError`, `TypeEnv`, `EffectEnv`, `Subst`,
  `TyVarSupply`, `unify`, `instantiate`, `generalize`, `infer`,
  `infer_top`, `infer_program`
- Top-level: `MiniError`,
  `run(source) -> Result<Value, MiniError>` (lex+parse+infer+eval),
  `MiniError::render(&self, source) -> String` for caret diagnostics

The `diag` module is `pub mod diag` but its items are reached
through `MiniError::render`; they are not re-exported at the
crate root in B1.

### B.5 Design Decisions (B0)

1. Hand-written recursive descent over `chumsky` / `lalrpop`. Per
   HANDOVER §3.3 (production compiler) and our B0 reasoning: the
   grammar will churn as effects and qualifiers are added; hand-written
   parsers absorb grammar changes more cheaply than combinator chains.
2. Plain `Box`-allocated AST. No `bumpalo`. The language is small
   enough that heap traffic is irrelevant for a research artifact.
3. Persistent `Arc`-cons-list environment for closures. Standard
   Crafting-Interpreters shape. May be revisited if broker-as-value-heap
   integration lands.
4. Errors are not span-tracked at B0. Spans land with B1 alongside
   the type checker so error highlighting can be meaningful from
   the start.
5. `BrokerError`-style two-flavour API (panicking + fallible) is
   NOT adopted here. Effects-proto is throwaway research code;
   panicking-only is acceptable and simpler. If a panic-free API
   becomes useful for embedding, it lands then.
6. (B1.1/B1.2) AST nodes carry spans via a `Spanned<T>` wrapper,
   not an inline `span` field on each variant. Confirmed cheap;
   parser pattern is `Spanned::new(kind, start_span.merge(end_span))`.
7. (B1.3) `rec` is a reserved keyword (`Token::Rec`), not a
   contextual one. `let rec` is the only place it can appear in B1.
8. (B1.4) Substitutions are eager (`apply` on `bind`), not
   union-find. Idempotency is maintained by `compose`. Fine at
   B1 scale; revisit only if profiling demands.
9. (B1.5/B1.6) Inference is Algorithm W in the textbook shape.
   `let rec` uses the standard HM treatment: monomorphic recursive
   occurrence inside the RHS, generalized scheme in the body.
   Polymorphic recursion is therefore unavailable without
   annotations -- this is intentional and matches ML/Haskell
   without explicit type signatures.
10. (ADR 0002) Function arrows are bare `Fun(Ty, Ty)`. Effect rows
    are deferred to B2 to keep B1 focused.
11. (B1.7) Diagnostics are hand-rolled (`diag.rs`, ~110 LoC, no
    `miette` dependency). Phase C will likely adopt miette; the
    prototype validates the shape (line/col header, source-line
    excerpt, caret underline) cheaply. `Display` for `MiniError`
    stays terse; pretty rendering is opt-in via `.render(src)`.
12. (B2.0) `Ty::Fun` now carries a `Row` per ADR 0002 / ADR 0004.
    `Row` is a distinct enum (`Empty | Var(RowVar) | Cons { .. }`),
    `RowVar` is a distinct kind from `TyVar`, `Subst` carries
    parallel `map` / `row_map` fields. B2.0 ships behaviour-
    preserving: every arrow gets `Row::Empty`, `unify_row` is a
    stub handling only the empty-vs-empty case (B2.1 fills in
    Remy-style row unification). Clippy `result_large_err` allowed
    crate-wide because `Row` pushed `TypeError` past the lint's
    128-byte threshold; STATE.md decision 5 already documented
    that effects-proto does not optimise error shape.
13. (B2.1) Row unification follows Remy 1989 / Leijen's
    extensible records: `unify_row` handles empty-vs-empty,
    var binding (with `row_occurs` check), matching `Cons`
    heads by recursing on arg/ret/tail, and label rewriting
    via `rewrite_row` when heads differ. Two new
    `TypeError` variants `RowMismatch` and `RowOccursCheck`
    carry the offending row and span. `unify` now threads
    `&mut TyVarSupply` to mint fresh row tails during
    rewriting; all call sites (App, If, BinOp, LetRec) were
    updated. Unit tests use `RowVar` IDs >= 100 to avoid
    collisions with the fresh-supply counter (which starts
    at 0). Inference still mints only `Row::Empty` at
    lambda introduction; user-visible effect behaviour
    arrives in B2.3.
14. (B2.2a) Surface for effect declarations and operations.
    Five new tokens (`Colon`, `Semicolon`, `Arrow`, `Effect`,
    `Do`) make `effect` and `do` reserved keywords; neither
    was used as an identifier in B1. Grammar:
    `effect Label : TyExpr ;` where TyExpr is required to be
    an arrow at the top level, and `do Label(arg)` at the
    atom level. Effect labels must start with an uppercase
    ASCII letter (parser-enforced via
    `ParseError::EffectLabelNotUpper`). A new surface-level
    `TyExpr` enum lives in `ast.rs` deliberately distinct
    from the inference-time `Ty` so the parser does not
    depend on the type system. Fix-A grammar (single
    `TyExpr` then split-on-arrow) was chosen over
    `ArgTy '->' RetTy` because the latter is ambiguous when
    `ArgTy` itself contains `->`. ADR 0004 will be amended
    in B2.5 to reflect the actual grammar production.
15. (B2.2b) `Perform` is parseable but rejected by inference
    with `TypeError::EffectNotYetSupported { label, span }`
    where span targets the label identifier (not the `do`
    keyword) so diagnostics caret the meaningful token.
    `EvalError::EffectNotYetSupported(String)` exists for
    defence in depth (callers bypassing inference) and is
    span-less to match the B1 `EvalError` precedent
    (decision 5 lineage; full span enrichment is a backlog
    item). `run()` now pipelines through `parse_program` +
    `infer_program`; effect declarations are parsed but
    inert (typing environment is unchanged). Real effect
    rows wire in B2.3.

16. (B2.3a, ADR 0005 D9) Effect-inference judgment refactored
    behavior-identical: `infer` returns `(Subst, Ty, Row)` and
    takes a new `eff_env: &EffectEnv` parameter; `Scheme` gains
    a `row_vars: Vec<RowVar>` field (empty default, source-
    compatible with `Scheme::mono`); every arm returns
    `Row::Empty`; `Perform` keeps its B2.2b `EffectNotYetSupported`
    behavior. Divergence from ADR 0005 D9 B2.3a list, deliberate:
    `TypeError::UnhandledEffects { row, span }` and the strict
    residual-row check in `infer_top` were pulled forward from
    the D9 B2.3b list. Both are unreachable in B2.3a (every arm
    returns `Row::Empty`, so `s.apply_row(&Row::Empty)` is always
    `Row::Empty`) and land here to isolate B2.3b's diff to
    semantics only -- this strengthens the bisection-point property
    ADR 0005 D9 Consequences sec.1 calls for. `UnknownEffect` is
    *not* pulled forward; it remains strictly B2.3b because it
    would be genuinely dead (never constructed) in B2.3a. Commit
    fd8eef6.

17. (B2.3b1, ADR 0005 D2 + ADR 0006 amended) Row mechanics landed
    behavior-extending: `Lambda` mints fresh `ρ` and embeds it in
    the arrow via `Ty::arrow_with`; the lambda's own row
    contribution stays `Row::Empty`. `App` mints `ρ_call`, unifies
    callee against `arrow_with(t_arg, ρ_call, result)`, and unions
    `r_callee`, `r_arg`, and resolved `ρ_call` into its
    contribution. `Let`, `LetRec`, `If`, `BinOp` union
    subexpression contributions. `Perform` still rejects with
    `EffectNotYetSupported` (B2.3b2 wires it). Default-close (ADR
    0006 D1) extended: `Row::Var` in *contributions* is treated as
    `Row::Empty` (in `row_union` and at `infer_top`'s residual
    check), because `App` legitimately produces free `ρ_call`s
    whenever the callee is a `Ty::Var` (recursive calls,
    higher-order parameters). Net +7 tests (one earlier B2.3b1
    test with a wrong premise was deleted). Commit f2a17d9.

18. (B2.3b2, ADR 0005 D9 closed) Perform inference + row
    generalization landed as a two-commit split. **b2.3b2-a**
    (4c69ed7) wires `do Label(arg)` through the effect environment:
    `infer_program` populates `EffectEnv` from `prog.effects`; the
    `Perform` arm looks up the label, infers and unifies the arg
    against the declared arg type, and contributes a closed
    single-label `Cons` row union'd with the arg's row. Unknown
    labels surface as a new `TypeError::UnknownEffect { label,
    span }` targeting the label token. **b2.3b2-b** (47cc5a1)
    extends `generalize` to quantify free row variables in the
    type (minus those free in the env) into `Scheme.row_vars`;
    `instantiate` freshens them via `fresh_row_var`. Row
    *contributions* are intentionally excluded from generalization
    — they describe the RHS's latent effect, not its binding
    scheme; conflating them would make every let-binding look
    effectful from the outside, breaking the default-close
    presentation contract (ADR 0006 D3). The placeholder
    `TypeError::EffectNotYetSupported` is removed entirely now
    that `Perform` is fully typed; `EvalError::EffectNotYetSupported`
    stays until B3 handlers land. Split rationale: semantics
    first, generalization second, each independently bisectable
    and each under ~300 LOC. Net +14 tests (10 b23b2_* unit + 5
    b23b2b_* unit + 2 integration − 2 obsolete b22b_perform_*
    rewritten − 1 integration test repurposed).


19. (B3.0, ADR 0007 D1/D2) Handler surface landed: `handle e with
    { L(x, k) => body, ..., return v => ret_body }` with arms
    comma-separated, trailing comma permitted, empty `{}` rejected,
    arm labels required to be uppercase (mirroring `do Label`).
    `handle` parses at `parse_expr` precedence (peer of `if`, `let`,
    `fn`). `return` becomes a globally reserved keyword — no
    existing test or code used `return` as an identifier, but the
    reservation is real and future surface work should account for
    it. AST: `ExprKind::Handle { body: Box<Expr>, arms:
    Vec<HandlerArm>, ret_arm: Option<ReturnArm> }`, with `HandlerArm`
    carrying `label`, `label_span`, `arg`, `kont`, `body`, `span` and
    `ReturnArm` carrying `var`, `body`, `span`. The return arm is
    optional; absence semantically defaults to `return v => v`, with
    the default introduced by the typer (D1 in the ADR), not the
    parser, so synthesized AST nodes with fake spans never enter the
    diagnostic path. New `ParseError` variants
    `HandlerArmLabelNotUpper` and `EmptyHandler`. Placeholder
    inference and eval errors (`TypeError::HandlersNotYetSupported`
    and `EvalError::HandlersNotYetSupported`) keep the build whole
    until B3.1 (typing) and B3.2 (runtime) replace them. Commit
    821b16a.

20. (B3.1, ADR 0007 D3/D4) Handler typing rule landed in two commits.
    **B3.1a** (febf379) adds `row_split` as the dual of the existing
    `rewrite_row`: given a row and a label, returns the discovered
    `(arg_ty, ret_ty)` signature and the residual row, with four
    cases per ADR 0007 D4 (head match, deeper match, row-var case
    minting fresh `α`/`β`/`tail` and binding the var, empty-row
    erroring with the new `HandlerLabelNotInRow`). Critically the
    row-var case mints a fresh *type* var for both arg and ret —
    this is what makes handlers compose with row-polymorphic
    callers, because the handler arm's typing of `x` and `k` later
    pins those fresh vars to concrete types. `rewrite_row` was not
    reused: it expects a known signature to unify against;
    `row_split` reads the signature out. Distinct errors, distinct
    intent, easier to diagnose. **B3.1b** (e7958e1) wires the
    typing rule per D3: infer body, reject duplicate arm labels with
    `DuplicateHandlerArm`, peel each arm's label out of the body's
    row threading substitution, capture the post-peel residual as
    `r_outer_initial`, mint `t_result`, type each arm body under
    `env + {x: arg_ty, k: ret_ty -> t_result ! r_outer_initial}` and
    unify with `t_result`, union each arm body's own row
    contribution into `r_accumulated`, handle the return arm
    (or default to `t_body = t_result` when absent). The
    continuation arrow uses `r_outer_initial` rather than the
    final `r_accumulated` — the standard Eff/Koka calculus, sound
    because `k`'s declared row is what `k` requires of its
    invocation context, not what the arm body around the `k`
    invocation may additionally perform. `TypeError::HandlersNotYetSupported`
    removed (variant + `lib.rs` span arm). Three planned items
    landed as no-ops: (a) the D7 canary collapses to
    `b31b_handle_identity_discharges_effect` because
    `infer_program`'s default-close policy (ADR 0006 D6) hides
    observable row polymorphism at the public type level — the
    canary's load-bearing property (generalize/instantiate compose
    with handler typing) is satisfied transitively by all `b31b_*`
    tests passing without modifying that machinery; (b) the D8
    positive/negative pair is unreachable through `infer_program`
    (permissive) and uncallable through `infer_top` (no effect
    env), so strictness coverage comes indirectly through
    `b31b_handle_two_arms_discharges_both` and
    `pipeline_handle_typechecks_then_runtime_is_placeholder`; (c)
    extending `TypeError::UnhandledEffects` was reviewed and
    rejected — `Row`'s `Display` already renders residuals as
    `{Label1, Label2 | tail}`, human-readable as-is. ADR 0007
    status flipped to ACCEPTED for B3.0 + B3.1; D9 amended with
    completion-marker status. Net +20 lib tests (4 b31_row_split_*
    + 7 b31b_* + 9 b30_ from B3.0) + 1 integration test rewritten
    (`pipeline_handle_typechecks_then_runtime_is_placeholder`).
    Eval-side placeholders remain pending B3.2.
Test coverage as of B3.2: 183 tests (169 lib + 14 integration). Lib
count unchanged from B3.1 because B3.2a was scaffolding (no behavior
change) and B3.2b rewrote three placeholder-asserting tests in place
rather than adding new ones (one direct-eval Perform test:
`b22b_eval_perform_directly_returns_effect_error` →
`b32b_eval_perform_directly_reifies_step_op`; one direct-eval Handle
test: `b30_eval_handle_directly_returns_handlers_not_yet_supported` →
`b32b_eval_handle_no_arms_no_ret_is_identity_on_value`; one
integration test: `pipeline_handle_typechecks_then_runtime_is_placeholder`
→ `pipeline_handle_resumes_with_arg_through_run`, the first Sentinel
program with effects to actually compute a value through `run()`).
B3.2c added four integration tests covering two-effect dispatch,
non-trivial arm body work, arm-body reading outer Let bindings via
the env bundled into `Value::Resumption`, and explicit return-arm
post-resume. Clippy clean under `-D warnings`; no doctests. See B.5
design decision 21.

21. (B3.2, ADR 0007 D5) Handler runtime landed in three commits.
    **B3.2a** (bdda217) is scaffolding: `eval`'s return type changed
    from `Result<Value, EvalError>` to `Result<Step, EvalError>` where
    `Step = Value(Value) | Op { label, arg, kont }`; every existing
    arm threaded `Step::Value` propagation (Int/Bool/Var/Lambda wrap;
    Let/LetRec/If/App/BinOp match on the recursive eval result,
    propagating Value on the happy path and pushing a `Frame` onto
    kont + re-raising Op otherwise). `Continuation` is a Vec<Frame>
    enum, not a boxed `FnOnce` — chosen for Debug/Clone ergonomics
    and for keeping the resume-point states explicit and greppable.
    Eight Frame variants enumerated by counting resume-points in the
    existing eval arms (LetBody, LetRecBody, IfBranch, AppArg,
    AppCall, BinOpRight, BinOpApply, PerformReify). `Continuation::resume`
    declared as `todo!("B3.2b")` so the API signature was fixed.
    Perform and Handle arms kept their B2.2b/B3.0 placeholder errors
    unchanged; tests stayed at 169 + 10 + 0. Two new EvalError
    variants added: `UnhandledOpAtTopLevel { label }` for
    defence-in-depth at `crate::run` (mirroring B2.2b's posture —
    the type system's default-close at `infer_program` should
    prevent open rows at the top level, but the runtime checks
    anyway), and `ContinuationAlreadyResumed` reserved for B3.2b's
    one-shot enforcement. `Step` re-exported from the crate root.

    **B3.2b** (a9cefb1) is the substantive commit. Five sub-patches
    in the working tree, single commit at the end: (1)
    `Value::Resumption { kont, arms, ret_arm, env }` variant added
    + `apply` refactored to dispatch on it (initially without the
    deep re-wrap, filled in next); (2) `Frame::HandleFwd { arms,
    ret_arm, env }` variant added — the 9th Frame, not visible in
    B3.2a because Handle didn't propagate before — and `handle_step`
    helper added as the shared dispatch hub used both by the Handle
    arm at evaluation start and by `apply` at resume re-wrap; (3)
    `Perform` arm now reifies — eval the arg, on Value produce
    `Step::Op { label, arg: v, kont: Continuation::empty() }`, on Op
    push `Frame::PerformReify` and re-raise; (4) `Handle` arm now
    dispatches — eager-Arc the arms and ret_arm once per Handle
    evaluation (so subsequent Resumption/HandleFwd clones are cheap
    refcount bumps), eval the body, route the resulting Step
    through `handle_step`; (5) cleanup —
    `EvalError::EffectNotYetSupported` and `EvalError::HandlersNotYetSupported`
    deleted now that their producing arms gained real
    implementations, `#[allow(dead_code)]` stripped from Frame.
    Three placeholder tests were rewritten in place to assert real
    behavior. The bundled-context shape of `Value::Resumption`
    (kont + arms + ret_arm + env) was chosen over a bare
    `Value::Continuation` because the deep re-wrap data must travel
    with the resumption value — `apply`'s dispatch stays in one
    place (sibling to `Value::Closure`) without an App-site lookup
    into handler state, and the alternative (synthesising a
    Lambda AST node that calls a builtin) requires a builtin-
    function mechanism the prototype does not have.

    **B3.2c** (8e3de20) adds four pipeline tests through `run()`
    along axes the B3.2b integration test did not cover:
    `pipeline_two_effect_handler_discharges_both` exercises deep
    re-wrap (apply's `handle_step` after `kont.resume`) and the
    splice mechanism (step_frame's `BinOpRight` branch producing a
    nested Op with `BinOpApply` prepended onto its inner kont);
    `pipeline_arm_body_computes_with_op_arg_before_resume` covers
    the rhs-raises BinOp path (sibling to the previous test's
    lhs-raises path) and confirms arm bodies can do non-trivial
    work with the operation's argument before invoking k;
    `pipeline_arm_body_uses_outer_let_binding` confirms the arm
    body sees outer Let bindings via the env bundled into
    `Value::Resumption` (the closest the current language reaches
    toward a state handler without CPS state-threading);
    `pipeline_return_arm_runs_on_resumed_value` exercises
    `handle_step`'s Some-ret_arm branch (the B3.2b integration test
    had no ret_arm, hitting only the None/identity branch).

    Two structural decisions worth recording inline. (a)
    `Continuation` derives `Clone` because `Value` is `Clone` and
    `Value::Resumption` holds `Continuation` by value. The
    `Cell<bool>` resumed-flag copies its bool on clone, so cloning
    a *resumed* `Continuation` produces a clone that also refuses
    to resume — the one-shot guarantee therefore holds
    per-Continuation-instance, not per-logical-resumption. Nothing
    in eval clones a Resumption today; user-level multi-shot via
    `Value::Clone` is not statically prevented. Stricter alternatives
    (move-only `Resumption` variant, or `Rc<Cell<bool>>` for a
    shared flag) are recorded inline at the `Continuation`
    definition for Sentinel proper. (b) A real CPS-state state
    handler in the Eff/Koka sense — Get/Put paired effects threading
    mutable state through resumption — was scoped *out* of B3.2c
    in favor of the simpler `pipeline_arm_body_uses_outer_let_binding`
    test. The prototype's unary lambdas preclude `k(state, value)`,
    and CPS-state via `state -> result` returns produces a test
    program ~6 lines long with a trace that obscures what is being
    tested. The load-bearing mechanic (outer Let visible inside arm
    via bundled env) is covered. Deferred to whatever phase
    introduces multi-arg functions or records. Filed in ADR 0007's
    "Considered and rejected" section.

    Net 0 lib tests, +4 integration tests, −1 lib test renamed,
    −2 lib test rewrites in place, −1 integration test rewrite in
    place. Placeholder-error variants gone:
    `EvalError::EffectNotYetSupported`,
    `EvalError::HandlersNotYetSupported`. New variants:
    `EvalError::UnhandledOpAtTopLevel`,
    `EvalError::ContinuationAlreadyResumed`. New public types: `Step`,
    `Continuation` (with `is_empty()` for tests).

Test coverage as of B4.0: 209 tests (192 lib + 17 integration). Net
+23 lib (+5 b40_ in types.rs from B4.0a, +3 b40a_ placeholder tests
in infer.rs, +6 b40b_ lexer tests, +9 b40c_ parser tests) and +3
integration (declassify rejected at infer, effect-decl-with-secret
rejected at infer, double-secret rejected at parse). Clippy under
`-D warnings` is broken by 5 pre-existing lints (Arc-not-Send/Sync,
extend-vs-append, into_iter-in-IntoIterator-context, unnecessary
map_or) that pre-date B4 and were inherited from B3.2; not a B4
regression. cargo test remains the project's green-gate. See B.5
design decision 22.

22. (B4.0, ADR 0008 D9) Secret/declassify surface landed in three
    commits. **B4.0a** (1693b8c) is the surface AST + placeholder.
    `Ty::Secret(Box<Ty>)` chosen over (a) qualifier-field-on-every-Ty
    and (c) parallel qualifier lattice per ADR 0008 D1: shape (b)
    falls out of HM unification with zero new machinery, while the
    no-α-leak unification restriction (D2) substitutes for full
    qualifier polymorphism. Idempotent smart constructor
    `Ty::secret` collapses `Secret(Secret(_))` so substitution and
    unification call sites don't worry about flattening; the parser
    separately rejects literal `secret secret T` (B4.0c) so the
    surface complains early but the inference layer is robust
    regardless. Display per D6 with arrow-parens (`secret int` vs
    `secret (a -> b)`). `Subst::apply` recurses via `Ty::secret`;
    `unify` adds a structural `(Secret, Secret)` arm. The B4.0 Var
    arm in `unify` will happily bind a TyVar to a `Ty::Secret(_)` —
    D2's no-α-leak rule is B4.1 — but this is unobservable in B4.0
    because every entry point that introduces `Ty::Secret` into
    inference is gated: `infer`'s `Declassify` arm returns
    `SecretsNotYetSupported`, and `infer_program` walks each effect
    decl's signature with a new `tyexpr_find_secret_span` helper,
    rejecting any decl that mentions `secret`. `eval` gets a
    `Declassify` arm that delegates to `inner.eval` (the
    declassification is type-level only; `Value` is qualifier-blind
    by B0 design, so no resume-point work is needed) — unreachable
    in B4.0 via the full pipeline because inference rejects first,
    but exists so eval is total over `ExprKind`. 3 placeholder tests
    in infer.rs build AST directly because the lexer/parser do not
    yet recognise the keywords; 5 `b40_*` tests in types.rs (added
    in the prior session, landed in this commit) pin display,
    idempotency, free-var recursion, close_rows recursion.

    **B4.0b** (63cd57b) adds `Token::Secret` and `Token::Declassify`
    to the lexer. Both globally reserved — cannot be used as
    identifiers, matching the policy applied to `rec` (B1.3) and the
    handler keywords (B3.0). Logos token attributes only; the
    existing Ident regex is tried after keyword matches. 6 lexer
    tests (standalone for each keyword, reserved-status in let-bind
    position for each, `secret Bytes` and `declassify(x)` full
    streams).

    **B4.0c** (0b6b2ce) adds the parser surface. `secret T` as a
    prefix on type atoms in `parse_ty_atom`, binding tighter than
    `->` per ADR 0008 D6: `secret Int -> Bool` parses as
    `(secret Int) -> Bool`, not `secret (Int -> Bool)`. This
    precedence falls out of recursing on `parse_ty_atom` (not
    `parse_ty_expr`); users wanting the arrow inside the secret
    write `secret (Int -> Bool)`. `declassify(e)` as
    atom-precedence expression in `parse_atom`, mandatory parens
    paralleling `do Label(arg)` and preserving the audit-point
    property called out in D5. `ParseError::DoubleSecret` rejects
    literal `secret secret T` (caught at the immediately-recursive
    call site) and `secret (secret T)` (caught after the paren
    collapse returns `TyExpr::Secret(_, span_with_parens)`). 9
    parser tests + 3 integration tests through `run()`. The
    integration trio confirms: full-pipeline `declassify(1)`
    rejected at inference with `SecretsNotYetSupported`;
    full-pipeline `effect ReadKey : Int -> secret Int ; 0` likewise;
    full-pipeline `effect F : Int -> secret secret Int ; 0` rejected
    at parse with `DoubleSecret` (proves the early parser rejection
    short-circuits ahead of the placeholder).

    Three structural decisions worth recording inline. (a) `secret`
    binds tighter than `->`. Considered the inverse (arrow tighter,
    so `secret Int -> Bool` parses as `secret (Int -> Bool)`),
    rejected because the former matches Rust's `&mut T -> U` reading
    that ADR 0008 D6 cites as precedent, and because `secret`
    binding loosely would force users writing single-arrow effect
    signatures `Int -> secret Bytes` to add redundant parens around
    the `secret Bytes` ret type. (b) `tyexpr_find_secret_span` walks
    the surface `TyExpr` rather than the lowered `Ty` because the
    surface tree carries human-source spans and is the right layer
    to point a diagnostic at. The walker is recursive structural —
    no row-handling needed because effect-decl signatures are pure
    `Int`/`Bool`/`Arrow`/`Secret` at this scope (no inline
    polymorphism in the surface yet). (c) Both `secret` in effect
    decls and `declassify` in expressions block at inference, not
    earlier. The parser produces well-formed AST for both surfaces;
    rejection happens at the inference layer specifically so that
    `infer_program` is the one boundary that decides "we're not
    ready to type secrets" — when B4.1 lands, the rejection sites
    delete and the typing rules replace them, no parser or lexer
    churn.

    Net +23 lib tests, +3 integration tests, no rewrites in place
    (the surface is wholly new). New `TypeError` variant:
    `SecretsNotYetSupported`. New `ParseError` variant:
    `DoubleSecret`. New `Token` variants: `Secret`, `Declassify`.
    New `Ty` variant: `Secret(Box<Ty>)` + `Ty::secret` smart
    constructor. New `ExprKind` variant: `Declassify { inner, span }`.
    New `TyExpr` variant: `Secret(Box<TyExpr>, Span)`. ADR 0008
    status flipped from PROPOSED to ACCEPTED with note that D1/D5/D6
    are confirmed by B4.0 and D2/D3/D4/D8 land in B4.1.

Test coverage as of B4.1: 222 tests (203 lib + 19 integration).
Net +13 lib / +2 integration from B4.0's 192 + 17. B4.1a landed
+5 lib (Declassify-on-non-secret-is-SecretFlow rename of the
B4.0a placeholder test, plus +5 fresh tests covering D2 direct,
Declassify positive via synthetic env, SecretFlow via catch-all,
Secret-Secret recursion sanity, Secret-Int-vs-Secret-Bool mismatch
on inners), 0 net integration (rename in place of the declassify
placeholder). B4.1b landed +8 lib (effect-decl-with-secret tests
rewritten from rejection to positive [+2 in place], +6 fresh
covering SecretBranch, SecretDivisor, D4 on three comparison shapes,
D4-Lt-Bool-rejects-on-inner) and +2 integration (password-verify
chain rejects with SecretBranch -- the HANDOVER §5.2 deliverable;
secret-in-arithmetic-is-SecretFlow). The effect-decl-rejection
integration test was rewritten as a positive type-check (net 0).
The placeholder variant `TypeError::SecretsNotYetSupported` is
gone. Clippy under `-D warnings` still has the 5 pre-existing
lints inherited from B3.2; chip filed to clean up separately.
See B.5 design decision 23.

23. (B4.1, ADR 0008 D2-D7) Secret typing landed in two commits.
    **B4.1a** (e760d57) is the foundation. Four new `TypeError`
    variants: `SecretFlow { from, to, span }` (the public/secret
    unification failure, raised by the catch-all arm of `unify`
    when either side is `Ty::Secret(_)` and the other is
    non-secret-non-Var), `SecretEscapesPolymorphism { var, span }`
    (D2 no-α-leak, raised by the Var arm of `unify` when a bare
    TyVar would bind to a secret type), `SecretBranch { span }`
    (declared; fires in B4.1b), `SecretDivisor { span }` (declared;
    fires in B4.1b). The `unify` Var arm gains a `matches!(t,
    Ty::Secret(_))` short-circuit; the catch-all Mismatch arm
    splits into SecretFlow (one side Secret) and Mismatch (neither
    side Secret). The Declassify infer arm replaces its B4.0a
    `SecretsNotYetSupported` placeholder with the real D5 rule:
    mint a fresh α, unify `t_inner` against `Ty::Secret(α)`, return
    `s.apply(α)` as the result type. Three failure cases handled
    naturally by the unify machinery: inner is concrete non-secret
    → SecretFlow via catch-all; inner is bare TyVar →
    SecretEscapesPolymorphism via Var arm; inner is Secret(t) →
    Secret-Secret arm binds α := t, result is t.

    **B4.1b** (52acc0a) is the CT-specific rejections + cleanup.
    `infer`'s If arm rejects `cond : Ty::Secret(_)` with
    SecretBranch before the Bool unify so the diagnostic is
    dedicated rather than the generic SecretFlow. `infer`'s BinOp
    arm rejects Div with a secret divisor with SecretDivisor
    (Sentinel-Mini has no Mod; ADR D3's `Div | Mod` is
    forward-looking). The Eq/Lt/Gt arms gain the D4 comparison
    rule: when either operand types as Secret(_), unwrap that side
    to its inner type, unify the other operand against the inner
    (the (Secret, Secret) arm of unify handles the both-secret case
    naturally via the same code path because both inners become
    non-Secret here), and produce Secret(Bool) as the result type.
    For Lt/Gt the inner type must additionally be Int per the
    existing binop_signature; that unify fires after the cross-side
    unify and produces a SecretFlow if the inner isn't Int (or
    Mismatch if neither side was secret).

    The `infer_program` `tyexpr_find_secret_span` walker and the
    associated SecretsNotYetSupported variant are deleted in B4.1b.
    ADR 0008 D7 (effect signatures may mention `secret`) is
    confirmed end-to-end: `effect ReadKey : Int -> secret Int ;`
    now type-checks, and `do ReadKey(0)` flows a Secret(Int) value
    into inference where D2/D3/D4 keep it safe.

    Two structural decisions worth recording inline. (a) The Var
    arm's D2 check is `matches!(t, Ty::Secret(_))` rather than a
    deeper walk that would refuse to bind any TyVar that contains a
    Secret inside a structural type (like `Fun(_, _, Secret(_))`).
    The shallow check is sufficient because Secret-inside-Fun is
    not a "bare TyVar binds to secret" violation -- the inner
    binding is fine if the function itself is concrete. Forward
    compatibility note: if a future ADR promotes the restriction
    to full qualifier polymorphism (shape (c)), the shallow check
    becomes a quantifier-restricted bind rule; the existing call
    sites already test it at the Var arm so the migration is
    contained.

    (b) The D4 comparison rule for Lt/Gt unifies the (potentially
    secret-unwrapped) lhs inner type against Int as a SECOND
    unification step, after the cross-side unify. This makes
    `Lt(Secret(Bool), Secret(Bool))` reject with Mismatch (the
    unwrapped Bool vs Int) rather than SecretFlow (which would be
    odd because both sides are secret-and-equal). The trade-off:
    diagnostic for Lt-on-secret-non-Int is "expected Int, found
    Bool" rather than something CT-specific. Acceptable because
    such a program is rare and the inner-type mismatch is what the
    user actually needs to fix.

    A positive end-to-end test for declassify-on-Secret cannot be
    written because ADR 0008 D5 intentionally omits a `classify`
    primitive (the Secret-introduction dual). The Secret-introducing
    path is restricted to typing of `do L(arg)` for effect-decls
    naming secret; resuming a continuation requires producing a
    Secret value, and the surface has no form that does so. Positive
    D5 coverage lives in lib's
    `b41a_declassify_on_secret_unwraps_the_inner_type` via a
    synthetic env. The integration tests file carries an inline note
    explaining the gap.

    D8 (generalization participation) was implicit by B4.0a's
    structural recursion of `collect_free_vars` and
    `collect_free_row_vars` into `Ty::Secret(_)`. No dedicated test
    added because no surface program exposes the generalization
    boundary with a polymorphic secret-typed value -- D2 prevents
    that by construction. Confirmed by code review of types.rs.

    Net +13 lib / +2 integration. New TypeError variants land:
    SecretFlow, SecretEscapesPolymorphism, SecretBranch,
    SecretDivisor. Variant removed: SecretsNotYetSupported.

Test coverage as of B4.2: 226 tests (203 lib + 23 integration).
B4.2 lands one commit covering the three HANDOVER §5.2 Phase B
validation demos plus README/STATE refresh. Net +4 integration
(supply-chain handler-mismatch, async-as-effect doubling-handler,
async-as-effect identity-handler, polished password-verify with the
CT-chain rationale block); 0 lib changes. The polished
password-verify is added next to the terse
`pipeline_password_verify_naive_rejects_with_secret_branch` rather
than replacing it -- the terse form is useful as a
regression-pin; the polished form is what HANDOVER §5.2 actually
calls for as the Phase B deliverable. Clippy clean under
`-D warnings`; no doctests. See B.5 design decision 24.

24. (B4.2, ADR 0008 D9 + HANDOVER §5.2) Phase B's three validation
    demos all live as integration tests under the `pipeline_b42_`
    prefix in `crates/sentinel-effects-proto/tests/integration.rs`.
    Implementation choices:

    (a) Tests, not example files. The crate has no `examples/`
    directory and remains library-only. Examples would have
    required `crate::run` and `Value` exposed as `cargo run`
    targets; the test runner already exposes both via the
    pipeline path. The trade-off accepted: examples are
    discoverable via `cargo run --example`, tests are
    discoverable via `cargo test pipeline_b42_`. The latter is
    sufficient given the prototype's "throwaway research artifact"
    framing in HANDOVER §5.

    (b) Supply-chain demo asserts `HandlerLabelNotInRow{label:
    "Storage"}`. The error diagnostic is from the handler's
    perspective ("I said I'd handle Storage but the body never
    raised it") rather than the user's ("the body raised Network
    without my permission"). Both readings are correct; the row
    machinery refuses the program either way. ADR 0007's row
    discipline doesn't carry user-intent metadata, so the
    diagnostic frame is mechanical. A comment in the test
    explains the supply-chain framing.

    (c) Async-as-effect demo is a pair of tests with the same
    `let app = fn(n) => do Tick(n) + 1 in ...` prefix; only the
    handler arm differs (`Tick(x, k) => k(x * 2)` vs
    `Tick(x, k) => k(x)`). The byte-identical-source assertion is
    pinned via test pairing, not via shared-string-constant
    machinery (which would require helper indirection and
    obscure the demo). The two tests assert different `Value::Int`
    results, which is the load-bearing observation.

    (d) Polished password-verify demo carries the full CT-chain
    rationale as a comment block above the test: the D7 effect
    signature, the D4 comparison rule producing `Secret<Bool>`,
    the D3 `SecretBranch` rejection of the surrounding `if`. The
    naive comment in the original B4.1b test is intentionally
    short; the polished version is the "publishable" form of the
    demo.

    (e) No featured `classify : T -> secret T` primitive added.
    The B4.2 task offered it as a design call (it would enable a
    positive end-to-end declassify test); rejected for B4.2 because
    Secret-introduction is exactly what ADR 0008 D5 deliberately
    omits to preserve the audit-point property. Adding `classify`
    would require its own ADR amendment justifying the exception
    and a strong-comment audit marker. Deferred indefinitely.

    Net +4 integration tests, 0 lib changes (no new variants, no
    surface changes; the demos are exercise programs over the
    existing surface). README updated to mark B1-B4 done, refresh
    the test count, and add a "What works today" paragraph for
    effects-proto with the `cargo test pipeline_b42_` invocation
    that runs the three demos. ADR 0008 D9 amendment updated to
    flip B4.2 from roadmap to landed. Phase B is now complete;
    Phase C (bootstrap compiler) per HANDOVER §6 is the next major
    phase.


### B.6 Known Limitations (intentional at B1)

- No effects. The whole reason this crate exists. (B2 onward — done.)
- `secret` qualifier and constant-time check: complete (B4.0 surface,
  B4.1 typing, B4.2 validation demos). D2 (no-α-leak), D3 (the four
  CT rejections), D4 (comparisons produce `Secret<Bool>`), D5
  (declassify typing) all wired. All three HANDOVER §5.2 Phase B
  validation deliverables exist as integration tests under the
  `pipeline_b42_` prefix. A positive end-to-end `declassify` test is
  intentionally absent because ADR 0008 D5 omits a `classify`
  primitive (audit-point property); positive D5 coverage lives in
  lib's `b41a_declassify_on_secret_unwraps_the_inner_type` via a
  synthetic env.
- No REPL, no driver binary. Library-only.
- `let rec` RHS must be a syntactic lambda. Parser enforces with
  `ParseError::LetRecNotLambda`. Relaxing this in B3 (when handlers
  arrive) is an open question; see ADR 0003.
- Polymorphic recursion is rejected, as in ML/Haskell without an
  explicit type signature. Test
  `b16_letrec_recursive_occurrence_is_monomorphic_inside_body`
  locks this.
- Equality (`==`) is polymorphic at the type level (`forall a. a -> a -> Bool`).
  The evaluator still rejects equality on closures; B2/B3 may
  refine via type-class-style constraints.
- `EvalError` variants carry no spans. Eval errors are rare
  post-type-check (div-by-zero, non-function application on a
  closure-typed value, the letrec uninitialised internal error)
  but they render without carets. B2 backlog item.
- Multi-line spans in `diag::render` clip to the first line. Sentinel-Mini
  programs are usually one-liners; Phase C diagnostics will handle
  multi-line ranges properly.
- Closures `clone()` the body `Expr` into an `Arc<Expr>`. Acceptable
  for a research artifact; revisit if body-clone becomes a hot path
  or if the broker becomes the value heap.
- `Value` does not derive `Eq` (closures aren't comparable); the
  error types also drop `Eq` to keep them embeddable in each other.
  They remain `Debug + Clone + PartialEq`, which is what tests use.

---

## Section C — bootstrap compiler (HANDOVER §6)

Phase C is the production Sentinel compiler in Rust per ADR 0009. C0
is the end-to-end pipeline for the smallest language subset (let,
arithmetic, if, function calls) compiling to LLVM with no type
system — everything i64 — to prove the pipeline shape works. Six
sub-phases C0.0-C0.5.

The Phase C compiler crates are populated in pipeline order across
C0-C5 per ADR 0009 D7. As of C0.0, sentinel-syntax has a lexer; the
remaining nine compiler crates (sentinel-ast, sentinel-resolve,
sentinel-types, sentinel-hir, sentinel-mir, sentinel-codegen,
sentinel-driver, sentinel-runtime, sentinel-lsp) remain 20-line
scaffold stubs.

### C.1 Phase Tracker

| Phase | Title                                                          | Status  | Commit  |
|-------|----------------------------------------------------------------|---------|---------|
| C0.0  | Tokens + lexer + tests/ui/ harness + 1 lex-error UI test       | Done    | 8f37381 |
| C0.1  | Hand-written parser + AST + `snc parse` subcommand             | Done    | 7e32e8c |
| C0.2  | LLVM codegen + `snc build` + first runnable binary + tests/pass/ | Done  | 0b07931 |
| C0.3  | let bindings + variable references (i64 everywhere)            | Done    | 80d2b6b |
| C0.4  | if/else + block expressions + `print` calls + first stdout     | Done    | baf68fc |
| C0.5  | fn definitions + main entry; **C0 go/no-go passes**            | Done    | 6ce8336 |
| C1.0a | sentinel-base crate: Salsa db trait + SourceFile input + Diagnostic accumulator | Done | 09dc8c3 |
| C1.0b | Wrap lex/parse as `#[salsa::tracked]` queries; driver uses SentinelDatabase | Done | 557cc60 |
| C1.0c | Codegen-salsa decision: defer until typed HIR (C1.2+); ADR 0011 D1 amended | Done | 8b58644 |
| C1.1.1 | Scaffold sentinel-resolve: VarId/FnId, parallel-tree resolved AST, resolve() + salsa query | Done | 438dd16 |
| C1.1.2 | Codegen consumes ResolvedProgram; driver chains resolve_query | Done | 9374edf |
| ADR 12 | Concrete C1 surface syntax (annotation grammar + bool/cmp/logical ops) | Done | 6ab3661 |
| C1.2.1 | Lexer `:` token                                                | Done | af16655 |
| C1.2.2 | AST/parser annotation grammar + 22 fixture rewrite             | Done | 90965a5 |
| C1.2.3 | sentinel-types scaffold (Type, TypedProgram, check, check_query) | Done | ded07bc |
| C1.2.4 | Codegen consumes TypedProgram; driver chains check_query       | Done | c9a21ff |
| C1.3.1 | Lexer: 11 C1.3 tokens (`true` `false` `==` `!=` `<` `<=` `>` `>=` `&&` `\|\|` `!`) | Done | 2801a81 |
| C1.3.2-4 | AST + parser + resolve + types + codegen for bool/cmp/logic/`!`; type universe widens to `{I64, I32, Bool}` | Done | cd1c0d4 |
| C1.3.5 | Retire ADR 0010 D9 C-style truthy; rewrite 6 if-fixtures; add 7 c13 pass-tests | Done | ba5fd9d |
| ADR 13 | Concrete C1.4 surface syntax (struct decl + literal + field access) | Done | e93635b |
| C1.4.1 | Lexer: `struct` keyword + `.` token                            | Done | f34b401 |
| C1.4.2-6 | AST + parser + resolve + types + codegen for structs (bundled) | Done | aa8f252 |
| ADR 14 | Concrete C1.5 surface syntax (`?T` nullables + null + unwrap_or/is_some) | Done | 3cb1238 |
| C1.5.1 | Lexer: `null` keyword + `?` token                              | Done | dff8642 |
| C1.5.2-6 | AST + parser + resolve + types + codegen for `?T` (bundled)   | Done | 1d0adae |
| ADR 15 | Concrete C1.6 surface syntax (arrays + heap + len + D11 unlock) | Done | 8924d38 |
| C1.6.1 | Lexer: `[` and `]` tokens                                      | Done | 3cfd49f |
| C1.6.2-6 | Runtime + AST + parser + resolve + types + codegen for arrays (bundled) | Done | 8c5bbbe |
| C1.7+ | Generics                                                        | Planned |         |

ADR 0010 (concrete C0 surface syntax) lands between C0.0 and C0.1
per ADR 0009 D8.

Test coverage as of C0.0: 13 sentinel-syntax tests (11 lexer + 1
smoke + 1 UI integration). The UI test runs
`tests/ui/lex_invalid_char.sentinel` (one-line `let x = @`) through
the lexer and snapshots the miette-formatted diagnostic at
`crates/sentinel-syntax/tests/snapshots/ui__ui_lex_invalid_char.snap`.

Test coverage as of C0.1: sentinel-syntax gains 23 parser unit
tests (six covering the precedence ladder and left-associativity
on both add and mul; three covering unary minus including the
unary-binds-tighter-than-mul rule; three covering span tracking
across full / parenthesized / unary expressions; seven covering
parse errors — unmatched open paren, unexpected close paren, EOF
after operator, EOF after unary, lex-error passthrough, int-lit
overflow, trailing garbage; plus an int_lit_zero edge case and a
nested-parens test). One new UI integration test
(`ui_parse_unbalanced_paren`, snapshotted at
`ui__ui_parse_unbalanced_paren.snap`) snapshots the
`unmatched_paren` diagnostic for the fixture `(1 + 2`. sentinel-ast
gains 6 Display tests (int literal, binary, nested precedence,
unary, plus the BinOp::symbol and UnaryOp::symbol helpers) on top
of its existing smoke. Workspace delta: +30 active tests.

Test coverage as of C0.2: sentinel-codegen lifts out of scaffold-
stub status with 1 new target-init probe (`target_init_does_not_panic`)
on top of its existing smoke. sentinel-driver gains 5 pass tests at
`crates/sentinel-driver/tests/pass.rs` (`pass_c02_arithmetic`,
`pass_c02_precedence`, `pass_c02_parens`, `pass_c02_unary`,
`pass_c02_division`) covering the four operators, precedence,
parens, and unary minus via the full pipeline. The runner uses
`CARGO_BIN_EXE_snc` to locate the snc binary that cargo builds
before integration tests run; per-fixture executables land in
`target/sentinel-pass/` (gitignored). Workspace delta: +6 active
tests (353 total).

Test coverage as of C0.3: sentinel-ast gains 5 Display tests
covering Var, StmtKind::Let, StmtKind::Expr, Program with empty
stmts (which prints just the tail — preserving C0.1/C0.2 output
verbatim), and Program with stmts (one stmt per line + tail).
sentinel-syntax lib gains 14 parser tests: 2 Var-in-expression
tests, 6 program-level happy paths (empty stmts, single let,
multiple lets, expr-statement followed by tail, let-uses-let, span
tracking on `let` covering keyword through `;`), and 6 program-
level error cases (empty input, only-let-no-tail, missing `;`,
missing `=`, missing identifier, bare `let`). sentinel-codegen
gains 4 lib tests: empty-stmts program, let program, undefined
variable rejection, redeclared variable rejection. sentinel-driver
gains 4 pass tests (`pass_c03_simple_let`,
`pass_c03_multiple_lets`, `pass_c03_let_uses_let`,
`pass_c03_expr_statement` — the last verifies that an expression-
statement is computed but its result is discarded). Workspace
delta: +27 active tests (380 total).

A UI snapshot for the `undefined_variable` codegen diagnostic was
deliberately deferred — the lib unit tests cover the error variants
and a dedicated UI runner for codegen errors lands when C0.4+
introduces more codegen-stage diagnostics worth coordinating
(unbound call target, arity mismatch, etc.).

Test coverage as of C0.4: sentinel-ast gains 7 new Display tests
covering block/if/call/expr_block and the call-zero/one/two-arg
variants. sentinel-syntax lib gains 20 new parser tests: 3 for
blocks (simple, with stmt, in arithmetic position as atom), 4 for
if/else (simple, else-if chain, with var condition, in parens
inside arithmetic), 6 for calls (zero/one/multi args, trailing
comma, in arithmetic, with complex arg), 1 for var-vs-call
disambiguation, 1 for a program with a print-statement, and 5
error cases (if missing else, if missing then-block, empty block,
call unclosed args, block unclosed after tail). sentinel-codegen
gains 6 new tests (if expression, block expression, call to
print, undefined function, arity mismatch too-many, arity
mismatch too-few). sentinel-runtime gains its first real
function (`sentinel_print`) with a smoke + return-zero test.
sentinel-driver gains 8 new pass-test fixtures (print_simple,
print_then_tail, if_true_branch, if_false_branch, if_with_var_cond,
else_if_chain, block_expression, if_with_print) — five assert
on exit codes, three assert on stdout content + exit. Workspace
delta: +43 active tests (423 total).

Test coverage as of C0.5: sentinel-ast gains 4 new FnDef-related
Display tests (display_program_one_main, display_program_two_fns,
display_fn_def_zero_params, display_fn_def_multi_params) and
retires 2 stmt+tail Program tests, net +2. sentinel-syntax lib
gains 14 new parser tests covering fn-def parsing (single fn,
fn with one/multi/trailing-comma params, multi-fn programs,
fn-def Display round-trip, span tracking) plus 6 fn-def error
cases (top-level not-fn, top-level bare expr, missing name, missing
parens, missing body, bad param name); a `parse_block_str` public
function is added so the existing C0.3-0.4 parse_program_* tests
keep working with brace-wrapped inputs. sentinel-codegen gains 1
net test (compile_main_with_int_lit, compile_main_with_let_program,
compile_rejects_missing_main, compile_rejects_redefined_function,
compile_rejects_user_redefining_print, compile_multi_fn_with_
forward_ref, compile_call_to_user_fn_arity_check — 7 new tests but
with restructuring of the C0.4 tests around the new main-required
shape, the net is +1). sentinel-driver gains 5 new pass-test
fixtures: c05_simple_fn (double + main), c05_multi_arg_fn (add),
c05_forward_ref (main calls fn defined after it), c05_call_chain
(quad = double(double(...))), and the C0 acceptance program
**c05_go_no_go** (the ADR 0010 appendix double + pick + main with
print). All 17 pre-C0.5 fixtures are mechanically rewrapped in
`fn main() { ... }` for the hard-break top-level shape change.
The UI fixture parse_unbalanced_paren rewrites to
`fn main() { (1 + 2 }` so it remains valid C0.5 program shape with
the embedded error. Workspace delta: +22 active tests (445 total).

Test coverage as of C1.0b: sentinel-syntax gains a new `query`
module hosting two `#[salsa::tracked]` queries and seven unit tests
covering positive lex / positive parse / diagnostic-accumulation on
lex error / diagnostic-accumulation on parse error / lex error
propagated through parse with lex stage preserved / cache stability
across reruns for both queries. sentinel-driver's bin gains a
concrete `SentinelDatabase` struct (Storage<Self> + salsa::Database
impl with no-op salsa_event + SentinelDb impl); `run_parse` and
`run_build` are refactored to instantiate the DB, set a SourceFile,
call `parse_query`, and collect diagnostics via the accumulator.
sentinel-ast types `Spanned<T>`, `Block`, `Param`, `FnDef`,
`Program`, `StmtKind`, `ExprKind` gain `#[derive(Hash)]`
(non-behavioral; required for salsa-friendliness of any future
tracked-struct fields, even though C1.0b avoids tracked structs by
going through the accumulator). Workspace delta: +7 active tests
(453 total: 90 syntax lib + 2 ui + 22 ast + 13 codegen + 22 pass +
2 runtime + 3 base + 5 stub + 69 broker + 226 effects-proto). All
22 C0 pass-test fixtures still run end-to-end through the new
query-based driver path; the C0 go/no-go program at
`tests/pass/c05_go_no_go.sentinel` still produces stdout `10\n`
exit 0. See C.3 design decisions 33-36.

### C.2 Crate Layout (C0.5)

    crates/sentinel-base/                 (C1.0a)
      Cargo.toml          deps: salsa, thiserror, tracing
      src/
        lib.rs            SourceFile (#[salsa::input] with `path`
                          and `text` fields); SentinelDb trait
                          (#[salsa::db], inherits salsa::Database);
                          Diagnostic accumulator (#[salsa::
                          accumulator]) with stage / severity /
                          code / message / span fields. Test-only
                          TestDb verifies the salsa machinery
                          (3 tests). Downstream pipeline crates
                          plug into SentinelDb at C1.0b (lex/parse)
                          and C1.0c (codegen).

    crates/sentinel-ast/
      Cargo.toml          deps: tracing, thiserror
      src/
        lib.rs            Span (= Range<usize>), Spanned<T>, BinOp
                          (Add|Sub|Mul|Div), UnaryOp (Neg), ExprKind
                          (IntLit | Var | Unary | Binary | Block | If
                          | Call), Expr = Spanned<ExprKind>; StmtKind
                          (Let { name, name_span, value } | Expr),
                          Stmt = Spanned<StmtKind>; Block { stmts,
                          tail, span }; Param { name, span }; FnDef
                          { name, name_span, params, body: Block,
                          span }; Program { fns: Vec<FnDef>, span }
                          (C0.5 top-level — was stmts+tail at
                          C0.3-0.4); Display impls for all (Program
                          prints fn-defs newline-separated, FnDef
                          prints `(fn name (params) body)`).
                          **C1.0b**: Spanned<T>, Block, Param, FnDef,
                          Program, StmtKind, ExprKind all derive
                          Hash; BinOp / UnaryOp already did. Required
                          for salsa-friendliness of any future
                          tracked-struct field, even though C1.0b
                          itself routes errors through the accumulator
                          and avoids tracked-struct fields.

    crates/sentinel-syntax/                (C1.0b adds query module)
      Cargo.toml          deps: sentinel-ast (path), sentinel-base
                          (path, C1.0b), logos, miette, salsa
                          (C1.0b), thiserror, tracing
                          dev-deps: insta
      src/
        lib.rs            module declarations + public re-exports
                          (lex, LexError, TokenKind from lexer;
                          parse, parse_expr, parse_block_str,
                          ParseError, Parser from parser;
                          lex_query, parse_query from query — C1.0b;
                          Program, Span, Spanned, Stmt, StmtKind
                          re-exported from sentinel-ast)
        lexer.rs          logos-based TokenKind (4 keywords + 11
                          punctuation kinds + Ident + IntLit; skip
                          patterns for whitespace and `//` line
                          comments); imports Spanned from
                          sentinel-ast; LexError (miette::Diagnostic);
                          pure lex() fn returning (tokens, errors)
        parser.rs         hand-written recursive descent. Three
                          pure entry points: parse(src) ->
                          Result<Program> at C0.5+ parses one or
                          more fn-defs (parse_program loops over
                          parse_fn_def, which eats `fn Ident
                          ( params? ) block`); parse_expr(src) ->
                          Result<Expr> retains the single-expression
                          contract for existing C0.1 tests + REPL;
                          parse_block_str(src) -> Result<Block>
                          parses a brace-wrapped block in isolation
                          (used by tests + future REPL/completion).
                          Internal: parse_block (for `{ stmt* tail
                          }` from `if` branches, atoms, and fn
                          bodies), parse_let_stmt (`let Ident =
                          expr ;`), parse_if (with else-if chain
                          via synthetic Block wrapping), parse_atom
                          dispatches IntLit / Ident-with-`(`-is-call
                          / bare-Ident-is-Var / `{`-is-Block /
                          `(`-is-paren. ParseError variants
                          unchanged since C0.3: Lex (transparent),
                          UnexpectedToken, UnexpectedEof,
                          UnmatchedParen, IntLitOverflow
        query.rs          (C1.0b) two `#[salsa::tracked]` queries:
                          lex_query(db, file) -> Vec<Spanned<TokenKind>>
                          (return_ref) and parse_query(db, file) ->
                          Option<Program> (return_ref). Each
                          accumulates `sentinel_base::Diagnostic`s
                          for errors via the salsa::Accumulator
                          trait. Private helpers
                          lex_error_to_diagnostic and
                          parse_error_to_diagnostic perform the
                          (stage, code, message, span) conversion;
                          ParseError::Lex forwards to the lex
                          converter so a lex-error-through-parse
                          still carries the lex stage/code. The
                          queries are independent — parse_query calls
                          `parse(src)` directly rather than depending
                          on lex_query, matching parse's existing
                          fail-fast-on-lex-error semantics. Test-only
                          TestDb mirrors the one in sentinel-base
                          (7 tests).
      tests/
        ui.rs             integration runner; shared themed-none
                          handler at 80 cols; ui_lex_invalid_char,
                          ui_parse_unbalanced_paren
        snapshots/
          ui__ui_lex_invalid_char.snap
          ui__ui_parse_unbalanced_paren.snap

    crates/sentinel-resolve/                (C1.1: populated)
      Cargo.toml          deps: sentinel-ast (path), sentinel-base
                          (path), sentinel-syntax (path), miette,
                          salsa, thiserror, tracing
      src/
        lib.rs            Stable identifiers: VarId(u32), FnId(u32),
                          plus the PRINT_FN_ID const (= FnId(0)).
                          FnSignature { id, name, name_span: Option<Span>,
                          arity, is_main, is_runtime }. Parallel-tree
                          resolved AST: ResolvedProgram { fns:
                          Vec<ResolvedFnDef>, fn_signatures: Vec<FnSignature>,
                          span }; ResolvedFnDef, ResolvedParam,
                          ResolvedBlock, ResolvedStmt(Kind),
                          ResolvedExpr(Kind) all mirror their AST
                          counterparts with Var/Call replaced by
                          IDs; binding sites retain their source
                          name for diagnostics + IR debug names.
                          ResolveError with 6 variants:
                          UndefinedVariable, RedeclaredVariable,
                          UndefinedFunction, ArityMismatch,
                          RedefinedFunction, MissingMain (moved from
                          CodegenError at C1.1.2). resolve(program:
                          &Program) -> Result<ResolvedProgram,
                          ResolveError> is the pure-function entry
                          point: pass 1 builds the fn table
                          (`print` as FnId(0), user fns following),
                          pass 2 resolves each fn body with a
                          per-fn vars HashMap. RHS of `let x = expr`
                          resolves BEFORE binding x, so `let x = x`
                          with no outer x is UndefinedVariable.
                          resolve_query(db, file) -> &Option<ResolvedProgram>
                          is the `#[salsa::tracked]` wrapper that
                          chains on sentinel_syntax::parse_query;
                          errors flow through the Diagnostic
                          accumulator with stage="resolve" and parse
                          errors propagate transitively. 21 tests
                          (positive paths + each error variant + 4
                          salsa query tests including parse-error
                          propagation and cache validation).

    crates/sentinel-types/                (C1.2.3: populated)
      Cargo.toml          deps: sentinel-ast (path), sentinel-base
                          (path), sentinel-resolve (path),
                          sentinel-syntax (path), miette, salsa,
                          thiserror, tracing
      src/
        lib.rs            Type universe: `Type::I64` only at C1.2 per
                          ADR 0012 D4; C1.3 adds I32 + Bool. Display
                          renders lowercase identifier (`i64`).
                          Parallel-tree typed AST mirrors
                          ResolvedProgram with `ty: Type` fields on
                          expressions: TypedProgram { fns, fn_signatures,
                          span }; TypedFnSignature { id, name,
                          name_span, param_types: Vec<Type>, return_type,
                          is_main, is_runtime }; TypedFnDef, TypedParam
                          (with `ty: Type`), TypedBlock (with
                          `ty: Type` == tail.ty), TypedStmt(Kind),
                          TypedExpr { kind, span, ty }, TypedExprKind
                          (variants identical to ResolvedExprKind).
                          TypeError with 4 variants: UnknownType
                          (annotation says something other than known
                          type name), Mismatch (cross-expression type
                          clash), ReturnTypeMismatch (fn body type ≠
                          declared return), CallArgMismatch (call arg
                          type ≠ callee param type). At C1.2 only
                          UnknownType is reachable since everything
                          types to I64; the others activate at C1.3.
                          check(program: &ResolvedProgram) ->
                          Result<TypedProgram, TypeError> pure-function
                          entry point: pass 1 builds typed_signatures
                          by resolving every fn's TypeExpr annotations
                          (print pre-registered); pass 2 walks each fn
                          body with a per-fn VarTypeEnv: HashMap<VarId,
                          Type> seeded from params. check_query(db,
                          file) -> &Option<TypedProgram> is the
                          #[salsa::tracked] wrapper that chains on
                          sentinel_resolve::resolve_query; errors flow
                          through the Diagnostic accumulator with
                          stage="types"; upstream stage diagnostics
                          propagate transitively. 15 unit tests:
                          positive paths (minimal main, annotated/
                          unannotated let, params+use, if/else
                          branches, print call, full go/no-go), 3
                          UnknownType paths (param, return, let
                          annotation), 4 salsa query tests including
                          resolve-error propagation and cache
                          validation.

    crates/sentinel-codegen/                (C1.2.4 rewrite — consumes TypedProgram)
      Cargo.toml          deps: sentinel-ast (path), sentinel-resolve
                          (path), sentinel-types (path, C1.2.4),
                          inkwell (llvm18-0 feature, workspace-pinned),
                          miette, thiserror, tracing
                          dev-deps: sentinel-syntax (for src-string
                          test driving via parse + resolve + check)
                          lints.rust: unsafe_code = "allow" (inkwell
                          uses unsafe internally for FFI)
      src/
        lib.rs            compile_to_object(program: &TypedProgram,
                          output_path) builds an LLVM module
                          containing all fns declared by the program.
                          **Two-pass**: pass 1 declares every fn by
                          iterating program.fn_signatures (the runtime
                          `print` maps to LLVM symbol `sentinel_print`
                          via signature.is_runtime; otherwise the
                          source name); pass 2 emits each user fn
                          body. `main` returns i32 (C ABI shape,
                          truncated from i64 body value) — gated on
                          signature.is_main via the TypedFnSignature;
                          other fns return i64. CodegenCtx<'ctx, 'a>
                          threads &context + builder + i64_type +
                          HashMap<FnId, FunctionValue> fns table +
                          current_fn + HashMap<VarId, PointerValue>
                          vars map. C1.2.4 changes: input shape
                          (Typed* instead of Resolved*) and how
                          is_main is read (via program.signature(id)
                          instead of fn_def.signature(program)).
                          Otherwise the LLVM lowering is unchanged
                          from C1.1.2 — at C1.2 every type is I64 so
                          the Type field on TypedExpr is not yet
                          driving instruction selection; that's
                          C1.3's bool/i32 work. find_var_name walk
                          ported to TypedBlock/TypedExpr. CodegenError
                          unchanged (5 LLVM-lowering-only variants
                          since C1.1.2).

    crates/sentinel-runtime/
      Cargo.toml          deps: tracing, thiserror
                          [lib] crate-type = ["lib", "staticlib"]
                          (the staticlib output is libsentinel_
                          runtime.a, linked into Sentinel programs
                          by the driver via the system cc; the rlib
                          remains for Rust consumers)
      src/
        lib.rs            sentinel_print(i64) -> i64 via
                          `#[no_mangle] extern "C"` — writes the
                          i64 to stdout as ASCII decimal + newline
                          and returns 0 (the call expression's
                          value per ADR 0010 D11)

    crates/sentinel-driver/                (C1.2.4 chains check_query)
      Cargo.toml          deps: sentinel-base (path, C1.0b),
                          sentinel-codegen (path), sentinel-resolve
                          (path, C1.1.2), sentinel-syntax (path),
                          sentinel-types (path, C1.2.4),
                          miette (with "fancy" feature), salsa
                          (C1.0b), thiserror, tracing
      src/
        main.rs           snc binary; subcommands `parse` and `build`
                          (C0.1+ shape; both lifted from Expr to
                          Program at C0.3). **C1.0b**: defines
                          `SentinelDatabase` (storage: salsa::Storage
                          <Self>) with `#[salsa::db]` impls for
                          `salsa::Database` (no-op salsa_event) and
                          `SentinelDb`. `run_parse` calls
                          sentinel_syntax::parse_query and pretty-
                          prints the AST. **C1.2.4**: `run_build`
                          chains sentinel_types::check_query (which
                          depends transitively on resolve_query and
                          parse_query in the salsa graph), collects
                          diagnostics via
                          check_query::accumulated::<Diagnostic>
                          — picks up parse + resolve + types-stage
                          diagnostics in one collection — and feeds
                          the resulting &TypedProgram to
                          sentinel_codegen::compile_to_object.
                          Diagnostics render through miette::MietteDiagnostic
                          (constructed at runtime from
                          stage/code/message/span — drops per-variant
                          help text and label text; rough but
                          functional, refinement deferred). `build`
                          then invokes the system `cc` on the
                          emitted `.o` plus `libsentinel_runtime.a`
                          (found via current_exe().parent()) to
                          produce the executable. Output defaults
                          to <file_stem>; exit codes 0 / 1 / 2.
      tests/
        pass.rs           pass-test runner; reads workspace-root
                          tests/pass/*.sentinel; uses CARGO_BIN_EXE_snc
                          to locate the binary cargo built for the
                          integration tests; builds each fixture into
                          target/sentinel-pass/ and asserts on the
                          executable's exit code

    tests/                                (workspace root, ADR 0009 D5)
      ui/
        lex_invalid_char.sentinel         `let x = @` fixture
        parse_unbalanced_paren.sentinel   `(1 + 2` fixture
      pass/                               (all wrapped in `fn main() { ... }` at C0.5)
        c02_arithmetic.sentinel           `6 + 7` -> exit 13
        c02_precedence.sentinel           `1 + 2 * 3` -> exit 7
        c02_parens.sentinel               `(5 + 3) * 2 - 1` -> exit 15
        c02_unary.sentinel                `-(-5)` -> exit 5
        c02_division.sentinel             `12 / 3` -> exit 4
        c03_simple_let.sentinel           `let x = 5; x` -> exit 5
        c03_multiple_lets.sentinel        `let x = 3; let y = 4; x + y` -> exit 7
        c03_let_uses_let.sentinel         `let a = 2; let b = a * 3; b + 1` -> exit 7
        c03_expr_statement.sentinel       `let x = 1; x + 99; 5` -> exit 5
        c04_print_simple.sentinel         `print(42)` -> stdout "42\n", exit 0
        c04_print_then_tail.sentinel      let + 2x print + tail -> stdout "7\n14\n", exit 21
        c04_if_true_branch.sentinel       `if 1 { 42 } else { 99 }` -> exit 42
        c04_if_false_branch.sentinel      `if 0 { 42 } else { 99 }` -> exit 99
        c04_if_with_var_cond.sentinel     let x=5; if x { x*2 } else { 0 } -> exit 10
        c04_else_if_chain.sentinel        let x=0; if/else-if/else -> exit 3
        c04_block_expression.sentinel     `let r = { let y = 4; y + 1 }; r * 2` -> exit 10
        c04_if_with_print.sentinel        if/print -> stdout "100\n", exit 0
        c05_simple_fn.sentinel            double + main -> exit 14
        c05_multi_arg_fn.sentinel         add(5, 6) -> exit 11
        c05_forward_ref.sentinel          main calls triple defined later -> exit 12
        c05_call_chain.sentinel           quad(3) = double(double(3)) -> exit 12
        c05_go_no_go.sentinel             ADR 0010 appendix: double + pick + main
                                          with print -> stdout "10\n", exit 0

    .cargo/
      config.toml         workspace-local cargo config (C0.2): [env]
                          sets LLVM_SYS_180_PREFIX (non-forcing —
                          developers with the env in zshrc are
                          unaffected); target.'cfg(target_os =
                          "macos")' rustflags add /opt/homebrew/lib
                          and /usr/local/lib to the link search path
                          so the linker can find brew-installed
                          zstd/libxml2 that LLVM 18 references

Three scaffold-stub compiler crates remain at 20-line
`crate_name() + smoke` stubs per ADR 0009 D7. Updated population
schedule (sentinel-resolve populated at C1.1, sentinel-types at
C1.2):

  - sentinel-hir:      C1.3+ (typed HIR may replace TypedProgram
                       once enough type-system features land to
                       motivate a separate HIR layer; or HIR may
                       remain folded into sentinel-types)
  - sentinel-mir:      C2 (SSA-form IR for region/borrow checking)
  - sentinel-lsp:      C5

### C.3 Design Decisions (C0)

ADR 0009 (D1-D8) is authoritative; in-source highlights:

1. Lexer uses `logos`. ADR 0009 D4 prescribes hand-written recursive
   descent for the parser only; lexers benefit from the regex-DFA
   payoff with no ergonomic cost.
2. `lex(src: &str) -> (Vec<Spanned<TokenKind>>, Vec<LexError>)` is
   a pure function per ADR 0009 D1a's "C0 pipeline stages are pure
   functions" discipline. No shared mutable state. No `&mut Cx`
   threading. Diagnostics accumulate via the return value.
3. No CST/AST split (ADR 0009 D4). The lexer's output is a flat
   `Vec<Spanned<TokenKind>>`; C0.1's parser will produce a direct
   AST enum.
4. Keywords (`let`, `fn`, `if`, `else`) lex via dedicated `#[token]`
   rules. logos's longest-match guarantees `letter` lexes as
   `Ident`, not `Let` + `ter`. See `lex_keyword_prefix_is_ident`.
5. Valid tokens still flow through when `LexError`s occur (the
   lexer collects errors rather than fail-fast). C0.1's parser
   decides whether to stop on lex errors or continue.
6. UI snapshot uses `GraphicalTheme::none()` + `width(80)` for
   host-independence. Terminal-width detection and ANSI colors
   would make snapshots host-dependent.
7. Workspace-root `tests/ui/` holds data files (HANDOVER §3.2); the
   integration runner lives in `crates/sentinel-syntax/tests/ui.rs`.
   insta snapshots stay at insta's default location (next to the
   runner). Centralizing snapshots under workspace-root
   `tests/snapshots/` is deferred until there's more than one
   runner crate to coordinate.
8. Parser is the pure function `parse(src: &str) -> Result<Expr,
   ParseError>` per ADR 0009 D1a. Lex errors block parsing and
   surface through a transparent `ParseError::Lex(LexError)` variant
   — the front end has a single error type for the driver to handle.
   Parse errors are fail-fast (one error per call); error recovery
   is deferred until parser ergonomics demand it.
9. `Span` and `Spanned<T>` live in `sentinel-ast` rather than
   `sentinel-syntax` because the AST is conceptually below syntax
   in the pipeline. The lexer's `Spanned<TokenKind>` and the parser's
   `Spanned<ExprKind>` are the same generic type. `sentinel-syntax`
   re-exports `Span` and `Spanned` for crates that consume tokens
   and AST nodes together.
10. Parens are syntactic only — they are not represented as a
    distinct AST node. `(1 + 2)` and `1 + 2` produce the same AST
    shape; the outer span on the parenthesized form covers the
    parens. C5+ LSP-style source-preserving formatting may revisit
    this if exact original-source round-trip becomes a requirement.
11. The driver uses miette's default (fancy color) `GraphicalReport
    Handler` for human-facing errors; UI tests use
    `GraphicalTheme::none()` + 80-col width in the test runner for
    host-independent snapshots. Two separate code paths, two
    separate concerns.
12. sentinel-codegen lowers `Expr` to an LLVM IR module via inkwell
    0.5 with the `llvm18-0` feature. The emitted module defines
    `main() -> i32` whose return value is the i64 expression value
    truncated to i32 — the temporary exit-code-is-the-answer
    convention. ADR 0010 D11's `print(x)` will replace it at C0.4
    when function calls land; the truncation goes away when stdout
    is the result channel.
13. Linking lives in the driver, not in sentinel-codegen. The
    driver's `build` subcommand invokes the system `cc` on the
    emitted `.o` to produce the executable. Linking is platform
    glue (linker flags, library search paths, dynamic loader
    conventions) rather than a compiler concern; sentinel-codegen
    stays focused on IR generation. The `cc` invocation will move
    behind a more controlled interface when cross-compilation is
    in scope (C5+).
14. `.cargo/config.toml` (workspace root) sets two things: (a)
    `LLVM_SYS_180_PREFIX` via cargo's non-forcing `[env]` so
    subprocess shells (CI, automation) see what the developer's
    interactive zshrc already provides; (b) target-conditional
    `rustflags` adding `/opt/homebrew/lib` and `/usr/local/lib`
    to the link search path so the linker finds brew-installed
    `zstd` and `libxml2` that LLVM 18 references but `llvm-sys`
    does not emit search paths for. Non-existent paths are
    silently ignored. This is a macOS-specific workaround; when
    Sentinel grows beyond macOS the configuration moves to a
    build script that probes `llvm-config --libdir`.
15. ADR 0009 D7 prescribed a `sentinel-types::check() -> Result<(),
    Diagnostic>` stub at C0.2 "so the pipeline shape is right when
    C1 fills it in." C0.2 deferred this to C0.3 (or possibly C1)
    because the stub adds no value at C0.2: arithmetic has no type
    semantics, the no-op `check()` would be threaded through the
    driver as `parse -> check (noop) -> codegen`, and the driver
    pipeline is already the right shape without it. C0.3 confirmed
    the deferral — let-binding scope tracking lives in codegen
    (see 20), not in a separate types pass. ADR 0009 status line
    records the deviation.
16. `ExprKind::Var(String)` stores the bound name as an owned
    `String` rather than an interned identifier. Interning is a C1
    concern: it lives in `sentinel-resolve` where name resolution
    formalises the symbol table. C0.3's `String` allocations are
    bounded by program size and disappear at codegen time, so the
    cost is acceptable.
17. Flat per-function scoping in C0.3. There is no notion of
    nested scope yet; redeclaring an existing name in the same
    function is `CodegenError::RedeclaredVariable` rather than
    shadowing. Block-scoped shadowing arrives with nested blocks
    at C0.4 (when `if`/`else` requires them). The choice was
    deliberate at the start of C0.3 — flat is simpler to
    implement and easier to diagnose; shadowing is a useful
    feature but not load-bearing at C0.
18. `StmtKind::Let { name, name_span, value }` carries the name's
    own span in addition to the wrapping `Stmt`'s full span. This
    is so redeclaration diagnostics can point at just the
    identifier rather than the whole `let x = expr;` statement.
    The pattern generalises: future statements with prominent
    sub-spans (struct definitions, function parameters) carry
    their own field-spans alongside the statement-level span.
19. `Program { stmts, tail, span }` is the AST top-level rather
    than an `ExprKind::Block(Vec<Stmt>, Box<Expr>)`. The Block-
    expression approach would have changed the C0.1/C0.2 parser
    tests (they would read the IntLit through a Block wrapper);
    keeping `Program` distinct lets `parse_expr(src)` keep its
    "single expression, no statements" contract that those tests
    rely on. Block arrives as an expression at C0.4 when `if`/
    `else` needs nested blocks; both representations coexist —
    Program at the top, Block inside expressions.
20. Codegen name resolution: the codegen pass maintains a
    `HashMap<String, PointerValue>` from variable names to LLVM
    alloca pointers. Each `let` enters the map; each `Var`
    reference reads from it. When `sentinel-resolve` lands at C1
    (per ADR 0009 D7), this map moves out of codegen and codegen
    becomes a pure structural lowering pass against a resolved
    AST. The C0.3 arrangement is a deliberate short-term
    architectural debt — STATE.md flags it in this list so the
    refactor at C1 is obvious.
21. `Block { stmts, tail, span }` and `Program { stmts, tail,
    span }` have the same shape. Both are AST struct types, and a
    Block can be promoted to an Expr via `ExprKind::Block(Box<Block>)`.
    Program is the top-level form (no surrounding braces in source;
    implicit body of the future `main` function); Block is the
    brace-wrapped nested form. Keeping them as distinct types lets
    a reader see at a glance which level of nesting a value
    represents, at the cost of one redundant struct. At C0.5 when
    `fn main() { … }` lands, Program may collapse into a list of
    fn-defs each containing a Block body.
22. `if`/`else` codegen uses an **alloca-based result slot** rather
    than LLVM `phi` nodes. The result alloca is created in the
    current insert position; both branches `store` the branch
    value; the merge block does `load` from the slot to produce
    the if-expression's value. At -O0 this is correct and easy to
    read in the IR; mem2reg promotes the alloca to phi when
    optimization is enabled. The C0.4 plan A pinned this choice up
    front.
23. `if` condition is `cond != 0` (C-style truthy per ADR 0010
    D9). The condition is computed as i64 and compared to zero via
    `IntPredicate::NE`. When C1 introduces `bool`, ADR 0010 D9's
    Revisit clause fires and the comparison goes away — the
    condition will be `bool`-typed by then.
24. `if` is positioned at the top of `parse_expr`, not inside the
    arithmetic precedence ladder. `if x { 1 } else { 2 } + 3` is
    a parse error ("expected end of input" after the if-expr);
    `(if x { 1 } else { 2 }) + 3` works because the parens accept
    any expression. The restriction parallels Rust's; revisited
    if a real program wants the looser form.
25. sentinel-runtime is built with `crate-type = ["lib",
    "staticlib"]` so cargo produces both `libsentinel_runtime.a`
    (linked into Sentinel programs by the system cc) and
    `libsentinel_runtime.rlib` (consumable from Rust if a future
    integration test wants to call the runtime directly). The
    driver locates the staticlib via
    `current_exe().parent().join("libsentinel_runtime.a")` because
    cargo puts the snc bin and the runtime in the same target
    directory — works for both `cargo run --bin snc` and
    `CARGO_BIN_EXE_snc`-driven integration tests.
26. `CodegenCtx<'ctx, 'a>` decouples the LLVM IR lifetime ('ctx,
    bound by Context) from the ctx struct's borrow lifetime ('a,
    bound by an inner scope in `compile_to_object`). The two
    lifetimes let the ctx be scoped to drop before `module.
    verify()` and `target_machine.write_to_file(&module, …)` run
    — the borrow checker can see that the ctx's borrows end at
    the inner block's `}` and so the later `module.verify()` is
    unaliased. C0.5 dropped the `&module` field from the ctx
    because pass 1 declares every function up-front, so pass 2
    never needs to mutate the module — it only emits IR through
    the builder against pre-existing FunctionValues.
27. C0.5 top-level shape is **`Vec<FnDef>`** with a mandatory
    `main` entry point — a hard break from the C0.3-0.4 implicit-
    main `stmt* tail_expr` form. The existing 17 C0.2-0.4 pass
    fixtures were mechanically rewrapped in `fn main() { ... }`.
    The hard break was chosen at the C0.5 start because clean
    shape going into C1 was worth the one-time fixture rewrite
    over preserving two top-level forms forever.
28. Codegen is **two-pass** per the C0.5 plan A. Pass 1 declares
    every function (including the runtime `print` mapped to
    `sentinel_print`); pass 2 emits each body. Forward references
    work because all signatures are in the module before any body
    is emitted. The cost is one extra walk of `program.fns`; the
    benefit is no defined-before-use constraint on user code.
29. `main` returns i32 while every other fn returns i64. This
    matches the C ABI's `main` signature so the system linker is
    happy with no extra glue. The i64 -> i32 truncation happens
    inside `compile_fn` only for `main`; other fns build a normal
    i64 return.
30. `print` is reserved: pre-declared in pass 1 as the runtime
    `sentinel_print` symbol, so a user-defined `fn print(x)` at
    pass-1 declaration time collides with the pre-declaration and
    surfaces as `CodegenError::RedefinedFunction`. The check is
    by name in the fns table, not a special-case in the parser.
31. Function parameters become per-fn allocas in the entry block:
    on `compile_fn` we clear `vars`, then for each param we
    allocate an i64 slot and `store` the incoming parameter value
    into it. The body then reads parameters via the same
    `vars.get` path as `let`-bindings — uniform treatment. C0.5
    arity check fires from `lower_call` via
    `fn_value.count_params()`; it covers both `print` (declared
    with one param) and user-defined fns uniformly.
32. `parse_block_str(src)` is a new C0.5 public entry point that
    parses a single brace-wrapped block. It's used by the
    parser's own tests (so the C0.3-0.4 stmt+tail tests can wrap
    their input in `{ ... }` and keep their assertions) and is
    available for any future REPL or LSP completion machinery
    that wants to parse just a block.

33. (C1.0b, ADR 0011 D1) The salsa retrofit lands lex and parse but
    not yet codegen. `parse_query` does NOT depend on `lex_query`;
    it calls the pure `parse(src)` entry point directly. Pros: the
    salsa-aware queries inherit `parse`'s existing fail-fast-on-
    lex-error semantics with zero divergence; lex errors flow
    through exactly one diagnostic path (via
    `parse_error_to_diagnostic(ParseError::Lex)`, which forwards to
    `lex_error_to_diagnostic` so the diagnostic carries the lex
    stage/code); no risk of double-emitting a lex diagnostic from
    both queries when the driver collects via parse_query's
    accumulator alone. Cons: parse_query re-lexes internally
    (wasted CPU; bounded by program size); no salsa cache benefit
    between lex and parse. C1.0c+ may revisit if codegen or
    sentinel-types want both tokens and AST in the same incremental-
    rebuild scope — a `parse_from_tokens(src, &tokens)` helper plus
    a parse_query that depends on lex_query is the obvious move,
    paired with peeking-the-accumulator from inside parse_query to
    avoid double-diagnostics (or accepting that minor cost).

34. (C1.0b) Errors flow through the `#[salsa::accumulator]`
    pattern rather than tracked-struct fields. The C1.0a session
    halted on `Vec<LexError> as tracked-struct field` because
    `miette::SourceSpan` doesn't derive Hash; the C1.0b resolution
    is that tracked-function return values carry ONLY the success
    payload (`Vec<Spanned<TokenKind>>`, `Option<Program>`) and
    errors get converted at the query boundary into
    `sentinel_base::Diagnostic`s — a Hash-friendly struct with
    `(stage, code, message, span: Range<usize>)` — and pushed via
    `Accumulator::accumulate(db)`. The conversion drops per-variant
    `#[diagnostic(help(...))]` text and per-`#[label(...)]` text
    that the lex/parse error enums carried; the driver renders
    using `miette::MietteDiagnostic` constructed at runtime, which
    produces a less ornamented but still source-pointed diagnostic.
    Refining the help/label preservation is a follow-up; the
    pipeline shape is the C1.0b deliverable, not diagnostic
    polish.

35. (C1.0b) Hash derives across sentinel-ast (Spanned<T>, Block,
    Param, FnDef, Program, StmtKind, ExprKind) land prophylactically.
    C1.0b itself routes errors through the accumulator and does
    NOT use tracked structs, so strictly speaking Hash isn't
    required for the retrofit to compile. But: (a) HANDOVER §0.2
    step 1 prescribed it based on the C1.0a investigation; (b)
    Hash is a strictly additive derive (no breakage); (c) future
    sub-phases that DO want to put AST nodes into tracked-struct
    fields (e.g., a `#[salsa::tracked]` Module struct in C1.1 with
    a resolved-program field) inherit Hash for free. Cost is one
    derive per type; benefit is "salsa will not surprise us at the
    next sub-phase."

36. (C1.0b) The concrete `SentinelDatabase` struct lives in
    sentinel-driver/src/main.rs, not in sentinel-base. ADR 0011 D1
    placed the cross-crate `SentinelDb` trait in sentinel-base
    deliberately so pipeline crates (sentinel-syntax,
    sentinel-codegen, future sentinel-resolve, sentinel-types) can
    depend on the trait without depending on the concrete database
    that wires every query. The driver is the assembly point; the
    concrete DB lives there. Tests inside individual crates (e.g.,
    `query.rs`'s tests in sentinel-syntax) declare their own
    `TestDb` rather than reaching into the driver — same pattern
    as `sentinel-base`'s test module. Repeated minimal `TestDb`
    boilerplate (~12 lines per crate) is acceptable; a shared
    test-util crate would invert the dep direction and bloat
    test-build times.

38. (C1.1.1, ADR 0011 D4) sentinel-resolve uses a **parallel-tree**
    representation rather than a side-table or generic-AST scheme.
    ADR 0011 D4 specifies that `ResolvedProgram` "is the input AST
    with name references replaced by stable identifiers." Three
    representations were considered: (a) the parallel-tree approach
    (chosen) where ResolvedProgram has its own ResolvedExprKind /
    ResolvedStmtKind etc. mirroring the AST shape; (b) a side-table
    approach where the original AST stays untouched and resolution
    state lives in HashMap<Span, ID> tables; (c) a generic-AST
    approach where ExprKind<R> takes a reference type R that
    instantiates to String pre-resolve and to VarId/FnId post-
    resolve. Reasoning: (a) wins on debuggability and ergonomics
    (each variant is concrete, no generics to chase through error
    messages or type signatures); (b) was rejected because span-
    keyed maps are fragile (duplicate spans break it; future
    macro-expanded code would too) and HashMap-of-pointers is
    ergonomically awful; (c) was rejected because the generics
    parameter propagates through every AST type's signature and
    inflates the surface that callers (codegen, future types pass,
    LSP) have to track. The cost of (a) is keeping two parallel
    type hierarchies in sync as the AST grows; the discipline is
    "every AST change at C1.3+ updates the resolved tree in the
    same commit," which matches the rhythm of the codebase already.

39. (C1.1.1) `let x = expr` resolves the RHS BEFORE binding the
    name. So `let x = x` with no outer `x` is UndefinedVariable,
    not a self-reference. This matches the C0 codegen's behavior
    (lower_expr on the RHS happens before vars.insert(name)) and
    keeps the language consistent with Rust on this point. The
    behavior is locked by the
    `let_x_equals_x_errors_when_outer_x_undefined` unit test.

40. (C1.1.2) Codegen preserves source names in LLVM SSA labels by
    walking the current fn's body for the binding's source name at
    each Var load. The walk lives in `find_var_name_in_block` /
    `find_var_name_in_expr`. This is **purely IR readability** —
    semantically the VarId is the load-bearing identifier;
    codegen never depends on the name for lookup. Cost: O(fn-body
    size) per Var reference at codegen time. Acceptable at C1 scale
    (no fn body is more than ~50 lines at C0); revisit if codegen
    becomes a profiling bottleneck. An alternative would be to
    pre-build a HashMap<VarId, &str> per fn at the start of
    compile_fn; the walk-on-each-load avoids the bookkeeping and
    is the simpler default until measurements demand otherwise.

41. (C1.2.2, ADR 0012 D1-D4) Annotation grammar wires through the AST.
    Three parallel additions to sentinel-ast: `Param.ty: TypeExpr`
    (mandatory at C1.2 per ADR 0012 D1), `FnDef.return_type:
    TypeExpr` (mandatory), `StmtKind::Let.ty_annot: Option<TypeExpr>`
    (optional per ADR 0012 D2). `TypeExpr = Spanned<TypeExprKind>`
    where `TypeExprKind::Ident(String)` is the only variant at
    C1.2 — the enum is deliberately open-ended so C1.4's struct
    names, C1.5's `?T`, C2's `&T`/`@region T`, and C3's `secret T`
    all land as new variants without churning the existing
    annotation sites. Display preserves source-readable form
    (`(fn name (p: i64 q: i64) -> i64 body)`) so `snc parse` output
    remains human-debuggable.

42. (C1.2.2, ADR 0012 D10) Fixture rewrite is a Python script
    (`/tmp/annotate_fixtures.py`) committed alongside the feat for
    reproducibility. The regex `fn IDENT(PARAMS) {` captures
    bare-name params and injects `: i64` plus appends `-> i64`
    after `)`. All 22 .sentinel pass-test fixtures + the
    parse_unbalanced_paren UI fixture were rewritten in one pass;
    snapshots re-blessed. The script is "throwaway tooling"
    (the rewrite happens once); checking it in alongside the
    commit makes the rewrite reproducible if a future hard break
    needs the same shape.

43. (C1.2.3, ADR 0011 D5) sentinel-types uses a **parallel-tree**
    representation matching sentinel-resolve's pattern from
    C1.1.1. ResolvedExpr and TypedExpr have different shapes —
    ResolvedExpr is `Spanned<ResolvedExprKind>`, TypedExpr is a
    struct `{ kind, span, ty }`. The inline `ty: Type` field
    avoids `Spanned<(TypedExprKind, Type)>` ergonomic awkwardness
    and gives codegen one-hop access to the type during lowering.
    Same for TypedBlock (carries `ty: Type` == tail.ty so codegen
    can read the block's type without recursion). TypedStmt is a
    struct `{ kind, span }` (statements have no value type at C1.2,
    but the struct shape stays consistent with the rest of the
    typed tree). The cost is keeping yet-another parallel tree in
    sync; at C1.3 this is "the same maintenance cadence as
    sentinel-resolve" which the codebase already absorbs.

44. (C1.2.3) The `check()` pass at C1.2 is mostly mechanical
    because the universe is just `I64`. Real cross-type validation
    (Mismatch, ReturnTypeMismatch, CallArgMismatch) is implemented
    but unreachable at C1.2 since every annotation must say `i64`
    (anything else hits UnknownType first). C1.3 activates the
    dormant variants when `bool` and comparison operators introduce
    multi-type expressions. The implementation already has the
    right shape (env: HashMap<VarId, Type>, signatures resolved
    once into TypedFnSignature.param_types/return_type, recursive
    walk threading the env); only the Type universe + a few
    operator-typing rules need to expand at C1.3.

45. (C1.2.4) Codegen sheds zero LOC and gains zero LOC of LLVM
    logic at C1.2.4 — the lowering is identical to C1.1.2's
    because every type is still I64. The change is **only the
    input shape**: ResolvedExpr → TypedExpr, ResolvedBlock →
    TypedBlock, etc. The `ty: Type` field on TypedExpr is
    available to codegen but unused at C1.2; C1.3 will start
    reading it (e.g., to emit `i1` for bool conditions and `i32`
    for i32-typed values). This minimal-diff refactor keeps
    C1.2.4 reviewable; the substantive type-aware codegen work
    happens at C1.3.

46. (C1.3.1, ADR 0012 D9) The lexer additions for C1.3 — six
    comparison ops (`== != < <= > >=`), three logical ops (`&& ||
    !`), two boolean keywords (`true` `false`) — landed as a
    single ~15-LOC patch + 9 new tests. logos's longest-match
    guarantee made the precedence-aware lexing automatic: `==`
    beats `=`, `!=` beats `!`, `<=` beats `<`, `>=` beats `>` —
    no manual reordering tricks needed beyond listing them as
    separate `#[token]` entries. The change is isolated to
    `crates/sentinel-syntax/src/lexer.rs`; nothing downstream
    sees the new tokens at this commit because parser changes
    follow in step 2.

47. (C1.3.2-4 / cd1c0d4) New ExprKind variants `Cmp(CmpOp, l, r)`
    and `Logic(LogicOp, l, r)` are kept separate from `Binary`
    rather than overloading `BinOp` with all 12 operators. The
    motivation is exhaustive matching at the type-check and
    codegen layers — `Binary` typing is "same int type → same
    int type", `Cmp` is "same type → Bool", `Logic` is "Bool,
    Bool → Bool with short-circuit". Three distinct typing rules
    map cleanly to three distinct ExprKind arms; collapsing them
    into Binary would require a runtime dispatch on `BinOp` in
    every consumer. Same argument applies at the codegen layer:
    arithmetic uses `build_int_add` / etc., comparison uses
    `build_int_compare`, logical uses basic-block control flow
    with a PHI — three different LLVM idioms that benefit from
    being separate match arms.

48. (C1.3.2-4 / cd1c0d4) The parser's precedence ladder grew
    three new levels (or > and > cmp) between `parse_expr` and
    `parse_add`. Comparisons (`cmp_expr`) are non-associative
    per ADR 0012 D6 — a second cmp op after the first surfaces
    as the new `ParseError::ChainedComparison` rather than
    parsing as `(a < b) < c` (which would type-error anyway
    because `(a < b)` is Bool and Bool can't compare with int).
    Parser-level rejection gives a better diagnostic ("chained
    comparison is not allowed; parenthesise one side") than
    the type error would. Aligns with Rust; diverges from Python.

49. (C1.3.2-4 / cd1c0d4) Codegen drops its `i64_type` ctx field
    in favour of a type-driven `llvm_int_type(Type)` helper that
    picks between `i1`, `i32`, and `i64` based on the typed
    expression's annotation. The `vars` HashMap changes from
    `VarId → PointerValue` to `VarId → (PointerValue, Type)` so
    `build_load` can pick the right element type for each
    variable. Function signatures consult `signature.return_type`
    and `.param_types` for the LLVM fn type. The `'ctx` and `'a`
    lifetimes on `CodegenCtx` collapsed to a single `'ctx` since
    the borrowed Context and the LLVM derived values share the
    same effective scope; the two-lifetime version was a hold-
    over from C1.2 that no longer earned its keep.

50. (C1.3.2-4 / cd1c0d4) Short-circuit `&&` / `||` lower to
    PHI-based control flow: one conditional branch on the lhs,
    a separate basic block for the rhs evaluation, and a join
    block with a PHI that takes the lhs value (when the rhs is
    skipped) or the rhs value (when it ran). The pattern is
    standard for compiled languages — same shape as LLVM's
    canonical short-circuit lowering and what Rust / Swift /
    Clang produce. Unary `!` is the simpler case: `xor x, 1`
    flips an i1 bit cheaper than a `compare-EQ-with-false`
    would. The PHI versus alloca-and-branch question was
    decided in favour of PHI because the join block is short
    and SSA-natural; alloca-and-branch makes more sense for the
    `if-else` lowering which has multi-statement branches.

51. (C1.3.5, ADR 0010 D9 retirement / ba5fd9d) Retiring C-style
    truthy + rewriting the 6 if-using fixtures is a single
    commit because the type-checker rule and the fixture
    rewrites are intrinsically coupled — flipping one without
    the other fails the suite. The fixture rewrites are
    mechanical (`if x` → `if x != 0`, `if 1` → `if true`, etc.);
    the c05_go_no_go program gets the full ADR 0012 appendix
    shape with `is_positive(x)` returning bool and
    `pick(cond: bool, ...)` consuming it. Codegen sheds the
    legacy "compare-NE-zero on int condition" path since the
    type checker now guarantees `cond.ty == Bool` — a
    `debug_assert_eq` pins the invariant. ADR 0012 D9 is fully
    exercised; ADR 0010 D9 is now historically retired (the
    "deliberate temporary ugliness" loop opened by C0.4 is
    closed).

52. (C1.3.5 / ba5fd9d) Short-circuit verification fixtures
    `c13_short_circuit_and` and `c13_short_circuit_or` pin the
    PHI-based control flow against regression. They use a
    `print(99) > 0` rhs operand that, if evaluated, would emit
    "99\n" to stdout; the test asserts stdout is empty. If a
    future codegen change accidentally collapses `&&` / `||` to
    eager evaluation (e.g., `build_and(l, r)` / `build_or(l,
    r)`), the side effect surfaces and the test fails. This is
    a stronger guarantee than just verifying the result value
    is correct — many bugs that produce the right value still
    eagerly evaluate.

53. (C1.3 / cd1c0d4) i32 is in the type universe per ADR 0012
    D3 but practically thin without literal-typing
    infrastructure. Integer literals always type as I64; there
    is no `5i32` suffix, no `cast(x as i32)`, no bidirectional
    inference. So `let r: i32 = some_fn_returning_i32()` works,
    `fn add32(a: i32, b: i32) -> i32 { a + b }` works (operands
    flow from parameters), but `add32(2, 3)` fails (the literal
    `2` types as I64). Future literal-typing work (C1.5+) will
    revisit this; documented here so the C1.3 scope is clear.

54. (C1.4.1 / f34b401) The C1.4 lexer additions are just two
    tokens (`struct` keyword + `.` punctuation) per ADR 0013 D8.
    logos's longest-match is not relevant here — `.` has no
    `..` / `.=` / `...` neighbours until ranges arrive in C2+.
    The `structure` / `structured` ident-prefix-vs-keyword
    regression is verified by a dedicated test. 4 new lexer
    tests + the existing lex_all_keywords / lex_all_punctuation
    extended.

55. (C1.4.2-6 / aa8f252) C1.4's AST adds new variants
    `ExprKind::StructLit` and `ExprKind::FieldAccess` —
    separate from `ExprKind::Call` because their typing rules
    differ. StructLit takes a struct name + a set of named
    field initializers; FieldAccess is postfix `.field` on any
    expression. Both could in principle be expressed via
    existing variants (StructLit as a magical `Foo` call,
    FieldAccess as a method-call shape), but separate variants
    let the type checker enforce per-feature rules without
    runtime branching on a generic `Call` shape. Same precedent
    as C1.3's Cmp / Logic separation.

56. (C1.4.2-6 / aa8f252) The parser's `allow_struct_lit: bool`
    mode flag per ADR 0013 D3a is the first piece of context-
    sensitive parsing in Sentinel. It's set to false while
    parsing an `if` condition so `if Foo { ... }` parses as
    if-condition `Foo` (Var atom) + if-then block `{ ... }`
    rather than struct-literal `Foo { ... }` + unexpected `else`.
    Parens always restore it (the LParen arm in parse_atom saves
    and re-sets the flag inside the parenthesized subexpr).
    Bounded — just a single Boolean threaded through `parse_expr`.
    Rust has the same shape and reasoning. C2+'s `while` / `for`
    / `match` will inherit the same forbidden-position list.

57. (C1.4.2-6 / aa8f252) Field access is implemented via a
    `parse_postfix()` wrapper that runs `parse_atom()` and then
    loops on `.field` chains. This pattern generalises to other
    postfix operators — C1.6's arrays will add `[index]`
    indexing; C4's classes will add `.method()` (which is `.`
    plus call shape). The loop shape gives left-associativity
    naturally: `a.b.c` parses as `(a.b).c`. parse_unary now
    calls parse_postfix instead of parse_atom directly.

58. (C1.4.2-6 / aa8f252) The resolve crate's struct table is
    built BEFORE fn signatures so that fn signatures can
    reference struct types in their TypeExprs. Resolution
    order in `resolve()`: (0) struct decls, (1) fn signatures,
    (2) fn bodies. Without this ordering, a fn `fn f(p: Point)
    -> i64` that mentions `Point` before its declaration in
    source order would fail. Sentinel doesn't require source-
    order declaration anyway (Sentinel-Mini taught this), so
    the multi-pass approach is correct.

59. (C1.4.2-6 / aa8f252) The recursive-struct check at
    sentinel-types runs a depth-first walk on the
    struct→field-struct edges using a Color enum (White / Gray
    / Black). Cycles surface when the walker hits a Gray node
    — the current path stack is the cycle. Direct cycles
    (`struct Node { next: Node }`) and mutual cycles
    (`struct A { b: B } struct B { a: A }`) both detected.
    Lifts at C1.5 when `?T` introduces indirection that breaks
    cycles. The error includes the full cycle path for the
    diagnostic.

60. (C1.4.2-6 / aa8f252) The type checker reorders struct-
    literal fields to declaration order at check() time, so
    codegen iterates by index without needing to consult field
    names. Source order `{ y: 4, x: 3 }` becomes
    `{ x: 3, y: 4 }` in TypedExprKind::StructLit.fields,
    matching the struct declaration's field order. The
    field_index in FieldAccess is set similarly — it's the
    GEP offset that codegen consumes directly.

61. (C1.4.2-6 / aa8f252) Codegen's value type widens from
    `IntValue<'ctx>` to `BasicValueEnum<'ctx>` to accommodate
    struct values. Every lower_* function's return signature is
    updated; arithmetic / cmp / logic / unary ops call
    `.into_int_value()` on their operands (the type checker
    guarantees they're int-typed) and `.into()` on results.
    The `vars` map's `(PointerValue, Type)` shape is unchanged
    at the surface but now backs struct-typed bindings — alloca
    uses `llvm_basic_type` instead of `llvm_int_type`. The
    refactor is mechanical but pervasive (every match arm
    touched). One-shot value-type widening is much cleaner than
    threading two return types.

62. (C1.4.2-6 / aa8f252) Struct literal codegen builds via a
    chain of `build_insert_value` starting from
    `struct_type.get_undef()`. Field access codegen uses
    `build_extract_value(struct_val, field_index)`. Both are
    SSA-native (no alloca dance needed for the value-level
    operations). Pass-by-value through fn calls is transparent
    — LLVM's ABI lowering handles the small-struct-in-registers
    vs large-struct-via-pointer choice automatically. C1.4
    doesn't need to think about it.

63. (C1.4.2-6 / aa8f252) Struct equality `==` / `!=` is
    rejected at C1.4 per ADR 0013 D6 — the Cmp typing rule
    explicitly rejects struct operands. Allows `struct == struct`
    in a future ADR (C1.5+) without retroactively breaking
    programs that depend on the current rejection. Same for
    struct arithmetic (`struct + struct`) — caught by the
    existing arithmetic rule that requires int operands.

64. (C1.4.2-6 / aa8f252) The codegen lifetime situation
    settled at C1.3 as `'ctx` alone parameterising `CodegenCtx`
    holds through C1.4. The new `struct_types: HashMap<StructId,
    StructType<'ctx>>` field shares that lifetime. Pass 0 of
    `compile_to_object` (declare LLVM struct types) happens
    before the CodegenCtx is built, so the types are passed in
    by-value at construction. This avoids re-creating struct
    types per fn body.

65. (C1.5.1 / dff8642) The C1.5 lexer additions are `null`
    keyword + `?` token per ADR 0014 D8. The `?` token is
    reserved for type-position only at C1.5 — `?.` / `??` /
    `?` propagation / `x!` force-unwrap are deferred per D11
    to avoid token-role conflicts. 6 new lexer tests including
    the `nullify` / `null_value` / `nullable` ident-prefix
    regression.

66. (C1.5.2-6 / 1d0adae) **Implementation revises ADR 0014 D4**:
    the D4 spec proposed `Type::Nullable(Box<Type>)` but the
    implementation went with a flat `NullableInner` subset enum
    instead. The reason: `Box<Type>` would force `Type` to lose
    `Copy`, cascading `.clone()` additions across ~20-30 use
    sites in sentinel-types + sentinel-codegen. The subset enum
    keeps `Type` `Copy` and makes the no-nested-nullables rule
    (ADR 0014 D6) structural rather than convention-based —
    `?(?T)` is literally unrepresentable. Cost: a duplicate
    constructor list (Type and NullableInner mirror each other
    minus the Nullable case); every new Type at C1.6+ adds one
    line to NullableInner. Worth it.

67. (C1.5.2-6 / 1d0adae) **Bidirectional checking infrastructure**
    via `check_expr(expr, expected: Option<Type>, ...)`. The
    `expected` is threaded through and bottoms out at
    `coerce_to_expected(synth, expected)` which either passes
    through, widens T→?T via `WidenToNullable`, or surfaces a
    `Mismatch` if synth type doesn't fit expected. Pushed into:
    let RHS, fn body tail (only when return type is Nullable to
    preserve ReturnTypeMismatch granularity), call args (only
    when param type is Nullable to preserve CallArgMismatch
    granularity), struct-lit field values, if/else branches.
    The selective-pushdown rule prevents the more specific
    error variants from being shadowed by Mismatch in the
    non-nullable case.

68. (C1.5.2-6 / 1d0adae) `null` literal has no synthesis type.
    The check_expr handles ResolvedExprKind::NullLit before the
    main match: if expected is Some(Type::Nullable(_)), the
    NullLit types as expected; otherwise `TypeError::AmbiguousNull`.
    This is the only place in the type checker where the
    expected type is REQUIRED (everywhere else it's optional
    extra info).

69. (C1.5.2-6 / 1d0adae) The generic builtins `unwrap_or` and
    `is_some` are special-cased at the Call typing arm because
    Sentinel doesn't have real generics yet. The signature
    table holds placeholder param types (?I64 / I64 for
    unwrap_or; ?I64 for is_some); the type checker bypasses
    the standard `CallArgMismatch` path when `Call.id` is one
    of the two known IDs and applies the ADR 0014 D9
    type-from-arg inference rule instead. The machinery
    evaporates at C1.7 when real generics arrive.

70. (C1.5.2-6 / 1d0adae) Codegen's `?T` representation is the
    LLVM struct `{ i1 valid, T payload }`. The widening
    `WidenToNullable` lowers as `insert_value` into a fresh
    `get_undef()` struct (set the i1 to true, fill in payload).
    `null` literal lowers as a const struct with i1 false + T
    zero. `unwrap_or` lowers inline as `build_select(valid,
    payload, default)` — both arms are unconditionally
    evaluated because they're already on the value stack;
    short-circuit semantics aren't needed (the builtin is a
    pure projection). `is_some` is just `extract_value(0)`.
    `x == null` extracts the valid bits from both sides and
    compares them as i1.

71. (C1.5.2-6 / 1d0adae) **Forward-reference fix in codegen pass 0**:
    the original C1.4 pass-0 code computed struct field types
    INSIDE the loop that inserts the struct into the struct_types
    map. For `struct Node { next: ?Node }`, computing the
    `next` field type requires looking up `Node` in
    struct_types — which fails because Node hasn't been
    inserted yet. The fix: two sub-passes. First, declare all
    opaque struct types via `context.opaque_struct_type(name)`
    + insert into struct_types. Then, in a second loop,
    compute field types (which can now see all struct names)
    and call `set_body`. The forward-reference path works for
    LLVM because struct types can be referenced opaquely before
    their body is set.

72. (C1.5.2-6 / 1d0adae) **ADR 0014 D10 deferral**: the
    cycle-check relaxation was documented in the ADR (cycles
    via nullable edges should be accepted) but the
    implementation choice of `?T = { i1, T }` inline makes
    recursive structs infinite-sized in LLVM. So C1.5 keeps
    the C1.4 cycle check unrelaxed — recursive structs via
    `?T` still error. Proper indirection (heap allocation,
    smart pointers) is C1.6+. The detect_struct_cycle docstring
    explains the deferral. ADR 0014 D10's text says "relaxes"
    but the runtime behavior says "deferred" — this is a known
    gap, documented in the docs commit's notes.

73. (C1.5.2-6 / 1d0adae) FnId remapping: with `unwrap_or` and
    `is_some` pre-registered alongside `print`, user fns now
    start at `FnId(3)` (was `FnId(1)` at C1.4). Updated 3
    existing resolve tests that hardcoded FnId(1)/(2) for
    user fns. Same shift in sentinel-types tests that asserted
    `fn_signatures[1].name == "main"` — now FnId(3) is main.
    Future C1.7+ generics will retire the builtins, shifting
    FnIds back; tests should not hardcode IDs.

74. (C1.6.1 / 3cfd49f) The C1.6 lexer additions are just two
    punctuation tokens: `[` (LBracket) and `]` (RBracket) per
    ADR 0015 D8. They serve three roles disambiguated by the
    parser at C1.6.2-6: array type `[T]`, array literal
    `[e1, e2, ...]`, and postfix indexing `a[i]`. No new
    keywords — `len` is a registered builtin per D4, not a
    reserved word. 6 new lexer tests.

75. (C1.6.2-6 / 8c5bbbe) **ADR 0015 D6 amendment: depth-1 type
    nesting.** The ADR proposed extending NullableInner with
    `Array(ArrayElem)` and ArrayElem with `Nullable(NullableInner)`
    to represent `?[T]` / `[?T]` combinations. Rust's mutual
    enum recursion forces Box indirection somewhere, which breaks
    `Type`'s Copy and cascades through the codebase. The
    implementation cap: NullableInner and ArrayElem stay as
    primitive-only subset enums (I64/I32/Bool/Struct). `?T` and
    `[T]` each contain primitives + structs only, never each
    other. `?[T]` and `[?T]` become "not yet representable" at
    C1.6; a future ADR adds them when generics or a more
    sophisticated representation is in place.

76. (C1.6.2-6 / 8c5bbbe) **Codegen value-type representation
    split for `?T`** per ADR 0015 D11. `?primitive` (I64, I32,
    Bool) stays as the flat `{ i1 valid, T payload }` from C1.5.
    `?Struct` switches to heap-indirect `{ i1 valid, ptr payload }`
    where payload points to a `sentinel_alloc`'d struct value.
    The asymmetry is the key insight: primitives are tiny and
    inline pays for itself; structs can be arbitrarily large and
    might be recursive, so the pointer indirection both bounds
    the parent's size AND breaks recursive struct cycles. The
    detect_struct_cycle pass relaxes to walk only direct
    `Type::Struct` edges, accepting cycles through `?Struct`.

77. (C1.6.2-6 / 8c5bbbe) **Runtime additions land in C1.6.**
    sentinel-runtime gains two new C-ABI symbols:
    `sentinel_alloc(i64 size) -> *mut u8` (libc malloc wrapper
    + abort on failure) and `sentinel_panic_oob(i64 idx, i64
    len) -> never` (print + abort on out-of-bounds index).
    Codegen pass 1 declares both as external `extern "C"` fns
    in the LLVM module; the actual implementations come from
    sentinel-runtime when the program is linked. NO `free`
    exposed — arrays + nullable struct payloads leak per ADR
    0015 D9. Documented limitation; C2 introduces region-based
    resource management.

78. (C1.6.2-6 / 8c5bbbe) **The fns HashMap "dummy entries" for
    inlined builtins.** Pre-C1.6 codegen aliased ALL is_runtime
    signatures to the same `sentinel_print` LLVM symbol (a
    latent name clash that worked only because LLVM's
    add_function with a duplicate name returns the existing fn).
    C1.6 fixes this: only `print` (FnId 0) gets a real LLVM
    declaration as `sentinel_print`; the inlined builtins
    (`unwrap_or` FnId(1), `is_some` FnId(2), `len` FnId(3))
    have fns HashMap entries pointing at print_fn as a sentinel
    value (never read because codegen special-cases their
    FnIds before calling `lower_call`). The runtime symbols
    `sentinel_alloc` and `sentinel_panic_oob` get their own
    declarations — they're called by codegen helpers, not by
    user code via the Call mechanism.

79. (C1.6.2-6 / 8c5bbbe) Array codegen lowers `[T]` to LLVM
    `{ i64 len, ptr data }`. Array literal allocates
    `n * sizeof(T)` bytes via `sentinel_alloc`, stores each
    element via GEP+store, then build_insert_values the
    `{ len, data }` struct. Indexing extracts both fields,
    bounds-checks (`0 <= idx < len`), branches: ok-branch does
    GEP+load, oob-branch calls `sentinel_panic_oob` then
    `build_unreachable`. The element type for GEP comes from
    `TypedExprKind::Index.elem_ty` (cached by the type checker)
    — LLVM uses opaque pointers since LLVM 15, so we track the
    payload type at the AST level.

80. (C1.6.2-6 / 8c5bbbe) The cycle-detector relaxation that
    closes ADR 0014 D10 is a one-line change: walk only
    `Type::Struct(_)` edges, not `Type::Nullable(NullableInner::
    Struct(_))`. The codegen change is more involved (the `?T`
    representation split per decision 76) but the type-check
    relaxation is trivial — the cycle is broken at the
    representation level, so the type checker can simply
    accept it.

81. (C1.6.2-6 / 8c5bbbe) Test FnId shift: user fns now start
    at FnId(4) because we have four builtins (print at 0,
    unwrap_or at 1, is_some at 2, len at 3). Updated 2 tests
    in sentinel-resolve and 1 in sentinel-types. Same caution
    as decision 73 — tests should not hardcode FnIds.

82. (C1.7 scaffolding / c1e5083) **No new lexer tokens at C1.7.**
    ADR 0016 D5: `<` and `>` from C1.3 comparisons get reused as
    type-param / type-arg delimiters. The parser disambiguates by
    position — `parse_type` looks for `<...>` after an Ident,
    expression-position grammar is unchanged. The cost is no
    turbofish (`f::<i64>(x)`) at call sites per ADR 0016 D4 —
    type-args are inferred bidirectionally. C1.7's parser change
    is bounded: parse_type_params, parse_type_args helpers + a
    Generic branch in parse_type + optional `<T1, T2>` clause on
    parse_fn_def / parse_struct_decl.

83. (C1.7.4a / d32a9fe) **TypeParam variants on the helper enums.**
    `NullableInner::TypeParam(TypeParamId)` and
    `ArrayElem::TypeParam(TypeParamId)` were added so `?T` and
    `[T]` are representable inside generic-fn bodies. This is the
    same flat-subset pattern from C1.5 / C1.6 (preserves `Type:
    Copy`); the TypeParamId is a `Copy` `u32` newtype re-exported
    from sentinel-resolve.

84. (C1.7.4a / d32a9fe) **Builtin typing routes through generics
    uniformly.** The three C1.5/C1.6 special-cased builtins
    (unwrap_or, is_some, len) had ad-hoc Call branches in
    check_expr that pre-computed T from arg[0] then pushed it
    down to arg[1]. C1.7.4a deletes ~75 LOC of those branches and
    re-expresses the builtins' signatures with real
    `Type::TypeParam` (e.g., `unwrap_or<T>(x: ?T, default: T) -> T`).
    The new unified `check_call` handles them identically to
    user-defined generic fns via iterative bidirectional
    inference. Code-path simplification + the right framing for
    user generic fns.

85. (C1.7.4a / d32a9fe) **Iterative call-site inference.** ADR
    0016 D4: bidirectional generic-call inference is implemented
    as a fixed-point loop. Each round, for each not-yet-typed
    arg, compute its effective expected type by substituting the
    param under the current `subst` (`try_substitute`). If the
    param has unbound TypeParams AND the arg is a null literal,
    skip that round — it'll be retried after another arg has
    bound the relevant T. After all args are typed, any unbound
    TypeParam surfaces `AmbiguousTypeArg`; any untyped null arg
    surfaces `AmbiguousNull`. The substituted return type is the
    call's `ty`. Handles e.g., `unwrap_or(null, 0)` correctly:
    arg[1]=0 → I64 → bind T=I64; arg[0]=null re-checked with
    expected `?I64` → typed.

86. (C1.7.5 / ad7e10d) **Eager monomorphisation via TypedFnDef::
    substitute.** Codegen materialises each `(FnId, type_args)`
    instantiation as a deep-cloned `TypedFnDef` with every
    `Type::TypeParam` substituted to its concrete type-arg. The
    substituted def has empty `type_params` and looks no
    different from a non-generic fn to `compile_fn`'s machinery
    — so codegen's per-fn lowering path stays unchanged. The
    eager-substitute approach (vs lazy substitution at every
    Type-access site) was chosen for safety: missing a single
    site in lazy substitution would silently emit wrong code,
    whereas eager substitution surfaces any TypeParam that
    leaks through llvm_basic_type's panic arm.

87. (C1.7.5 / ad7e10d) **Worklist for transitive monomorphic
    instantiations.** A user generic fn calling another user
    generic fn (`fn first_or<T>(?T, T) -> T { unwrap_or(...) }`
    works because unwrap_or is inline, but `fn foo<T>(x: T) -> T
    { bar<T>(x) }` requires `bar<concrete>` to be emitted when
    foo is monomorphised) needs the codegen pre-pass to
    transitively discover instantiations. The implementation is a
    worklist: seed from non-generic fn bodies, pop each pending
    instance, walk its body substituting type_args under the
    current subst, queue any new instances. Substitution may
    extend `instances` table (when nested generics produce new
    `Pair<i64, bool>`-style entries).

88. (C1.7.5 / ad7e10d) **Generic-fn name mangling.** Each
    monomorphic LLVM fn gets a deterministic mangled name:
    `id__i64`, `pick__bool__i64`, etc. Per ADR 0016 D7. Format
    is `{name}__{type1}__{type2}...` where each type is rendered
    by `mangle_type` (e.g., `Box_i64` for nested generics).
    Stable across runs given the same input program — useful for
    LLVM IR inspection. Internal-only; no surface implication.

89. (C1.7.4b / 2c6c652) **Interned generic-struct instances
    preserve `Type: Copy`.** ADR 0016 D6a. The naive approach
    (`Type::GenericInstance { struct_id, args: Box<[Type]> }`)
    would break Copy and force ~30 site .clone() refactors. The
    interned-id approach wraps each unique `(struct_id, args:
    Vec<Type>)` in a `Copy` `u32` newtype (`GenericInstanceId`).
    The underlying `Vec<Type>` lives in a program-level table
    (`TypedProgram.generic_instances`) keyed by id. Mirrors the
    StructId / FnId pattern. Linear search through the table at
    intern time — fine at C1.7 program scale; HashMap interning
    is a profile-driven future optimisation.

90. (C1.7.4b / 2c6c652) **Codegen extends the instance table
    during substitution.** The type checker populates
    `program.generic_instances` with every instance it sees
    (`Pair<i64, i64>` annotations, return types after call-site
    inference, etc.). Codegen owns a *mutable copy* of this
    table and may extend it during monomorphisation when nested
    generic substitution produces new `(struct_id, args)` pairs
    not seen by the type checker (e.g., `fn f<T>() -> Pair<T, T>
    { ... }` instantiated as `f<i64>` produces `Pair<i64, i64>`
    — same instance the type checker may or may not have seen
    directly). The abstract-vs-concrete filter in pass 0
    (`arg_contains_typeparam`) skips abstract instances (those
    with TypeParam args) from LLVM struct-type emission since
    they never materialize at runtime.

91. (C1.7.4b / 2c6c652) **Generic-struct unification recurses
    into args.** `unify_one(Type::GenericInstance(p_id),
    Type::GenericInstance(a_id), ...)` doesn't shortcut on
    `p_id == a_id` equality alone (two different abstract
    instances with the same shape would have different ids).
    Instead it looks up both data entries, verifies same
    `struct_id`, then unifies each arg pairwise. This is what
    makes `fst<A, B>(p: Pair<A, B>)` called with `p: Pair<i64,
    i64>` correctly infer A=i64, B=i64.

92. (C1.7.4b / 2c6c652) **Bidirectional pushdown extended for
    generic-instance returns.** The C1.5 ADR 0014 D5 rule pushed
    the return type down only for nullable returns. C1.7.4b
    extends to generic-instance returns too — without this, the
    body of `fn make_pair<A, B>(...) -> Pair<A, B> { Pair {
    first: a, second: b } }` would hit `AmbiguousGenericStructLit`
    because the literal can't synthesize its own type args.

93. (C1.7 retrospective) **ADR 0011 D6 estimated "4-6 weeks" for
    C1.7; actual was ~1 session across 5 commits (e411ded +
    c1e5083 + d32a9fe + ad7e10d + 2c6c652).** Faster than
    estimated, in line with the C1.4 / C1.5 / C1.6 pattern
    (estimated "2-4 weeks" each, actual ~1 session each). The
    speedup came from (a) the interned-instance design choice
    (avoided the Type-loses-Copy refactor cascade), (b) the
    eager-substitute approach for codegen (avoided the
    lazy-substitution audit risk), (c) the well-established
    pattern of "scaffolding commit → typing commit → codegen
    commit → fixtures commit" inherited from C1.4/5/6.

37. (C1.0c, ADR 0011 D1 amendment) Codegen stays outside the salsa
    query graph through Phase C1.0. ADR 0011's original D1 sketch
    had "… through codegen" in the query list, suggesting
    `compile_to_object` would eventually become `#[salsa::tracked]`.
    C1.0c reconsiders and rejects that for now. Three options
    were weighed (see ADR 0011 D1 amendment for the full
    write-up); the chosen option (2: don't wrap codegen at all)
    is justified by three factors: (a) codegen gets rewritten at
    C1.2 against typed HIR anyway, so investing in a pre-types
    salsa wrapper amortises over weeks at most; (b) the LLVM
    `'ctx` lifetime woven through `Context`, `Module<'ctx>`,
    `Builder<'ctx>`, and `FunctionValue<'ctx>` doesn't trivially
    fit salsa's `'static`-ish query model — fitting it requires
    either bitcode-roundtripping or single-fn codegen, and the
    cost-benefit isn't favorable yet; (c) the C1.0b front-end
    retrofit is what LSP / `cargo check`-style tooling actually
    cares about, since those tools exit after types-but-not-
    codegen — codegen incremental rebuild has near-zero practical
    value at C0/C1 scale. The cost is a small piece of explicit
    architectural debt (driver does a direct function call from
    parse_query's Program output into non-salsa codegen). It is
    revisited automatically at C1.2 because the codegen rewrite
    for typed HIR will touch the call site. ADR 0009 D1a's pure-
    function discipline preserved through C0+C1.0 keeps the
    retrofit mechanical whenever we do choose to do it. No code
    change at C1.0c; this is a pure docs commit capturing the
    architectural decision.

---

## Conventions

### Build & Test Commands

The standard check suite for a clean tree, applied per-crate:

    cargo build -p <crate>
    cargo clippy -p <crate> --all-targets -- -D warnings
    cargo nextest run -p <crate>           # or `cargo test -p <crate>`
    cargo test -p <crate> --doc

All four must pass for any commit on `main`. Current expected counts:

  - sentinel-broker:        69 tests + 1 doctest
  - sentinel-effects-proto: 226 tests (203 lib + 23 integration) + 0 doctests
  - sentinel-syntax:        195 tests (193 lib + 2 UI integration) + 0 doctests
                            (lib at C1.6: 186 lexer/parser + 7 query;
                             C1.5 added 6 lexer + 9 parser tests;
                             C1.6 added 6 lexer + 20 parser tests for
                             `[T]` array type / `[...]` literal / `a[i]`
                             indexing / nullable+array / nested array
                             parsing / error cases)
  - sentinel-ast:           42 tests (1 smoke + Display impls + op symbols);
                            C1.5 added 3; C1.6 added 4 (ArrayLit + Index +
                            array type Display + empty array)
  - sentinel-codegen:       41 tests (1 smoke + 1 target init + 39 positive
                            compile) + 0 doctests; C1.5 added 8; C1.6
                            added 9 (array literal, index, len, empty
                            array, array as fn arg, linked list,
                            nullable-struct widening, array of struct,
                            C1.6 phase-go)
  - sentinel-resolve:       36 tests (32 from C1.1-5 + 4 new C1.6:
                            array literal pass-through, array index
                            pass-through, len builtin pre-registration,
                            redefining len rejected) + 0 doctests
  - sentinel-types:         86 tests (71 from C1.2-5 + 14 new C1.6:
                            array type resolution + array literal typing
                            + array index typing + len builtin +
                            empty-needs-annotation + IndexOnNonArray +
                            IndexNotInt + NestedArray + mixed-element
                            rejection + len-on-non-array + array in
                            struct + linked-list-unlock + direct-cycle
                            still-rejected + C1.6 phase-go;
                            recursive_nullable_struct_now_accepted
                            replaces the C1.5 deferral test)
                            + 0 doctests
  - sentinel-driver:        47 pass integration tests + 0 doctests
                            (22 from C0 + 7 c13_* + 5 c14_* + 6 c15_* +
                            7 new c16_*: array_basic, empty_array,
                            array_as_arg, array_of_struct,
                            array_in_struct, linked_list_node,
                            c16_go_no_go)
  - sentinel-runtime:       2 tests (smoke + sentinel_print_returns_zero) + 0 doctests
                            — note: sentinel_alloc + sentinel_panic_oob
                            (C1.6 / ADR 0015 D9) are exercised via the
                            c16_* pass-test integration tests, not direct
                            Rust unit tests (they abort on failure paths)
  - sentinel-base:          3 tests (salsa query runs/caches + source file accessors) + 0 doctests
  - other compiler crates:  1 scaffold smoke test each, 0 doctests
                            (sentinel-hir, -mir, -lsp)

Total active workspace tests: **744**.

### Script Convention

Every code change lands via a script under `scripts/`:

  - `NN-<phase>.sh`          primary generator/patch for a milestone
  - `NNa-`, `NNb-`, ...      follow-up patches (lint fixes, etc.)
  - `NNz-commit-<phase>.sh`  pre-commit checks + commit creation

Scripts are committed alongside source changes so contributors can
see exactly what was patched, in what order. They are also a useful
debugging aid: re-run the most recent `NNa-` under `set -x` to
inspect a patch.

Output convention: each script prints `======` delimited sections
(BUILD / CLIPPY / TESTS / DOC TESTS). When asking for help, paste
those sections back.

### Working Norms (from HANDOVER §0.1)

- Trust STATE.md (this file), not the git log.
- Terminal heredocs: single-level only. Use `cat > /tmp/script.py`
  then `python3 /tmp/script.py` instead of nested heredocs.
- Avoid `set -e` and bare `exit` in pasted scripts — they can close
  the user's terminal. Use `return 1 2>/dev/null || exit 1` and
  rely on `PIPESTATUS[0]` for return codes.
- Small patches, build between each. Land type/trait changes first,
  build, then implementations, build, then tests.
- Honest disclosure beats confident-but-wrong.
- Examples held to `-D warnings`.
- Check before overwriting docs (`p.exists()` and merge patterns).

---

*End of document. Update on every commit that changes phase
status, public API surface, or invariants.*
