# HANDOVER.md — Sentinel Bootstrap Compiler Implementation

This document is the practical handover for starting work on the Sentinel
bootstrap compiler in Rust. It assumes you have read SENTINEL_DESIGN.md
and SENTINEL_DESIGN2.md and have decided to proceed with the staged
validation approach described in Section 16.3 of the design document.

Read this top to bottom once before writing any code. Then use it as a
reference as you work through the milestones.

---

## 0. Current Implementation Status

> **The bootstrap fixed point is reached — the Sentinel compiler compiles
> itself — and the per-unit separate-compilation back end is functionally
> complete.** This section used to carry a full milestone-by-milestone running
> log; that chronology is archived in [`HISTORY.md`](HISTORY.md). For the
> authoritative current state read [`STATE.md`](STATE.md); this section is the
> concise handoff + resume pointer. Sections 1–12 below are the durable
> implementation plan + working norms.

**Done**

- **Phases A–C** complete; **Sentinel 1.0** closed (2026-05-30) with
  machine-verified constant-time `secret` (`sentinel::mir::secret_leak`).
- **Phase D self-hosts** (ADR 0031–0045). The language build-out — sum types +
  `match`, strings + `u8`, growable `Vec<T>`, file I/O, `while`/`break`/
  `continue`, modules/`use` — plus the full compiler port to
  `selfhost/*.sentinel`, validated byte-for-byte against the Rust `snc` oracle.
  Full-corpus codegen parity is reached (all 123 pass fixtures); the bootstrap
  fixed point holds via both the merge-to-source and self-hosted-merge paths.
  The Rust `snc` remains the bootstrap seed + differential oracle.
- **Per-unit separate compilation** (ADR 0037 (a)) is functionally complete:
  `snc build --separate` → per-unit objects + module-qualified `abi-v1`
  linking, with every `pub` item kind crossing a boundary (incl. cross-**unit**
  `perform`/`handle`), `linkonce_odr` generic dedup (primitives + cross-module
  structs + enums), and item-granular incremental caching (`.o.fp` sidecars).
  Opt-in until full parity with the merge path (`snc merge` / `snc build`, both
  still green). Full detail in the `sentinel_separate_compilation` auto-memory
  + ADR 0037.
- **ADR 0046** (the partial-move-through-field double-free, the one historical
  borrow-check *under*-rejection) closed in both `snc` and `scg`.
- **External-review plan** (`docs/REVIEW_ACTION_PLAN.md`, untracked): the P0
  band, P1, P2 (bar P2.4), P3.1, and P3.2 (this STATE/HANDOVER split) are done.
- **Constant-time check audited** against the post-Phase-D language: SOUND — a
  secret reaching an `if`/`while` condition, a secret `&&`/`||`, a secret
  array/Vec index, a secret `match` scrutinee, or a secret divisor is all
  rejected. Added the `while` conformance test (`c52_secret_in_while`) and made
  the `SecretBranch` diagnostic name the actual construct.

---

> ⚠ **A PRE-EXISTING FRONT-END HOLE IN THE SECURITY CLASS was found while reviewing
> `4ac7bdc` and reported through the private channel per CONTRIBUTING.md — not described
> here, and NOT introduced by any commit in this session (it reproduces with no trait,
> impl or class anywhere). If you are picking this work up and have not been briefed on
> it, ask before touching the struct drop path.
>
> ⚠ **A SECOND PRE-EXISTING HOLE IN THE SECURITY CLASS was found on 2026-09-02 while verifying
> the D15 fix through inkwell, and is deliberately NOT described here** — same handling as the
> one above, per CONTRIBUTING.md. It is NOT the struct drop path and NOT introduced by any commit
> in this session (it reproduces with no generics, no traits, no impls and no classes; the
> compiler accepts it and the built program corrupts the heap at run time). It was found by
> running an investigator's probe, not by looking for it. If you are picking this up and have not
> been briefed, ask before touching the ARRAY drop path or reference deref-copy.
>
> ⚠ **D15/D16 (`45f231b`) HAD A FOUR-LENS REVIEW *AND* ITS VERIFY PHASE, and the review paid for
> itself twice over.** It found (i) a BLOCKER I introduced — the first field-substitution fixpoint
> walked every instance the vec grew by, on a termination argument I never constructed, and
> `struct A<T> { x: ?A<A<T>>, n: i64 }` stack-overflowed the compiler on source the previous
> release compiled; (ii) that the delta fix had turned a clean diagnostic into a panic on
> `impl as Get for Box`; (iii) that it made a known unbounded-instantiation hang newly reachable
> from method bodies, trading wrong output for a denial-of-service; and (iv) that BOTH new
> fixtures were VACUOUS. All are fixed and re-verified. **The two fixture failures are the ones
> to learn from**: an alpha route was masked because one line of the fixture SPELLED the instance
> the defect was supposed to create, and a three-root fixture collapsed to one monomorphic
> instance because all three roots used the same type. A fixture that exercises a defect at ONE
> type, or that names the thing it means to synthesise, tests nothing.
>
> ⚠ **ADR 0016 A1 HAD A FULL FIVE-LENS REVIEW *AND* ITS VERIFY PHASE** — the first slice in
> this sequence where both halves ran to completion. 18 findings were settled by
> construction: 11 REFUTED, 3 CONFIRMED-PRE-EXISTING, 4 CONFIRMED-INTRODUCED. **All four
> introduced ones were errors in my PROSE, not the code**, and all four are fixed. They are
> worth knowing because three of them are the same mistake in different clothes — asserting a
> consequence I had not constructed:
>   * "A1 adds TWO members to the collision class" + "primitive spellings shadow the struct
>     table, THEREFORE `struct f64 {}` can never reach the mangler". The shadowing is real in
>     TYPE position and irrelevant: the MONO-KEY route needs no spelling, only a value, so
>     `idg(f64 { z: 20 })` tags `f64` and collides. Six more members, constructed.
>   * "the `llvm-as` gate WOULD have fired pre-A1". It would have been SILENT — the gate fires
>     only when scg is rejected AND the oracle is clean, and pre-A1 BOTH were rejected. Only
>     the byte comparison would have caught it, and only because the two were wrong
>     DIFFERENTLY. **Read that as a standing property of the harness, not a one-off.**
>   * "no fixture is possible for `process`/`sealedchannel` (the mono-key route needs a real
>     subprocess)". Backwards: the runtime-false guard is what MAKES them fixturable, since a
>     tag is a compile-time property. Both are now pinned; the fixture covers eight tags.
> The review also found **D22** (a `?Guard` moved through a generic call leaks the lock on the
> not-taken branch) by attacking the new fixture itself, and **D23** (a latent `chan_secret`
> mangling collision A1 arms for M1.4c-2). Lens 2 independently swept all 339 corpus programs
> old-binary-vs-new and found exactly the two new fixtures changed, which is the strongest
> corroboration of the "no shipped symbol moves" claim.
>
> ⚠ **ADR 0023 A5 (`4ac7bdc`) had all four review lenses report**, and every substantive
> finding was verified by construction and acted on — including a mutation test proving the
> fixture pins the one load-bearing state reset. Its VERIFICATION phase was stopped.
>
> ⚠ **ADR 0023 A4 (`0fef8a4`) LANDED ON A PARTIAL REVIEW.** Its four-lens review ran and
> THREE lenses reported; every finding was verified by hand and acted on, and it earned its
> keep — the Path 1 default-impl dispatch bug came from it, as did four prose corrections
> including a false invariant. The FOURTH lens never returned and was stopped. What that
> leaves unexamined is unknown by definition, so if the trait/impl typing path misbehaves,
> re-run `wf_950fbfba-1c4`'s lens set against that commit.
>
> ⚠ **ADR 0072 LANDED WITHOUT THE ADVERSARIAL REVIEW.** The five-lens review
> (`wf_dc254647-26a`: over-refusal, boundary-holes, parity, correctness, prose) was launched
> and then STOPPED before any lens returned, at the maintainer's call, and the slice was
> committed on the four-check plus hand verification instead. Everything claimed for it below
> was checked by hand and is reproducible — but the change touches BOTH back ends and moves
> the oracle, which is exactly the class the review exists for. **If anything in the effects
> path looks wrong, suspect this first, and consider running that review retroactively.** The
> hand checks that did happen: four-check green (1811 passed / exactly the 18 known Windows
> failures); both bootstrap fixed points green; a sweep of every effect-bearing corpus program
> confirming nothing pre-existing is newly refused; oracle-vs-scg byte-equality on the new
> fixture at types, mir and llvm; and the secret-taint check in both directions.

### ▶ RESUME HERE (2026-09-03 — committed at `3b6bfa1`, four-check GREEN (**exactly the 18 known Windows failures**), both bootstrap fixed points byte-identical, tree CLEAN. This session closed **D17** (`6337bea`), **D31** (`6f57d6a`), **D34** (`b9c0aa7`) and **D24+D25+D26** (`3b6bfa1`), and DELETED two `DEFERRED_PROGRAMS` entries — the only proof this project accepts for that list. ⚠ **THE REGISTER'S OWN PRESCRIPTION FOR D24 WAS WRONG** and the deferred program could not have shown it: emission follows DISCOVERY order, not the LIFO drain, and a branching graph is what distinguishes them. Treat a register entry that names a fix as a hypothesis. ⚠ **THE REVIEWS KEEP FINDING REGRESSIONS THE FOUR-CHECK CANNOT** — D34 turned the ADR-0066-D8 secret fence FAIL-CLOSED→FAIL-OPEN, and D25's first cut added a spurious type decl to a program that had been byte-identical, with `llvm-as` silent on it. Both were found by a lens constructing an input, neither by any test. ⚠ A SECURITY-CLASS finding in the SHIPPING compiler is tracked privately with the maintainer and is deliberately not described here or in any commit — see the caveat block; **D35 is blocked behind it**. **NEXT:** D29 (the channel arms never `strip_secret` — small, and the direct sibling of D34's fence bug), or D36/D37/D39 (the container and generic `scg` gaps), or D33→D30 (f64/ptr, where D33 blocks D30). Newly filed: **D38-D41**.)

> **▶ THE OPEN MENU (2026-08-30) — real remaining work, verified against the repo.**
>   1. **`select` over channels** — the flagship concurrency gap. Its RUNTIME is already PINNED
>      (ADR 0066 D11: a Sentinel-owned `parking_lot` `Mutex<VecDeque<i64>>`+`Condvar`+waiter
>      registry replacing mpsc — ABI-safe, `SentinelChannel` is opaque). The SURFACE is OPEN and
>      must be decided BEFORE code: a select returns TWO things (which arm fired + the value)
>      where `?T` carries one; receive-only is complete while channels are unbounded; and it
>      should be pinned together with the `Sender`/`Receiver` split (D11) since that re-types it.
>      Design-first (ADR amendment → then build). The pinned queue also unblocks (2).
>   2. **`Channel<secret T>` (M1.4c-2)** — the SAME parking_lot queue rewrite unblocks it (today
>      it is gated on "in-transit values sit in mpsc nodes Sentinel does not allocate, so they
>      can be neither mlocked nor scrubbed" — under the Sentinel-owned queue it owns the nodes).
>      Typing side already unblocked: `channel_chanid_for` secret slots 6..=9 reserved. Decide
>      the memory policy, then implement. Natural to do WITH (1) since both need the queue.
>   3. **ADR 0058 f64 CODEGEN** — the front-end mirror is DONE (`339f437` + `3eeb34a`, ADR 0058
>      A8), so what is left is the back end, and it is a JOINT `snc`+`scg` slice rather than a
>      mirror: the `snc llvm` TEXT oracle itself Errs on every float literal
>      (`float literal not ported`) and on `llvm_ty(F64)`, which is precisely why the codegen
>      differential skips float programs today and why the front-end mirror needed no codegen
>      half. Doing it means teaching the text oracle `fadd`/`fsub`/`fmul`/`fdiv`/`fneg`/`fcmp`
>      (A5's predicate table: `oeq`/`olt`/`ole`/`ogt`/`oge` but `une` for `!=`),
>      `sitofp`/`fptosi`, `llvm.sqrt.f64` and a `double` constant — then mirroring all of it.
>      THREE traps are already signposted in `selfhost/` for whoever takes it: `scalar_bits(4)`
>      answers 64, so the Cast arm's width-equality test reads an int↔f64 cast as a no-op;
>      the THREE handle-dispatch chains in `cg_effects.sentinel` DEFAULT an unknown handle to
>      "pointer", so an f64 element would get `ptrtoint`/`inttoptr` (invalid IR on a `double`)
>      instead of `bitcast` — the spawn-wrapper return encode (`reth`), fid 22 `send`, and
>      fid 23 `recv`. ⚠ This used to say "the FOUR word-scalar chains" and the count was simply
>      wrong; a first attempt to correct it blamed D17 and narrowed the set to "the CHANNEL
>      chains", which was worse — it dropped the spawn-wrapper chain, the one whose own comment
>      carries the trap being signposted. D17 changed NOTHING in that file's chain set; what it
>      ADDED is a SEPARATE pair of CONTAINER helpers in `cg.sentinel` (`cg_container_encode` /
>      `cg_container_decode`), which strip the secret first and emit NOTHING for f64/ptr rather
>      than defaulting to pointer (see D30/D33 for why emitting nothing is the correct choice
>      until `cgo_ty` grows real `double`/`ptr` arms); and `cg_mangle_to` needed a real
>      scalar-4 arm, which it now has. Also worth doing with it: `examples/math/quadratic.sentinel` and
>      `sentinel_library/std/math/float.sentinel` currently reach only lex/ast, because
>      `snc merge`'s Bar-A printer rejects both a float literal and `sqrt` (menu item 5).
>   4. **THE FILED-DEFECT REGISTER — FORTY-ONE items (D1-D41); **D1, D2, D3, D5, D8, D9, D15, D16, D17, D24, D25, D26, D29, D31 and D34 are DONE**, the rest verified against a pre-slice binary. MOST are
>      unregistered in any `DEFERRED_PROGRAMS` / `KNOWN_SCG_BUGS` list because no corpus program
>      reaches them — but FOUR are, and the blanket "NONE" that stood here was falsified by
>      this register's own new entries: D24/D25/D26 share the
>      `examples/lang/generic_calls_generic.sentinel` entry, and D34 is
>      `examples/lang/process_channel_typed.sentinel`, so closing D34 DELETES a deferred entry.** They were also filed as task chips, but chips are EPHEMERAL UI
>      and this list is the record — if the two disagree, this one wins, and a chip that is
>      closed or superseded should be dismissed so the two do not drift. Ordered by what they
>      cost if left.
>
>      **D1 — DONE (`5609fb7`, ADR 0072).** `snc build` used to miscompile a qualified `let`
>      in an effecting fn, returning a raw pointer as the program's answer. Fixed, and the
>      shape now LOWERS rather than merely being refused. The audit that cleared it found the
>      same hole had five more faces — a silently DROPPED effect, a 7-byte OOB read off a
>      narrow captured param, an inkwell panic, a verifier abort, invalid IR at exit 0 — all
>      closed by the same gate. Four scg gaps it exposed are filed separately (see the chips).
>
>      **D2 — DONE (`bd1e36c`).** ELEVEN arms, not the eleven this entry listed and not the
>      ten I first counted: the entry's list omitted `Handle`, which an adversarial review
>      found, and included `Spawn`, which does not diverge. The fix is NOT the "same
>      three-line fix shape" per arm this entry prescribed — probing each arm first turned
>      up a better one. An arm that ALREADY widens returns the EXPECTED type, and
>      `widen_kind` compares the expectation's INNER type against the node's, so it answers
>      0 and the splice degenerates to a copy. ONE splice at the expression dispatcher
>      therefore covers every arm, leaves the correct ones untouched, and covers any arm
>      added later — gated on a cheap `widen_possible` so the hottest recursion only pays
>      for a temp where the expectation is a `?T`/`secret T`.
>      ⚠ The review caught the first version wrapping DIVERGENT nodes: the oracle skips
>      `coerce_to_expected` when `expr_diverges`, so `let v: secret i64 = if c { 42 } else
>      { return 1 };` must leave the `return` bare. `dump_texpr` now consults a divergence
>      flag, set only by `Return` — measured, not narrowed: the oracle REFUSES every
>      program that would put a fully-diverging block/`if`/`match` in a widened position.
>      The ADR 0051 A1 ARRAY-ELEMENT widen stays deferred (`widen_kind` has no array arm;
>      0 widens before and after). Pinned by `tests/pass/c19_widen_arm_family.sentinel`.
>      Original entry:
>      
>      **(historical) D2 — the widen FAMILY. ⚠ WORSE THAN ITS ORIGINAL FILING: not a text divergence.**
>      At a `let` these arms cost only the wrapper, but at the ASSIGN and RETURN positions
>      D3 threaded, they cost INVALID IR — the store/`ret` is rendered from the target type
>      while the operand comes back un-widened. Measured: `o = pv.f` with `o: ?i64` gives
>      `store { i1, i64 } %v4` with `%v4 : i64`; llvm-as rejects it and the oracle's IR for
>      the same program is clean. Pre-existing (a pre-D3 stage emits identical bytes), but
>      it carries the same severity that justified closing D3, so it is no longer a
>      cosmetic follow-up. `Field`, `Index`, `Method`, `Array`, `StructLit`, `Declassify`,
>      `Perform`, `Await`, `Qcall`, `ClassInit`, `Spawn` all take a real `exp` and discard it,
>      exactly as `Call`/`Unary` did before `34e1a8f`. `let v: secret i64 = p.x;`, `= a[0];` and
>      `= declassify(s);` diverge in one six-line program; at MIR the dropped `opaque` shifts
>      every later value number. Same three-line fix shape as `34e1a8f`; probe each arm for
>      OVER-widening rather than batching them blind.
>
>      **D3 — DONE (`28dd77d`, ADR 0051 A5 + the ADR 0014 D3 mirror).** A call ARGUMENT, a
>      `return` OPERAND and an ASSIGNMENT right-hand side supplied no expected type, so the
>      widen wrapper was dropped in each; all three were shape-independent, and the
>      assignment one emitted INVALID IR (`store { i1, i64 } 42`). A1 had deferred the
>      `secret` call-arg/return widens by design — A5 amends that, because the same missing
>      machinery also dropped ADR 0014 D3's `?T` pushdown, which nothing ever deferred.
>      A1's ARRAY deferral stands. ⚠ THREE THINGS THIS MAKES VISIBLE WITHOUT FIXING, all
>      filed and all proved unchanged by building a pre-change stage and diffing:
>      (i) the sibling ARM family (D2), now reached through arguments and assignments —
>      and NOT cosmetic there: at a `let` it costs only the wrapper, but at the ASSIGN and
>      RETURN positions it costs INVALID IR (`o = pv.f` with `o: ?i64` gives
>      `store { i1, i64 } %v4` where `%v4 : i64`), because the store is rendered from the
>      target type while the operand comes back un-widened;
>      (ii) a BUILTIN's argument, which the oracle DOES widen and scg does not, emitting
>      `extractvalue i64 5, 0` — invalid IR, and a retraction of a claim this session wrote
>      into an ADR and three comments (see STATE);
>      (iii) every argument path except a plain `f(x)` — method, impl-method, qualified,
>      class `init`, enum construct, `spawn`, and generic — where the oracle widens a plain
>      literal and scg does not.
>
>      **D4 — a generic USER fn inverts the widen.** The oracle seeds `T` from the EXPECTED type
>      (`unify_one(signature.return_type, exp, …)` in `check_call`) and widens the ARGUMENT, so
>      `let s: secret i64 = idg(7)` monomorphises at `secret i64` with no outer wrapper; scg
>      monomorphises at `i64` and wraps outside. `34e1a8f` improved it (pre-fix scg emitted
>      `store { i1, i1 } %v0` with `%v0` an `i1`, which `llvm-as` rejects; post-fix IR assembles)
>      but it still diverges. Fix = thread `exp` into `dump_generic_call`.
>
>      **D5 — DONE (ADR 0016 A1, the ADR's first amendment).** The mangling scheme D7 left
>      "TBD in implementation" is now pinned, structural (a tag names the type's SHAPE, never
>      an interner index), exhaustive, and identical in all three back ends. Table in
>      `docs/abi-v1.md` §4. Pinned by `examples/lang/phantom_type_param.sentinel` (the phantom
>      route, 20 markers, byte-identical + `llvm-as`-clean + runs) and
>      `tests/pass/c16_mono_key_handle_tags.sentinel` (the mono-key route).
>
>      **What the entry got right:** the oracle's fallback was UNPARSEABLE, not merely
>      unstable, so the oracle had to move first; and mirroring by enumeration only became
>      possible once it did.
>      **What it got wrong, and both errors are the same shape — a claim generalised from one
>      instance:** (i) it scoped the defect to two container kinds; ELEVEN variants reach the
>      fallback from real source, and `Class`/`Enum` do it with no concurrency feature at all.
>      (ii) it described one route; there are two, and the second — a generic fn's MONO KEY,
>      `idg(v)` on `fn idg<T>(x: T) -> T` — is the only way `Task`, `Process` and `Guard`
>      reach a tag, since none is writable in type position. It corrupts a FUNCTION SYMBOL,
>      not a type declaration. A fix validated only against the `Pair<i64, X>` phantom shape
>      would have missed four variants.
>      Also closed here: `scg`'s `Shared`/`Mutex` type-position arms are element-generic
>      (they discarded the element, so two distinct oracle instances interned to ONE scg
>      handle — a structural collapse, not a naming one).
>      **⚠ A1 knowingly WIDENS the tag/user-name collision class (D18) in the two PRINTING
>      back ends.** The oracle's old tags contained PARENTHESES (`ty_Shared(SharedId(0))`),
>      which are illegal in a Sentinel identifier, so they were accidentally UNFORGEABLE;
>      every tag A1 introduces or renames is a legal identifier and therefore forgeable.
>      Constructed: `struct process {}` beside a `Process` value now emits `@idg__process`
>      twice, and `struct f64 {}` beside a real `f64` value makes scg emit `@idg__f64` twice
>      (pre-A1: `@idg__f64` + `@idg__ty_F64`). The review also refuted the tempting defence —
>      primitive spellings shadow the struct table in TYPE position, but the MONO-KEY route
>      needs no spelling, only a value, so nothing is safe by shadowing. Accepted because
>      inkwell already used every one of these spellings (no shipped behaviour moves; LLVM
>      uniquifies) and the alternative is the printing back ends disagreeing forever with the
>      one that ships — against which A1 removes a TEN-way UNCONDITIONAL collision and every
>      unparseable name.
>
>      **D6 — scg OVER-ACCEPTS `{ …; return e; }`**, which `snc`'s parser rejects ("blocks must
>      end with an expression"), and synthesizes a `0` tail out of nothing. Worth more than its
>      size, because of what it exposes: **every differential skips a program the ORACLE rejects,
>      so the whole class "scg accepts what snc rejects" is untested BY CONSTRUCTION.** A
>      `tests/ui` guard asserting the self-hosted stage also fails would close the class.
>
>      **D7 — `resume-kont` is typed from the op's DECLARED result** rather than from the handled
>      computation, so a `secret i64`-returning effecting fn gives `:i64` where the oracle gives
>      `:secret i64`. Fix site: the resume-kont branch of `dump_te_call`,
>      `selfhost/types/borrow_arms.sentinel` ~997-1002. No effects fixture has a
>      `secret`-returning effecting fn, which is why nothing saw it.
>
>      **D9 — DONE (`4ac7bdc`, ADR 0023 A5).** The two Rust back ends disagreed about
>      whether `impl as Trait for <struct>` was compilable: inkwell compiled and ran it,
>      the TEXT oracle refused it, and `selfhost_codegen` skips whatever the oracle
>      errors on — so the codegen differential could never see the shape. The refusal was
>      SELF-FULFILLING (its "every corpus impl targets a class" justification was true
>      only because the refusal kept such fixtures out). Removing it un-hid an abort in
>      scg's codegen, so both halves landed in one commit; oracle and scg are now
>      byte-identical on the shape and the corpus is 168 emitted / 0 diverged.
>
>      **D8 — DONE (`0fef8a4`, ADR 0023 A4).** scg mishandled a trait impl whose target is a
>      STRUCT. Three symptoms, not the two reported: the dump printed `class#` with no id, a
>      `self.<field>` read ABORTED the stage, and — found by probing — receiver-typed
>      dispatch keyed on the class id alone made the FIRST struct impl match EVERY struct
>      receiver, which is correct by accident with one struct in the program and wrong with
>      two. Front end only: `snc build` compiles these but `snc llvm` refuses them, so the
>      codegen differential cannot see the shape (filed separately).
>
>      **D10 — ⚠ `snc llvm` emits an invalid fall-through for a NULLABLE return.** ANY
>      `return` in a fn whose return type is nullable makes the ORACLE emit a bare `0` into
>      the `{ i1, i64 }` merge slot — `fn rn() -> ?i64 { return 5 }` alone reproduces it, the
>      same fn without a `return` is clean, and the `secret i64` twin is clean (secret lowers
>      to i64). `llvm-as` rejects it; `snc build` (inkwell) compiles the same program
>      correctly, so this is text-oracle-only. NOTHING CATCHES IT: `selfhost_codegen`'s IR
>      check flags scg only when the ORACLE's IR is valid (deliberately — so scg is not blamed
>      for reproducing the oracle), and here both are invalid, so the differential stays green
>      and silent. Consider adding an assertion that the ORACLE's own IR assembles over the
>      fixtures it emits for; that whole class has no test today.
>      `tests/pass/c19_widen_arg_return_assign` contains the shape and documents it.
>
>      **D11 — a BUILTIN's call argument is not widened.** The oracle widens it and scg does
>      not, emitting `extractvalue i64 5, 0` — invalid IR. `let y: i64 = unwrap_or(5, 0);`
>      reproduces it with a bare literal. ⚠ A bare `unwrap_or(5, 0)` in TAIL position is
>      rejected by the oracle for an unrelated reason (generic inference with no context); an
>      earlier session generalised from that and wrote "the oracle does not widen a builtin's
>      argument" into an ADR and three code comments before having to retract it — do not
>      repeat that. ADR 0051 A5 threaded USER-fn params only; the builtin arms infer their type
>      argument from the ARGUMENTS rather than from an expectation, which is the same shape as
>      D4, so this needs its own probe battery for over-widening. `secret` is affected too and
>      SHIPPED code relies on it: ADR 0052 describes `push(&mut v, i64_to_u8(...))` as using
>      "the ADR 0051 call-arg widen", which scg does not perform.
>
>      **D12 — four scg effecting-path gaps**, all invisible today because ADR 0072 made the
>      oracle refuse the programs that would expose two of them. (a) the c35d embedded-perform
>      shape is not mirrored for a `secret`-returning fn (oracle 2 resumers, scg 0) — this one
>      IS differential-reachable and green only for want of a fixture; (b) scg captures ALL
>      params in declaration order where the oracle captures the tail's free vars in
>      first-reference order, so an unused param makes scg over-capture; (c) `cg_tailk` is a
>      sticky flag that nothing clears when a tail is pure, so `do_work(); 5` emits
>      `ret ptr 5`; (d) the `?i64` let-shape stores 8 bytes into a 16-byte slot, leaving the
>      discriminant uninitialised — and `llvm-as` ACCEPTS that, so it is valid IR with garbage
>      semantics, a worse failure mode than the oracle's. ⚠ Do NOT "fix" (d) by falling through
>      to `cg_eff_normal`; that path reproduces the exact miscompile ADR 0072 removed.
>
>      **D13 — an effecting METHOD has no `Kont*` ABI at all.** `dump_method` never consults
>      `uses_kont_abi`, so a class/impl method with an effect row is emitted with the ordinary
>      value ABI while its body performs — `Box__fetch` is declared to return `i64` and then
>      does `ret i64` on a `Kont*`. A separate path from the one ADR 0072 fixed, and the reason
>      `expr_suspends` deliberately does NOT treat method calls as suspending: under today's
>      ABI they really do return a value. Decide first whether to give methods the ABI or to
>      refuse an effect row on a method until that exists. Zero corpus programs have one.
>
>      **D14 — scg's codegen aborts on a ZERO-FIELD struct.** `struct E { }` with
>      `let e: E = E { };` and no impl anywhere: the oracle emits valid IR, scg exits 127 on an
>      out-of-bounds index. Differential-reachable — any tests/pass fixture with an empty
>      struct turns it red — and green only because no corpus program declares one.
>
>      **▸ D15-D21 were all found DURING the D5 work (2026-09-02) and are all verified by
>      construction. Two of them are CRASHES IN THE SHIPPED COMPILER on ordinary source,
>      which makes them worth more than their position in this list suggests.**
>
>      **D15 — DONE (`45f231b`).** Four defects, not one, and both halves of the filed
>      diagnosis were too narrow: the boundary is "an instance the TYPE CHECKER never
>      interned", it has TWO producers (mono-fn BODY substitution AND Pass-0 generic-struct
>      FIELD substitution, which needs no generic fn at all), and NESTING is not required —
>      a plain `Box<i64>` trips it. A fourth, SILENT member (defect δ: class init / class
>      method / impl method were not monomorphisation roots, so a generic fn reached only
>      from a method body was called and never defined) came out of the same worklist.
>      ⚠ The adversarial review found the FIRST version of the fix regressed working
>      programs — its termination comment claimed a recursive-by-value generic struct
>      "cannot exist", and `struct A<T> { x: ?A<A<T>>, n: i64 }` falsifies it in four lines
>      (finite layout, recursion behind a pointer), stack-overflowing the compiler. The
>      closure now walks only BY-VALUE fields and is capped; the mono worklist is capped
>      too, which also closed the pre-existing unbounded-instantiation HANG. Original entry:
>      
>      **(historical) D15 — a generic fn returning a GENERIC INSTANCE crashes `snc build`.** Six lines,
>      no phantom, no containers, no concurrency:
>      `struct Box<T> { v: T }` / `fn inner<T>(x: T) -> Box<T> { Box { v: x } }` /
>      `fn outer<T>(x: T) -> i64 { let b = inner(x); 7 }` /
>      `fn main() -> i64 { let b1: Box<i64> = Box { v: 1 }; outer(b1) }`.
>      `snc build` panics at `crates/sentinel-types/src/lib.rs:2756` (index out of bounds,
>      len 2 index 2); `snc llvm` panics at `crates/sentinel-driver/src/llvm_dump.rs`'s
>      `llvm_ty` GenericInstance arm with the same shape; `scg` does not crash but emits
>      `call %Box_Box_i64 @inner__Box_i64(...)` for a function it never defines — `llvm-as`:
>      "use of undefined value '@inner__Box_i64'". Diagnosis (from the review, not yet
>      confirmed by a fix): substitution EXTENDS a CLONED copy of `program.generic_instances`
>      (llvm_dump.rs ~652/685 clone it) while `llvm_ty`/`mangle_type` keep indexing the
>      ORIGINAL, so the instance created by substituting `Box<T>` at `T = Box<i64>` has no
>      entry. **The highest-severity item on this list: a panic on ordinary generic code in
>      the compiler that ships.** Loud rather than silent, which is the only mercy.
>
>      **D16 — DONE (`45f231b`).** THREE unguarded callers of `ArrayElem::to_type()`, not
>      two — `mono_args_dedup_safe` was the third, and it is the one that reaches cross-unit
>      dedup. All now recurse through the `arrays` interner. `scg` already emitted the right
>      tag, so the fix moved the two Rust back ends TO it. ⚠ The shortcut the sibling
>      `contains_type_param` uses rests on a FALSE premise — `struct Box<T> { v: [[T]] }`
>      type-checks and the dump prints `[[<T#0>]]`; what is true is only that it cannot be
>      INSTANTIATED. Original entry:
>      
>      **(historical) D16 — a NESTED ARRAY as a generic type argument crashes both Rust back ends.**
>      `struct Box<T> { v: T }` + `let b: Box<[[u8]]> = Box { v: ["a"] };` panics at
>      `crates/sentinel-types/src/lib.rs:669`, `ArrayElem::Array` reaching `to_type()`'s
>      `unreachable!("use TypedProgram::array_elem_type (ADR 0068)")`. The phantom form
>      (`Pair<i64, [[u8]]>`) and `&[[u8]]` panic identically. Two call sites reach it:
>      `llvm_dump.rs`'s `type_has_typeparam` and `sentinel-codegen`'s
>      `arg_contains_typeparam`, both of which call the bare `ae.to_type()` instead of
>      `program.array_elem_type(ae)`. `scg` handles it correctly today (`arr_arr_u8`), and
>      the oracle's post-fix tag would be `arr_arr_u8` too — so the fix gains parity for
>      free. ⚠ `to_type()` on an `ArrayElem` has SEVEN more callers
>      (`mono_args_dedup_safe`, `contains_type_param`, `try_substitute`, `unify_one` x3,
>      `impl Display for Type`) — grep the SET before declaring it closed; the `Display` one
>      means a diagnostic that formats a `[[T]]` may panic too.
>
>      **D17 — DONE (`6337bea`).** `scg` emitted no width conversion around a container's
>      runtime call, so a non-`i64` container ELEMENT produced invalid IR. FOUR sites (this
>      entry previously named no count at all): the encode before `shared_new` (fid 38) and
>      `mutex_new` (fid 40), the decode
>      after `shared_get` (fid 39), and — in a DIFFERENT FILE, which is why it is easy to
>      miss — the decode after the `load` in the `*g` guard-deref READ
>      (`borrow_arms.sentinel`). `lock` (fid 41) needs nothing: its `?Guard` payload is the
>      cell HANDLE (a ptr) for every element, verified over seven element types. Two new
>      helpers, `cg_container_encode` / `cg_container_decode`, which **strip the secret
>      FIRST** — the detail a verbatim copy of the channel arms gets wrong, since `cgat`
>      holds a type HANDLE and a `secret u8` element matches no width arm.
>
>      **`f64` and `ptr` deliberately get NO arm**, which is the opposite of a faithful
>      mirror and is the finding the review paid for. `cgo_ty` has no arm for scalar code
>      4 or 5 — both render `i64` — so spelling them out emits `ptrtoint i64 %v to i64`,
>      which `llvm-as` rejects. A first cut DID spell them out; `fn take(s: Shared<ptr>) ->
>      ptr { shared_get(s) }` emitted clean `ret i64 %v2` BEFORE and non-assembling IR
>      AFTER. Do not re-add the arms before `cgo_ty` grows real `double`/`ptr` arms.
>
>      Measured: a **347-program sweep** against a pre-change `scg` binary moved EXACTLY
>      the two new fixtures, both DIFF → byte-identical-to-the-oracle; **14 mutations, 13
>      killed**, all four wired sites individually covered, and neither new fixture
>      redundant (the constant-operand mutation is killed only by `c71_container_widths`,
>      the `strip_secret` one only by `c71_secret_container_widths`). The lone SURVIVOR is
>      "give the f64/ptr fall-through an arm" — nothing in the corpus catches it, which is a
>      third missing pin. Fixtures:
>      `tests/pass/c71_container_widths`, `tests/pass/c71_secret_container_widths`,
>      `tests/ui/c71_shared_element_unsupported`.
>
>      ⚠ **ONE COST, DISCLOSED:** `*g = v` on a non-`i64` element emits a NARROW STORE
>      into the 8-byte slot. That store is PRE-EXISTING (the pre-change binary emits the
>      same line) but D17 removed an unrelated `mutex_new` invalidity that had been masking
>      it, so the shape is now SILENTLY wrong where it was LOUDLY wrong. See D28.
>
>      **D18 — the tag/user-identifier collision class (`abi-v2` encoding work).**
>      A mangled tag shares one flat namespace with user type names: `struct arr_i64 {}` tags
>      `arr_i64`, exactly like `[i64]`, so `Pair<arr_i64, i64>` and `Pair<[i64], i64>` mangle
>      identically — the oracle emits one `%Pair_arr_i64_i64` for two DIFFERENT layouts
>      (`{ %Struct.0 }` vs `{ { i64, ptr } }`), and `scg` does the same. PRE-EXISTING (it
>      predates every handle tag; `opt_`, `sec_`, `vec_`, `ref_` all reproduce it) and made
>      easier by the fact that `mangle_instance` joins type arguments with a SINGLE `_`, which
>      a multi-token tag can already contain — so the boundary between "where one tag ends"
>      and "where the next begins" is not recoverable from the name. That half is pre-existing
>      too (`arr_`/`opt_`/`sec_` are multi-token), confirmed by a review lens against a
>      pre-change binary. **Not closable by enumeration**: `_` and `$` are both legal Sentinel identifier
>      characters (`[A-Za-z_][A-Za-z0-9_$]*`), so no separator built from them is
>      unforgeable. `.` is the one character LLVM accepts in an unquoted name that the lexer
>      cannot produce, so a `.`-separated or length-prefixed encoding is the fix — and it
>      moves every existing tag, hence `abi-v2` (cf. ADR 0029 D8, and the `__` soft-spot
>      already noted in `abi-v1.md` §4). ⚠ The SHIPPED back end is unaffected: LLVM
>      uniquifies a duplicate type or function name, and the colliding program builds and
>      runs correctly (verified end to end). It is the two PRINTING back ends that emit
>      invalid IR. A1 added `process`/`sealedchannel` to this class deliberately — see D5.
>
>      **D19 — the `linkonce_odr` dedup's `$`-separator justification was FALSE** (comments
>      corrected in the A1 commit; NO code change, and none is safe without thought).
>      `mangle_type_dedup` `$`-joins a module path onto a type name and its comment claimed
>      `$` "can't appear in a bare type name or path segment, so distinct origins never
>      collide". The lexer accepts `$` inside an identifier — added deliberately so merged
>      source round-trips — so `struct util$geo$Point` compiles and tags identically to
>      module `util::geo`'s `Point` (verified by compiling it), and module `util`'s
>      `struct geo$Point` would too. What actually keeps two distinct instantiations off one
>      `linkonce_odr` symbol is `mono_args_dedup_safe` requiring a KNOWN ORIGIN, not the
>      separator. **So that gate is load-bearing and must not be widened on the old belief** —
>      and the ABI doc already contemplates widening it ("class / generic-instance args +
>      trait/class-method dedup remain the deferred tail"). Same species as the
>      `is_process_channel_elem` bug: a fence resting on a false premise about the grammar.
>      NOT constructed end-to-end (a two-module `--separate` build; the four
>      `separate_*_linkonce_*` tests fail on Windows at `link.exe` for unrelated reasons), so
>      the aliasing is shown at the TAG level only — that much is certain from the code.
>
>      **D20 — `scg` has no unknown-type diagnostic: an unrecognised type name in type
>      position silently resolves to handle 0 (`i64`).** Verified with `Bogus`, `Task<i64>`,
>      `Process`, `Guard<i64>`, bare `Vec`. Normally masked because the oracle refuses first
>      and the differential skips whatever the oracle rejects — the D6 blind spot, in a new
>      place. It is LIVE for the two spellings the oracle ACCEPTS: `Pair<i64, u128>` and
>      `Pair<i64, SealedChannel<secret i64>>` both collapse onto a genuine `Pair<i64, i64>`
>      instance, so `scg` emits ONE declaration where the oracle emits two. Both sides emit
>      VALID IR — they simply disagree — which makes this the quiet member of the D5 family
>      and the one no `llvm-as` gate can catch. `u128` needs `scg` to model `u128` at all
>      (`scalar_code` has no arm); `SealedChannel` needs a `type_of_typeexpr` arm and is the
>      pre-existing ADR 0069 deferral behind three `DEFERRED_PROGRAMS` entries.
>
>      **D21 — `__spawn_wrapper_<FnId>` embeds a source-ORDER-dependent id in an
>      unqualified symbol.** Verified: adding one unrelated `fn` before the spawn target
>      moves the symbol from `@__spawn_wrapper_42` to `@__spawn_wrapper_43`. All three back
>      ends agree (both compilers compute the same FnId), so no differential sees it; the
>      hazard is cross-unit, where `mangle_qualified` is NOT applied
>      (`crates/sentinel-codegen/src/lib.rs:1230` formats the name inline) and two units each
>      holding a spawn target at FnId 42 would both define it. Under default linkage that is
>      a duplicate-symbol LINK ERROR rather than silent wrong code — NOT verified end-to-end.
>      It is a 22nd naming site that the mangling census missed because it is built by an
>      inline `format!` containing no "mangle" token.
>
>      **▸ D22-D23 came from the ADR 0016 A1 ADVERSARIAL REVIEW rather than the
>      implementation, and neither is a mangling defect — the review found them by
>      attacking the new FIXTURES.**
>
>      **D22 — moving a `?Guard` into a call leaks the lock on the not-taken branch.**
>      The scope-exit unlock is emitted only on the branch that performed the move:
>      `let cond: bool = false; let m: Mutex<i64> = mutex_new(2); let g = lock(m);`
>      `let r: i64 = if cond { let h = idg(g); 2 } else { 0 }; r + 42`
>      borrow-checks CLEAN, and the `else` path holds an acquired lock with no
>      `sentinel_mutex_unlock` anywhere on it. PRE-EXISTING — a pre-A1 `scg` emits the
>      identical unlock structure (verified; the two binaries differ only in the
>      mangled name) — and it sits inside ADR 0071's deliberately deferred no-escape
>      tail, which pinned `lock()` to a direct `let` RHS (`GuardNotLetBound`) but never
>      constrained what happens to the binding afterwards. A generic call is the
>      easiest way out: `idg(g)` needs no `unwrap`, no borrow and no deref, so none of
>      the three existing guard pins fires.
>      ⚠ `tests/pass/c16_mono_key_handle_tags.sentinel` is the FIRST corpus program to
>      move a guard through a generic call. Its shape is sound — the condition IS
>      `is_some(g)`, so the `else` arm is by construction the null-guard arm where no
>      lock was ever taken — and the fixture now says so at the site, precisely so it
>      is read as a warning rather than as a licence to copy the shape with some other
>      condition. Fixing D22 properly means either a conditional-move drop (the guard
>      dropped on the paths that did NOT move it) or extending the pin to refuse the
>      move; the second is cheaper and matches the existing conservative posture.
>
>      **D23 — `channel_elem_for`'s reserved secret slots answer the PUBLIC scalar, and
>      A1 gave that map a second consumer.** Slots 6..=9 are the M1.4c-2
>      `Channel<secret T>` reservation and map to `I64/I32/U8/Bool`, deliberately, so
>      the map agrees with the placeholder rows in the `channels` table. Unreachable
>      today. But `mangle_type` now reads the element from here to build `chan_<tag>`,
>      so the moment M1.4c-2 makes those slots reachable, `Channel<secret i64>` tags
>      `chan_i64` — the same name as `Channel<i64>`, a silent collision of exactly the
>      class A1 exists to remove. The sibling maps are already correct:
>      `shared_elem_for` / `mutex_elem_for` / `guard_elem_for` route the secret range
>      through `secret_elem_for_slot` first and yield `shared_sec_i64` (verified by
>      construction on all eight secret containers). NO code change here — the slots
>      are unreachable, so changing them now would be untestable — but the site now
>      carries the warning, and **M1.4c-2 must make `channel_elem_for` match its
>      siblings, not merely "change it together with the table".**
>
>      **▸ D24-D27 came out of the D15/D16 work (2026-09-02). D24-D26 are all `scg`, all
>      verified on `examples/lang/generic_calls_generic.sentinel`, which is registered in
>      `DEFERRED_PROGRAMS` naming all three. D27 is the Rust back ends and is the fourth
>      face of D15's defect δ.**
>
>      **D24 — DONE (`3b6bfa1`).** `scg`'s mono worklist had ZERO transitive closure: a
>      generic reached only through another generic's body was called and never defined.
>      Closed with TWO passes — discovery under a LIFO stack with output muted, then
>      emission in DISCOVERY order.
>
>      ⚠ **THIS ENTRY'S OWN PRESCRIPTION WAS WRONG** and is worth reading as a lesson
>      about the register rather than deleting. It said "turn the mode-4 loop into a real
>      worklist — it must be LIFO". That is not sufficient and not quite right: the
>      oracle keeps TWO structures. `order` is appended when an instance is first
>      DISCOVERED and is what gets emitted; `pending` is a LIFO stack that only decides
>      which body is re-walked next. Measured on `main->g1,g2; g1->h1; g2->h2`, the
>      oracle emits `g1, g2, h2, h1` while the body-walk order is `g2, h2, g1, h1`. A
>      single emitting pass CANNOT produce the former. Following the entry would have
>      passed the deferred program — a single chain, where all three candidate orderings
>      coincide — and silently reordered branching ones. **A register entry that names a
>      fix is a hypothesis; construct the discriminating input before trusting it.**
>
>      **D25 — DONE (`3b6bfa1`).** `scg` took no generic-struct FIELD closure, emitting
>      `%Wrap_bool = type { %Box_bool }` while never declaring `%Box_bool`. Laying out
>      `Wrap<bool>` CREATES `Box<bool>` — `subst_type` interns — and the emission loop
>      had snapshotted its bound. Closed with an explicit closure pass that chases only
>      BY-VALUE fields, **filtered on the DECLARED type**. ⚠ Filtering AFTER substitution
>      is the trap: it is what the ORACLE got wrong when D15 first added its closure, its
>      comment says so, and the first cut of this fix re-made it in the mirror — adding a
>      spurious `%Box_bool` to a program that had been byte-identical, with `llvm-as`
>      silent so the validity gate could not catch it.
>
>      **D26 — DONE (`3b6bfa1`).** `scg` emitted `%Struct.N = type { i64 }` for a generic
>      DECL, which has no runtime layout. The Pass-0 guard existed; the PREDICATE was
>      wrong — it scanned the struct's FIELDS for a type param, so a PHANTOM parameter
>      (`struct Shelf<S> { label: i64 }`) read as non-generic. Genericity now comes from
>      the declaration's type-param count. The field scan was never wrong when it
>      answered TRUE; it was incomplete, which is the harder kind of wrong to notice —
>      and the comment above it had already observed that a phantom parameter "would need
>      a real type-param count", then used that as a reason not to add one.
>
>      **▸ D38-D41 came out of the D24/D25/D26 work (2026-09-03), all pre-existing, none
>      a regression of that change, each verified by construction.**
>
>      **D38 — `scg`'s mono discovery walk cannot REFUSE, so a non-convergent program
>      gets a truncated module instead of a diagnostic.** `fn grow<T>(x: T, n: i64) ->
>      i64 { let b = [x]; grow(b, n) }` instantiates itself at a strictly larger type
>      argument for ever. The oracle caps at `MAX_MONO_INSTANCES` and returns `Err`; mode
>      4 has no error path, so `scg` now caps the drain and abandons mono emission
>      entirely (188 bytes). That is the best available answer and still not the oracle's.
>      ⚠ Capping WITHOUT the bail is worse than useless — measured, it turns the stack
>      overflow into a 70 MB module. Giving mode 4 a refusal channel is the real fix.
>
>      **D39 — `scg` renders a NULLABLE generic-instance field BY VALUE where the oracle
>      renders `{ i1, ptr }`.** `struct Node<T> { next: ?Node<T>, v: T }` gives
>      `%Node_i64 = type { { i1, %Node_i64 }, i64 }` against the oracle's `{ i1, ptr }` —
>      a self-referential layout, and invalid IR. Pre-existing and independent of D25,
>      but D25's declared-type filter is what keeps it LOUD: without the filter the
>      closure chased those fields and buried the bug under 8.4 MB of declarations.
>
>      **D40 — emission order diverges when a generic call sits in another generic call's
>      ARGUMENT subtree.** The oracle pushes the callee key BEFORE walking args; `scg`
>      records during codegen, which lowers operands first. `outer(inner(true))` gives
>      oracle `outer__i64, inner__bool` and `scg` the reverse. Pre-existing — the
>      pre-change binary diverges identically on the non-transitive form — but D24's
>      worklist now lets it CASCADE through several defines. No corpus program reaches it.
>
>      **D41 — type-decl order diverges for an instance born only inside a transitively
>      discovered body.** The oracle interns while substituting mono bodies in DISCOVERY
>      order; `scg`'s discovery pass interns in DRAIN order. It is the same
>      two-structures distinction D24 applied to `define`s and did not apply to
>      interning. Not a regression (these programs were invalid IR before, with
>      coincidentally correct type order), and no corpus program reaches it.
>
>      **D27 — `__spawn_wrapper_<FnId>` collection is a FOURTH mono root with exactly
>      defect δ's shape.** D15's δ fix added class init, class methods and impl methods
>      as monomorphisation roots because they are "lowered as ordinary functions but do
>      not live in `program.fns`". The `spawn`-target scan ~30 lines below it in the same
>      function still reads `for f in program.fns.iter()...` and was NOT extended, so a
>      `spawn` inside a class method, an impl method or a generic fn body emits
>      `call ... @__spawn_wrapper_42` with no definition — silent invalid IR, the same
>      failure signature δ was filed for. PRE-EXISTING (both binaries), verified on three
>      shapes, and `scg` gets it right, so the polarity matches δ exactly. Deliberately
>      NOT folded into the D15 commit: δ's fix was about the mono worklist, this is the
>      spawn-wrapper worklist, and conflating them would have made a clean byte-neutral
>      change into one that moves `spawn` output.
>
>      **▸ D28-D34 came out of the D17 work (2026-09-02). Every one was verified BY
>      CONSTRUCTION — the oracle and both `scg` binaries run on a written program — and
>      every one is PRE-EXISTING (pre- and post-change `scg` byte-identical) unless it says
>      otherwise. Ranked by what they cost. D31 is the one to read first.**
>
>      **D28 — `*g = v` on a non-`i64` element emits a NARROW STORE into the 8-byte
>      protected slot.** `llvm-as` ACCEPTS it, so it is a cell where `scg`'s output is
>      assemblable and semantically wrong — the class the `llvm_rejects` gate structurally
>      cannot catch. It is NOT the only one, though a first draft of this entry said so: the
>      f64 element (D30) is the same class by a different route, since `scg` folds `as f64` to
>      a no-op, so its whole module assembles and passes the INTEGER where the oracle passes
>      the double's bits. For a `secret` element
>      it also leaves the upper bits of the PREVIOUS secret in the slot — residue, not
>      merely stale bytes. D17 did not introduce it, but D17 removed an unrelated
>      `mutex_new` invalidity in the same module that used to make `llvm-as` reject first,
>      so it went from loud to silent. NOT reportable, and here are all three legs,
>      measured: the ORACLE refuses the program outright ("guard assign element not ported
>      (i64 only)", applied AFTER its own `unsecret`, so `i64` and `secret i64` are the
>      only elements it admits); **inkwell — the back end that actually emits objects — encodes it
>      correctly** (`build_int_z_extend` / `build_bit_cast` / `build_ptr_to_int`, then
>      `store i64`); and nothing links `scg`'s IR for it. **Oracle-first**: port the
>      `llvm_dump.rs` gate, THEN give `borrow_arms`'s `un_place` branch
>      `cg_container_encode` and widen the store to `i64`.
>
>      **D29 — DONE (`__D29__`).** `scg`'s CHANNEL arms (fid 22/23) dispatched the
>      element WIDTH on a raw type HANDLE, so a `secret` element — an interned handle,
>      not a scalar code — matched no width arm and fell into the pointer catch-all.
>      `Channel<secret i64>` emitted a bogus `ptrtoint` on send AND a bogus `inttoptr` on
>      recv (`llvm-as` reports only the first and stops — never read an error count as a
>      site count). Both arms now `strip_secret`. The pointer catch-all STAYS: it is
>      load-bearing for `Channel<Channel<i64>>`, and `secret Channel<i64>` still lands in
>      it correctly, because stripping unwraps to a handle that is still >= `tbase()`.
>
>      **This is the OPPOSITE call from D34 and the distinction is ADR 0066 D8's own**,
>      not an inference from a diagnostic name: "In-process thread boundary → NO fence.
>      Cross-process / IPC boundary → fence", and D8 names the in-process channel by hand,
>      calling secrets over it "a *feature*: it enables parallel constant-time crypto over
>      secret data". The review attacked the distinction from four directions — every
>      escape path (process pipe, `sealed_channel` bridge, FFI, spawn, a declassify-free
>      public read) is independently refused — and it held.
>
>      ⚠ **D8 ALSO SAYS "`recv` must *not* strip the qualifier"**, and this change adds
>      `strip_secret` to the recv arm. They are compatible: D8's invariant is TYPE-level
>      (so the receiver stays taint-checked) and this strip is the codegen WIDTH dispatch
>      on a local. The arm carries a ⚠⚠ block saying so, because anyone grepping `recv`
>      + `strip` lands on both.
>
>      ⚠ **UNPINNED, AND STRUCTURALLY UNPINNABLE by the current harness.** The
>      differentials `continue` on a program the ORACLE rejects, before `scg` is ever run,
>      so no `Channel<secret T>` can reach `llvm_rejects` — a straight revert of these two
>      lines is caught by NOTHING, and a 380-program sweep moves zero. The best evidence
>      it is right is that post-change `Channel<secret u8>` is byte-identical to the
>      oracle's `Channel<u8>`, which is exactly the lowering M1.4c-2 needs.
>      **D8 obliges a `tests/pass/` fixture and a `tests/ui/` rejection pinning "the
>      element carries `secret` end-to-end"; that obligation is still UNMET and only
>      M1.4c-2 can meet it. Carry it there.**
>
>      **D30 — an `f64` container element IS differentially reachable and DOES diverge,
>      and TWO existing comments said it could not.** This is the claim the D17 review
>      killed. The guard is NOT "the oracle Errs on every `as f64`"; it is that the oracle
>      Errs wherever an f64 TYPE must be RENDERED (`lty`). A container element is never
>      rendered — it rides the i64 slot — so `shared_new(x as f64)` and `mutex_new(3 as
>      f64)` LOWER: the oracle emits `bitcast double %v1 to i64` and `scg` emits nothing.
>      (`let d: f64 = x as f64` DOES Err, which is the shape both comments generalised
>      from.) Writing the arm would not fix it: `cgo_ty` renders code 4 as `i64`, and
>      **the oracle's own f64 output is itself INVALID IR** (`bitcast double 3 to i64` —
>      "integer constant must have integer type"), which is exactly why the `llvm_rejects`
>      gate stays silent: it fires only when `scg` is rejected AND the oracle is clean.
>      Needs real f64 lowering in both back ends. The three PRE-EXISTING copies are corrected
>      in `6337bea`, but the same commit wrote TWO NEW ones — `cg_container_encode`'s
>      `else`-arm comment and the new fixture's registration in
>      `crates/sentinel-driver/tests/ui.rs` — both falsified by the same constructed
>      `Shared<f64>`, and the second refuted verbatim by its own fixture's header. Fixed in
>      the follow-up. **GREP THE SET before calling this closed.**
>
>      **D31 — DONE (`7e5ca56`).** A by-value container argument to a GENERIC user fn
>      dropped its refcount clone, so `scg` emitted an OVER-RELEASE — a use-after-free.
>      It was the only item on this list that was DIFFERENTIALLY LIVE: the oracle compiles
>      the program fine, so the corpus differential would have turned red the moment a
>      fixture reached it.
>      Repro, four lines — `fn wrap<T>(x: T) -> i64 { 1 }` and
>      `let s: Shared<i64> = shared_new(6); let n: i64 = wrap(s); shared_get(s) + n`.
>      The oracle emits **1 `sentinel_shared_clone` against 2 `sentinel_shared_release`**,
>      with 1 `sentinel_shared_new` — so `#new + #clone == #release` and the count balances;
>      `scg` emits **0 clones against the same 2 releases**, one release too many, so the rc
>      hits zero inside the callee and the following `shared_get(s)` reads freed memory.
>      (⚠ Count CALL sites, not `grep -c` on the symbol — the `declare` line inflates every
>      one of these by one, and it inflated this entry's first draft.) A NON-generic callee is byte-identical, so it is specifically
>      the generic path; `Mutex<T>` behaves the same. Locus: `dump_args_capture_all`
>      (`selfhost/types/infer.sentinel`) omits the `cg_clone_shared_arg(c, t0);` that
>      `dump_targs` has, while its own comment claims it "mirrors dump_targs /
>      dump_args_capture_first" — false in exactly that one line. **Scope check, before
>      anyone treats it as a security matter:** `scg`-only (inkwell and the oracle are both
>      correct), and `selfhost/*.sentinel` declares no container-typed binding at all, so
>      nothing that is built and run links it. Same class as the `mvbv` over-count fixed in
>      `c010c2b`, with the polarity — and so the severity — reversed: that one leaked,
>      this one frees early.
>
>      **⚠ THE FIX IS NOT THE OBVIOUS ONE, and the obvious one is a REGRESSION.** The
>      first cut was a bare `cg_clone_shared_arg(c, t0);`, mirroring `dump_targs` exactly.
>      The review caught that it made THREE previously byte-identical shapes DIVERGE —
>      `takes({ s })`, `takes(if c { s } else { t })`, `takes(p.h)` — because the helper
>      gates on `mvbv >= 0`, which is **not** "the argument is a named Var": `mvbv` is
>      reset per node and set by the `Var` arm, so ANY compound argument whose tail happens
>      to be a Var leaks it upward and the helper fires. The oracle decides STRUCTURALLY
>      (`clone_if_shared_var` returns early unless the whole argument is a `Var` node). The
>      landed fix therefore adds `ndump`, a monotonic count of `dump_texpr_node` entries: a
>      delta of exactly 1 across the argument means it was a LEAF, and `mvbv >= 0` means
>      that leaf was a Var — together, the oracle's test. **Do not "simplify" the gate back
>      to a bare helper call**; nothing in `tests/pass` can catch it (see D35).
>
>      Verified: a 347-program sweep against the pre-change binary moved ZERO programs, and
>      the fixture `tests/pass/c71_generic_container_arg_rc.sentinel` is byte-identical to
>      the oracle. It carries two negative cases — an RVALUE argument and an
>      ENUM-CONSTRUCT payload — because the mutation lens showed a wrong implementation
>      dropping either of the gate's other two conjuncts (`mvbv`, `cg_ufarg`) passes every
>      positive case and the whole corpus. ⚠ **It was OBSERVED a slice earlier and not filed**: the ADR
>      0016 A1 work hit it, excluded containers from `c16_mono_key_handle_tags` because of
>      it, and described it in that fixture's header as "`scg` drops the refcount `clone` on
>      a container passed as a GENERIC call argument" — accurate, and never given a number,
>      so it did not reach this register until the D17 review reconstructed it. A defect
>      recorded only in a fixture comment is a defect nobody is tracking.
>
>      **D32 — `scg` has no element-domain gate on `Shared`/`Mutex` at all, and takes the
>      element from the wrong place.** Three faces, each constructed: it ACCEPTS
>      `Shared<P>` for a struct `P` and emits clean IR where the oracle says "`Shared<T>`
>      element type is not supported yet"; it ACCEPTS `let s: Shared<u8> = shared_new(3)`,
>      taking the element from the ANNOTATION, where the oracle takes it from the ARGUMENT
>      and rejects with "expected <shared#2>, found <shared#0>"; and it PANICS —
>      `sentinel: index out of bounds: idx=-101, len=3`, exit 127 — on a CALLED generic
>      container fn (`fn getit<T>(s: Shared<T>) -> T { shared_get(s) }` plus a call).
>      ⚠ Merely DECLARING that generic does NOT panic; `scg` exits 0 with clean IR. The
>      CALL is the trigger, and a probe that only declares it files a false report — mine
>      did, before I constructed the calling version.
>
>      **D33 — `cgo_ty` has no arm for scalar code 4 (`f64`) or 5 (`ptr`).** Both fall to
>      its trailing `i64`. Not cosmetic: it is what makes D17's omission of those two
>      elements CORRECT rather than lazy, and it is the first thing that has to change
>      before D30 can be closed. Its `ll_type_to` twin needs the same.
>
>      **D34 — DONE (`b9c0aa7`).** `process_send`/`process_recv` (fids 29/30) were the
>      last unmirrored member of D17's set. Three parts: a new `dump_process_recv` whose
>      element comes from the EXPECTED type — a `Process` is ANONYMOUS and carries none,
>      so this is the `channel_new` (fid 21) shape, NOT the `recv` (fid 23) shape, and
>      choosing between those two is the whole difficulty; the codegen reusing D17's
>      `cg_container_encode`/`_decode` unchanged plus an element-typed `{ i1, T }`
>      aggregate; and the DEFERRED_PROGRAMS entry deleted, which is the only proof this
>      project accepts for that list. The pin is byte-identical at the TYPES, MIR and
>      CODEGEN stages; the i64 control is unchanged at all three; a 347-program sweep
>      moved exactly the pin; the merged-compiler fixed point was rebuilt and compared
>      (4,941,871 bytes, identical).
>
>      **Two things from it that generalise, both learned the expensive way:**
>
>      ⚠ **THE DEFERRED ENTRY EXISTED IN THREE LISTS** — `selfhost_types.rs`,
>      `selfhost_mir.rs`, `selfhost_codegen.rs`. Deleting one and running the suite is how
>      the other two surfaced ("now MATCHES the oracle — delete it from
>      DEFERRED_PROGRAMS"). The harness catches this, but only after a full run; grep the
>      set first.
>
>      ⚠ **THE ENCODE HAD TO BE GATED ON THE FENCE, and the ungated version was a
>      FAIL-OPEN REGRESSION on a security boundary.** `cg_container_encode` strips
>      `secret` FIRST — correct for a container, where the taint lives in the type system
>      and the value rides the i64 slot; WRONG at a process pipe, where a secret may not
>      cross at all (ADR 0066 D8). Ungated, D34 turned that fence in `scg` from
>      FAIL-CLOSED to FAIL-OPEN: `secret u8`/`i32`/`bool` went from emitting IR `llvm-as`
>      rejects to emitting clean assembling IR. Measured, gated, re-measured; the gate
>      costs nothing (zero corpus movement, pin and control still byte-identical).
>      `secret i64` still assembles — a PRE-EXISTING hole, untouched, and worth its own
>      look. **The lesson is reusable-helper-shaped: a helper that is correct for one
>      boundary can be exactly wrong at another, and `strip_secret` is the tell.**
>
>      **▸ D35-D37 came out of the D31 work (2026-09-02), all verified BY CONSTRUCTION
>      and all PRE-EXISTING (pre- and post-change `scg` byte-identical).**
>
>      **D35 — THREE call paths still carry the leaky `mvbv` gate**, not the one this
>      entry first named. Settled by construction (each shape run against both binaries):
>      `let y = p.h;` (the let-init duplication), `takes(p.h)` through `dump_targs` (the
>      ordinary-call arg walker), and `spawn work(p.h)` (the capture). All three
>      over-clone — `scg` emits 1 where the oracle emits 0 — which is a LEAK and a
>      divergence. D31's generic site is now correct, which is the independent
>      confirmation that its `ndump` leaf test is scoped right.
>
>      **⚠ THREE EMITTERS, THREE DIFFERENT PREDICATES**, which is why this pair has
>      drifted twice: `llvm_dump.rs` and the inkwell twin both test
>      `matches!(expr.kind, Var(_))`, while `selfhost/types/cg.sentinel`'s
>      `cg_clone_shared_arg` tests `mvbv >= 0`, a per-node most-recent-`Var` tracker.
>      Any fix must move all three together, and any new fixture for these shapes will
>      diverge until it does. NOT fixed here deliberately: the ORACLE is the wrong side
>      on some of these, so it is oracle-first and cannot be done from `selfhost/`.
>      There is more to this item than the parity divergence and that part is tracked
>      privately with the maintainer — ask before working it, and do not widen it.
>
>      **D36 — a generic fn that RETURNS its container param over-releases in `scg`.**
>      `fn ident<T>(x: T) -> T { x }` at `Shared<i64>`: the oracle omits the scope-exit
>      release in the mono body (correct — ownership moves out with the return value),
>      `scg` emits it. Pre- and post-D31 `scg` emit the identical callee body, so D31
>      neither caused nor fixed it; D31 improved the surrounding balance from −2 to −1.
>      ⚠ It also showed that the `SharedReturnNotSupported` guard does not cover the
>      generic form: the non-generic `fn ident(x: Shared<i64>) -> Shared<i64> { x }` is
>      REJECTED ("returning a named `Shared<T>` binding is not yet supported") while the
>      generic form compiles, because the guard matches on `Type::Shared(_)` and a generic
>      body's tail types as `TypeParam` — it runs pre-monomorphisation and nothing
>      re-checks the instance. That particular route is SOUND in `snc build` (inkwell
>      implements the transfer exemption the guard was compensating for, verified by
>      running it). The general question of what that guard does and does not cover is
>      tracked PRIVATELY with the maintainer; ask before extending it.
>
>      **D37 — `scg` PANICS on a generic `&T` parameter given a container.**
>      `fn takes<T>(x: &T)` called as `takes(&s)` with `s: Shared<i64>` aborts both `scg`
>      binaries with `sentinel: index out of bounds: idx=-101, len=4` while the oracle
>      emits cleanly. Same family as D32's panic (a CALLED generic container fn), different
>      trigger.
>
>      **▸ Two small pins the D17 review found missing, one fixture each:**
>      `MutexElementNotSupported` has NO ui fixture in either position, and
>      `SharedElementNotSupported` is unpinned in VALUE position (the new
>      `c71_shared_element_unsupported` pins the type position only).
>
>      **▸ ALSO CLOSED BY THE D15 COMMIT, though filed under the family rather than as
>      its own item: the unbounded-instantiation HANG.** `fn f<T>(x: T) -> i64 { let b =
>      mk(x); f(b) }` type-checks and used to consume memory for ever with no depth cap —
>      a compiler denial-of-service from four accepted lines. The monomorphic worklist is
>      now capped and refuses with a diagnostic. The same commit added a second cap on
>      the generic-struct field closure, for `struct S<T> { c: S<S<T>> }`, which is
>      DECLARABLE (only the non-generic `struct N { n: N }` trips "recursive struct has no
>      representable size") and has no finite layout.
>   5. **Widen `snc merge`'s Bar-A source printer** — the highest-leverage COVERAGE item.
>      `crates/sentinel-driver/src/source_dump.rs` rejects **98 of the 119** real programs
>      (re-measured after `demos/` joined the corpus), dominated by `cast` (37) and `declassify`
>      (36), then `ptr_of` (9), `float literal` (6), `scope` (5), `return` (4), `sqrt` (1).
>      A CONCRETE payoff is now on the record: the widen gap `34e1a8f` fixed is written 18 times
>      in `chacha20_stream` / `chacha20poly1305` / `std/security/aead`, and no differential
>      could see any of them because those files are rejected here (`cast`) AND in the direct
>      form (`use` imports). Because the stage oracles do no module discovery, the MERGED
>      form is the only way a multi-module program reaches resolve/types/effects/borrow/mir/
>      ctverify — so widening the printer would take those stages from 22 real-program
>      comparisons to ~200, and would de-vacuum the ctverify sweep (today all 22 of its
>      comparisons are `"" == ""` because no program reaching it declares a `secret`; every
>      secret-bearing program in `examples/` is blocked by `declassify`/`cast`). Contained: it
>      only changes output for programs `snc merge` currently REJECTS, so no existing merge
>      output moves and both fixed points are untouched.
>   6. **The `export "C"` MERGE side, both halves together** (ADR 0059 A10). `snc merge`
>      silently DROPS the export prefix: `source_dump::emit_fn` has no `is_export_c` branch,
>      exactly the lossiness `emit_externs` was added to fix ("the merge output is meant to be
>      a faithful single-file rendering"). VERIFIED end-to-end: `snc build --lib` succeeds on
>      an export program and then FAILS on that same program's `snc merge` output with
>      "`build --lib` produced no `export \"C\"` functions". `scg`'s two merge dispatchers are
>      unmirrored to match — `emit_item` drops the prefix identically (so the two agree today
>      and no differential can see it), and `build_rename` would module-QUALIFY the symbol
>      that ADR 0059 A3 says must stay BARE (`merge_modules` skips exports from the rename
>      map: `if f.is_export_c { continue; }`). The halves MUST land together — mirroring
>      `scg` alone makes its merged TEXT differ from the oracle's and breaks merge parity, and
>      fixing the oracle alone leaves `scg` dropping the prefix. Unreachable today (nothing in
>      the corpus, the fixtures or `selfhost/` merges an export), and `snc build` is unaffected
>      (it merges ASTs via `merge_modules`, not text). Wants an ADR 0059 amendment for the
>      qualify-or-not decision, plus a multi-module export program registered in the sweep.
>   7. **Named-impl qualification** (`delegation.sentinel`, the LAST `KNOWN_SCG_BUGS` entry in
>      `selfhost_codegen.rs`) — recording the name in the merge's `build_rename` is the missing
>      half, but alone it CRASHES scg via the unguarded `-1` at
>      `selfhost/types/borrow.sentinel:651` (`impl_lookup` → an out-of-bounds table index).
>      Both halves must land together; the `-1` guard is worth adding regardless (an unresolved
>      impl name should be a diagnostic, not a panic).
>   8. **Type-param over-rename** (pre-existing, contrived) — scg's merge renamer has no
>      type-param scoping, so a generic param named identically to a same-module type is wrongly
>      qualified. The Rust `Renamer` skips in-scope type params; scg has no `tparams` set.

> ⚠ **KNOWN-AND-TOLERATED scg-over-accepts (documented at `selfhost/types/interner.sentinel`'s
> `is_channel` arm — do NOT "fix" blindly, they are snc-only and IR-invisible):** scg does not
> re-enforce the oracle's one-level channel nesting cap. That is the "scg is dump-only, trusts
> oracle-valid input" contract, like the guard/return pins.
> (The `Channel<f64>`-renders-`Channel<i64>` entry that used to head this list is **CLOSED** —
> the ADR 0058 mirror made `f64` scalar code 4, so it now renders correctly, verified at
> `types` and `mir`. `?f64` and `Fn<f64,f64>` came with it.)
> ⚠ **The `is_shared` / `is_mutex` element-hardcoding that used to sit here is CLOSED**
> (ADR 0016 A1). The two arms are now element-generic, a 2-line mirror of the `is_channel`
> arm. Two corrections to what this entry used to say, both measured:
> **(i)** it was NOT "snc-only and IR-invisible" — with the element discarded,
> `Tagged<i64, Shared<u8>>` and `Tagged<i64, Shared<i64>>` interned to the SAME handle, so
> scg emitted one generic-instance declaration where the oracle emitted two; that structural
> collapse is what forced the fix into the A1 mangling slice rather than a later one.
> **(ii)** the entry implied that making the arms element-generic would make non-i64
> container elements work. It did not: at the time, scg's CODEGEN emitted no width conversion
> around a container's runtime call, so `fn take(s: Shared<u8>) -> u8 { shared_get(s) }` gave
> `ret i8 %v` for an `i64` `%v`, which `llvm-as` rejects — verified by diffing a pre-change
> against a post-change scg binary on that exact program and getting BYTE-IDENTICAL output.
> That half was untouched ADR 0071 M1.4c work; it was registered as **D17** and is now
> **CLOSED** (`6337bea`). That same program is byte-identical to the oracle today.

> **▶▶ THE BOUNDARY-PREDICATE AUDIT (`61bcac3`) — what it found, so it need not be re-run.**
> Prompted by the M1.2c fence bug (a security boundary written as `is_spawn_word_scalar(ty) &&
> ty.to_nullable_inner().is_some()` — a coincidental intersection that widened silently). Swept
> every value-admission / ownership / constant-time boundary. RESULT: the exact conjunction
> shape exists in ONE place (the fixed `is_process_channel_elem`); every other value-admission
> fence is a default-closed explicit list. Fixed: `needs_drop` (the only fence that failed OPEN,
> `_ => false`) is now exhaustive like `is_copy_type`; two doc-comments that described their list
> AS the intersection were de-coupled. Routed onward: **B1 — the constant-time verify reads
> taint off `Type`, so a future operator that forgets to re-wrap `secret` could silently disarm
> the leak check** (same mode as the bug, NOT live — the primary SecretBranch/Divisor/
> ShiftAmount rejections are eager + explicit). **✅ B1 DONE** (`e450102`): a fail-closed
> secret-preservation TEST (not a runtime assert — a secret sink type-checks with a public
> result + secret operand, so a central assert would false-positive on the hot path). Each
> operator family is matched EXHAUSTIVELY (new variant = compile error until classified);
> mutation-proven non-vacuous; a 3-lens review found no live defect. B4 (`secret_scalar_slot`
> position invariant) left — explicit + test-pinned. **All four audit findings now resolved.** **The lesson that generalises: a security
> boundary must be an explicit list, never a derived intersection of two maps maintained for
> other purposes — and each such boundary must be pinned by a rejection fixture.**

> **▶▶ M1.2c LANDED — what a resuming session needs.** `Channel<Channel<T>>`, ONE level, at
> fixed `ChanId`s 10..=15; 6..=9 are RESERVED for M1.4c-2 `Channel<secret T>` (the map and the
> `channels` table now BOTH spell them out — they must change together). Nesting is bounded
> because `channel_chanid_for` is table-free by design; deeper nesting is a diagnostic.
> `NullableInner::Channel` exists ONLY so `recv` on a channel-of-channels can return
> `?Channel<T>` — without it the recv arm's `.expect()` PANIC-ASSERTS. Runtime/ABI unchanged.
>
> ⚠ **THE LESSON, which generalises past channels:** `is_process_channel_elem` gated the
> cross-process fence as a COINCIDENTAL INTERSECTION (`is_spawn_word_scalar ∩
> to_nullable_inner.is_some()`), which excluded handles only because no handle had a `?T`
> form. Giving `Channel` one silently widened a SECURITY FENCE from a distance — a live
> channel pointer could be written into a pipe, and an integer from the far end turned back
> into a handle the runtime dereferences on a plain `send()`, from safe source. It is now an
> explicit list + `tests/ui/c66_process_channel_handle_fence.sentinel`. **Audit any other
> predicate that defines a boundary by intersecting two unrelated maps.**
>
> **▶ NEXT on this track, in order:**
>   1. **✅ DONE — the scg channel-element mirror** (`0a7075e`, M1.2b-cont + M1.2c together):
>      element-generic channel typing + send/recv encode/decode; `channel_generic` +
>      `addressed_reply` are byte-identical and OFF `DEFERRED_PROGRAMS`. It also needed an
>      ORACLE extension (the text backend errored on channel-of-channels — no `Channel` arm in
>      send/recv; a channel element is a `ptr`, ptrtoint/inttoptr, matching inkwell). ⚠ Two
>      known scg-over-accepts, both snc-only (documented at the `is_channel` arm): `Channel<f64>`
>      mis-renders `Channel<i64>` in the TYPES dump (scg's pre-existing no-f64-handle gap; LLVM
>      identical, no corpus program) and the one-level nesting cap is not re-enforced. If a
>      `Channel<f64>` fixture is ever added, it will fail the types differential unregistered —
>      register it or close scg's f64 gap first.
>   2. **`select`** — runtime PINNED (D11: a Sentinel-owned `parking_lot` queue; ABI-safe
>      because `SentinelChannel` is declared fully opaque, so the four C-ABI signatures stay
>      byte-identical and existing codegen does not churn). Surface still OPEN and worth
>      deciding BEFORE code: a select returns TWO things (which arm + value) where `?T` carries
>      one; receive-only is the complete design while channels are unbounded; and it should be
>      pinned together with the `Sender`/`Receiver` split (D11) since that would re-type it.
>   3. **M1.4c-2 `Channel<secret T>`** — the same queue rewrite unblocks it (Sentinel would own
>      the in-transit nodes, so they can be mlocked/scrubbed); slots 6..=9 are already reserved.

### ▶ (previous) 2026-07-21 — **the `extern "C"` self-host mirror is COMPLETE** (resolve + parser + lexer `5c911f5`, types/codegen `83a0473`), the oracle's `spawn <extern>` invalid-IR bug is fixed (`e17a631`), AND `spawn <runtime builtin>` is now rejected (`3791337`) — closing the three-way split the spawn-extern review surfaced. All three 2026-07-20-registered divergences are closed and the two follow-on spawn bugs resolved. First `extern`/`module` fixtures added. See "▶ ALSO OPEN" for what remains)

> **▶▶ M1.4c-1 LANDED (snc-side)** — commit `248a6f0`. Secret containers work end-to-end
> through inkwell + the `snc llvm` oracle. Full design rationale is the ADR's 2026-07-19
> amendment + the M1.4c implementation log; STATE.md's Latest entry carries the summary.
> The four design forks were resolved on my recommendations (the maintainer dismissed the
> question dialog and said "resume") and are recorded as ADR amendments — **overturnable**:
> `?Guard<secret T>` over `OpenResult`; broker primitives per-cell over a broker arena;
> `Shared`+`Mutex` now with `Channel<secret T>` as M1.4c-2; elements i64/i32/u8/bool.
>
> **THE KEY MECHANISM** (read before touching this): the container element maps are
> TABLE-FREE, so an element's interner id must be a knowable CONSTANT — which a
> source-encounter-ordered `SecretId` is not. Hence the secretable scalars are pre-interned
> at FIXED `SecretId`s 0..=3 (`SECRET_SCALARS`, interned FIRST in `check_program`), with
> container slots 6..=9 (`secret_scalar_slot`/`secret_elem_for_slot`). Verified invisible to
> the differential (dumps render secrets structurally; mangling is structural; no dump
> iterates the interner tables) — the only churn was `<secret#0>`→`<secret#2>` in one ui snap.
>
> **▶▶ M1.4c-1b LANDED — the scg mirror (element-generic containers)** — commit `c010c2b`.
> scg's containers were i64-hardcoded (`builtin_ret` + the annotation arm); its interner is
> STRUCTURAL, so `Shared<secret i64>` needed no new representation, only threading:
> `container_elem`/`is_secret_ty` (`strip_secret` already existed — reuse it),
> `dump_container_call` + `is_container_builtin` dispatched from `dump_te_call` for FnIds
> 38/39/40/41, the `_secret` constructor choice in `cg_effects.sentinel`, and two
> `cg_used_*secret` flags (+ tyctx init + declares + the `cg_anydecl` chain). **The `*g`
> deref needed NO change** — its result type is already `ta[ta[guard]]`, structurally correct.
> Wrinkle: the READERS (`shared_get`/`lock`) must emit `(targs <elem>)` for a non-i64 element
> (the Rust side records `type_args` there), so they buffer args first (the `dump_gcall`
> shape) while the CONSTRUCTORS dump straight through (the `dump_apply_call` shape); the i64
> case emits no targs, so all pre-M1.4c fixtures stay byte-identical.
>
> **⚠ It exposed a PRE-EXISTING scg REFCOUNT BUG — worth knowing, the class will recur.**
> `mvbv` (the direct-Var tracker) is reset only on ENTRY to `dump_texpr` and set by the `Var`
> arm, so a var dumped as a CALL ARGUMENT leaks up: `let s = shared_new(a)` saw `mvbv = a`,
> concluded "the RHS is a named binding", and emitted a spurious `sentinel_shared_clone`. An
> rvalue source TRANSFERS its refcount unit and must not clone — so that is an overcount that
> LEAKS the cell, diverging from the oracle (which decides structurally on the RHS
> expression). Latent because every earlier fixture passed a LITERAL to the constructors.
> Fixed by clearing `mvbv` before `dump_te_call` returns (a call's value is always an
> rvalue). Same class as the `mvbv` compound-tail leak behind `SharedReturnNotSupported` —
> **if you touch `mvbv` consumers, check for nested-Var leakage.**
>
> **▶ SEPARATE TRACK — the self-host differential blind spot is now CLOSED (`d8552c9`).**
> The differential swept only `tests/pass` + `tests/ui` (single-construct fixtures), so
> divergence in REAL programs was structurally invisible. `selfhost_codegen.rs` gained
> `sentinel_codegen_matches_oracle_on_real_programs`, sweeping `examples/`,
> `sentinel_library/` and `tools/` — it stages `sentinel_library/` into the work dir (the
> `examples.rs` `assemble()` approach) so `use std::…` resolves for the oracle AND scg's
> own merge, which is what gets **48 real programs emitting** (an ad-hoc `SNC_LIB_PATH`
> pass reached only 14). Divergences must be fixed or REGISTERED in one of two
> deliberately-separate lists: `DEFERRED_PROGRAMS` (7 ADR-documented feature gaps) and
> `KNOWN_SCG_BUGS` (real defects). A registered program is still run, and one that starts
> MATCHING fails the test, so neither list can rot.
>
> **It found 2 REAL scg bugs on its first run, neither previously known** (both have
> spawned tasks): (1) **`extern "C"` merge mis-renaming** — scg qualifies the extern
> IMPORT (`@std$sys$posix$getpid`) and leaves the real fn bare (`@pid`), so the caller
> emits **NO CALL AT ALL** and the IR **does not assemble** (`store i64 %v0, ptr %v0`);
> minimal repro is in the test's comment, and `uid()` in the same module is fine, so it is
> extern-specific. (2) **class/impl symbols lose the module prefix** (`@Logged__init` vs
> `@input$Logged__init`) — a cross-module collision hazard.
>
> ⚠ **The same blind spot still applies to the OTHER stages** (types/mir/borrow/effects/
> resolve differentials all use the fixture-only `collect_fixtures`). Extending them the
> same way is the obvious follow-on.
>
> **▶▶ THE VERIFICATION TRACK (this session's second half) — read this before touching
> `selfhost/` or `llvm_dump.rs`.** The self-host differential swept only `tests/pass` +
> `tests/ui` (single-construct fixtures) and compared BYTES only. Both gaps are now closed,
> and closing them found real, previously-invisible defects:
>
> 1. **Real-program sweep** (`d8552c9`) — `selfhost_codegen.rs` gained
>    `sentinel_codegen_matches_oracle_on_real_programs`, sweeping `examples/`,
>    `sentinel_library/`, `tools/`. It stages `sentinel_library/` into the work dir (the
>    `examples.rs` `assemble()` trick) so `use std::…` resolves for the oracle AND scg's own
>    merge — that is what gets **48 real programs** comparing. Divergences must be fixed or
>    REGISTERED in one of two deliberately-separate lists: `DEFERRED_PROGRAMS` (ADR-documented
>    feature gaps) vs `KNOWN_SCG_BUGS` (real defects, carrying their diagnosis). A registered
>    program is still RUN, and one that starts MATCHING fails the test, so neither list can rot.
> 2. **IR-validity check** (`8061aae`) — the harness now assembles scg's output with
>    `llvm-as`. It is DIFFERENTIAL (scg is at fault only when the ORACLE verifies and scg does
>    not) because it immediately showed the oracle itself was invalid for ~30 programs.
>    ⚠ `llvm-as` EXITS 0 while printing "does not verify as correct!" — check stderr, not the
>    exit code.
> 3. **Three scg merge miscompiles fixed** (`8061aae`): `emit_item` had no `extern` arm, so a
>    block's BODYLESS decls reached `emit_fn_decl`, which parses a block that is not there and
>    DELETED the following declaration — a loud miscompile AND a **silent** one
>    (`getpid() + getuid()` → `add i64 %v0, %v0`: assembled, ran, wrong answer). `build_rename`
>    also never recorded trait (54) / class (56) / **effect (57)**; the effect omission made a
>    cross-module `handle` fall into `unreachable` — the oracle's binary exits 42 where scg's
>    **traps**.
> 4. **Oracle IR validity fixed** (`83c1f21`): ~30 invalid programs → **0**. (a) shift-amount
>    width coercion (`shl i32 %v4, %v7` with an i64 amount — every chacha20/ssh/ct example);
>    (b) `extern "C"` imports were called but never `declare`d. ORACLE-MOVING, mirrored into
>    scg in the same commit. **inkwell was never affected** — those programs always ran fine;
>    this was purely the text backend, which is the ground truth the whole differential
>    verifies scg against.
>
> **▶▶ DONE — scg `extern "C"` support in types/codegen** (`83a0473`). `process_ids` is
> byte-identical (15378 B) and `llvm-as`-clean; its `KNOWN_SCG_BUGS` entry is DELETED.
>
> **THE MECHANISM** (read before touching extern handling): the difficulty was **FnId
> NUMBERING**, not parsing. The Rust resolver registers every own fn FIRST and every
> `extern "C"` decl AFTER (`resolve_module`) — but the merged TEXT does not say so: the Rust
> `source_dump` emits the block FIRST and scg's own merge emits it MID-FILE. So Pass 1
> records only each block's POSITION (`xpos`) and registration is **deferred** to
> `register_externs`, which appends after the depth-0 scan completes; **`nufn`** freezes the
> fn/extern split point in the user-fn table. Parity was confirmed on the merged text
> (`snc types` byte-identical; externs numbering 49..52 after main..report) **before** codegen
> was touched — that ordering of work is what kept the risk contained, and is worth repeating
> for the resolve mirror below.
>
> Externs then take a full row in every parallel table via the **ordinary `scan_fn_sig`** (an
> extern's `fn name(params) -> ty` IS a fn signature; only the terminator differs, `;` for
> `{`), so `fn_lookup` / `cg_emit_call` / `mir_put_callee` needed **no extern-specific code**.
> The synthetic **itemkind 100** is ignored by every pass-2 group — that is what suppresses
> the `define`; `cg_extern_declares` emits the `declare`s at the oracle's position;
> `dump_moves` bounds on `nufn` (an extern has no body → no `snc borrow` line). The depth-0
> recognizer is gated on the FOLLOWING string literal, not the `extern` text alone: unlike the
> merge's `is_extern` it sees every depth-0 token, and fn-signature params sit at depth 0 too.
>
> **⚠ THE REVIEW EARNED ITS KEEP AGAIN — a pre-existing SILENT MISCOMPILE** (`52d42d2`).
> `emit_extern_block`'s ADR 0057 A9 `link(...)` loop terminated on token kind **13**, but 13
> is **`==`**; `)` is kind **5** (`(`/`)` are 4/5 — check `selfhost/parser.sentinel`'s operator
> table, do not trust prose). It ran from `link(` to the next `==` anywhere in the file, or to
> EOF, eating the block's decls and every item between. `extern "C" link("m") { … } fn main()`
> collapsed scg from the oracle's 190 bytes to **38** — target triple only. With an `==` later
> in the file it stopped there instead and emitted **valid IR that assembled, ran, skipped the
> FFI call, and returned the wrong answer**: the `llvm-as` guard cannot see that class. A
> second defect emitted `link(""m"")` (a tag-63 span ALREADY includes its quotes). **The same
> false paren-kind belief was in a comment the extern slice had just added** — corrected there.
> Latent in-repo only: no fixture under `tests/` contains the string `extern`, and every
> shipped `link(...)` program bails on the ORACLE side first (`ptr`/`f64` "not yet ported").
>
> **▶ ALSO OPEN (self-host):**
>   - **✅ DONE `snc resolve` extern support** (`5c911f5`) — mirrored `register_externs` into
>     `selfhost/resolve`; `effects` verified as not needing it (numbers externs last, no
>     shift). Parser + lexer arms landed in the same commit; first `extern`/`module` fixtures.
>   - **✅ DONE `spawn <extern>` oracle IR** (`e17a631`) — legalized (inkwell + scg already
>     supported it); the `program.externs` fallback converges the oracle onto scg.
>   - **✅ DONE `spawn <runtime builtin>` rejected** (`3791337`) — `TypeError::SpawnBuiltin`,
>     an `is_runtime` gate in the type-checker's `Spawn` arm, pre-codegen so oracle + inkwell
>     + scg all converge on rejection. snc-only (differential-skipped ui fixture); the scg
>     panic is now unreachable in any real pipeline. ADR 0066 D6 clarification.
>   - **The OTHER stage differentials still have the real-program blind spot** — lex/ast/
>     resolve/types/borrow/effects/mir all use the fixture-only `collect_fixtures`, which is
>     exactly what hid the module/part lex gap (25 `selfhost/` files use `module`, zero
>     fixtures did). Extending them to real programs the way `selfhost_codegen.rs` does would
>     have caught it mechanically; it also covers `part`, which no single-file fixture can.
>   - **Named-impl qualification** (`delegation.sentinel`, 8 diff lines). Recording the name in
>     `build_rename` is the missing half BUT alone it CRASHES scg: `borrow.sentinel:651`
>     `impl_lookup` → `impl_trait_of` → `trait_method_index` passes an unguarded `-1` into a
>     table index. Both halves must land together (and that unguarded `-1` is worth fixing
>     regardless — any unresolved impl name is an index panic, not a diagnostic).
>   - **Type-param over-rename** (pre-existing, contrived trigger): scg's renamer has no
>     type-param scoping, so a generic param named identically to a same-module struct/enum/
>     trait/class is wrongly qualified. The trait/class arm widened its reach. The Rust
>     `Renamer` skips in-scope type params; scg has no `tparams` set.
>
> **▶ NEXT on this track — M1.4c-2: `Channel<secret T>`.** D6.1's fix already unblocks the
> typing side (`channel_chanid_for` gains the same secret slots 6..=9). What needs its own
> decision is the memory policy: a channel's IN-TRANSIT values sit in `std::sync::mpsc` queue
> nodes that Sentinel does not allocate, so they can be neither mlocked nor scrubbed without
> replacing the queue. Decide that before implementing.
>
> ⚠ **REVIEW-HYGIENE LESSON (cost real time, will recur):** the adversarial review's mutation
> pass left two planted `if false` guards in the working tree that DISABLED BOTH secret-scrub
> arms, plus a scratch test dir. A pre-commit diff audit caught them. **After any
> mutation-testing review, grep the diff for `if false` / `MUTANT` / stray files before
> committing** — a silently disabled security scrub passes every test.
>
> ⚠ **BOX GOTCHA (unchanged):** `cargo test` does NOT regenerate
> `target/debug/sentinel_runtime.lib` — run `cargo build` after ANY runtime change or snc
> links the STALE staticlib.
>
> **⚠ REVIEW-HYGIENE, RESTATED (it paid off twice more this session).** The multi-lens review
> of `83a0473` produced 11 surviving findings across 5 lenses, and the two that mattered were
> both things the implementer had verified and believed settled: a comment asserting token
> kinds that were simply wrong, and — traced from that same wrong belief — a silent
> wrong-answer miscompile 500 lines away in a different file. It also **refuted** a claimed
> regression in the new code (`ptr`/`f64` extern types rendering as `i64`): the ORACLE bails on
> those first ("not yet ported (8a scalars only)"), so the differential can never reach them.
> Run the review, and read the refutations as carefully as the findings.
>
> Related: a task filed mid-session asserted a reachable one-byte divergence in `cg_anydecl`;
> checking it before fixing showed the class is UNREACHABLE (`Process` is not a nameable type,
> so nothing can set `procsend`/`procrecv` without `procspawn`). The fix landed anyway
> (`4dbf4b2`) on the honest ground — invariant drift whose harmlessness rests on an accident —
> not on the overstated one. **Verify the consequence, not just the discrepancy.**
>
> HANDOFF STATE (verified against the repo 2026-08-30): four-check green — `cargo test
> --workspace --no-fail-fast` = **1813 passed / the 18 known Windows failures**, zero new;
> `pass` **164**, `ui` **54** (this session added `c59_export_call`, `c58_float_math`,
> `c57_ptr_of`, `c19_widen_call_unary`, `c35_effecting_let_secret`, `c42_impl_for_struct`
> + the four `ui/c35_effecting_*` boundary fixtures); all 9 differential stages byte-identical, both bootstrap fixed points hold;
> working tree clean. abi count **51**. **scg scalar codes: i64=0, i32=1, bool=2, u8=3,
> f64=4 (ADR 0058), ptr=5 (ADR 0057); unary op-codes 1..=5 are the PREFIX operators and
> 6..=9 the CALL-shaped RESERVED-NAME family (6 sqrt, 7 ptr_of, 8 ptr_of_mut, 9 is_null) —
> every one of the five symbol tables now enumerates them all and falls back to a
> non-re-parseable `<unary?>`, because a real operator as the catch-all is what once rendered
> `sqrt` as `&mut`. Parser token tags run 1..66 and are APPEND-ONLY** (resolve's decl scanner
> compares raw tag integers, so inserting one mid-table mis-classifies silently). **The
> real-program sweeps take FOUR roots — `demos/`, `examples/`, `sentinel_library/`, `tools/`
> — enumerated by hand in eight files; `demos/` was missing until 2026-08-30, so add any new
> root to all eight.**
> Interner kinds: Channel=13, Shared=17, Mutex=18, Guard=19; **next free = 20**. Secret
> container slots 6..=9 (i64/i32/u8/bool); channel slots i64=0..ptr=5, secret reservation
> 6..=9, channel-of-channels 10..=15 (`channel_chanid_for`).
> **REGISTRIES — there are now NINE, one per stage differential, not one.** In
> `selfhost_codegen.rs` (unchanged this session): `DEFERRED_PROGRAMS` = **6** (2× `fn_value`
> ADR 0070, 3× `sealed_*` ADR 0069, `process_channel_typed` M2.3b), `KNOWN_SCG_BUGS` = **1**
> (`delegation.sentinel`, named-impl qualification). NEW, from the real-program sweep:
> **lex** 0 · **ast** 0 · **resolve** 0 · **types** 4 · **mir** 4 · **effects / borrow /
> ctverify** 0, and every `KNOWN_SCG_BUGS` is EMPTY. Everything the first sweep registered has
> been FIXED rather than re-labelled: 8 float entries, 5 `ptr_of`-family entries and 3
> `export` entries are gone, each pinned by a fixture on the way out. What remains at types +
> mir is ADR 0070 D3-revisit (×2) and ADR 0066 M2.3b. The only multi-cause entry left is
> `fn_value_generic`, NARROWED by the float mirror from float+ADR-0070 down to ADR 0070 alone
> — when a slice closes one cause of a multi-cause entry, EDIT it down, never delete it. Every real-program registry has a written DEFERRED-vs-BUG criterion at its head;
> read it before adding an entry. ⚠ The ctverify real-program sweep is **VACUOUS** by
> construction (all 22 comparisons are `"" == ""`) — its doc says so; do not read its empty
> registry as verifier parity. Lexer keyword table = Rust's exactly (38; derive-and-diff command
> is in `ident_kind`'s header comment — do not eyeball it).
> ⚠ **BOX GOTCHAS:** no `just`/`nextest` here (use raw `cargo`); the MSVC+LLVM env recipe is in
> the `build-environment-windows` auto-memory (vcvars64 into the session + `LLVM_SYS_180_PREFIX=
> G:\llvm-18`); run `cargo build` before `cargo test` (test does NOT regenerate the runtime
> staticlib). **Token kinds, VERIFIED against
> `selfhost/parser.sentinel`** (an unverified list caused a miscompile): `(`=4, `)`=5, `{`=6,
> `}`=7, `+`=9, `-`=10, `*`=11, `/`=12, `==`=13, `!=`=14, ident=2, fn=3, `;`=44, struct=52,
> enum=53, trait=54, impl=55, class=56, effect=57, use=58, pub=59, string=63.

> **▶ (archived) M1.4b COMPLETE — slice 4, the D5a opt-in `Deadlock` wait-for-graph tier** (feat `36ad1af` + docs `a82e994`); ADR 0071 ACCEPTED for M1.4b. Runtime-only + non-oracle-moving (abi stayed 49): opt-in via `SENTINEL_DEADLOCK_DETECT`; a detected cycle returns the EXISTING `?Guard` null arm immediately + a stderr cycle report; wait edges are DEADLINE-STAMPED after the review caught a stale-edge false positive. Full detail in the ADR's D5 amendment + implementation log.

> **▶ (archived) slice 3c (`*g` guard deref, feat `06eba8d` + docs `7148eaa`)** — full detail
> in the **ADR 0071 M1.4b implementation log** + STATE.md's 2026-07-16 entry. Key facts:
> `sentinel_mutex_data(m, valid)` (abi 49) ABORTS on a timed-out/null guard; guard-deref is
> confined to the pinned shape — `& *g` → `GuardBorrowNotAllowed`, computed `*{g}` →
> `GuardDerefNotVar` (both snc-only, differential-skipped) — after a 4-lens adversarial
> review reproduced a use-after-free pre-commit; inkwell `*g = v` is place-then-value.

> **▶ (archived) slice 3b (guard unlock-on-drop, feat `f15c16a`) + slice 3a/2b/2a/1** — the
> full slice-by-slice detail now lives permanently in the **ADR 0071 M1.4b implementation log**
> ([decisions/0071-shared-ownership-and-mutex.md](decisions/0071-shared-ownership-and-mutex.md)),
> and STATE.md's Latest entry carries the running summary. The current RESUME HERE (3c, above)
> is the single live handoff. Key facts a resuming session needs: guard payload = the mutex cell
> handle `m`; `?Guard` conditionally unlocks on the valid drop arm; `Guard`/`?Guard` are **Move**;
> the guard no-escape rules are the **conservative pins** `GuardNotLetBound` (`lock()` only as a
> direct immutable-`let` RHS) + slice-3c's `GuardBorrowNotAllowed` / `GuardDerefNotVar` — all
> **snc-only** (scg is a dump-only port; the ui rejection fixtures are differential-skipped, like
> the peer `SharedReturnNotSupported` / `MutexReturnNotSupported`). The codegen differential
> compares the **`snc llvm` oracle** (`llvm_dump.rs`) against **`scg`**, NOT inkwell (inkwell is
> behavioural-only via `tests/llvm.rs`) — so a new backend change must be byte-identical
> oracle≡scg, and inkwell only behaviourally correct.
> ⚠ The plan file `C:\Users\bryan\.claude\plans\misty-nibbling-shore.md` (this Windows box) pins
> the original M1.4b selfhost design; the 3b work was done on the Mac WITHOUT it and validated by
> the byte-identical differential, so a divergence is possible in principle — re-sync if a
> selfhost design question comes up.

> **▶ (background, pre-slice-3b) M1.4a (`Shared<T>`) is COMPLETE + committed** (`d73322c` runtime cell → `e1e1c9f` FnId-base shift 38→40 → `d3eafe6` `Type::Shared` + lowering → `8f0a2c6` refcount clone/drop accounting → `c18d7be` the named-Shared-return guard). `Shared<T>` over public word-scalar `T` is a real refcounted handle: **`Copy` for the borrow checker** (frictionless duplication, no move-tracking) **yet drop-emitting** (`sentinel_shared_release` rc-- at scope exit, freed at zero) — the first such type. `shared_new(v)` (rc=1) / `shared_get(s)` (copy the value out); rc++ (`sentinel_shared_clone`) fires at each duplication of a NAMED `Shared` binding into a new owner (let-init / by-value user-fn arg / spawn capture), an rvalue source transfers (no clone); invariant `#new + #clone == #release`, freed exactly once. Verified across inkwell (`snc build`, exit-42 leak-checked fixtures `c71_shared` / `c71_shared_rc`) AND byte-identical `snc llvm` oracle ≡ `scg` on all 9 differential stages, both bootstrap fixed points green. ADR 0071 D2/D3/D8 is the design of record; the plan is at `C:\Users\bryan\.claude\plans\misty-nibbling-shore.md`.
>
> **ONE deliberately-guarded gap (slice 3b, partial):** returning a NAMED `Shared` binding (a bare `Var` in tail or `return` position) is **rejected** at the types stage (`SharedReturnNotSupported`, ui fixture `c71_shared_return_named`). It transfers a refcount unit to the caller and so must be EXEMPT from the drop drain; inkwell does this via `tail_returned_var`, but the byte-identical oracle+scg mirror needs a reliable direct-Var-tail signal that scg's `mvbv` can't give for a COMPOUND tail (it leaks the last nested Var — e.g. `return len(buf)` would mark `buf`), so a correct mirror needs new machinery in the differential-critical `dump_texpr` walk. The common factory pattern — return `shared_new(...)`/a call DIRECTLY (an rvalue transfer, no exemption) — works. **Slice-3b-full = lift the guard:** add a reliable direct-Var-tail signal (a depth/sequence tag on `mvbv`, or reset it across compound arms) then mirror inkwell's returned-Var drop exemption into the `llvm_dump.rs` oracle (a `ret_exempt` field/param + `tail_returned_var` helper; ALSO handle nested-block-tail + `if/else`-branch-tail returns) and scg (the Block/Return/param-frame drop drains). A prior all-in-one 3b attempt was reverted (the `mvbv` compound-tail leak shrank existing array/struct fixtures) — a correct fix must FIRST make direct-Var-tail detection reliable.
>
> **▶ NEXT STEP — M1.4b: `Mutex<T> = Shared<SentinelMutex<T>>`** (ADR 0071 D4/D5, ~400 LOC). The co-ownership/refcount/drop is now solved by `Shared<T>`; `Mutex` layers on: the `SentinelMutex` runtime cell (near-clone of `SentinelChannel`, `parking_lot` behind a raw ptr), the lock/unlock builtins, the fallible `lock() -> ?Guard<T>` (the `recv`-style `(status, out)` → success arm binds a guard whose `*guard` reads/writes `T` and whose scope-exit drop unlocks — a new guard no-escape borrow-check rule, D3), and the D5a deadlock tiers (`LockTimeout` always-on via `try_lock_for`; opt-in `Deadlock` wait-for-graph over public lock identity). Follows the same slice rhythm (runtime cell → FnId-base shift both compilers → type+lowering → guard drop → self-host mirror). Then **M1.4c** = secret `Shared<secret T>`/`Mutex<secret T>` (D6: the container-interner secret fix — also unblocks `Channel<secret T>` — + broker-backed mlock/zero alloc + the `OpenResult`-shaped secret `lock()`).
>
> HANDOFF STATE: four-check green (build clean; `cargo test --workspace --no-fail-fast` = exactly the 18 known pre-existing Windows failures, zero new; `pass` 144 incl. `c71_shared`+`c71_shared_rc`; ui 41 incl. `c71_shared_return_named`; all 9 self-host differential stages byte-identical, both bootstrap fixed points hold; doctests + clippy clean); working tree clean (3b committed `c18d7be`). Interner kinds in use: Shared = 17; next free = 18 (reserved for `Mutex` per ADR 0071 D9). `is_spawn_word_scalar` now includes `Shared`.
>
> **Deferred/other directions** (if the maintainer pivots off M1.4): slice-3b-full (the named-Shared-return exemption, above); full capturing closures (ADR 0024 D10); the D3-revisit direct-call syntax (`op(x)`) unmirrored in scg; generalizing `Channel<T>`/`process_send`/`process_recv` beyond `i64` in scg; **M2.4c-2**; BACKLOG §11.6/§11.7 + ADR-0067 tails + macOS verification.

**▶ DONE (2026-07-02) — `pass_c5d4_file_io` fixed; the "runtime crash" it was blamed on does not exist (a misread exit code).** A background task flagged this test as a Windows runtime crash: it exits `-1073740791` (`0xC0000409`) instead of the expected `5`, and that NTSTATUS is *named* `STATUS_STACK_BUFFER_OVERRUN`, so the working hypothesis was unsafe buffer handling in the `sentinel_write_file` runtime builtin on an I/O failure. **That hypothesis is wrong, proven by a minimal repro:** a standalone 2-line Rust program whose entire body is `std::process::abort()` produces the *exact same* exit code on this MSVC toolchain — because Windows `abort()` terminates via `__fastfail`, which reports `0xC0000409` regardless of whether any stack cookie was actually involved. So the code is a red herring: the runtime was aborting *cleanly and by design* (ADR 0035 D5 mandates file I/O is panic-on-failure — `sentinel_write_file` does `eprintln!(...); std::process::abort();` on any `std::fs::write` error), and `crates/sentinel-runtime/src/lib.rs`'s `write_file`/`read_file`/`path_from_bytes` have **no unsafe buffer bug** — reading them confirms every raw-pointer deref is length-guarded and the failure path is a plain `abort()`. The runtime was left untouched (changing it would violate ADR 0035 D5 anyway). **The actual bug was two hardcoded Unix `/tmp` paths:** the fixture `tests/pass/c5d4_file_io.sentinel`'s own `write_file("/tmp/sentinel_c5d4_io.txt", ...)` call AND the `pass_c5d4_file_io` test's Rust-side path both hardcoded `/tmp/...`, which on Windows resolves drive-relative to a nonexistent `G:\tmp` → `write_file` correctly aborted → the test read the abort code as a crash. Fixed both: the fixture now uses a plain relative filename (`sentinel_c5d4_io.txt`; Sentinel has no portable-temp-dir builtin), and `pass_c5d4_file_io` no longer uses the shared `build_and_run` helper (which doesn't control the child's CWD) — it inlines that helper's build+run steps but spawns the compiled binary with `.current_dir(std::env::temp_dir())` so the relative path resolves into the OS temp dir on every platform, and removes the temp file before + after for hermeticity. The fixture is in the self-host differential corpus, so the path change was re-verified byte-identical across all 9 stages (a relative vs absolute string literal changes the emitted bytes but stays identical between oracle and scg). Four-check green; the known pre-existing Windows-only failure count drops 19→18 (the `pass` target now fully passes, 141→142 green). **Lesson for the record (this corrects a note I myself added to these docs in the prior turn):** `0xC0000409` on Windows means "a Rust `abort()`/panic-abort fired," NOT "memory was corrupted" — do not diagnose it as a buffer overrun without a minimal repro.

**▶ DONE (2026-07-02) — ADR 0071 (`Shared<T>` + `Mutex<T>`) PROPOSED+PINNED, and M1.4-0 (the D5 gate) built + assessed.** The user picked M1.4 Mutex off the decision menu, then chose the full `Shared<T>` + `Mutex` path (over the leaked-atomic MVP or example-only) and secret-in-scope-for-v1. Because M1.4 is design-first (a `Mutex` needs *shared* ownership, which the channels/ownership-transfer spine deliberately avoids), the deliverable was an ADR, not code — grounded by a **5-agent parallel research workflow** over the borrow checker, drop machinery, runtime+broker, secret discipline, and prior design intent. The research isolated the crux: every concurrency handle today is `Copy` + deliberately leaked, and `Shared<T>` must be the **first handle that is `Copy`-for-the-checker (frictionless duplication, no move-tracking/false-rejections) YET drop-emitting (refcount-- on scope exit, free-on-zero)** — a combination that doesn't exist in the type lattice today; reconciling it is the whole M1.4 cost. It also found the good news: drop *timing* is fully solved and reusable (scope-exit drop incl. early return/break/continue), the drop *content* gap is a small localized arm (no general `Drop` trait needed), and the secret analysis is clean (no new `secret_leak` sink — a secret-dependent lock decision is already a rejected branch). [ADR 0071](decisions/0071-shared-ownership-and-mutex.md) pins D1–D9 (see the STATE.md Latest entry for the full list); maintainer sign-off 2026-07-02; committed `fc6fbcc`. **M1.4-0 — honoring D5's own "don't build until an `examples/` program proves channels insufficient" gate as the FIRST step, not skipping it:** two real, compiling, deterministic examples — `shared_counter_via_channel` (the shape channels handle WELL: commutative fan-in accumulation, 3×14=42) and `shared_sequence_via_channel` (worker-side correlated request-reply, which hits a HARD WALL — replies are unaddressed and Sentinel has no channel-of-channels + no select to correlate them, so it's an expressiveness wall, not just a throughput cost, 7×+6=42) — plus a written weigh-up ([0071-m14-0-analysis.md](decisions/0071-m14-0-analysis.md)). **Honest verdict:** the gap is real but its incidence in Sentinel's mostly-embarrassingly-parallel crypto/security domain is low (no in-domain example needed it); proceeding to M1.4a is justified primarily on `Shared<T>`'s standalone value (shared read-only data + atomics + the `Channel<secret T>` unblock), with the correlated-RMW `Mutex` case as the pinning motivation — recorded so the narrowness is on the record. Both examples registered in `examples.rs` (both `--separate` and merge paths green); `every_example_is_registered` + clippy clean; **no compiler change yet** (the 9 selfhost differential stages are untouched). Next: M1.4a (see RESUME HERE).

**▶ DONE (2026-07-02) — scg self-host mirror: `Type::Fn`/`apply` CODEGEN, closing the last scope cut from the prior entry.** The user asked for the codegen-stage oracle to be ported and scg's own indirect-call codegen designed — done via plan mode, with an independent validation pass (a Plan-mode review agent given the full design + every cited file's live content, tasked with stress-testing it against the actual source before any code was written) that caught concrete issues before implementation. **Oracle port (`crates/sentinel-driver/src/llvm_dump.rs`, 3 changes):** `llvm_ty` gained `Type::Fn(_) => Ok("ptr".to_string())`; `TypedExprKind::FnRef`'s `Err(...)` became `Ok(format!("@{}", sig.name))` (a bare function-pointer constant, no instruction — mirrors `VEC_NEW_FN_ID`'s "return a constant literal" shape); `lower_call` gained an `APPLY_FN_ID` special case emitting `%v{v} = call {ret_ty} {f_op}({param_ty} {x_op})`, lowering `f` before `x` to match the real inkwell `lower_apply`'s own evaluation order. Confirmed empirically against LLVM 18's own verifier (`opt -passes=verify`) that an indirect call through a register and a call through a bare `@name` constant use identical surface syntax — no separate function-pointer-type annotation needed under opaque pointers. **scg's own side — its first codegen shape with no existing pattern to copy:** the blocker was that `cgo_operand` (94 call sites, no `src` parameter) needs a function's *source-level name* to render a `Fn` value as `@<name>`. Solved by mirroring a pattern already live in this exact codebase for struct names: new `ufnb`/`ufns`/`ufne` TyCtx fields cache fn-name bytes at the existing Pass-1 top-level scan (mirroring `snb`/`sts`/`ste`, whose own comment says *"so `render_type` needs no `src`"*), read back via a new `cg_fn_name_to` (mirrors `cg_struct_name_to`), consumed by a new `cgo_operand` kind 5. **The validation pass caught a real bug before it was ever written:** `cgo_operand`'s final branch was a bare `else` (not `else if kind == 3`) — silently catching "anything unrecognized." Bolting kind 5 on after it would have made every `Fn` value silently misrender as a null constant (`{ i1 0, ... }`) instead of `@name` — a wrong-output bug with no compiler error, exactly the kind of defect this project's differential discipline exists to prevent. Fixed by making kind 5 an explicit `else if` *before* the (now-implicit) kind-3 fallback. **The same pass also caught a coverage gap:** grepping every `apply(` call site in the repo (corpus and `examples/` alike) showed the callee argument is *always* a bound Var — never a bare fn name passed directly — meaning the kind-5-as-callee path would have shipped completely untested. Fixed by extending `tests/pass/c70_scg_fn_apply.sentinel` to call `use_fn(square)` alongside the existing `use_fn(sq)` (`36 - 36 + 42 = 42`), forcing both a register-callee and a bare-constant-callee indirect call through the same fixture. `dump_apply_call` (added last session) gained the actual instruction emission: captures a snapshot of the collected-arg stack before dumping either arg, reads both operands back after collection, emits the indirect call, and calls `cg_reg` — relying on `cg_emit_call`'s own subsequent, unconditional call (which matches none of its dispatch arms for FnId 37 and falls to a no-op) to perform only its mandatory arg-stack cleanup without ever touching the result register. Verified three ways: `sentinel_codegen_matches_oracle_on_corpus` now *compares* the fixture's codegen for the first time (previously silently skipped); both bootstrap-fixed-point tests hold (scg's own new source self-compiles byte-identically); and a manual byte-diff reproduction confirmed the oracle and scg produce identical 642-byte output, including `call i64 %v1(i64 6)` (indirect call through a register) and `call i64 @use_fn(ptr @square)` (a bare-constant `Fn` operand) side by side. Four-check green (same known pre-existing Windows-only failures, zero new). **This closes the full 6-gap scg-mirror effort with zero remaining scope cuts on `Type::Fn`/`apply`** — see the NEXT DECISION menu above for what's left (only the distinct D3-revisit direct-call syntax follow-up).

**▶ DONE (2026-07-01) — scg self-host mirror: `Type::Fn`/`apply` (ADR 0070), 6 of 6 originally-tracked gaps closed — scoped to resolve/types/borrow/effects/mir, codegen structurally excluded.** The user's "proceed" pick off the prior menu's item 1, then narrowed to "mirror the 5 verifiable stages" once research surfaced that the codegen-stage oracle (`crates/sentinel-driver/src/llvm_dump.rs`, hand-maintained, used only by `selfhost_codegen.rs`'s differential) deliberately errors on `FnRef`/`apply` (`"Fn value not ported to the snc llvm oracle (ADR 0070 snc-only)"`) — so the differential framework can never reach scg's codegen for this feature regardless of what scg does; porting the oracle is a separate snc-side prerequisite, not a "mirror into scg" task. **New `Type::Fn` interner kind (16)**, storing `(param_ty, ret_ty)` handles directly (`mk_fn`/`render_type`'s `"Fn<A,B>"` text, no space after the comma) rather than Rust's arithmetic `FnValueSigId` scheme — scg's `intern_type` already stores two arbitrary handles uniformly, so this is simpler here. A new `fn_ref_sig` eligibility helper mirrors the Rust gate (non-generic, effect-free, exactly one word-scalar param, word-scalar return) for deciding when a bare fn-name reference is a valid `Fn` value. `borrow.sentinel`'s shared `Expr::Var` arm now falls back from a failed `sc_lookup` (not a bound var) to `fn_lookup` + a new `dump_te_fnref` helper (renders `(fnref #id)`, MIR-emits a zero-operand Opaque via the existing `mir_emit_opaque0`); `dump_te_call` gained an `fid == 37` special case (`dump_apply_call`, a new function in `infer.sentinel`) that dumps `apply`'s first arg, decodes `(param_ty, ret_ty)` from its `Type::Fn` handle, dumps the second arg against `param_ty`, and returns `ret_ty` as the call's own type — the "return type depends on the concrete Fn value passed in" shape `check_call`'s own `apply` branch already has. **Two real, previously-unmirrored gaps were found and fixed along the way, both invisible until a genuinely clean-typing `apply` fixture existed:** (1) `types/interner.sentinel`'s OWN separate `builtin_id` copy never had an `"apply"` entry at all (only `resolve.sentinel`'s copy did, added in v1 specifically to keep a `tests/ui` rejection fixture resolve-stage-clean — that fixture never reached the types stage, so this gap was invisible until now); the missing entry made `fn_lookup` return `-1`, and `append_int` — which has no negative-number handling, silently emitting zero bytes rather than a `-` sign — rendered `(call #` with the id missing entirely. (2) `types/mir.sentinel`'s `mir_put_callee` callee-name table also lacked an `"apply"` arm, silently falling through to its catch-all default (`"print_bytes"`) instead. Both fixed with one new arm each, mirroring every neighboring builtin's exact pattern. **A second, more serious bug surfaced only via the bootstrap-fixed-point tests (which compile scg's OWN new source through scg's OWN general-purpose codegen — not the corpus differential, which the fixture is structurally excluded from):** `dump_apply_call`'s inner `match rest { Args::Cell(..) => {...; 0}, Args::End => 0 }` is used as a discarded statement (its value never bound or returned) — the first such shape anywhere in 14k+ lines of selfhost source. Root cause, isolated to the exact LLVM register via manual oracle-vs-self-compiled IR comparison: the match-expression cg arm in `borrow.sentinel` reserves its result alloca via `cg_alloca(c, exp)`, and a discarded expression-statement is dumped with `exp == -1` ("no expectation" — an existing, widely-used convention, e.g. `Stmt::SExpr`). That `-1` collided with `cg_alloca_ptr`'s OWN reservation of `-1` in the same alloca-type pool to mean "force a bare `ptr`" (a kont-slot convention) — so the match's result alloca'd as `ptr` instead of `i64`, diverging from the oracle (which reads the expression's own already-resolved type, never a caller expectation, so it has no equivalent ambiguity). An initial fix attempt (deferring the alloca's type-commit until after the arms render, so it could fall back to the match's own inferred type) was tried and reverted — it broke alloca *ordering* (hoisted allocas must render in strict reservation order to stay byte-identical with the oracle, and deferring the outer match's commit past its nested children's own commits reordered it, breaking two unrelated pre-existing fixtures, `c5d1_enum.sentinel` and `selfhost_ast_drop.sentinel`). The actual fix is smaller and touches nothing order-sensitive: `cg_alloca_ptr` now reserves `-2` instead of `-1` (the one call site that needs a real "always ptr" sentinel), and the three render-loop copies (`cg_class.sentinel`, `cg_effects.sentinel` ×2) check `== -2` instead of `< 0` — `-1` now safely falls through to `cgo_ty`'s own pre-existing unmatched-handle fallback (`"i64"`), which is exactly what a discarded match-statement needs. Verified via the same manual byte-diff reproduction technique used for the SealedChannel MIR bug earlier this session (stage `selfhost/{parser,types}.sentinel` + parts into a scratch dir, build a real `scg.exe` via `snc build`, `snc merge`+`snc llvm` for the oracle, run `scg.exe` on its own merged source, diff line-by-line) before trusting the full test suite. New fixture `tests/pass/c70_scg_fn_apply.sentinel` (`apply(op, 6)` only — never bare `op(x)`, since the D3-revisit direct-call unification is a distinct, still-unmirrored feature; scg's `dump_te_call` keeps the same unconditional "vars win over fns" dispatch the Rust resolver has) brings resolve/types/borrow/effects/mir into parity for the first time; confirmed byte-identical across all 9 self-host differential stages, **both bootstrap-fixed-point tests hold**. Four-check green; `cargo test --workspace --no-fail-fast` showed exactly the same 19 known pre-existing Windows-only failures, zero new (`pass.rs` 140→141). **This closes the last of the 6 originally-tracked scg-mirror gaps within its now-clarified scope** — see the NEXT DECISION menu above for what's left (codegen needs a separate snc-side oracle port first; D3-revisit direct-call syntax is a distinct follow-up).

**▶ DONE (2026-07-01) — scg self-host mirror: `sealed_channel`/`sealed_process` bridge (ADR 0066 M2.4a / ADR 0069), 5 of 6 tracked gaps closed.** Continuing the same session's scg-mirror work (item 1 of the prior menu). Added scg's first NEW type interner kind this session — `SealedChannel` (kind 15) — mirroring `Process` (kind 14) across every touchpoint found by grepping ALL of `Process`'s own occurrences first: `mk_sealed_channel` + the `render_type` dump-text arm (`selfhost/types/interner.sentinel`), `is_move_type`'s Copy arm (`selfhost/types/borrow.sentinel`), a new `cg_is_sealed_channel` + its use in the handle→LLVM-type dispatch (`selfhost/types/cg.sentinel`), `builtin_id`/`builtin_ret` in both files, and the MIR callee-name table. Confirmed (not assumed) that `Process`/`SealedChannel` aren't writable in TYPE position on either compiler (no `resolve_type_expr`/`type_of_typeexpr` arm exists for either name) — so no type-expression-resolution work was needed for this slice. **Codegen is a genuine no-op**, matching the Rust oracle's `return self.lower_expr(&args[0])` exactly: both `Process` and `SealedChannel` lower to the same opaque `ptr`, so scg re-threads the argument's own `(kind, value)` operand pair directly (`(*c).cglk`/`(*c).cglv`) instead of allocating a new register — the same "value-level no-op" trick `declassify` already uses in this codebase (confirmed by reading its own cg-mode handling first). **A real divergence was caught, not missed:** the new fixture initially failed `sentinel_mir_matches_oracle_on_corpus` — scg's MIR dump showed `SealedChannel` where the oracle showed `SealedChannel<secret i64>`. Root cause: the Rust side has TWO type-name renderers — the ordinary `Display` impl (used for diagnostics, plain `"SealedChannel"`) and a separate `type_display` function (used for MIR/dump output, which hardcodes the `<secret i64>` suffix as a fixed string for this still-monomorphic type) — `render_type` in scg mirrors the latter, not the former. Fixed by hardcoding the same suffix; re-verified with a manually-reproduced oracle-vs-scg diff (byte-for-byte match) AND a full `--no-fail-fast` differential re-run (this also caught that `selfhost_resolve`/`selfhost_types` hadn't actually run yet in the first attempt — cargo's default per-target fail-fast stopped at the first failure alphabetically, a repeat of an earlier lesson this session). New fixture `tests/pass/c70_scg_sealed_bridge.sentinel` (guarded like `c66_process.sentinel`) brings the new type kind + bridge lowering into every self-host differential stage; all 9 confirmed byte-identical, both bootstrap-fixed-point tests hold. Four-check green; `cargo test --workspace --no-fail-fast` showed exactly the same 19 known pre-existing failures, zero new (`pass.rs` 139→140). **Only 1 gap remains** — see the NEXT DECISION menu above.

**▶ DONE (2026-07-01) — scg self-host mirror: `stdin_recv`/`stdout_send`/`arg_count`/`arg` lowering (ADR 0066 M2.4b/M2.4-follow-on), 4 of 6 tracked gaps closed.** The user's pick off the decision menu. A dedicated research pass first mapped the EXACT state of all 6 unmirrored areas (sealed_channel/sealed_process, stdin_recv/stdout_send, arg_count/arg, process_send/recv generalization, Channel<T> generalization, Type::Fn/apply) by reading every scg dispatch table directly — not inferring from the FnId-mirror comments, which turned out to undersell how absent some of these were (e.g. `apply`'s FnId was resolvable by NAME in scg since ADR 0070 v1, but had ZERO type-check or codegen dispatch — a "reserve the number" stub, not a partial implementation). Scoped this session to the 4 builtins that were both genuinely green-field AND mechanically identical to already-proven scg patterns (verified by reading the Rust oracle's exact text-IR side by side with scg's existing arms before writing anything): `stdin_recv` = `channel_recv`/`process_recv`'s shape minus the handle arg; `stdout_send` = `channel_send`/`process_send`'s shape minus the handle arg; `arg_count` = `channel_new`'s bare zero-arg call; `arg` = `process_read`'s `{i64,ptr}`-assembly shape. Touched the same 7 mechanical sites every prior 2-builtin addition needed (confirmed via grep across ALL of `process_send`'s touchpoints first): `builtin_id` in BOTH `selfhost/resolve.sentinel` AND `selfhost/types/interner.sentinel` (each keeps its own separate copy — a real gotcha for anyone assuming one shared table), `builtin_ret` (only `stdin_recv`→`mk_nullable(c,0)` and `arg`→`mk_array(c,3)` needed entries; `stdout_send`/`arg_count` correctly fall through to the existing i64 default, like `process_send`/`channel_close`), the `cg_emit_call` dispatch in `selfhost/types/cg_effects.sentinel` (hand-transcribed from the oracle's exact text — byte-identical on the FIRST try, no iteration needed), 4 `cg_used_*` struct fields + declare-group entries + `tyctx` inits, and the MIR callee-name table. Correctly added the 4 new flags to the `cg_anydecl` OR-chain (noted, but deliberately did NOT fix, that `cg_used_procsend`/`cg_used_procrecv` are curiously absent from that same chain — a separate, unverified, pre-existing question outside this task's scope). **No FnId renumbering** (33-36 already existed on both sides), no new type interner kind, no resolve-stage dump changes needed. New fixture `tests/pass/c70_scg_stdio_arg.sentinel` (guarded behind a runtime-`false` flag exactly like `c66_process.sentinel`, since real stdin/argv content is environment-dependent) brought all 4 builtins into every self-host differential stage for the first time — confirmed byte-identical across all 9 stages including both bootstrap-fixed-point tests, verified by actually running them (not assumed). Four-check green; the full `cargo test --workspace --no-fail-fast` run showed exactly the same 19 known pre-existing Windows-only failures across the same 5 targets, zero new ones (`pass.rs` 138→139 passed, reflecting the new fixture). **5 gaps remain** — see the NEXT DECISION menu above for the precise breakdown and suggested order.

**▶ DONE (2026-07-01) — ADR 0070 D3-revisit: unified `apply(f, x)` with ordinary `f(x)` call syntax, same session as v1 + M-cont.** Closes D3's own named follow-up (chosen off the post-M-cont decision menu). A `Fn<T,R>`-typed local var can now be called directly (`let op = square; op(5)`) — `apply(f, x)` stays valid too, the two spellings unified rather than one replacing the other. **The whole change lives at the TYPES stage; ADR 0020 D5's "vars win over fns" resolve-stage dispatch is untouched, not even renamed** — resolve never carried type information and never needed any (it already deferred validation to types for the pre-existing Kont-only case). `check_resume_kont_expr` (`crates/sentinel-types/src/lib.rs`) becomes a three-way match on the callee var's type: `Type::Kont` keeps its exact pre-existing body byte-for-byte; a new `Type::Fn(sig_id)` arm decodes `(param_ty, ret_ty)` via the existing `fn_value_sig_param_ret`, type-checks the one argument, and hand-builds `TypedExprKind::Call{id: APPLY_FN_ID, args: [f, x], type_args: []}` — the **identical** shape `check_call`'s `apply` branch already produces for `apply(f, x)`, so every downstream stage (borrow-check, effect-check, MIR, both codegen backends) needed **zero new code**; any other type is the new `TypeError::CalleeNotCallable`. No new runtime symbol, no ABI change, and — unlike nearly everything else this session — **no `FnId` renumbering** (reuses `APPLY_FN_ID`, not a new builtin). Two more new diagnostics: `FnValueArityMismatch` (wrong arg count — `Fn<T,R>` is always one param) and `FnValueArgMismatch` (wrong arg type — kept distinct from `apply`'s own `CallArgMismatch` since there's no `apply` token in the source to name). **A genuine pre-existing bug found and fixed along the way:** `apply`'s own `CallArgMismatch` on a wrongly-typed argument had been dead code since the M-cont amendment — `check_expr(&args[1], Some(param_ty), ...)` routes an `Some(expected)` hint through `coerce_to_expected`, which throws a generic `Mismatch` first. Fixed at both call sites by passing `None` instead (mirroring `check_handle_expr`'s own arm-body check, which already documents exactly this reasoning); loses no legitimate coercion since `Fn<T,R>`'s param is always a plain word-scalar. **Self-host impact: none, verified.** Resolve's dump format is unchanged (no `selfhost/resolve` mirror needed); the 3 new `tests/ui/c70_*.sentinel` fixtures (`callee_not_callable`, `fn_value_arity_mismatch`, `fn_value_arg_mismatch`) are permanent types-stage rejects, so while they sweep into `selfhost_resolve.rs`'s corpus test (safe — resolve untouched), they're excluded from `selfhost_types.rs`'s corpus test (skips fixtures the oracle doesn't type-check successfully) — so a real pre-existing gap in `selfhost/types/borrow_arms.sentinel`'s `dump_te_call` (no type gate at all on the resumed var) stays provably unreached. No shared helper between `check_call`'s `apply` branch and the new arm (matches this codebase's own convention — 8 near-duplicate `check_call` special-cases, none sharing a helper); drift is guarded instead by a new unit test asserting `apply(op, 5)` and `op(5)` type-check to the identical shape. Demonstrators: `examples/lang/fn_value.sentinel` gained a `call_direct` helper alongside `apply_to`; `fn_value_generic.sentinel`'s `apply_bool` switched to direct syntax (proving the unification for a non-`i64` instantiation too) — both still exit 42. Four-check green; the full `cargo test --workspace --no-fail-fast` run showed exactly the 19 known pre-existing Windows-only failures (5 targets), zero new ones; all 9 selfhost differential stages byte-identical, both bootstrap fixed points hold. See ADR 0070's Amendment 2 (D13-D16).

**▶ DONE (2026-07-01) — ADR 0070 M-cont: `Fn<T,R>` GENERALIZED to any word-scalar pair, same session as v1.** Mirrors the `Task<i64>`→`Task<T>` (M1.1) / `Channel<i64>`→`Channel<T>` (M1.2b-cont) precedent — ship the
smallest concrete shape, generalize immediately after. `Type::Fn` is now `Type::Fn(FnValueSigId)`, but
UNLIKE `Channel<T>`'s pre-interned-`ChanId` trick (needed because 4 builtins consume the element) or a
general interner table, the id is **pure arithmetic**: `fn_value_sig_id_for(param_ty, ret_ty)` /
`fn_value_sig_param_ret(id)` compute `param_index * 6 + ret_index` over the same 6-word-scalar
enumeration `channel_chanid_for` uses (independently duplicated — no incidental coupling between the two
features). **NO interner table, NO new `TypedProgram` field, NO threading through `resolve_type_expr` or
the check pipeline** — simpler than `Channel<T>`'s own generalization, because `Fn`'s only consuming
builtin (`apply`) can just decode the id directly. `apply(f, x)` — concrete in v1, so it rode the generic
call-checking path free — now needs `check_call` special-casing (the `process_recv` pattern): type-check
`f` first, decode `(param_ty, ret_ty)` from its `Type::Fn(id)`, check `x` against `param_ty`, return
`ret_ty`; a non-`Fn` first arg gets a dedicated `ApplyTargetNotFn` diagnostic (not `CallArgMismatch`,
which would misleadingly imply one "expected" type). `lower_apply` derives both LLVM types from the typed
AST directly (`x.ty`, the `Call`'s own `expr.ty`) — no `type_args` needed. **GOTCHA CHECK (verified, not
assumed): does this reopen v1's resolve-stage-corpus surprise?** No — both `tests/ui/` fixtures were
re-examined against `selfhost_resolve.rs`'s `collect_fixtures` sweep: `c70_fn_value_ineligible` is
unchanged (still covered by v1's `builtin_id`/`Expr::Var` mirror); the replacement
`c70_fn_type_args_unsupported` (now `Fn<u128, i64>`, still genuinely rejected — `u128` isn't a
word-scalar) only touches the "Fn" TYPE-EXPRESSION arm, which carries no semantic type info at resolve
time — so **zero further `selfhost/` changes** were needed, confirmed by running all 9 differential
stages, not by assumption. Demonstrator `examples/lang/fn_value_generic.sentinel` (`Fn<u8,u8>` /
`Fn<f64,f64>` / `Fn<bool,bool>` through the same `apply` builtin, exit 42, both `--separate` and merged).
Four-check green; all 9 selfhost differential stages byte-identical. See ADR 0070's M-cont amendment
(D10-D12) for the full design writeup.

**▶ DONE (2026-07-01) — ADR 0070 v1: non-capturing first-class function values (`Fn<i64,i64>` v1).** The
foundational-rock item from the prior menu, scoped DOWN to the safe slice: not full closures (ADR 0024
D10 stays deferred), just a non-capturing function VALUE — a top-level, non-generic, non-builtin,
effect-free `(i64) -> i64` fn referenced by bare name (`let op = square;` → `ResolvedExprKind::FnRef` /
`TypedExprKind::FnRef`, typed `Type::Fn` — a plain **unit** variant, Copy, lowering to a bare LLVM
function pointer; no interner table at v1, mirroring the `Type::SealedChannel`-at-M2.4a precedent rather
than `Channel`'s interned-from-day-one). Invoked **indirectly** via a new builtin **`apply(f: Fn<i64,i64>,
x: i64) -> i64`** (FnId 37, user-fn base 37→38) — deliberately NOT ordinary `f(x)` syntax: `ident(args)`
over a bound local var already unconditionally resolves to a kont resume-call (ADR 0020 D5, "vars win
over fns"), and touching that dispatch to disambiguate kont-vs-Fn is exactly the differential-critical,
security-relevant surface this session chose not to rush. `apply` resolves through the *existing*
`fn_table` lookup, never the `vars`-shadowing branch — zero overlap. Unblocks the worker-pool motivation:
a pool/worker body can take `op: Fn<i64,i64>` instead of one hand-written worker per operation
(`spawn` of an indirect target stays deferred — `Type::Fn` isn't a spawn-word-scalar yet). No new runtime
symbol, no ABI change, **zero borrow-checker capture machinery**. Demonstrator
`examples/lang/fn_value.sentinel` (two distinct fns through one `apply_to` param, 36+6=42, both
`--separate` and merged); ui rejections `c70_fn_value_ineligible` (wrong arity) + `c70_fn_type_args_unsupported`
(`Fn<T,R>` fixed at `Fn<i64,i64>` for v1). **GOTCHA (a genuine surprise, corrected mid-session): "keep the
demonstrator in `examples/`" does NOT fully shield the differential** — `selfhost_resolve.rs`'s corpus
check sweeps EVERY `tests/pass` + `tests/ui` fixture that resolves cleanly into the resolve-stage
byte-parity requirement, regardless of which LATER stage rejects it. A ui fixture demonstrating D4
eligibility inherently resolves cleanly (eligibility is a types-stage check), so it landed in that corpus
and diverged. Fixed with two small, resolve-stage-ONLY additions to `selfhost/resolve.sentinel`:
`builtin_id("apply") → 37`, and the `Expr::Var` dump arm's `sc_lookup`→`fn_lookup` fallback (mirroring the
Rust resolver's `ExprKind::Var` fix) so `(fnref #N)` renders identically. The type-check/codegen lowering
mirror (an interner-or-unit kind for `Type::Fn` + cg arms in `selfhost/types/cg*.sentinel`) is STILL
genuinely deferred — no `tests/pass` fixture exercises it — folded into the existing tracked scg-mirror
follow-up (now item 5 above). Four-check green; **all 9 selfhost differential stages byte-identical**,
both bootstrap fixed points hold. See ADR 0070 (its D7 has the full "lesson learned" writeup for whoever
next tries the "ship snc-only, demonstrator in examples/" deferral pattern).

**▶ DONE (2026-06-30) — ADR 0066 M1.2b-cont: GENERIC word-scalar in-process channels (`Channel<u8>`/`<bool>`/`<i32>`/`<f64>`/`<ptr>`).** The in-process twin of the M2.3b process-channel generalization. The element is encoded into / decoded from the channel's i64 slot (the M1.1 spawn encode: zext narrow int / bitcast f64 / ptrtoint ptr) — runtime stays i64-based, **NO new symbol, NO FnId change**. The word-scalar `Channel<T>` types are **pre-interned at FIXED ChanIds 0..=5** at sig setup (`channel_chanid_for`/`channel_elem_for` — so a `Channel<T>` annotation maps to a stable ChanId WITHOUT threading the `channels` interner through the checker, the snag the design avoids), and the 4 builtins are **special-cased in `check_call`** (the M2.3b pattern): `channel_new()` context-typed (element from the expected type), `send` encodes (by value kind, no type_args), `recv -> ?T` decodes (element from the channel arg's ChanId, type_args for non-i64), `channel_close` accepts any `Channel`. Codegen `lower_channel_send`/`_recv` + the `snc llvm` arms gained the encode/decode (mirror `lower_process_send`/`_recv`). **The `i64` case is BYTE-IDENTICAL to M1.2** → all 6 selfhost differentials green with **NO scg change** (snc-side, like M2.3b; generic elements live in `examples/lang/channel_generic.sentinel` → 42). **GOTCHA: this surfaced the `c66_channel_element_unsupported` ui fixture** — it used `Channel<bool>` (now ACCEPTED), so it stopped rejecting → the MIR differential picked it up + diverged (snc's pre-interned ChanId 3 for bool vs scg's on-demand ChanId 1); FIXED by pointing it at `Channel<u128>` (a non-word-scalar — doesn't fit the i64 slot — still `ChannelElementNotSupported`) + re-blessing the snapshot. (`unwrap_or(m, 0)` on a `?u8` needs `0 as u8` — the default literal else infers `i64` → a type-arg-inference conflict.) Four-check green. **The reusable worker-pool LIBRARY stays BLOCKED on a first-class-function/closure mechanism (Sentinel has none)** — M1.3 remains the demonstrator examples; closures would be their own ADR-bearing rock.

> **▶ TWO ITEMS DEFERRED (maintainer-confirmed 2026-06-30, "defer both") — assessed lower-ROI; here is the assessment so the next session can decide quickly:**
> - **scg self-host mirror of the sealed/stdio/arg builtins** — *medium value* (lets `scg` LOWER `sealed_channel`/`sealed_process`/`stdin_recv`/`stdout_send`/`arg_count`/`arg` → self-hosting parity), but **DIFFERENTIAL-CRITICAL**: a mistake silently breaks a bootstrap fixed point (a serious regression). The FnId BASE is already mirrored (the corpus differential is green); what's missing is the per-builtin LOWERING in `selfhost/types/cg*.sentinel` (mirror the existing `process_read`/`channel_recv`/`channel_send` selfhost arms) + a SealedChannel type kind (like `Process` kind 14) + a focused `tests/pass` fixture exercising the builtins (guarded, NO crypto stack — just the lowering) to bring them into the differential. Do this in a FRESH focused session (it's the rush-dangerous one).
> - **M2.4c-2 generic word-scalar `SealedChannel<secret T>`** — *low value*: promoting the unit `Type::SealedChannel` → interned `SealedChannel(SealId)` (the `Channel<T>` pattern, ~11 sites + a context-typed constructor like `vec_new`) buys only cosmetic type-precision — the value-based `seal`/`open`/`sealed_send` API doesn't tie to the channel's element type, and **M2.4c-1's variable-length `secret [u8]` already covers arbitrary secret payloads**. If done, do it BEFORE the scg mirror (so the mirror covers the final type shape). Genuinely optional.
>

**▶ DONE (2026-06-30) — own command-line argument reflection (`arg_count`/`arg`), an M2.4 follow-on.** Two builtins **`arg_count() -> i64`** + **`arg(i: i64) -> [u8]`** let a program read its own invocation (symmetric to `process_spawn` passing argv to a child — so a self-spawn / child-role-detect pattern no longer needs path templating). Runtime `sentinel_arg_count`/`sentinel_arg` over `std::env::args` (reads the OS args directly → works under the custom LLVM entry); abi-v1 36→**38**; **FnId base shift 35→37** in both compilers (mirrored into `selfhost/`, all 6 differentials byte-identical); codegen mirrors the `read_file` result shape for `arg`. Custom test `crates/sentinel-driver/tests/argv.rs` (real args: `arg_count` reflects the count, `arg(1)` of `"*"`→42). Four-check green.

**▶ DONE (2026-06-30) — ADR 0066 M2.4c-1: variable-length `secret [u8]` sealed records + D5 fixed-width padding.** `seal_bytes`/`open_bytes` (in `std::security::sealed`) seal an arbitrary `[secret u8]` message, not just a `secret i64` — padded to `4 + maxlen` bytes (a length prefix + the message + zeros) so the wire ciphertext length is CONSTANT for a given `maxlen` (no length leak, D5). `open_bytes` declassifies the length prefix (the verified receiver's legitimate knowledge — the wire was constant-length) + clamps it (a forged tag → empty, no OOB read); the message bytes re-emerge `secret`. Reuses the ssh record cipher (no new primitive, **NO compiler change**). `examples/lang/sealed_bytes.sentinel`: a 3-byte message (sum 42) round-trips secret + authenticated, and two different-length messages frame to the same length → 42.

**▶ DONE (2026-06-30) — ADR 0066 M2.4b REAL PIPE TRANSPORT: the self-stdin/stdout builtins + a real parent↔child sealed handshake over a process pipe (snc-side).** Closes the blocker (a spawned CHILD could not read its own stdin). **Two new builtins** `stdin_recv() -> ?i64` + `stdout_send(v: i64) -> i64` — the child-side twins of `process_recv`/`process_send` (read/write one 8-byte LE i64 frame on THIS process's OWN stdin/stdout, flushed). Runtime symbols `sentinel_stdin_recv(out)->i64` (0 some/1 EOF) + `sentinel_stdout_send(v)->i64` (0/-1), **abi-v1 34→36** (`abi_v1_runtime_symbol_set`). Adding them shifted the **user-fn FnId base 33→35** in both compilers (Rust auto + `FIRST_USER_FN` + the 4 driver golden dumps +2 + resolve/types unit tests; selfhost via the mechanical FnId-base sed `33→35` over `selfhost/{resolve,types,effects}.sentinel` — **all 6 selfhost differentials byte-identical**, both fixed points hold). Codegen: inkwell `lower_stdin_recv`/`lower_stdout_send` + the `snc llvm` arms, i64-only (no `Process` arg, no element encode — simpler than the process_* twins). **Runtime-verified** via a smoke test (a program `stdout_send(42)` writes the exact 8-byte frame; `stdin_recv()` reads a piped frame → 42). A new stdlib **`std::security::sealed_pipe`** drives the M2.4b KEX over the actual pipe: `sealed_pipe_client(sc: SealedChannel<secret i64>, …)` frames bytes with `process_send`/`process_recv` over the child's `Process` (recovered via `sealed_process`); `sealed_pipe_server(…)` frames on the child's OWN stdin/stdout via the new builtins; both call the same transport-free `sealed_kex` core (Q_C=32B=4 frames, the response=128B=16 frames, a sealed record=40B=5 frames — fixed-width, D5). **End-to-end test `crates/sentinel-driver/tests/sealed_pipe.rs`** (a NEW custom test, not examples.rs): builds the CHILD program (`tests/fixtures/sealed_pipe/child.sentinel`, the server), templates the PARENT (`parent.sentinel.in`, `__CHILD_PATH__`→the child's absolute forward-slashed path, since Sentinel has no argv/own-path reflection yet) + builds it, runs the parent → it spawns the child, runs the authenticated x25519 handshake **over a real pipe** (the parent pins the child's ed25519 host key), seals `secret i64` 42, sends it as a record; the child `open`s it (the secret **re-emerges secret on the verified child**) and exits 42; the parent `process_wait`s → 42. **So the core M2.4 vision — a privilege-separated, authenticated, AEAD-encrypted secret-cross-process channel — works END-TO-END for `secret i64`.** Four-check green (clippy clean; pass 138 + pre-existing `/tmp`; all differentials; the new test). **GOTCHAS:** (1) the FnId-base shift is again differential-critical — mirror the selfhost base or the whole corpus diverges (the M2.1/2/3/M2.4a lockstep); (2) `Process` is anonymous (not writable in type position), so the pipe helpers take `SealedChannel<secret i64>` (writable, via the M2.4a `resolve_type_expr` arm) + recover the `Process` with `sealed_process`; (3) NO deadlock — the handshake is strictly request/response and every frame is flushed, the byte-strings (32/128/40 B) fit the OS pipe buffer; (4) the demo needs the child's path templated in (no argv reflection — a separate future gap if a self-spawn pattern is wanted). **▶ NEXT:** (a) **M2.4c** — generic `secret T` elements + variable-length `secret [u8]` records + the D5 padding policy (promote the unit `Type::SealedChannel` → interned `SealedChannel(SealId)`); OR (b) the **scg self-host mirror** of the whole sealed stdlib (seal/open + the bridge + sealed_kex + sealed_pipe lowering + stdin_recv/stdout_send cg — then `tests/pass` differential fixtures become possible). Also a smaller follow-on if wanted: **argv / own-path reflection** (so a program can self-spawn / detect a child role without templating). Still pending: the scg mirror of M2.3b; the generic in-process channel builtins; the worker-pool library.

**▶ DONE (2026-06-30) — ADR 0066 M2.4b-crypto: authenticated x25519 KEX + per-direction keys + counter-nonce sealed stream, verified IN-PROCESS (snc-side; ADR 0069 D3/D4).** Establishes a SealedChannel's session keys via the SAME x25519 KEX + ed25519 host-key auth + `ssh_kdf` key derivation `std::net::ssh` runs over a socket — here as **transport-free CORE functions** in the new stdlib **`std::security::sealed_kex`** (`sealed_kex_client_msg(eph)->[u8]` / `sealed_kex_server(eph,host_seed,qc_pub,cookies,banner)->SealedServerKex{resp,keyc,keyd,session_id}` / `sealed_kex_client_finish(eph,resp,expected_host,cookies,banner)->SealedSession{keyc,keyd,session_id,authed}` take/return the wire BYTES, no I/O), **reusing the pub verified-CT `x25519`/`ed25519_{pubkey,sign,verify}`/`ssh_{kexinit_payload,hostkey_blob,exchange_hash,kdf}`** — NO new primitive (D2), **NO compiler change** (pure stdlib + example, so NOT differential-touching; both fixed points byte-identical by construction). **Auth (D3, ssh host-key model):** the initiator (parent) verifies the responder's (child's) ed25519 host signature over the exchange hash AND **pins** the host key (`ct_eq(&hpubv, expected_host, 32)` — the parent spawned the child, so it knows the expected key) → `authed = sig_ok & pin_ok` (an unauth/MITM'd KEX = `authed 0`; no unauth default). **Directional keys + nonces (D4):** the exchange yields `keyc` (initiator→responder) / `keyd` (responder→initiator) via `ssh_kdf` letters 'C'(67)/'D'(68); a sealed stream uses **monotonic counter nonces** per direction (the `seqnr` of M2.4a's `seal`/`open`, 0,1,2… never reused). Demonstrator `examples/lang/sealed_session.sentinel` runs **both KEX halves in-process** (wire bytes passed between them) → both derive matching `keyc`/`keyd`, the initiator authenticates the host, a **3-message counter-nonce sealed stream** (seqnr 0/1/2) re-emerges secret → exit 42 (5+15+20 + keyc_match + authed). **This removes M2.4a's two big caveats at the crypto level** (the fixed pre-shared key → an authenticated x25519 exchange; single-message → a counter-nonce stream). **KEY ENABLER:** all the ssh KEX helpers are `pub` + `ssh.sentinel` is socket-free, so the transport-free orchestration reuses them with ZERO crypto duplication; `ssh_seal`/`ssh_open_*` already BORROW the key, so multi-message reuse is clean. **▶ DEFERRED (the real-pipe BLOCKER):** driving the handshake over a REAL parent↔child pipe needs a **self-stdin-read builtin** — the spawned CHILD must read what the parent sends, but ALL `sentinel_process_*` runtime symbols are parent→child (no "read MY OWN stdin"); the child also writes its stdout via `print_bytes` (exists) but can't READ its stdin. That builtin is a follow-on (a new runtime symbol → another FnId-base shift → selfhost mirror; + a child-program demo mechanism that doesn't fit the examples.rs harness — likely a custom test). Maintainer chose the in-process-verification path (option 1) for M2.4b; the self-stdin infra is the next decision. clippy + both sealed example tests (`--separate` + merged → 42) green; `every_example_is_registered` green. **▶ NEXT:** (a) the **self-stdin builtin** + a real parent↔child sealed handshake demo (the "fully real" D3 transport), OR (b) **M2.4c** (generic `secret T` + variable-length `secret [u8]` + D5 padding; promote the unit `Type::SealedChannel` → interned `SealedChannel(SealId)`). Also still pending: the scg mirror of the sealed stdlib (seal/open + the bridge + sealed_kex lowering — then `tests/pass` differential fixtures become possible); the scg mirror of M2.3b; the generic in-process channel builtins; the worker-pool library.

**▶ DONE (2026-06-30) — ADR 0066 M2.4a: `SealedChannel<secret i64>` (snc-side), the AEAD-encrypted secret-cross-process path (ADR 0069, the D8a escape).** A `secret` crosses a process boundary only via a **cryptographic `declassify`**: `seal` AEAD-encrypts a `secret i64` so only PUBLIC ciphertext touches the pipe; `open` decrypts it so the value re-emerges `secret i64` on the verified receiver (`secret_leak` keeps CT). **Architecture (maintainer chose "compiler Type + stdlib ops"):** a unit **`Type::SealedChannel`** interner variant (the fence-as-type, D1/D9 — a non-secret element is a type error: ui `c66_sealed_channel_public_element`) wired as a sibling of `Type::Process` across ~11 sites in sentinel-{types,codegen,driver,borrow-check}; two identity-ptr **bridge builtins** `sealed_channel(Process)->SealedChannel` / `sealed_process(SealedChannel)->Process` (FnId 31/32) → **user-fn base shift 31→33** (auto in Rust + the 4 driver golden dumps +2 + resolve/types unit tests; mirrored into `selfhost/{resolve,types,effects}.sentinel` via the mechanical FnId-base sed — **NOT deferrable, the differential breaks otherwise**; the bridge LOWERING is never reached by the corpus so no scg cg mirror needed). A stdlib **`std::security::sealed`** module (`seal`/`open`/`sealed_send`/`sealed_recv`) reuses the verified-CT **ssh record cipher** (`ssh_seal`/`ssh_open_verify`/`ssh_open_payload`, key64+seqnr) — **NO new crypto primitive** (D2), **NO new runtime symbol** (`abi-v1` untouched at 34; the 40-byte fixed-width frame = 5 PUBLIC i64 over M2.3 `process_send`/`process_recv`, D5). `open -> OpenResult { ok: i64, v: secret i64 }` because **`?(secret i64)` is unrepresentable** (`NullableInner` has no `Secret` variant — sentinel-types:389/734) → the public verdict bit is a separate field (auth failure = typed verdict, never a panic). A `resolve_type_expr` "SealedChannel" arm (snc-only) makes `SealedChannel<secret i64>` writable in type position (so the stdlib param annotation works); `Process`/`SealedChannel` stay anonymous elsewhere (write `let p = process_spawn(...)` inferred, NOT `let p: Process`). Demonstrator `examples/lang/sealed_channel.sentinel` does an **in-process `seal`→`open` round-trip (RUNTIME-verified: ok==1, v re-emerges secret==42 → exit 42)** + a guarded pipe path (compiles `sealed_send`/`sealed_recv` + the bridge without a cross-platform child round-trip). Full four-check green under vcvars (resolve 112 / types 264 unit; 4 golden dumps; ui 35; pass 138 + the pre-existing `/tmp` `c5d4_file_io`; clippy clean); **all 6 selfhost differentials byte-identical → both bootstrap fixed points hold**. **GOTCHAs:** (1) the FnId-base shift IS differential-touching even though seal/open live in `examples/` — mirror the selfhost base or the WHOLE corpus diverges (the M2.1/2/3 lockstep, NOT the M2.3b "no-FnId-change" pattern); (2) `chacha20poly1305_decrypt` does NOT exist — `open` composes verify+decrypt from `ssh_cipher`; (3) `chacha20_xor` consumes (Move) key/nonce, but `ssh_seal`/`ssh_open_*` BORROW `key64` — hence reusing the ssh cipher (not the plain AEAD) is the clean path. **M2.4a uses a FIXED pre-shared key + FIXED single-message nonce** (sound ONLY for one message/direction — the LOUD flag is in the module + example headers). **▶ NEXT: M2.4b** — the authenticated x25519 handshake over the pipe (adapt `ssh_{client,server}_handshake` from `conn:i64` sockets to the `Process` pipe — a transport abstraction or a pipe-specific handshake, D3) + per-direction HKDF keys + counter-nonces (D4); `seal`/`open`'s `key64`/`seqnr` params are shaped so only the key/nonce SOURCE changes. Then M2.4c (generic `secret T` + variable-length `secret [u8]` + D5 padding; promote the unit `Type::SealedChannel` → interned `SealedChannel(SealId)`). Also pending: the scg mirror of seal/open + the bridge lowering (then a `tests/pass` differential fixture becomes possible); the scg mirror of M2.3b; the generic in-process channel builtins; the worker-pool library.

### ▶ (prior resume — SUPERSEDED by the M2.4a block above) ADR 0066 CONCURRENCY RESUMED (maintainer's call (b)): M1.2b `Channel<T>` reaches TYPE-ANNOTATION position → a channel-typed fn PARAM → the cross-thread producer/consumer WORKER, fully self-hosted. Added the `resolve_type_expr` "Channel" arm in BOTH compilers (Rust `sentinel-types`; selfhost `types/interner.sentinel` `type_of_typeexpr`) — `Channel<i64>` only at the minimum (`ChannelElementNotSupported` rejects a non-`i64` element, `tests/ui/c66_channel_element_unsupported`). New differential fixture `tests/pass/c66_channel_worker` (spawn `produce(ch)` with a Channel arg + main drains as consumer → exit 42). Fixed a latent spawn-lowering divergence (a copy-var spawn arg's `load` must precede the args-struct alloc) by aligning ALL THREE emitters (inkwell + `snc llvm` + selfhost cg) on collect-then-store; taught the selfhost borrow checker `Channel` is Copy. NO new runtime symbols / ABI change. **THEN M1.3 (worker pattern) EXAMPLES landed:** `examples/lang/worker_pool.sentinel` (a two-worker fan-out/fan-in pool — workers spawned with their channel endpoints, a work-stealing shared queue + a results fan-in channel; squares 1/4/5 → 42, built BOTH `--separate` and merged) + `tests/pass/c66_channel_pipeline` (a `relay(src,dst)` worker = the corpus's FIRST 2-arg spawn, pinning the multi-arg packed-args lowering byte-identical). Full self-host differential (9 stages) GREEN, both fixed points byte-identical, Windows four-check green. **THEN `?T` was made FULLY GENERAL over scalars (maintainer's call):** `NullableInner` gained `U8`/`U128`/`F64`/`Ptr` (so `?u8`/`?u128`/`?f64`/`?ptr` are representable — the enabler for generic channel ELEMENTS, since `recv<Channel<u8>> -> ?u8` needs `u8` to have a `NullableInner`); fixture `tests/pass/c66_nullable_u8`, self-host needed NO change (scg's nullable is type-handle-general), differential green. **THEN NESTED ARRAYS `[[T]]` (ADR 0068 ACCEPTED, maintainer chose "lift the array-depth rule"):** `[[u8]]` is now representable (depth-1 rule lifted via an `ArrayElem::Array(ArrayId)` interner + `TypedProgram.arrays`, keeping `Type: Copy`). Threaded `arrays` through resolve + the check pipeline; element promotion routes through `array_elem_type`/`_in` (table-less `to_type()` can't resolve `Array(id)`); drop is single-free (consistent with existing arrays — element heap leak is a pre-existing accepted limitation). Self-host needed NO change (handle-based array type). Fixture `tests/pass/c68_nested_array`, all 9 stages byte-identical, both fixed points hold, runs 42. **THEN M2.1 PROCESS SPAWN landed + self-hosted (ADR 0066 D7):** `process_spawn(path:[u8], args:[[u8]]) -> Process` + `process_wait(Process)->i64` over `std::process::Command`; `Type::Process` (plain handle → ptr, kind 14 in scg); 2 `sentinel_process_*` symbols (abi-v1 §3/§5; symbol set 28→30); the **FnId base shift 25→27** in both compilers (auto in Rust, 31 hardcoded sites in scg — the delicate lockstep part); the secret fence implicit via the public `[u8]`/`[[u8]]` ABI; built on nested arrays (ADR 0068) for `[[u8]]` argv. Fixture `tests/pass/c66_process` (all 9 stages byte-identical, both fixed points hold) + `sentinel_process_*` runtime unit tests (real `cmd`/`sh` spawn) + inkwell e2e (exit 42). **THEN the `Subprocess` CAPABILITY EFFECT landed (M2.1 completion, D7):** `process_spawn` carries the auto-registered built-in `Subprocess` effect (joins `Async`), so a spawning fn declares `! { Subprocess }` and it BUBBLES to `main` (Pass-3 main-check EXEMPTS `Subprocess`); it is a capability effect (not perform-based) so a `! { Subprocess }` fn is exempt from the Kont* ABI (`uses_kont_abi` excludes it in codegen + `snc llvm` + selfhost `eff_row_is_kont`); resolve_dump/types_dump emit `Subprocess` after `Async`; selfhost `effects.sentinel` walk_eff/dump_rows + effect_lookup_slice mirror it. Also refreshed the stale resolve/types/borrow/effects driver GOLDEN dumps (pre-existing — stuck at the pre-channels base #21). **THEN M2.2 BYTE-PIPE IPC landed + self-hosted (D7/D8):** `process_spawn` pipes the child stdin/stdout; `process_write(Process,[u8])->i64` (write + close stdin) / `process_read(Process)->[u8]` (read stdout to EOF, the read_file shape); 2 more `sentinel_process_*` symbols (symbol set 30→32); the **FnId base shift 27→29** (auto in Rust; ~30 selfhost sites — incl. a 3-fn golden test that needed a manual +2 the `#27/#28`-only sed missed); the cross-process **secret fence (D8) is STRUCTURAL** — the public `[u8]` pipe payload rejects a `[secret u8]` as a type mismatch (ui fixture `c66_process_secret_fence`, "argument expects u8, got <secret>"). `process_wait`/`_write`/`_read` are effect-free (operate on an already-acquired handle). `tests/pass/c66_process` extended to exercise write/read in `spawner` (differential covers the IR; runtime unit tests cover the real `cat`/`findstr` round-trip). All 9 differential stages byte-identical, both fixed points hold, Windows four-check green (`pass_c5d4_file_io`'s `/tmp` fail is pre-existing). **THEN M2.3 TYPED FRAMED CHANNEL OVER PIPES landed + self-hosted (D8/D10/D11):** `process_send(Process,i64)->i64` (frame an i64 to the child's stdin, 8-byte LE, keep stdin OPEN — multi-message) + `process_recv(Process)->?i64` (read one frame; null=closed/EOF) — the cross-process TWIN of the M1.2 in-process channel `send`/`recv`; the runtime symbols + the `?i64`-from-status codegen mirror the channel ABI EXACTLY, so the scg cg arm = the `channel_send`/`recv` arm (byte-identical for free, no new lowering risk). `process_wait` now CLOSES stdin before reaping (so a loop-until-EOF child terminates; runtime-internal, no IR change). 2 more `sentinel_process_*` symbols (set 32→**34**); **FnId base shift 29→31** (auto in Rust; ~30 selfhost sites via a sed of the FnId-ONLY `29 +`/`- 29`/`>= 29`/`< 29` patterns — NOT the token-kind `== 29`/`!= 29` COMMAS; + the FOUR driver golden dumps (resolve/types/borrow/effects.rs) need +2 per user FnId). The cross-process **secret fence (D8) stays STRUCTURAL** — the public `i64` element rejects a `secret i64` (ui fixture `c66_process_channel_secret_fence`). `process_send`/`_recv` are effect-free (operate on an already-acquired handle). Fixture `tests/pass/c66_process_channel` (guarded send/recv) byte-identical across all 9 stages, both fixed points hold; runtime round-trip unit test (LE-i64 through `cat`, **Unix-gated** — Windows `findstr` line-buffers binary, can't echo interactive frames). **GOTCHA: surfaced a PRE-EXISTING latent scg bug — NOW FIXED** (separate follow-up, see below): an EMPTY `[[u8]]` literal `[]` lowered its element size as `i64` (8B) instead of the annotation's `[u8]` (`{i64,ptr}`, 16B), diverging from snc (the differential's first empty-nested-array case; c68_nested_array uses non-empty, the fence ui fixture is differential-skipped). The M2.3 pass fixture uses a non-empty argv (the spawn is guarded anyway). ▶ NEXT (maintainer's call): **M2.3b** generic word-scalar channel ELEMENTS ({i64,i32,u8,bool,f64,ptr}, length-prefixed framing for variable-width) + a distinct `ProcessChannel<T>` type (the M2.4 sealed-vs-raw split needs it) → M2.4 `SealedChannel<secret T>` (own ADR, the AEAD escape D8a); the **generic IN-PROCESS channel BUILTINS** (UNBLOCKED by `?T`-general) + a reusable worker-pool LIBRARY; or alt tracks (a) ADR-0067 TAILS; (c) a deferred backlog item (§11.6 `return value;` terminator, §11.7 cross-module `pub class`, BACKLOG2 §10.9 module-system guide doc).)

> **▶ DONE (2026-06-30) — scg empty-nested-array element-type fix (ADR 0068 follow-up).** The pre-existing latent bug M2.3 surfaced: an EMPTY array literal `[]` can't infer its element type from the (absent) elements, so the self-hosted `scg` defaulted it to i64 — diverging from the Rust `snc` oracle for empty NESTED arrays (`let argv: [[u8]] = []` emitted `getelementptr i64, …` + dumped `:[i64]` instead of `{ i64, ptr }` + `:[[u8]]`). FIX (selfhost-only, snc already correct → NOT oracle-moving): `dump_array_elems` (selfhost/types/infer.sentinel) returns a `-1` sentinel for an empty literal, and the array arm (selfhost/types/borrow.sentinel) resolves the element from the expected/annotation type via `array_elem_of(exp)` (which is i64 when `exp` is absent/-1 or non-array — preserving the old default for empty FLAT arrays, whose element IS i64). The let-RHS already threads its declared type as `exp` (borrow_stmts.sentinel `dump_texpr(…, letexp, …)`). Fixes the type dump, the MIR `aty`, AND the codegen size-GEP in one place. Fixture `tests/pass/c68_nested_array_empty` (empty `[[u8]]` + non-empty + empty `[i64]`, exit 42 = 0+0+2+2+3+35) byte-identical across all 9 differential stages, both fixed points hold. (Remaining: the same `exp`-threading for an empty array as a call-arg / return value is untested — a follow-up if a program needs it.)

> **▶ DONE (2026-06-30) — ADR 0066 M2.3b: GENERIC word-scalar elements for the typed process channel (snc-side).** `process_send`/`process_recv` generalized from i64-only to any word-scalar element `{i64,i32,u8,bool,f64,ptr}` (`is_process_channel_elem` = `is_spawn_word_scalar` ∩ has-`NullableInner`). The element is ENCODED into the 8-byte i64 frame on send (the M1.1 spawn encode: zext narrow int / bitcast f64 / ptrtoint ptr) and DECODED back on recv (trunc / bitcast / inttoptr) — the RUNTIME STAYS i64-based, **no runtime/ABI/FnId/symbol change**. `process_send`'s element = the value-arg type (encode by LLVM value kind, no `type_args`); `process_recv -> ?T` takes `T` from the EXPECTED return type (context-typed, default `?i64`), carried in `type_args` ONLY for non-i64. Implemented as a `check_call` SPECIAL-CASE (right after `LEN_FN_ID`) in sentinel-types + the encode/decode in BOTH snc emitters (inkwell `lower_process_send`/`_recv` + `llvm_dump`). **The i64 case is BYTE-IDENTICAL to M2.3** (no encode/decode, no `type_args`), so the differential corpus's `c66_process_channel` is unchanged → **NO scg change, both fixed points stay byte-identical** (the snc-only pattern, like `u128`/`f64`/`task_generic`). Secret fence D8 holds (a secret element isn't a word-scalar → rejected; the `c66_process_channel_secret_fence` message is UNCHANGED — same `CallArgMismatch`). Demonstrator `examples/lang/process_channel_typed.sentinel` (u8/i32/bool send/recv, guarded spawn → 42, built BOTH `--separate` and merged). **scg MIRROR DEFERRED** (generalize the selfhost cg arms + a context-typed `process_recv` in scg — needs the selfhost to thread the expected type to the recv builtin, like the empty-array fix now does for `let`). ▶ NEXT: **M2.4a — implement `SealedChannel<secret T>`** per **[ADR 0069](decisions/0069-sealed-channel.md) (PROPOSED, design PINNED + maintainer-signed-off 2026-06-30; NO code yet).** Pinned: D3 reuse the ssh host-key auth model (authenticated x25519 over the pipe; v1 authenticates), D4 counter-nonces + per-direction HKDF keys, D5 fixed-width frames at the secret-i64 minimum (padding later), D9 add ONLY `SealedChannel<secret T>` (one new interner Type; the fence becomes a static type property; leave M2.3's raw path as the bare-`Process` builtins). M2.4a = `Type::SealedChannel` + the fence-as-type + seal/open over `secret i64` via the stdlib `aead` + frame the public ciphertext over the M2.2/M2.3 pipe; **the gnarly part is adapting `ssh_*_handshake` from sockets (`conn:i64`) to the `Process` pipe** (a transport abstraction or a pipe-specific handshake). snc-side first (like M2.3b). Then M2.4b (the real handshake/keys/nonces) → M2.4c (generic `secret T` + variable-length + D5 padding). Also pending: the scg mirror of M2.3b; the generic IN-PROCESS channel builtins; the worker-pool library. **(User chose to pick up M2.4a in a fresh focused session — the security-critical crypto composition deserves full context.)**

> **▶ DONE (2026-06-30) — ADR 0066 M1.2b: `Channel<T>` type annotation + channel-typed fn param +
> cross-thread worker (self-hosted).** Un-paused the concurrency track. M1.2 interned `Channel<i64>`
> only as `channel_new`'s result; M1.2b makes `Channel<i64>` writable in TYPE position so a function
> can take a channel endpoint as a parameter (ADR 0066 D4 — the worker pattern).
>
> - **Mechanism (both compilers):** a `"Channel"` arm in `resolve_type_expr` — Rust `sentinel-types`
>   (`resolve_type_expr_with_scope`, returns the `ChanId(0)` singleton `Channel<i64>`, "no threading")
>   + selfhost `types/interner.sentinel` (`type_of_typeexpr` `TGeneric`, returns `mk_channel(c, 0)`).
>   Non-`i64` element → the new `TypeError::ChannelElementNotSupported` (Rust-only; the differential
>   skips the rejected ui fixture). Channel was already Copy / `is_spawn_word_scalar` / `ptr`-lowered /
>   await-decoded from M1.2 — the annotation arm was the only missing front-end piece.
> - **The latent spawn-lowering bug this surfaced + fixed:** `c66_task_bool` spawns with a *constant*
>   arg (no eval instruction), so the order of "alloc args-struct" vs "evaluate args" never mattered.
>   A `Channel` endpoint arg is a copy-var whose eval emits a `load` — exposing that the selfhost cg
>   collects (evaluates) args BEFORE the alloc, while inkwell + `snc llvm` alloc'd FIRST. Aligned all
>   three on **collect-then-store** (eval every arg → alloc → store-all): behavior-preserving, and now
>   byte-identical for ANY arg count (not just constants). Also: selfhost `is_move_type` learned kind
>   13 (Channel) is Copy, matching Rust `is_copy_type`.
> - **Fixtures:** `tests/pass/c66_channel_worker` (differential; spawn-with-channel-arg + cross-thread
>   drain → 42) + `tests/ui/c66_channel_element_unsupported` (the non-`i64` rejection). Both new
>   `tests/pass` fixtures are exit-validated in `pass.rs` too (they were auto-discovered by the
>   differential but `pass.rs` registers fixtures EXPLICITLY — easy to forget).
> - **NOT oracle-moving for the ABI:** no new runtime symbols, no `abi-v1` change (the channel runtime
>   is unchanged from M1.2). Constant-time + the lexical borrow checker UNCHANGED.
> - **M1.3 (worker pattern) — EXAMPLES landed (same session):** `examples/lang/worker_pool.sentinel`
>   (a two-worker fan-out/fan-in pool: workers spawned with their `Channel<i64>` endpoints, a shared
>   work-stealing queue (mpsc receiver behind a mutex) + a results fan-in channel; squares 1/4/5 → 42,
>   built BOTH `--separate` and merged, registered in `examples.rs`) + `tests/pass/c66_channel_pipeline`
>   (a `relay(src, dst)` worker — the corpus's FIRST 2-argument spawn, pinning the multi-arg
>   packed-args lowering byte-identical after the collect-then-store alignment). NOT oracle-moving (no
>   compiler change — pure library/fixture). The *reusable* worker-pool LIBRARY is deferred (it needs
>   generic `Channel<T>` AND a first-class-function/closure mechanism for the worker body — Sentinel
>   has neither yet), so M1.3 ships as the demonstrator examples.
> - **NEXT (when resumed):** M1.2b cont. — generic word-scalar channel ELEMENTS — `channel_new<T>` /
>   `send<T>` / `recv<T> -> ?T` / `channel_close<T>`, which need `?T` for `u8`/`f64`/`ptr` (a
>   `NullableInner` extension) before `recv` can be generic; then the reusable pool library, then M2
>   (processes).

> **▶ DONE (2026-06-30) — SELF-HOST MODULARIZATION via MULTI-FILE MODULES (ADR 0067
> ACCEPTED-WITH-AMENDMENTS).** The maintainer's "maintainability is biting now" focus (BACKLOG §11.8)
> is complete. **Multi-file modules** (several files declare the same `module X;` → one logical module;
> internals module-private across the files; public API + `types::` namespace unchanged — the Rust `mod`
> / C++-namespace model) are implemented in BOTH `snc` and `scg`, and the 13,718-line
> `selfhost/types.sentinel` monolith is split into a **3,371-line root + 5 parts**
> (`types/{interner,infer,borrow,cg,mir}.sentinel`), the full self-host differential GREEN and both
> bootstrap fixed points byte-identical at every step.
>
> - **Model (all 4 D-points maintainer-confirmed 2026-06-29):** directory = module + a **`part` manifest**
>   (root `a/b.sentinel` lists `part name;` → `a/b/<name>.sentinel`, read BY PATH — no directory-listing
>   builtin, so no new runtime symbol / FnId-base shift); **module-wide private** (two levels, `pub`
>   unchanged = cross-module export); the build ENTRY is exempt from the decl-vs-location check.
> - **Mechanism (both compilers):** `snc` — `module`/`part` tokens + parse + directory/parts discovery
>   (`read_module_with_parts`, with the entry-exemption) + module-scoped union rename in `merge_modules`.
>   `scg` — `selfhost/parser.sentinel` parse/dump (tags 64/65) + `selfhost/merge.sentinel` skip-directives
>   + **`append_module_parts`** (concatenates a module's parts onto its root → per-module rename + part
>   discovery fall out for free; emission order matches the Rust union rename byte-for-byte).
> - **Mechanism commits:** `27070f827` (D9.1 Rust) · `363eaa5db` (D9.3a parse/dump/skip) · `7531c343e`
>   (D9.3b concat) · `fc5773a434`/`cc5b1420b5`/`798722f3ae`/`5b5ec67037` (D9.4 the four types split cuts).
> - **THEN the rest of the self-host was split (maintainer request, same day), full differential green
>   at every step:**
>   - **Round 2 (`dafdb943a6`/`f7b4b060f3`/`c090eb7247`/`4a92ed2`):** `parser` → root + parser/{parse,dump};
>     `resolve` → root + resolve/{dump,decls}; `merge` → root + merge/{emit,engine}; `types/cg` →
>     cg/cg_class/cg_effects; `types/borrow` → borrow/borrow_stmts. A scg merge bug was fixed: a
>     no-`use` multi-file entry must still QUALIFY (`merge_mode` triggers on a `part`, matching snc).
>   - **Round 3 (`ce6c93d`/`08cfd1f`/`19bed14`):** the `types` root's decl-emit → types/decl_{fn,class,synth};
>     **`dump_texpr` broken up** (a behavior-preserving refactor: its 7 biggest match-arm bodies →
>     `dump_te_*` helpers in `types/borrow_arms.sentinel`, verified byte-identical); `cg_effects` split
>     3-way (cg_effects/cg_chained/mir_emit); `run`'s ~210-line `TyCtx{…}` literal → `new_tyctx` in
>     `types/tyctx.sentinel`.
> - **Technique for single-fn giants:** Sentinel `match` has NO binding catch-all (`other =>` — only
>   `_`), so extract arm BODIES into per-arm helpers (unused params tolerated → uniform param set);
>   drove it with an awk pass (match `^        Expr::X(`, redirect body until `^        },` into a fn).
> - **ADR-0067 TAILS (NOT required — ADR 0067 *Revisit*):** `run`'s remaining body is a tightly-coupled
>   multi-pass pipeline (emit passes + assembly share ~12 live locals — `itembuf`/`garbage`(=gb)/`out`/
>   `result`/`rsrc`/`rbs`/`rbe`/`itempos`/`itemkind`/`ctx`), so its extraction needs careful param
>   threading — DEFERRED; `cg_emit_call` (~558-line `if fid==N` builtin dispatch) left as one fn. Plus
>   the `module X;` decl sweep + mandatory-enforcement flip; `--separate` over multi-file modules.
> - Constant-time `secret` + the lexical borrow checker UNCHANGED (a front-end/discovery/merge concern).
>
> **— Concurrency track (ADR 0066): M1.1 + M1.2 DONE & self-hosted; rest PAUSED (resumable next) —**

> **▶ ADR 0066 (threading + multi-processing) is ACCEPTED (roadmap; `f658d0778`).** Spine = channels
> + ownership-transfer (fits the lexical borrow checker; no `Arc`). Secret fence (D8) is a BOUNDARY
> property: NO fence in-process (the verified receiver still runs `secret_leak`), FENCE cross-process
> (generalizes the FFI fence); D8a adds a `SealedChannel<secret T>` AEAD escape over the verified-CT
> `aead`/`x25519` stdlib. Mutex deferred + gated (D5/D5a: runtime deadlock detection → typed error).
> Roadmap: M1.1 → M1.2 → M1.3 → M1.4 (Mutex); M2.1 process spawn → M2.2 IPC → M2.3/M2.4; M3 actors.
>
> **▶ M1.1 — generic `Task<T>` (DONE, self-hosted; `48c38c6e5`, `23c78300c`).** Lifted the ADR 0024
> `Task<i64>`-only restriction to any **word-sized scalar** (i64/i32/u8/bool/f64/ptr/Task/Channel) for
> spawn args + result: the per-spawn wrapper loads each arg with its real type and ENCODES the result
> into the Task's i64 slot (zext/bitcast/ptrtoint), `.await` DECODES it — runtime/ABI unchanged, the
> i64 case byte-identical. Both text emitters byte-identical; `tests/pass/c66_task_bool` in every corpus.
>
> **▶ M1.2 — channels (DONE, self-hosted; latest `8894f5f87`).** `Type::Channel(ChanId)` (a **Copy**
> handle, like Task) + the builtins `channel_new`/`send`/`recv`/`channel_close` at **FnId 21..=24** —
> which SHIFTED the user-fn base 21→25 in BOTH compilers (the `__spawn_wrapper_<id>` symbol embeds the
> FnId, so it had to be lockstep; watch the THRESHOLD comparisons `fid >= N` / `fid < N`, not just the
> `fid - N` index forms — a missed one crashed scg until M1.2 Phase B). 4 `sentinel_channel_*` runtime
> symbols over cross-platform `std::sync::mpsc` (abi-v1 §3/§5; `SentinelChannel` is opaque). `recv ->
> ?i64` (some/null; the runtime returns the i64 STATUS, codegen does `icmp eq 0` for the valid bit).
> Fully self-hosted: both text emitters byte-identical, full differential green, `tests/pass/c66_channel`
> in every corpus. Demos `examples/lang/{task_generic,channel}.sentinel`. **M1.2 minimum = `Channel<i64>`
> only** (concrete builtins; generic `Channel<T>` is M1.2b). **M1.2b — the `Channel<i64>` type
> annotation + channel-typed fn param + cross-thread worker — is now DONE (see the M1.2b block under
> ▶ RESUME HERE).** Concurrency next: M1.2b generic ELEMENTS (`Channel<bool>`/`<u8>`/…, gated on `?T`),
> then M1.3 (worker-pool library + examples), or M2 (processes). See the
> `threading-multiprocessing-planned` auto-memory.
>
> **▶ BACKLOG — deferred, ADR-needed (maintainer-confirmed 2026-06-29):** **BACKLOG.md §11.6** — make
> `return value;` parse as a fn terminator (a CONFIRMED gap: only `return 42` WITHOUT a semicolon, or a
> bare tail value, works today — `{ return 42; }` errors "blocks must end with an expression") + the
> larger deprecation of the implicit "hanging" tail return; **§11.7** — cross-module `pub class`
> (`pub class` is rejected → classes are module-local; the cross-module "type with behaviour" is a
> `pub struct` + `pub fn`s); **§11.8** — multi-file modules (THE NEXT FOCUS, above); **BACKLOG2 §10.9** —
> a module-system guide doc (the worked multi-file example is DONE: `examples/modules/rect_demo.sentinel`
> uses a `pub struct` + `pub fn`s from the library module `std::math::geometry`).
>
> **▶ ADR 0065 (early `return`) — PAUSED.** The effect-free path is complete + self-hosted (both fixed
> points byte-identical); `return` is in every differential corpus (`tests/pass/c65_return*`). What's
> left is snc-only faithfulness (the text-emitter mirror of `sentinel_kont_free` + the selfhost MIR
> collapse for a `return` crossing a `handle`) + the macOS confirmation; then ADR 0065 ACCEPTED. Full
> detail in the git history / `tests/ui/c65_handle_perform_in_control_flow`.

All on `main`, **NEVER pushed**. Verified on **Windows only** (see the macOS caveat);
the dev box is `x86_64-pc-windows-msvc` with a from-source LLVM 18.1.8 at `G:\llvm-18`
(`LLVM_SYS_180_PREFIX` set), no `just` — see the `build-environment-windows` auto-memory
for the build/test commands (incl. the `vcvars64.bat` recipe for the link-touching tests)
+ gotchas.

**Done 2026-06-28 — explicit early `return`, effect-free path (ADR 0065 stages 1–2, commit
`ddef38dc`):**

- **`return expr` exits a function early** as a **divergent expression** (`ExprKind::Return`,
  Rust-style), valid as a branch tail (`if guard { return x } else { … }`). Threaded through
  ast → parser → resolve → types → borrow → effect → mir → codegen + the driver dumps. The
  template for the ~30 mechanical sites was `Declassify(Box<Expr>)` (a single-inner passthrough);
  only the **divergent typing** and the **control-flow codegen** are real new logic.
- **Typing (the subtle part):** the inner checks against the enclosing fn's return type — stashed
  on the per-fn `VarTypeEnv` as `current_return_type` (like `loop_depth` for break/continue) — and
  `expr_diverges`/`block_diverges` (structural: `Return`, and `Block`/`If`/`Match` all of whose
  paths diverge) integrate at the **If join**, the **fn-body** check, and the **method-body**
  checks so a `return` branch unifies with the other branch and a fully-returning body skips the
  tail-vs-return-type match. Mismatch reuses the generic `Mismatch` ("expected X, found Y").
- **Codegen (the ADR 0036 break/continue shape, floor = the function):** `emit_return_drops`
  drains EVERY live scope frame, value-aware (skips the returned binding so its heap survives — the
  use-after-free guard); `build_fn_return` converts + `ret`s with the SAME ABI as the epilogue
  (`main` i64→i32, effecting → kont_pure) — the bug it fixes was an early `return 42` in `main`
  emitting a raw `ret i64` against the `i32` LLVM `main`; then a dead block is parked so the
  unreachable if-arm store/merge never appends to a terminated block. Single-free holds because the
  return-block and the merge-block are mutually exclusive.
- **Constant-time UNCHANGED** — `return` is unconditional control flow, no new `secret_leak` sink
  (a secret `if`-condition is still rejected at the `if`); returning a `secret` value is fine. MIR
  carries the inner Opaque (so a secret divisor inside `return a / b` is still flagged).
- **snc-side only; demonstrator `examples/lang/early_return.sentinel`** (heap binding live across
  the return; both back ends, exit 42) + `tests/ui/c65_return_type_mismatch`. **Kept in `examples/`
  (NOT `tests/pass`)** so `return` stays OUT of the scg differential corpus — both fixed points are
  byte-identical without the stage-4 mirror (the u128/f64 pattern). Windows four-check green
  (build · cargo test · doctests · clippy `-D warnings`); analysis unit tests + the selfhost
  **dump** differential (types/resolve/mir/borrow/effects) unchanged → no regression.
- **▶ CONFIRMED the bootstrap is NOT broken:** the `selfhost_codegen` (scg-run) test fails on
  Windows, but it fails **identically on clean HEAD** (git-stash-verified) — the scg self-hosted
  binary has never run on Windows (the macOS-only-validated path, already backlogged). Not my
  change.
- **Pending — stage 3 (`return` crossing a `handle`, ADR 0065 D6):** the `sentinel_kont_free` +
  handle-region teardown on the effect runtime (the youngest subsystem — risk-noted). **Stage 4
  (selfhost mirror + both fixed points)** is fundamentally a **macOS** task (scg doesn't run on
  Windows). **Known v1 stubs:** `match`-arm divergence (a `match` arm that `return`s over-rejects
  the arm-join — restructure for now) and the `snc llvm` text-IR dump for `return`.

**Done 2026-06-28 — the `/sentinel_library` reorg + the `Sentinel::` core base (ADR 0064
ACCEPTED, was NEXT item 0):**

- **The libraries now live under a top-level `sentinel_library/` tree** so the repo can host
  MANY first-party libs. A new **`Sentinel::` core base** (`sentinel_library/Sentinel/`) holds
  the constant-time `secret` vocabulary: **`Sentinel::ct`** (moved from `std::security::ct`) +
  **`Sentinel::secrets`** (the `widen`/`reveal` boundary pair, DRY'd from the three byte-identical
  copies in `examples/export/crypto_lib` + `tools/trust/{sign,keygen}_core`). `std::` is the
  batteries, relocated to `sentinel_library/std/` (namespace unchanged) + `use`-ing `Sentinel::ct`
  in its crypto modules.
- **Done in two four-check-green phases** (commits `884d57a8` relocate, `1b520f78` carve-out;
  ADR `544acf39`): **(1)** `git mv std → sentinel_library/std` + repoint the harnesses
  (`examples.rs`/`export.rs` copy `sentinel_library/` next to each entry) + the `--lib-path`
  refs (`sign_core.rs`, `trust_tools.rs` hint, `demos/win32` docs); **(2)** carve out `Sentinel::`
  — move `ct`, add `secrets`, switch the 12 `use std::security::ct::X` → `use Sentinel::ct::X`
  and the 3 boundary sites → `use Sentinel::secrets::{widen,reveal}`.
- **NAMING:** the owner proposed `Sentinel::secret`, but `secret` is a keyword (a `use`-path
  segment must lex as `Ident`), so the module is **`Sentinel::secrets`** (plural — owner-confirmed).
- **NOT oracle-moving** (filesystem move + module-path edits + harness search-root change; no
  lex/parse/IR change; no `selfhost/` moves) → no re-bless / mirror; both fixed points byte-identical.
- **Windows-verified:** clippy `--all-targets` green; the ct-direct examples
  (`secure_compare`/`ct_memcmp`/`chacha_qr`/`siphash_round`) green BOTH `--separate` + merge (so
  `Sentinel::ct` crosses separate compilation); the 6 ct-consuming std-crypto examples
  (`sha256`/`siphash24`/`poly1305`/`sha512`/`sha3`/`chacha20_stream`) build+run 42 (merge);
  `sign_core` byte-identical-to-the-Rust-twin STILL holds (DRY'd `widen`/`reveal` unchanged) +
  `keygen` flow green; `crypto_lib` multi-module `--lib` archives clean. macOS differential +
  the crypto `--separate`/`export.rs` cc paths remain on the standing macOS BACKLOG (not
  oracle-moving → expected byte-identical).

**Done 2026-06-28 — the module-search path (HANDOVER §0 NEXT item 1, ADR 0037 point 12):**

- **`--lib-path <dir>` (repeatable) + `SNC_LIB_PATH`** now add fallback module-search
  dirs, tried AFTER the entry file's own directory (first hit wins, so a local module
  still shadows a library one). `discover_module_graph` takes an explicit `search_dirs`
  slice; a `lib_search_dirs(cli)` helper unions the CLI flags (priority) with the env
  (split on the platform separator) at each call boundary. The flag is on all three
  build modes (`build`, `build --lib`/`--shared`, `build --separate` — the last routed
  through a new `run_build_separate_cli` arg-loop); **every** subcommand honors
  `SNC_LIB_PATH` (incl. `llvm`/`merge`). `ModuleNotFound` now lists every dir tried.
  **PROVEN:** `demos/win32/messagebox_compact.sentinel` builds **in place** via
  `snc build … --lib-path .` (no more copy-to-root) — resolves the real
  `std::sys::win32`→`std::c` FFI graph through the search path, CT-checks, self-links
  `user32`, links with MSVC → a working `.exe`. NOT oracle-moving (changes only *where*
  a module's source is read, not the lowered program; no `selfhost` fixture uses a
  search path) → no re-bless, no `selfhost` mirror. 6 new `tests/modules.rs` tests
  (resolve out-of-tree via flag + env; entry-dir shadows; flag outranks env; tried-dirs
  in the error; `--separate` too). Four-check on the driver crate green on Windows
  (build · `cargo test` · doctests [vacuous — binary crate] · clippy `-D warnings`);
  pre-existing Windows test failures unchanged (see below).
- **Code signing & supply-chain trust — ADR 0061 v1 COMPLETE** (see NEXT item 1 below for
  the full breakdown): a new **`sentinel-trust`** crate (Ed25519 verify twin of
  `std::security::ed25519`) + the format + `snc sign`/`keygen`/`verify` + the build-time
  gate (`--require-signatures`) + capability bounding. 6 commits `6be5e4e`…`a032888`.
- **Conditional compilation — ADR 0062 (file-level), PROPOSED→IMPLEMENTED** (`48a291c`).
  A module `a::b` resolves per the active target to `b_<os>.sentinel` → `b_unix.sentinel`
  (linux/macos) → `b.sentinel` (most-specific first); the suffix is a selector stripped to
  module `a::b`, so importers always write `use a::b::item`. Target = host OS by default
  (codegen is host-only, ADR 0060); **`snc build --target windows|linux|macos`** overrides
  it (win32/darwin aliased). NOT oracle-moving (only `discover_module_graph`'s candidate
  list grows — base-major over the ADR 0037 search path, suffix-minor within a base; no
  selfhost file uses a suffix → byte-identical resolution; no lex/parse/IR change). Item-
  level `#[cfg]` (in-file, oracle-moving) is the deferred follow-up; `tests/conditional.rs`.
- **`std::sys::random` — DONE (`c230ff3`), the ADR 0062 first consumer; the Windows
  `keygen` gap is CLOSED.** `random_unix.sentinel` (getentropy) + `random_windows.sentinel`
  (RtlGenRandom via advapi32's `SystemFunction036`, self-linking advapi32 — ADR 0057 A9);
  both export `random_bytes(n) -> [u8]`. `keygen_core` now `use`s `std::sys::random`, so it
  builds + runs on **all three** targets. **PROVEN on Windows:** `keygen_core` links (was
  blocked on `getentropy`), two draws give different seeds (real entropy), and the full
  author flow `snc keygen` (generated key) → `sign` → `verify` works end to end. So ADR
  0061 v1's `snc keygen` is now cross-platform.
- **Pre-built library consumption — [ADR 0063](decisions/0063-prebuilt-library-consumption.md)
  PROPOSED + phase 1a (`5ef9938`/`8d8f99c`).** The other half of the pre-built-libraries
  rock: consume a *compiled* lib via a signed **`.sif` interface descriptor** (the serialized
  ADR 0037 exports table). Format **(a)** (owner-chosen): a structured header (dedicated
  reader) + an interface body of Sentinel signature decls reconstructed via `parse` +
  `extract_exports` (no new syntax → not oracle-moving). **Phase 1a DONE:**
  `crates/sentinel-driver/src/descriptor.rs` (emit + read non-generic `pub fn` signatures,
  round-trip tested). **▶ Found + recorded:** the `.sif` pairs with an object exporting `pub
  fn` **abi-v1** symbols, but `snc build --lib` is the **C-ABI** path — so the producer needs
  settling (recommend: extend `--separate`, which already emits those symbols). REMAINING:
  producer wiring (`--emit-interface`), `.sif`-backed resolution + link, the ADR 0061 gate
  over the `.sif`, struct/enum/generic slices.
- **A proper delegation example (`5ef9938`).** `examples/lang/delegation.sentinel` (a NEW
  `examples/lang/` category) shows MULTIPLE LEVELS (3-deep transitive delegation) + the
  MULTIPLE-INHERITANCE problem resolved by NAMED impls + qualified calls (no diamond/MRO);
  `tests/ui/c43_delegate_same_method_ambiguous` pins the ambiguous-call rejection. Built via
  both back ends → 42.
- **macOS still BACKLOGGED** — the workspace `just check-all` + self-host differential
  has not run (argued not oracle-moving, but confirm on the differential). Also note:
  on Windows, `tests/modules.rs`'s 3 `separate_*_linkonce_*` tests fail at MSVC
  `link.exe` and `separate_same_named_*` needs `nm` (absent) — **pre-existing**
  (identical on clean HEAD, the `--separate` generic-dedup path was only ever run on
  macOS), not from this change.

**Done 2026-06-27:**

- **Windows host support (ADR 0060, PROPOSED→implemented).** **P1**:
  `sentinel-runtime` compiles on Windows (cfg the one POSIX `OsStrExt` path).
  **P2**: a `HostToolchain` link backend replaces the macOS-hardcoded
  `cc`/`libtool`/`.a` with host dispatch — Windows `link.exe` + `lib.exe` + the
  runtime's native libs (`ws2_32`/`userenv`/`dbghelp`/… + `/defaultlib:msvcrt`);
  Linux `cc`/`ar` wired (unvalidated). `snc build` / `--lib` produce working
  `.exe`/`.lib` on Windows. The whole workspace builds + the 929 analysis-pipeline
  unit tests pass on Windows.
- **FFI library ergonomics (ADR 0057 A8/A9).** `snc build --link <lib>` threads an
  extra native lib into the link; **`extern "C" link("user32") { … }`** self-links,
  so a binding module declares its own native libs and consumers need no `--link`.
- **std reorg + first binding library.** `std::c` (portable C-string helpers
  `cstr`/`cstr_len`/`cstr_read`); **`std::sys::win32`** (user32: `message_box` /
  `screen_width|height` / `beep`, self-linking `user32`); `demos/win32/` (standalone
  `messagebox` + library-using `messagebox_compact`). PROVEN end-to-end — a Sentinel
  program pops a real Win32 message box and reads the live screen size (2560×1440).
- **Docs.** REVIEW_ACTION_PLAN P3.3/P3.4 + the P3.6 diagnostics audit
  (`docs/diagnostics-audit.md`); `docs/project-context.md` added + imported by a root
  `CLAUDE.md`.
- **macOS NOT verified — BACKLOGGED.** Everything above is Windows-validated only; the
  macOS `just check-all` (workspace nextest · clippy --all-targets · doctests) +
  self-host parity has not run. Argued **not oracle-moving** (no emitted-IR or
  `ast`/`lex`-dump change; no `selfhost` fixture uses `extern`), so no `selfhost`
  mirror — but the differential should confirm it.

**▶ NEXT (the owner's stated direction — ADR-first):**

0. ✅ **DONE (2026-06-28, ADR 0064 ACCEPTED) — the `/sentinel_library` reorg + the `Sentinel::`
   core base.** See the "Done 2026-06-28 — the `/sentinel_library` reorg" block above for the
   full record. Libraries now live under `sentinel_library/` (`std/` batteries + the `Sentinel/`
   core base = `Sentinel::ct` + `Sentinel::secrets`); landed in two four-check-green phases,
   Windows-verified, not oracle-moving. (Module name is `Sentinel::secrets`, plural — `secret`
   is a keyword.)

1. **Pre-built libraries with a trust model (trusted / untrusted).** Consume a
   *compiled* library (`snc build --lib` already emits a `.a`/`.lib` + a C header)
   instead of re-`use`-ing source, with an explicit TRUST designation tied to
   Sentinel's supply-chain thesis (SENTINEL_DESIGN2 §2 signatures · BACKLOG2 §2 ·
   AI_TOOLING §7.1: "a dependency not in the trust manifest fails to compile").
   - **The signing & trust model is [ADR 0061](decisions/0061-code-signing-and-trust.md) —
     v1 COMPLETE (ACCEPTED-WITH-AMENDMENTS, 2026-06-28, Windows-verified, four-check green
     per crate).** A new **`sentinel-trust`** crate + `snc` subcommands; commits
     `6be5e4e`/`a2158d1`/`49f49b9`/`b5024e9`/`d25da3c`/`a032888`:
     - **Verify (D4)** — in-process **Ed25519 + SHA-512** in Rust, the TweetNaCl **twin** of
       `std::security::ed25519`, KAT-validated on RFC 8032 (verify lives INSIDE snc's trust
       boundary). **Byte-identical** to the Sentinel signer (committed guard).
     - **Format (D2/D3)** — `canonical_payload = domain‖algo‖pubkey‖grants‖SHA512(body)`
       (Rust-only; the Sentinel signer signs OPAQUE bytes — no second format impl) + the
       detached `.sig` / in-file `//` carrier + `verify_signed`. Tests prove **a changed
       comment byte fails** (D2) and **tampered grants fail** (D3).
     - **Sign (dogfood)** — `tools/trust/{sign,keygen}_core.sentinel` (the constant-time
       `std::security::ed25519`) + **`snc sign` / `snc keygen` / `snc verify`** (Rust
       orchestration shelling out to the cores).
     - **Gate (D7)** — **`snc build --require-signatures off|warn|strict`** + `--trust
       <manifest>` (the consumer `sentinel-trust.toml`, D5); runs over the module graph after
       discovery, before compile/link. off = no behavior change.
     - **Capability bounding (D6)** — a trusted module's used capabilities ⊆ its key's grants;
       v1 enforces **`ffi`** (an `extern "C"` block) — a trusted-but-over-reaching key is
       refused. Tested: signed + trusted + `extern` granted only `alloc` → strict REFUSES.
     - **Amendments** (ADR 0061 A1–A5): detached carrier is canonical (in-file-block gate
       deferred); verify is the Rust twin (the ADR 0059 link-in to make verify *literally*
       Sentinel is the north star); rigid byte payload (no TOML-canon TCB); `ffi`-first
       capability taxonomy; `keygen` entropy is POSIX (Windows RNG a follow-up; `snc sign`
       runs everywhere).
     - **v1 DEFERRED** (next, all in ADR 0061 *Phasing*): the in-file-block gate +
       `--separate`/`--lib` gating; TOFU/issuer policies; the keystore + hardware keys;
       rotation; revocation; build-env attestation; multi-party signing.
   - **The other half of the rock — now DESIGNED: [ADR 0063](decisions/0063-prebuilt-library-consumption.md)
     (PROPOSED).** Consume a *compiled* Sentinel library via a **signed interface descriptor**
     (`foo.sif`): the serialized ADR 0037 exports table (pub fn signatures + effect rows,
     struct/enum layouts — the table is AST-based, not interned types, so it serializes
     cheaply). The consumer resolves `use foo::item` against the `.sif` (the existing
     extern-fn-in-FnId-space import model, ADR 0037 D5.1) + links the object; the ADR 0061
     gate verifies the `.sif` + capability-bounds it. **The one real fork = the descriptor
     format:** (a) a structured file read by a dedicated reader — **NOT oracle-moving,
     recommended**; vs (b) a Sentinel-source `pub extern fn` form — oracle-moving. Phasing:
     (1) descriptor emission, (2) descriptor-backed resolution + link, (3) the trust gate
     over the `.sif`. v1 scope mirrors `--separate` (non-generic fns + structs/enums). **▶
     READY TO IMPLEMENT** pending the format fork.

---

**▶ (Superseded) prior pointer — the examples-as-tests + core-libraries track (COMPLETE):
HEAD was `048bfac`, 1636 tests, four-check green.**

**▶ ONE-LINE STATUS: the owner's WHOLE big-list is implemented** — the comprehensive
constant-time crypto suite, the networked `sshd`, the data & text trio, and **all three
remaining language-gap rocks (ADR 0058 `f64` floats · ADR 0057 `extern "C"` FFI import · ADR
0059 C-ABI export)** + the float follow-ups (libm transcendentals · `f64`⇄string · JSON
float numbers) + the export `&[u8]` INPUT and owned-`[u8]` RETURN buffer ABI + MULTI-MODULE
`--lib` + `--shared` `.dylib` (the verified-constant-time `std/security` suite — `sha256` +
`hmac` — callable from C as a static OR dlopen/ctypes-loadable drop-in library) + the
FFI-IMPORT buffer ABI (ADR 0057 A6/A7 — the `ptr` type + `ptr_of`/`ptr_of_mut` + `is_null`:
OS randomness via `getentropy`, env vars via `getenv`).
**What remains are only deferred Phase 1b/2
tails — none a new rock; the menu is in the *Next* bullet below.** Read STATE.md §"Current State"
for the authoritative live picture. Full detail follows.

SHIPPED to
date, in four bands (per-increment record in STATE.md + the commit log + the
[[sentinel_examples_and_corelibs]] memory): (1) a comprehensive **constant-time crypto
suite** — `std/security`, ~30 modules, all on canonical vectors (detail below); (2) **NINE
language gaps closed ADR-first** (ADRs 0047–0055, incl. the `u128` type); (3) a **complete
networked `sshd` over real sockets** — the full SSH-2 sequence (transport KEX → host-key
auth → §7.2 KDF → encrypted channel → publickey userauth → windowed channel →
`CHANNEL_REQUEST "exec"`) in `std/net/{tcp,ssh,ssh_cipher,ssh_handshake,ssh_userauth,
ssh_connection}` + 7 `examples/net/*`, all over real loopback TCP via the ADR 0056 socket
builtins; and (4) the **data & text trio** (below). A real **use-after-free** was found +
fixed along the way (ADR 0034 D8 — push now consumes its Move-typed element, both backends
byte-identical). **▶ AND THE FIRST OF THE THREE REMAINING LANGUAGE-GAP ROCKS IS NOW
IMPLEMENTED: the IEEE-754 `f64` float type (ADR 0058 → ACCEPTED-WITH-AMENDMENTS A1–A7),
built `snc`-side in four staged commits** (lexer float token → `Type::F64` +
arithmetic/casts → float literals [IEEE bits] → the `sqrt` intrinsic) **+ a demonstrator**
(`std/math/float` + `examples/math/quadratic`: the quadratic formula + 2-D geometry).
PUBLIC-ONLY — `secret f64` is a type error (float ops aren't constant-time), so floats are a
disjoint public domain and the constant-time guarantee is unweakened. Like `u128` (ADR 0055)
it is `snc`-side only with NO `FnId` shift (a `Type` variant; `sqrt` is a `UnaryOp`, not a
builtin) — the demonstrator lives in `examples/` (NOT `tests/pass`), so the `scg` mirror is
deferred and every selfhost differential + both bootstrap fixed points stay byte-identical.
**AND THE SECOND ROCK IS NOW IMPLEMENTED: the `extern "C"` FFI import (ADR 0057 →
ACCEPTED-WITH-AMENDMENTS A1–A5), Phase 1 VALUE ABI** (public `i64`/`f64`) — a
user-declarable `extern "C" { fn …; }` block resolved by SYMBOL NAME (a user-range `FnId`
like a cross-module import, so no builtin `FnId` shift) + declared `External` under the bare
C symbol so `cc` links it against libc; the FFI fence rejects a `secret` reaching an
`extern` arg. OS / native bindings are now *library* work: `std/sys/posix` (process
identity) + `std/math/float`'s **libm transcendentals** are the wrappers
(`examples/sys/process_ids`, `examples/math/transcendental`). Deferred to a Phase 1b: the
`ptr` opaque type + `ptr_of`/`cstr` + pointer/buffer libc calls (`getenv`/`getentropy`),
`i32` widths, structs, extra `-l`, Win32. The **float follow-ups** also landed:
`f64`⇄string in `std/text/str` (`parse_f64`/`f64_to_str`) and `std/data/json` now
parses/serializes non-integer numbers as `Float(f64)`. **AND THE LAST ROCK IS NOW
IMPLEMENTED: the C-ABI export (ADR 0059 → ACCEPTED-WITH-AMENDMENTS A1–A5), Phase 1a VALUE
ABI** — an `export "C" fn` (un-mangled C symbol, secret-fenced) + a `snc build --lib`
static-archive mode (no `main`) + `--emit-header`. The HEADLINE is proven: a C driver
(`examples/export/driver.c`) links the snc-built `.a` + header and calls a Sentinel
`export "C"` constant-time select that widens public ints to `secret`, runs the
machine-checked branch-free select, and `declassify`s — a foreign caller getting a verified
constant-time primitive over a plain C ABI (`tests/export.rs` asserts exit 42). **SO ALL
THREE BIG-LIST ROCKS — 0057 (FFI import) · 0058 (floats) · 0059 (C-ABI export) — are
implemented**, and the export side now reaches real byte-buffer crypto (Phase 1b): a
`&[u8]` export param is presented to C as a `(const uint8_t*, int64_t)` pair (a generated
wrapper rebuilds the Sentinel slice), so a verified constant-time byte comparison
(`ct_byte_eq`, the MAC/tag-verification primitive) is callable from C over real buffers.
What remains is the deferred Phase 1b/2 tails (see **Next**). **The
active sub-track is now DATA & TEXT LIBRARIES** (owner-chosen over float-math / FFI-bindings):
the **`std::text::str` string library** shipped (`c6c0503` — case/trim/substring/concat/
compare/`parse_int`/`int_to_str`/index-based split/pad over `[u8]`; `examples/text/str_demo`,
~45 assertions). Maps were attempted first but PROBING found a real **use-after-free**:
pushing a Move-typed struct (one owning a `[u8]`) into a `Vec` through a by-value parameter
double-freed the element (the ADR 0034 D8 deferred case) — now FIXED in both backends
(`e66f57f`, fixture c60), which unblocked the **`std::collections::map` string-keyed hash
map** (`3c08f2d` — `[u8]`→`i64`, parallel-array storage + FNV-1a + resize) and **`std::data::json`**
(`9b2777b` — parse/serialize over a cons-list recursive enum); so the tractable data&text
trio (strings + map + JSON) is done — the remaining big-list items are the float-type and
FFI/bindings language gaps, which now have design ADRs 0057/0058/0059;
HEAD `2431aee`, 1597 tests,
four-check green).** Real,
idiomatic Sentinel programs that double as feature tests + the first **core
libraries**. **Dogfoods modules + `--separate`**, **stress-tests the constant-time
guarantee on real code**, and surfaces concrete language gaps — finding + fixing those
is the most valuable output. The crypto suite is now a full stack, all constant-time +
on canonical vectors: **hashes** SHA-256 / SHA-512 / SHA-3 (SHA3-256/512); **XOFs**
SHAKE128/256; **MACs** HMAC-SHA256 + KMAC128/256; **SP 800-185 derived functions**
cSHAKE128/256 + TupleHash128/256 + ParallelHash128/256; **AEAD** ChaCha20-Poly1305 +
AES-128-GCM; **block cipher** AES-128 (table-free field-inversion S-box); **key
exchange** X25519; **signatures** Ed25519 (SIGN + VERIFY, the latter with point
decompression via a field sqrt + the `S < L` malleability check); and **KDF**
HKDF-SHA256 (RFC 5869, extract-then-expand composing HMAC); and **X448** ECDH + **Ed448**
signatures (RFC 7748 / RFC 8032) over the new shared `fe448` field (GF(2^448-2^224-1),
28 radix-2^16 limbs — radix 2^28 would overflow i64). The shared `fe25519`/`fe448`
fields underpin X25519/Ed25519 and X448/Ed448. The ninth language gap — **the numeric
gap**, ADR 0055's `u128` type (the 128-bit / radix-2^51 path) — is now closed too,
demonstrated by `fe25519_64` + `x25519_64` (X25519 at radix 2^51, the field multiply in
`secret u128`, cross-checked against the radix-2^16 X25519). All four owner-approved
options for that run (HKDF, SP 800-185, X448/Ed448, the numeric gap) are SHIPPED.

**Flagship direction — a secure network service.** The owner asked whether an `sshd`
or a webserver is the better showcase; `sshd` was chosen (it is crypto end-to-end, so
the constant-time/`secret` guarantee covers ~all of it, and the shipped suite already
IS an SSH cipher suite). Both are blocked on the same missing piece — **sockets** — so
the service is being built **loopback-first**. Two pieces landed: **ADR 0056** (a TCP
sockets runtime surface — seven file-I/O-style builtins, blocking + one-OS-thread-task-
per-connection mapping onto `scope`/`spawn`, error-returning, the socket as a public
declassify boundary; now IMPLEMENTED end to end) and **`std::net::ssh`** — the SSH-2
transport key exchange (`curve25519-sha256` + `ssh-ed25519`, RFC 4253 / RFC 8731): the
SSH wire codec, the exchange hash, the host-key signature, and the §7.2 KDF, all in the
secret domain, run loopback (`examples/net/ssh_handshake`, validated against a Python
model cross-checked vs paramiko + pyca). FINDING: SSH's `mpint` of the secret shared
secret has a value-dependent length (a ~1-bit side channel); Sentinel's type system
forces it into an explicit `declassify` (the audit point) — a leak other SSH stacks
make silently. The **record cipher** has now landed too — `std::net::ssh_cipher`, the
`chacha20-poly1305@openssh.com` binary-packet AEAD (seal / open / tag-verify) that
protects every packet after NEWKEYS, composing the shipped ChaCha20 + Poly1305
(`examples/net/ssh_channel`: a sealed packet matches a pyca-anchored model, the tag
verifies, the payload round-trips, a tampered record is rejected). The three pieces are
stitched into ONE end-to-end loopback session in `examples/net/ssh_session` (KEX →
host-key auth → key derivation → an encrypted application packet; `ssh_kdf` gained a
second SHA-256 block for the 64-byte key). So the loopback SSH transport is complete
end to end. Toward running it over a REAL connection, ADR 0056's sockets are now LIVE
end to end: the runtime layer (`sentinel-runtime`: the seven libc TCP primitives, with a
Rust loopback-echo test, `b90a889`), the compiler builtins (`tcp_listen`/`local_port`/
`accept`/`connect`/`read`/`write`/`close` at FnId 14..=20, mirroring the file-I/O
builtins — resolve/types/codegen + the `scg` builtin-table bump, `0845b8a`), and a real
Sentinel program over them — `examples/net/tcp_echo` (`669665a`): bind 127.0.0.1:0,
`spawn` a concurrent server task, connect, send, read the echo back, exit 42. The
FnId-shift crux is closed (builtins 0..=20 ⇒ user fns start at #21; byte-exact, both
bootstrap fixed points hold). AND the SSH transport now runs OVER a real socket —
`examples/net/ssh_over_tcp` (`aac69a8`): the client and server are separate concurrent
tasks doing a DISTRIBUTED curve25519-sha256 KEX (each derives the shared secret K
independently), host-key auth, the §7.2 KDF, and an encrypted record across the wire; the
wire-public values are `declassify`d to send and widened back to `[secret u8]` on receipt
(K never crosses the socket), so the loopback `sshd` transport is now real-socket end to
end. The cleanly-open next
steps are in **Next**
below,
with array-repeat `[x; N]` and the `scg` widen-mirror deferred as low value.

- **Decisions locked with the owner:** top-level `std/` + `examples/`, each
  subdivided by **functional category** (security, math, …), examples mirroring
  std. The harness (`crates/sentinel-driver/tests/examples.rs`) assembles a temp
  project (copies the `std/` tree next to the flattened entry — module discovery
  roots at the entry's dir), builds each example BOTH `--separate` and merged, and
  asserts both back ends agree with the expected exit (a free differential). The
  flagship sequencing targets `[secret T]` arrays; the lib set is `ct` / `math` /
  `bits` (post-shifts) / `bytes`, built incrementally.
- **Done (commits on `main`):** `25e95a8` foundation (scalar `ct` lib +
  `secure_compare` + harness + corpus READMEs), `d1dace8` `math::num`, `a4f4b45`
  docs, and **four fully self-hosted language features** (each ADR-first, byte-
  identical across `snc` + `scg`, both bootstrap fixed points held):
  - **`1a84081` — `[secret T]` arrays (ADR 0047 A1–A6) + `ct_memcmp` over
    `[secret u8]`.** Arrays of secret ELEMENTS (`ArrayElem::Secret(SecretId)`) — a
    front-end-only change (one crate); the selfhost corpus differential proves `scg`
    == `snc llvm` on `tests/pass/c53_secret_array_memcmp`.
  - **`c3e3d7c` (Phase 1, snc) + `47abfd5` (Phase 2, selfhost mirror) — shift
    operators `<<` / `>>` (ADR 0048 A1–A5).** Logical right shift; a shift by a
    *secret* amount is rejected (a secret value by a public amount is
    constant-time). Reconstructed in the parser from two span-adjacent `<`/`>`
    (no lexer token, so nested generics still close). Shipped `std/bits` (rotates)
    + `std/security/ct::ct_rotl64` + a **SipHash-style ARX round over secret words**
    (`examples/security/siphash_round`) — the recognizable branch-free primitive.
    Validated by `tests/pass/c53_shift` across all 8 selfhost differentials.
  - **`99eadb5` (Phase 1, snc) + `0b69a7b` (Phase 2, selfhost mirror) — integer
    cast `x as T` (ADR 0049 A1–A4).** Closes the i32-construction gap (an int
    literal is `i64`); trunc/sext/zext, preserves secrecy, no CT sink (a new
    `ExprKind::Cast`, the `as` token reused; chosen over conversion builtins, which
    would shift FnIds in dumps). Shipped `std/security/ct::ct_rotl32` + a true
    32-bit **ChaCha quarter-round over `secret i32` words**
    (`examples/security/chacha_qr`) reproducing the RFC 8439 §2.1.1 vector.
    Validated by `tests/pass/c54_cast`. The `u8` conversion builtins remain.
  - **`977d6b5` (Phase 1, snc) + `873775f` (Phase 2, selfhost mirror) — mutable
    index assignment `a[i] = v` (ADR 0050 A1–A5).** Lifts the ADR 0017 D12
    deferral so an array / `Vec` element is written through a public index (the
    read path's bounds-checked element GEP + a store; the GEP factored — inkwell
    `lower_index_elem_ptr`, scg `cg_emit_index_addr` — and shared by read+write).
    Constant-time with NO new sink: a secret LHS index is rejected by the existing
    `IndexNotInt` rule (same as reads); a secret value stored is fine; MIR
    unchanged (`Opaque(value)`, like a field/deref store). MVP = Copy elements
    (scalars + `secret` scalars); a Move element → `IndexAssignNonCopyElem`.
    Parser unchanged (already parsed `Assign{Index,..}`). Shipped a full **ChaCha20
    block** over a `[secret i32]` state permuted in place
    (`examples/security/chacha20_block`, RFC 8439 §2.3.2). Validated by
    `tests/pass/c55_index_assign` across all 8 selfhost differentials.
- **Crypto band on the shipped 0047–0050 surface (no language change):**
  `eabafdb` **SipHash-2-4 keyed MAC** (`std/security/siphash`, canonical
  `0xa129ca…` vector + tamper detection), `26a58e6` **ChaCha20 stream cipher**
  (`std/security/chacha20::chacha20_xor`, RFC 8439 §2.4.2; the block refactored
  into the shared lib), `48b1248` **Poly1305 one-time MAC** (`std/security/
  poly1305`, radix-2^26 `secret i64` limbs, constant-time freeze, RFC 8439 §2.5.2
  — verified across 10 message lengths). These MACs each hit the "bind every
  public constant secret first" friction → motivated ADR 0051.
- **`a05659e` (Phase 1, snc) + `c68e670` (Phase 2, selfhost mirror) — implicit
  public→secret widening (ADR 0051 A1–A4).** A public `T` widens to `secret T` in
  operand position (`secret_x + 5`), as a call arg, a return, and `[u8] → [secret
  u8]` — via the existing `WidenToSecret` node (a codegen no-op). Monotone +
  sound: the CT sinks (shift amount / divisor / branch / index) are untouched and
  still reject (Div + shifts are EXCLUDED from the widen). The **operand widen**
  (binop+cmp) is fully self-hosted (`tests/pass/c56_operand_widen`); the call-arg/
  array/return widens are snc-side (no selfhost source or corpus fixture uses
  them, so the differentials + fixed points stay byte-identical without them).
- **`8b09774` — `std/algorithms/seq`** (in-place insertion `sort` via index-assign
  + `binary_search` over public `[i64]`) + `examples/algorithms/sort_search` — the
  first `algorithms` category (the public-data counterpart to `security`).
- **`b625601` — ADR 0051 payoff cleanup:** the crypto libs (`ct` / `siphash` /
  `poly1305`) dropped their widen-only `let`s (`ct_not` = `x ^ (0 - 1)`, etc.; the
  SipHash per-word + Poly1305 per-limb `let _: secret …` widens) — same behavior,
  re-verified by the example tests.
- **`5f02d36` — ChaCha20-Poly1305 AEAD** (`std/security/aead::chacha20poly1305_encrypt`
  + `examples/security/chacha20poly1305`, RFC 8439 §2.8): composes the shipped
  ChaCha20 + Poly1305 (OTK gen counter-0 → encrypt counter-1 → MAC
  AAD‖pad‖CT‖pad‖le64×2). Required **`poly1305` to take a SECRET key**
  (`&[secret u8]`, read via the secrecy-preserving `(secret u8) as i64` cast —
  `u8_to_i64` is public-only); the standalone poly1305 example now builds its key
  via the `[u8] → [secret u8]` widen. Reproduces both the ciphertext + tag for the
  §2.8.2 key/nonce. Only declassify boundaries: the public ciphertext + tag.
- **`Vec<secret T>` growable secret buffers (ADR 0052, `VecElem::Secret`) — fully
  self-hosted, the sixth gap closed.** The sibling of `[secret T]` (ADR 0047) on the
  `Vec` path: a *variable-length* secret byte buffer (`Vec<secret u8>`) built with
  `vec_new` + `push` and indexed to yield `secret u8` (public index → the CT taint;
  pointer/length/capacity public; a secret index rejected `IndexNotInt`; a branch on
  a secret element rejected). One crate (`sentinel-types`): `VecElem::Secret` +
  `to_type` arm + a guarded `to_vec_elem_secret` (resolution) + the Index-arm demote
  (`to_array_elem_secret`, else the `.expect` panics) — codegen/MIR/borrow unchanged
  (layout-free secret tag). The ONE substantive difference from `[secret T]`: the
  `Vec` builtins are GENERIC, so `vec_new<T>()` over `T:=secret u8` needs the
  substitution round-trip to yield `Vec<secret u8>` — a second `secrets`-free demote
  `to_vec_elem_subst` (no `secrets`-threading ripple). **No selfhost change** (`scg`'s
  structural interner already represents `mk_vec(mk_secret(u8))`); `tests/pass/
  c57_secret_vec` proves `scg`==`snc` byte-for-byte across all 8 differentials, both
  fixed points hold. Shipped `security/ct::ct_vec_eq` (the variable-length sibling of
  `ct_memcmp`, over two growable secret buffers) + `examples/security/ct_vec_eq`
  (builds the buffers by `push` in a loop). FINDINGS: `secret [u8]` (secret-of-array)
  IS representable, so the resolution guard is load-bearing to reject
  `Vec<secret [u8]>`; cross-module `pub fn` over `Vec<secret u8>` crosses `--separate`
  fine; `scg` does NOT mirror a `secret u8` let-widen from a CALL result (it mirrors
  literal/var let-widens + the operand widen) — orthogonal ADR 0019/0051 gap, the
  fixture routes around it (bind the public byte to a `u8` var, widen the var).
- **`vec_to_array` over a secret `Vec` (ADR 0053) + the full §2.8.2 vector + SHA-256 +
  HMAC.** Continuing "do these in recommended order":
  - **ADR 0053 — `vec_to_array(Vec<secret u8>) -> [secret u8]`** (the seventh gap, the
    symmetric completion ADR 0052 deferred). `vec_to_array` is a generic builtin, so
    its result ran through the ARRAY substitution demote (bare `to_array_elem`, rejects
    secret → fell back to `[T]`); fixed with `to_array_elem_subst` (the array twin of
    `to_vec_elem_subst`) at the three array substitution sites. No codegen/CT/`scg`
    change; `tests/pass/c58_secret_vec_to_array` byte-identical across all 8
    differentials. (`cf9b0ab`.)
  - **Full RFC 8439 §2.8.2 114-byte AEAD vector** (`examples/security/chacha20poly1305_full`)
    — the payoff: the 114-byte secret plaintext is built by `push` into a `Vec<secret u8>`
    and `vec_to_array`'d into the `[secret u8]` the AEAD consumes; ciphertext + tag match
    byte-for-byte (verified vs a Python reference), both `--separate` + merge.
  - **`std/security/sha256` — a constant-time SHA-256 over a `secret` message** (`a573822`).
    Branch-free `secret i32` compression; the 64-word schedule is a `Vec<secret i32>`
    (ADR 0052), the padded message a `Vec<secret u8> → [secret u8]` (ADR 0053), the
    running state mutated in place through a `&mut [secret i32]` borrow (ADR 0050, so it
    is never moved across the block loop — the move-checker rejects threading an
    aggregate by value through a loop; this is THE finding). Ch via the no-NOT identity
    `g ^ (e & (f ^ g))`; added `ct_rotr32`. Verified vs NIST "abc"/""/100×'a' (multi-block).
    Drafted by a focused sub-agent iterating against a Python reference, then reviewed +
    re-verified.
  - **`std/security/hmac` — HMAC-SHA256 over a `secret` key** (`d50c7b0`), composing
    sha256. The key + both padded blocks are secret → fully constant-time; the over-long
    key is hashed first via an `if`-expression branching on the PUBLIC key length.
    Verified vs RFC 4231 TC1/TC2/TC6 (TC6's 131-byte key exercises the hash-first path).
- **`std/security/aes` — a constant-time AES-128 block cipher** (`5fd89c7`), the
  sharpest constant-time demonstration so far + a no-language-change library increment.
  The textbook table-lookup S-box (`sbox[secret_byte]`) is a secret value indexing
  memory → REJECTED by the CT check, so the library computes the S-box arithmetically:
  S(x) = Affine(x^-1) with the GF(2^8) inverse as `x^254` (a fixed squaring/multiply
  chain) + the affine map (byte-rotations + `^ 0x63`) — table-free, branch-free, and it
  reproduces the standard AES S-box for all 256 inputs. Every transform (ShiftRows,
  MixColumns via xtime/gf_mul3, AddRoundKey, the RotWord/SubWord/Rcon key schedule) is
  branch-free with PUBLIC indices/shift amounts. KEY IDIOM: a byte is a byte-valued
  `secret i64` in [0,255] (AES is pure 8-bit GF math, so the i64 masks `& 255`/`& 1`/
  `>> 7`/`0 - x` need NO width casts — unlike SHA-256's `secret i32`), entering via
  `(secret u8) as i64` and leaving via `(secret i64) as u8`; the 16-byte state +
  176-byte key schedule are mutated in place through `&mut [secret i64]` /
  `&Vec<secret i64>` borrows (ADR 0050). Only the robust OPERAND widen is used (`^ 99`,
  `^ rcon[j]`) — `xtime`/`gf_mul3` avoid passing public constants as secret call-args.
  Verified vs FIPS-197 §C.1 + AES-128(0,0), both `--separate` + merge; an independent
  differential review confirmed it over 5000 random key/plaintext pairs. Scope = the
  raw ECB block primitive (no mode). NO ADR, NO `scg` change, NO fixture (library growth,
  snc-only example like the §2.8.2 full vector).
- **`std/security/x25519` — constant-time X25519 / ECDH** (`b671436`), the FIRST
  public-key primitive + (with AES) the sharpest constant-time demonstration. A naive
  Montgomery ladder branches on the secret scalar bits (`if bit { swap }`), leaking the
  private key by timing → REJECTED by the CT check, so the ladder's conditional swap is
  the branch-free mask-based `sel25519`; the build is the proof the scalar mult is
  constant-time in the secret scalar. NO language change (library growth) — the probe
  proved it expressible: field GF(2^255-19) = 16 limbs radix 2^16 in `secret i64` (the
  TweetNaCl rep; the schoolbook multiply's accumulator peaks ~2^43 < 2^63, so NO 128-bit
  arithmetic — deliberately NOT radix-2^51, which WOULD hit the 128-bit-multiply wall =
  the real numeric gap, dodged). KEY IDIOMS: (1) carries need an ARITHMETIC right shift
  on signed limbs, but `>>` is logical (ADR 0048) → a branch-free `arith_shr(x,n) =
  (x>>n) | ((0-((x>>63)&1)) << (64-n))` reconstructs it (no new operator); (2) field
  elements are mutated in place via `&mut [secret i64]` borrows and NEVER aliased —
  TweetNaCl's output-aliases-input ops become in-place `fadd_assign`/`fsub_assign` or a
  scratch `tmp` + `fe_copy`; (3) NEW borrow idioms (probe-proven): forwarding a `&mut`
  borrow into a nested fn (`fmul`→`car25519`) and forwarding a `&` borrow twice
  (`fsq`→`fmul`) both work. Verified vs RFC 7748 §5.2 + the §6.1 DH round-trip (both
  parties' shared secrets agree + match the published value), both `--separate` + merge;
  an independent review modeled the Sentinel source vs a big-integer ladder ground truth
  over 50 random pairs (no pass-by-luck), confirming the no-alias ladder is byte-identical
  + the Fermat exponent is exactly 2^255-21. NO ADR/`scg`/fixture (snc-only example).
  Scope = raw X25519 (one scalar mult; ECDH = two calls).
- **Four-increment session (owner-approved "do all four", HEAD `b671436`→`3f35b67`),
  four-check green each, NEVER pushed:**
  - **`std/security/sha512` — constant-time SHA-512** (`d6f7ef9`), the 64-bit-word twin
    of SHA-256. NATIVE `secret i64` words (no width casts — cleaner than SHA-256's
    `secret i32`), 80 rounds, 128-byte blocks, the SHA-512 rotations; + `ct_rotr64`.
    Verified vs NIST "abc"/""/200×'a'. Library growth, no gap.
  - **`std/security/aes_gcm` — constant-time AES-128-GCM AEAD** (`ac57eeb`), composing
    the AES block + GHASH. The GHASH GF(2^128) carry-less multiply (a 128-bit value =
    two `secret i64` limbs [hi, lo]; branch-free bit-by-bit shift-and-XOR + reduce, the
    textbook version branches on bits of the SECRET auth key H = AES_K(0)) is the new
    field primitive — probe-proven, no 128-bit integer type needed. ⚠ FINDING: the
    Python ref had to be validated vs OpenSSL (the McGrew "TC3" tag I first hand-typed
    was a transcription error); reproduces the McGrew/OpenSSL TC4 vector (60B pt + 20B
    AAD, partial blocks) + a no-AAD differential. Library growth, no gap.
  - **`&mut a[i]` / `&a[i]` element borrows (ADR 0054)** (`0ec884f`) — the eighth
    language gap + the only COMPILER change. Phase 1 = ONE arm (the `Index` arm of
    `check_mutable_borrow_target` recurses on the base, like `FieldAccess`); the
    element-address GEP (ADR 0050) + the borrow-check Index recursion + the Ref typing
    were ALL already in place, so codegen + borrow-check needed NO change. Whole-array
    borrow granularity (pre-Polonius); secret index still `IndexNotInt`. Phase 2 = NO
    `scg` change (codegen reuses ADR 0050's `cg_emit_index_addr`); `tests/pass/
    c59_borrow_index` validates `scg`==`snc` across all 8 differentials. +
    `std::math::num::clamp_assign(&mut i64,…)` + `examples/math/inplace`.
  - **`std/security/ed25519` — constant-time Ed25519 SIGNING** (`3f35b67`), the capstone.
    Factored the shared **`std/security/fe25519`** field module (GF(2^255-19), the proven
    X25519 radix-2^16 ops made `pub` + the Edwards constants d2/Bx/By); ed25519 composes
    it + `sha512`. A faithful TweetNaCl `crypto_sign` port: extended-coord `point_add`,
    the cswap double-and-add ladder over the SECRET scalar (branch-free), `point_pack`,
    `modL` (signed shifts via `arith_shr`). A point = 4 separate `[secret i64]` (aggregates
    can't move through a loop). ⚠ NEW IDIOM: forwarding a `&mut` param as a `&` arg needs
    an explicit reborrow `&(*x)`. Drafted by a focused sub-agent vs a `cryptography`-verified
    reference, then an independent review modeled the Sentinel source vs `cryptography` over
    120 random seeds (600/600 pk+sig) + 27015 modL cases + 55 point-add/ladder cases — all
    byte-identical. Reproduces 3 RFC 8032 vectors (incl. empty msg). Library growth, no gap.
  - **Ed25519 VERIFY** (`9e497a0`) — the natural completion (owner: "proceed on
    recommendation"). `ed25519_verify` decompresses `-A` from the public key (recover x
    from y via a field square root `(num/den)^((p+3)/8)` = the new `fe25519::fe_pow2523`
    `z^((p-5)/8)` chain + `fe_sqrtm1`/`fe_d`/`unpack25519`; the sqrt branch-correction +
    parity sign-fix are branch-free mask selects), computes `[S]B - [h]A`, accepts iff it
    re-encodes to `R`. A probe reproduced TweetNaCl `unpackneg(pk1)` byte-for-byte first.
    ⚠ FINDING (independent review, 150 valid + 310 forgery + 1000 decode-parity cases vs
    `cryptography`): the faithful TweetNaCl `crypto_sign_open` LACKS the RFC 8032 `S < L`
    check → `(R, S+L)` is a second accepted signature (malleability, a false-accept).
    FIXED with a constant-time MSB-first lexicographic `S < L` compare AND-ed into the
    accept; the example asserts the malleated sig is rejected. Verify is public (the
    boolean is the sole declassify) but stays branch-free (reuses the CT machinery).
  - **`std/security/sha3` — constant-time SHA-3 / Keccak** (`8908461`) — SHA3-256 +
    SHA3-512, the Keccak SPONGE (a different construction from SHA-2's Merkle-Damgård).
    The Keccak-f[1600] permutation (24 rounds θ/ρ/π/χ/ι) is branch-free bitwise ops over
    a 25-lane `[secret i64]` state mutated in place: XOR/AND, rotations by PUBLIC ρ
    offsets (`ct_rotl64`), and χ's complement via the no-NOT identity `~B = B ^ (0-1)`.
    Round constants + ρ offsets are PUBLIC tables, the `x mod 5` index wraps are public
    arithmetic, so only the lane VALUES are secret — the build is the CT proof. The
    sponge absorbs at the rate (136 B for SHA3-256, 72 B for SHA3-512) with the SHA-3
    pad `0x06..0x80`. Verified vs a hashlib-checked reference over abc / "" / multi-block
    / the 135-byte padding edge (`0x06` and `0x80` share a byte) — both instances. NO
    compiler/scg change (library growth). **Extended with the SHAKE128/256 XOFs**
    (`8d0b4f8`): `keccak_sponge` gained a domain byte (0x1F vs 0x06) + an arbitrary-
    length multi-block/partial squeeze (emit ≤ rate bytes, permute, repeat), so output
    is any length — `sha3_256/512` keep their fixed output, `shake128/256(msg, out_bytes)`
    are the XOFs. Verified vs hashlib over lengths 16..400 incl. >rate (two-permutation)
    outputs + the XOF prefix-extension property. **Then KMAC128/256** (`b6e031b`, SP
    800-185): the Keccak keyed MACs, wrapping a SECRET key around cSHAKE with the
    `left_encode`/`right_encode`/`encode_string`/`bytepad` length encodings (the new
    pieces) + the 0x04 cSHAKE domain byte. CT: the encodings prefix the input with
    PUBLIC byte-counts, so only the key+message bytes are secret. The Python ref
    reproduces 5 NIST SP 800-185 samples (empty + tagged customization, both variants,
    4- + 200-byte data); the Sentinel port reproduces 3 byte-for-byte.
- **Also done:** `d1dace8` `math::num` + `3e98443` **`std/bytes`** (`eq`/`find`/
  `contains`/`count`/`starts_with`/`repeat` over `&[u8]` borrows) + `examples/bytes/
  scan` — the agreed `ct`/`bytes`/`bits`/`math` starter set is complete. (Finding:
  byte utilities must take `&[u8]`, not `[u8]` by value, or the first call consumes
  the array; `&[u8]` params + `(*a)[i]` indexing work today.)
- **Next (owner's call):** the SSH / networked-`sshd` track, the data & text trio, **the
  `f64` float type (ADR 0058)**, the **`extern "C"` FFI import (ADR 0057 Phase 1)**, the
  **float follow-ups** (libm transcendentals · `f64`⇄string · JSON float numbers), and **the
  C-ABI export (ADR 0059 Phase 1a)** are all DONE — **the owner's whole big-list is
  implemented.** What remains are the deferred **Phase 1b/2 tails** of the FFI/export
  rocks, each a focused follow-up: **(a) the rest of the buffer ABI** — the export `&[u8]`
  INPUT side (`ct_byte_eq` from C) AND the owned-`[u8]` RETURN side are both DONE: ADR 0059
  A7 ships the return via the out-param pair `(uint8_t** out_data, int64_t* out_len)` +
  `sentinel_free_bytes`, with the ADR's headline demo — a verified-constant-time
  `sha256_oneshot` callable from C (`examples/export/digest_lib.sentinel`); what remains is
  the caller-provides-buffer convention (fixed-size, no-alloc outputs). ADR 0057's
  import-side buffer ABI is now DONE: the `ptr` type + `ptr_of`/`ptr_of_mut` (A6 —
  `getentropy`/`strlen`) AND the C-string READ-back (A7 — `is_null` + `cstr_read`/`env_get`
  via libc `strlen`+`memcpy`, so `getenv` is callable; `std/sys/ffi`,
  `examples/sys/ffi_buffers`); what remains there is only the niceties (a `?[u8]` to tell an
  unset var from an empty one);
  **(b)** `i32`/`u32` FFI widths + struct-by-pointer; **(c)** extra-library `-l` (so libm
  works on Linux; the Linux `ar`-MRI `--lib` + `cc -shared` paths) — `--shared` `.dylib` is
  now DONE on macOS (ADR 0059 A9 — `dlopen`/`ctypes` substrate; `examples/export/dlopen_driver.c`);
  **(d)** the Python (`ctypes`) / Rust (`-sys`) binding generators (ADR 0059 Phase 3 — now
  unblocked: `ctypes.CDLL(path)` loads the `--shared` dylib);
  **(e)** ~~multi-module export libraries (`--lib` with `use`)~~ DONE (ADR 0059 A8 —
  `--lib` now discovers + merges the `use` graph; `examples/export/crypto_lib` exports the
  real `std::security` SHA-256 + HMAC to C); **(f)** Win32 (ADR 0057/0059
  Phase 3). Plus the standalone float follow-ups: `f32`, an `extern`-symbol-aliasing form
  (`fn name = "c_sym"(…)` so a libm wrapper can reuse the idiomatic name), and a
  correctly-rounded float formatter (Ryū/Grisú vs the first-cut fixed-precision
  `f64_to_str`). The `scg` self-host mirror of `f64`/FFI/export also remains deferred until
  a `tests/pass` fixture exercises them. The detailed list below is now the HISTORICAL
  per-increment record — most of
  its "cleanly open next" items are shipped (read STATE.md §"Current State" + the commit
  log + the [[sentinel_examples_and_corelibs]] memory for the live picture).
  - **More crypto** — the §2.8.2 vector, SHA-256/512, SHA-3 (Keccak sponge), HMAC,
    HKDF, the full SP 800-185 derived-function family (cSHAKE / KMAC / TupleHash /
    ParallelHash), AES + AES-GCM, X25519, Ed25519 (SIGN + VERIFY), and X448 + Ed448
    (SIGN + VERIFY, over the new fe448 field) are all shipped — a full asymmetric +
    symmetric + hash + XOF + keyed-MAC + KDF suite. The **numeric gap is now CLOSED**:
    ADR 0055's `u128` type (LLVM `i128`) shipped, with `fe25519_64` + `x25519_64` (X25519
    at radix 2^51) as the demonstrator — the field multiply runs in `secret u128`,
    cross-checked byte-for-byte against the radix-2^16 X25519. (Of the run's four items
    only the numeric gap needed a language change; HKDF / SP 800-185 / X448 / Ed448 were
    pure library work. Ed448's port surfaced a real bug the Python model had hidden — its
    Barrett reduction leaned on a final big-int `% L` after only 3 conditional
    subtractions; the constant-time port does 8 always-executed masked subtractions.)
    Cleanly open next: **toward a real `sshd`** — the loopback transport is complete END
    TO END (KEX + KDF in `std::net::ssh` + the `chacha20-poly1305@openssh.com` record
    cipher in `std::net::ssh_cipher`, stitched in `examples/net/ssh_session`), and the ADR
    0056 sockets are now LIVE end to end: runtime layer (`b90a889`), compiler builtins at
    FnId 14..=20 (`0845b8a`, the FnId-shift crux closed byte-exact — builtins 0..=20 ⇒
    user fns start at #21, both bootstrap fixed points hold), and a real Sentinel program
    over them — `examples/net/tcp_echo` (`669665a`: bind 127.0.0.1:0, `spawn` a concurrent
    server task, connect/send/echo/read, exit 42; harness-tested on both back ends). AND
    THE SSH SESSION NOW RUNS OVER A REAL SOCKET — `examples/net/ssh_over_tcp` (`aac69a8`):
    the client and server are SEPARATE concurrent tasks (server `spawn`ed on an OS thread
    bound to a real ephemeral listener; client connecting from the main task) doing a
    DISTRIBUTED curve25519-sha256 KEX — each peer holds only its own ephemeral secret and
    derives the shared secret K independently (real DH, not one fn computing both views) —
    then ssh-ed25519 host-key auth, the §7.2 KDF, and an encrypted record that round-trips
    across the wire. ⚠ THE TRUST BOUNDARY (the point): the whole handshake is `[secret u8]`
    (build = the CT proof), but a socket carries PUBLIC bytes; the wire-public values
    (ephemeral pubkeys, host key, signature, ciphertext record) are `declassify`d per byte
    to send and WIDENED back to secret (public->secret, ADR 0049) on receipt — K and the
    session key never cross the socket. A `read_exact` helper coalesces a TCP write split
    across reads. `examples/net/ssh_session` stays as the vector-anchored in-memory
    reference. So the loopback `sshd` transport is now REAL-SOCKET end to end. A thin `std/net/tcp` wrapper now factors the socket-boundary helpers (`read_exact`
    + the secret<->public `declassify_bytes`/`append_declassified`/`widen_bytes` + a
    `loopback_host`) out of both socket examples (`8ba497c`; `ssh_over_tcp` and `tcp_echo`
    both dogfood it). AND the data phase now runs too — `examples/net/ssh_channel_stream`
    (`894a13d`): a bidirectional, multi-packet encrypted channel over a real socket — both
    directional keys ('C'/'D', RFC 4253 §7.2), a request/response ping-pong of N
    `chacha20-poly1305@openssh.com` records each way, each under its per-direction seqnr (a
    fresh nonce per packet), every record tag-verified BEFORE it is decrypted (verify-then-
    decode). The seqnr is load-bearing — folded into the Poly1305 nonce, so a desynced
    counter fails the tag (proven with a negative probe); the two directions use different
    keys so reusing seqnr i on both at once is safe. So the SSH transport is now real-socket
    end to end through BOTH the handshake and the data phase. AND the handshake is now
    FACTORED into a reusable library API — `std::net::ssh_handshake` (`e552fdf`):
    `ssh_client_handshake` / `ssh_server_handshake` each run their half of the
    curve25519-sha256 KEX over a connected socket and return an
    `SshSessionKeys { keyc, keyd, authed, session_id }` (the two directional keys, the
    client's host-key verdict, and the session id = the first exchange hash H).
    `examples/net/ssh_full_session` runs a COMPLETE session over one connection via that
    API: connect → handshake → N encrypted request/response packets → close. ⚠ a struct
    returned across a module boundary needs the struct type explicitly `use`d for
    `--separate` (the merge path resolves it transitively through the returning fn;
    separate compilation of the importing unit does not — "unknown type"). AND the
    **`ssh-userauth` layer** now exists — `std::net::ssh_userauth` + `examples/net/ssh_pubkey_auth`
    (`2d55f41`): RFC 4252 §7 publickey auth over a socket — the client Ed25519-signs a
    request bound to the session id (so a captured signature can't be replayed on another
    session), and the server verifies the signature AND enforces authorized_keys before
    granting access (both failure modes proven by negative probes — an unauthorized key and
    a wrong-session-id signature each rejected cleanly). AND the **connection layer** now
    exists — `std::net::ssh_connection` + `examples/net/ssh_channel_window` (`79bd5e8`):
    RFC 4254 channel messages (`CHANNEL_OPEN`/`_CONFIRMATION`/`_DATA`/`_WINDOW_ADJUST`/
    `_CLOSE`) over the encrypted transport, a "session" channel with WINDOWED FLOW CONTROL
    — a 32-byte payload crosses in two 16-byte `CHANNEL_DATA` chunks gated by a 16-byte
    window the server replenishes with `WINDOW_ADJUST`; each message a sealed record under a
    per-direction seqnr, sent length-prefixed (a seqnr-corruption probe rejects cleanly).
    **So the core SSH-2 sequence a real sshd runs is now COMPLETE end to end over real
    sockets: transport KEX → host-key auth → key derivation → encrypted channel → publickey
    user auth → windowed session channel** — every step machine-checked constant-time, both
    back ends byte-identical. AND `CHANNEL_REQUEST "exec"` now runs too — `std::net::ssh_connection`
    grew the exec/exit-status messages + the framed record I/O (`ssh_send_record`/`ssh_recv_record`,
    factored out of `ssh_channel_window`), and `examples/net/ssh_exec` (`61c6682`) does
    `ssh user@host some-command` end to end: open a channel → `CHANNEL_REQUEST "exec"` →
    `CHANNEL_SUCCESS` → command output as `CHANNEL_DATA` → `"exit-status"` → close. The SSH
    track is at a natural stopping point (the rest is peripheral: a `userauth`
    failure/retry loop; a `"shell"` request; multiple concurrent channels). **▶ THE ACTIVE
    TRACK IS NOW DATA & TEXT LIBRARIES** (owner-chosen). Shipped: `std::text::str` (`c6c0503`),
    the UAF fix that unblocked maps (`e66f57f`), and a **string-keyed hash map**
    `std::collections::map` (`3c08f2d` — `[u8]`→`i64`, parallel-array + index-chaining storage,
    public FNV-1a, power-of-two resize/rehash; `examples/collections/map_demo`). ⚠ KNOWN
    GENERIC GAPS (probed, shaped the map's concrete design): generic ENUMS don't parse
    (`enum Opt<T>` — so no `Option<T>`); generic struct-literal type-args don't infer through
    `push`; `Vec<[u8]>` (Vec-of-arrays) is unsupported and a non-Copy `Vec` element can't be
    mutated in place (D.3 MVP). AND **JSON shipped** — `std::data::json` (`9b2777b`):
    `json_parse`/`json_stringify` over a recursive `enum Json { Null, Bool(i64), Num(i64),
    Str([u8]), ArrNil, ArrCons(Json, Json), ObjNil, ObjCons([u8], Json, Json) }`. ⚠ arrays/
    objects are CONS-LISTS (nested recursive variants), NOT `Vec<Json>` — `Vec<enum>` /
    moving a non-Copy element out of a `Vec` by index isn't supported, but a recursive enum
    + a consuming `match` builds/frees cleanly (the self-host AST pattern); numbers are `i64`
    (no float type — a fractional part is parsed + dropped); string-lits support `\"`. **So
    the tractable data&text trio the owner asked for is DONE: strings + map + JSON.** The
    REMAINING items from the owner's big list are the LANGUAGE-GAP rocks (both now have a
    DESIGN ADR — PROPOSED/design-only): a **float type** (**ADR 0058** — an IEEE-754 `f64`,
    PUBLIC-ONLY: ⚠ NO `secret f64` because float ops aren't constant-time [subnormals/div/NaN
    microcode], so floats are a disjoint PUBLIC domain and the CT proof is unweakened —
    contrast `u128`, which IS secret-able; `Type::F64`→LLVM `double`, float literals,
    `fadd`/`fcmp`/`sitofp`, `sqrt` via the intrinsic, transcendentals + float↔string deferred
    to a `std/math/float` lib; snc-side first like u128, no FnId shift; the only thing
    blocking "math functions" beyond integers) and the
    **FFI/bindings** story — a C-ABI EXPORT for Python/C/C++/Rust calling INTO Sentinel, and a
    general `extern "C"` IMPORT for the native macOS/Win32/Linux bindings (today only ~31
    hardcoded runtime builtins). (Crypto from the list was already comprehensive.) **The FFI
    IMPORT side now has a DESIGN — ADR 0057 (`extern "C"`), PROPOSED/design-only:** a
    user-declarable `extern "C" { fn …; }` resolved against libc by the existing `cc` link, an
    opaque `ptr` type + `ptr_of`/`cstr` marshalling, a **secret-fence** (a `secret` value can't
    cross an FFI arg — keeps the CT proof sound, like the socket boundary), `std/sys/*`
    safe-wrapper modules, and macOS+Linux-first / Win32-later phasing. ⚠ `extern` fns resolve
    BY SYMBOL NAME (NOT a builtin `FnId`) → they AVOID the FnId-shift pain by construction. The
    ADR is the gate; Phase 1 (declarations + `ptr` + secret-fence + a `std/sys/posix` wrapping
    ~6 libc calls + a demonstrator) is the first implementation increment when picked up. **The
    C-ABI EXPORT side now has a DESIGN too — ADR 0059 (PROPOSED/design-only):** an `export "C"
    fn` annotation (un-mangled C symbol, the ADR-0057 FFI-safe ABI) + a `--lib` build mode
    (no `main`; archive the object(s) + the runtime staticlib into a `.a`) + `--emit-header`
    (a generated C header) + a `(ptr,len)` buffer ABI + `sentinel_free_bytes`. ⚠ THE HEADLINE:
    the secret-fence means an export takes PUBLIC bytes, WIDENS them to `secret` internally
    (ADR 0049), runs the machine-checked constant-time impl, and `declassify`s the result — so
    a C/Python/Rust caller gets **verified constant-time crypto as a drop-in library**. Phasing:
    static `.a` + header + a C-driver demo first (macOS+Linux); shared libs + struct exports +
    the Python(`ctypes`)/Rust(`-sys`) generators + Win32 `.dll` later. **▶ ALL THREE big-list
    rocks now have design ADRs: 0057 (FFI import) · 0058 (float) · 0059 (C-ABI export) — the
    design phase for the owner's whole list is complete; what remains is IMPLEMENTATION (each
    a multi-stage compiler increment).**
    Also
    open: a `secp256k1` / `P-256` field at radix 2^52 (more
    `u128` mileage + a recognizable new curve, possibly ECDSA); the **`scg` mirror of
    `u128`** (+ a `tests/pass/cNN` fixture) to fully self-host it (ADR 0055 deferred this
    — snc-side only today); more SP 800-185 XOF variants.
  - **Two deferred items from the list, now LOW value — recommend skipping unless
    wanted:**
    - **array-repeat `[x; N]`** — SHA-256/HMAC built cleanly WITHOUT it (`Vec<secret T>`
      + `push` covers fixed- and variable-length buffers), so it is no longer driven by
      any real program. A genuine minor convenience (a `[0; N]` initializer), but a full
      pipeline feature (parser + types + codegen + selfhost mirror) for small marginal
      value over `Vec`. Deferred, not blocked.
    - **Self-hosting completion** — mirror into `scg` the ADR 0051 call-arg/array/return
      widens + the call-RHS `secret` let-widen (ADR 0052 A5). Pure completionism: NO
      selfhost-corpus program uses these constructs, so mirroring them has zero
      functional impact (and carries byte-identity risk) until a fixture needs them.
      Deferred.
    - **`&mut a[i]` element borrows — DONE** (ADR 0054, `0ec884f`). The remaining
      borrow-checker limit is element-granular loans (whole-array granularity
      over-rejects `swap(&mut a[i], &mut a[j])`), deferred to the Polonius migration.
  - **More `std` categories** — networking / threading / process need runtime/
    syscall surface that does not exist yet (a real gap to scope first).
  - **Lower value (and partly blocked):** **Linux CI (P2.4)** needs a Linux/CI
    environment (cannot be validated on this macOS host — needs the owner's CI); the
    separate-comp tail (class/generic-instance dedup) is explicitly low-value; the
    deeper post-codegen constant-time verifier is research-hard.
  Keep both bootstrap fixed points + the selfhost differentials byte-identical (the
  full nextest is the gate).

**Other open tracks** (owner's call, none on the critical path):

- **Harden constant-time `secret` (the deep version)** — the check is sound for
  the current language (see Done) but runs pre-LLVM and doesn't force
  constant-time *emission*; a post-codegen / post-optimization verifier is the
  highest-ceiling but research-hard item. The real-program work above should
  inform it (you'll learn what the optimizer does to branch-free secret code).
- **Linux CI (P2.4)** — glibc surfaces heap bugs macOS masks; worth landing
  before `abi-v1` ossifies. Rest of P3 (LSP, diagnostics); P4 (perf, deferred
  ergonomics like `[secret T]` arrays — which the flagship work above will
  likely force).
- **Separate-compilation tail** — class / generic-instance type-arg dedup and
  trait/class-method dedup. Low value (classes can't be imported; trait impls
  are unit-local) — recommend not grinding it.
- **Borrow checker** is pre-Polonius — sound but over-rejects; the documented
  over-rejections are in [`borrow-check-limitations.md`](borrow-check-limitations.md),
  deferred to the Polonius migration.

**Working norms** (the four-check per change, ADR-first for frozen-ABI changes,
never push, additive — keep the merge path + both fixed points green) live in
`## Conventions` of [`STATE.md`](STATE.md) and in §9 / §11 below.

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
