# Sentinel — implementation history (archived running logs)

These are the chronological, milestone-by-milestone running logs that
accumulated in `STATE.md` and `HANDOVER.md` over the project's life. They were
extracted here on **2026-06-23** (the P3.2 docs cleanup) so the live docs can
carry a concise *current* state instead of an unbounded log.

They are kept **verbatim** for provenance and are **not maintained** going
forward — when this archive disagrees with `STATE.md` / the README, those win.
For the current state, read [`STATE.md`](STATE.md) (per-crate reference) and the
[README](../README.md) (overview).

---

## STATE.md milestone log (archived 2026-06-23)

_The running narrative formerly at the top of `STATE.md`: the per-unit
separate-compilation banner followed by the `▼ PRIOR MILESTONE` chronology back
through the self-host port and Phases A–C._

Last updated: **▶ (3/N) — INCREMENTAL CACHING WORKS (`299f17c` per-unit `.o` reproducibility + `ced9130`
the cache): a `--separate` rebuild reuses an unchanged unit's cached `.o`, so editing one module recompiles
only it + its importers (the rest print `fresh`), now at ITEM granularity (`63d93cc` — a body-only change to
an imported fn doesn't even recompile importers, they relink). THE PAYOFF of separate compilation. EARLIER
this session: cross-UNIT effect perform/handle (`b43b7d6`+`4c3a28b`) + `linkonce_odr` GENERIC DEDUP over
primitives (`c5ca3b8`+`8d14db8`) + cross-module STRUCTS (`a75a62c`+`7c6767a`) + ENUMS (`7d5dd46`) via
origin-qualified mono tags. See the running log below. NEXT (remaining tail, all LOWER value) =
class/generic-instance type args + trait/class-method dedup. ▶ TRACK — the per-unit SEPARATE-COMPILATION
back end (ADR 0037 (a)).
ADR 0037 FLIPPED toward the per-unit (1/N): D7 module-qualified mangling (`_S` + a
length-prefixed module segment per path segment + a length-prefixed item; **empty module
path → the bare item**, so single-file ABI is byte-unchanged → an AMENDMENT, not abi-v2)
+ the per-unit ID model (D5.1 — extern-fn-in-FnId-space: an imported fn is an extern
`FnSignature` + a module-qualified `link_symbol`, no body, NO new expr variant; imported
types are layout-only, no symbol) + the codegen-per-unit boundary (D5.2 — the 3 whole-
program assumptions to break: whole-program `collect_mono_instantiations`, the single
`fns: HashMap<FnId, FunctionValue>`, and `self.fns.get(&id)` call resolution) are all
PINNED — a frozen-ABI decision settled BEFORE code. Built ADDITIVELY: the Path A merge +
`snc merge` + BOTH bootstrap fixed points stay green; the per-unit path is opt-in
(`--separate`) until (2/N) parity, then becomes the default for `snc build`. NEXT: code
D.6 (1/N) — the non-generic vertical slice (`resolve_module` + per-unit codegen +
deterministic multi-object link + the D10 two-module phase-go). ✅ CODE STEP 1 DONE: the
D7 mangling primitive `mangle_qualified(module_path, item)` + the `abi_v1_mangling_qualified_is_stable`
golden test + the `docs/abi-v1.md` §4/§7/§8 amendment landed BEHAVIOR-PRESERVINGLY (`671b012`):
wired at the free-fn site with an EMPTY module path (== the bare name), so single-file ABI is
byte-unchanged — the pre-existing `abi_v1_mangling_is_stable` + the selfhost differentials + BOTH
bootstrap fixed points stay byte-identical (four-check green, 1498 tests). ✅ CODE STEPS 2-3 DONE:
the per-unit ID model's resolve + types halves — **`resolve_module`** (`8bd44c3`:
`FnSignature.extern_origin`, imported fns as externs in the per-unit FnId space after builtins,
`resolve()` = `resolve_module(&[])`) + **`check_module`** (`a6c6113`: `TypedFnSignature.extern_origin`,
each extern's typed signature built from a `TypedImportedFn`, `check()` = `check_module(&[])`) — both
additive + behavior-preserving (single-file byte-identical), +5 tests, 1503 tests, both fixed points
byte-identical. ✅ CODE STEPS 4-6 DONE — **THE PER-UNIT BACK END's FIRST VERTICAL SLICE IS LANDED +
WORKING:** (b-codegen) `compile_to_object_for_module` threads the module path + DECLARES imported
externs (`55bb5f6`); (c)+(d) `snc build --separate` (`b8d0ecb`) — discover → `resolve_imports` gate →
a pub-signature pre-pass (`extract_exports`, SCALAR sigs) → the exports table → per module
`resolve_module → check_module → effect/borrow/CT → compile_to_object_for_module(&module.path)` → N
`.o` → path-sorted `cc` link. 🎯 **THE D10 PHASE-GO IS GREEN: `main.sentinel` `use`s a `pub fn` from
`util/math.sentinel` → main.o + util_math.o emitted INDEPENDENTLY → linked via `_S4util4math3add` →
exit 42** + `ModuleNotFound`/`PrivateItem` ui tests. Two whole-program assumptions relaxed for library
units (no `main`): `MissingMain` moved to the `resolve()` wrapper; `effect_check` Pass 3 guards the
`main` lookup (both behavior-preserving for the corpus). ✅ CROSS-MODULE TYPES (struct + enum)
LANDED — `c2646ab` `pub struct` + `7b3529b` `pub enum`: a cross-module type is LAYOUT-only (no link
symbol, ADR 0037 D4), so the importer INLINES the imported decl into its per-unit program (driver
clones the program, clears `uses`, appends the imported struct/enum decls) and re-materializes it in
its own StructId/EnumId space — transparent to types + codegen (field/variant-payload types are
id-independent AST TypeExprs, re-resolved per unit). Phase-gos: main imports `Point` (struct) /
`Shape` (enum w/ payloads) from `util/geo.sentinel`, constructs + uses/matches locally → exit 42, both
`.o` independent. ✅ **TYPE-IN-SIGNATURE too** (`5f59591`): a cross-module fn whose signature
takes/returns an imported type (`sum(p: Point) -> i64`) now works — `TypedImportedFn` carries param/return
**TypeExprs** that `check_module` RE-RESOLVES in the importer's type space (so `Point` → the importer's
LOCAL StructId), subsuming the scalar path (`scalar_type` removed); the struct crosses by value (units
agree on layout). Phase-go: main constructs Point + passes it to `sum` across the boundary → exit 42.
🎯 **D.6 (1/N) — NON-GENERIC SEPARATE COMPILATION — is FUNCTIONALLY COMPLETE** (per ADR 0037 D9 (1/N):
`resolve_module` + per-unit codegen + D7 mangling + extern-symbol cross-module fn calls + cross-module
types [layout import] + deterministic multi-object link). 1509 tests, both fixed points byte-identical,
four-check green. ✅ **(2/N) OPEN — cross-module GENERIC FNS work** (`f533a62`): a `pub fn id<T>`
instantiated in a different module is INLINED into the importer + monomorphized LOCALLY (the simplest
correct model; `linkonce_odr` dedup deferred) — `ExportedItem::GenericFn(Box<FnDef>)`, the importer
`prog.fns.extend`s the imported generic body, and codegen module-qualifies the mono instance symbol by
THIS unit's path (`mangle_qualified(module_path, &mangle_mono_name(…))` → `_S4main…id__i64`; calls
resolve via the `(FnId, args)` map not by name, so 1 site; empty path → bare = single-file byte-identical).
Phase-go: main imports `id` from util/math, instantiates id<i64> → exit 42. 1510 tests, both fixed points
byte-identical. ✅ **generics over a CROSS-MODULE STRUCT work too** (`9a8376e`: `id<Point>` w/ Point
imported → 42) — the type-tag-collision concern does NOT bite the inline-local model (each importer
qualifies its instance by ITS OWN path + self-contains the inlined struct; no shared symbol to collide),
so it is needed ONLY for the `linkonce_odr` model. ✅ **cross-module TRAITS too** (`934f08c`): a `pub
trait` is INLINED into the importer, which impls it for its OWN class + dispatches — this REQUIRED
module-qualifying the last 3 codegen symbol kinds (class init/method + impl method, all via
`mangle_qualified`, calls resolving by ClassId/ImplId+idx maps so localized + byte-identical for the
empty-path corpus). ⚠ classes are module-LOCAL (the trait is the cross-unit construct, not the class).
counter.sentinel `pub trait Counter`; entry impls it for `class Tally` → 42. ✅ **cross-module EFFECT
DECLS too** (`a0e9595`, first cut): a `pub effect` decl is INLINED; FIRST-CUT scope = `perform`+`handle`
BOTH in the importer (so the EffectId / runtime `op_id=(eid<<16)|op` is unit-local + consistent — pure
inline, no codegen change). io.sentinel `pub effect Io`; entry performs + handles it → 42. ✅ **cross-UNIT
perform/handle DONE — THE HARD CASE** (`b43b7d6` op-id base map [1/2] + `4c3a28b` the feature [2/2]): a
library `io::source` PERFORMS in ITS OWN unit, the entry HANDLES it in ANOTHER → exit 40. **So ALL pub item
kinds now cross a `--separate` boundary: fn (extern + generic inline) / struct / enum / trait / effect-decl
/ effecting fn.** The `EffectId` is OVERLOADED (the `(eid<<16)|op` op-id basis AND the `effect_decls[]`
index), so a shared effect can't take a global eid — DECOUPLED via a build-wide **op-id base map** (effect
NAME → a graph-stable sorted index): codegen's `CodegenCtx::encode_op_id_ctx` consults it with a fallback
that DELEGATES to the standalone `encode_op_id`, so an EMPTY map (single-file / merge / corpus) is
BYTE-IDENTICAL and the oracle `llvm_dump.rs` is UNTOUCHED; the `compile_to_object` wrapper +
`run_build_merged` pass empty, `run_build_separate` computes name→index from all modules' own effects.
COUPLED: `extract_exports` stops rejecting effecting `pub fn`s + `ExportedFn`/`TypedImportedFn` carry
`effect_row_names`; `check_module` re-resolves them to the importer's EffectIds (effect analogue of the
param-TypeExpr re-resolution `5f59591`) → the non-empty row drives the extern's Kont* ABI via
`uses_kont_abi`; a name not `use`d → the new `TypeError::UnknownImportedEffect`. ⚠ KEY FINDING: a single-arm
top-level handler's `unreachable` default makes LLVM ignore a wrong op id, so the load-bearing phase-go uses
an op-id COLLISION (the entry handler lists arms for its own `Local` + the imported `Io`; no base map → the
io-unit kont hits the `Local` arm → exit 7, with → the `Io.read` arm → exit 40). ⚠ base map keyed by NAME
(MVP) → same-named cross-module effects collide (origin-qualified = robust upgrade). 1515 tests, both fixed
points byte-identical, four-check green. ✅ **`linkonce_odr` GENERIC DEDUP — PARTIALLY REALISED** (`c5ca3b8`
codegen [1/2, inert] + `8d14db8` the feature [2/2]): a mono instance of an IMPORTED generic over
COLLISION-SAFE args (primitives) now emits under the ORIGIN-qualified symbol with `linkonce_odr` linkage, so
N importers of `util::id<i64>` share ONE `_S4util4math…id__i64` def (the linker dedups) instead of each
carrying an importer-qualified copy. The driver threads a `generic_origins: FnId → origin-path` map to
codegen (mirroring the op-id base map); `mono_args_dedup_safe` GATES it to primitive args (a named-type tag
aliases by bare name — ADR point 8 — so those stay per-importer, sound, until the type-tag fix). Empty map
(single-file / merge / corpus) → the importer-qualified default path, BYTE-IDENTICAL. Load-bearing test
(two importers of `util::id<i64>`): the build LINKS (two external defs → duplicate-symbol error, so a clean
link proves the linkage — confirmed by hand) + `nm` shows ONE symbol → exit 42. 1516 tests, both fixed
points byte-identical, four-check green. ✅ **TYPE-TAG FIX (ADR point 8) DONE FOR STRUCTS** (`a75a62c`
codegen [1/2, inert] + `7c6767a` the feature [2/2]): a cross-module struct's tag in a `linkonce_odr` mono
key is now ORIGIN-qualified (`id__util$geo$Point` — `$`-joined module path via `mangle_type_dedup` + a
driver-supplied `StructId → origin` map), so `id<geo::Point>` dedups across importers (`mono_args_dedup_safe`
widened: primitive OR cross-module struct OR array/nullable/vec thereof; a LOCAL struct stays per-importer),
AND two SAME-NAMED structs from different modules get DISTINCT tags (`id__a$Point` vs `id__b$Point`) so the
linker never merges an 8-byte `id` with a 16-byte one — the exact unsoundness point 8 closes. Empty
`struct_origins` (single-file / merge / corpus) → a struct is "local" → byte-identical. Two load-bearing
tests: `id<geo::Point>` across 2 importers → ONE `id__geo$Point` (nm); + the same-named-Point collision →
distinct symbols → exit 12. 1519 tests, both fixed points byte-identical, four-check green. ✅ **+ ENUMS too**
(`7d5dd46`): the point-8 fix extends to cross-module enums (`id<shape::Shape>` → `id__shape$Shape`, dedups
across importers) — `mangle_type_dedup` gains an Enum arm + the two origin maps are BUNDLED as a pub
`NamedTypeOrigins { structs, enums }` (so the codegen sig stays at 6 args and a future named-type map is one
more field, not one more param). So `linkonce_odr` dedup now covers primitives + cross-module STRUCTS +
ENUMS. 1520 tests, both fixed points byte-identical, four-check green. ✅ **(3/N) INCREMENTAL CACHING WORKS
— THE PAYOFF** (`299f17c` per-unit `.o` reproducibility foundation + `ced9130` the cache): a rebuild REUSES
an unchanged unit's cached `.o`, so editing one module recompiles only it + its importers (the rest print
`fresh <module>`, cargo-style). `unit_fingerprint` = a `DefaultHasher` (fixed keys → process-stable) hash
over the unit's source + every imported module's source + the graph-wide sorted effect names (op-id base
map) + module path + compiler version, stamped into an `<obj>.o.fp` sidecar; a matching fingerprint + an
on-disk object skips the WHOLE per-unit pipeline. SOUND because per-unit codegen is reproducible (the
repro.rs foundation: two `--separate` builds emit byte-identical per-unit `.o`s). Driver-only → byte-identity
untouched. Tests: no-op rebuild → all `fresh` + runs; edit one of two sibling modules → the sibling stays
`fresh`, the edited module + `main` (imports it) recompile (the `!fresh main` assertion guards invalidation
soundness), edit takes effect → 42. ✅ **ITEM-GRANULAR fingerprint** (`63d93cc`): the fingerprint hashes the
imported ITEMS a unit uses (the `ExportedItem`s, `ExportedFn`/`ExportedItem` derive `Hash`), NOT whole module
sources — so editing one item doesn't recompile importers of an unchanged sibling, AND a non-generic imported
fn's BODY change keeps importers `fresh` (they extern-call it → relink picks up the new body; an inlined
struct/enum/generic/trait/effect carries its full decl so a change there does recompile). Tests cover both
directions (body change → importer fresh; struct field reorder → importer recompiles). NOT Salsa-backed (the
`--separate` path bypasses Salsa, a content hash is the analogue). ⚠ remaining coarseness: a graph-wide effect
change still recompiles every unit (the op-id base map is global). ▶ NEXT (remaining (2/N) tail, all LOWER
value): class / generic-instance type args + dedup cross-module trait/class METHODS. 1524 tests, both fixed
points byte-identical, four-check green.
▼ PRIOR MILESTONE — 🎯 Phase D movement 2 — the SELF-HOST PORT — (8g) THE BOOTSTRAP FIXED
POINT IS REACHED (ADR 0045 A18): the Sentinel compiler compiles ITSELF — `scg` lowers the
whole merged compiler to `.ll` byte-identical to the `snc llvm` oracle (83,536 lines), and
`cc`-ing that `.ll` yields `scg'` which re-emits the same `.ll` byte-for-byte (a true fixed
point). Owner-chosen path (b) merge-to-source (`snc merge` = `merge_modules` + a new
`source_dump.rs` un-parser, fed to the single-file `scg`; `$`-in-identifier lexer extension);
two (8g)-revealed `types.sentinel` cg gaps fixed to match the oracle (field-place GEP base →
the var's alloca slot; `match` `_` wildcard → final-else body+br, not `unreachable`). 1473
tests, modes 0–4 byte-identical, `scg` leak-free, four-check green. **✅ PATH (a) COMPLETE — THE TRUE
FULL SELF-HOST (owner-chosen; ADR 0045 A19–A20):** `selfhost/merge.sentinel` ports the D.6 module
merge to Sentinel, and `codegen.sentinel` now `use`s it — so **`scg` DISCOVERS + MERGES + EMITS the
whole multi-module compiler ITSELF, with NO Rust pre-pass**, lowering it to `.ll` byte-identical to
the `snc llvm` oracle (94,390 `.ll` lines for the full `codegen`+`merge`+`types`+`parser` graph);
`cc`→`scg'` self-reproduces; leak-free. (a-1)+(a-1b) the Sentinel un-parser (port of `source_dump.rs`)
+ (a-2)+(a-3) the self-hosted merge (BFS discovery + per-module SELF-CONTAINED rename map + fused
rewrite — one module at a time, no `Vec<Program>`/HashMap) + (a-4) `scg` self-merges (`merge_source` →
`types::run`). **1476 tests**, four-check green; guarded by
`sentinel_codegen_self_merges_the_compiler_and_reaches_fixed_point` +
`sentinel_merge_matches_oracle_on_multi_module_stages`. **BOTH fixed-point paths now reached — (b) 8g
merge-to-source + (a) self-hosted merge.** **▶ NOW: BAR B (full-corpus parity → ADR 0045 ACCEPTED; A21/A22).**
Per-slice lock-step (oracle `llvm_dump.rs` + Sentinel mode-4, mirroring the inkwell backend) for each
construct the corpus uses but the selfhost compiler doesn't. ✅ `print` (FnId 0) → **57 → 73**; ✅ **nullable
`?T`** (A22, `8150ccc`) → **73 → 80** (`?T` layout + null-lit/widen/`is_some`/`unwrap_or`/`x == null`); ✅
**secret + declassify** (A23, `7b471a9`) → **80 → 85** (strip-to-inner; `cgo_ty`/`Emit::lty` strip kind-3,
declassify/widen-secret identity); ✅ **generics (a) — generic STRUCT instances** (A24, `d3be39b`) →
**85 → 87** (`Decl<args>` → a structurally-named `%Box_i64` aggregate + subst-field layout/drop); ✅
**generics (b) — generic fns / MONOMORPHIZATION** (A25, feat `170a13a`) → **87 → 92** — the GENERICS slice is
COMPLETE (a generic fn emits once per instance under `id__i64`; oracle reuses inkwell's
`collect_mono_instantiations`; Sentinel re-walks each instance with the type-param scope bound to concrete
args; un-parser preserves fn `<T>`; the 5 c17 fixtures incl. `pick__i64`+`pick__bool`, leak-free, both
fixed-point paths preserved); ✅ **classes / traits / impls / delegates** (A26, feat `a1a3341`) → **92 → 98**
(c41/c42/c43/c4_named_impl). Pointer ABI mirroring inkwell: a class is `%Class.N` held by value; `init` =
`void @Class__init(ptr out,…)`, methods `@Class__m(ptr self,…)`, impl methods
`<prefix>__<Type>__<Trait>__<m>` (`default` or the impl name); `self` binds to `%arg0` (no alloca);
delegates need NO special cg (the type layer synthesised them into ordinary `self.field.m(args)` impls).
⚠ Both un-parsers (`source_dump.rs` + `merge.sentinel`) had to learn class/trait/impl/delegate DECLs (were
rejected/dropped). Sentinel side: a `cgcls` buffer (class/impl defines after the fns — the oracle's order),
operand kind 4 = `%arg0`, `cg_self_var`/`cg_arg_base`; ✅ **effects/handlers c35a — inline perform/handle/
resume** (A27, feat `29e3027`) → **98 → 101** (c35_handle_inline_perform, c35_handle_log_returns_msg, + the
type-clean negative c37_perform_outside_handle). The restricted handler case (a `handle` body that IS a
direct `perform`): `perform`→`sentinel_perform_op`; `handle`→a dispatch LOOP (load op_id @0, an IF-ELSE
CHAIN per arm — NOT a `switch`, since `HArms` is single-consumption — + a PURE_RETURN/`consume_pure` tail,
result memory cell, NO phi); `k(v)`→`kont_resume` + a pure-vs-bubble split. Kont ABI: op_id `(eid<<16)|op`,
PURE = `u32::MAX`, runtime owns kont memory (leak-free, no cg drops). ✅ **effects/handlers c35b — the
effecting-fn `Kont*` ABI + pure-return** (A28, feat `02891fd`) → **101 → 107** (c35b_handle_fn_call_body /
_multi_arm / _pure_return + c32_go_no_go + c33_go_no_go + the C5 phase-go **c5_go_no_go**). A fn with a
non-`Async` effect row returns `ptr` (a continuation), so a `handle` body that is a CALL to an effecting fn
dispatches on the returned kont; a pure tail wraps via `sentinel_kont_pure`. Oracle: lift the `dump_fn`
gate + `uses_kont_abi`/`validate_effecting_fn_body` (defer let-bound/embedded/chained perform) + `lower_call`
returns ptr. Un-parser (`merge.sentinel`): `emit_fn_decl` re-emits the `! { E }` row (the A24 `<T>` analog —
else the merged source loses it). Sentinel: a per-FnId `ufeff` table + `cg_eff`/`cg_tailk`. Modes 0–3
byte-identical; both fixed-point paths preserved (selfhost uses no effects). ✅ **effects/handlers c35c —
let-bound perform + the captured frame** (A29, feat `96c54b9`) → **107 → 110** (c35c_let_bound_perform /
_with_capture + the C3.7 phase-go **c37_go_no_go**: perform-with-arg + captured var + `print` → stdout 85).
A let-bound perform in non-tail position (`let v: i64 = perform Op(); <pure tail>`) reifies a CAPTURED FRAME:
the FIRST sub-slice emitting **TWO defines per source fn** + using `sentinel_kont_push`. PARENT (Kont* ABI):
alloc the captured struct (`i64[N]`, or null), lower the RHS perform → kont, `kont_push(kont, @__resume_<name>,
captured)`, ret. RESUMER `@__resume_<name>(i64 %arg0, ptr %arg1)`: bind the let var to %arg0 + captures from
%arg1, lower the pure tail, `kont_pure`-wrap, ret. Runtime owns kont/frame/captured (leak-free, no cg drops).
Oracle: `detect_let_shape` + `dump_let_shape_fn` + `collect_captured_vars` + a `kont_push` RuntimeSym. Sentinel
(`cg_emit_fn_eff` → `cg_letshape_emit`/`cg_eff_normal`): a single-SLet effecting fn IS a let-shape (the oracle
defers all other performing bodies); the capture set = the param list (c35c corpus: ≤1 param). Un-parsers
unchanged (no new syntax). Modes 0–3 byte-identical; both fixed-point paths preserved. ✅ **effects/handlers
c35d — embedded perform via placeholder substitution** (A30, feat `ecd150c`) → **110 → 113**
(c35d_binop_with_perform / _perform_in_call_arg / _perform_with_capture_and_binop, all exit 42). A statement-
free tail mixing exactly ONE perform into pure context (`perform Op()+1`, `f(perform Op())`) reuses the c35c
two-define frame: the PARENT lowers JUST the perform + `kont_push`es; the RESUMER re-evaluates the FULL tail
with the perform substituted by the resumed value. Oracle: `detect_embedded_shape` (a `collect_performs`
walker; before `validate_effecting_fn_body`) + `dump_embedded_shape_fn` + an `Emit::embed_ph` placeholder slot
(the Perform arm emits a `load` from it, not a perform call); the captured walk skips the Perform subtree
(= inkwell's substituted-tail walk). Sentinel: move semantics bar inspect-then-reuse, so `type_fn` re-parses a
disposable CLASSIFICATION COPY from the same tokens (mode-4 effecting fns only) → `eff_classify` extracts the
perform as a 1-element `Args` list → `cg_embed_emit` (the letshape mirror, ANONYMOUS `cg_ph` slot instead of a
let binding; `cg_emit_phload` in the Perform arm). Un-parsers unchanged. Modes 0–3 byte-identical; both
fixed-point paths preserved. ✅ **effects/handlers c35e — chained effecting lets** (A31, feat `6bdd23b`) →
**113 → 116** (c35e_chained_perform / _chained_dependent_perform / _chained_perform_with_capture, all exit 42).
A body of 2+ `let v: i64 = perform …` + a pure tail emits **N+1 defines**: the PARENT pushes resumer-0 onto
let-0's kont; each chaining resumer-i performs let-(i+1) + pushes resumer-(i+1) (the runtime BUBBLES the fresh
kont so the handle re-dispatches); the last wraps the tail. Oracle: `detect_chained_lets_shape` →
`dump_chained_lets_fn`; `compute_chained_captures(i)` = vars in (lets[i+1..].RHS + tail) minus lets[i..] (a
chained RHS's perform args ARE captured — the emitting resumer lowers them, unlike c35d). Sentinel: the
2+-stmt branch of `cg_emit_fn_eff` → `cg_chained_emit` — phase 1 re-parses a copy to bind the let vids, phase 3
consumes the ORIGINAL body for the N+1 lowerings, the capture sets are computed on-demand from fresh re-parses
(`cg_chained_caps` + the `cg_walk_ex` name-collector/disposal walk); `cg_chained_parent` reuses emit_tparams,
`cg_chained_resumer` cg_resets per define mirroring the oracle's alloca/fresh interleaving. ⚠ The self-compiled
alloca surfaced a scg quirk — an inline discarded `match` defaults its result to `ptr` (vs the oracle's `i64`),
so the navigate-collect lives in a helper (`cg_caps_collect`, tail position → i64-directed). Un-parsers
unchanged. Modes 0–3 byte-identical; both fixed-point paths preserved (the fixed point now also covers the new
c35e SOURCE self-compiling identically). ✅ **effects/handlers c36a — handle `return` arm + pure-body wrap**
(A32, feat `caf4175`) → **116 → 119** (c36a_return_arm_transform / _return_arm_after_resume / c37_handle_return,
exit 42/42/84). A non-identity `return v => body` arm transforms the pure value at each pure-drain site (the
dispatch pure block AND each k(v) pure path — Phase B's deep-handler re-wrap); a PURE body (`handle 42`) wraps
via `kont_pure`. Oracle: `lower_handle` drops the return-arm Err gate + defers a nested-Handle body (c36b) +
wraps pure bodies; `apply_return_arm` inlines the arm body at each site (the arm is carried in `handle_stack`).
Sentinel (the hard half — the body lowers at MULTIPLE sites but move semantics bar reusing the Expr): RE-PARSE.
`Ret::YesRet` gains the var + body token indices (parser); the tokens are copied into `TyCtx` (cgtk/cgts/cgte)
ONLY when mode 4 + a `return` token is present (the selfhost compiler has none → the fixed point pays nothing);
`cg_apply_return_arm` re-parses the body via `parse_expr` at each site. The `Ret::YesRet` AST change rippled
mechanically to 4 stages (parser/resolve/effects/merge — all dumps byte-unchanged). Pure-body detection via
`cg_tailk`. Modes 0–3 byte-identical; both fixed-point paths preserved (the new YesRet shape + types.sentinel
code self-compile identically). ✅ **effects/handlers c36b — nested handles** (A33, feat `b63cc98`) → **119 →
121** (c36b_nested_handle_basic / _inner_full, exit 42/42) — **THE LAST HANDLER SLICE; ALL EFFECTS DONE.** A
`handle` whose body is a handle: the inner (NESTED) handle lowers to a **Kont\*-typed result** — arms wrap their
i64 via `kont_pure`, the PURE_RETURN case passes the kont through (or re-wraps the return-arm'd value), and the
switch DEFAULT **propagates** the un-caught kont to the merge so the OUTER handle dispatches it. Oracle:
`lower_handle` gains a `handle_depth` counter (is_nested = depth > 1; split into `lower_handle` +
`lower_handle_inner` so depth decrements on every exit), a `ptr` result cell when nested, `store_handle_result`
(wrap-if-nested), the propagate default, and a nested-Handle body treated as kont-producing. Sentinel: a
`cg_h_depth` counter mirrors it; `is_nested` threads to `dump_tharms` (the arm store via `cg_store_hresult`) +
the dispatch tail (ptr rslot, passthrough/wrap, propagate, ptr merge load); a nested handle sets
`cg_tailk = is_nested` so the enclosing handle's body-kont detection sees the inner produced a Kont\*. Modes
0–3 byte-identical; both fixed-point paths preserved. ✅ **structured concurrency** (A34, feat `0f360cf`) →
**121 → 123** (c44_go_no_go [scope+spawn+await] + c4_go_no_go [the full C4 surface], exit 42/42) — **THE LAST
CONSTRUCT; ALL 123 PASS FIXTURES NOW EMIT; BAR B COMPLETE → ADR 0045 ACCEPTED.** `scope`/`spawn`/`await` lower
to the ADR 0024 runtime (`sentinel_scope_enter`/`_exit`/`_register` + `sentinel_task_spawn`/`_await`); a
`spawn` packs args into a heap buffer + spawns a `__spawn_wrapper_<id>` (synthesized once per unique target,
emitted last); a `Task<T>` is an opaque `ptr`. Oracle: 5 RuntimeSyms + `Emit::current_scope` +
`collect_spawn_targets_*`/`dump_spawn_wrapper` + `Type::Task → ptr` (lower_expr is now EXHAUSTIVE — the catch-all
is gone). Sentinel: the Scope/Spawn/Await arms gain cg (Spawn branches on `cg_on`, collects args via
`dump_targs` + `cg_emit_spawn` rather than invoking the target), `cg_scope`/`cg_spawn_t` +
`cg_emit_spawn_wrapper` (into `cgcls`), `cg_is_task → ptr`. Un-parsers unchanged (concurrency is out of
`source_dump`'s Bar-A scope, like declassify). Modes 0–3 byte-identical; both fixed-point paths preserved.
**BAR B IS COMPLETE: the Sentinel compiler reaches the bootstrap fixed point AND emits the full corpus
byte-identically to the Rust oracle. ADR 0045 → ACCEPTED-WITH-AMENDMENTS (A1–A34).** Deferred follow-on: the
per-unit separate-compilation back end (ADR 0037 (a)). (Full Bar-B breakdown in ADR 0045 A21; classes in A26;
c35a in A27; c35b in A28; c35c in A29; c35d in A30; c35e in A31; c36a in A32; c36b in A33; concurrency in A34.)
**POST-PORT (review action plan P1.2): the partial-move-through-field DOUBLE-FREE is CLOSED — `snc` AND `scg`
(ADR 0046 → ACCEPTED-WITH-AMENDMENTS A1–A3).** `snc`: per-(VarId, field) move state in `sentinel-borrow-check`
+ the `DropPlan.moved_fields` skip in BOTH codegen backends (the inkwell `sentinel-codegen` AND the `snc llvm`
`.ll` oracle `llvm_dump.rs` — A1) + 5 unit tests. `scg` (the D6 mirror): `selfhost/types.sentinel` records the
partial move (a Move-typed field consumed by value on a directly-named base — the base detected via the new
mode-independent `mvbv` channel, A2), dumps the `#<vid>.<field>` set, and elides the field in the mode-4
recursive drop; `selfhost/borrow.sentinel` is a thin wrapper, unchanged. The borrow + codegen differentials are
byte-identical over the WHOLE corpus and both bootstrap fixed points hold. The reproducer is accepted + correct
(exit 37, 0 leaks); a non-consuming-read regression (exit 4) + a use-after-partial-move reject + the pre-existing
`c17_go_no_go` (which RETURNS a generic field by value — A3) exercise it. See [[borrow-check-limitations]].
The full slice log:
(4/N) TYPES COMPLETE;
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
CONSTANT-TIME VERIFIER OPENED — ADR 0044 PROPOSED (owner-chosen over codegen-first); (7a)
the `snc mir` ORACLE + the SSA data-model PROBE DONE (A1).** The oracle (`run_mir`+`mir_dump.rs`,
8 goldens, `ce29b1e`) accepts the 123 type-clean fixtures, 0 panics; the standalone probe
CONFIRMED D4 (the data model — flat append-only parallel-`Vec` pools, NO Vec index-assign) +
de-risked the (7b) if branch-merge EARLY (reproduces `snc mir`'s `dbl`+`g` byte-for-byte,
leak-free). **(7a) straight-line INTEGRATION LANDED (A2, `dc20dd8`) — `selfhost/mir.sentinel`
(the 7th stage) via fused `mode 2`: 8 seeds match `snc mir`, modes 0/1 stay 123/123
byte-identical, leak-free. (7a) COMPLETE.** ⚠ one Sentinel rule found: passing `&mut
(*c).field` to a USER fn re-borrows `c` → render into a LOCAL buffer + `push`-fold into the
ctx field. **(7b) CONTROL FLOW LANDED (A3, `c15ce6d`)** — the branch+merge in the If/Logic
(`&&`/`||`) arms (VarId-sorted diverged-var merge params; an SAssign rebind via `mir_lastvid`);
+8 control-flow seeds match `snc mir`, modes 0/1 stay 123/123, leak-free. **(7c) the `Opaque`
catch-all + `Load` + calls LANDED + (7e) the full-corpus PHASE-GO is GREEN (A4, `bd4ca96`+`a0a5a3c`)
— `selfhost/mir.sentinel` matches `snc mir` BYTE-FOR-BYTE over the ENTIRE clean-lowering corpus
(123/123, `sentinel_mir_matches_oracle_on_corpus`), modes 0/1 stay 123/123, leak-free.** Mechanisms:
a `margs` operand stack + `emit_va` (call/opaque), op 5 load / 8 declassify, the widen-Opaque,
`mir_suppress` (place stores + handle arms), the unbound-Var (match-payload) Opaque. **✅ (7d) the
CONST-TIME VERIFIER LANDED (A5, `1868b38`) — (7/N) is COMPLETE; ADR 0044 → ACCEPTED.**
`selfhost/ctverify.sentinel` (via `types::run` `mode 3`, reusing the mode-2 MIR build gated by
`mir_on`=`mode>=2`) matches `snc ctverify` byte-for-byte over the type-clean corpus (123/123,
`sentinel_ctverifier_matches_oracle_on_corpus`) — empty for every CT fixture (no false positives),
`(leak Branch)` for `c52_secret_leak` (a secret `&&` — the ONLY type-clean MIR leak; index/divisor/
pointer are source-level type rejections), leak-free, modes 0/1/2 byte-identical. **The whole
pipeline lexer→…→borrow-check→MIR-lowering+const-time is ported. ▶ (8/N) CODEGEN — the GRAND
FINALE + the bootstrap fixed-point — is OPENED; ADR 0045 PROPOSED (2026-06-05), 3 owner
decisions settled:** (1) emission target = **textual LLVM `.ll`** (write_file → external
`clang`/`llc` → object → link `libsentinel_runtime.a`; probe-validated — a hand-written `.ll`
in the codegen style, alloca/load-store + memory-cell branch-merge **NO phi**, calling a
`sentinel_*` symbol, clang/llc-18 compiles + runs exit-correct at `-O0`); (2) oracle = a NEW
Rust **`snc llvm`** (`llvm_dump.rs`) canonical-`.ll` byte-parity differential (the port's
method — NOT inkwell `print_to_string`) **+ behavioural clang-run-parity + the fixed-point
capstone**; (3) scope = **fixed-point-first** (Bar A = the non-exotic core the selfhost sources
use → reach the bootstrap fixed-point; Bar B = effects/handlers/concurrency/classes/generics/
nullable for full-corpus parity, after). 🔑 SCOUT/PROBE findings that shrink the finale:
**no phi** (codegen is alloca/load-store at `-O0`, so the `.ll` needs no SSA merge — simpler
than 7/N's MIR), **secret codegen is a no-op** (strip-to-inner, codegen lib.rs:1594; the CT
guarantee = source rejections + the 7/N D5 verifier), **the selfhost sources declare 424 fns/
3 structs/14 enums + 0 traits/impls/classes/effects/generics + no nullable** (so Bar A is a
fraction of the 8263 lines; the ~2300-line handler/concurrency machinery is Bar B). Reuse =
the 6/N `types::run`-with-`mode` template, a new **`mode 4`** (8a probe: fused vs hybrid;
re-verify modes 0–3 byte-identical). 3-pass structure (type-decls / fn+runtime-symbol-decls /
body-emission) reproduced; `compile_to_object` reads TypedProgram + DropPlan (codegen
lib.rs:168). **✅ (8a-i) the `snc llvm` ORACLE LANDED (`1931496`, ADR 0045 A1):** `run_llvm` +
`llvm_dump.rs` (canonical `.ll`, partial-by-Err); 3-layer validation in `tests/llvm.rs` — goldens
pin the spec, a 0-panics sweep (16 emit / 125 Err over 141), and **16/16 behavioural parity**
(emitted `.ll` via `cc` == inkwell `snc build`). AS-BUILT spec: NO phi (alloca/load-store, `%vN`
counter, `%argN` params), `main`→i32-trunc, FnId order. D4 reuse SETTLED = **fused `mode 4`**
(mirrors the proven MIR `mode 2`: a `cgout` buffer + an operand-threading field like `lastval` +
a VarId→slot append-only pool like `mvdv` + a value counter; `type_fn` emits the define
header/footer). **✅ (8a-ii) `selfhost/codegen.sentinel` LANDED (`2ed426a`, ADR 0045 A2) —
(8a) COMPLETE:** the 8th + final Sentinel stage emits `.ll` straight-line, matching `snc llvm`
byte-for-byte (`sentinel_codegen_matches_oracle_on_corpus`, 16/16 emitted), leak-free, modes
0–3 byte-identical. The fused `mode 4` mirrors MIR `mode 2` 1:1 (cgout buffer + cglk/cglv
operand ≈ lastval + cgsv/cgsr slot pool ≈ var_defs + value counter; `mir_on`→2/3 only, `cg_on`
=4). 🔑 KEY FINDING: the A2 "no `&mut (*c).field` to a user fn" rule is sidestepped by
direct-to-`cgout` helpers using the BUILTIN `push` + consuming `[u8]` args by value (simpler
than MIR's render-to-local-then-fold; NO phi — alloca/load/store). **✅ (8b) CONTROL FLOW
COMPLETE (ADR 0045 A3; `c76db27` 8b-1 + `7b33d49` 8b-2):** if/else (br + a memory-cell merge,
NO phi) + while/break/continue (the real loop CFG + back-edge + a loop-target stack + a
dead-block-after-divergence) + short-circuit `&&`/`||` — byte-identical to `snc llvm` (26/26
emitted) + behavioural (cc==inkwell) + leak-free; modes 0–3 byte-identical. 🔑 THE ALLOCA HOIST
(8b-1) is the foundational refactor: every alloca (params/lets/if-results) is hoisted to the
entry block — solving both the if-result's late-known type (the parser AST has no precomputed
types) AND ADR 0036 per-iteration loop-stack growth; codegen.sentinel buffers the body in
`cgbody` + records allocas as (slot,type) pairs + a `cg_putc` router (cg_to_body) sends emission
to cgbody (walk) vs cgout (teardown assembly: header + hoisted allocas + folded body + ret).
**✅ (8c-1) STRUCTS LANDED (ADR 0045 A4) — opens slice (8c):** struct type decls + literals +
field reads, byte-identical to `snc llvm` (`sentinel_codegen_matches_oracle_on_corpus`, **32/32
emitted** — the 5 C1.4 pure-struct fixtures light up) + 4 seeds + 1 golden, behavioural
(cc==inkwell) + leak-free; modes 0–3 + effects byte-identical. A struct is a first-class SSA
VALUE — `insertvalue`/`extractvalue` over an aggregate `%vN` (the inkwell rvalue path), NOT
alloca/GEP — so `let`/`Var`/param/return/call carry it through the EXISTING alloca/store/load the
moment `cgo_ty` learns structs (kind-6 handle → `%Struct.N` via `struct_of_handle`). **Pass 0**
(`cg_pass0` in the mode-4 preamble + a buffer-targeted `ll_type_to`) emits `%Struct.N = type {
… }` per struct in StructId order. **struct-lit** = COLLECT-then-emit (the oracle switched
interleave→collect so both agree on side-effecting field values) reusing the call-arg stacks
(`cg_collecting`/`cgak`/`cgav`/`cgat` + a new `cg_emit_structlit`); **field read** = a new
`cg_extract` (`extractvalue`; chained `o.inner.x` nests). Generic structs (8h/Bar B) + field
ASSIGNMENT (`p.x = …`, the oracle's non-Var-lvalue limit) stay deferred — no emitting fixture
needs them. NEXT = **(8c-2) arrays** (lit + index + `sentinel_panic_oob` bounds-check) →
`[u8]`/strings (closing 8c) → (8d) Vec + builtins + drops.
**✅ (8c-2) ARRAYS LANDED (ADR 0045 A5):** array literals + indexing + `len`, byte-identical to
`snc llvm` (**42/42 emitted** — the 10 C1.6/C2.3 array fixtures light up) + 3 seeds + 1 golden,
behavioural (cc==inkwell) + leak-free; modes 0–3 + effects byte-identical. `[T]` is the abi-v1
`{ i64 len, ptr data }` — ONE inline literal type for every element type (data = opaque heap ptr),
so NO Pass-0 name; `let`/`Var`/param/return/call carry it via the EXISTING alloca/store/load once
`cgo_ty` learns `Type::Array` → `{ i64, ptr }`. A literal heap-allocs `n * sizeof(elem)` (the
GEP-sizeof idiom `getelementptr T, null, n` + `ptrtoint` — correct for any element incl. padded
structs) via `sentinel_alloc`, GEP-stores each element, builds `insertvalue {len,ptr}` (reusing
the call-arg collect stacks + `cg_emit_arraylit`). `a[i]` extracts len(0)/data(1), bounds-checks
(`sge 0` + `slt len` + `and`, br to ok/oob, OOB = `sentinel_panic_oob` + `unreachable`), GEP+loads
(`cg_emit_index`, reusing `cg_fresh_block`); `len` = `extractvalue 0`. **First runtime-symbol
declares** — emitted ONLY for symbols a program uses (per-symbol `used_alloc`/`used_panic`), so
8a–8c-1 stay byte-identical (`c16_empty_array` declares only `sentinel_alloc`). ⚠ Debug find: `len`
(FnId 3, a generic builtin) routes through `dump_gcall`/`dump_args_capture_first` which walked the
first arg WITHOUT `cg_collect`ing it (same gap as `dump_array_elems`) → empty `cgak` → SIGABRT;
fixed by collecting the first arg in both. NEXT = **(8c-3) `[u8]`/string literals** (closing 8c) →
(8d) Vec + builtins + drops.
**✅ (8c-3) `[u8]`/STRING LITERALS LANDED (ADR 0045 A6) — slice (8c) aggregates COMPLETE:** string
literals (+ the char-literal cg operand), byte-identical to `snc llvm` (**43/43 emitted** —
`c5d5_break_continue` joins, its `len("tok")=3` driving the exit) + 2 seeds + 1 golden, behavioural
(cc==inkwell) + leak-free; modes 0–3 + effects byte-identical. A string literal IS a `[u8]` (ADR
0033) — the decoded bytes heap-copied (`sentinel_alloc` + N constant `i8` stores) into `{ i64, ptr
}`, EXACTLY a u8 array literal of byte constants → reuses the array machinery: the oracle factored
the array-buffer scaffold into `emit_array_buffer` (shared by ArrayLit + StringLit so they can't
drift); the Sentinel `Str` arm pushes each byte as an `i8` literal operand then reuses
`cg_emit_arraylit` (read before `sink_name` consumes `sb`). Closed a latent gap: the `Char` arm now
sets the cg operand (`cglk=1`/`cglv=cv`, a u8 constant like `Int`) — needed by `c5d2_strings` (8d).
NEXT = **(8d) `Vec<i64>`/`Vec<u8>`** (`{len,cap,ptr}` + `sentinel_realloc`) + runtime builtins
(`str_eq`/`read_file`/`write_file`/`print_bytes`) + **heap drops** (DropPlan `sentinel_free`).
**✅ (8d, runtime builtins) LANDED (ADR 0045 A7) — the byte-array builtins, done FIRST within (8d):**
`str_eq`/`print_bytes`/`read_file`/`write_file`, byte-identical to `snc llvm` (**45/45 emitted** —
**`c5d2_strings`** (D.2 strings phase-go, str_eq) AND **`c5d4_file_io`** (D.4 file-IO phase-go,
read/write/print — REAL file I/O) join) + 2 seeds + 1 golden, behavioural (cc==inkwell) + leak-free;
modes 0–3 + effects byte-identical. Each builtin `extractvalue`s its `[u8]` into len(0)+ptr(1) and
calls the `sentinel_*` symbol as `(ptr, i64, …)`; `read_file` uses a hoisted out-len slot then
reassembles the `[u8]`. Refactor: the per-symbol declare bools became a **`RuntimeSyms`** struct
(merge + emit_declares, fixed order alloc/panic_oob/str_eq/read_file/write_file/print_bytes); the
Sentinel side mirrors via `cg_used_*` flags + a `cg_lenptr` helper (extractvalue len/ptr → len reg,
ptr=len+1). NEXT = **(8d rest) Vec** (`{len,cap,ptr}` + `sentinel_realloc`; `push`/`pop` take
`&mut Vec` → needs Bar-A **refs** `&`/`&mut`/`*`/`*p=x` first) + **heap drops** (DropPlan
`sentinel_free` — byte-parity-neutral for behaviour but needed for a clean fixed-point).
**✅ (8d-refs) REFERENCES LANDED (ADR 0045 A8) — the Vec prerequisite:** `&`/`&mut`/`*`/`*p=x`,
byte-identical to `snc llvm` (**53/53 emitted** — the 8 C2 ref fixtures light up) + 2 seeds + 1
golden, behavioural (cc==inkwell) + leak-free; modes 0–3 + effects byte-identical. A ref `&T`/`&mut
T` is an opaque `ptr` (mutability ignored; pointee from `program.refs` at the deref); a ref param is
a `ptr` slot. `&v`/`&mut v` = v's alloca slot (the slot IS the pointer, NO instruction); `*r` =
`load <pointee>, ptr <r-val>`; `*r = x` = `store <pointee> x, ptr <r-val>` (target ptr emitted FIRST
— the Sentinel walks an assign target-then-value). Sentinel REUSES the existing `cg_suppress`: `&`/
`&mut` suppress the inner Var's load + read its slot (`cg_slot_get`); `*` loads through r, or (assign
place) leaves r's pointer + `cg_lastvid=-1`; `&*r` keeps the deref-place's pointer. ⚠ `&*r` was a
1-byte miss first (`cg_slot_get(-1)`→`%v-1`) — fixed by an `un_vid>=0` guard. NEXT = **(8d-Vec)**
(`vec_new` constant + `push` realloc-grow CFG via `&mut Vec` + pop/len/vec_to_array, `{len,cap,ptr}`
data=field 2) → **heap drops** (DropPlan).
**✅ (8d-Vec-1) Vec IN-PLACE OPS LANDED (ADR 0045 A9):** `vec_new`/`push`/`pop`/`len`/`v[i]`,
byte-identical to `snc llvm` (**54/54 emitted** — `c5d5_loops` joins) + 1 seed + 1 golden, behavioural
(cc==inkwell) + leak-free; modes 0–3 + effects byte-identical. A `Vec<T>` is `{ i64 len, i64 cap, ptr
data }` (data = FIELD 2, vs `[T]`'s field 1). `vec_new` = the constant `{0,0,null}` (a new
`cgo_operand` kind 2); `push(&mut v,x)` = load len/cap through the `&mut Vec` field GEPs, grow if
`len==cap` (`sentinel_realloc` to `max(1,cap*2)*sizeof` via `select`+GEP-sizeof) + store cap/data,
then `data[len]=x`/`len++` (a grow/cont CFG, no phi, returns i64 0); `pop` = empty-check + decrement +
return `data[len-1]`; `len`/`v[i]` use the arg's ACTUAL aggregate type (`cg_emit_index` gained the
target type, data field keyed on `cg_is_vec`). New `sentinel_realloc` declare (`RuntimeSyms`/`cg_used_*`
gained realloc). ⚠ the oracle lowers both push args before the GEPs (matching the Sentinel
collect-both-first). **The grow-CFG matched the differential first try.** NEXT = **(8d-Vec-2)**
`vec_to_array` (the `Vec`→`[T]` memcpy bridge → `c5d3_collections` emits) → **heap drops** (DropPlan).
**✅ (8d-Vec-2) `vec_to_array` LANDED (ADR 0045 A10) — the `Vec`→`[T]` memcpy bridge:**
`vec_to_array(v: Vec<T>) -> [T]`, byte-identical to `snc llvm` (**55/55 emitted** —
`c5d3_collections`, the D.3 Vec phase-go, joins) + 2 seeds + 1 golden (`llvm_vec_to_array_bridge`),
behavioural (cc==inkwell) + leak-free; modes 0–3 + effects byte-identical. The Vec is passed BY VALUE
(`{i64,i64,ptr}`): `extractvalue 0`=len, `extractvalue 2`=src data; size=`len*sizeof(T)` (GEP-sizeof
idiom); `sentinel_alloc` dest; `call void @llvm.memcpy.p0.p0.i64(... i1 false)` the live prefix;
`insertvalue` the `[T]` `{len,dest}`. NON-consuming (an independent copy). `llvm.memcpy` is a NEW
intrinsic declare (`RuntimeSyms`/`cg_used_*` gained `memcpy`; declares LAST — it's `@llvm.*`, not
`sentinel_*`). FnId 10 routes via `dump_gcall` → `cg_emit_call`'s `fid==10` arm; elem via
`vec_elem_of` (no `strip_ref` — a bare `Vec`, not `&mut Vec`). **Matched the differential first try.**
⚠ DO-FIRST honoured: the ~83s behavioural test was SAMPLED (targeted inkwell-vs-cc on the one changed
fixture + 2 seeds), then run once as the final gate. NEXT = **heap drops** (DropPlan `sentinel_free` at
scope exit — the textual `.ll` now allocs `Vec`/array buffers it never frees; byte-parity-NEUTRAL for
behaviour, needed for a clean fixed-point) → **(8e) enums/match** → **(8f) calls/recursion/multi-module**
→ **(8g) THE BOOTSTRAP FIXED-POINT.**
**✅ (8d-drops-1) SCOPE-EXIT HEAP DROPS LANDED (ADR 0045 A11) — array/`Vec`/`[u8]` `sentinel_free`:**
at each block exit the un-moved heap bindings are freed in reverse decl order; byte-identical to `snc
llvm` (**55/55**, now WITH drops) + 2 seeds + 5 updated + 1 new golden, behavioural (cc==inkwell — drops
don't change exit/stdout) + **leak-free** (13/15 heap fixtures; 1466 tests, four-check green); modes
0–3+effects byte-identical; scg compiler leak-free. 🔑 TWO simplifying calls (leaks-validated): (1)
**per-binding `sentinel_free`, NO arena** (the arena is a perf opt; per-binding is equally leak-free +
our canonical spec — `c54_scope_arena` clean); (2) **skip `moved_sources` ALONE, no tail-returned guard**
(the body tail is walked consuming → `{tail}⊆{moved}`; `cg_is_moved` ⟺ oracle `DropPlan`, proven == in
6/N — `c24_moved`/`c23_array_move` no double-free). **Two-frame fn** (scope-0 params + scope-1 body;
body drops then param drops before `ret`). Oracle: `run_llvm` runs `borrow_check`, threads `DropPlan`
into `dump`. Sentinel: `cgdv`/`cgdt`/`cgds` pool + `cg_drop_record`/`cg_drop_frame`/`cg_emit_drop`/
`cg_is_moved`; `sentinel_free` declare after realloc. ⚠ REMAINING: `c16_array_in_struct` → **(8d-drops-2)**
struct recursive field drop; `c5d5_break_continue` → **(8d-drops-3)** loop-exit drops. Then → **(8e)
enums/match**.
**✅ (8d-drops-2) RECURSIVE STRUCT-FIELD DROP LANDED (ADR 0045 A12):** dropping a struct GEPs into each
heap-backed field (decl order) + frees it recursively; byte-identical to `snc llvm` (**55/55** —
`c16_array_in_struct` now frees its `[i64]` field) + 1 seed + 1 golden (`llvm_struct_field_recursive_drop`),
behavioural (cc==inkwell exit 12) + **leak-free** (c16_array_in_struct 0 leaks, a real free now); modes
0–3+effects byte-identical; 1467 tests; scg leak-free. `emit_drop_for_binding`'s `ptr_reg` is uniformly a
`%vN` (alloca slot OR a `getelementptr %Struct.K, …, i32 0, i32 idx` field reg); a `needs_drop`/
`cg_needs_drop` predicate (array/Vec true; struct→any field) gates which fields GEP (the GEP idx = the
field's DECL position). Sentinel scans the flat `fldo`/`fldty` table (like `cg_pass0`). Only
`c16_array_in_struct` changed. NEXT = **(8d-drops-3)** loop-exit drops (`c5d5_break_continue` — per-iter
`[u8]` leaks on break/continue paths). Then → **(8e) enums/match**.
**✅ (8d-drops-3) LOOP-EXIT DROPS LANDED (ADR 0045 A13) — HEAP DROPS COMPLETE:** a `break`/`continue`
drains the open frame(s) down to the loop-body frame BEFORE branching, so a per-iteration heap binding is
freed on the early-exit path too. Byte-identical to `snc llvm` (**55/55**) + 2 seeds (break+continue) + 1
golden (`llvm_loop_exit_drops_on_break`), behavioural (cc==inkwell) + **leak-free**; modes 0–3+effects
byte-identical; 1468 tests; scg leak-free. **🎯 ALL 15 heap fixtures leak-free under `leaks --atExit`** —
generated programs match inkwell; heap drops (drops-1/2/3) DONE. Each loop records a `scope_floor` (scope
depth at body entry); break/continue drain every frame `>= floor` (innermost first) WITHOUT popping (dead
remainder re-drops into an unreachable block → each path frees once). Oracle: `loops` gains `scope_floor`;
`emit_scope_drops`→`emit_frame_drops(idx)`+`emit_loop_exit_drops(floor)`. Sentinel: a `cg_loop_floor`
stack + `cg_drop_range(floor)` (drop-only, no truncate); 🔑 reversing the flat `cgdv[floor..]` == the
oracle's top-down per-frame reverse. NEXT = **(8e) enums/match** → **(8f) calls/recursion/multi-module** →
**(8g) THE BOOTSTRAP FIXED-POINT.**
**✅ (8e-1) ENUM TYPE + ENUM-CONSTRUCT + ENUM DROP LANDED (ADR 0045 A14):** an enum is the abi-v1
`{ i32 tag, ptr payload }` (ADR 0032); construct = `{tag, heap-boxed-payload-or-null}` (unit→null;
payload→`insertvalue` struct + GEP-sizeof box + store); drop = null-check payload + `sentinel_free` (BOX-
FREE-ONLY, matches prod D.1b limit; `needs_drop(Enum)`=any variant has payload). Byte-identical to `snc
llvm` (**55/55** — `c5d1_enum` needs match (8e-2), corpus unchanged) + 2 seeds + 1 golden
(`llvm_enum_construct_and_drop`), behavioural (cc==inkwell) + leak-free; modes 0–3+effects byte-identical;
1469 tests; scg leak-free. `{i32,ptr}` is INLINE (no Pass-0 name); payload struct `{…}` recovered per
variant (`varpay[varps[j]..]` via `variant_flat`). Sentinel: `cgo_ty`/`ll_type_to` learn `enum_of_handle`;
`cg_emit_pstruct` + `cg_emit_enum_construct` (Qcall enum arm wraps `dump_cargs` in `cg_collecting`);
`cg_emit_drop`/`cg_needs_drop` enum arms. NEXT = **(8e-2) match** (switch on tag → per-variant payload
bind + memory-cell merge; lights up `c5d1_enum`) → **(8f) calls/recursion/multi-module** → **(8g) THE
BOOTSTRAP FIXED-POINT.**
**✅ (8e-2) MATCH LANDED (ADR 0045 A15) — (8e) ENUMS COMPLETE:** `match` = an IF-ELSE CHAIN over the
variant arms (NOT a switch — single-pass, since the Sentinel walks the arm cons-list consuming; a switch
would need a temp buffer) + per-arm payload bind + memory-cell merge. Byte-identical to `snc llvm`
(**57/57** — `c5d1_enum` + recursive-enum `selfhost_ast_drop` join) + 2 seeds + 1 golden
(`llvm_match_if_else_chain`), behavioural (cc==inkwell: c5d1→42, ast_drop→11) + leak-free; modes
0–3+effects byte-identical; 1470 tests; scg leak-free. **THE MOST COMPLEX EMISSION — byte-identical FIRST
TRY.** Per arm: `icmp eq tag,vidx`+`br arm/next`; arm: bind payloads (`getelementptr <pstruct>, payload,
0, i` + load → hoisted slot, keyed by VarId, NOT drop-recorded — aliases the box) + body + `store result`
+ `br merge`; `unreachable` default; merge `load`. Result slot type = `expr.ty` (oracle) / `exp`
(Sentinel). Sentinel: 6 `cg_m_*` fields (tag/payload/result/merge SAVE+RESTORE for nesting; armnext
captured into a dump_tarms local; pj for the pstruct); prologue+bind in `dump_tpat`/`dump_tbinds`,
epilogue in `dump_tarms`. NEXT = **(8f) calls/recursion/multi-module** → **(8g) THE BOOTSTRAP
FIXED-POINT** (the Bar-A construct set is now complete — only whole-program plumbing remains).
**✅ (8f-1) THE SELFHOST FRONT-END STAGES SELF-HOST (ADR 0045 A16):** the Sentinel codegen (`scg`) emits
its OWN `selfhost/lexer.sentinel` (390 lines → 4378 `.ll` lines) + `selfhost/parser.sentinel` (2590 →
21606) **byte-identically to `snc llvm`**, AND the `cc`-compiled `.ll` runs == inkwell — the compiler
compiling its own source. **NO codegen change** — the Bar-A set (closed at 8e) already suffices; these
stages exercise it at 20×–500× the largest corpus fixture. Locked in by
`sentinel_codegen_matches_oracle_on_selfhost_stages`; 1471 tests. The self-contained stages (no `use`)
lower via the single-file pipeline; the multi-module stages (`types`/`codegen`/…) need the merged path
(`snc llvm` rejects `use` today; `snc build` already merges via `run_build_merged`). NEXT = **(8f-2)** wire
`snc llvm` to the merged multi-module path (mirror `run_build`+`merge_modules`, emit `.ll`) → **(8g) THE
FIXED-POINT** (the harder half: `scg` is single-file, so self-compiling the multi-module compiler needs
`scg` to merge too — port discovery+`merge_modules`+`Renamer` to Sentinel, or a pre-merged source).
**✅ (8f-2/8f-3) `snc llvm` LOWERS THE FULL SELF-HOSTING COMPILER (ADR 0045 A17):** the multi-module path
is wired (`run_llvm` → `discover_module_graph` → `merge_modules` → new `run_llvm_merged`, mirroring
`run_build`) + the last two lvalue stragglers ported (`&mut (*c).f` + `(*c).f = x` — address-of/assign-to
a field through a `&mut` ptr). So **`snc llvm` emits EVERY selfhost stage**, incl. the merged
`codegen.sentinel` (parser→types→codegen, ~83k `.ll` lines); `cc`-run == inkwell. The Bar-A construct
coverage is PROVEN COMPLETE on the real compiler. Oracle: `lower_lvalue_ptr` gained `FieldAccess` (GEP
into the target's pointer) + the `FieldAccess` assign target. 🔑 Sentinel needed ONE change — a
`FieldAccess`-under-`cg_suppress` GEP branch signalling `cg_lastvid=-1`; the existing `&mut`/`SAssign`
machinery already treats `-1` as "the operand IS the place address", so both forms fell out.
Seed-validated (scg==oracle, cc==inkwell, leak-free); 1472 tests, four-check green.
**🎯🎯🎯 ✅ (8g) THE BOOTSTRAP FIXED POINT — REACHED (ADR 0045 A18) — THE SELF-HOST CAPSTONE:** the
Sentinel codegen `scg` (`snc build`, inkwell) lowers the WHOLE multi-module self-hosting compiler AND
reproduces it. **(1) Self-compilation:** `scg` reads the MERGED compiler source (`snc merge`) and emits
`.ll` BYTE-IDENTICAL to the `snc llvm` oracle (83,536 `.ll` lines). **(2) Fixed point:** `cc` that `.ll`
→ a fresh compiler `scg'`, which re-emits the SAME `.ll` byte-for-byte — the compiler reproduces its own
output. **THE SENTINEL COMPILER COMPILES ITSELF.** Owner-chosen **path (b), merge-to-source** (ADR
0045 D8(ii)): the Rust driver `merge_modules` + a new `source_dump.rs` un-parser print the multi-file
compiler to ONE `$`-qualified `.sentinel` fed to the unchanged single-file `scg` (every STAGE
lex→…→codegen runs in `scg`; only the module-merge pre-pass stays in Rust). Enablers: a `$`-in-identifier
lexer extension (Rust regex + `parser.sentinel`/`lexer.sentinel`, corpus-neutral — `$` unused) so merged
names round-trip; the Rust-only round-trip gate (`snc llvm <merged-source>` == `snc llvm <entry>`). TWO
(8g)-revealed `types.sentinel` cg gaps — surfacing only in the merged `types`/`codegen` (not the
corpus/lexer/parser) — fixed to match the oracle: **(i) field-place GEP base** (`&mut c.f`/`c.f = x` on a
LOCAL struct → GEP the var's alloca SLOT via `cg_slot_get`, not the stale operand the A17 `cg_suppress`
left; `cg_lastvid>=0`→slot else operand, mirroring the oracle's `lower_lvalue_ptr`); **(ii) `match`
wildcard** (a `_` arm emits body+`store`+`br merge` as the final else via a save/restored `cg_m_wild`
flag, not `unreachable`). Capstone test `sentinel_codegen_reaches_the_bootstrap_fixed_point`; **1473
tests**, modes 0–4 byte-identical (8 stage differentials green), `scg` leak-free (`leaks --atExit`: 0
leaks lowering the merged compiler), four-check green. **The headline self-host milestone is reached.**
NEXT (optional, for ADR 0045 → ACCEPTED over the full ~123 corpus): **Bar B** — generics + nullable +
classes/traits/impls + effects/handlers + concurrency (D7/D10); OR re-scope Bar B as a deferred
follow-on and close the port at the fixed-point (owner call). Path (a) — self-hosting the module merge
itself — is a separable strictly-stronger follow-on.
The kickoff ADR's own line below was the prior NEXT pointer (now superseded).
The back-half scout REFRAMED the handover's "HIR/MIR → codegen": **HIR is a no-op** (a 101-line
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


---

## HANDOVER.md status log (archived 2026-06-23)

_The running narrative formerly in `HANDOVER.md` `§0. Current Implementation
Status`: the milestone log, the `▶ RESUME AT` / `▶ NOW ON` blocks, the recipes,
and the code maps that tracked the bootstrap → self-host → separate-compilation
work as it landed._


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
**Phase C1.5 — nullable types `?T` + null literal + unwrap_or / is_some builtins — complete (D10 deferred; retired at C1.6).**
**Phase C1.6 — arrays `[T]` + indexing `a[i]` + `len` builtin + heap runtime + ADR 0014 D10 unlock — complete.**
**Phase C1.7 — witness-table generics (generic fns + generic structs + monomorphisation) — complete. Phase C1 closes.**
**Phase C2.0.1 — lexer (`&` token + `mut` keyword) — complete.**
**Phase C2.0.2 — refs / mutability / deref / assignment infrastructure — complete.**
**Phase C2.1 — shared-only lexical borrow checker — complete.**
**Phase C2.2 — `&mut T` + shared-XOR-mutable rule — complete.**
**Phase C2.3 — move semantics + use-after-move — complete.**
**Phase C2.4 — RAII / drop + `sentinel_free` (closes the C1.6+ heap-leak deferral) — complete.**
**Phase C2.5 — polish + Polonius migration plan + struct-field recursive drop + ADR 0017 → ACCEPTED-WITH-AMENDMENTS — complete. Phase C2 closes.**
**Phase C3.0(a) — lexer: six new keywords (`effect`, `secret`, `declassify`, `handle`, `with`, `perform`) — complete.**
**Phase C3.0(b) — AST + parser + resolve pass-through + types-layer rejection for effect_decl / effect_row / `secret T` / `declassify(e)` — complete.**
**Phase C3.1 — secret typing: `Type::Secret(SecretId)` interner + `declassify(e)` + implicit `T → secret T` widening + 2 of 4 CT rejections (SecretBranch, SecretInRefDeref) — complete.**
**Phase C3.1b — operator-secret-preserving rules + SecretDivisor — complete.**
**Phase C3.2(a) — effect_decl + effect_row data model in resolve + types — complete.**
**Phase C3.2(b) — sentinel-effect-check crate + effect_check_query salsa pass — complete.**
**Phase C3.3 — typing-layer close-out: c33_go_no_go fixture + ADR 0019 → ACCEPTED-WITH-AMENDMENTS — complete. Phase C3 typing layer closes.**
**Phase C3.4 — handler runtime typing layer per ADR 0020 D5+D6: AST + parser + resolve + Type::Kont interner + type-check + effect discharge; codegen lands at C3.5/C3.6 — complete.**
**Phase C3.5(a) — restricted-case handler codegen per ADR 0020 D7: 3 new runtime symbols + lower Perform/ResumeKont/Handle (body must be a direct `perform`); end-to-end runnable for the inline case — complete.**
**Phase C3.5(b) — effecting fn ABI + handle-of-call per ADR 0020 D7: effecting fns return Kont* at the IR level; sentinel_kont_pure wraps pure values via PURE_RETURN_OP_ID; handle codegen unified around a runtime switch — complete.**
**Phase C3.5(c) — let-bound perform via per-let resumer fns + sentinel_kont_push per ADR 0020 D7: first piece of per-eval-site frame reification; SentinelKont grows a frame-chain; sentinel_kont_resume replays frames head→tail — complete.**
**Phase C3.5(d) — unified embedded-perform shape per ADR 0020 D7: count_performs / find_unique_perform / substitute_perform_with_var walkers; supports binop / struct-lit / fn-call-arg / index / etc. with single embedded perform — complete.**
**Phase C3.5(e) — chained effecting lets via resumer-can-perform per ADR 0020 D7: sentinel_kont_resume returns *mut SentinelKont (bubble-aware); handle becomes a dispatch loop with alloca'd current_kont_slot; compile_effecting_fn_with_chained_lets emits N per-let resumers — complete.**
**Phase C3.6(a) — non-identity return arm per ADR 0020 D4: lower_handle binds return arm value + lowers body in pure_block; HandleContext carries return_arm so k(v)'s pure-unwrap path applies it per Phase B's deep-handler re-wrap — complete.**
**Phase C3.6(b) — nested handles per ADR 0020 D3: handle_depth counter detects nesting; inner handles emit Kont*-typed merge values; switch default propagates un-caught op to outer's dispatch via the merge — complete.**
**Phase C3.7 — handle body lift + phase-go fixtures + ADR 0020 → ACCEPTED-WITH-AMENDMENTS — complete. Phase C3 closes.**
**Phase C4.0 — lexer keywords for classes / traits / delegation / structured concurrency per ADR 0021 D11 — complete.**
**Phase C4.1 (1/N) — class AST + parser per ADR 0022 D1-D4 — complete.**
**Phase C4.1 (2/N) — resolve / types / codegen wiring + postfix `.method(args)` + `Name::init(args)` per ADR 0022 D3+D5+D7+D9 — complete. Definite-assignment via flat any-assigned check (branch-aware merge deferred); ADR 0022 D11 phase-go (Point with manhattan/translate) runs end-to-end at exit 42.**
**Phase C4.1 close — ADR 0022 → ACCEPTED-WITH-AMENDMENTS — complete. Phase C4.1 closes.** Two amendments: A1 D4 definite-assignment is partial (flat any-assigned, branch-aware merge + InitFieldReadBeforeAssign deferred); A2 D8 general `Self` in type position deferred (only positional `self: &Self` via parse_self_param).
**ADR 0023 PROPOSED — concrete C4.2 trait + impl surface — docs-only.** Twelve D-decisions covering trait declarations (D1+D2), default + named impl block grammar (D3+D4), three dispatch paths (D5: receiver-typed + qualified-named + bounded-generic), method-call resolution (D6), `Self` resolution via `Type::TraitSelf(TraitId)` (D7), typing pipeline (D8: two new resolve passes + one new types pass), per-impl codegen with witness tables (D9), out-of-scope list (D10), lexer state recap (D11), and the c42_go_no_go phase-go (D12). Bounded-generic + named-impl pairing deferred at C4.2 minimum per D5/D10 amendment — defaults only for Path 3.
**Phase C4.2 (1/N) — trait + impl AST + parser per ADR 0023 D1+D3+D4 — complete.** Trait declarations + default-and-named impl blocks + `ImplName::method(args)` qualified calls parse end-to-end at AST + parser. Downstream resolve rejects with TraitDeclNotYet / ImplDeclNotYet / QualifiedCallNotYet diagnostics until C4.2 (2/N) lands the impl table + dispatch + codegen. +14 tests (1138 total).
**Phase C4.2 (2/N) — resolve / types / codegen wiring + Path 1 + Path 2 dispatch per ADR 0023 D5+D6+D8+D9 — complete. ADR 0023 → ACCEPTED-WITH-AMENDMENTS.** Trait + impl declarations flow through the full pipeline. Receiver-typed dispatch (`s.write(10)` → default impl when class has no class-method `write`) and qualified-named dispatch (`Doubling::write(&mut s, 16)` → named impl) ship end-to-end. Three amendments at C4.2 close: A1 D5 Path 3 (bounded-generic dispatch) DEFERRED — needs `<W: Writer>` bounded-generic surface; A2 D9 witness-table values not emitted (scaffolding for Path 3); A3 D7 `Type::TraitSelf(TraitId)` interner SHIPPED but unused (params/returns don't reference Self, only positional via self_kind — mirrors C4.1 A2). +16 tests (1154 total). ADR 0023 D12 phase-go (FileSink with default + named Doubling Writer impls) runs at exit 42.
**Phase C4.3 — delegation auto-forwarders per ADR 0021 D6 — complete.** `delegate field: T to Trait;` inside a class body synthesizes a default impl of `Trait` for the class that auto-forwards every trait method to `self.field.method(args)`. Synthesis happens entirely at the resolve layer — Pass 0e extension registers delegate impls alongside user impls (coherence collision = DuplicateDefaultImpl); Pass 3 (resolve_class_decl) adds the synthesized field; new Pass 4.5 synthesizes the per-trait-method auto-forwarder bodies. Types + codegen unchanged. No detail ADR — lands under ADR 0021 D6 + C4.0 D11 lexer reservation. +14 tests (1168 total). c43_go_no_go phase-go (Logger delegates Writer to FileSink; `l.write(42)` returns 42) runs at exit 42.
**ADR 0024 PROPOSED — C4.4 structured concurrency surface + Async effect + runtime scheduler — docs-only.** Twelve D-decisions covering scope/spawn/await grammar (D1-D3), Type::Task interner (D4), Async built-in effect (D5), thread-per-spawn scheduler (D6), 5 new runtime symbols (D7), lowering (D8 + D9), out-of-scope (D10), lexer recap (D11), c44_go_no_go phase-go (D12), 2-sub-iteration split (D13). Key amendment to ADR 0021 D9: direct runtime API rather than async-as-effect (multi-shot continuations from ADR 0020 D2 would be required). User surface identical to the async-as-effect vision.
**Phase C4.4 (1/N) — scope / spawn / await AST + parser per ADR 0024 D1+D2+D3 — complete.** Structured-concurrency surface parses end-to-end at AST + parser. Downstream resolve rejects with ScopeNotYet / SpawnNotYet / AwaitNotYet until C4.4 (2/N) lands the typing + runtime + codegen. +9 tests (1177 total). ADR 0024 stays PROPOSED.
**Phase C4.4 (2/N runtime) — sentinel-runtime symbols per ADR 0024 D6+D7 — complete.** Thread-per-spawn substrate ships: sentinel_task_spawn / sentinel_task_await / sentinel_scope_enter / sentinel_scope_exit / sentinel_scope_register + SentinelTask (32-byte C-stable struct) + SentinelScopeCtx. Uses std::thread internally; real work-stealing scheduler is a deferred follow-on per ADR 0024 D6. Cancellation on early scope exit DEFERRED per ADR 0024 D9. +6 runtime tests (1182 total active workspace). ADR 0024 stays PROPOSED. The typing layer + codegen wiring + c44_go_no_go phase-go land in a follow-on iteration.
**Phase C4.4 (2/N) — types + codegen + phase-go per ADR 0024 D4+D5+D8 — complete. ADR 0024 → ACCEPTED-WITH-AMENDMENTS. Phase C4.4 + Phase C4 close.** The `scope concurrent { spawn fn(args); expr.await }` surface compiles + runs end-to-end. Types: `Type::Task(TaskId)` (tenth interner variant) + `TaskData` + `intern_task` + `TypedProgram.tasks` threaded through check_expr; `TypedExprKind::Scope/Spawn/Await`; spawn validates a Call target returning i64 (Task<i64>-only per D7), await requires a `Type::Task` receiver; 3 TypeErrors (SpawnMustBeCall / SpawnResultMustBeI64 / AwaitOnNonTask). Resolve: scope/spawn/await pass-through (NotYet dropped); built-in `Async` effect auto-registered (appended after user effects — deviation from D5's "EffectId(0)"). Effect-check: spawn/await contribute Async; `scope concurrent` discharges Async (handler-style); spawn/await outside a scope bubbles Async to main → rejected (D5 discipline SHIPPED). Codegen: 5 runtime externs + per-spawn-target wrapper synthesized in a compile_to_object pre-walk (before CodegenCtx — it lacks `&Module`); lower scope/spawn/await; Async-only fns keep the value ABI not the C3 Kont* ABI (`uses_kont_abi` excludes Async). Runtime fix: `_pad` → `owned` flag so explicit `.await` inside a scope is safe against the scope's exit-time auto-await (closes a UAF/double-free in the C4.4 2/N symbols). +1 pass fixture (c44_go_no_go exit 42) + 3 UI fixtures + 2 effect-check tests + 1 runtime test (~1188 total active workspace). Amendments A1 (work-stealing deferred), A2 (cancellation deferred), A3 (Task<i64> + i64 args only), A4 (Async discipline shipped, 2 deviations), A5 (explicit `Task<T>` annotations deferred — use inference). Four-check suite green.
**Phase C4.5 — close-out per ADR 0021 D13+D14 — complete. ADR 0021 → ACCEPTED-WITH-AMENDMENTS. Phase C4 closes.** Combined full-surface phase-go `tests/pass/c4_go_no_go.sentinel` (class + `&mut Self`/`&Self` methods + init + trait + impl + delegation + scope/spawn/await in one program; exit 42) + `tests/pass/c4_named_impl.sentinel` (two named impls of one (trait,type) co-existing via qualified calls; exit 42). The D13 phase-go's `spawn lb.write(42)` (a method call) was adapted to spawn a free fn `buffered_write` that drives the class/delegation surface on the worker thread, since ADR 0024 D2 restricts spawn to a direct fn call (ADR 0021 amendment A2). ADR 0021 amendments: A1 (D9 async-as-effect superseded by ADR 0024's direct-runtime API — surface identical, lowering differs), A2 (D13 phase-go adapted), A3 (per-sub-phase amendments roll-up), A4 (D14 estimate beaten), D10/D12 out-of-scope confirmed (actors → C5). +2 driver pass-tests (123 driver pass; ~1191 total active workspace). Docs-only sub-phase. Four-check suite green. **Next: Phase C5 → Sentinel 1.0** (broker integration + constant-time secret codegen + cross-process + actors + stable ABI + reproducible builds + tooling per HANDOVER §6.2). **ADR 0025 PROPOSED drafted** (Phase C5 kickoff / productionization plan — 14 D-decisions, 8-sub-phase split; resume at C5.0).
**Phase C5.0 — go/no-go decision + test infra (D11) + reproducible-build audit (D8) — complete.** Per ADR 0025: the 1.0 go/no-go program is a single-process, single-file TLS 1.3 handshake (D1/D13), resolving D6 (cross-process) + D9 (modules) both → post-1.0. D11: `cargo nextest` adopted (`.config/nextest.toml`) + the 15 driver UI rejections migrated from `stderr.contains(code)` to `insta` blessed full-diagnostic snapshots (`crates/sentinel-driver/tests/ui.rs`, portable via relative-path snc invocation). D8: the reproducible-build audit found the C0–C4 build already byte-identical across independent `snc` processes (codegen's std `HashMap`s are lookup-only; emission walks source-ordered `Vec`s; mach-O has no timestamp), locked in by `crates/sentinel-driver/tests/repro.rs` (compile-twice + diff). 3 commits (`3908cf6` decision docs + `a217707` D11 feat + `5fe7fd3` D8 feat); four-check green via `cargo nextest run --workspace` (1195 tests) + `cargo test --doc`.
**ADR 0026 PROPOSED — C5.1/C5.2 HIR/MIR pipeline + constant-time secret codegen — docs-only.** Ten D-decisions: HIR desugar stage (`hir_query` — dispatch-resolved + monomorphic + drops-explicit + secret-preserved; D1), minimal SSA MIR (`mir_query`; D2), codegen re-targets `TypedProgram`→HIR with MIR as the analysis substrate + a documented escape hatch if the re-target over-runs (D3), constant-time secret emission (branch-free select + ADR 0008 speculation barriers; x86-64/aarch64; D4), the MIR constant-time verification pass (taint-track secrets in SSA, `sentinel::mir::secret_leak` diagnostic; D5), secret taint representation (D7), out-of-scope (D8: full opt suite, codegen-consumes-MIR SSA lowering, oblivious secret indexing), phase-go (D9: `c51` behaviour-preservation across the whole pass suite + `c52_secret_ct` + `c52_secret_leak`), 4-sub-phase split C5.1a→C5.2b (D10).
**Phase C5.1a (1/N) — HIR pipeline seam introduced; ADR 0026 D3 escape hatch INVOKED — complete.** `sentinel-hir` is now a real stage: a pure `lower_to_hir(&TypedProgram, &DropPlan) -> HirProgram` the driver calls after borrow-check, with `compile_to_object` consuming `&HirProgram` (a thin borrowing bundle of the typed program + drop plan at this increment). Behaviour-preserving by construction — all 1195 tests pass + every `tests/repro.rs` object byte-identical (`cdbc483`). The **D3 escape hatch was then INVOKED** (decided with the developer): codegen couples to the typed tree at ~295 `TypedExprKind` / 342 `TypedExpr` refs across 90 signatures, so a *thick*-HIR migration is a multi-session high-risk rewrite not required for the 1.0 constant-time-`secret` capability. Codegen STAYS on the typed program (via the seam, `HirProgram::program()`); the thick HIR desugar (dispatch/mono/explicit-drops) + the codegen-consumes-HIR migration are **post-1.0** (still Phase-D-valuable); **C5.1a closes at the seam**. Next: **C5.1b** — `sentinel-mir` + `mir_query`, an SSA/CFG lowered from the typed program (via the seam) for the C5.2 D5 constant-time verification; then C5.2 constant-time emission (D4, a codegen pass) + verification (D5).
**Phase C5.1b (1/N) — MIR data model (minimal SSA/CFG) — complete.** `sentinel-mir` is no longer a stub: `MirProgram` / `MirFunction` / `MirBlock` (SSA block-params = the phi-equivalent) / `MirInst` / `MirOp` / `MirTerminator` / `MirValue`, built to host the C5.2 D5 constant-time verification. Each SSA value carries its `Type`, so secrecy reads off `Type::Secret(_)` (`MirFunction::is_secret` — the taint seed); the three D5 sinks are representable (a `Branch` condition, a `Load` index, a `Binary` Div/Rem operand); non-secret-relevant constructs funnel through `MirOp::Opaque` / `MirTerminator::Unreachable` carrying their operands so taint stays sound. Additive — nothing consumes MIR yet (codegen stays on the typed program per the escape hatch); zero regression risk; 1195 tests green (`1b0a10d`). Next: **C5.1b (2/N)** — `lower_to_mir` (typed fn bodies → MIR SSA); then C5.2 = D5 verification + D4 constant-time emission (codegen pass).
**Phase C5.1b (2/N) — `lower_to_mir`: typed fn bodies → MIR SSA — complete.** `sentinel-mir` now lowers each free function's type-checked body into a `MirFunction` in SSA/CFG form. No loops in the surface ⇒ the CFG is a DAG and SSA falls out of one structured walk (no dominance-frontier phi placement): `if`/`&&`/`||` → `MirTerminator::Branch` into fresh blocks reconciled at a merge block via SSA block-params, with a variable reassigned on one arm threaded through a merge param (deterministic, `VarId`-sorted `BTreeMap` env). `&&`/`||` lower as control flow because `secret bool && secret bool` type-checks (`SecretBranch` only rejects `if`) — a short-circuit branch on a secret is the leak the C5.2 D5 pass must see. The three D5 sinks lower precisely (`if`/short-circuit cond → `Branch`; `a[i]`/`*p` → `MirOp::Load` so a secret index *or* address is visible; `a / b` → `Binary(Div)`); `declassify(e)` → `MirOp::Declassify` (the one taint sink); everything else → `MirOp::Opaque` carrying its operands so taint can't vanish. Scope: top-level fns only (class/impl/init method bodies a mechanical follow-on); generic defs as-is (`TypeParam` is never secret); no monomorphisation (MIR is analysis-only per the D3 escape hatch). Additive — nothing consumes MIR yet (the D5 pass at C5.2 is its first consumer; the driver will call `lower_to_mir(hir.program())` then); zero regression risk; codegen stays on the typed program. +7 lowering tests (1202 total) (`1a223c8`). ADR 0026 stays PROPOSED (flips at C5.2 close). Next: **C5.2** — D5 constant-time verification (`sentinel::mir::secret_leak`) + D4 constant-time emission (codegen pass) — the 1.0 headline.
**Phase C5.2b (1/N) — the D5 constant-time verification pass — complete.** `verify_constant_time(&MirProgram) -> Vec<SecretLeak>` is the first consumer of `lower_to_mir` and the machine-checkable form of ADR 0008's guarantee: it rejects any `secret` value reaching a conditional-branch condition, a load index/address, or a division divisor, emitting a `sentinel::mir::secret_leak` what/why/how diagnostic (`SinkKind` names the sink). **Taint oracle:** each SSA value carries its `Type`, and the type checker's operator-secret-preserving rules already computed the taint fixpoint (`declassify` clears; fn-signature boundaries respected), so the pass reads taint off the type (`is_secret`) and inspects each sink — no separate def-use propagation at the typed-program level (the ADR's forward propagation is only needed once MIR is lowered from *post-optimisation* code → post-1.0; recorded as a **D5 amendment**). The one leak this catches beyond the C3.1 source rejections is `secret bool && secret bool` (a secret short-circuit `Branch`), which type-checks because `SecretBranch` only rejects `if`. MIR data model gains a `span` on `MirInst` + `MirTerminator::Branch`, threaded in `lower_to_mir`, so the diagnostic points at source. Additive — not yet wired into the driver (C5.2b (2/N)); zero regression risk. +4 verify tests (1206 total) (`9bcc271`). ADR 0026 stays PROPOSED. Next: **C5.2b (2/N)** — wire D5 into the driver (a real `secret_leak` compile error) + `c52_secret_leak` (UI snapshot) + `c52_secret_ct` (branch-free masked-select pass) fixtures.
**Phase C5.2b (2/N) — the D5 verification is wired into `snc` + c52 phase-go — complete.** `snc build` runs the constant-time check: after `check_query` the driver lowers the typed program to MIR (`lower_to_mir` — now a real pipeline consumer) and runs `verify_constant_time`; a `secret` reaching a conditional branch / load index|address / division divisor is a `sentinel::mir::secret_leak` compile error (exit 1) gating codegen. (Codegen still consumes the typed program via the HIR seam per the D3 escape hatch; MIR stays analysis-only.) Fixtures (ADR 0026 D9): `c52_secret_leak` (UI) — `secret bool && secret bool` type-checks (`SecretBranch` only rejects `if`) but lowers to a secret short-circuit `Branch` → rejected (`insta` snapshot, label on the short-circuited operand); `c52_secret_ct` (pass) — a branch-free masked select over secrets (`c*a + (1-c)*b`) compiles, runs, **passes** D5, exit 42. **c51 bar holds**: existing pass/ui fixtures unchanged + `tests/repro.rs` byte-identical (D5 runs before, and gates, an unchanged codegen). +2 tests (1208 total) (`e81bdbf`). ADR 0026 stays PROPOSED. Next: **C5.2a** — D4 constant-time *emission* (codegen pass: branch-free select + ADR 0008 speculation barriers, x86-64/aarch64); **open question: does the 1.0 go/no-go even need D4?** A branch-free *arithmetic* primitive already passes D5 on the existing codegen (no bitwise/`select` ops in the surface), so D4 may be scoped out of 1.0 — settle with the developer before building it. ADR 0026 flips once C5.2a lands or is consciously scoped out.
**ADR 0027 PROPOSED — bitwise operators (`& | ^`, then `<< >> ~`) — docs-only.** Decision (with the developer): do bitwise operators next, **deferring C5.2a/D4** — the go/no-go's constant-time `Finished` MAC verify is an XOR-accumulate compare that needs `^`/`|`, and the surface has none (`BinOp` = only Add/Sub/Mul/Div). Ten D-decisions: target `& | ^ << >> ~` in two waves (D1) — **C5.3 = `& | ^`** (token-clean: new `Pipe`/`Caret`, reuse `Amp` as infix bit-and with prefix `&` still borrow — D2; Rust precedence `&`>`^`>`|` between cmp and add, ladder gains parse_bitor/bitxor/bitand — D3); extend `BinOp` (no new ExprKind/TypedExprKind/MirOp variants — D4); secret-preserving integer-only typing mirroring C3.1b arithmetic, **no new SecretXxx rejection** (bitwise is constant-time, the *sanctioned* secret computation — D5); LLVM and/or/xor codegen (D6); **MIR + D5 need no change** (the `Binary` arm is op-generic; bitwise ops are non-sinks — D7); **C5.4 = `<< >> ~`** with the `>>`-vs-nested-generic-close split (Rust-style: the type-arg parser splits `Shr` into two `>` — D9). Out of scope (D8): shifts/complement at C5.3, rotate, bitwise-on-bool, compound-assign, `[secret T]` arrays (the flat ArrayElem subset has no Secret variant — a separate deferred surface). Phase-go (D10): `c53_bitwise` + `c53_ct_eq` (a real XOR-accumulate constant-time equality over scalar secrets that passes D5 — upgrading the C5.2b faked masked-select). Next: **C5.3 (1/N)** — lexer (`Pipe` + `Caret` tokens).
**Phase C5.3 (1/N) — lexer: bitwise `|` (`Pipe`) + `^` (`Caret`) tokens — complete.** First wave of the bitwise surface per ADR 0027 D2: two new logos tokens; longest-match keeps `||` → `PipePipe`; the infix bitwise-and **reuses** `&` (`Amp`), to be disambiguated from the borrow prefix by parser position at 2/N. No `<<`/`>>` (C5.4 — the `>>`/nested-generic-close split). +4 lexer tests (longest-match `|`/`||` + `&`/`&&` regressions + packed bitwise); additive (parser doesn't consume them yet); four-check green (1212) (`b3d1d48`). Next: **C5.3 (2/N)** — `& | ^` end-to-end: extend `BinOp` (Display + all exhaustive matches in one pass); parser precedence ladder gains `parse_bitor`/`parse_bitxor`/`parse_bitand` between `parse_cmp` and `parse_add`; secret-preserving integer typing mirroring C3.1b (no new TypeError/SecretXxx); codegen and/or/xor; MIR+D5 unchanged (Binary arm op-generic, bitwise non-sink) + `c53_bitwise`/`c53_ct_eq` fixtures; ADR 0027 flip.
**Phase C5.3 (2/N) — bitwise `& | ^` surface end-to-end. ADR 0027 → ACCEPTED-WITH-AMENDMENTS.** The operators compile + run. Surface was small because the `Binary` pipeline is op-generic (resolve passes `BinOp` through; types' Binary handler op-agnostic except the `Div`→`SecretDivisor` check; `lower_to_mir`/D5 handle `Binary` generically) — so only AST + parser + codegen changed. AST: `BinOp` += `BitAnd`/`BitOr`/`BitXor` (+ symbol(); no new ExprKind/TypedExprKind/MirOp). Parser: levels `parse_bitor`→`parse_bitxor`→`parse_bitand` between cmp and add (`&`>`^`>`|`); infix `&` = bit-and, prefix `&` = borrow (positional). Types: NO change — inherits C3.1b secret-preserving integer rule (mixed secret/public → Mismatch; bool → Mismatch); **no new SecretXxx** (bitwise is constant-time, the sanctioned secret computation). Codegen: LLVM and/or/xor. MIR+D5: unchanged (bitwise non-sink). Fixtures: `c53_bitwise` (`5 & 6 ^ 3 | 8`==15) + `c53_ct_eq` (constant-time equality over secrets — XOR-accumulate+OR-reduce+declassify — compiles, runs, passes D5; the go/no-go MAC-verify shape). +9 tests (1221 total) (`76bfea3`). **ADR 0027 amendment A1:** `<< >> ~` (C5.4) deferred — the constant-time compare needs only `^`/`|`; shifts (with the `>>`/generic-close split) are a follow-on if the go/no-go computes hashes in-language. Next: **developer-scope call** — C5.4 shifts, OR begin assembling the TLS go/no-go (constant-time compare now writable), OR another C5 productionization sub-phase.
**ADR 0028 PROPOSED — broker integration (D4) — docs-only.** Developer chose broker integration as the next productionization sub-phase (**C5.4**). Finding that shaped it: the Phase A broker is an *arena* allocator (bump = bulk-free / `free` unimplemented; slab = fixed-size slots; typed `Handle<T>`) that does **not** fit a drop-in `sentinel_alloc` (arbitrary-size, individual-free, raw `*u8`); and there's no secret heap data yet (`[secret T]` unrepresentable) so the secret-memory policy is scaffold. ADR 0028's design (10 D-decisions): map Sentinel scopes → broker bump arenas so individual free becomes scope-exit **bulk** free (D1/D2), reusing the borrow-check **`DropPlan`** (moved=escapes vs dropped-at-scope-exit=safe-to-arena) so **no new escape analysis** is needed (D3); ship a **runtime-only foundation first** (C5.4 (1/N), D4): process-wide `Broker` backing `sentinel_alloc`/`free` via a size-classed slab pool + ptr→handle registry — **c51-safe because codegen is untouched** (objects byte-identical) — unlocking budgets/recording/stats; then the **scope→arena codegen** (C5.4 (2/N), D3, may defer post-1.0). Budgets = the go/no-go hook (`within_budget`; a `scope budget(N)` surface deferred — D5). Secret-policy scaffold (D6). Numbering: ADR 0025 D14's "0027 = broker" superseded (0027 = bitwise; broker = **0028**); the bitwise *shift* wave (ADR 0027 A1) is unnumbered-deferred, not C5.4. Next: **C5.4 (1/N)** — the runtime-only broker foundation.
**Phase C5.4 (1/N) — broker-arena substrate (ADR 0028) — complete.** The Phase A broker backs a scope-arena C-ABI in the runtime. Finding: the broker is a safe *handle* allocator (bump bulk-frees, `free` unimplemented; slab fixed-size) with no public raw pointer, so a drop-in `sentinel_alloc` doesn't fit — added a public raw-bytes API (`Arena::alloc_bytes`/`ArenaHandle::alloc_bytes` → `NonNull<u8>`, exposing the strategy's `alloc_raw`). Runtime: a process-wide lazy `Broker` + `sentinel_arena_enter` (create bump arena) / `sentinel_arena_alloc` (16-byte-aligned bump alloc) / `sentinel_arena_exit` (`destroy_arena` + drop → `BumpStrategy::drop` frees the backing buffer). **Additive + c51-safe**: codegen still emits `sentinel_alloc`/`sentinel_free` (libc) and does NOT call the arena fns yet → objects byte-identical. (The ADR's "runtime-only" was slightly off — a small broker API addition was needed — and the malloc-replacement framing was set aside: the broker is arena-, not malloc-, shaped.) +5 tests (1226 total) (`b49a5ef`). ADR 0028 stays PROPOSED. Next: **C5.4 (2/N)** — the scope→arena codegen: route a scope's non-escaping heap allocations (those the borrow-check `DropPlan` frees at that scope exit, hence provably non-escaping) into the scope arena, replacing N per-binding `sentinel_free`s with one `sentinel_arena_exit`. The careful (UAF-sensitive) part — do it fresh; full escape analysis stays post-1.0 (ADR 0026 D2).
**Phase C5.4 (2/N) — the scope→arena codegen; ADR 0028 → ACCEPTED-WITH-AMENDMENTS. Phase C5.4 closes.** Codegen routes a scope's **non-escaping** primitive array-literal heap buffers into a broker bump arena (`sentinel_arena_enter`/`_alloc`) and replaces that scope's per-binding `sentinel_free`s with **one** `sentinel_arena_exit` at scope exit. A program-wide `compute_arena_routed` pre-pass produces a `HashSet<VarId>` = *exactly* the bindings `emit_scope_drops` frees (`∉ moved ∧ ≠ tail_returned_var(&block.tail)`), restricted to `let x = [i64/i32/bool array literal]` in **non-generic, non-effecting fns**; that **one set drives both** the alloc-routing (`lower_stmt`→`lower_array_lit`) and the free-skip (`emit_scope_drops`), so they cannot diverge. The per-scope arena handle lives in a new `ScopeFrame` (replacing the bare `Vec<VarId>`; a `push` method keeps the 12 push sites unchanged), created **lazily** on first routed alloc → scopes routing nothing stay byte-identical. **Airtight argument:** the routed set is a strict *subset* of the proven-non-escaping free set, so routing is as safe as today's free (same bindings, lifetime, point; bulk vs individual). **Verified by reasoning + disassembly** (routed scopes emit `arena_enter`/`_alloc`/`_exit` + **zero** `sentinel_free`/`_alloc`; moved/returned arrays stay on libc — a single negative case `c24_moved_array_no_double_free` confirms) + the c24/c25 array-RAII guards. **Amendment A2 — ADR 0028's "verified UAF hole" was wrong about the mechanism:** a tail-returned array (`fn make() -> [i64] { let a=[1,2,3]; a }`) **IS** in `moved_sources` (the borrow checker walks the tail `Var` as a *consuming move* before snapshotting the `DropPlan`; empirically dumped), so `∉ moved` alone already excludes returned arrays — the `tail_returned` half is belt-and-suspenders for heap types, kept anyway to mirror `emit_scope_drops` exactly. +1 fixture (`c54_scope_arena` — body-scope + nested-block arenas, exit 42); 1227 tests (`8e7b38f`). Four-check green. Deferred (post-1.0): per-scope arena *sizing* (capacity 0 → runtime 1 MiB default), routing in methods/generics/effecting fns, non-primitive-element arrays, `scope budget(N)` surface, full escape analysis (ADR 0026 D2). **Next: developer-scope call** — assemble the TLS go/no-go (constant-time compare + scope arenas now both in hand), or another C5 productionization sub-phase (stable ABI ADR 0025 D7 / LSP D10).
**ADR 0029 PROPOSED — stable ABI (D7) — docs-only.** Recommended + drafted as the next C5 sub-phase (no codegen hazard; a prerequisite for the go/no-go's runtime↔codegen link + Phase D self-hosting). Ten D-decisions: define + **document** (`docs/abi-v1.md`) + **freeze at `abi-v1`** + **test** the ABI — calling convention (D3: C ABI / SysV+AAPCS64; main→i32; ordinary fns by value; effecting fns→`*SentinelKont`; class init→out_ptr), the `Type`→LLVM **layout catalog** (D4: structs field-order, `[T]`=`{i64,ptr}`, `?primitive`=`{i1,T}`, `?Struct`=`{i1,ptr}`, ref/kont/task=opaque ptr, `secret T`≡`T`), **name mangling** (D5: `base__<tag>`, `arr_`/`opt_`/`ref_`/`sec_`, `Name__init`/`Name__method`/`Name__Type__Trait__method`), the **runtime-symbol contract** (D6: the ~18 `sentinel_*` + the `#[repr(C)]` `SentinelKont`/`Frame`/`Task`/`ScopeCtx` layouts), a **layout-stability test suite** (D7: extend the size/align asserts + DataLayout `Type`-layout asserts + mangling/symbol golden tests — the enforcement, drift→red test), versioning (D8: `abi-v1` freeze; reproducible builds folded in, already guarded by `repro.rs`; mangling length-prefixing is the one `abi-v2` soft-spot). **No emitted bytes change** → c51 bar holds by construction. Out of scope (D9): the separate-compilation linker/module surface (ADR 0025 D9, post-1.0), cross-arch beyond x86-64/aarch64, FFI header gen. Sub-phase split D7 (1/N) spec+struct/mangling/symbol tests, (2/N) `Type`-layout DataLayout asserts + flip. Next: **D7 (1/N)**.
**Phase C5 D7 (1/N) — the stable-ABI spec + layout-stability tests (ADR 0029) — complete.** `docs/abi-v1.md` documents + **freezes** the ABI codegen already emits (calling convention, the `Type`→LLVM layout catalog, the `#[repr(C)]` runtime struct layouts, name mangling, the ~18 `sentinel_*` runtime-symbol contract), each cross-linked to its bootstrap source. Stability tests pin it so a drift turns a test red rather than silently miscompiling: `abi_v1_struct_layouts_are_stable` (size/align + `offset_of!` for `SentinelKont`/`Frame`/`Task`/`ScopeCtx`) + `abi_v1_runtime_symbol_set` (addresses of all 18 runtime symbols → rename/removal is a compile error) in sentinel-runtime; `abi_v1_mangling_is_stable` (golden strings for `mangle_type`/`mangle_mono_name`) in sentinel-codegen. **No emitted bytes change** (documents/tests existing behaviour) → c51 bar + `repro.rs` hold by construction; reproducible builds (D8) fold in. +3 tests (1230) (`0304a9c`). **ADR 0029 stays PROPOSED** (flip is at 2/N). Next: **D7 (2/N)** — the `Type`-layout DataLayout assertions (query the lowered LLVM type's size/align/field-offsets through the target `DataLayout`, assert the `abi-v1` values) + a negative "drift turns it red" check + the ADR flip. (May then move to LSP D10 or the TLS go/no-go.)
**Phase C5 D7 (2/N) — `Type`-layout DataLayout assertions; ADR 0029 → ACCEPTED-WITH-AMENDMENTS. Phase C5 D7 (stable ABI) closes.** `abi_v1_type_layouts_via_datalayout` (sentinel-codegen) lowers each `Type` via the real `llvm_basic_type` and asserts its size / alignment / struct-field offsets **and field types** through the target `DataLayout` (same target setup as `compile_to_object`) — the concrete `abi-v1` §2 byte layouts (i1=1, i32=4, i64=8, `[T]`/`?T`=16 with the inner at offset 8, ptr=8). **Amendment A1:** field-type asserts were added beyond the ADR's "size+offsets" wording — equal-sized fields' *order* (e.g. `[T]`'s `{i64 len, ptr data}`) isn't pinned by offsets alone; the negative check (deliberately reordering the array fields) was verified to turn the test **red**, then reverted. **A2:** `Struct`/`Class`/`GenericInstance` layouts (which need codegen's pass-0 `struct_types` cache to lower a real `Type::Struct(id)`) are pinned via a representative `{i64,i64}` struct built as codegen builds user structs (`struct_type(fields, false)`); the cache-free arms (scalars, `[T]`, `?T`, ref/kont/task) go through `llvm_basic_type` directly. No emitted bytes change → c51 bar + `repro.rs` hold. +1 test (1231). Four-check green. **Next: developer-scope call** — LSP (ADR 0025 D10) or assemble the TLS 1.3 go/no-go (D13): both 1.0 headline capabilities (constant-time `secret` compare + broker scope arenas) **and** a frozen `abi-v1` are now in hand. Deferred (post-1.0, ADR 0029 D9): the separate-compilation linker/module surface, cross-arch beyond x86-64/aarch64, a length-prefixed mangling scheme (`abi-v2`).
**ADR 0030 PROPOSED — the 1.0 go/no-go: a TLS-1.3-handshake-shaped program (D13) — docs-only.** Opened after a **readiness/scoping pass** against the current surface, which found the go/no-go is an *assembly of already-proven patterns* (constant-time `Finished` verify = `c53_ct_eq`; state machine + trait + delegation = `c4_go_no_go`; I/O-as-effects = `c37_go_no_go`; bounded iteration = recursion, verified working) + deliberate modelling choices, **not** a dependency on big new machinery. Nine D-decisions: the close-bar goal (D1: single-process single-file handshake — accept → ECDHE → HKDF → `Finished` verify — that compiles + runs + **passes D5 constant-time verification**; closing it = **Sentinel 1.0**); reduced handshake-shaped crypto over `secret` scalars at fixed sizes (D2: a Montgomery-ladder *step*, an HKDF-`expand`-shaped fixed mix, the `c53_ct_eq` compare — not real AES/X25519/SHA-256, which are ecosystem per §15.3); **D3 — descope the connection actor** (a *deviation* from C5.0 D5: a sequential single-process handshake needs no mailbox → drops the largest remaining language-design sub-phase off the 1.0 path; actors → post-1.0; **developer's call**); modelling choices (D4: bytes/labels→`i64`, secret material→`secret` scalars, iteration→recursion — no new surface); **shifts `<< >> ~` as a conditional JIT prerequisite** (D5: land ADR 0027 A1 only *iff* a reduced primitive needs them; the chosen shapes use only `+ - * & | ^`); the constant-time bar (D6: must pass `verify_constant_time` — the decisive 1.0 validation); out of scope (D7: real crypto, `u8`/byte type + string literals, actors, modules, cross-process, loops); phase-go (D8: `c5_go_no_go` fixture green + D5-clean → flip ADR 0025 → ACCEPTED); 3-sub split (D9: skeleton → CT primitives → close). Gaps found but non-blocking for the reduced program: no loops (recursion substitutes), no bytes/strings (model as `i64`), no shifts/`%` (reduced primitives avoid; `%` isn't even lexed), no `[secret T]` arrays (secret scalars). Next: **go/no-go (1/N)** — the skeleton (state machine + cipher trait + `Net`/error effects + 4-stage flow, stubbed crypto; compiles + runs).
**Phase C5 go/no-go (1/N) — the TLS-handshake-shaped skeleton (ADR 0030) — complete.** `tests/pass/c5_go_no_go.sentinel` composes the full surface the go/no-go needs in one single-file program — a handshake state-machine `class` (`&mut Self` `ecdhe` method + init) + a `Kdf` cipher-suite `trait`/`impl` (receiver-typed `suite.derive` dispatch) + a `Net` I/O `effect` + `handle … with` + the 4-stage flow (accept/recv → ECDHE → HKDF → `Finished`) — with **stubbed** crypto, and runs end-to-end to **exit 42** (handler resumes recv→5; 5*9=45; 45+3=48; finished_diff(48,48)=0; 42-0=42). **It compiled on the first try** — empirical confirmation of ADR 0030's scoping verdict (the go/no-go is an assembly of proven patterns, not new machinery). +1 test (1232) (`9e2ef6a`). **ADR 0030 stays PROPOSED.** Next: **go/no-go (2/N)** — fill the constant-time primitives over `secret` scalars (a Montgomery-ladder step + cswap, an HKDF-`expand`-shaped fixed mix, the `c53_ct_eq` `Finished` verify) and make the program **pass the D5 constant-time check** (`verify_constant_time`) — the decisive 1.0 validation; land ADR 0027 A1 (shifts) first *iff* a primitive needs it. Then (3/N): close → **declare Sentinel 1.0** + flip ADR 0025 → ACCEPTED.
**Phase C5 go/no-go (2/N) — constant-time crypto over secrets; the close bar is MET (ADR 0030 D8) — complete.** `tests/pass/c5_go_no_go.sentinel` now does real **constant-time** crypto over `secret` scalars: a Montgomery-ladder step + branch-free `cswap` (`mask = sec(0) - bit`), an HKDF-`expand`-shaped mix via the `Kdf` trait, and the `c53_ct_eq` `Finished` verify (XOR-accumulate + `declassify`). It **passes the D5 constant-time check** (`verify_constant_time` gates `snc build`) and runs to exit 42 — the headline 1.0 capability (express + *prove* constant-time crypto) exercised end-to-end. Constant-time by construction: every secret op is `+ - * ^ & |` (no D5 sink), the lone `declassify` is the `Finished` accumulator, no secret reaches a branch/index/divisor (verified the secret typing is live — a deliberate secret array index is rejected at type-check). **Ergonomic finding:** C3.1b makes a mixed secret/public op a type error (no in-expression widening), so constant-time code lifts public constants/labels into the secret domain first — here via a `sec(x) { let s: secret i64 = x; s }` helper (widening happens only at a `let` with a `secret` annotation, not at a return). 1232 tests (`e5a40e9`). **ADR 0030 stays PROPOSED.** Next: **go/no-go (3/N) — the formal 1.0 declaration** (flip ADR 0025 → ACCEPTED + declare Sentinel 1.0). **That milestone call is intentionally left to the developer**; the substantive close bar (program runs + passes D5) is met.
**🎉 SENTINEL 1.0 (2026-05-30) — go/no-go (3/N); Phase C5 + Phase C close.** The developer declared 1.0: **ADR 0025 (Phase C5 kickoff) + ADR 0030 (go/no-go) → ACCEPTED-WITH-AMENDMENTS.** The close bar was met (the constant-time TLS-handshake go/no-go runs + passes D5). The 1.0 language = full types + witness-table generics + borrow check + RAII + `secret`/effect typing + handler runtime + classes/traits/delegation + structured concurrency + **machine-verified constant-time `secret`** + bitwise `& | ^` + broker scope arenas + a frozen `abi-v1`; single-process, single-file, loop-free-by-design. 1232 tests, four-check green. Scoped out of 1.0 (analysed follow-ons): constant-time *emission* (ADR 0026 D4), shifts `<< >> ~` (ADR 0027 A1), actors (ADR 0030 D3), LSP (ADR 0025 D10), `[secret T]` arrays, modules, cross-process, a `u8` type, loops, full escape analysis. **Next: Phase D — self-hosting (ADR 0031 PROPOSED).** ⚠ Self-hosting is a *major* multi-stage effort: the 1.0 language has no strings / file I/O / growable collections / modules, all of which a compiler-in-Sentinel needs, so **Phase D opens with a language + stdlib build-out, NOT lexer-in-Sentinel** — see ADR 0031's honest readiness assessment + staged path.
**ADR 0031 PROPOSED — Phase D kickoff: self-hosting — docs-only.** Opens the project's largest phase. **Honest readiness verdict:** the 1.0 language *cannot* self-host yet — verified gaps (none at 1.0): no sum types / `match` (an AST is a sum type — the biggest blocker), no strings / `char` / byte type (a compiler is text processing), no growable collections (`Vec`/`Map` — only fixed `[T]`), no file I/O (only `sentinel_print`), no modules/multi-file, no loops (recursion-only). **Strategy (D2):** language+stdlib build-out FIRST, then incremental self-host, keeping the Rust `snc` as the **reference oracle** (every Sentinel-written stage differentially validated against it on the fixture corpus), converging on a **bootstrap fixed-point** (the Sentinel compiler compiles itself byte-identically — which is *why* C5 shipped `abi-v1` + reproducible builds). **Prerequisite roadmap (D4, each its own ADR):** sum types + `match` → strings + byte type → growable collections → file I/O (stdlib) → modules → loops; also retires the thick-HIR/MIR migration + full escape analysis + shifts. **Self-host sequence (D5):** lexer → parser → resolve → types → HIR/MIR → codegen, in Sentinel, each matching the Rust stage before replacing it. **First sub-phase D.1 = sum types + pattern matching** (its own PROPOSED ADR next — the foundational AST-enabler; a C1–C4-style type-system + codegen feature). No timeline promise; Phase D is plausibly the longest phase. Next: **Phase D.1 (sum types + `match`)** — write its kickoff ADR, then implement.
**ADR 0032 PROPOSED + Phase D.1 (1/N) — sum types + pattern matching: the kickoff + lexer.** ADR 0032 designs `enum`/`match` end-to-end (12 D-decisions): surface (`enum Name { V, V(T), V(T,T) }` + `Name::Variant(args)` + exhaustive `match s { Pat => e }`); `Type::Enum` interner variant; the abi-v1 layout **`{ i32 tag, ptr payload }`** heap-boxed (necessary for *recursive* enums — the AST case — reusing the `?Struct` rationale + drop path); `match` → LLVM `switch`; RAII payload drop; the constant-time guard (a secret-tagged match is a branch sink; no `secret enum` at MVP); generic enums (`Option`/`Result`) a fast-follow via mono (D9); MVP out-of-scope (named-field variants, or-/nested patterns, guards). **D.1 (1/N) ships the lexer:** two new logos keyword tokens (`enum`, `match`); `=>`/`::`/`_`-as-`Ident` already exist, so the lexer surface is complete. Additive (parser consumes at 2/N). +3 lexer tests (1235) (`87e955c`). Four-check green. Next: **D.1 (2/N)** — AST + parser (`EnumDecl`, `ExprKind::Match`, `Pattern`; `parse_enum_decl`/`parse_match`).
**Phase D.1 (2/N) — enum + match AST + parser; resolve rejects (NotYet) — complete.** Per ADR 0032 D8. **AST** (sentinel-ast): `EnumDecl` + `VariantDecl` (unit + positional-tuple-payload variants) on `Program.enums`; `ExprKind::Match { scrutinee, arms }` + `MatchArm` + `Pattern` (qualified `Enum::Variant(binds)` + `_` wildcard); the s-expr `Display` covers both. **Parser** (sentinel-syntax): top-level `enum` dispatch + `parse_enum_decl`/`parse_variant`; `match` dispatched in `parse_expr` alongside `if`, with `parse_match_expr`/`parse_match_arm`/`parse_pattern`/`parse_pattern_binding` (scrutinee forbids struct literals like the if-cond; arms comma-separated; `=>`/`::`/`_` reuse existing tokens). **Additive**: resolve rejects non-empty `enums` (`EnumDeclNotYet`) + `match` (`MatchNotYet`) until 3/N; **blast radius contained to ast+syntax+resolve** (downstream crates match the resolved/typed parallel trees, which gain no `Match` variant — confirmed by build). +7 tests (1242) (`e368a72`). Four-check green. Next: **D.1 (3/N) — resolve + types**: the `Type::Enum` interner variant (+ `EnumData`/`VariantData`), variant construction + `match` type-check + **exhaustiveness** (NonExhaustiveMatch / UnknownVariant); then (4/N) codegen (`{tag,ptr}` layout + `switch` + drop + abi-v1 entry + `c5d1_enum`), (D.1b) generic enums.
**Phase D.1 (3/N) — enum + match the type layer; `enum`/`match` now TYPE-CHECK end to end (codegen rejects until 4/N) — complete.** Per ADR 0032 D8 (the resolve + types slice). **resolve**: `EnumId` + `ResolvedEnumDecl`/`ResolvedVariantDecl` on `ResolvedProgram.enums`; a Pass-0 enum table (names share the type namespace with structs/classes/traits → `RedefinedEnum`; in-enum dup → `DuplicateVariant`); the `Name::Variant(args)`/`Name::Variant()` construction is **disambiguated** from `ImplName::method` / `Class::init` *at resolve* (the AST parses all three as `QualifiedCall`/`ClassInit`; when the leading name is an enum → `ResolvedExprKind::EnumConstruct`); `match` → `ResolvedExprKind::Match` + `ResolvedPattern`/`ResolvedMatchArm` with per-arm payload-binding `VarId`s scoped into the arm body only (snapshot/restore of `vars`, mirroring `resolve_handle_expr`; `_` slots get a VarId but stay out of scope; same-name twice → `DuplicatePatternBinding`); the `EnumDeclNotYet`/`MatchNotYet` rejections are dropped. `ImplCtx` grew an `enum_table` field (Copy bundle — no per-fn signature churn). **types**: `Type::Enum(EnumId)` (the 11th interner-style `Copy`-`Type` variant) + `EnumData`/`VariantData` + `TypedProgram.enums` + `enum_data` accessor; the `Type::Enum` cascade got real-or-rejecting arms across every exhaustive `Type` match (`type_display`/`Display`/`to_nullable_inner`/`to_array_elem`/`substitute`/`try_substitute`/`contains_type_param`); enum names resolve in **type position** (`resolve_type_expr` precedence struct→class→enum→primitive — threaded a new `enum_table` param to its ~20 call sites); `EnumConstruct` type-checks (variant→index, payload arity + per-arg coercion → `Type::Enum`); `match` type-checks (scrutinee is `Type::Enum`; arm bodies checked with `expected` pushed down + unified; pattern bindings typed from variant payloads + bound in env with save/restore; **exhaustiveness** = every variant covered or a `_`). Five new `TypeError`s: `UnknownVariant`, `VariantPayloadArityMismatch`, `MatchScrutineeNotEnum`, `NonExhaustiveMatch`, `MatchArmTypeMismatch`. **Directly-recursive enums type-check** (the AST enabler — heap-boxed payloads per ADR 0032 D4 need no nullable indirection, unlike recursive structs; verified). **downstream** (the new `Resolved`/`Typed` `Match`+`EnumConstruct` variants forced coordinated arms — the C1.3.2-4 cascade): codegen's `llvm_basic_type` lowers `Type::Enum` → the abi-v1 `{ i32 tag, ptr payload }` (heap-boxed/recursive-safe, ?Struct-style) so **enum-typed signatures lower**, `mangle_type` renders by name; the construction/`match` *expression* lowering rejects with a clean `CodegenError::EnumCodegenNotYet` (the ~8 pre-pass walkers recurse into children so they don't panic; drop is a no-op gated by `field_type_needs_drop=false`). MIR → `MirOp::Opaque` carrying operands (taint-safe; no `secret enum` ⇒ a match tag is never secret — the D7 sink is a future guard). effect-check + borrow-check pass-through walks (enum is **Move** — owns its heap payload; `match` arms move-merge like `if`/`else`). So `enum`/`match` **type-check but codegen rejects until (4/N)**. +27 tests (1265) incl. a `c5d1_non_exhaustive_match` UI snapshot. Four-check green. Next: **D.1 (4/N)** — codegen: the `{tag,ptr}` construction (alloc payload + set tag) + `match`→LLVM `switch` (D5) + recursive payload drop (D6) + the abi-v1 enum-layout entry + stability test + the `c5d1_enum` pass fixture; **ADR 0032 flips to ACCEPTED**. Then (D.1b) generic enums (`Option`/`Result`) via mono.
**Phase D.1 (4/N) — enum + match codegen; the D.1 MVP closes. ADR 0032 → ACCEPTED-WITH-AMENDMENTS — complete.** `enum`/`match` compile + run end to end (ADR 0032 D4/D5/D6/D11). **Construction (D4):** `lower_enum_construct` builds the `{ i32 tag, ptr payload }` — heap-box (`sentinel_alloc`) a struct of the variant's payload fields + store the args, `null` payload for unit variants, tag = variant discriminant; `enum_payload_struct_type` is the single source of truth for the payload layout (construct / match / drop share it). **`match` (D5):** `lower_match` extracts the tag + payload ptr, emits an LLVM `switch` into one block per variant arm (each GEP/loads the payload fields into the bindings' alloca slots keyed by `VarId`, lowers the body), reconciling arm results through a result-alloca merge block (the `if`-merge machinery); `_` = switch default, else the default is `unreachable` (exhaustiveness is a type-check guarantee — no runtime fallback). **Drop (D6) — amendment A1:** scope-exit drop loads the `{tag,ptr}` and `sentinel_free`s the payload if non-null (the `?Struct` drop arm with a null test); `field_type_needs_drop`(`Enum`) is true iff some variant carries a payload (pure-unit enums stay drop-free). Recursive *payload-field* drop is **deferred** — a heap-typed payload / recursive enum leaks its nested boxes here, because inline expansion of a recursive enum's drop is infinite (needs synthesized per-enum drop *functions*); leaks only, no UAF/double-free (verified: a returned enum escapes the callee, a value moved into a fn is freed once). **Surface:** bare `Enum::Variant` unit *construction* now parses (a small `parse_postfix` branch — `Name::Seg` with no `(` → a 0-arg `QualifiedCall`, mapped to a unit `EnumConstruct` at resolve when `Name` is an enum) — matching ADR D2 + the pattern surface; no regression (bare `Name::method` for a non-enum, previously a parse error, is now a resolve error — no test depended on it). **abi-v1:** §2 gains the `Enum` = `{ i32 tag, ptr payload }` entry + a DataLayout stability assertion (16 bytes, align 8, tag@0 / ptr@8); the `(3/N)` `EnumCodegenNotYet` reject is removed. **c51 bar holds** (enum paths additive; existing emission unchanged + `repro.rs` byte-identical). Phase-go `tests/pass/c5d1_enum.sentinel` (Shape `Unit`/`Circle(i64)`/`Rect(i64,i64)` constructed + `match`ed → exit 42). +2 tests (1268). Four-check green. **Next: developer-scope call** — **D.1b** (generic enums `Option`/`Result` via the witness-table mono machinery, ADR 0016 reuse + the recursive-payload-drop follow-on), OR the next ADR 0031 D4 prerequisite (strings + a byte type, or growable collections). The recursive-enum *drop* gap is the carried-forward debt — see the (5/N) investigation below (the box-free drop is **leak-free for the standard recursive-consume walk**, empirically; it leaks only when a payload binding isn't moved out or an enum is dropped unmatched — narrower than first stated; the full fix needs the payload-ownership model, not just drop fns).
**Phase D.1 (5/N) — recursive-drop investigation: it needs the payload-ownership model, NOT just synthesized drop fns. Code reverted to (4/N) box-free; docs-only.** Attempted the A1 follow-on (synthesize a per-enum `drop_<Enum>(ptr)` *function* so a recursive enum's drop is a runtime *call*, not infinite inline expansion, and recurse-drop the active variant's payload fields). **Built, then reverted — it double-frees** (empirically a `Tree` sum aborts, exit 133). The bug is **ownership**, not codegen: `match t { Node(l, r) => sum(l) + sum(r) }` loads `l`/`r` out of `t`'s payload and *moves* them into `sum(...)` (which frees their boxes), but `t` is also dropped at scope exit (the match reads it non-consumingly), so `drop_Tree(t)` recurse-frees the **same** child boxes again. **Correct fix = the payload-ownership model** (Rust semantics): (1) `match` **consumes** the scrutinee (partial move — not dropped at the enclosing scope); (2) a by-value binding **owns** its payload field, and the `match` frees the payload **box** without recurse-dropping the fields; (3) bindings are registered in the **drop plan** as arm-scoped locals, so an un-moved binding is dropped at arm-scope exit (reusing `emit_scope_drops` + the moved-set); (4) the synthesized `drop_<Enum>` fn is then sound, for the *non-match* drop paths (a `let`-bound never-matched enum; an un-moved binding). This is a **coordinated borrow-check + drop-plan + codegen change** (real partial-move tracking for enum payloads) — a proper sub-phase, bigger than "add drop fns". Recorded in ADR 0032 (A1 follow-up). **Box-free (leak-safe, no UAF) stays the shipped MVP behavior**; the tree still runs to exit 42 (leaking children). No code change (reverted); 1268 tests, four-check green. **Follow-up (empirical, via `leaks --atExit`):** the box-free debt is **narrower than first stated** — the standard recursive-consume walk (`match` + recurse-with-move on all children — the AST-evaluator shape) AND flat enums are **leak-free** (0 leaks); leaks occur only when a payload binding isn't moved out (bind-and-ignore) or an enum is dropped unmatched. So the ownership model is a *completeness* fix, not a self-hosting blocker, and is now **verifiable** via `leaks`. (Corrected "an AST leaks per node" in the docs.)
**ADR 0033 PROPOSED — Phase D.2 kickoff: strings + a byte (`u8`) type — docs-only.** The developer chose to iterate the ADR 0031 D4 roadmap in order; strings is next (a self-hosted lexer's input is text). ADR 0033 designs it end-to-end (9 D-decisions): the load-bearing call is **a string IS a `[u8]`** (byte array) — maximal reuse of the C1.6 array machinery (`len`/index/RAII drop/move/escape/arena/`abi-v1` `{i64,ptr}` all apply unchanged); plus `Type::U8` (an integer-scalar primitive → `i8`, reusing the op-generic arithmetic/cmp/bitwise + secret pipelines — almost no new typing surface; the next exhaustive-`Type`-match cascade after `Type::Enum`); char literals `'a'` (→ `u8`, escapes `\n \t \r \0 \\ \' \xHH`); string literals `"…"` (→ `[u8]`, **heap-copied** from a private global `[N x i8]` constant so they drop/move uniformly with no global-free hazard); the lexer ops `s[i]`/`len(s)` (reused) + a `str_eq` builtin + `u8`↔`i64` conversions (mixed-width stays a type error). 4-sub-phase split (lexer → AST/parser → `Type::U8`+cascade → codegen/runtime + `c5d2_strings` phase-go). Out of scope (D8): a nominal growable `String` (collections sub-phase D.3), UTF-8/Unicode code points, concat/substring/slice, `[u8]` mutation, signed `i8`. Next: **D.2 (1/N)** — the lexer.
**Phase D.2 (1/N) — lexer: string + char literals — complete.** Per ADR 0033 D7. Two new logos `TokenKind`s: `StringLit` (`"…"`) + `CharLit` (`'…'`), recognised with escape-aware regexes (`([^"\\\n]|\\.)*` between quotes — a `\`-escape or any non-quote/non-backslash/non-newline char; excluding raw newlines makes an unterminated literal fail fast). **Recognise-not-decode**, exactly like `IntLit`: the byte value(s) are recovered from the span at parse time (2/N), so `TokenKind` stays payload-free. Confirmed **`u8` needs no keyword token** — it lexes as an `Ident`, recognised as a primitive type name at the types layer like `i64`/`i32`/`bool` (verified: `u8` → `Ident`, `u8_to_i64` → one `Ident` by longest-match). Additive — the new tokens cascade nowhere (no exhaustive `TokenKind` match downstream); existing programs unaffected. +7 lexer tests (1275 total), four-check green. Next: **D.2 (2/N)** — AST + parser (`ExprKind::CharLit`/`StringLit` + escape decoding; `u8` in `TypeExpr`).
**Phase D.2 (2/N) — AST + parser: char/string literals — complete (`f310f15`).** Per ADR 0033 D7 (2/N), mirroring the enum/`match` (2/N) shape — the blast radius stays in **ast + syntax + resolve** (verified: every codegen/types/mir/borrow/effect match is on the *typed/resolved* tree, so adding raw-`ExprKind` variants touches only the AST `Display` + `resolve_expr`). AST: `ExprKind` += `CharLit(u8)` + `StringLit(Vec<u8>)` carrying the **decoded** bytes (a string IS a `[u8]` — D3), with s-expr `Display` arms `(char N)` / `(string b0 b1 …)`. Parser: **decode the span at parse time** (like `IntLit`) — strip the quotes by byte index, then `decode_byte_literal` processes the escapes `\n \t \r \0 \\ \' \"` and `\xHH` (two hex digits → one byte via `hex_digit`) into the bytes; non-escape bytes (incl. multi-byte UTF-8) pass through verbatim, so a string is exactly its UTF-8 source bytes. A char literal must decode to **exactly one byte** — `''` (empty) / `'ab'` (too many) / a multi-byte source char are rejected (`CharLitNotSingleByte`); an unknown escape letter or a malformed `\x` (non-hex or < 2 digits) is rejected (`InvalidEscape`). Two new `ParseError`s + their `query.rs` diagnostic arms; the decoder is bounds-checked (panic-free) via `.get(..).ok_or(())?`. `u8` needed **no type-parser change** — it already parses as a `TypeExpr` `Ident`; `[u8]` / `u8` in param/return position are parse-confirmed by test. Resolve: **rejects** char/string literals with `CharStringLitNotYet` (an or-pattern arm before the type layer) — so **`ResolvedExprKind` gains no new variant** and the downstream typed-tree crates are untouched until 3/N brings up `Type::U8`. Additive end-to-end. +21 tests (2 ast `Display`, 17 parser decode/validate/reject + `u8`-in-`TypeExpr`, 2 resolve reject) — **1296 total**, four-check green. Next: **D.2 (3/N)** — the type layer: `Type::U8` + the cascade across every exhaustive `Type` match + `ArrayElem::U8`; char → `u8`, string → `[u8]`; `str_eq` + `u8`↔`i64` conversion builtins typed; resolve → typed mirrors.
**Phase D.2 (3/N) — the type layer: `Type::U8` + char/string typing + byte builtins — complete (`56f69f0`).** Per ADR 0033 D3/D4/D5, mirroring the enum (3/N) discipline (land the `Type` cascade together → build → semantics → codegen-rejects-until-(4/N)). char/string now **type-check** end to end; the blast radius is **types + codegen + borrow + mir + effect + resolve** (the enum-(3/N) surface; no ast/syntax — those landed at 2/N). **`Type::U8`** is a new primitive variant (no interner, like `I32`/`Bool`) that cascades across every exhaustive `Type` match — found via the "add the variant, let the compiler enumerate the cascade" discipline: types (`is_int` += U8 / `to_array_elem` → `ArrayElem::U8` / `to_nullable_inner` → `None` (no `?u8` at MVP) / `substitute` / `try_substitute` / `contains_type_param` / `type_display` / `Display`), codegen (`llvm_basic_type` → `i8` / `llvm_int_type` → `i8` / `mangle_type` → `u8` / the needs-drop + drop-emit no-drop groups), borrow (`is_copy` → Copy — a `u8` byte is a 1-byte copy scalar; a `[u8]` is `Array(_)` → Move). **`ArrayElem::U8`** is the one genuinely new array element (`[u8]` IS the string — ADR D3). `u8` resolves in type position like `i64`/`i32`/`bool` (`resolve_type_expr`); `[u8]` flows through the existing array type-expr path. `ResolvedExprKind`/`TypedExprKind` gain `CharLit(u8)`/`StringLit(Vec<u8>)`; the type checker assigns char → `Type::U8`, string → `Type::Array(ArrayElem::U8)`. **The op-generic pipeline absorbs `u8` with ONE change** (`is_int += U8`): arithmetic + bitwise type via `Binary`, comparison via `Cmp` (→ bool), and mixed-width `u8 + i64` stays a `Mismatch` (the existing `l.ty != r.ty` operand check — no new `TypeError`); `secret u8` inherits the C3.1b secret-preserving rules for free. **Three runtime builtins typed** (concrete, non-generic): `str_eq([u8],[u8]) → bool`, `u8_to_i64(u8) → i64`, `i64_to_u8(i64) → u8` — registered as `FnId(4..=6)` in resolve + `TypedFnSignature`s in types, so **user fns shift +3** (main: 4 → 7; the ~5 hardcoded-`FnId` test sites in resolve/types/effect/borrow updated). resolve drops the (2/N) `CharStringLitNotYet` reject (now produces the literals); mir lowers literals to `MirOp::Opaque` (public constants — taint-safe for the D5 pass); effect/borrow treat them as pure leaves (a string's owned `[u8]` drops via its binding's type, like any array). **Codegen lowers `u8` → `i8`** so the cascade is real (a `u8` fn body `c + c` compiles to an object, exit 0) but **rejects char/string literals + the three builtin calls** with `CodegenError::StringCodegenNotYetSupported` until (4/N) (the enum-(3/N) codegen-rejects-until-(4/N) discipline; **empirically verified**: `let s = "hi"` type-checks then rejects cleanly at codegen — exit 1, no panic). +13 tests (12 type-layer unit: char/string typing, `u8` arith/cmp/bitwise, mixed-width + no-implicit-widen rejects, `[u8]` index → `u8`, `str_eq`/conversions typing + a `[i64]`-arg reject, `secret u8`; + the `c5d2_mixed_width` UI snapshot — `u8 + i64` Mismatch renders as "expected u8, found i64") — **1309 total**; the 2 resolve (2/N) reject-tests became resolve-tests. Four-check green. Next: **D.2 (4/N)** — codegen + runtime: the `i8` char constant; the string-literal private global `[N x i8]` + heap copy (so `[u8]` drops/moves uniformly — ADR D6); `sentinel_str_eq`; the `zext`/`trunc` conversions; `abi-v1` `u8` entry + tests + the `c5d2_strings` phase-go (verified leak-free via `leaks --atExit`); ADR 0033 flip.
**Phase D.2 (4/N) — codegen + runtime: the strings + `u8` MVP runs end to end — complete (`891ec98`). ADR 0033 → ACCEPTED-WITH-AMENDMENTS. Phase D.2 closes.** Per ADR 0033 D6/D9 — the (3/N) `StringCodegenNotYetSupported` rejects are replaced by real lowering. **Codegen:** a char literal is an `i8` constant; a string literal **heap-copies** its decoded bytes (`sentinel_alloc(N)` + N `i8` stores) into an owned `[u8]` that drops/moves via the existing array paths; `u8` lowers to `i8` with **unsigned** ops — `udiv` + unsigned `icmp` predicates (`is_unsigned = strip_secret(lhs.ty) == U8`), so a byte `≥ 0x80` compares large not negative; `u8_to_i64` is a `zext`, `i64_to_u8` a `trunc`; `str_eq` lowers to a call to the new runtime `sentinel_str_eq(ptr, i64, ptr, i64) -> i1` (extract `{len,ptr}` from each `[u8]` struct value). **Runtime:** `sentinel_str_eq` — equal length + byte-wise equality (Rust `extern "C" -> bool` lowers to `i1 zeroext`, matching codegen's `i1` decl; `#[allow(not_unsafe_ptr_arg_deref)]` per the existing runtime convention); the `[u8]` args are **borrowed** (the C2.3 runtime-builtin rule treats them non-consuming, like `len`), so `str_eq` does **not** free them — the caller's bindings drop them. **abi-v1:** `u8` → `i8` (size 1, align 1, mangles `u8`; `[u8]` → `arr_u8`, same `{i64,ptr}` layout); `sentinel_str_eq` joins the symbol contract (now **19**); the doc + `abi_v1_type_layouts_via_datalayout` / `abi_v1_mangling_is_stable` / `abi_v1_runtime_symbol_set` tests pin it. **Verified empirically** (exit-code-is-the-answer + `leaks --atExit`): `tests/pass/c5d2_strings` parses the 2-digit source "42" → exit 42, **0 leaks** (char-lit `u8` compare via `is_digit`, `u8`↔`i64` via `digit_val`, `[u8]` indexing, `str_eq` over bound keywords); `tests/pass/c5d2_u8_unsigned` pins the unsigned paths (`200 > 100` true, `200 / 100 == 2`) → exit 42, **0 leaks**; a `u8` fn body compiles to an object; the **c51 repro bar holds** (an unused `sentinel_str_eq` decl doesn't change emitted objects). **Amendments:** **A1** string literals heap-copy via **direct byte-stores**, not D6's private global `[N x i8]` + `memcpy` — `CodegenCtx` holds no `&Module` to add a global from a lowering method (the same constraint that pre-walks spawn wrappers); identical owned-heap-copy semantics, the global deferred as a measured optimisation. **A2** inline string-literal **arguments** to borrowing builtins leak — the **pre-existing general temporary-drop gap** (empirically: `len([i64-array])` temporaries leak identically when unreachable; **not** D.2-introduced), tied to the deferred full escape analysis (ADR 0026 D2, post-1.0). **Bound variables are leak-free** (the phase-go binds every literal, as a real lexer holds its data); a future `&[u8]` builtin signature is the cleaner long-term form. **A3** `str_eq` args are **borrowed, not consumed** (the C2.3 builtin rule). +2 pass fixtures (**1311 total**), four-check green. **Phase D.2 (strings + a `u8` byte type) MVP closes.** Next: the ADR 0031 D4 roadmap continues — **growable collections** (ADR 0034 PROPOSED) → file I/O (stdlib) → modules → loops, then the self-host port.
**ADR 0034 PROPOSED — Phase D.3 kickoff: growable collections (`Vec<T>`; `String` = `Vec<u8>`) — docs-only.** Per ADR 0031 D4 item 3 — a lexer accumulates an identifier byte-by-byte and a parser accumulates token/node lists, neither expressible with the fixed `[T]` array (no `push`/growth). ADR 0034 designs it end-to-end (10 D-decisions): the load-bearing lever is **a `Vec<T>` is `[T]` plus capacity + mutation** — `Type::Vec(VecElem)` mirrors `Type::Array(ArrayElem)` exactly (the same flat element subset `I64`/`I32`/`Bool`/`U8`/`Struct`, an `abi-v1` `{ i64 len, i64 cap, ptr data }` layout, element-generic builtins that recover `T` from the typed arg like array-index/`len` already do), so it **reuses the array index/bounds-check/move/drop machinery with NO new monomorphisation and NO lexer/parser change** (`Vec<u8>` already parses as a `Generic { name: "Vec", args: [u8] }` TypeExpr; the types layer recognises the name). **`String` = `Vec<u8>`** (a growable byte buffer — the 0033 "a string is its bytes" lever; not a separate nominal type). New pieces: a **capacity field + growth** (`push` reallocs `max(1, cap*2)` on overflow — libc `realloc`, NOT the broker bump arena which can't realloc); **`push(&mut v, x)`** — the **first heap-mutation primitive**, reusing `&mut T` + the C2.2 shared-XOR-mutable rule; `vec_new()` (element type inferred from the binding annotation, like `null`'s `?T`); `len(v)` (extend the existing builtin to `Vec`); `v[i]` (reuse the C1.6 bounds-checked `Index`). `Vec<T>` is a **builtin generic** (like `[T]`), NOT a `class Vec<T>` (generic classes deferred, ADR 0022 D1). Drop frees the buffer; **primitive-element `Vec` (`Vec<u8>`/`Vec<i64>`) is leak-free**, droppable-element `Vec` (`Vec<Struct>`/`Vec<[u8]>` recursive element drop) is **deferred** (the enum-A1-shaped follow-on). One new runtime symbol (`sentinel_vec_grow`/`realloc`). **3-sub-phase split (D9):** (1/N) `Type::Vec` + the cascade + `vec_new`/`push`/`len` typed + codegen + growth runtime + `&mut Vec` borrow + primitive-element drop (end to end — a growable `Vec<u8>`/`Vec<i64>`); (2/N) `v[i]` + `pop` + the `Vec<u8>`→`[u8]` bridge (`str_eq` a built string against a keyword); (3/N) close — `c5d3_collections` phase-go (leak-free via `leaks --atExit`) + `abi-v1` `Vec` entry + ADR flip. Out of scope (D8): a `Map`/`HashMap` (its own ADR), droppable-element `Vec` drop, `Vec`-in-generic-fns (`VecElem::TypeParam`), `with_capacity`/`insert`/`remove`/slicing/iterators/`for`, a `?T`-returning `pop`, `secret Vec`, broker-backing. Next: **D.3 (1/N)** — `Type::Vec` + the cascade + `vec_new`/`push`/`len` end to end.

**Phase D.3 (1/N) — growable `Vec<T>`: `Type::Vec` + `vec_new`/`push`/`len` end to end — complete (`a64883c`). ADR 0034 stays PROPOSED (3-sub-phase split; (1/N) Amendments recorded).** Per ADR 0034 D9 (1/N): a growable, owned, mutable `Vec<T>` — `[T]` plus a capacity field + mutation. **types:** `Type::Vec(VecElem)` (the flat element subset mirroring `ArrayElem`) + the full exhaustive-`Type`-match cascade (`substitute` / `try_substitute` / `contains_type_param` / `unify_one` — the last gets an explicit `(Vec,Vec)` arm so generic inference binds the element — plus `Display`, the `to_nullable_inner` / `to_array_elem` None groups, and `is_vec` / `to_vec_elem`); `resolve_type_expr` recognises `Vec<T>` as a builtin generic (flat element via `to_vec_elem`, else the new `VecElementNotSupported`); `vec_new<T>() -> Vec<T>` (element pinned from the binding / return annotation — the body-tail expected-type seeding extended to `Vec`) and `push<T>(&mut Vec<T>, T) -> i64` type through the **uniform generic-call path** (no special-casing), while `len` gets a contained `check_call` overload over `[T]` + `Vec<T>` (the `[T]` error path preserved exactly). **codegen:** `Vec` → `{ i64 len, i64 cap, ptr data }` (data is **field 2**); `lower_vec_new` builds `{0,0,null}`; `lower_push` loads the `&mut Vec`, grows via `sentinel_realloc` to `max(1, cap*2)*sizeof(T)` when `len==cap` (the grow block stores cap+data back, the continuation re-loads — no PHI), writes `data[len]=x`, bumps `len`; drop frees field 2 (null-safe); `len` reuses the field-0 extract. **runtime:** one new symbol `sentinel_realloc` (libc realloc; `realloc(null,n)==malloc` serves the first push). **borrow-check:** a `&mut Vec` builtin arg registers a **mutable borrow** (extends the ADR 0033 A3 runtime-builtin-arg rule to references), so `push` participates in shared-XOR-mutable and a non-`mut` `Vec` push is rejected (`BorrowMutOfImmutable`); `Vec` is **Move**. Builtins shift the FnId base (vec_new=7, push=8; main 7→9 — fixed the hardcoded-FnId test sites in resolve / effect-check / borrow-check / types). **abi-v1:** §2 `Vec` layout (`{i64,i64,ptr}`, 24/8, data@16) + §4 `vec_` mangling + §5 `sentinel_realloc` (now **20** symbols); `abi_v1_type_layouts_via_datalayout` + `abi_v1_runtime_symbol_set` pin it. **Amendments (ADR 0034):** A1 `String`=`Vec<u8>` deferred to (2/N) with the bridge; A2 return-type pushdown extended to `Vec`; A3 `len` overload special-case; A4 `sentinel_realloc` (not `sentinel_vec_grow`); A5 `VecElementNotSupported`; arena routing unchanged (a `Vec` init is a `Call`, not an `ArrayLit`, so `is_primitive_array_lit` already excludes it). **Verified** (exit-code + `leaks --atExit`): `tests/pass/c5d3_collections` builds a multi-growth `Vec<i64>` (6 pushes), a char-pushed `Vec<u8>`, and a `Vec` moved out of a helper (the escape path) → **exit 67, 0 leaks**; +13 tests (**1324 total**), four-check green. DEFERRED: (2/N) `v[i]` (the `Index` node carries `ArrayElem` + hard-codes the field-1 data ptr — real typed-tree + codegen work) + `pop` + the `Vec<u8>`→`[u8]` bridge + the `String` alias; (3/N) the richer phase-go + the ADR flip; (D8) droppable-element `Vec` drop. **Phase D.3 (1/N) lands.** Next: **D.3 (2/N)**.

**Phase D.3 (2/N) — `Vec` `v[i]` / `pop` / the `Vec<u8>`->`[u8]` bridge / `String` — the growable-`Vec` MVP is COMPLETE (`8430b0a`). ADR 0034 → ACCEPTED-WITH-AMENDMENTS.** Folded in the thin (3/N) close (comprehensive phase-go + ADR flip), since these four pieces exhaust the D.3 MVP (the rest is D8-deferred) and the abi-v1 Vec entry already landed in (1/N). **`v[i]`:** reuses the C1.6 bounds-checked `Index` with NO new typed node — the type checker accepts a `Vec` target (its `VecElem` demotes to the structurally identical `ArrayElem` for the node), and `lower_index` reads the data pointer from **field 2** (`Vec`) vs **field 1** (array), keyed on the secret-stripped target type; `len` (field 0) + the OOB trap are reused verbatim. **`pop<T>(&mut Vec<T>) -> T`** and **`vec_to_array<T>(Vec<T>) -> [T]`** (the bridge) are new builtins (FnId 9 / 10; main 9→11) flowing the uniform generic-call path like `push`: `pop` decrements `len` (buffer retained) and traps on empty; `vec_to_array` is non-consuming (`memcpy`s the live `len*sizeof(T)` bytes into a fresh `sentinel_alloc`'d `[T]`, so the Vec + array own independent buffers — both freed), keeping `str_eq`'s `[u8]` surface unchanged. **`String` = `Vec<u8>`** (Amendment A1 resolved): the bare name resolves to `Type::Vec(VecElem::U8)` in `resolve_type_expr`; a string *literal* is still a `[u8]` (so `let s: String = "hi"` is a Mismatch — build via `vec_new`+`push`; the bridge closes the loop the other way). **Amendments B1** (`v[i]` reuses Index, no `VecIndex` node — no typed-tree cascade), **B2** (`pop`/`vec_to_array` uniform-path builtins), **B3** (`String` alias). **Verified** (exit-code + `leaks --atExit`): `v[i]` reads + OOB trap, `pop` + empty-pop trap (exit 134), the bridge (positive + negative `str_eq`, non-consuming, empty), `String` build; the comprehensive `c5d3_collections` (`Vec<i64>` push/index/pop/len + escape; `String` "let" built/indexed/bridged/`str_eq`'d) runs at **exit 55, 0 leaks**. +9 type-layer tests (**1334 total**), four-check green. **Phase D.3 MVP closes; ADR 0034 ACCEPTED-WITH-AMENDMENTS.** Next: **D.4 — file I/O (ADR 0031 D4 item 4; ADR 0035 to be written).**

**ADR 0035 PROPOSED — Phase D.4 kickoff: file I/O via a minimal stdlib (`read_file` / `write_file`) — docs-only.** Per ADR 0031 D4 item 4 — a self-hosting compiler must read its source + write its artifact, and the only runtime I/O is `sentinel_print(i64)`. ADR 0035 designs it (10 D-decisions). **Load-bearing call (D2): file I/O = runtime builtins (like `print`), NOT the algebraic-effect/handler machinery** — ADR 0020 effects are *resumable user computations* (a `handle` arm resumes via `k`); OS I/O is irreversible side effects, and the effect-check forbids an effectful `main` + provides no runtime handler, so an `Io` effect would force a `handle` whose arm *still* calls a syscall builtin (pure ceremony). This **amends ADR 0031 D4's "effects + handlers" framing**; the effect-ROW promotion (`print`/I/O → `! { Io }`) stays a deferred orthogonal concern (D3). **Surface (D4):** `read_file(path: [u8]) -> [u8]` (whole file → owned byte array, reusing the `{len,ptr}` array machinery), `write_file(path: [u8], data: [u8]) -> i64` (create/truncate), `print_bytes([u8]) -> i64` (stdout) — all builtins in the `print`/`str_eq` mould, backed by new libc `sentinel_read_file`/`sentinel_write_file`/`sentinel_print_bytes` wrappers joining `abi-v1` §5. **Error model (D5):** panic-on-failure (abort like OOB/bad-alloc); a recoverable `?[u8]`/`Result` is deferred (wants D.1b generic enums). Paths are `[u8]`, NUL-terminated by the wrapper; `write_file`/`print_bytes` borrow their args (the ADR 0033 A3 rule). **2-sub-phase split (D9):** (1/N) `read_file`+`write_file` + the 2 runtime symbols + a write-then-read-back round-trip phase-go; (2/N) close — `print_bytes` + abi-v1 + ADR flip. Out of scope (D8): recoverable errors, the `Io` effect row, streaming/handles/`seek`/`fd`s, `read_stdin`, directories/`stat`, append, `Vec<u8>`-return, `secret` I/O. **3 OPEN DESIGN POINTS (settle before (1/N)):** (1) effects-vs-builtins (proposed: builtins), (2) error model (proposed: panic), (3) MVP surface (proposed: read_file+write_file, +`print_bytes` in 2/N; is `read_stdin` in?). Next: **D.4 (1/N)** — settle the open points, then `read_file`+`write_file` end to end.

**Phase D.4 (1/N) — file I/O: `read_file` + `write_file` end to end — complete (`2c530f6`). ADR 0035 stays PROPOSED ((1/N) landed; the 3 open design points resolved at the proposed defaults).** Per ADR 0035: file I/O = **runtime builtins** (like `print`), NOT algebraic effects (D2). **`read_file(path: [u8]) -> [u8]`** reads a whole file into a fresh owned byte array; **`write_file(path: [u8], data: [u8]) -> i64`** creates/truncates + writes (returns 0); both **panic on failure** (D5). Typed as non-generic `[u8]` builtins (the `str_eq` template); dispatched in `lower_call`. **Runtime (Amendment A1):** uses Rust `std::fs::read`/`write` (the runtime is a Rust crate linking `std`), NOT raw libc `fopen`/`fread` (D6's sketch); `read_file` **copies** the bytes into a `sentinel_alloc`'d (libc-malloc) buffer so the caller's `Type::Array` scope-exit drop frees it. Paths build a Unix `OsStr` from raw `[u8]` (non-UTF-8 OK), aborting on an embedded NUL. **ABI (A2):** `sentinel_read_file(path_ptr, path_len, out_len: *i64) -> data_ptr` (out-param for the count); `sentinel_write_file(path_ptr, path_len, data_ptr, data_len) -> i64`; `write_file`'s args borrowed (ADR 0033 A3). abi-v1 now **22** symbols. Builtins FnId 0..=12 (read_file=11, write_file=12; main 11→13 — fixed the hardcoded-FnId test sites). **Verified** (exit-code + `leaks --atExit`): a write-then-read-back round-trip (`str_eq` + a `back[i]` byte spot-check + `len`), exact-byte fidelity, the missing-file abort (exit 134, clear message); phase-go `c5d4_file_io` round-trips "hello" → **exit 5, 0 leaks** (the harness removes the temp file). +5 tests (**1339 total**), four-check green. ⚠ inline string-literal ARGS to read_file/write_file leak (the ADR 0033 A2 temp-drop gap — bind paths/payloads). DEFERRED: (2/N) `print_bytes` (stdout) + the ADR flip; (D8) recoverable errors / `Io` effect row / streaming / `read_stdin` / directories. **Phase D.4 (1/N) lands.** Next: **D.4 (2/N)** — `print_bytes` + close.

**Phase D.4 (2/N) — `print_bytes` (stdout) — the file-I/O MVP is COMPLETE (`fb1b51b`). ADR 0035 → ACCEPTED-WITH-AMENDMENTS.** **`print_bytes(data: [u8]) -> i64`** writes a byte array to stdout — the byte/string companion to `print` (one i64). The `write_file` template minus the path: a runtime builtin (FnId 13; main 13→14), arg borrowed (ADR 0033 A3), backed by `sentinel_print_bytes(data_ptr, data_len)` (abi-v1 now **23** symbols). **Amendment B1:** writes **exactly** `data_len` bytes — NO added newline (unlike `print`'s `println!`) — then **flushes** stdout, so the bytes are visible before the C-ABI `main` return and interleave correctly with `print` (shared `std::io::stdout`; `od -c`-verified: `print_bytes("AB"); print(7); print_bytes("AB")` → `AB7\nAB`). **Verified** (exit + stdout + `leaks --atExit`): the comprehensive phase-go `c5d4_file_io` now round-trips a file (write → read → `str_eq` + `back[i]` + `len`) AND `print_bytes` the read-back content, asserting **both exit 5 AND stdout "hello"**, 0 leaks. +2 type-layer tests (**1341 total**), four-check green. **Phase D.4 MVP closes; ADR 0035 ACCEPTED-WITH-AMENDMENTS** (read_file + write_file + print_bytes; recoverable errors / `Io` effect row / streaming / `read_stdin` / directories stay deferred per D8). Next: **D.5** — the next ADR 0031 D4 prerequisite (#5 modules or #6 loops; dev picks — loops recommended as smaller + unblocking iteration), then the self-host port.

**ADR 0036 PROPOSED — Phase D.5 kickoff: loops (`while`) — docs-only.** Per ADR 0031 D4 item 6 (the dev chose loops over #5 modules). The surface has been recursion-only by design since 1.0; a compiler's iteration-heavy passes (scan a byte buffer, drain a token `Vec`) want bounded, stack-safe iteration. ADR 0036 designs it (10 D-decisions). **Load-bearing calls: (D3) a loop is a STATEMENT** — `StmtKind::While { cond, body }` alongside `Let`/`Assign`/`Expr`, NOT an expression (a loop has no value; Sentinel has no unit type, so an expression form would force a synthetic `i64` 0). **(D4) `while` lowers to the first BACKWARD CFG branch** — three blocks `loop_cond` / `loop_body` / `loop_after` with a back-edge body→cond (all prior control flow — `if`/`match` — merged *forward* into a tree CFG). **(D5) per-iteration drop** — the body is a `lower_block` scope, so its bindings drop each iteration via the back-edge (a body that allocates each pass is leak-free, not accumulating — the load-bearing correctness property). **(D8, the key risk) the loop-carried move rule** — the borrow checker walks the body once but it runs N times, so moving an *outer* binding inside the body is a use-after-move on re-entry; proposed: conservatively **reject moving an outer Move-typed binding in a `while` body**. **Surface (D2):** `while <bool> { <body> }`; loop-carried state is a `let mut` outside + `Assign` inside (`i = i + 1`); `cond` must be `bool`. NO new `Type`, no cascade, no FnId-shift (not a builtin). `while` is a NEW lexer token (`for` is taken by `impl … for …`). **2-sub-phase split (D9):** (1/N) `while` (token + parser + `StmtKind::While` + bool-cond rule + the D8 move rule + back-edge codegen + per-iteration drop); (2/N) `break`/`continue` (branch to loop_after/loop_cond + a loop-target stack). Out of scope (D8): `for` / ranges / iterators, labeled break, `break`-with-value / loop-as-expression, do-while, a termination check (`while true {}` is well-formed). **3 OPEN DESIGN POINTS (settle before (1/N)):** (1) the loop-carried move rule (conservative reject vs. dataflow), (2) break/continue → (2/N), (3) while-as-statement. Next: **D.5 (1/N)** — settle the open points, then `while` end to end.

**Phase D.5 (1/N) — the `while` loop — end to end — complete (`adec9c3`). ADR 0036 stays PROPOSED ((1/N) landed; the 3 open design points resolved at the proposed defaults).** A `while` loop through the WHOLE pipeline. **(D3) a loop is a STATEMENT** (`StmtKind::While { cond, body }`, not an expression — no loop value, no unit type): lexer (new `while` token; `for` is taken by `impl … for …`), parser (statement position; `parse_loop_body` = `parse_block_inner(allow_stmt_only=true)` synthesises a discarded unit tail for a statement-only body — Amendment A1, since Sentinel blocks require a tail per ADR 0010 D6; struct lits forbidden in the cond like `if`), AST/resolve (body scope, snapshot/restore vars)/types (cond must be `bool`; `secret bool` → SecretBranch; body value discarded) + the StmtKind cascade across mir/effect-check/codegen (8 codegen walk sites + lower_stmt). **(D4) the FIRST backward CFG branch:** `lower_stmt::While` emits `loop_cond`/`loop_body`/`loop_after` with a back-edge body→cond; the body lowers via `lower_block` so its bindings **drop per-iteration** (D5 — a body allocating each pass is leak-free). **(D8, the key risk) the loop-carried move rule:** moving an *outer* Move-typed binding inside the cond/body is a use-after-move on re-entry → rejected (`MovedInLoopBody`, a new BorrowError); implemented by snapshotting in-scope + moved sets before the loop and flagging any outer binding newly moved. A body-local move is fine; loop-carried `Assign`/`push(&mut v)` are fine. **Amendment A2 (load-bearing codegen fix): entry-block alloca hoisting.** A body `alloca` emitted inline in `loop_body` runs every iteration → stack grows → overflow at large N (verified: a 2M-iteration body-`let` loop SIGSEGV'd). Fix: a `loop_depth` counter (bumped around `lower_block(while-body)`); when >0, per-binding allocas (`let`/`if`-result/`match`-result) go to the fn entry block (executed once, slot reused) via `binding_alloca`. Non-loop codegen (`loop_depth==0`) keeps the inline alloca → **byte-identical** to pre-D.5 (c51 bar holds). **No new `Type`, no `Type` cascade, no FnId-shift.** **Verified** (exit + `leaks --atExit`): counter loop, Vec-built-in-loop, body-allocating loop (leak-free), 2M-iteration loop (no overflow), zero-iteration loops, loop-carried-move rejection; phase-go `c5d5_loops` → **exit 67, 0 leaks**. +9 tests (3 type + 3 borrow + 2 parser + the fixture; **1350 total**), four-check green. DEFERRED: (2/N) `break`/`continue` + the ADR flip; (D8) `for`/ranges/iterators, labeled break, `break`-with-value, termination check. **Phase D.5 (1/N) lands.** Next: **D.5 (2/N)** — `break`/`continue` + close.

**Phase D.5 (2/N) — `break` / `continue` — end to end — complete. ADR 0036 → ACCEPTED-WITH-AMENDMENTS (D.5 closed).** Loops gain early exit / skip. **`break` / `continue` are payload-free STATEMENTS** (`StmtKind::Break`/`Continue` alongside `While`; new `break`/`continue` lexer keywords) branching to the innermost enclosing loop's `loop_after` (break) / `loop_cond` (continue). Pipeline: lexer tokens (+ ident-prefix regression) → AST/resolve/types/mir/effect-check/borrow/codegen StmtKind cascade (the resolve/mir/effect-check/borrow arms are no-ops — no sub-expr, no move, no effect). **(C2) the loop-target STACK:** a `LoopTarget { cond_bb, after_bb, scope_floor }` pushed onto `CodegenCtx::loop_targets` entering a `while` body, popped on exit; `break`/`continue` read the top (innermost — no labels). **(C1) the load-bearing drains-before-branch:** a break/continue branches out of the *middle* of the body, skipping `lower_block`'s end-of-body `emit_scope_drops`, so codegen **drops every scope frame from the top down to the loop body BEFORE branching** — `emit_loop_exit_drops(scope_floor)` (the body scope + any nested `if`/block scopes, innermost first; `scope_floor` = the body frame's index captured at loop entry). `emit_scope_drops` was split into a per-frame `emit_frame_drops` to share the logic. Each runtime path frees a binding exactly once (early-exit drop, or body-end drop on fall-through — mutually exclusive blocks); **verified leak-free** with a `[u8]` live across a break AND a continue, incl. a nested inner loop (inner break drains only the inner scope). **(C3) first mid-block divergence:** Sentinel has no early `return`, so break/continue is the first construct to terminate a block mid-stream — the statically-lowered, now-dead remainder parks on a fresh `after_loopctl` block (never append to a terminated block; covers a stmt-only body's synth unit tail + `lower_if`'s store/merge). **(C4) out-of-loop rejection:** `break`/`continue` outside any loop → `TypeError::LoopControlOutsideLoop` (names the kw), via a `loop_depth: u32` on `VarTypeEnv` (bumped around a `while` body — threads through nested `if`/`match`; legal iff `>0`; fresh per fn so no break across a fn). **(C5) ergonomic note:** a *conditional* break uses the tail idiom `if c { break; 0 } else { 0 };` (`if` requires `else` + a tail per ADR 0010/0013 — pre-existing, not break's fault; cleaner ergonomics is a Revisit). **No new `Type`, no cascade beyond the StmtKind arms, no FnId-shift.** Phase-go `c5d5_break_continue` (break-terminated sum 15 + continue-filtered evens 30 + two loops that break/continue with a `[u8]` live, 30+40) → **exit 115, 0 leaks**; `c5d5_loops` still exit 67. +11 tests (5 type + 3 parser + 2 lexer + the fixture; **1361 total**), four-check green. **Phase D.5 COMPLETE.** Next: **#5 modules** (ADR 0031 D4 — the last prerequisite before the self-host port).

**ADR 0037 PROPOSED — Phase D.6 kickoff: modules / multi-file — docs-only.** The sixth and **last** ADR 0031 D4 prerequisite before the self-host port. Two decisions settled with the language owner: **(1) module surface = file-as-module + `use`** — a file IS a module, its path relative to the source root (the entry file's dir) IS its module path; `use a::b::Item;` imports a `pub` item; `pub` (parsed since C4.1, a no-op) becomes the cross-module visibility gate; NO `mod` blocks (the Go/Python shape, not Rust's in-file tree). **(2) compilation model = TRUE separate compilation** (NOT a whole-program multi-file merge) — each module compiles to its own `.o` independently, cross-module refs resolved at LINK time via stable `abi-v1`-keyed symbols. **The biggest architectural D-change:** it breaks 3 whole-program codegen assumptions — `collect_mono_instantiations` (whole-program generic-instance discovery), the single `fns: HashMap<FnId, FunctionValue>` map, and `self.fns.get(&id)` call resolution — and makes cross-unit symbols ABI surface (the current bare-source-name mangling is single-file-only + not collision-free → D7 = a module-qualified, length-prefixed `abi-v1` mangling amendment, test-enforced). **Sub-phase split (D9):** **(1/N)** surface + resolve module graph (per-unit ID spaces + namespaces + visibility) + per-unit type-check against imported signatures + **non-generic** separate compilation (per-unit `.o`, module-qualified mangling, extern-symbol cross-module calls + types, deterministic link); **(2/N)** **cross-module generics** (per-unit instantiation + `linkonce_odr` dedup — the C++ template model) + cross-module trait/impl methods; **(3/N)** incremental caching (Salsa) + per-unit `.o` repro. NO new runtime `sentinel_*` symbols (a front-end + linking concern). 4 OPEN DESIGN POINTS (settle at (1/N)): import cycles (lean allow); amend `abi-v1` vs bump `abi-v2` (lean amend); source root = entry-file dir; `use a::b::c` = item `c` in module `a::b`. **D.6 (1/N) IN PROGRESS (multi-file COMPILES + RUNS, via the owner-chosen lower-risk Path A merge, not yet true per-unit separate compilation):** `use` front-end + module-graph discovery + top-level `pub` + import resolution/visibility + the merge (`merge_modules` qualifies EVERY top-level item's name by module path — fn/struct/enum/trait/effect/class/named-impl — + rewrites all call/type/trait/effect references via a per-module `Renamer` → existing pipeline → one object → link). Cross-module `pub fn`/`pub struct`/`pub enum`+`match`/`pub trait`/`pub effect` all compile + run, same-named items across modules coexist, cross-module GENERICS work (whole-program mono over the merged graph), and the merged path runs effect-check (an unhandled-effect `main` is rejected). FOLLOW-UPS: the true per-unit back end (objects + module-qualified `abi-v1` mangling + multi-object link, incl. per-unit `linkonce_odr` generics); span-accurate multi-source diagnostics. The language gate for the self-host port (D5) is effectively cleared. See §0.3 RESUME-AT + ADR 0037 Implementation notes.
**ADR 0038 → ACCEPTED-WITH-AMENDMENTS — Phase D movement 2: the self-host port — kickoff + (1/N) lexer-in-Sentinel COMPLETE.** Movement 1 (the language/stdlib build-out, ADR 0031 D2: D.1 sum types → D.6 modules) is **complete**, so the self-hosting gate is cleared and **movement 2** (ADR 0031 D5 — port `snc` to Sentinel stage by stage, each differentially validated against the Rust `snc` oracle) opened. **(1/N) the LEXER landed:** `snc lex` (oracle) + `selfhost/lexer.sentinel` (the first compiler stage in Sentinel, all 69 `TokenKind`s) + a corpus differential test (139/139 clean-lexing fixtures match `snc lex`). Amendments: A1 direct dump emission (no Token enum yet — (2/N) adds the token list); A2 worked around two Sentinel quirks (flat per-fn var namespace; deep-if tail `&mut` borrow conflict); A3 lex-error parity deferred; A4 reads a fixed `input.sentinel` (no argv yet). **The differential-oracle method (D2):** the Rust `snc` gains a canonical stage-dump subcommand per ported stage; the Sentinel stage emits the byte-identical dump; a test diffs both over the `tests/pass` + `tests/ui` corpus. **First sub-phase (D3–D7) = the lexer:** add `snc lex <file>` (a line-oriented token dump `<KIND> <start> <end> [<lexeme>]`, variant *names* not discriminants — D4); write `selfhost/lexer.sentinel` (a new `selfhost/` `.sentinel` tree, growing into a D.6 module graph — D5) reproducing the Rust lexer's 69-variant `TokenKind` stream; a differential test asserts a corpus-wide match (D10). **Back-end-agnostic (Related/D8):** the port is Sentinel *source*, indifferent to merge vs per-unit objects, so it builds on the Path A merge and does NOT gate on the per-unit back end (ADR 0037 follow-up). Out of scope at (1/N): parser+ stages (each its own ADR), lexer *error* parity (follow-on), perf. Indicative split (D9): lexer → parser → resolve → types → HIR/MIR → codegen, each with its own oracle dump. The Rust `snc` stays the production compiler + oracle until the bootstrap fixed-point bakes (ADR 0031 D6). Next: **self-host port (2/N) — the parser** (`snc parse` is the oracle; grow `selfhost/lexer.sentinel` to RETURN a token list the parser consumes; its own ADR).
**ADR 0039 → ACCEPTED-WITH-AMENDMENTS — Phase D self-host port (2/N): the parser-in-Sentinel; (2a) LANDED.** The compiler's biggest stage, so it is explicitly sub-sliced. **Oracle (D2):** the existing `snc parse` `Display` is NOT complete (`Program`'s `Display` omits enums/traits/impls/classes), so add a new `snc ast <file>` — a complete, regular, S-expression-style canonical AST dump (every decl/stmt/expr/type/pattern; node *names* not tags; golden-tested; a dev surface, not abi-v1), which the Sentinel parser reproduces byte-for-byte (diffed over the corpus). **Token model (D3):** refactor `selfhost/lexer.sentinel` to RETURN a token stream as **struct-of-arrays of scalars** (`kinds`/`starts`/`ends`: `Vec<i64>`; lexemes re-sliced from `src`) — dodges the D.3 `Vec<struct-with-[u8]>` drop gap; tags stay internal; the lexer keeps its (1/N) dump so its test stays green. **AST model (D4):** Sentinel recursive enums/structs mirroring `sentinel-ast` — ⚠ the AST is the deepest recursive structure yet, and ADR 0032 A1's box-free recursive-enum drop is UNTESTED at AST scale, so **(2a) gates on a recursive-AST build→dump→drop `leaks` validation** (if it leaks/UAFs, land D.1b payload-ownership first). **Recursive descent (D5)** mirrors the Rust parser (token cursor + precedence-climbing). **Sub-slices (D6):** (2a) lexer-returns-tokens + AST scaffold + drop-validation + minimal expr parser + `snc ast` + seed diff; (2b) full expressions; (2c) statements + fns; (2d) the remaining decls + oracle completeness. Out of scope: resolve+, parser ERROR parity (happy-path first), perf. Reuses ADR 0038 A2 quirk-workarounds (flat per-fn namespace; deep-`if` tail-borrow). **(2a) LANDED** (amendments A1–A3): A1 `snc ast` oracle (run_ast + ast_dump.rs, golden-tested); A2 recursive-AST drop gate (selfhost_ast_drop.sentinel, 0 leaks — no D.1b needed); A3 parser structure settled by probe — `Vec<non-primitive>` unsupported (AST = recursive `Expr` enum returned by value + consuming-dumped, NOT an arena), refs index via explicit `(*r)[i]` (auto `r[i]` fails; the recursion enabler), left-assoc folds via recursion not loops (moved-in-loop rule), match arms need commas. `selfhost/parser.sentinel` (the 2nd Sentinel stage) parses paramless `fn`-bodied integer arithmetic → matches `snc ast` (tests/selfhost_parse.rs, 5 seeds, leak-free). Next: **(2b)** full expressions (vars/calls/if/match/struct-lit/…), then (2c) stmts+fns, (2d) decls — each growing the parser + diff corpus toward the full tests/pass+tests/ui set.
**Phase D self-host port (2/N) parser — (2b) increment-1: full operator-precedence expressions — complete (`0e84f36`). ADR 0039 amendment A4.** D6's "(2b) full expressions" row spans ~28 `ExprKind` variants, so (2b) is itself sub-sliced; **increment-1** grows `selfhost/parser.sentinel` from (2a)'s integer arithmetic to the **complete operator-precedence ladder**, mirroring the Rust parser exactly (`parse_expr → or → and → cmp → bitor → bitxor → bitand → add → mul → unary → atom`) so the AST *tree shape* — hence the `snc ast` dump — matches byte-for-byte. New surface vs (2a): logical `|| &&` (short-circuit precedence), the six **non-associative** comparisons `== != < <= > >=`, bitwise `| ^ &` (`&`>`^`>`|`), prefix unary `- !`, and the **scalar atom leaves** — integer / `true` / `false` / `null` literals + variable references (plus the existing int + parens). The `Expr` enum gains `Bool(bool)` / `Null` / `Var([u8])` / `Unary(i64, Expr)` + a unified `Binary(i64, Expr, Expr)` whose i64 **op-code** encodes both the dump category (`binop`/`cmp`/`logic`) and the operator symbol; the consuming recursive dump maps it back. `true`/`false`/`null` lex as identifiers but parse to the literal nodes the oracle emits (in-place byte compare, no allocation), **never** `(var …)`. The internal tokenizer is extended to longest-match all the new operators (`==`/`=`, `!=`/`!`, `<=` `>=` `&&` `||` plus `| ^ &`), keeping `->` + the (2a) set. Reuses every proven idiom (ADR 0038 A2 / ADR 0039 A3): recursive `Expr` by value + consuming `match` (no `Vec<Expr>`); shared token arrays + `src` indexed via `(*r)[i]`; a `&mut i64` cursor; left-assoc folds via `parse_X_rest` accumulator recursion (a loop accumulator trips moved-in-loop); flat per-fn unique locals; the dump computes prefix+symbol as `[u8]` values FIRST then emits (sibling `&mut out` borrows in `if` tails read as overlapping). **Verified:** the differential test now diffs **26 seeds** spanning every precedence level (incl. two interleaving the whole ladder) against `snc ast` — all byte-identical; leak-free under `leaks --atExit` (the recursive `Var` / `Unary` / `Binary` payloads drop via the consuming dump). **1402 tests, four-check green.** **Deferred to later (2b) increments:** postfix (call / index / field / method), `if`/`match` expressions, struct/array literals, perform/handle, qualified-call / class-init / scope / spawn / await / declassify; then (2c) statements + fns-with-params/blocks, (2d) the top-level decls.
**Phase D self-host port (2/N) parser — (2b) increment-2: function calls + the postfix chain — complete (`1b7d17c`). ADR 0039 amendment A5.** Adds, mirroring the Rust parser, free calls `f(args)` → `(call f …)` (an *atom* case — the callee is a NAME, not an expr; only a postfix `.m(args)` calls a value) and the **postfix chain** applied left-to-right over an atom: field `t.field` → `(field t field)`, index `t[i]` → `(index t i)`, method `t.m(args)` → `(method t m …)`. A new `parse_postfix` layer sits between `parse_unary` and `parse_atom` (`parse_unary` now falls through to it). **The data-model call:** an argument list is variadic and `Vec<non-primitive>` is unsupported, so args are a **second enum `Args = End | Cell(Expr, Args)`, mutually recursive with `Expr`** (`Expr` gains `Call([u8], Args)` / `Method(Expr, [u8], Args)` / `Field(Expr, [u8])` / `Index(Expr, Expr)`) — extending (2a)'s single-self-recursive-enum drop gate to **two mutually-recursive enums + enum-typed payloads**, which was **DE-RISKED by a probe first** (build → consuming-dump → `leaks`: compiles, correct, 0 leaks) before growing the parser, the same probe-first discipline as the (2a) structure work. `parse_args` builds the cons-list head-first by recursion + consumes the closing `)`; the postfix chain folds via `parse_postfix_rest` accumulator recursion (a loop accumulator trips moved-in-loop); the tokenizer gains `.` `[` `]` `,` (tags 26–29). **Verified:** the differential test now diffs **45 seeds** — calls (zero/one/many/expr/nested args), field/index/method, and chains like `a.b(c)[d].e` and `x.foo(1).bar[k].baz(y, z)` — all byte-identical to `snc ast`; leak-free under `leaks --atExit` (the `Args` cons-list + nested `Expr`s drop via the consuming dump). **1402 tests, four-check green.** **Still deferred to later (2b) increments:** the `::` paths (qualified-call / class-init / enum construction), struct + array literals, `if`/`match`, perform/handle, scope/spawn/await/declassify; then (2c) statements + fns-with-params/blocks, (2d) the top-level decls.
**Phase D self-host port (2/N) parser — (2b) increment-3: `::` paths + array literals — complete (`aa3307a`). ADR 0039 amendment A6.** Adds the identifier-prefixed `::` forms (parsed in `parse_atom` after an ident) + array literals, all reusing A5's `Args` cons-list: `Name::method(args)` → `(qcall Name method …)`; `Name::init(args)` → `(class-init Name …)` (the `init` name **with parens** is the only class-init form); a **paren-less** `Name::tail` (e.g. bare enum-unit `Enum::Variant`) → a qualified call with empty args — the enum-vs-impl meaning is a *resolve* concern, so the parser emits a uniform `(qcall …)` (matching the Rust parser). An **atom-position `[`** is an array literal `[e1, e2, …]` → `(array …)`, distinct from the **post-atom `[`** index operator (A5) by position (`parse_atom` vs `parse_postfix_rest`). `Expr` gains `Qcall([u8], [u8], Args)` / `ClassInit([u8], Args)` / `Array(Args)`; `parse_args` is generalised with a **terminator-tag** param (`)`=5 for call args, `]`=28 for array elements); the tokenizer gains `::` (30) + `:` (31); an `is_kw_init` slice-compare picks `init` (the self-contained tokenizer has no `init` keyword). **Verified:** the differential test now diffs **59 seeds** — qcall (with/without args/parens), class-init (with/without args), bare-init→qcall, arrays (empty/expr elems), array-then-index `[1,2][0]`, and deep nests like `g(A::b(x), [1, h(3)], Point::init(y, z))` and `[A::b(), c.d][0].e` — all byte-identical to `snc ast`; leak-free under `leaks --atExit`. **1402 tests, four-check green.** **Still deferred to later (2b) increments:** struct literals, `if`/`match`, perform/handle, scope/spawn/await/declassify; then (2c) statements + fns-with-params/blocks, (2d) the top-level decls.
**Phase D self-host port (2/N) parser — (2b) increment-4: `if`-expressions + brace blocks — complete (`4837622`). ADR 0039 amendment A7.** Adds the first **control-flow expression** + the **block** machinery: `if <cond> { <then> } else { <else> }` → `(if cond (block then) (block else))`, and a brace block `{ <expr> }` → `(block expr)`. `if` is dispatched at the **TOP of `parse_expr`** (so it is a full expression, never an operator operand — matching the Rust parser, whose `parse_add` operand is `parse_mul`, not `parse_expr`); `else` is **mandatory** (Sentinel has no bare `if`), and `else if` chains by wrapping the inner `if` in a block (matching the oracle). A brace block is also a `parse_atom` case. **Blocks are statement-FREE for now** — `BlockE(Expr)` holds just the tail; the full statement list lands at (2c), when `BlockE` grows a statement cons-list. `if` / `else` are **tagged in the tokenizer** (32 / 33) like `fn` (new `is_kw_if` / `is_kw_else`), so the parser dispatches + consumes them by tag. `Expr` gains `If(Expr, Expr, Expr)` + `BlockE(Expr)`; `parse_block` + `parse_if` (recursing for `else if`). **Verified:** the differential test now diffs **68 seeds** — basic `if`, cond exprs, `else if` chains, nested `if`, brace blocks, and `if` inside call args / array elements — all byte-identical to `snc ast`; leak-free under `leaks --atExit`. **1402 tests, four-check green.** **Still deferred to later (2b) increments:** `match` (the last control-flow expr — adds a `Pattern` enum + `parse_pattern` + arms), struct literals (need the Rust `allow_struct_lit` flag), perform/handle, scope/spawn/await, declassify.
**Phase D self-host port (2/N) parser — (2b) increment-5: `match` expressions + patterns — complete (`6e89d2a`). ADR 0039 amendment A8.** Adds the last control-flow expression: `match <scrutinee> { pat => body, … }` → `(match scrut (arm pat body)…)`, dispatched at the **top of `parse_expr`** alongside `if` (a `match` keyword tag); arms comma-separated (trailing comma allowed); arm **bodies are expressions** (`parse_expr`, not blocks — matching the Rust `parse_match_arm`). Patterns are the `_` wildcard → `(pat _)` or a qualified variant `Enum::Variant` with an optional **positional binding list** → `(pat Enum Variant b1 b2)` (each binding an ident, itself possibly `_`). **The data model is the deepest mutual recursion yet — four enums in a cycle** (`Expr → Arms → {Pattern → Binds, Expr}`): `Expr` gains `Match(Expr, Arms)`; new `Arms = ArmEnd | ArmCell(Pattern, Expr, Arms)`, `Pattern = PatWild | PatVariant([u8], [u8], Binds)`, `Binds = BindEnd | BindCell([u8], Binds)` — **de-risked by a probe first** (build → consuming-dump → `leaks`: 0 leaks), as with A5's `Args`. `parse_match` / `parse_arms` / `parse_pattern` / `parse_binds` build them by recursion (the cons-lists consume their closing bracket); the tokenizer gains the `match` keyword (34) + `=>` FatArrow (35) + `is_kw_match` / `is_wildcard`. **Verified:** the differential test now diffs **78 seeds** — multi-arm, single/multi/wildcard bindings, match-on-call scrutinee, if/match/call arm bodies, nested `match`, `match` in call args, trailing comma, and an AST-walker shape `match parse(t) { Node::Bin(op, l, r) => eval(l) + eval(r), … }` — all byte-identical to `snc ast`; leak-free under `leaks --atExit`. **1402 tests, four-check green.** **Still deferred to later (2b) increments:** struct literals (need the Rust `allow_struct_lit` flag), perform/handle, scope/spawn/await, declassify.
**Phase D self-host port (2/N) parser — (2b) increment-6: struct literals — complete (`4935335`). ADR 0039 amendment A9.** `Name { f1: e1, f2: e2 }` → `(struct-lit Name (field f1 e1) (field f2 e2))`. **The disambiguation is the story.** The Rust parser threads a stateful `allow_struct_lit` flag through the *whole* expression descent (set `false` in an `if`/`while`/`match` head so `if x { … }` reads `x` as the cond, not `x { … }` as a struct lit). Rather than thread a `bool` through ~19 parse functions, the port uses a **context-free lookahead `{ Ident :`** — a brace, an identifier, then a **single** colon — which *only ever* begins a struct literal (no block / match-body / if-body starts with a single-colon `Ident :`: no statement form is `name :`, and variant patterns use `::`). On all clean-parsing input this yields the identical AST to the flag, with **no threading** — the same "different implementation, byte-identical output" trade the lexer made. Verified that heads stay conditions: `if x { P { a: 1 } } else { Q { b: 2 } }`, `match s { St::A => P { v: 1 }, … }`, `(P { x: 1 }).x`. `Expr` gains `StructLit([u8], Fields)`; new `Fields = FieldEnd | FieldCell([u8], Expr, Fields)`; `parse_fields` parses `name : value` pairs (trailing comma OK); no tokenizer change (`:` already tag 31). **Documented limitation:** an **empty** struct literal `Name {}` is deferred (no `field :` to key on; collides with an empty `match`/`while` body — the flag is the eventual fix). **Verified:** the differential test now diffs **88 seeds** — single/multi-field, expr values, nested structs, structs in call args / arrays, trailing comma, and the head-disambiguation seeds — all byte-identical to `snc ast`; leak-free under `leaks --atExit`. **1402 tests, four-check green.** **Still deferred to later (2b) increments:** perform/handle, scope/spawn/await, declassify.
**Phase D self-host port (2/N) parser — (2b) increment-7: declassify / perform / scope / spawn / await — complete (`af76636`). ADR 0039 amendment A10.** The effect/concurrency leaf expressions: `declassify(e)` → `(declassify e)`; `perform Eff.op(args)` → `(perform Eff op args…)`; `scope concurrent { block }` → `(scope (block …))`; `spawn <postfix>` → `(spawn …)`; and the `.await` **postfix** → `(await target)`. `declassify`/`perform`/`scope`/`spawn` are keyword-led atom cases in `parse_atom` (`scope` skips the positional `concurrent` ident then `parse_block`; `spawn` parses its target via `parse_postfix`; `perform` reuses `parse_args`); `.await` is checked right after the `.` in `parse_postfix_rest`, before the field/method dispatch. Five new keyword tags (declassify 36 / perform 37 / scope 38 / spawn 39 / await 40) + `is_kw_*` helpers; `Expr` gains `Declassify`/`Perform([u8],[u8],Args)`/`Scope`/`Spawn`/`Await`. **The `scope` (and `while`) body stays statement-free** until (2c): a body with a `;`-separated statement — and the `;` token itself — is (2c) territory, so the seeds use statement-free bodies. **Verified:** the differential test now diffs **102 seeds** — each form alone + composed (`g(perform E.op(1))`, `declassify(x) + perform E.op()`, a `match` arm `declassify(perform Tls.verify(mac))`, `if ready { spawn w(x) } else { compute().await }`, `scope concurrent { declassify(perform Net.recv()) }`) — all byte-identical to `snc ast`; leak-free under `leaks --atExit`. **1402 tests, four-check green.** **Remaining in (2b):** only `handle … with { arms }` — which closes the expression grammar.
**Phase D self-host port (2/N) parser — (2b) increment-8: `handle` — the EXPRESSION GRAMMAR IS COMPLETE — (`189990e`). ADR 0039 amendment A11.** `handle <body> with { Eff.op(params) => arm, … return v => arm }` → `(handle body (arm Eff op armbody)… (return v body))` (a `parse_atom` keyword case). **Two subtleties, both handled faithfully:** (i) handler-arm **params are parsed but NOT dumped** (skipped to the closing `)`; the dump is just `(arm Eff op body)`); (ii) the optional `return v => body` arm is kept **separate** from the handler-arm list and **dumps LAST** regardless of source position — the arm parse fills a **`&mut Ret` out-param** when it sees `return` (mirroring the Rust `return_arm`). That out-param **assigns an enum through a `&mut` ref** (`*ret = Ret::YesRet(…)`) — the first non-primitive `&mut` assignment in the port (the cursor is `&mut i64`); **de-risked by a probe** (compiles, runs, leak-free; return-not-last verified to still dump last). `Expr` gains `Handle(Expr, HArms, Ret)`; new `HArms = HEnd | HCell([u8],[u8],Expr,HArms)` and `Ret = NoRet | YesRet([u8],Expr)`; three keyword tags (handle 41 / with 42 / return 43). **Verified:** the differential test now diffs **110 seeds** — single/multi-arm handlers, return arms (incl. return-not-last + empty params), and composed forms (`g(handle …)`, `handle perform … with …`) — all byte-identical to `snc ast`; leak-free under `leaks --atExit`. **1402 tests, four-check green.** **(2b) the full expression grammar is COMPLETE** — operators, atoms, calls, postfix, `::` paths, arrays, `if`/blocks, `match`/patterns, struct literals, declassify/perform/scope/spawn/await, handle. **Next: (2c)** statements (`let`/assign/`while`/`break`/`continue`/expr-stmt — turning the statement-free `BlockE` into a real block) + `fn`-with-params/return-type/effect-row; then **(2d)** the top-level decls.
**Phase D self-host port (2/N) parser — (2c-1): statements + real blocks — complete (`b293c62`). ADR 0039 amendment A12.** A block is now `{ <stmt>* <tail> }` → `(block <stmt>… <tail>)`. Statements: `let [mut] name = e` → `(let [mut] name _ e)` (the `_` is the not-yet-supported type annotation — (2c-2)); `target = e` → `(assign target e)`; `while cond { body }` → `(while cond (block …))`; `break` → `(break)`; `continue` → `(continue)`; an expr-statement → `(expr e)`. The block loop (`parse_stmts`) mirrors `parse_block_inner` — dispatch `let`/`while`/`break`/`continue` by keyword tag, else parse an expr and classify by what follows (`=` → assign-stmt, `;` → expr-stmt, else → the tail). **The block tail is a `&mut Expr` out-param** (the A11 `&mut Ret` technique) defaulting to a **nullary `Expr::SynthZero`** that dumps `(int 0)` — the oracle's synth unit tail for a statement-only `while` body. **⚠ Leak found + fixed in-flight:** the default was first `Expr::Int(0)`, whose `i64` payload is heap-**boxed**, and `*tail = ex` through the ref doesn't free the old enum (consistent with A11 — `NoRet` is nullary), so the boxed `Int(0)` leaked once per overwritten tail (2 leaks / 32 bytes). A nullary default is leak-free + dumps identically. **Reusable rule:** a `&mut Enum` out-param's default must be a payload-free variant. `Expr` `BlockE(Expr)` → `Block(Stmts, Expr)` + `SynthZero`; new `Stmts`/`Stmt` enums; the fn body is now a real block. New tokens: `;` (44) + let/mut/while/break/continue (45–49). **Verified:** the differential test now diffs **122 seeds** — let/assign/expr-stmt, `while` with break/continue/assign bodies, statement-only `while` bodies (synth tail), nested statements, composites — all byte-identical to `snc ast`; leak-free under `leaks --atExit`. Existing statement-free seeds unchanged (a zero-statement block dumps identically). **1402 tests, four-check green.** **Next: (2c-2)** `let` type annotations + a `parse_type`; **(2c-3)** `fn` definitions.
**Phase D self-host port (2/N) parser — (2c-2): `let` type annotations + a `parse_type` — complete (`5f47546`). ADR 0039 amendment A13.** The optional `: type` on a `let` → `(let [mut] name <type> e)` (vs `_` when absent). `parse_type` mirrors the Rust one: `secret T` → `(secret T)`; `&T`/`&mut T` → `(ref T)`/`(refmut T)`; `?T` → `(opt T)`; `[T]` → `(arr T)`; `Ident` → the name; `Ident<args>` → `(generic Ident args…)` (the generic arg list is a cons-list terminated by `>`). **Nested generics close without a `>>` split:** the tokenizer has no `>>`, so `Vec<Box<i64>>` lexes its trailing `>>` as two `Gt` tokens and each `>` closes one level (the Rust parser needs an explicit `Shr`-into-two-`>` split; the port sidesteps it). New recursive `TypeE` enum + `TyArgs` cons-list + a `TyOpt` (`NoTy`/`SomeTy`); `Stmt::SLet` gains the `TyOpt` field. New tokens: `?` (Question 50) + the `secret` keyword (51). In `parse_let`, after the name a `:` opens the annotation. **Verified:** the differential test now diffs **135 seeds** — i64/bool idents, `[u8]` arrays, `Vec<i64>`/`Map<i64,[u8]>`/`Box<Vec<i64>>` generics (incl. nesting), `?T`, `&T`/`&mut T`, `secret T`, `secret [u8]`, mixed annotated/un-annotated lets — all byte-identical to `snc ast`; leak-free under `leaks --atExit`. **1402 tests, four-check green.** **Next: (2c-3)** `fn` definitions (params/return-type/effect-row — reusing `parse_type`); then **(2d)** the top-level decls.
**Phase D self-host port (2/N) parser — (2c-3): `fn` definitions — closes the fn-level grammar — complete (`b2a9c3b`). ADR 0039 amendment A14.** `main`'s hard-coded paramless `fn NAME() -> TYPE` header is replaced by a real fn parse: `fn name <type-params>? ( [mut] p: T, … ) -> RET ! { eff, … }? { body }` → `(fn name ((param [mut] p <type>) …) <ret> <block>)`. The param list is a `Params` cons-list; **the param-list dump has no leading space before the first param**, so a first/rest split (`dump_params` + `dump_params_rest` over a shared `dump_param_body`). The `-> RET` return type now routes through `parse_type`/`dump_type` (so a non-`Ident` return like `[u8]`/`?T`/`Vec<T>`/`secret T` dumps right — previously dumped raw via `append_slice`). **Generic type-params `<…>` + the postfix effect row `! { … }` are parsed-and-SKIPPED** — `dump_fn` emits neither (confirmed against `ast_dump.rs`); `skip_type_params` is depth-balanced over `<`/`>`, `skip_effect_row` skips to the `}`. No new tokens. **⚠ Sentinel-`if`-is-an-expression reminder:** `skip_type_params` first used statement-only `if` branches + a bare `if` (no `else`) → compile error ("blocks must end with an expression"); rewrote it as `depth = if … { depth+1 } else if … { depth-1 } else { depth }` inside a `while depth > 0` loop. **Verified:** the differential test now diffs **148 seeds** — params (single/multi/`mut`), `[u8]`/`?T`/`Vec<T>`/`secret` return types, ref params, generic fns (type-params skipped), effect-row fns (skipped), multi-fn programs, and a composite generic+secret+effect+statements handler — all byte-identical to `snc ast`; leak-free under `leaks --atExit`. **This CLOSES the fn-level grammar** — every `Stmt`/`Expr`/`TypeExpr`/`Pattern` + the fn header now parse. **1402 tests, four-check green.** **Next: (2d)** the top-level decls (struct/enum/trait/impl/class/effect/use) + completing `snc ast`'s `Program` dumper for them — the last parser slice.
Phase C2 (regions + refs + mutability + borrow check + RAII drop
per HANDOVER §6.2 / §6.3) is **complete** per ADR 0017 (now
ACCEPTED-WITH-AMENDMENTS, 6 sub-phases, ~6 effective sessions
actual vs ADR 0017 D9 estimate "6-13 sessions" — low end of
the range). ADR 0018 (Polonius migration plan) PROPOSED;
records the plan only, no migration code yet. Phase C3 (effect
system + secret typing per ADR 0019) is **typing-layer
complete** as of C3.3 — ADR 0019 ACCEPTED-WITH-AMENDMENTS with
all D-decisions exercised except D8 (handler runtime, deferred
to ADR 0020) and D9 (async, deferred indefinitely). Phase C3
runtime layer (ADR 0020) is **complete** per ADR 0020 (now
ACCEPTED-WITH-AMENDMENTS, 8 sub-phases C3.4 → C3.7,
~9 effective sessions vs ADR estimate "5-9 sessions"; D2
multi-shot relaxation deferred indefinitely per the
amendment). All twelve D-decisions exercised modulo D2: D1
free-monad lowering, D2 one-shot (multi-shot deferred), D3
deep-handler + propagation, D4 surface + default + non-
identity return arm, D5 AST + parser + resolve, D6 effect
discharge, D7 all 5 runtime symbols, D8 op-arg packing (single
i64 at MVP), D9 sub-phase split, D10 out-of-scope deferrals,
D11 main integration, D12 phase-go (c37_go_no_go +
c37_handle_return + c37_perform_outside_handle). The handler
surface compiles and runs end-to-end for: direct-perform
bodies, fn-call-that-performs bodies (effecting fn ABI returns
Kont*), pure-bodied effecting fns (PURE_RETURN wrap), let-
bound performs, any pure surrounding context with a single
embedded perform (binop, struct-lit, fn-call-arg, index, etc.),
chained effecting lets where each let's RHS is a direct
perform / effecting call, `return v => body` arms that
transform the resumed/returned value per Phase B's deep-
handler re-wrap, nested handles where an inner handle's un-
caught op propagates to the outer handle's dispatch, and
**any-expression handle body** including pure i64 wrapped via
sentinel_kont_pure. Five runtime symbols in place:
sentinel_perform_op, sentinel_kont_resume (bubble-aware,
returns Kont*), sentinel_kont_panic_resumed, sentinel_kont_pure
+ sentinel_kont_consume_pure, sentinel_kont_push. Pipeline at
Phase C3 close (unchanged since C3.3): parse → resolve →
check → **effect_check** → borrow_check → codegen.
Phase C1 (type system per HANDOVER §6.2) is **complete** per ADR
0011 (now ACCEPTED, 8 sub-phases, ADR's honest 5-6 month estimate
beaten — actual elapsed across C1.0a through C1.7.4b was ~10-12
sessions, ~5-6x faster than estimated; the infrastructure
investment compounded). All eight sub-phases landed:

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

  - **ADR 0015** (8924d38, PROPOSED→ACCEPTED-WITH-AMENDMENTS):
    concrete C1.6 surface — `[T]` array type syntax (D1),
    `[e1, e2, ...]` array literal (D2), postfix `a[i]` indexing
    (D3), `len(a) -> i64` builtin (D4), empty array needs
    annotation (D5), `Type::Array(ArrayElem)` flat subset
    representation (D6 — amended to depth-1: NullableInner and
    ArrayElem stay primitive-only, no `?[T]` / `[?T]` at C1.6
    because mutual enum recursion would force Box and break
    Type's Copy), bidirectional element typing (D7), lexer
    additions (D8: `[` + `]`), heap runtime (D9:
    `sentinel_alloc` + `sentinel_panic_oob`, no `free`),
    bounds-check semantics (D10: 0 <= idx < len; panic_oob on
    failure), ADR 0014 D10 unlock implemented (D11: `?Struct`
    codegen switches to heap-indirect `{ i1, ptr }` so
    recursive structs through `?T` work; cycle detector
    relaxes), out-of-scope list (D12: mutable indexing,
    slicing, push/pop, multi-dim, methods, free, ==), fn main
    invariant stays (D13). The ADR 0014 D10 deferral retires
    here.
  - **C1.6.1** (3cfd49f): lexer adds `[` and `]` tokens per
    ADR 0015 D8. Two new TokenKind variants disambiguated by
    the parser into three roles: array type / array literal /
    postfix index. +6 new lexer tests.
  - **C1.6.2-6** (8c5bbbe): the array surface + heap runtime
    + ADR 0014 D10 unlock land end-to-end. sentinel-runtime
    gains `sentinel_alloc` (libc malloc wrapper + abort on
    failure) and `sentinel_panic_oob` (abort with diagnostic).
    AST gains `ExprKind::ArrayLit` + `ExprKind::Index` +
    `TypeExprKind::Array`. Parser handles `[T]` in parse_type,
    `[...]` in parse_atom (with empty-needs-annotation per D5),
    `a[i]` in parse_postfix alongside `.field`. Resolve
    pre-registers `len` builtin at FnId(3); user fns now start
    at FnId(4). Types widens with `Type::Array(ArrayElem)` flat
    subset (D6 amendment — primitives only; no `?[T]` / `[?T]`),
    bidirectional element typing (D7), array literal / index /
    len typing rules (D2/D3/D4), four new TypeError variants
    (AmbiguousEmptyArray, IndexOnNonArray, IndexNotInt,
    NestedArray), and **the cycle-detector relaxation** that
    closes ADR 0014 D10: only direct struct edges contribute to
    cycles; `?Struct` edges break them via heap indirection.
    Codegen: array as `{ i64 len, ptr data }`; ArrayLit lowers
    to alloc+store+insert_value; Index lowers to bounds-check
    + GEP + load (two basic blocks idx_ok/idx_oob); len
    extract_value(0); the `?Struct` representation switches
    from inline `{ i1, T }` to heap-indirect `{ i1, ptr }`;
    `WidenToNullable` for struct types allocates+stores +
    wraps in pointer. +52 tests across all crates. Seven new
    c16_* pass-test fixtures (array_basic, empty_array,
    array_as_arg, array_of_struct, array_in_struct,
    linked_list_node, c16_go_no_go).

  - **ADR 0016** (e411ded, PROPOSED→ACCEPTED): concrete C1.7
    surface — generic fn syntax `fn name<T>(x: T) -> T` (D1),
    generic struct syntax `struct Box<T> { ... }` (D2), type args
    in type position `Box<i64>` (D3), no turbofish at call sites
    (D4) with iterative bidirectional inference, no new lexer
    tokens (D5 — `<` and `>` reused from C1.3 comparisons),
    interned-instance `Type::GenericInstance(GenericInstanceId)`
    representation preserving `Type: Copy` (D6a), monomorphic
    codegen (D7) chosen over witness tables because unbounded
    generics trivialise them to "empty", builtins typing routes
    through the unified inference path (D8a) but codegen stays
    special (D8b — no Sentinel-1.7 source bodies for force-
    unwrap / pattern-matching / runtime-metadata extraction),
    resolve-side type-param scoping with DuplicateTypeParam
    diagnostic (D9), out-of-scope list (D10: bounds, lifetimes,
    HKT, const generics, turbofish, generic methods), `fn main`
    not generic (D11), Pair<A,B> phase-go (D12).
  - **C1.7 scaffolding** (c1e5083): AST + parser + resolve
    infrastructure. AST gains `TypeParam` struct, `type_params:
    Vec<TypeParam>` on `FnDef` / `StructDecl`, and
    `TypeExprKind::Generic { name, args }`. Parser gains
    `parse_type_params` / `parse_type_args` helpers and the
    `Ident<...>` branch in `parse_type`. Resolve gains
    `TypeParamId`, `ResolvedTypeParam`, `DuplicateTypeParam`
    error, and `FnSignature.type_params_count` (builtins flagged
    as generic with count=1). +19 parser tests + 7 resolve tests
    = 777 total.
  - **C1.7.4a** (d32a9fe): types crate generic-fn typing +
    builtin re-route. New `Type::TypeParam(TypeParamId)` +
    matching variants on `NullableInner` / `ArrayElem`. New
    helpers `Type::substitute`, `try_substitute`,
    `contains_type_param`. Builtin signatures rewritten with
    real `Type::TypeParam` (`unwrap_or<T>(?T, T) -> T`,
    `is_some<T>(?T) -> bool`, `len<T>([T]) -> i64`); the
    ~75 LOC of special-cased Call branches in check_expr
    collapse into one unified `check_call`. Iterative
    bidirectional inference handles null literals via
    fixed-point typing (`unwrap_or(null, 0)` works:
    arg[1]=0→I64 binds T=I64; arg[0]=null re-checked with
    expected `?I64`). `TypedExprKind::Call` gains
    `type_args: Vec<Type>`. New error variants: GenericMain,
    AmbiguousTypeArg, TypeArgInferenceConflict,
    GenericStructNotYetSupported (placeholder for C1.7.4b).
    Codegen: skip generic user fn declarations + bodies
    + emit `CodegenError::GenericCallNotYetSupported` at
    user generic-fn call sites pending C1.7.5. +12 types
    tests = 789 total.
  - **C1.7.5** (ad7e10d): codegen monomorphization for user
    generic fns. `TypedFnDef::substitute` deep-clones a generic
    fn with TypeParams substituted to concrete types — the
    monomorphic def looks no different from a non-generic fn
    to compile_fn. Worklist algorithm
    (`collect_mono_instantiations`) walks non-generic fn bodies
    seeding instantiations, then transitively processes each
    pending instance under its substitution. Per-instance LLVM
    fn declaration with mangled name (`id__i64`, etc.) via
    `mangle_mono_name` + `mangle_type`. Builtin lowering stays
    inline per ADR 0016 D8b. Four new c17 fixtures
    (c17_id stdout "42", c17_two_instantiations "41",
    c17_generic_nullable "100", c17_generic_array "6"). +4
    pass tests = 793 total.
  - **C1.7.4b** (2c6c652): generic structs end-to-end + ADR
    0016 D12 phase-go. New `Type::GenericInstance(GenericInstanceId)`
    variant + interner table on TypedProgram. NullableInner /
    ArrayElem gain `GenericInstance` variants (partially
    closing the ADR 0015 D6 deferral: `?Box<i64>` and
    `[Box<i64>]` now work; `?[T]` and `[?T]` stay deferred).
    `Type::substitute` extended to take `&mut Vec<GenericInstanceData>`
    for interner-extending substitution. Same threading
    through TypedFnDef/Block/Stmt/Expr::substitute. New
    `check_call` extensions: unify_one recurses into
    GenericInstance args; bidirectional pushdown extended for
    generic-instance returns (so `fn make_pair<A, B>(...) -> Pair<A, B>
    { Pair { ... } }` works). Codegen pass 0 splits into
    declare-then-set-bodies passes for both regular structs and
    generic-struct instances, with abstract instances
    (TypeParam-using args) filtered out via
    `arg_contains_typeparam`. Two new fixtures (c17_box stdout
    "42", c17_go_no_go stdout "42" — the full ADR 0016 D12
    Pair<A,B> + make_pair / fst / snd / pick_int program). +5
    types tests + 2 pass tests = 798 total.

  - **C1.7 docs commit** (4028dd7): STATE.md banner refresh +
    HANDOVER §0 close-out + ADR 0011/0016 flips to ACCEPTED.

  - **ADR 0017** (ea4bcfd, **ACCEPTED-WITH-AMENDMENTS at
    C2.5 close**): Phase C2 kickoff. 14 D-decisions covering
    reference syntax (`&T` / `&mut T` per D1), mutability
    (`let mut` + `mut` param prefix per D2), borrow-take +
    dereference (`&expr` / `&mut expr` / `*expr` per D3 / D4),
    lvalue / rvalue distinction (D5), borrow-checker formulation
    (lexical first, Polonius later per D6), region representation
    (lexical only at C2 minimum per D7; named regions deferred),
    drop / RAII (auto-drop at scope exit + `sentinel_free` per
    D8 closes the C1.6+ heap-leak deferral), move semantics +
    use-after-move (D9), lexer additions (D10), interned
    `Type::Ref(RefId)` (D11 — the fifth ADR running to preserve
    `Type: Copy` via internment), out-of-scope items (D12),
    `fn main` invariant (D13), and phase-go program spec (D14).
    Sub-phase split table: C2.0 (infrastructure) → C2.1
    (shared borrow checker) → C2.2 (`&mut` + XOR — the largest)
    → C2.3 (move semantics) → C2.4 (RAII + drop) → C2.5
    (Polonius migration plan + STATE.md / HANDOVER close-out).
    Three amendments at C2.5: A1 the C2.4 recursive-field-drop
    closure slipped to C2.5(a); A2 the Polonius plan shipped as
    standalone ADR 0018; A3 partial-move-through-field-projection
    soundness gap documented for closure in a follow-on sub-phase.

  - **ADR 0018** (PROPOSED): Polonius migration plan — lexical
    → flow-sensitive borrow check via `polonius-engine 0.13`.
    Six D-decisions: D1 (trigger: empirical friction not
    principle); D2 (preserved surface: BorrowError variants +
    DropPlan + pipeline shape stay); D3 (adopt polonius-engine
    library; vendor-fork fallback); D4 (representation changes:
    CFG + origins + loans + liveness); D5 (three-step rollout:
    fact generator → output lowering → flip default); D6 (out-
    of-scope: field-precise borrows + first-class refs +
    closures + traits). No migration code at C2.5; ADR records
    the plan only.

  - **C2.5** (this session): Phase C2 close-out. Four
    deliverables: (a) recursive struct + generic-instance
    field drop closes the C2.4 known gap — `emit_drop_struct_fields`
    now threads `&TypedProgram` and iterates `program.struct_decl(id).fields`
    (substituting through `program.generic_instance(id).args` for
    generic instances), with a `field_type_needs_drop(ty, program)`
    helper short-circuiting pure-data fields and a cycle guard
    for `?Struct` recursion. (b) ADR 0018 ships the Polonius
    migration plan. (c) `docs/borrow-check-limitations.md`
    documents two known lexical over-rejections + a soundness
    gap (partial move through field projection + drop ⇒ double-
    free). (d) STATE.md banner + HANDOVER §0 + ADR 0017 flip.
    +4 c25 fixtures: c25_struct_field_drop (stdout "19"),
    c25_nested_struct_drop ("607"), c25_generic_struct_array_drop
    ("66"), c25_go_no_go ("190" — the D14 phase-go combining
    XOR + move + recursive field drop). Workspace test count
    935.

  - **C2.0.1** (d7b18c2): lexer adds `&` (Amp) token + `mut`
    keyword per ADR 0017 D10. The `*` token already exists for
    multiplication from C0; per D10 the parser disambiguates
    dereference vs multiplication positionally at C2.0.2. No
    `'a` lifetime syntax at C2 minimum per D7. logos longest-
    match handles `&&` (AmpAmp) staying a single token. +10
    new lexer tests = 808 total.

  - **C2.4** (8d72679): RAII / drop + `sentinel_free` runtime
    symbol per ADR 0017 D8. **Closes the C1.6+ heap-leak
    deferral** that's been open since arrays + `?Struct`
    payloads landed. Auto-drop at scope exit for un-moved heap-
    backed bindings. Integration with C2.3 via new `DropPlan`
    artifact from borrow checker — per-fn `BTreeMap<FnId,
    BTreeSet<VarId>>` of moved-source VarIds. Salsa pipeline
    becomes parse → resolve → check → borrow_check (returns
    `Option<DropPlan>`) → codegen (consumes DropPlan).
    sentinel-runtime gains `sentinel_free(ptr)` (libc free
    wrapper). CodegenCtx gains `scope_stack`, `current_fn_id`,
    `free_fn`, `drop_plan` field. compile_fn / lower_block push/
    pop scopes; emit_scope_drops fires at exit (reverse decl
    order; skips moved + tail-returned). emit_drop_for_binding
    dispatches on Type: Array → free data ptr; ?Struct → cond-
    branch free payload if valid; primitive / ref → no-op.
    Struct field recursive drops DEFERRED (known gap; closes
    at C2.5). FnId / VarId gain PartialOrd + Ord. +4 c24
    fixtures: c24_array_dropped (24), c24_moved_array_no_double_free
    (66), c24_nested_blocks_drop (10), c24_go_no_go (160 —
    phase-go). +2 borrow-check unit tests (DropPlan), +2
    runtime tests (sentinel_free).

  - **C2.3** (50c826b): move semantics + use-after-move
    detection per ADR 0017 D9. Adds `FnCtx.moved: HashMap<VarId,
    Span>` tracking which bindings have been consumed +
    `is_copy_type(ty)` classification (Copy = primitives + refs +
    ?Copy-inner; Move = struct / array / generic-instance /
    ?Move-inner / TypeParam-conservative). Var(x) reads in
    CONSUMING context transition Live → Moved; subsequent reads
    surface `BorrowError::UseAfterMove` with three-label miette
    diagnostic (decl_span / move_span / use_span). Non-consuming
    contexts (postfix receivers `p.field` / `xs[i]`, lvalue
    operands `&x` / `&mut x`, runtime-builtin call args `len(xs)`
    / `is_some(x)` / `unwrap_or(x, d)` / `print(x)`) check
    use-after-move without transitioning. Branch-aware merge at
    if/else: snapshot before each branch + restore between +
    merge after with "moved in either branch → moved after" —
    this is what makes `if c { fst(p) } else { snd(p) }` accept.
    c17_go_no_go fixture updated for move semantics (previous
    `pick_int(true, p) + pick_int(false, p)` double-used p; now
    constructs p1 + p2). +12 borrow-check unit tests + 4 driver
    pass-test fixtures (c23_move_struct / c23_branch_isolation /
    c23_array_move / c23_go_no_go). c23_go_no_go runs: stdout
    "100\n", exit 0 (Account struct, transfer moves src + dst,
    balance_of in if/else).

  - **C2.2** (4a0ca92): `&mut T` + shared-XOR-mutable rule per
    ADR 0017 D6. Extends sentinel-borrow-check with place-
    tracking + transient/rooted borrow lifetimes + five new
    `BorrowError` variants. The XOR invariant: at any point a
    place P is either (a) borrow-free, (b) has N ≥ 1 shared
    borrows, or (c) has exactly one mutable borrow.
    Per-source-VarId `PlaceState { shared: Vec, mut_borrow:
    Option }` in FnCtx; each `&x` / `&mut x` adds a
    `BorrowInstance` with lifetime `Transient` (default) or
    `UntilScope(depth)` (rooted in a ref-typed let).
    `clear_transients()` runs after every stmt — this is what
    keeps c20_go_no_go's `add(&a, &b);` + `increment(&mut a);`
    valid (shared borrows from `add` are transient + die
    before `increment`'s `&mut a` is taken).
    `promote_transients(depth)` rooting fires at ref-typed
    `let r = &x;` or equivalent assign — promotes any new
    transients to live until the binding's scope pops.
    `BorrowSource::Incoming` now carries a VarId payload so
    place-tracking routes conflicts through the param's place
    key. New `walk_assign_target` (LHS lvalue walk + write-
    conflict check on Var leaves) + `walk_expr_lvalue` (`& x`
    / `&mut x` operand walk; no read-check on inner Var). Five
    new error variants: MutableBorrowOfShared (& then &mut),
    SharedBorrowOfMutable (&mut then &), BorrowConflict (two
    &mut), WriteWhileBorrowed (`x = v;` while borrowed),
    ReadWhileMutBorrowed (reading `x` while &mut x active).
    +14 borrow-check unit tests + 4 driver pass-test fixtures
    (c22_multi_shared / c22_scoped_mut / c22_transient_then_mut /
    c22_go_no_go). c22_go_no_go runs: stdout "35\n", exit 0
    (shared block computes 20; mut block increments x to 15;
    35 total).

  - **C2.1** (64edf3d): shared-only lexical borrow checker per
    ADR 0017 D6. New crate `sentinel-borrow-check` (~600 LOC
    including tests). New salsa-tracked query `borrow_check_query`
    chains on `check_query`; the pipeline becomes parse_query →
    resolve_query → check_query → borrow_check_query → codegen
    with diagnostics accumulating transitively. Driver wires the
    gate: a borrow failure blocks codegen + exits with code 1.
    Two `BorrowError` variants at C2.1: `OutlivesSource`
    (canonical use-after-scope) and `ReturnsLocalRef` (a fn
    returns a `&T` whose source is fn-local — per ADR 0017 D7's
    "second-class refs" rule). Borrow-source representation is
    a bounded enum `{ Local(VarId), Incoming, LocalAnonymous }`;
    the analysis is per-fn with limited inter-procedural
    reasoning (a call returning a ref inherits the most-
    restrictive source among its ref args). Inner blocks push/
    pop scopes; let-stmts record source from RHS before declaring
    the binding; assign-stmts to ref-typed Vars update the
    recorded source. ADR 0017 D6's lexical-first formulation is
    now exercised; D7's second-class-refs rule enforced.
    +15 borrow-check unit tests + 4 driver pass-test fixtures
    (c21_borrow_local_ok / c21_pass_through_ref / c21_reborrow /
    c21_go_no_go). c21_go_no_go runs: stdout "168\n", exit 0
    (`sum_two(&a, &b) + triple(&a) + triple(&b)` = 42 + 30 + 96).

  - **C2.0.2** (9516ebb): bundled AST + parser + resolve +
    types + codegen for refs end-to-end per ADR 0017 D1-D5 +
    D11. AST gains `UnaryOp::Ref` / `UnaryOp::RefMut` /
    `UnaryOp::Deref`; `TypeExprKind::Ref { mutable, inner }`;
    `StmtKind::Let.mutable`; `StmtKind::Assign { target, value }`;
    `Param.mutable`. Parser handles `&T` / `&mut T` (with
    whitespace tolerance), `&expr` / `&mut expr` / `*expr`
    prefix unaries, `let mut`, `mut` params, and assignment
    statements (after parsing an expression at stmt position,
    a following `=` triggers Assign-statement parsing).
    Resolve passes the new variants through with `mutable`
    bits threaded on ResolvedParam / ResolvedStmtKind::Let;
    new ResolvedStmtKind::Assign. Types adds `Type::Ref(RefId)`
    + `RefData { mutable, inner }` + `intern_ref` per ADR
    0017 D11, mirroring the C1.7.4b GenericInstance interner
    pattern (keeps `Type: Copy`). `NullableInner` gains `Ref`
    for `?&T`; ArrayElem stays primitive-only (refs in arrays
    rejected at parse-array-type time with `RefInArray`).
    Type::substitute extended to recurse through Ref (clone,
    substitute inner, re-intern). unify_one extended for Ref
    (mutability match + inner recursion — enables generic+ref
    inference). VarTypeEnv becomes `HashMap<VarId, (Type,
    bool)>` to track mutability. New TypeError variants:
    NestedRef, RefInArray, RefInStructField, BorrowOfRvalue,
    AssignToRvalue, AssignToImmutable, BorrowMutOfImmutable,
    DerefOfNonRef, AssignThroughSharedRef,
    IndexAssignNotSupported. check_expr dispatches Unary
    Ref/RefMut/Deref: Ref requires lvalue; RefMut requires
    mutable lvalue; Deref requires Type::Ref operand and
    returns its inner. check_stmt's Assign arm validates LHS
    is a mutable lvalue (recursive through field-access),
    pushes target.ty down to RHS for widening, and Mismatch's
    on type disagreement. Codegen lowers refs as LLVM opaque
    pointers (LLVM 15+ no-typed-pointer). New
    `lower_lvalue_ptr` helper handles Var → alloca ptr, `*r` →
    load r's value (the ptr), `p.field` → struct_gep into the
    field; assignment and `&` / `&mut` both delegate through
    it. `*r` lowers as load-from-pointer of the inner type
    (looked up via `program.refs[id].inner`). Tests: +62 (870
    total) — +8 ast, +23 syntax, +21 types, +6 codegen, +4
    driver pass-test fixtures (c20_ref_basic / c20_mut_basic /
    c20_deref_basic / c20_go_no_go). c20_go_no_go runs:
    stdout "53\n", exit 0 (the full ADR 0017 D14 program with
    `add(&a, &b)` shared-borrows + `increment(&mut a)`
    exclusive-borrow + `let mut a` + deref-assignment + print).

**Phase C2 closes at C2.5.** All six sub-phases shipped
(C2.0.1 + C2.0.2 + C2.1 + C2.2 + C2.3 + C2.4 + C2.5 — seven
feat/docs commits across ~6 effective sessions vs the ADR 0017
D9 estimate "6-13 sessions across 5-6 sub-phases" — low end
of the range). ADR 0017 ACCEPTED-WITH-AMENDMENTS; ADR 0018
(Polonius migration plan) PROPOSED. Two C2 follow-ons
documented but deferred: (a) Polonius migration per ADR 0018
(trigger: empirical friction, not a calendar date); (b)
per-(VarId, FieldPath) move state to close the documented
partial-move-through-field-projection soundness gap (highest-
priority post-C2 work; see `docs/borrow-check-limitations.md`).

**Next: Phase C3** — effect-system integration from Phase B
Sentinel-Mini per HANDOVER §6.2. Pre-flight: write ADR 0019
PROPOSED covering effect-row representation in sentinel-types
+ effect annotations + inference vs annotation + `secret T`
qualifier promotion + handler lowering. See §0.2 for the
patterns to argue.

**Workspace test count**: 935 active across all crates (+1
doctest at sentinel-broker; +4 over C2.4: +4 driver pass-test
fixtures (c25_*)). All four check-suite checks green. c05 go/
no-go (C1.3 bool flow) runs: stdout "10", exit 0. c14 go/no-go
(C1.4 struct flow) runs: stdout "7", exit 0. c15 go/no-go (C1.5
nullable flow) runs: stdout "142", exit 0. c16 go/no-go (C1.6
array flow) runs: stdout "15", exit 0. c17 go/no-go (C1.7
generics flow — updated for move semantics) runs: stdout
"42", exit 0. c20 go/no-go (C2.0.2 refs+mut+assign flow) runs:
stdout "53", exit 0. c21 go/no-go (C2.1 shared-borrow flow)
runs: stdout "168", exit 0. c22 go/no-go (C2.2 XOR alternation
flow) runs: stdout "35", exit 0. c23 go/no-go (C2.3 move
semantics) runs: stdout "100", exit 0. c24 go/no-go (C2.4 RAII
/ drop) runs: stdout "160", exit 0. **c25 go/no-go (C2.5 D14
— XOR + move + recursive field drop on Bag-with-array) runs:
stdout "190", exit 0.** See STATE.md "Conventions" for the
per-crate breakdown.

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
  - 0011 phase-c1-kickoff-and-type-system-plan   ACCEPTED — all 12
                                                 D-decisions exercised
                                                 across C1.0 through
                                                 C1.7. D6's eight-
                                                 sub-phase budget is
                                                 closed (every C1.x
                                                 done). D12's perf
                                                 discipline measured:
                                                 sub-second cold
                                                 builds + sub-100ms
                                                 incremental rebuilds
                                                 on the current
                                                 corpus.
  - 0012 concrete-c1-surface-syntax              ACCEPTED — every
                                                 D-decision exercised
                                                 across C1.2-3
  - 0013 concrete-c1-4-struct-syntax             ACCEPTED — every
                                                 D-decision exercised
                                                 across C1.4
  - 0014 concrete-c1-5-nullable-syntax           ACCEPTED — D1-D11
                                                 all fully exercised:
                                                 D1-D9 + D11 at C1.5;
                                                 D10 retires at C1.6
                                                 via ADR 0015 D11
                                                 (`?Struct` heap
                                                 indirection unlocks
                                                 the recursive-
                                                 struct relaxation).
                                                 D4 representation
                                                 stays as the flat
                                                 NullableInner subset
                                                 enum (C1.5
                                                 amendment)
  - 0015 concrete-c1-6-array-syntax              ACCEPTED-WITH-
                                                 AMENDMENTS — D1-D5
                                                 + D7-D13 all fully
                                                 exercised; D6 amended
                                                 (NullableInner +
                                                 ArrayElem stay
                                                 primitive-only,
                                                 deferring `?[T]` and
                                                 `[?T]` to a future
                                                 ADR; C1.7.4b partially
                                                 closes by adding
                                                 GenericInstance
                                                 variants — `?Box<i64>`
                                                 and `[Box<i64>]` work,
                                                 but `?[T]` and `[?T]`
                                                 stay deferred); D11
                                                 implementation
                                                 closes ADR 0014 D10
  - 0016 concrete-c1-7-generics-syntax           ACCEPTED — all 12
                                                 D-decisions exercised
                                                 cleanly across the
                                                 C1.7 scaffolding +
                                                 4a + 5 + 4b commits.
                                                 No amendments — each
                                                 D-decision survived
                                                 implementation as
                                                 drafted.
  - 0017 phase-c2-kickoff-and-region-plan        ACCEPTED-WITH-
                                                 AMENDMENTS — all
                                                 14 D-decisions
                                                 exercised across
                                                 C2.0.1 + C2.0.2 +
                                                 C2.1 + C2.2 +
                                                 C2.3 + C2.4 +
                                                 C2.5. Three
                                                 amendments at
                                                 C2.5: A1 the
                                                 C2.4 recursive-
                                                 field-drop
                                                 closure slipped
                                                 to C2.5(a); A2
                                                 the Polonius
                                                 plan shipped as
                                                 standalone ADR
                                                 0018; A3 the
                                                 partial-move-
                                                 through-field-
                                                 projection
                                                 soundness gap
                                                 documented for
                                                 closure in a
                                                 follow-on sub-
                                                 phase.
  - 0018 polonius-migration-plan                 PROPOSED —
                                                 documents the
                                                 lexical → flow-
                                                 sensitive
                                                 borrow-check
                                                 migration via
                                                 polonius-engine
                                                 0.13. Six D-
                                                 decisions: D1
                                                 trigger
                                                 (empirical
                                                 friction); D2
                                                 preserved
                                                 surface
                                                 (BorrowError
                                                 variants +
                                                 DropPlan stay);
                                                 D3 adopt
                                                 polonius-engine;
                                                 D4 representation
                                                 changes (CFG +
                                                 origins + loans
                                                 + liveness); D5
                                                 three-step
                                                 rollout; D6 out-
                                                 of-scope items.
                                                 No migration
                                                 code at C2.5;
                                                 ADR records the
                                                 plan only.
  - 0019 phase-c3-kickoff-and-effects-plan       ACCEPTED-WITH-
                                                 AMENDMENTS — all
                                                 14 D-decisions
                                                 exercised across
                                                 C3.0 + C3.1 +
                                                 C3.1b + C3.2 +
                                                 C3.3. Two
                                                 amendments at
                                                 C3.3: A1
                                                 SecretEscapesPolymorphism
                                                 subsumed by
                                                 monomorphic
                                                 generics +
                                                 SecretFlow-via-
                                                 Mismatch; A2
                                                 runtime builtins
                                                 declared effect-
                                                 free. D3 (RowId
                                                 interner) and D8
                                                 (handler runtime)
                                                 both deferred —
                                                 RowId becomes
                                                 useful when
                                                 handler runtime
                                                 lands at ADR
                                                 0020; D9 (async)
                                                 deferred
                                                 indefinitely.
  - 0020 phase-c3-handler-runtime                ACCEPTED-WITH-
                                                 AMENDMENTS — all
                                                 twelve D-decisions
                                                 exercised modulo
                                                 D2 across eight
                                                 sub-phases (C3.4
                                                 + C3.5(a/b/c/d/e)
                                                 + C3.6(a/b) +
                                                 C3.7). The
                                                 amendment: D2's
                                                 multi-shot
                                                 relaxation is
                                                 deferred
                                                 indefinitely;
                                                 one-shot suffices
                                                 for the bootstrap.
  - 0021 phase-c4-kickoff                        PROPOSED — Phase
                                                 C4 umbrella
                                                 (classes + traits
                                                 + delegation +
                                                 structured
                                                 concurrency). 14
                                                 D-decisions; six
                                                 sub-phases C4.0
                                                 → C4.5; 8-12
                                                 sessions estimate.
                                                 C4.0 + C4.1 done;
                                                 flips at C4.5
                                                 close.
  - 0022 concrete-c4-1-class-syntax              ACCEPTED-WITH-
                                                 AMENDMENTS — all
                                                 eleven D-decisions
                                                 exercised across
                                                 C4.1 (1/N) AST +
                                                 parser and C4.1
                                                 (2/N) resolve /
                                                 types / codegen +
                                                 method-call + init
                                                 call + close-out.
                                                 Two amendments at
                                                 C4.1 close: A1 D4
                                                 definite-assignment
                                                 ships as a flat
                                                 any-assigned check
                                                 (branch-aware
                                                 merge +
                                                 InitFieldReadBeforeAssign
                                                 deferred); A2 D8
                                                 general `Self` in
                                                 type position
                                                 deferred (only
                                                 positional `self:
                                                 &Self` via
                                                 parse_self_param).
  - 0023 concrete-c4-2-trait-impl-syntax         ACCEPTED-WITH-
                                                 AMENDMENTS —
                                                 flipped at C4.2
                                                 (2/N) close. All
                                                 twelve D-decisions
                                                 landed modulo
                                                 three amendments:
                                                 A1 D5 Path 3
                                                 (bounded-generic
                                                 dispatch via
                                                 witness tables)
                                                 DEFERRED — needs
                                                 `<W: Writer>`
                                                 surface (parser +
                                                 AST + resolve +
                                                 types + codegen
                                                 monomorphisation
                                                 extension); A2
                                                 D9 witness-table
                                                 values not emitted
                                                 (scaffolding for
                                                 Path 3); A3 D7
                                                 `Type::TraitSelf`
                                                 interner SHIPPED
                                                 but unused
                                                 (params/returns
                                                 don't reference
                                                 Self, only
                                                 positional via
                                                 self_kind —
                                                 mirrors C4.1 A2).
                                                 Implementation
                                                 footprint: ~2
                                                 sessions across
                                                 C4.2 (1/N) + (2/N)
                                                 — low end of the
                                                 ADR 0021 D14
                                                 estimate.
  - 0024 c4-4-structured-concurrency             PROPOSED — C4.4
                                                 surface
                                                 (scope / spawn /
                                                 await) + Async
                                                 built-in effect +
                                                 thread-per-spawn
                                                 runtime scheduler.
                                                 Twelve D-decisions.
                                                 C4.4 (1/N) parser
                                                 layer done; C4.4
                                                 (2/N runtime)
                                                 done (5 new C-ABI
                                                 symbols + Task +
                                                 ScopeCtx structs).
                                                 What remains: types
                                                 + codegen + phase-
                                                 go fixture. ADR
                                                 flips at C4.4
                                                 (2/N) close.

### 0.1 Working norms (carry forward into Phase C3)

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

### 0.2 Next session opening (Phase C5 kickoff)

> **Phase C4 is COMPLETE — all of C4.0 through C4.5 shipped;
> ADR 0021 ACCEPTED-WITH-AMENDMENTS.** C4.4 (2/N) landed the
> structured-concurrency types + codegen + phase-go (ADR 0024);
> C4.5 closed Phase C4 with the combined full-surface phase-go
> (`tests/pass/c4_go_no_go.sentinel` — class + trait + impl +
> delegation + scope/spawn/await, exit 42) + `c4_named_impl`
> (two named impls co-existing) + the ADR 0021 flip. The
> detailed C4.4 (2/N) punch list retained below is HISTORICAL —
> every item shipped; do NOT re-do it.
>
> **Resume at Phase C5 kickoff** per HANDOVER §6.2 (NOT "Phase
> D" — the roadmap after C4 is C5 per ADR 0021's close): broker
> integration, cross-process safety, reproducible-build
> guarantees, stable ABI definition, LSP/tooling polish, plus
> **actors** (deferred from C4 per ADR 0021 D10) + constant-time
> secret codegen (deferred from C3 per ADR 0019 D12). Sentinel's
> 1.0 release is at C5 close. **ADR 0025 PROPOSED is drafted**
> (`docs/decisions/0025-phase-c5-kickoff-and-productionization-plan.md`)
> — 14 D-decisions + an 8-sub-phase split (C5.0–C5.8). **C5.0 is
> COMPLETE:** the go/no-go program is CHOSEN — a single-process,
> single-file TLS 1.3 handshake (ADR 0025 `## C5.0 resolution`) — which
> pins **D6 (cross-process) → post-1.0** and **D9 (modules) →
> post-1.0** (the handshake is self-contained + fits one file) and
> forces the headline constant-time `secret` capability (D3) an HTTP
> server would leave unexercised; **D11** test infra landed (`cargo
> nextest` + `insta` full-diagnostic UI snapshots, replacing the ad-hoc
> `stderr.contains` checks); and the **D8** reproducible-build audit
> found the C0–C4 build already byte-identical across independent `snc`
> processes (locked in by `crates/sentinel-driver/tests/repro.rs`).
> **ADR 0026 PROPOSED is drafted** (the C5.1/C5.2 HIR/MIR pipeline +
> constant-time secret codegen surface — 10 D-decisions, 4-sub-phase
> split C5.1a→C5.2b). **C5.1a (1/N) is DONE** — the thin HIR seam
> (`lower_to_hir`; codegen consumes `HirProgram`), behaviour-preserving
> (1195 tests + repro byte-identical). **The D3 escape hatch is then
> INVOKED** (codegen has ~295 `TypedExprKind` refs across 90 signatures —
> a thick-HIR migration is multi-session/high-risk and not needed for the
> 1.0 constant-time capability): codegen STAYS on the typed program (via
> the seam), the thick HIR desugar + codegen migration go **post-1.0**,
> and **C5.1a closes at the seam**. **C5.1b (1/N) is then DONE** — the
> `sentinel-mir` data model: a minimal SSA/CFG (`MirProgram` /
> `MirFunction` / `MirBlock` with SSA block-params / `MirOp` /
> `MirTerminator`), every value carrying its `Type` so secrecy reads off
> `Type::Secret(_)` (`is_secret`), the three D5 sinks representable, the
> rest via `Opaque`; additive (nothing consumes MIR yet). **C5.1b (2/N)
> is then DONE** — `lower_to_mir` lowers typed fn bodies → MIR SSA
> (`if`/`&&`/`||` → `Branch` + merge block-params; a variable reassigned
> on one arm threaded through a merge param; the three D5 sinks precise —
> `Branch` cond / `Load` index+address / `Binary(Div)` — and `declassify`
> → `MirOp::Declassify`, the rest `Opaque`); top-level fns only; still
> additive (nothing consumes MIR yet). **C5.2b (1/N) is then DONE** — the
> D5 verification pass `verify_constant_time(&MirProgram)` (type-directed
> taint: each value's `Type` already carries the fixpoint, so the pass
> reads `is_secret` and inspects the branch/load/div sinks →
> `sentinel::mir::secret_leak`), additive (not yet driver-wired).
> **C5.2b (2/N) is then DONE** — D5 is wired into `snc` (after
> `check_query`: `lower_to_mir` → `verify_constant_time` → render
> `secret_leak` + exit 1, gating codegen) with the `c52_secret_leak` (UI)
> and `c52_secret_ct` (branch-free masked-select, pass) fixtures; the c51
> bar holds (`repro.rs` byte-identical). **The C5.2a/D4 open question is
> resolved (with the developer): defer D4 and do bitwise operators next**
> — the go/no-go's constant-time `Finished` MAC verify is an XOR-accumulate
> compare that needs `^`/`|`, and the surface has none. **C5.3 (1/N) lexer
> is DONE** (`Pipe`+`Caret` tokens, additive). **C5.3 (2/N) is then DONE**
> — `& | ^` end-to-end (AST `BinOp` + parser precedence + codegen
> and/or/xor; types/MIR/D5 unchanged — `Binary` is op-generic), with
> `c53_bitwise` + `c53_ct_eq` (the real constant-time MAC-verify shape
> passing D5); **ADR 0027 → ACCEPTED-WITH-AMENDMENTS** (A1: `<< >> ~`/C5.4
> deferred). **The fork is resolved (with the developer): broker
> integration next (C5.4, ADR 0028 PROPOSED).** **C5.4 (1/N) is DONE** —
> the broker-arena substrate: a public broker raw-bytes API
> (`Arena::alloc_bytes` → `NonNull<u8>`, exposing the strategy's
> `alloc_raw`) + runtime C-ABI `sentinel_arena_enter`/`_alloc`/`_exit` on
> a process-wide lazy `Broker` (bump arenas; `_exit` destroys → frees the
> backing buffer). Additive + c51-safe (codegen untouched → objects
> byte-identical). Refinements vs the ADR: not "runtime-only" (a small
> broker API was added), and the malloc-replacement framing (slab pool /
> ptr→handle registry) was dropped — the broker is arena-, not malloc-,
> shaped. **C5.4 (2/N) is then DONE — Phase C5.4 closes; ADR 0028 →
> ACCEPTED-WITH-AMENDMENTS.** The scope→arena codegen routes a scope's
> non-escaping primitive array-literal heap buffers into a per-scope
> broker bump arena and replaces that scope's per-binding `sentinel_free`s
> with one `sentinel_arena_exit`; escaping/moved/returned values stay on
> `sentinel_alloc` (libc). A `compute_arena_routed` pre-pass produces a
> single `HashSet<VarId>` = exactly `emit_scope_drops`'s free set
> (`∉ moved ∧ ≠ tail_returned_var(&block.tail)`), narrowed to
> `let x = [i64/i32/bool array literal]` in non-generic non-effecting fns;
> that one set drives both alloc-routing and free-skip, and the per-scope
> arena handle lives in a new `ScopeFrame` created lazily. UAF-safety came
> from reasoning (routed ⊆ proven-non-escaping free set) + disassembly +
> the c24/c25 guards + `c54_scope_arena`. **Amendment A2: the prep's
> "verified UAF hole" was wrong** — a tail-returned array IS in
> `moved_sources` (the borrow checker walks the tail `Var` as a consuming
> move before the L626 snapshot; empirically dumped), so `∉ moved` alone
> already excludes returned arrays (both checks kept anyway to mirror
> `emit_scope_drops`). **Resume at a developer-scope call:** assemble the
> TLS 1.3 go/no-go (constant-time compare + scope arenas both in hand), or
> another C5 sub-phase — stable ABI (ADR 0025 D7) or LSP (D10). Deferred
> (post-1.0): per-scope arena *sizing*, routing in methods/generics/
> effecting fns, `scope budget` surface, full escape analysis; the bitwise
> shift wave (`<< >> ~`, ADR 0027 A1) + D4 constant-time emission + the
> ADR 0026 flip remain deferred follow-ons.
>
> **Available C4 follow-ons** (none blocking C5): work-stealing
> scheduler (ADR 0024 A1), scope cancellation (A2), `Task<T>`
> for T≠i64 + non-i64 spawn args (A3), explicit `Task<T>`
> type-position annotations via threading `tasks` through
> `resolve_type_expr` (A5), Path-3 bounded-generic dispatch
> (ADR 0023 A1), Polonius migration (ADR 0018), and the
> partial-move-through-field-projection soundness gap
> (`docs/borrow-check-limitations.md`).
>
> Historical C4.4 (2/N) punch list (all shipped) follows.

**Prior-art reference**: the prior session attempted the
types+codegen layer end-to-end and rolled back the partial
work (kept only the runtime scaffolding). Lessons learned + a
known sticking point for the next attempt are captured below.

**C4.4 (2/N types) deliverables** (per ADR 0024 D4 + D5):

  - Add `Type::Task(TaskId)` as the tenth interner-table-
    style variant (preserve `Type: Copy + Hash`); add
    `TaskData { result_ty: Type }`; add `intern_task` helper
    matching `intern_kont`'s shape.
  - Add `tasks: Vec<TaskData>` to `TypedProgram`. Thread
    `tasks: &mut Vec<TaskData>` through `check_expr` and its
    callees — there are many call sites; a sed/python pass
    after `konts` is the cleanest mechanical update (see the
    prior session's commit history for the pattern).
  - Add `TypedExprKind::Scope { mode, body: Box<TypedBlock> }`,
    `Spawn { call: Box<TypedExpr>, task_id: TaskId }`,
    `Await { task_expr: Box<TypedExpr>, task_id: TaskId }`.
  - In `check_expr`: Scope types as the body's tail type;
    Spawn validates the inner is a `TypedExprKind::Call`
    (per ADR 0024 D2), restricts call's return type to I64
    (per ADR 0024 D7), interns `Task<I64>`; Await requires
    the receiver be `Type::Task(_)` and produces the
    interned TaskData's result_ty.
  - Three new `TypeError` variants: `SpawnMustBeCall`,
    `SpawnResultMustBeI64`, `AwaitOnNonTask`.
  - Update all Type-match sites for `Type::Task(_)` arms
    (≈8 sites: try_to_nullable_inner, try_to_array_elem,
    substitute, type_display, Display impl,
    coerce_to_expected match, is_nullable, llvm_basic_type's
    callers).
  - Update all TypedExprKind-match sites for Scope/Spawn/
    Await arms in: `substitute`, `walk_expr_for_mono`,
    `expr_performs`, `count_performs`, `find_unique_perform`,
    `walk_collect_var_refs`, `find_var_name_in_expr`,
    `substitute_perform_with_var`. **Prior attempt missed
    `substitute_perform_with_var` and one other** — the
    error site list is preserved in the prior session's
    last build output (2 remaining match arms at codegen
    lines ~5572 and ~5852).
  - Drop the C4.4 (1/N) NotYet variants from `ResolveError`
    AND from the resolve `check_expr` arms (replace with
    full pass-through). Drop the C4.4 (1/N) `unreachable!`
    stub in types' check_expr.

  **Decision deferred** (ADR 0024 D5 — Async effect-row
  enforcement): the prior attempt's MVP plan was to skip
  effect-row enforcement entirely at C4.4 (2/N) and treat
  spawn/await as effect-free. This keeps the phase-go
  trivially typecheckable but contradicts ADR 0024 D5.
  Recommended: ship the enforcement at C4.4 (2/N) — auto-
  register the Async effect in resolve (pre-register
  `EffectId(0)` as Async before user effects), make
  spawn/await contribute Async to effect-check's row, make
  `scope concurrent { ... }` discharge Async at the
  effect-check layer. This requires ~50 extra LOC in
  sentinel-effect-check (a new TypedExprKind arm) + a small
  resolve change. Document any deferral as an amendment to
  ADR 0024 D5.

**C4.4 (2/N codegen) deliverables** (per ADR 0024 D8):

  - **Add 5 runtime-fn externs to CodegenCtx + declare in
    pass 1**: task_spawn_fn / task_await_fn / scope_enter_fn /
    scope_exit_fn / scope_register_fn. Signatures per ADR
    0024 D7 (all opaque `ptr` types except task_await
    returns i64; scope_enter takes no args; etc.).
  - **Per-spawn-fn wrapper synthesis** (the tricky bit). One
    wrapper per unique spawn-target FnId (not per spawn
    site). The wrapper signature is
    `void wrapper(*Task task_ptr, *u8 args)`. Body: GEPs
    into args at i64 offsets to unpack each arg, calls the
    target fn, stores result in task_ptr->result (offset 0),
    writes 1 into task_ptr->done (offset 8).
    **Sticking point from prior attempt**: synthesizing
    wrappers from inside CodegenCtx fails because CodegenCtx
    doesn't hold a `&Module` reference. The cleanest fix:
    pre-walk the typed program in compile_to_object (BEFORE
    CodegenCtx is created) via a `collect_spawn_targets` fn
    that returns a `Vec<FnId>`. Then for each FnId, declare
    the wrapper via `module.add_function(...)` and emit its
    body using a fresh `context.create_builder()` (local to
    the pre-walk loop). Pass the resulting
    `spawn_wrappers: HashMap<FnId, FunctionValue<'ctx>>` to
    CodegenCtx by ownership. The prior attempt's wrapper
    code is at `/dev/null` (reverted) but the pattern is in
    the runtime tests showing the wrapper's expected shape.
  - **Lower Spawn at the call site**: `sentinel_alloc(N*8)`
    for args storage; lower each arg + store at offset i*8
    via GEP; call sentinel_task_spawn(wrapper_ptr,
    args_storage, args_size_bytes); if a current_scope is
    pushed on CodegenCtx, also call sentinel_scope_register.
  - **Lower Await**: lower task to a ptr value + call
    sentinel_task_await + return the i64 result.
  - **Lower Scope**: call sentinel_scope_enter to get a
    ScopeCtx*; push `current_scope: Option<PointerValue>`
    on CodegenCtx (save+restore for nested scopes); lower
    body; call sentinel_scope_exit; pop current_scope.

  **Drop emission**: add `Type::Task(_) => {}` arm to
  emit_drop_for_binding — Task cleanup is the runtime's job
  (sentinel_task_await / sentinel_scope_exit free the
  Task). Add `Type::Task(_) => false` to
  field_type_needs_drop_inner.

**Phase-go fixture** per ADR 0024 D12: `c44_go_no_go.sentinel`
— `scope concurrent { let t = spawn double(21); t.await }`
returns 42 via the thread + auto-await.

**UI fixtures**: c44_spawn_non_fn_call (D2 restriction —
spawn target isn't a Call shape), c44_await_on_non_task (D3
— await receiver isn't Task<T>), c44_spawn_result_must_be_i64
(D7 — spawned fn returns non-i64).

**Close-out**: ADR 0024 PROPOSED → ACCEPTED or
ACCEPTED-WITH-AMENDMENTS. Likely amendments per the prior
attempt's learning: A1 D6 thread-per-spawn vs work-stealing
(thread-per-spawn shipped; work-stealing deferred); A2 D9
cancellation on early exit (deferred); A3 D7 Task<T>
restricted to T=I64 (broader T deferred); A4 D5 (if Async
effect-row enforcement gets deferred to C4.5).

**Estimated footprint after the runtime is already in**:
~700-900 LOC across types (~300) + codegen (~400) + fixtures
(~80). Achievable in 1-1.5 sessions of focused work.

**Lessons from the prior C4.4 (2/N) attempt** (preserved for
the next attempt):

  - The `tasks` plumbing through `check_expr` is mechanical
    but the chain is long (≈40 call sites). Use a Python /
    sed script to add `tasks` next to `konts` in both
    signatures and call sites; the prior attempt's pattern
    was `'        konts,$' → '        konts,\n        tasks,'`.
  - The TypedExprKind walks (10+ helper fns) all need the
    same three new arms — easier to do them all in one pass
    via grep+edit rather than chasing build errors.
  - **The codegen wrapper-synthesis approach matters**:
    inline-during-lower_expr DOES NOT WORK because
    CodegenCtx lacks a `&Module` reference. The clean
    pattern is to pre-walk in compile_to_object before
    CodegenCtx exists.
  - The `wrapper_builder.build_*` patterns must match
    inkwell's BuilderError types — chains of `.expect("call")`
    work but verify the inkwell version's Result vs panic
    semantics before committing. The prior attempt's
    wrapper code had `wrapper_builder.build_call(...)
    .expect("call")` which compiles cleanly in inkwell 0.4+.
  - Once spawn lowering exists, validate with
    `cargo run -p sentinel-driver -- tests/pass/c44_go_no_go.sentinel`
    before adding the scope cleanup arm — bug-isolation is
    easier in stages.

**Alternative — Path 3 bounded-generic follow-on** (per ADR
0023 A1 amendment):

  - Parser: `fn use_writer<W: Writer>(w: &mut W) -> i64` — new
    `<T: Bound>` syntax in `parse_type_params`.
  - AST: `TypeParam.bounds: Vec<Spanned<String>>`.
  - Resolve: link bound name → TraitId.
  - Types: bounded `Type::TypeParam` representation; bound
    used at call sites to validate that the concrete mono
    instance satisfies the bound.
  - Codegen: per-instance witness-table GEP + indirect call
    OR (simpler) per-instance specialisation via the C1.7.5
    monomorphisation worklist with the trait's default impl
    inlined at the call site.
  - Estimate: ~1 session.
  - Detail ADR: amend ADR 0023 OR write a follow-on ADR.

**C4.1 follow-ons remain available** (none blocking C4.3):

**C4.1 follow-ons remain available** (none blocking C4.2):

  - **Branch-aware definite-assignment**: extend the flat
    any-assigned check with if/else snapshot+merge mirroring
    the C2 borrow CFG; add a UI fixture pinning the per-arm
    bitmap (init assigning `x` only in the if-branch).
  - **InitFieldReadBeforeAssign**: detect reads of
    `self.field` before the field's first assignment within
    the init body.
  - **Non-lvalue receiver MethodCall**: support
    `Point::init(1, 2).manhattan()` directly via alloca +
    store + GEP detour at the call site.
  - **`Block.tail` → `Option<Expr>`**: closes the placeholder-
    `0` workaround in init bodies. Cross-cutting parser +
    AST + typing + codegen change.
  - **`ClassConstructionMustUseInit` promotion**: move from
    types-level to resolve-level so struct-lit-on-class
    gives the clearer diagnostic instead of UndefinedStruct.
  - **Generic classes**: `class Pair<A, B>` per ADR 0022 D1's
    revisit trigger. Resolve+types+codegen monomorphisation
    extension; AST already supports the surface shape. The bootstrap language surface at C3.7 close has the
full memory-safety + secret-typing + effect-system trifecta
from HANDOVER §6.2:

  - Memory safety: Phase C2's lexical borrow check + RAII
    drop (ADR 0017 ACCEPTED-WITH-AMENDMENTS).
  - Secret typing: Phase C3.1's `Type::Secret(SecretId)` +
    declassify + implicit widening + four CT rejections
    (ADR 0019).
  - Effect system: Phase C3.2/3.3's effect-row machinery +
    main-must-be-effect-free (ADR 0019).
  - Handler runtime: Phase C3.4 → C3.7's deep-handler one-
    shot continuations + bubble-aware resume + nested
    handles + non-identity return arms (ADR 0020).

What's deferred past Phase C3:

  - **Multi-shot continuations** (ADR 0020 D2 amendment).
    The upgrade path stays mechanical: deep-clone the kont's
    frame chain + captured-state on each resume entry.
    Promotes to a follow-on perf ADR if Sentinel surfaces a
    use case.
  - **Non-i64-returning ops**: placeholder type hardcoded
    to i64. A future ADR can generalise (per-fn resumer
    types, or boxed values).
  - **Embedded performs inside chained-let RHSes**:
    e.g. `let a = perform Op() + 1`. The C3.5(d) embedded-
    shape detector requires `stmts.len() == 0`; the C3.5(e)
    chained-lets detector requires each let RHS to be a
    direct perform / effecting call. The union case stays
    a follow-on extension to C3.5.
  - **Polonius migration** (ADR 0018): flow-sensitive
    borrow-check. PROPOSED only; the lexical formulation
    serves the C3 bootstrap minimum.
  - **Partial-move-through-field-projection soundness gap**
    (docs/borrow-check-limitations.md): a documented C2
    soundness hole. Highest-priority post-C2 work on the
    borrow-check side.

For **Phase C4** per HANDOVER §6.2: classes + traits + named
impls + delegation + structured concurrency. ADR 0021 is the
phase kickoff (PROPOSED). The sub-phase plan from ADR 0021
D14 (C4.0 complete; six total):

  - C4.0 lexer (1 session, **DONE**): class / trait / impl /
    init / delegate / scope / spawn / await / Self / self /
    as / for keywords reserved.
  - C4.1 class + method + init (2-3 sessions): parallel-
    tree mirror across AST / parser / resolve / types /
    codegen + definite-assignment check.
  - C4.2 traits + named impls (2-3 sessions): trait decls +
    impl blocks (default + named) + witness-table dispatch.
  - C4.3 delegation (1 session): auto-forwarder codegen for
    `delegate inner: T to Trait`.
  - C4.4 structured concurrency (2-3 sessions): Async effect
    + scope/spawn/await surface + runtime scheduler
    (substantial new runtime work; warrants ADR 0024 at
    sub-phase open).
  - C4.5 close-out (0-1 session): phase-go + ADR 0021
    PROPOSED → ACCEPTED flip.

Per-sub-phase ADRs follow ADR 0021: ADR 0022 (C4.1 surface),
ADR 0023 (C4.2 surface + dispatch), ADR 0024 (C4.4
scheduler + Async effect) — each PROPOSED at its sub-phase
open.

**C3.7 retrospective** (this session): ADR 0020 D9 estimated
0-1 sessions for C3.7. Actual: ~0.3 sessions. The substantive
piece was the body-restriction lift: `lower_handle` now accepts
any expression, wrapping pure i64 bodies via
sentinel_kont_pure so the dispatch loop is uniform. The empty-
arms case (`handle X with { return v => body }`) fell out from
the existing merge phi logic — just take the result type from
pure_val when arm_results is empty. The phase-go fixtures are
mechanical given the C3.5(c) let-shape coverage. **ADR 0020
flips PROPOSED → ACCEPTED-WITH-AMENDMENTS** with D2 multi-shot
deferred indefinitely (Phase B validation demos all worked
one-shot; the bootstrap minimum doesn't surface a multi-shot
use case). Phase C3 closes here.

**Pre-C3.7 — C3.6(b) retrospective**: ADR 0020 D9 estimated
1 session for C3.6(b). Actual: ~0.4 sessions. The substantive
piece was understanding what "inner emits Kont*" meant at the
IR layer + how arms/pure/propagate paths each contribute
Kont*s to the merge. Once the model was clear, the edit fell
out: arms wrap their i64 via sentinel_kont_pure; pure-path
passes current_kont through (or re-wraps after return arm);
default routes to a propagate block. Design notes:

  - **handle_depth counter vs handle_stack.len()**:
    handle_stack is pushed AFTER body lowering, so its
    .len() doesn't reflect "we're inside a handle's body
    lowering". A separate counter incremented at entry +
    decremented at exit gives the right reading during the
    body-lowering recursion.
  - **Wrap-then-unwrap cost**: nested handles allocate +
    free more konts than necessary (each arm wraps i64 →
    pure_kont; outer's pure-path consume_pures it). For C3
    minimum perf budget this is acceptable; a future ADR
    can revisit (e.g., elision when outer is statically
    known not to need the wrap).
  - **Type-check already supports this**: the partial-
    handle case (body has more effects than the handler
    catches) was already valid in the typing layer per
    ADR 0019 D2. C3.6(b) is purely a codegen change.

Workspace test delta: +2 tests (1072 total) — +2 driver
pass-tests (c36b_nested_handle_basic,
c36b_nested_handle_inner_full).

**Pre-C3.6(b) — C3.6(a) retrospective**: ADR 0020 D9 estimates
1-2 sessions for all of C3.6 (return arms + nested + multi-
shot). C3.6(a) alone was ~0.3 sessions — quick once the
`HandleContext` extension fell out. Design notes:

  - **Deep-handler re-wrap via k(v) path**: Phase B's `k :=
    \v. handle (kont.resume v) with H` semantics push the
    return arm into the k(v) call site. Without this, the
    return arm only fires for the pure-body case (which is
    rare in practice). With it, the return arm fires
    uniformly — matching what users expect when they write
    `return v => transform(v)`.
  - **HandleContext non-Copy**: storing TypedReturnArm
    (which contains TypedExpr — non-Copy) forced dropping
    the Copy derive. lower_resume_kont now snapshots via
    `last().unwrap().clone()` once at function entry.
  - **No re-typing pass needed**: the return arm's body was
    already typed during C3.4; codegen just lowers the
    existing TypedExpr.

Workspace test delta at C3.6(a) close: +2 tests (1070 total)
— +2 driver pass-tests (c36a_return_arm_transform,
c36a_return_arm_after_resume).

**C3.5(e) retrospective** (this session): ADR 0020 D9
estimated "1-2 sessions" for C3.5(e). Actual: ~1 session. The
substantive piece was the runtime + lower_handle restructure
to support bubble. Once `sentinel_kont_resume` returns a kont*
and the handle becomes a dispatch loop, chained-lets codegen
is straightforward — each resumer is a smaller chained-lets
fn. Design notes:

  - **Uniform Kont* return**: changing
    `sentinel_kont_resume`'s return type to `*mut SentinelKont`
    unifies the pure-return and bubble cases at the caller
    site — both go through the same `load op_id → check
    PURE_RETURN_OP_ID` check. The "always wrap final value
    in pure-return" cost is one extra alloc/free pair per
    `k(v)` call in trivial cases, which is acceptable at C3
    minimum perf budget.
  - **Alloca-backed dispatch slot**: using an alloca for
    `current_kont_slot` instead of an LLVM phi node avoided
    threading PhiValue references through the
    `handle_stack`. The phi-based alternative is equivalent
    and would generate slightly tighter IR after mem2reg.
  - **HandleContext stack**: lower_resume_kont consults
    `self.handle_stack.last()` to find its enclosing
    handle's loop block + slot. Pushed on lower_handle
    entry; popped on exit. Nested handles will need this
    plus an "ownership" rule (an inner handle's resumer
    shouldn't bubble to an outer handle's loop) — a C3.6
    concern.
  - **Captures: precise vs conservative**:
    `compute_chained_lets_captures` walks
    `lets[i+1..].rhs + tail` and excludes future-let-bound
    ids + the let_i value param. Conservative
    "everything-in-scope" would over-allocate; precise
    walking matches what each resumer actually reads.

Workspace test delta: +3 tests (1068 total) — +3 driver
pass-tests (c35e_chained_perform,
c35e_chained_perform_with_capture,
c35e_chained_dependent_perform).

**Pre-C3.5(e) — C3.5(d) retrospective**: ADR 0020 D9
estimated "1 session" for C3.5(d). Actual: ~1 session. The
unified approach (count_performs / find_unique_perform /
substitute_perform_with_var walkers + detect_embedded_perform_shape)
turned out to be a clean generalisation of C3.5(c)'s
per-let approach — they share the resumer codegen pattern
(alloca for placeholder + captureds; lower substituted body;
wrap via sentinel_kont_pure). The substitution walker is
~150 LOC of mechanical recursion through TypedExprKind variants.
Design notes:

  - **Disjoint shape detection**: let-shape (stmts.len() == 1
    with effecting let-RHS) and embedded-shape (stmts.len()
    == 0 with single embedded perform) don't overlap. compile_fn
    dispatches to either path or falls back to the C3.5(b)
    validate-and-lower path.
  - **Per-shape resumer map**: each shape gets its own
    HashMap entry. embedded_perform_resumers stores the
    substituted tail (as TypedExpr) + placeholder VarId
    alongside the resumer FunctionValue + captured VarIds.
    Stored at pass-1 detection time so compile_fn doesn't
    re-substitute.
  - **Placeholder VarId = u32::MAX**: synthetic constant
    chosen to avoid collision with resolve-assigned VarIds
    (which grow from 0). Each compile_fn run binds the
    placeholder in its own env; no cross-fn collision since
    env is reset per compile_fn.

Workspace test delta at C3.5(d) close: +3 tests (1065 total)
— +3 driver pass-tests (c35d_binop_with_perform,
c35d_perform_with_capture_and_binop, c35d_perform_in_call_arg).

**Pre-C3.5(d) — C3.5(c) retrospective**: ADR 0020 D9
estimated "1-2 sessions" for C3.5(c). Actual: ~1 session.
The substantive piece was the per-let resumer fn codegen +
captured-state struct layout. The runtime extensions
(SentinelFrame, sentinel_kont_push, resume's drain loop) were
small (~50 LOC added). The codegen for the let-shape was
larger (~200 LOC: detect_let_shape + collect_captured_vars +
walk_collect_var_refs + compile_effecting_fn_with_let). Design
choices:

  - **Pre-declare resumers in pass 1**, looked up by FnId in
    `let_resumers: HashMap<FnId, (FunctionValue, Vec<VarId>)>`.
    Avoids the lifetime gymnastics of holding a `&Module` in
    CodegenCtx; CodegenCtx still owns just the FunctionValues
    + captured-VarId lists.
  - **Captured struct via byte-offset GEP**, not LLVM struct
    type. Each captured var is an i64; the struct is `i64[N]`
    on the heap, accessed via `i8`-indexed GEP at offsets
    `0, 8, 16, ...`. Keeps the codegen layout-stable without
    needing per-fn LLVM struct types.
  - **Resumer signature uniform** `ptr (i64, ptr)` so every
    `sentinel_kont_push` call site shares one fn-pointer type
    on the runtime side. The resumer always returns a pure-
    return kont at C3.5(c) MVP (resumers don't perform); nested
    perform inside resumers lands at C3.5(d) / C3.6.
  - **MVP restrictions**: single let-stmt only (no chains of
    effecting lets); let-bound type must be i64; captured vars
    are filtered to fn params only (no earlier let-bindings
    since stmts.len() == 1).

Workspace test delta from C3.5(b) close: +1 test (1062 total)
— +2 driver pass-tests (c35c_let_bound_perform,
c35c_let_bound_perform_with_capture), -1 driver UI test
(c35b_effecting_fn_let_bound_perform was retired since the
shape it asserted now succeeds).

**Pre-C3.5(c) — C3.5(b) retrospective**: the effecting fn
ABI piece took ~1 session within the original 1-2 session
C3.5(b) budget. Three substantive design choices:

  - **Effecting fn ABI returns Kont*** (plain ptr, no
    struct). Avoids the calling-convention overhead of a
    multi-value return on every call. Tradeoff: a wrap is
    needed when the body is pure (handled via
    sentinel_kont_pure + the PURE_RETURN_OP_ID sentinel).
  - **Unified runtime switch in lower_handle**: the C3.5(a)
    static arm dispatch was retired in favour of always
    emitting a switch on the kont's op_id. Adds 4-5 LLVM
    instructions per handle but simplifies codegen and
    handles multi-arm cases naturally. The switch includes
    a dedicated PURE_RETURN_OP_ID case that calls
    sentinel_kont_consume_pure to unwrap pure values — the
    runtime expression of ADR 0020 D4's default
    `return v => v`.
  - **Validation, not transformation**, for the limited
    case. Effecting fns whose body's tail produces a kont
    (Perform / Call-to-effecting / Block-thereof) lower
    directly; pure tails get wrapped. Tails that mix
    perform with surrounding pure context (e.g.,
    `perform Op() + 1`) require frame reification and
    surface `effecting_fn_body_not_direct` — deferred to
    C3.5(c).

Workspace test delta from C3.5(a) close: +3 tests (1061
total). Three new pass fixtures (`c35b_handle_fn_call_body`,
`c35b_handle_multi_arm`, `c35b_handle_pure_return`) + one
UI fixture (`c35b_effecting_fn_let_bound_perform`); the
previous c34 UI fixture (asserting codegen rejected
do_work() bodies) was retired because that shape now
compiles + runs end-to-end.

**Pre-C3.5(b) — C3.5(a) retrospective**: ADR 0020 D9
estimated "2-3 sessions" for the full C3.5. Actual for the
restricted-case slice: ~1 session. Restriction shrank the
problem: with handle body fixed to a direct Perform, the
arm dispatch becomes a compile-time choice (no runtime
op_id switch needed), and frame reification at
intermediate evaluation sites is skipped entirely. The
kont struct (24-byte `{ op_id: u32, _pad: u32, arg: i64,
consumed: u8 }`) is the minimal payload that the
follow-on C3.5(b) work will extend with a frames vector.
The novel pieces:

  - **Static arm dispatch**: instead of emitting a switch
    on the kont's op_id at runtime, we lookup the matching
    arm at compile time (resolve guarantees uniqueness via
    DuplicateHandlerArm) and emit a direct call into the
    arm's body. Saves a load + switch in IR and keeps the
    restricted-case path minimal.
  - **GEP-based arg read**: the handler arm's op-param
    VarId is bound to the value at byte offset 8 of the
    kont struct (after `op_id: u32 + _pad: u32`). Codegen
    emits a `getelementptr i8` + `load i64` pair. Layout
    is asserted stable via a runtime test
    (`sentinel_kont_struct_layout_is_stable`).
  - **Opaque-pointer Type::Kont**: `llvm_basic_type` for
    `Type::Kont(_)` returns a plain `ptr`. The
    underlying struct layout lives in sentinel-runtime;
    codegen reads fields via byte-offset GEP rather than
    declaring an LLVM-level struct type.

Workspace test delta: +6 tests (1058 total) — +4 sentinel-
runtime (kont layout / initialisation / resume / round-trip),
+2 driver pass-tests (c35_*). The previous c34 UI fixture
(do_work() body) now surfaces the more specific
`handle_body_not_direct_perform` diagnostic — the driver
test was updated accordingly.

**Three C3 follow-ons are documented but deferred**:

  - **C3.6 — handle codegen for general case** (frame
    reification, the substantive runtime piece). Overlaps
    with C3.5(b) since the same call-site machinery is
    needed.
  - **Partial-move-through-field-projection soundness gap**:
    still open from C2; postfix `.field` on a Move-typed
    binding is non-consuming, leading to double-free at
    drop. Documented in
    `docs/borrow-check-limitations.md`.
  - **ADR 0018 (PROPOSED)**: Polonius migration plan, also
    from C2. Trigger is empirical friction.

**C3.4 retrospective** (kept for reference): ADR 0020 D9
estimated "1-2 sessions" for C3.4. Actual: ~1 session. The
AST + parser additions were mechanical given the C3.0(a)
reserved keywords (`handle`, `with`, `perform`) and the C3.2
effect data model. Two new lexer tokens were needed
(`FatArrow` / `=>` and `Return` contextual keyword) — small
lexer delta. The seventh interner table (`Type::Kont`)
followed the established Secret/Ref/GenericInstance pattern.

Alternative path (defer remaining handler runtime): start
**Phase C4** (traits + structured concurrency) per HANDOVER
§6.2 with a new ADR 0021 PROPOSED. C4 is larger in volume
but lower per-piece risk than the C3.5/C3.6 runtime work.
Either choice is defensible. The handler-runtime path
completes the Phase B effect-system vision and is the
natural "close what we started" move; the traits path is
the larger landing zone for the production-shape language.

C2 retrospective (estimate vs actual): ADR 0017 D9 estimated
"6-13 sessions across 5-6 sub-phases"; actual was ~6 sessions
across 6 sub-phases (split into C2.0.1 + C2.0.2 + C2.1 + C2.2
+ C2.3 + C2.4 + C2.5 = seven feat/docs commits). Low end of
the estimate range. C1's "1-session-per-sub-phase" rhythm
DIDN'T fully carry to C2 — C2.2 (&mut + XOR), C2.3 (move),
C2.4 (RAII drop) each used a full session because the borrow
checker is genuinely novel machinery. The infrastructure
investment compounded on the *surrounding* pieces (lexer +
parser + AST + resolve were trivial deltas); the substantive
work was in `sentinel-borrow-check` (~1500 LOC) and the codegen
drop emission. Notes captured in STATE.md decisions for
each C2.x sub-phase.

C1.7 retrospective (estimate vs actual): ADR 0011 D6 estimated
"4-6 weeks" (the longest single C1 sub-phase); actual was ~1
session across 5 commits (e411ded ADR 0016 PROPOSED + c1e5083
scaffolding + d32a9fe types + ad7e10d codegen + 2c6c652 generic
structs end-to-end). Faster than estimated, in line with the
C1.4/5/6 pattern. The substantive pieces:
(a) the interned-instance design choice (`Type::GenericInstance(
GenericInstanceId)` with the args in a program-level table)
preserved `Type: Copy` and avoided a ~30-site clone-cascade
refactor; (b) the eager-substitute approach for codegen
(`TypedFnDef::substitute` deep-clone + lower the substituted
def via the existing per-fn path) avoided the lazy-substitution
audit risk; (c) the unified `check_call` consolidation —
deleting the ~75 LOC of special-cased C1.5/6 builtin Call
branches — is the cleanest payoff of the C1.7 design. Notes
captured in STATE.md decisions 82-93.

C1.6 retrospective (kept for reference): "3-4 weeks" estimated;
~1 session actual. Notes in STATE.md decisions 74-81.

C1.5 retrospective (kept for reference): "2-3 weeks" estimated;
~1 session actual. Notes in STATE.md decisions 65-73.

C1.4 retrospective (kept for reference): "3-4 weeks" estimated;
~1 session actual. Notes in STATE.md decisions 54-64.

C1.3 retrospective (kept for reference): "2 weeks" estimated;
~1 session actual. Notes in STATE.md decisions 46-53.

**C1 overall retrospective**: ADR 0011 D6's honest 22-28 week
budget for all of C1 was generous; actual elapsed across C1.0a
through C1.7.4b was ~10-12 sessions (~5-6x faster). The
infrastructure investment (Salsa retrofit + per-pass crate split
+ parallel-tree pattern + per-sub-phase ADR-first discipline)
compounded heavily — each sub-phase reused the same scaffolding
and the same five-step rhythm (ADR → lexer → bundled AST/parser/
resolve/types/codegen → fixtures → docs). C2's region work
likely won't compound the same way (borrow checking is novel
machinery) but the ADR-first norm, the parallel-tree pattern,
and the salsa pipeline all carry forward intact.

C1.5 retrospective (kept for reference): "2-3 weeks" estimated;
~1 session actual. The bidirectional checking infrastructure
and the D4/D10 amendments were the highest-thought-cost pieces.
Notes in STATE.md decisions 65-73.

C1.4 retrospective (kept for reference): "3-4 weeks" estimated;
~1 session actual. The codegen value-type widening from
`IntValue<'ctx>` to `BasicValueEnum<'ctx>` was the substantive
change. Notes in STATE.md decisions 54-64.

C1.3 retrospective (kept for reference): "2 weeks" estimated;
~1 session actual. Notes in STATE.md decisions 46-53.

### 0.3 Quick-status block for session start

> **▶ (8/N) CODEGEN — the GRAND FINALE + the bootstrap fixed-point — is OPENED; ADR 0045
> PROPOSED (2026-06-05), the 3 design calls SETTLED WITH THE OWNER (as 5/N–7/N were). (7/N)
> the MIR + const-time stage is COMPLETE — ADR 0044 → ACCEPTED.** The whole pipeline through
> codegen's input is ported (lexer → … → borrow-check → MIR-lowering + const-time; `snc
> lex`/`ast`/`resolve`/`types`/`borrow`/`mir`/`ctverify` all 123/123, `effects` 122/122,
> leak-free). The ONLY thing left is codegen → the object → the fixed-point.
>
> **The 3 owner-chosen (8/N) decisions (ADR 0045):** (1) **emission target = textual LLVM
> `.ll`** (`write_file` → external `clang`/`llc` → object → link `libsentinel_runtime.a`) —
> over emit-C / emit-asm; (2) **oracle = a NEW Rust `snc llvm` (`llvm_dump.rs`) canonical-`.ll`
> byte-parity differential** (the port's signature method — a canonical spec WE define, NOT
> inkwell `print_to_string`) **+ behavioural clang-run-parity** (each `.ll` → clang → run →
> diff exit/stdout vs the fixture) **+ the bootstrap fixed-point capstone** — over
> behavioural-only / inkwell-parity; (3) **scope = fixed-point-first** (Bar A = the non-exotic
> core the selfhost sources use → reach the fixed-point; Bar B = effects/handlers/concurrency/
> classes/generics/nullable for full-corpus parity, AFTER).
>
> **🔑 SCOUT + PROBE findings that shrink the 8263-line finale** (this session): (a) **NO
> phi** — codegen is `alloca`+`load`/`store` at `-O0` (mem2reg off), merging branches through
> memory cells, so the textual `.ll` needs value-numbering only for instruction temporaries,
> NOT SSA merge points — *simpler* than 7/N's MIR (the `var_defs`-snapshot/VarId-sorted-merge
> muscle is NOT needed); a hand-written `.ll` in this exact style **clang/llc-18 compiles +
> runs exit-correct at `-O0`, linking a `sentinel_*` symbol** (probe-validated). (b) **Secret
> codegen is a no-op** — `secret T` strips to `T` (codegen lib.rs:1594; CT-emission deferred
> post-1.0 per ADR 0019 D12); the CT guarantee = the source rejections + the 7/N D5 verifier.
> (c) **The bootstrap subset is small** — the selfhost sources declare **424 fns / 3 structs /
> 14 enums + 0 traits/impls/classes/effects/generics**, and use no nullable/handle/perform/
> spawn/scope/declassify/secret → Bar A is a fraction of 8263 lines; the ~1500-line handler/
> kont + ~765-line shape-detection + concurrency/class machinery is Bar B. (d) 3-pass structure
> (type-decls / fn+runtime-symbol-decls / body-emission); `compile_to_object` reads TypedProgram
> + DropPlan (codegen lib.rs:168); link = `cc obj libsentinel_runtime.a -o exe`.
>
> **Reuse = the 6/N `types::run`-with-`mode` template, a new `mode 4`** (emit-`.ll`). ⚠ THE
> (8a) PROBE: **fused `mode 4` vs a hybrid** (the driving walk a `mode 4` in `types.sentinel`
> reusing type/VarId/scope, the `.ll`-emit helpers in a `codegen.sentinel` module — keeps the
> monolith bounded); **re-verify modes 0–3 (`snc types`/`borrow`/`mir`/`ctverify`)
> byte-identical BEFORE bulk emission** (the 0044 D3 gate, widened to FOUR accepted stages).
> **✅ (8a-i) the `snc llvm` ORACLE LANDED** (`1931496`, ADR 0045 A1): `run_llvm` +
> `llvm_dump.rs` emit the canonical `.ll` (partial-by-Err — emits the supported subset, Errs +
> exits nonzero on the rest so the differential skips it). 3-layer validation in `tests/llvm.rs`:
> goldens pin the spec; a 0-panics corpus sweep (**16 emit / 125 Err** over 141, no crash); and
> **16/16 behavioural parity** (each emitted `.ll` via `cc` behaves identically to inkwell `snc
> build`). AS-BUILT spec: **NO phi** (alloca/load-store, a per-fn `%vN` counter, `%argN` params),
> `main`→i32-trunc, FnId order; ops add/sub/mul/sdiv/udiv/and/or/xor, icmp (signed+unsigned),
> sub-0-neg, xor-1-not, call, zext/trunc width builtins. **D4 reuse SETTLED = fused `mode 4`**
> (mirrors the proven MIR `mode 2`): a `cgout` buffer + an operand-threading field (like
> `lastval`) + a VarId→slot append-only pool (like `mvdv`) + a value counter; `type_fn` emits the
> `define` header/footer. The hybrid (emit-helpers in a separate module) is deferred — fuse first.
> **✅ (8a-ii) `selfhost/codegen.sentinel` LANDED (`2ed426a`, ADR 0045 A2) — (8a) COMPLETE:**
> the 8th + final Sentinel stage emits `.ll` straight-line, matching `snc llvm` byte-for-byte
> (`sentinel_codegen_matches_oracle_on_corpus` + `_on_seeds`, 16/16 emitted), leak-free, modes
> 0–3 byte-identical (all 8 corpus differentials green). The fused **`mode 4`** mirrors MIR
> `mode 2` 1:1: a `cgout` buffer + `cglk`/`cglv` operand (≈ `lastval`) + a `cgsv`/`cgsr` slot
> pool (≈ `var_defs`) + a value counter; `mir_on`→2/3-only, `cg_on`=4 (every cg emit guarded →
> modes 0–3 dead by construction). 🔑 **KEY FINDING:** the A2 "no `&mut (*c).field` to a USER
> fn" rule is sidestepped by **direct-to-`cgout` helpers** using the BUILTIN `push` + consuming
> any `[u8]` arg by value (simpler than MIR's render-to-local-then-fold). `type_fn` emits the
> define header + param allocas + ret/`}` (main → i32-trunc via `cg_is_main`); `emit_tparams`
> reserves param slots; `dump_targs` collects call args (`cg_collect`). **NO phi** — alloca/
> load/store. ⚠ bind-inner-first bit once (`cgo_ty(c, (*c).cgat[i])` re-borrows c).
>
> **✅ (8b) CONTROL FLOW COMPLETE (ADR 0045 A3; `c76db27` 8b-1 + `7b33d49` 8b-2):** if/else +
> while/break/continue + short-circuit `&&`/`||`, byte-identical to `snc llvm` (26/26 emitted)
> + behavioural (cc==inkwell) + leak-free; modes 0–3 byte-identical. 🔑 THE ALLOCA HOIST (8b-1)
> is the foundational refactor — every alloca (params/lets/if-results) is hoisted to the entry
> block, solving BOTH the if-result's late-known type (the parser AST has no precomputed types,
> so the slot is reserved after the then walk) AND ADR 0036 per-iteration loop-stack growth.
> codegen.sentinel buffers the body in `cgbody` + records allocas as (slot,type) pairs + a
> `cg_putc` router (`cg_to_body`) sends emission to cgbody (walk) vs cgout (teardown assembly:
> header + hoisted allocas + folded body + ret); the (8a) helpers' 29 cgout pushes were routed
> through `cg_putc`. if/else + `&&`/`||` are no-phi memory-cell merges; while is the real loop
> CFG (back-edge + a loop-target stack `cg_loop_cond`/`cg_loop_after` + a dead-block after
> break/continue).
>
> **✅ (8c-1) STRUCTS COMPLETE (ADR 0045 A4) — opens slice (8c):** struct type decls + literals
> + field reads, byte-identical to `snc llvm` (**32/32 emitted** — the 5 C1.4 pure-struct
> fixtures light up) + 4 seeds + 1 golden (`llvm_struct_decl_lit_and_field`), behavioural
> (cc==inkwell) + leak-free; modes 0–3 (types/borrow/mir/ctverify) + effects byte-identical. 🔑
> A struct is a first-class SSA VALUE — `insertvalue`/`extractvalue` over an aggregate `%vN`
> (the inkwell rvalue path), NOT alloca/GEP — so `let`/`Var`/param/return/call carry it through
> the EXISTING alloca/store/load once `cgo_ty` learns structs (kind-6 → `%Struct.N` via
> `struct_of_handle`); the slice is small. **Pass 0** (`cg_pass0` in the mode-4 preamble + a
> buffer-targeted `ll_type_to`) emits `%Struct.N = type { … }` per struct in StructId order.
> **struct-lit** = COLLECT-then-emit (the oracle switched interleave→collect so both backends
> agree on side-effecting field values) reusing the call-arg stacks
> (`cg_collecting`/`cgak`/`cgav`/`cgat` + a new `cg_emit_structlit`); **field read** = a new
> `cg_extract` (`extractvalue`; chained `o.inner.x` nests). ⚠ Generic structs (8h/Bar B) + field
> ASSIGNMENT (`p.x = …`, the oracle's non-Var-lvalue limit) are deferred — no emitting fixture
> needs them.
>
> **✅ (8c-2) ARRAYS COMPLETE (ADR 0045 A5):** array literals + indexing + `len`, byte-identical
> to `snc llvm` (**42/42 emitted** — the 10 C1.6/C2.3 array fixtures light up) + 3 seeds + 1 golden
> (`llvm_array_lit_index_and_len`), behavioural (cc==inkwell) + leak-free; modes 0–3 + effects
> byte-identical. 🔑 `[T]` is the abi-v1 `{ i64 len, ptr data }` — ONE inline literal type for every
> element type (data = opaque heap ptr), so NO Pass-0 name; `let`/`Var`/param/return/call carry it
> via the EXISTING alloca/store/load once `cgo_ty` learns `Type::Array` → `{ i64, ptr }`. A literal
> heap-allocs `n * sizeof(elem)` (the **GEP-sizeof idiom** `getelementptr T, null, n` + `ptrtoint`
> — correct for any element incl. padded structs) via `sentinel_alloc`, GEP-stores each element,
> builds `insertvalue {len,ptr}` (reusing the call-arg collect stacks + `cg_emit_arraylit`). `a[i]`
> extracts len(0)/data(1), bounds-checks (`sge 0` + `slt len` + `and`; br to ok/oob; OOB =
> `sentinel_panic_oob` + `unreachable`), GEP+loads (`cg_emit_index`, reusing `cg_fresh_block`);
> `len` = `extractvalue 0`. **First runtime-symbol declares** — emitted ONLY for the symbols a
> program uses (per-symbol `used_alloc`/`used_panic`), so 8a–8c-1 stay byte-identical
> (`c16_empty_array` → only `sentinel_alloc`). ⚠ Debug find: `len` (FnId 3, a generic builtin)
> routes through `dump_gcall`/`dump_args_capture_first`, which walked the first arg WITHOUT
> `cg_collect`ing it (same gap as `dump_array_elems`) → empty `cgak` → SIGABRT; fixed by collecting
> the first arg in both.
>
> **✅ (8c-3) `[u8]`/STRING LITERALS COMPLETE (ADR 0045 A6) — slice (8c) aggregates DONE:** string
> literals + the char-literal cg operand, byte-identical to `snc llvm` (**43/43 emitted** —
> `c5d5_break_continue` joins, its `len("tok")=3` driving the exit) + 2 seeds + 1 golden, behavioural
> (cc==inkwell) + leak-free; modes 0–3 + effects byte-identical. 🔑 A string literal **IS a `[u8]`**
> (ADR 0033) — decoded bytes heap-copied (`sentinel_alloc` + N constant `i8` stores) into `{ i64,
> ptr }`, EXACTLY a u8 array literal of byte constants → reuses the array machinery: the oracle
> factored the array-buffer scaffold into `emit_array_buffer` (shared by ArrayLit + StringLit, can't
> drift); the Sentinel `Str` arm pushes each byte as an `i8` literal operand then reuses
> `cg_emit_arraylit` (read BEFORE `sink_name` consumes `sb`). Closed a latent gap: the `Char` arm now
> sets the cg operand (`cglk=1`/`cglv=cv`, a u8 constant like `Int`) — needed by `c5d2_strings` (8d).
> **✅ (8d, runtime builtins) COMPLETE (ADR 0045 A7) — the byte-array builtins, done FIRST within
> (8d):** `str_eq`/`print_bytes`/`read_file`/`write_file`, byte-identical to `snc llvm` (**45/45
> emitted** — **`c5d2_strings`** (D.2 strings phase-go, str_eq) AND **`c5d4_file_io`** (D.4 file-IO
> phase-go, read/write/print — REAL file I/O) join) + 2 seeds + 1 golden, behavioural (cc==inkwell)
> + leak-free; modes 0–3 + effects byte-identical. 🔑 Each builtin `extractvalue`s its `[u8]` into
> len(0)+ptr(1) and calls the `sentinel_*` symbol as `(ptr, i64, …)`; `read_file` uses a HOISTED
> out-len slot then reassembles the `[u8]`. Refactor: the per-symbol declare bools became a
> **`RuntimeSyms`** struct (merge + emit_declares; fixed order
> alloc/panic_oob/str_eq/read_file/write_file/print_bytes); the Sentinel side mirrors via `cg_used_*`
> flags + a **`cg_lenptr`** helper (extractvalue len/ptr → len reg, ptr=len+1). Non-generic builtins
> route through `dump_targs` (args collected) already.
>
> **✅ (8d-refs) REFERENCES COMPLETE (ADR 0045 A8) — the Vec prerequisite:** `&`/`&mut`/`*`/`*p=x`,
> byte-identical to `snc llvm` (**53/53 emitted** — the 8 C2 ref fixtures light up) + 2 seeds + 1
> golden + behavioural (cc==inkwell) + leak-free; modes 0–3 + effects byte-identical. 🔑 A ref
> `&T`/`&mut T` is an opaque **`ptr`** (mutability ignored; pointee from `program.refs` at the
> deref); a ref param is a `ptr` slot. `&v`/`&mut v` = v's alloca slot (the slot IS the pointer, NO
> instruction); `*r` = `load <pointee>, ptr <r-val>`; `*r = x` = `store <pointee> x, ptr <r-val>`
> (⚠ target ptr emitted FIRST — the Sentinel walks an assign TARGET-then-value, and a deref target
> emits a load). Sentinel **REUSES the existing `cg_suppress`**: `&`/`&mut` suppress the inner Var's
> load + read its slot (`cg_slot_get`); `*` loads through r, or (assign place) leaves r's pointer +
> `cg_lastvid=-1`; `&*r` keeps the deref-place's pointer (`un_vid=-1`). ⚠ `&*r` reborrow was a 1-byte
> miss first (`cg_slot_get(-1)`→`%v-1`) — guarded by `un_vid>=0`.
>
> **✅ (8d-Vec-1) Vec IN-PLACE OPS COMPLETE (ADR 0045 A9):** `vec_new`/`push`/`pop`/`len`/`v[i]`,
> byte-identical to `snc llvm` (**54/54 emitted** — `c5d5_loops` joins) + 1 seed + 1 golden,
> behavioural (cc==inkwell) + leak-free; modes 0–3 + effects byte-identical. 🔑 A `Vec<T>` is
> `{ i64 len, i64 cap, ptr data }` (data = FIELD 2, vs `[T]`'s field 1). `vec_new` = the constant
> `{0,0,null}` (a NEW `cgo_operand` kind 2); `push(&mut v,x)` = load len/cap through the `&mut Vec`
> field GEPs, grow if `len==cap` (`sentinel_realloc` to `max(1,cap*2)*sizeof` via `select` +
> GEP-sizeof) + store cap/data back, then `data[len]=x`/`len++` — a grow/cont CFG, NO phi, returns
> i64 0; `pop` = empty-check + decrement + `data[len-1]`; `len`/`v[i]` use the arg's ACTUAL aggregate
> type (`cg_emit_index` gained the target type; data field keyed on `cg_is_vec`). The Vec builtins
> are GENERIC (→ `dump_gcall` → first arg collected); the element type from `strip_ref` +
> `vec_elem_of`. New `sentinel_realloc` declare (`RuntimeSyms`/`cg_used_*` gained realloc). ⚠ the
> oracle lowers both push args before the GEPs (matching the Sentinel collect-both-first, for
> side-effecting push elements — the lexer pushes computed bytes). **The grow-CFG matched the
> differential FIRST try.** **NEXT = (8d-Vec-2)** `vec_to_array` (extract len/data + `sentinel_alloc`
> + `llvm.memcpy` the live prefix + build `[T]` → `c5d3_collections` (D.3 phase-go) emits) → **heap
> drops** (DropPlan `sentinel_free`; byte-parity-NEUTRAL for behaviour, needed for a clean
> fixed-point) → (8e) enums/match → (8f) calls/recursion/multi-module → **(8g) the bootstrap
> fixed-point**. ⚠⚠ The behavioural test rebuilds each emitted fixture twice (~83s at 54) —
> **SAMPLE/CACHE it now** (the cost is `snc build` + `cc` per fixture).
> Sub-slices (8a–8l) in ADR 0045 D10 (Bar A 8a–8g → the FIXED-POINT; Bar B 8h–8l → full corpus →
> ADR 0045 ACCEPTED). `/tmp/tb` build/run/leak + the four norms unchanged (below).
>
> **(7/N) recap** (ADR 0044, A1–A5) reused `types.sentinel` via `types::run`-with-`mode`:
> `mode 2` builds + dumps the MIR (a FUSED `if mir_on(c)` side-build of the pass-2 walk +
> a `lastval` ctx field; flat append-only SSA pools; `if`/`&&`/`||` → branch + a VarId-sorted
> merge; a `margs` operand stack; `mir_suppress` for place stores + handle arms); `mode 3`
> reuses that build then runs `verify_constant_time` (4 sink checks on `is_secret`). Reusable:
> (a) a ctx-field `&mut (*c).field` to a USER fn re-borrows `c` (render to a local + `push`-fold);
> (b) the ONLY type-clean MIR leak is a secret `&&`/`||` Branch; (c) the verifier's corpus
> differential is a no-false-positive sweep + the one `c52_secret_leak` positive.
> **The BORROW-CHECK stage
> (6/N, ADR 0043) is COMPLETE — ADR 0043 → ACCEPTED.**
> `selfhost/borrow.sentinel` matches `snc borrow` byte-for-byte over the ENTIRE
> clean-borrow corpus — **123/123 fixtures**
> (`sentinel_borrow_checker_matches_oracle_on_corpus`, the D8 phase-go) + 5 seeds,
> leak-free; `snc types` stays byte-identical. **The port now has lexer + parser +
> resolve + types + effect-check + borrow-check — the WHOLE FRONT END + both analysis
> passes**, each differentially validated against the Rust `snc`.
>
> **(6/N) this session** (ADR 0043) was the **back-half inflection** + the FIRST stage to
> REUSE a prior one. borrow-check needs the full typed program (Copy/Move per binding), so
> the move analysis is **fused into the typer's pass-2 walk** (a TyCtx `consuming: bool`
> save/restore flag — false at receivers/borrows/conditions/match-scrutinee, true at
> user-fn/construct/method args + let-RHS; the `Var` arm records iff `consuming &&
> is_move_type(env[vid])`); `snc types` stays byte-identical (the recording is a pure
> side-effect). `types.sentinel`'s `main` → **`pub fn run(src, mode, result)`** (mode 0 =
> the type dump, mode 1 = `dump_moves`); `borrow.sentinel` is ~10 lines reusing
> `types::run` via a D.6 chain (borrow→types→parser). ⚠ oracle-revealed: **builtin args
> are non-consuming** (only user-fn args move, `argcons = fid>=14`); **a match scrutinee
> is not a move**; the branch-merge is a plain UNION (no per-path state).
>
> **(7/N) = the LAST ANALYSIS PASS — NOT the transform half (the scout REFRAMED it).** The
> back-half scout found the handover's "HIR/MIR → codegen" is three different things: **HIR
> is a no-op** (a 101-line identity bundle), **MIR is an analysis SIDE-BRANCH** (it feeds
> ONLY `verify_constant_time`; `compile_to_object(hir)` reads the `TypedProgram` directly
> via `hir.program()` — codegen IGNORES MIR, confirmed), and **codegen is the real
> transform** (→ a separate **8/N**). So (7/N) ports `lower_to_mir` (TypedProgram → a
> minimal SSA/CFG IR) + `verify_constant_time` (the secret-leak gate) — the 4th + final
> typed-program gate (types→effect→borrow→**const-time**). **ADR 0044 (PROPOSED) settles
> the plan:** oracle = a new **`snc mir` lowered-form dump** (the LOAD-BEARING differential
> — the verifier is near-empty on clean fixtures, so the IR dump carries it); reuse = the
> **6/N `types::run`-with-`mode` template** (a new `mode 2`; `mir.sentinel` is thin); data
> model = parallel-Vec SSA (⚠ `var_defs` `VarId→MirValue` is index-assigned in Rust → use
> the resolve append-only-scope idiom; the `if`/`&&`/`||` branch-merge is the central new
> muscle + a ⅛-scale codegen rehearsal). **The (7a) PROBE — twin-walk vs fused `mode 2` +
> the `var_defs` SSA model — is the de-risk gate; settle it + re-verify `snc types`/`snc
> borrow` 123/123 byte-identical BEFORE lowering logic** (the 0043 A1 discipline).
> Sub-slices (7a..7e) in ADR 0044 D8. Clean stage boundary, no in-flight slice. **8/N =
> codegen + the bootstrap fixed-point** (its own ADR — Sentinel has no LLVM FFI, so emit
> `.ll` via `write_file` + external `llc`/`clang`; the oracle likely shifts to behavioural
> run-parity). `/tmp/tb` build/run/leak + the four norms unchanged (below).

For pasting into a fresh chat to bootstrap context:

    Continue Sentinel-lang (self-hosting compiler port). Repo:
    /Users/bryan/Desktop/github_repos/Sentinel-lang — a Rust workspace under crates/
    building `snc`, plus selfhost/*.sentinel (the compiler being rewritten in Sentinel
    itself, each stage differentially validated against the Rust `snc` oracle).

    Verify HEAD with `git log -1` — HEAD is a `docs: session handoff` commit (this verify-HEAD +
    RESUME-AT refresh), atop the **recon-validated recipe for cross-unit effect perform/handle**
    (`e98b233`, docs). ⬇ THIS SESSION (25 commits) built the **per-unit SEPARATE-COMPILATION back end**
    (ADR 0037 (a)) from the proposed ADR to broadly functional — `snc build --separate` compiles each module
    to its OWN `.o`, linked by module-qualified `abi-v1` symbols, for EVERY `pub` item kind. Chain (newest
    feats first): `a0e9595` cross-module `pub effect` decls (first cut) · `934f08c` cross-module traits +
    module-qualified class/impl methods · `9a8376e`/`f533a62` cross-module generic fns (over primitives + a
    cross-module struct) · `5f59591` type-in-signature fns · `7b3529b`/`c2646ab` cross-module enum/struct
    layout import · `b8d0ecb` `run_build_separate` (THE WORKING BACK END — D10 phase-go: two `.o` linked via
    `_S4util4math3add` → exit 42) · `55bb5f6` `compile_to_object_for_module` + extern decls · `a6c6113`
    `check_module` · `8bd44c3` `resolve_module` · `671b012` the D7 `mangle_qualified` ABI freeze ·
    `c8ce078`/`7696f5d` the ADR 0037 flip (D7/D5.1/D5.2 PINNED, the ADR-first gate). **1513 tests, four-check
    green, both bootstrap fixed points byte-identical across ALL 25 commits** (the per-unit machinery is
    ADDITIVE — dormant on the single-file / Path-A-merge paths, which stay byte-unchanged). Atop the prior
    **session handoff** (`4825529`),
    atop the **scg parser-hardening mirror** (`99d5ee1` — feat(selfhost): mirror
    the P2.5 parser deep-nesting hardening into scg. The self-hosted parser had the same DoS (SIGSEGV at
    ~6000 nested brackets). A faithful depth limit would thread `&mut i64` through ~30 recursive parser
    fns (high churn / differential risk) and the Sentinel parser has no error channel (ADR 0043 D5/D7),
    so instead a cheap pre-pass: `guard_nesting` (new in parser.sentinel) scans the token stream for max
    BRACKET nesting and, if >256, CLEARS it so the walk sees an empty program — called after `tokenize`
    by both the parser `main` AND the shared `types::run` [types/borrow/mir/scg-codegen route through it].
    Inert for valid programs [nests <~30] → all differentials + both fixed points green [1497 tests];
    6000 parens now exits cleanly [was SIGSEGV]. Covers the demonstrated paren/brace/bracket DoS;
    operator-ambiguous nesting [unary/generic] is a residual whose failure stays a CONTROLLED abort; the
    `pub`-alone case needs no mirror [the Sentinel parser already fails it as a memory-safe bounds-check
    abort, not snc's `unreachable!()`]), atop the
    **front-end panic-freedom fuzzer + 2 bug fixes** (`ce2ad2e` —
    review-plan P2.5 / F11: a dependency-free, deterministic, CI-integrated fuzzer
    [`crates/sentinel-syntax/tests/fuzz_panic_freedom.rs` — random bytes + token soup + corpus
    mutations; `SENTINEL_FUZZ_ITERS` override, 2M-iter campaign clean] that FOUND + FIXED two real
    parser bugs: a DoS stack overflow on ~256 nested `(` → a `MAX_EXPR_DEPTH=128` guard +
    `ParseError::RecursionLimit`, and a panic on `"pub"` alone [trailing `pub` hit an `unreachable!()`]
    → a clean `unexpected_eof`. Both error-path-only → differentials + both fixed points unaffected;
    the selfhost parser.sentinel mirror is a follow-on), atop the
    **CT-model doc + secret-flow conformance suite** (`ec81d77` —
    review-plan P2.1 + P2.2 / F3: `docs/ct-model.md` catalogs every construct's CT modeling [4 sinks;
    PRECISE vs `Opaque`/`Call`-conservative vs off-the-1.0-CT-path; handler arm bodies are unchecked]
    + 7 ui rejects + 1 pass [`c52_secret_*`] proving taint SURVIVES the Call/field/match `Opaque`
    funnels → `mir::secret_leak`, the direct sinks caught at the type level, the no-sink accept side at
    exit 80; probing found NO gap; +8 tests, 1493 total), atop the **lookup_var loud-fallback guard**
    (`11c4c78` — review-plan P2.3 / F4: a `debug_assert!` makes an unbound-VarId resolver bug LOUD in
    debug/test builds [release keeps the total fallback], distinguishing the benign `match`-arm-binding
    case via a new `expected_unbound` set; output-neutral, so differentials + selfhost + fixed points
    are unaffected — the full corpus passes with the assert ACTIVE), atop the
    **PROGRAMMING_GUIDE rewrite** (`2aac7cd` — review-plan P3.1 / F6:
    the guide was pre-C1 "i64 only" + untyped `fn f(x)` [now a parse error]; rewritten as a tour of
    CURRENT Sentinel covering every feature [types/let, operators, fns, if/while, structs, enums+match,
    refs/moves/borrow, strings/[u8]/Vec, file I/O, generics, ?T, secret+CT, effects, classes/traits/
    delegation, concurrency, modules], EVERY example real current syntax drawn from / verified against
    the CI-tested `tests/pass/` fixtures [each adapted snippet compiled + run to its claimed exit code,
    incl. a multi-file module build], C0 demoted to a historical appendix; docs-only), atop the
    **P0 claim-calibration batch** (`0810491` — docs: P0 claim
    calibration and posture, review-plan P0.1–P0.6, docs/config only: the README constant-time claim
    reworded from "proves it" to the precise property + its two boundaries [type system as taint oracle;
    pre-LLVM, no forced emission — copied from the sentinel-mir doc comments]; the borrow checker's
    posture stated honestly [the under-rejection is CLOSED; remaining limits are over-rejections];
    `CONTRIBUTING.md` created; `SECRETS_LIFECYCLE.md` got a VISION/not-on-roadmap banner; `ci.yml` +
    `just check-all` gained the missing `cargo test --doc` step; rode-along Status staleness fixes
    [1476→1484 tests, Bar B done not "Remaining"]), atop the **ADR 0046 ACCEPTED docs** (`cfc055d` — ADR
    flip + STATE banner + borrow-check-limitations + HANDOVER), atop the
    **scg-mirror feat** (`714ce3f` — feat(selfhost): mirror the partial-move-through-field skip into scg,
    ADR 0046 D6: the self-hosted `scg` no longer double-frees a Move-typed field consumed by value. The
    `snc borrow` dump [`borrow_dump.rs`] gains the `#<vid>.<field>` partial-move suffix; the `snc llvm`
    `.ll` oracle [`llvm_dump.rs`] gains the drop field-skip [A1 — the feat below updated only inkwell, so
    the `.ll` oracle was stale]; `selfhost/types.sentinel` mirrors record + dump + the mode-4 drop-skip,
    detecting the direct-Var base via a new mode-independent `mvbv` channel [A2 — Sentinel match can't peek
    the AST]; an EXISTING fixture `c17_go_no_go` already exercised it [A3 — `fst`/`snd` RETURN a generic
    field by value]; +3 fixtures [reproducer exit 37 / non-consuming regression exit 4 / use-after reject];
    borrow + codegen differentials byte-identical, both fixed points hold, 0 leaks, four-check green, 1484
    tests; ADR 0046 → ACCEPTED-WITH-AMENDMENTS A1–A3), atop the **partial-move soundness feat** (`a16881b` —
    ADR 0046: the gap CLOSED in `snc`; per-(VarId, field) move state in sentinel-borrow-check + the
    `DropPlan.moved_fields` drop-skip in sentinel-codegen; reproducer accepted+correct; 5 unit tests), atop
    the **ADR 0046 PROPOSED** doc (`162d8ae`), atop the
    `docs(selfhost): ADR 0045 A34 — concurrency + ACCEPTED` commit (`672885a`), atop the
    **structured-concurrency feat** (`0f360cf` — scope/spawn/await; **FULL-CORPUS PARITY**, emitting set 121 →
    123: c44_go_no_go [scope+spawn+await] + c4_go_no_go [the full C4 surface]; ALL 123 PASS FIXTURES NOW EMIT →
    **Bar B COMPLETE, ADR 0045 ACCEPTED**; oracle 5 RuntimeSyms + `Emit::current_scope` +
    `collect_spawn_targets_*`/`dump_spawn_wrapper` + `Type::Task → ptr` [lower_expr now EXHAUSTIVE — catch-all
    gone]; Sentinel Scope/Spawn/Await cg [Spawn branches on cg_on], `cg_scope`/`cg_spawn_t` +
    `cg_emit_spawn_wrapper` into cgcls, `cg_is_task → ptr`; un-parsers unchanged [concurrency out of
    source_dump's Bar-A scope, like declassify]), atop the
    **effects/handlers c36b feat** (`b63cc98` — nested handles, Kont*-merge + propagate; THE LAST HANDLER
    SLICE; emitting set 119 → 121: c36b_nested_handle_basic + _inner_full [exit 42/42]; oracle `lower_handle`
    gains a `handle_depth` counter [is_nested = depth>1] + `lower_handle_inner` + `store_handle_result` [nested
    arms/pure wrap i64→ptr] + propagate default + nested-Handle body treated as kont-producing; Sentinel
    `cg_h_depth` + `is_nested`→`dump_tharms`/`cg_store_hresult` + the passthrough/propagate tail + `cg_tailk =
    is_nested`), atop the
    **effects/handlers c36a feat** (`caf4175` — handle `return` arm + pure-body wrap; emitting set
    116 → 119: c36a_return_arm_transform + _return_arm_after_resume + c37_handle_return [exit 42/42/84]; oracle
    `lower_handle` drops the return-arm gate + wraps pure bodies + `apply_return_arm` inlines at the dispatch
    pure block AND `lower_resume_kont`'s k(v) path; Sentinel RE-PARSES the body [`Ret::YesRet` gains 2 token
    indices, tokens copied to `TyCtx` only when mode 4 + a `return` token; `cg_apply_return_arm` via
    `parse_expr`]; the `YesRet` AST change rippled to 4 stages [bind+ignore]; pure-body via `cg_tailk`), atop
    the **effects/handlers c35e feat** (`6bdd23b` — chained effecting lets; emitting set
    113 → 116: c35e_chained_perform + _chained_dependent_perform + _chained_perform_with_capture [exit 42
    each]; oracle `detect_chained_lets_shape`/`dump_chained_lets_fn` + `compute_chained_captures`; Sentinel
    `cg_chained_emit` [3-phase: re-parse-bind / consume-original-for-lowering / on-demand capture re-parses]
    + `cg_walk_ex` collector/disposal + `cg_chained_parent`/`cg_chained_resumer`/`cg_caps_collect`; ⚠ the
    SELF-COMPILE surfaced a scg quirk — an inline discarded `match` defaults to `ptr` not `i64`, fixed by the
    tail-position `cg_caps_collect` helper), atop the
    **effects/handlers c35d feat** (`ecd150c` — embedded perform via placeholder substitution; emitting set
    110 → 113: c35d_binop_with_perform + _perform_in_call_arg + _perform_with_capture_and_binop [exit 42
    each]; oracle `detect_embedded_shape`/`dump_embedded_shape_fn` + `Emit::embed_ph` [the Perform arm
    emits a load, no substituted tree]; Sentinel re-parsed CLASSIFICATION COPY → `eff_classify` →
    `cg_embed_emit` + `TyCtx.cg_ph`/`cg_emit_phload`) + the A30 docs (`6ffa4de`), atop the prior session
    handoff (`5b1152e`), atop
    `docs: update README for Phase D self-hosting` (`d70de5c` — refreshes the stale README to reflect that the
    compiler now self-hosts; no code change), atop the `docs(selfhost): ADR 0045 A29 — effects c35c` commit
    (`f76ca76`), atop the
    **effects/handlers c35c feat** (`96c54b9` — let-bound perform + the captured frame; emitting set
    107 → 110: c35c_let_bound_perform + _with_capture + the C3.7 phase-go c37_go_no_go [perform-with-arg +
    captured var + print → stdout 85]; the FIRST sub-slice emitting TWO defines per fn + using
    `sentinel_kont_push`) + the A28/c35b docs + the **effects/handlers c35b feat** (`02891fd` — the
    effecting-fn `Kont*` ABI + pure-return; emitting set
    101 → 107: c35b_handle_{fn_call_body,multi_arm,pure_return} + c32/c33_go_no_go + the C5 phase-go
    c5_go_no_go) + the A27/c35a docs + the **effects/handlers c35a feat** (`29e3027` — inline perform/handle/
    resume; emitting set 98 → 101) + the A26-scout docs (`43863f6` — SUPERSEDED) + the **classes/traits/impls/
    delegates feat** (`a1a3341` — Bar B's 6th construct; emitting set 92 → 98) + the A26 docs (`90b5586`)
    + the A25/generic-fns-mono feat (`170a13a` — GENERICS COMPLETE) + the A24/generic-structs feat
    (`d3be39b`) + the A23/secret feat (`7b471a9`) + the A22/nullable feat (`8150ccc`) + the
    **A21**/print docs (`749d32a`) + the `print` feat (`391ae58`). Below: the **A20** path-(a) commit
    (`6355c23` — PATH (a) COMPLETE, the
    self-hosted merge) + the path-(a) feats (a-4 scg self-merges `d33397c`, a-2/a-3 the self-hosted merge
    `ef56104`, a-1 the un-parser `71efa09` / A19 docs `d13d17e`), atop the 8g fixed-point (A18 docs
    `7054145` + feat `41f00fa`). The 8/N codegen chain below: (8f-2/8f-3) snc llvm lowers the full compiler
    (`59c30a7` A17 + `67fa808`) · (8f-1) selfhost stages self-host (`8b89726` A16 + `3c389cd`) · (8e)
    enums+match (A14/A15) · heap drops (A11–A13) · Vec (A9/A10) · 8d refs/builtins · 8c aggregates · 8b
    control flow · 8a scalars+oracle. **ADR 0045 → ACCEPTED-WITH-AMENDMENTS (A1–A34); BAR B COMPLETE —
    `print` + nullable + secret + generics + classes + ALL effects [c35a/b/c/d/e + c36a/c36b] + structured
    concurrency [scope/spawn/await] done, ALL 123 PASS FIXTURES EMIT byte-identically `scg` == `snc llvm`** (the
    20 ui/ negatives Err by design); the
    **bootstrap fixed point is reached via BOTH paths** — (b) 8g merge-to-source + (a) the self-hosted merge (`scg` discovers+merges+emits itself).
    **The full-corpus codegen-parity goal is MET. The remaining deferred track is the per-unit
    separate-compilation back end (ADR 0037 (a)) — independent of the port.**
    ⚠ The dev pushes via GitHub Desktop — `git status` may show "ahead N" (uncommitted-to-origin local
    commits); that's expected, never push.
    Clean tree; four-check green (cargo build + `cargo nextest run --workspace` + `cargo test
    --doc --workspace` + `cargo clippy --workspace --all-targets -- -D warnings`); **1476 tests**
    (the codegen differential [57/57] + `sentinel_codegen_matches_oracle_on_selfhost_stages` [lexer+parser
    self-host byte-identically] + `snc_llvm_lowers_the_merged_compiler` + **`sentinel_codegen_reaches_the_
    bootstrap_fixed_point`** [the (8g) capstone] + the `snc llvm` oracle tests `tests/llvm.rs`;
    `mir`/`ctverify`/etc. still 123/123). macOS + LLVM 18 (clang/llc/opt 18.1.8, arm64-apple-darwin — the
    (8/N) `.ll` → object → link toolchain).

    STATE OF THE PORT: lexer (1/N) + parser (2/N, ADR 0039) + resolve (3/N, ADR 0040)
    + types (4/N, ADR 0041) + effect-check (5/N, ADR 0042) + borrow-check (6/N, ADR 0043)
    + **MIR-lowering + const-time (7/N, ADR 0044)** — **ALL COMPLETE + ACCEPTED.** The whole
    front end + both analysis passes + the transform-half's MIR lowering + the const-time
    verifier are ported, each matching its `snc <stage>` oracle byte-for-byte over the
    corpus, leak-free: `snc lex`/`ast`/`resolve`/`types` (123/123) / `effects` (122/122) /
    `borrow` (123/123) / `mir` (123/123) / `ctverify` (123/123). selfhost/ has lexer,
    parser, resolve, types, effects, borrow, mir, ctverify, **codegen** .sentinel. **(8/N)
    CODEGEN + THE BOOTSTRAP FIXED POINT — DONE** (ADR 0045 A1–A18): `snc llvm` (the canonical-`.ll`
    oracle) + `selfhost/types.sentinel` mode-4 emit byte-identical `.ll` over the whole corpus, and
    **`scg` (the Sentinel-built compiler) lowers the FULL merged compiler `.ll`-identical to the oracle
    (83,536 lines) AND `cc`-ing that `.ll` yields `scg'` which re-emits the same `.ll` byte-for-byte —
    THE SENTINEL COMPILER COMPILES ITSELF** (8g, path (b) merge-to-source). Remaining for ADR 0045 →
    ACCEPTED over the full corpus: **Bar B** (generics/nullable/classes/effects/concurrency), or an
    owner-deferred close at the fixed-point — see RESUME AT.

    🎯🎯🎯 **(8g) THE BOOTSTRAP FIXED POINT IS REACHED (ADR 0045 A18) — THE SELF-HOST CAPSTONE. THE
    SENTINEL COMPILER COMPILES ITSELF.** `scg` (`snc build`, inkwell) lowers the WHOLE merged compiler to
    `.ll` **byte-identical to the `snc llvm` oracle** (83,536 lines); `cc` that `.ll` → `scg'`, which
    re-emits the **same** `.ll` byte-for-byte (a true fixed point). Owner-chosen **path (b),
    merge-to-source**: the Rust driver `merge_modules` + a new `crates/sentinel-driver/src/source_dump.rs`
    un-parser print the multi-file compiler to ONE `$`-qualified `.sentinel` (`snc merge <entry>`), fed to
    the unchanged single-file `scg` — every STAGE (lex→…→codegen) runs in `scg`; only the module-merge
    pre-pass stays in Rust. Enablers: a `$`-in-identifier lexer extension (Rust regex +
    `parser.sentinel`/`lexer.sentinel`, corpus-neutral); the Rust-only round-trip gate
    (`snc llvm <merged-source>` == `snc llvm <entry>`). TWO (8g)-revealed `types.sentinel` cg gaps (only
    in the merged `types`/`codegen`, not the corpus/lexer/parser) fixed to match the oracle: **(i)**
    field-place GEP base — `&mut c.f`/`c.f = x` on a LOCAL struct → GEP the var's alloca SLOT
    (`cg_slot_get`, `cg_lastvid>=0`→slot else operand), not the stale operand the A17 `cg_suppress` left
    (which was right only for a `*c` Deref target); **(ii)** `match` `_` wildcard → body+`store`+`br merge`
    as the final else (a save/restored `cg_m_wild` flag), not `unreachable`. Capstone test
    `sentinel_codegen_reaches_the_bootstrap_fixed_point`; **1473 tests**, modes 0–4 byte-identical, `scg`
    leak-free (`leaks --atExit`: 0 leaks lowering the merged compiler), four-check green.

    ▶ **RESUME AT: the per-unit separate-compilation back end (ADR 0037 (a)) — (3/N) INCREMENTAL CACHING WORKS
    (`299f17c` per-unit `.o` reproducibility + `ced9130` the cache): a `--separate` rebuild reuses an unchanged
    unit's cached `.o` (a `unit_fingerprint` → `<obj>.o.fp` sidecar; editing one module recompiles only it +
    its importers, the rest print `fresh <module>`) — THE PAYOFF of separate compilation, sound because per-unit
    codegen is reproducible. (2/N) is otherwise complete: EVERY pub item kind crosses a boundary (incl. cross-UNIT
    effect perform/handle `b43b7d6`+`4c3a28b`), and `linkonce_odr` GENERIC DEDUP covers PRIMITIVES (`c5ca3b8`
    +`8d14db8`) + cross-module STRUCTS (`a75a62c`+`7c6767a`) + ENUMS (`7d5dd46`) via origin-qualified mono tags
    (`id__util$geo$Point` / `id__shape$Shape`, `mangle_type_dedup` + a `NamedTypeOrigins { structs, enums }`
    bundle; same-named cross-module types stay distinct, proven sound). The cache fingerprint is now
    ITEM-granular (`63d93cc` — a body-only change to an imported fn doesn't recompile importers, they relink).
    See the ▶ NOW ON block below. NEXT (remaining tail, all LOWER value): extend the type-tag fix to class /
    generic-instance type args (add their arms to `mangle_type_dedup` + a `NamedTypeOrigins` field — same
    shape, rarer) + dedup cross-module trait/class METHODS (mirror the generic-fn `generic_origins` model with
    a method-origin map). The self-host port AND the ADR 0046 partial-move soundness fix are both
    COMPLETE.** **The
    self-host port: Bar B closed, ADR 0045 ACCEPTED-WITH-AMENDMENTS
    (A1–A34): all 123 pass fixtures emit byte-identically `scg` == `snc llvm`, and the bootstrap fixed point
    holds via BOTH paths.** **ADR 0046 (the partial-move-through-field double-free, review-plan P1.2):
    CLOSED in `snc` AND `scg` → ACCEPTED-WITH-AMENDMENTS A1–A3** (feat `a16881b` snc + `714ce3f` the scg
    mirror): per-(VarId, field) move state in the borrow checker + the `moved_fields` drop-skip in BOTH
    codegen backends (inkwell + the `snc llvm` `.ll` oracle — A1), mirrored into `selfhost/types.sentinel`
    (record + dump + mode-4 drop-skip; the direct-Var base via the new `mvbv` channel — A2); the borrow +
    codegen differentials are byte-identical over the whole corpus (incl. `c17_go_no_go`, which RETURNS a
    generic field by value — A3) and both fixed points hold; +3 corpus fixtures (reproducer exit 37 / a
    non-consuming-read regression exit 4 / a use-after-partial-move reject), 0 leaks, 1484 tests. The borrow
    checker's remaining limitations are all OVER-rejections (ergonomics, ADR 0018) — sound. **The
    review-plan P0 band is ALSO DONE** (`0810491`, docs/config only): P0.1 the README constant-time claim
    calibrated ("proves it" → the precise property + its two boundaries, copied from the `sentinel-mir`
    doc comments: the type system is the taint oracle, and the check runs pre-LLVM so it constrains the
    program, not the optimized machine code / forced emission); P0.2 the borrow checker's posture stated
    honestly; P0.3 `CONTRIBUTING.md`; P0.4 the `SECRETS_LIFECYCLE.md` vision banner; P0.5 the stale ADR
    0019 pointer (already gone); P0.6 the CI/justfile `cargo test --doc` step. So **P1.1 + P1.2 + the
    whole P0 band are complete — AND review-plan P3.1 (the PROGRAMMING_GUIDE rewrite, `2aac7cd`) AND P2
    (P2.3 `11c4c78` + P2.1/P2.2 `ec81d77` + P2.5 `ce2ad2e` + the scg parser-hardening mirror `99d5ee1`)
    are done — only P2.4 remains in P2.**
    **▶ NOW ON: THE PER-UNIT SEPARATE-COMPILATION BACK END (ADR 0037 (a)) — the big deferred architectural
    track (per-unit `.o` + module-qualified `abi-v1` mangling + multi-object link + `linkonce_odr`
    cross-module generics → incremental builds; independent of the port). ✅ STEP 1 DONE (ADR-FIRST): ADR
    0037 is FLIPPED toward the per-unit (1/N) (`7696f5d`, docs-only) — the frozen-ABI decisions are PINNED
    before code: (D7 mangling) `_S` + length-prefixed module segments + a length-prefixed item; **empty
    module path → the bare item**, so single-file ABI is byte-unchanged → an AMENDMENT not abi-v2;
    `main`/`sentinel_*` exempt; `_S` reserved; item = the existing intra-module abi-v1 §4 symbol wrapped as
    ONE length-prefixed blob. (D5.1 per-unit ID model) the extern-fn-in-FnId-space model — an imported fn is
    an extern `FnSignature` + a module-qualified `link_symbol`, no body, NO new expr variant; imported types
    are layout-only (no symbol); `resolve_module(program, imports)` generalizes `resolve()` (`imports==[]` =
    today's single-file path, byte-unchanged). (D5.2 codegen boundary) the 3 whole-program assumptions:
    whole-program `collect_mono_instantiations`→per-unit+`linkonce_odr` (2/N); the single
    `fns: HashMap<FnId, FunctionValue>`→per-unit, externs DECLARED; `self.fns.get(&id)`→mechanically
    unchanged (returns a declaration for externs, the linker binds it). Built ADDITIVELY (opt-in `--separate`
    until (2/N) parity, then default; the Path A merge + `snc merge` + BOTH bootstrap fixed points stay
    green). ✅ **CODE STEP 1 DONE: the D7 mangling primitive** (`671b012`, feat) — `mangle_qualified(module_path,
    item)` (empty path → the bare item; non-empty → `_S` + length-prefixed module segs + length-prefixed item)
    + the `abi_v1_mangling_qualified_is_stable` golden test + the `docs/abi-v1.md` §4/§7/§8 amendment, all in
    ONE commit (the C5 D7 discipline). WIRED BEHAVIOR-PRESERVINGLY at the free-fn site with an EMPTY module
    path (== `signature.name`), so single-file ABI is byte-unchanged: the pre-existing `abi_v1_mangling_is_stable`
    + the selfhost differentials + BOTH fixed points stay byte-identical (four-check green, 1498 tests). Not an
    oracle-moving change → no selfhost mirror needed yet. ▶ **NEXT (D.6 1/N): the COHESIVE per-unit vertical
    slice** (resolve + codegen + driver together — `use` does not RUN until all land):
    ✅ (a) DONE (`8bd44c3`): `resolve_module(program, imports: &[ImportedFn])` in sentinel-resolve —
    `FnSignature.extern_origin: Option<Vec<String>>`; an imported `pub fn` = an extern (sig + fn_table entry, no
    body) after builtins / before own fns; `resolve()` = `resolve_module(program, &[])` (single-file
    byte-identical); the `UseDeclNotYet` gate now fires only when `imports==[]`. Carries `origin` (the module
    path), NOT a pre-mangled `link_symbol` — codegen owns ALL mangling (LOCAL = `mangle_qualified(self.module_path,
    name)`, EXTERN = `mangle_qualified(&origin, name)`).
    ✅ (b-types) DONE (`a6c6113`): `check_module(resolved, imports: &[TypedImportedFn])` —
    `TypedFnSignature.extern_origin` (propagated); each extern's typed sig built from its `TypedImportedFn`
    {name, param_types, return_type, effect_row} (id from the resolved extern; the existing `sort_by_key(id)`
    places it; externs have no body → the body loop skips them); `check()` = `check_module(resolved, &[])`.
    ✅ (b-codegen) DONE (`55bb5f6`): `compile_to_object_for_module(hir, module_path, output)`
    (`compile_to_object` delegates with `&[]`); the free-fn site computes the symbol by case — `is_main` →
    bare `main`; `extern_origin == Some(origin)` → DECLARE external (`mangle_qualified(&origin, name)`, no
    body — Pass 2 iterates `program.fns` which excludes externs); else LOCAL = `mangle_qualified(module_path,
    name)`. ⚠ class/impl/mono mangling stays UNqualified (free-fns only, this slice). Byte-identical for the
    empty path.
    ✅ (c)+(d) DONE (`b8d0ecb`) — **`snc build --separate` WORKS END-TO-END.** `run_build_separate`:
    discover → `resolve_imports` (the PrivateItem/ModuleNotFound/UnknownImport gate, reused) → a pub-signature
    PRE-PASS (`extract_exports`, a lightweight AST walk — FIRST SLICE = SCALAR sigs i64/i32/bool/u8, non-generic,
    pure) → the exports table keyed by `(module_path, fn_name)` → per module: build `ImportedFn` +
    `TypedImportedFn` externs from the module's `use`s, then `resolve_module → check_module → effect → borrow →
    CT → hir → compile_to_object_for_module(&m.path)` → one `.o`; the entry (module 0) must have `main`, a
    non-entry `main` is rejected → `link_objects` (path-sorted `cc` + runtime). ⚠ TWO whole-program assumptions
    relaxed for library units (no `main`): `MissingMain` moved from `resolve_module` to the `resolve()` wrapper;
    `effect_check` Pass 3 guards the `main` lookup (both behavior-preserving for the corpus — all has main). The
    D10 phase-go is GREEN: `main.sentinel` `use`s `add` from `util/math.sentinel` → main.o + util_math.o emitted
    INDEPENDENTLY → linked via `_S4util4math3add` → exit 42 (test asserts BOTH `.o` exist) + the two negatives.
    ✅ **CROSS-MODULE TYPES (struct + enum) DONE** (`c2646ab` struct + `7b3529b` enum): a cross-module type
    is LAYOUT-only (NO link symbol, D4), so the importer INLINES the imported decl — the driver clones the
    per-unit program, CLEARS `uses`, and appends the imported `pub struct`/`pub enum` decls (`prog.structs/.enums.extend`);
    `resolve_module` then re-materializes them in the unit's StructId/EnumId space, TRANSPARENT to types +
    codegen (a `ResolvedStructDecl`/variant payload holds id-independent AST `TypeExpr`s, re-resolved per unit).
    No resolve/types signature change. `ExportedItem = Fn|Struct|Enum`; `extract_exports` pulls pub structs
    (non-generic) + enums. Phase-gos: main imports `Point`(struct)/`Shape`(enum w/ payloads) from
    `util/geo.sentinel`, uses/matches LOCALLY → exit 42, both `.o` independent.
    ✅ **TYPE-IN-SIGNATURE DONE** (`5f59591`): a cross-module fn whose sig takes/returns an imported type
    (`sum(p: Point) -> i64`) works. `TypedImportedFn` carries param/return **`TypeExpr`s** that `check_module`
    RE-RESOLVES in the importer's type space (struct/enum tables incl. the inlined imported types) via
    `resolve_type_expr_with_scope` → `Point` maps to the importer's LOCAL StructId; this subsumes the scalar
    path (`scalar_type` removed). The struct crosses by value (units agree on layout). main imports Point+sum,
    constructs Point + passes it → 42.
    🎯 **D.6 (1/N) — NON-GENERIC SEPARATE COMPILATION — is FUNCTIONALLY COMPLETE** (ADR 0037 D9 (1/N):
    `resolve_module` + per-unit codegen + D7 mangling + extern-symbol cross-module fn calls + cross-module
    types [struct/enum layout import, incl. in fn signatures] + deterministic multi-object link).
    ✅ **(2/N) OPEN — cross-module GENERIC FNS WORK** (`f533a62`): a `pub fn id<T>` instantiated in a
    different module is INLINED into the importer + monomorphized LOCALLY (the SIMPLEST correct model;
    `linkonce_odr` dedup DEFERRED, allowed by the ADR Revisit). `ExportedItem::GenericFn(Box<FnDef>)` carries
    the body; the importer `prog.fns.extend`s it (resolve/check/`collect_mono` treat it as a local generic);
    codegen module-qualifies the mono symbol by THIS unit's path: `mangle_qualified(module_path,
    &mangle_mono_name(…))` → `_S4main…id__i64` (each importer self-contains its instances, no clash).
    ⚠ CALLS resolve via the `(FnId, Vec<Type>)` `mono_fns` map, NOT by name — so the symbol rename is ONE
    codegen site (`compile_to_object_for_module`'s mono pre-pass) + the call sites follow automatically; empty
    path → bare = single-file byte-identical. Phase-go: main imports `id` from util/math, instantiates id<i64>
    (inferred from 42) → exit 42. ✅ **generics over a CROSS-MODULE STRUCT work too** (`9a8376e`, test:
    `id<Point>` w/ Point imported → 42). ⚠ KEY FINDING: the type-tag-collision concern (`id<a::b::Point>` vs
    `id<c::d::Point>` mis-keying) does NOT bite the inline-local model — each importer qualifies its instance by
    ITS OWN path (`_S4main…id__Point`) + self-contains the inlined struct, so there is no shared symbol to
    collide. The type-tag fix is needed ONLY for the ORIGIN-qualified `linkonce_odr` model. ▶ **NEXT in (2/N):**
    ✅ **cross-module TRAITS DONE** (`934f08c`): a `pub trait` is INLINED into the importer
    (`ExportedItem::Trait`, `prog.traits.extend`), which impls it for its OWN class + dispatches; the trait is
    a decl (no link symbol), the impl+class are the importer's own. This REQUIRED module-qualifying the LAST 3
    codegen symbol kinds — class init (`Class__init`), class method (`Class__method`), impl method
    (`<prefix>__<Type>__<Trait>__<method>`) — all `mangle_qualified(module_path, …)`; calls resolve via the
    `class_init_fns`/`class_method_fns`/`impl_method_fns` maps by ClassId/ImplId+idx (NOT by name) so the
    rename is localized + byte-identical for the empty-path corpus. ⚠ classes are module-LOCAL (can't be
    `pub`) — the TRAIT is the cross-unit construct, not the class.
    ✅ **cross-module EFFECT DECLS DONE (first cut)** (`a0e9595`): a `pub effect` decl is INLINED
    (`ExportedItem::Effect`, `prog.effects.extend`). FIRST-CUT scope = the `perform` + `handle` BOTH live in the
    importer, so the effect's EffectId (→ runtime `op_id = (eid<<16)|op`) is unit-local + consistent — PURE
    INLINE, NO codegen/eid change (op_id is a NUMBER, not a symbol). io.sentinel `pub effect Io`; entry imports
    it, performs in `do_work` + `handle do_work() with { Io.read(k) => k(42) }` → 42. **So ALL pub item kinds
    now inline: fn (extern + generic inline) / struct / enum / trait / effect-decl.**
    ✅ **cross-UNIT perform/handle DONE — THE HARD CASE — THE LAST (2/N) item-kind** (`b43b7d6` the codegen op-id
    base map [1/2, inert] + `4c3a28b` the feature [2/2]): a library `io::source` PERFORMS in ITS OWN unit, the
    entry HANDLES it in ANOTHER → exit 40. **So ALL pub item kinds now cross a `--separate` boundary: fn (extern +
    generic inline) / struct / enum / trait / effect-decl / effecting fn.** The `EffectId` is OVERLOADED (BOTH the
    `(eid<<16)|op` op-id basis AND the `effect_decls[]` index), so a shared effect can't take a global eid —
    DECOUPLED via a build-wide **op-id base map** (effect NAME → a graph-stable sorted index). (1) Codegen: a new
    `CodegenCtx::encode_op_id_ctx` (field `op_id_base` on `CodegenCtx`; new param on `compile_to_object_for_module`)
    consults the map at BOTH `encode_op_id` call sites (perform + handle dispatch), and on a miss DELEGATES to the
    standalone `encode_op_id` → an EMPTY map (single-file / merge / corpus) is BYTE-IDENTICAL, so the oracle copy
    in `llvm_dump.rs` is UNTOUCHED and both fixed points stay green; the `compile_to_object` wrapper +
    `run_build_merged` pass empty, `run_build_separate` computes name→index sorted from all modules' own effects.
    (2) Coupled — extern EFFECTING fns: `extract_exports` stops rejecting `effect_row` `pub fn`s, and
    `ExportedFn`/`TypedImportedFn` carry `effect_row_names`; `check_module` RE-RESOLVES them to the importer's
    EffectIds (effect analogue of the param-TypeExpr re-resolution `5f59591`, against the driver-inlined effect
    decls) → the non-empty row drives the extern's Kont* ABI via `uses_kont_abi`; a name not `use`d → the new
    `TypeError::UnknownImportedEffect`. ⚠⚠ **KEY FINDING beyond the recon recipe** — the recipe's "multi-effect
    importer so eids differ" is NECESSARY but NOT SUFFICIENT for a load-bearing test: a SINGLE-arm top-level
    handler has an `unreachable` default, so LLVM collapses the lone arm to run UNCONDITIONALLY (ignoring the op
    id) → a WRONG id still passes. The load-bearing phase-go needs the wrong id to hit a DIFFERENT REACHABLE arm:
    the entry handler lists arms for BOTH its own `Local` (local eid 0) AND imported `Io` (local eid 1); without
    the base map the io-unit kont's id (0) COLLIDES with the `Local` arm → resumes `k(7)` → exit 7, with it
    `Io`→base 0 selects `Io.read` → exit 40 (verified by hand: removing the base map flips 40→7). ⚠ base map keyed
    by NAME (MVP) → same-named cross-module effects collide (origin-qualified = the robust upgrade). Tests:
    `separate_cross_unit_perform_handle_compiles_and_runs` (40) + `separate_effecting_import_without_its_effect_is_rejected`.
    ✅ **`linkonce_odr` GENERIC DEDUP — PARTIALLY REALISED** (`c5ca3b8` codegen [1/2, inert] + `8d14db8` feature
    [2/2]): a mono instance of an IMPORTED generic over COLLISION-SAFE args (primitives — `mono_args_dedup_safe`)
    emits under the ORIGIN-qualified symbol (`_S4util4math…id__i64`) with `linkonce_odr` linkage, so N importers
    share ONE definition (the linker dedups). The driver records each imported generic's (name, origin) at the
    `use` site, maps name→resolved FnId after resolve, and threads a `generic_origins: HashMap<FnId, Vec<String>>`
    to `compile_to_object_for_module` (mirroring the op-id base map; the `compile_to_object` wrapper +
    `run_build_merged` pass empty → byte-identical). NAMED-type args stay per-importer (sound) — gated by
    `mono_args_dedup_safe` because `mangle_type` renders a named type by BARE name (point 8). Load-bearing test
    (`separate_cross_module_generic_is_linkonce_deduped_across_importers`): two importers of `util::id<i64>` → (a)
    LINKS (two EXTERNAL defs would be a duplicate-symbol error — confirmed by hand: disabling the linkage →
    "1 duplicate symbols") + (b) `nm` shows ONE symbol (the inline-local model emits two importer-qualified copies)
    → exit 42.
    ✅ **TYPE-TAG FIX (ADR point 8) DONE FOR STRUCTS** (`a75a62c` codegen [1/2, inert] + `7c6767a` feature
    [2/2]): a cross-module struct's tag in a `linkonce_odr` mono key is ORIGIN-qualified (`id__util$geo$Point`,
    `$`-joined path) via a NEW `mangle_type_dedup`/`mangle_mono_name_dedup` (a `mangle_type` variant that
    qualifies a cross-module struct + DELEGATES everything else, so they stay in lock-step) + a driver-threaded
    `struct_origins: HashMap<StructId, Vec<String>>` (built like `generic_origins`: record imported struct
    (name, origin) at the `use` site, map name→resolved StructId after resolve). `mono_args_dedup_safe` WIDENED:
    primitive OR cross-module struct (origin known) OR array/nullable/vec thereof; a LOCAL struct (origin
    unknown) stays per-importer. KEY INSIGHT (vs the prior handoff's fear): the inline-local model erases struct
    origin in the StructId SPACE, but the driver still KNOWS the (name, origin) at the `use` site — so threading
    a name→StructId→origin map is enough; no de-inlining needed. Two load-bearing tests: `id<geo::Point>` across
    2 importers → ONE `id__geo$Point` (nm); + `separate_same_named_cross_module_structs_dont_alias_in_dedup`
    (a::Point{x} 8-byte vs b::Point{x,y} 16-byte → DISTINCT `id__a$Point`/`id__b$Point` → no wrong merge → exit
    12, the exact unsoundness point 8 closes; the `nm` assertion is load-bearing — those tags exist only with
    the fix). Golden test `abi_v1_mangling_dedup_qualifies_cross_module_struct_tag` + abi-v1.md §4 pin the
    format.
    ✅ **+ ENUMS too** (`7d5dd46`): the point-8 fix extends to cross-module enums (`id<shape::Shape>` →
    `id__shape$Shape`) — `mangle_type_dedup` gained an Enum arm + the two origin maps were BUNDLED into a pub
    `NamedTypeOrigins { structs, enums }` (so the codegen sig stays 6 args + a future named-type map is one
    field, not one param). Driver records imported enum (name, origin) at the `use` site → name→resolved
    EnumId. Golden test now covers both; `separate_cross_module_generic_over_enum_is_linkonce_deduped` (nm=1,
    load-bearing) → 42. So `linkonce_odr` dedup covers primitives + cross-module STRUCTS + ENUMS.
    ✅ **(3/N) INCREMENTAL CACHING WORKS — THE PAYOFF** (`299f17c` per-unit `.o` reproducibility foundation +
    `ced9130` the cache). FOUNDATION: `repro.rs` now builds a multi-file project twice in independent processes
    and asserts every per-unit `.o` is BYTE-IDENTICAL (a cached `.o` may only be reused if rebuilding reproduces
    it; the dedup origin maps are lookup-only, so deterministic). THE CACHE: `DiscoveredModule` retains its
    `source`; `unit_fingerprint(unit)` = a `DefaultHasher` (FIXED keys → process-stable, unlike a HashMap seed)
    hash over the unit's source + EVERY imported module's source (the importer inlines their decls / extern-links
    their fns) + the graph-wide sorted effect names (op-id base map) + module path + compiler version, stamped
    into an `<obj>.o.fp` sidecar. On rebuild a unit with a matching fingerprint + an on-disk object is REUSED —
    the WHOLE per-unit pipeline (resolve→check→effect/borrow/CT→codegen) is skipped, printing `fresh <module>`.
    So editing one module recompiles only it + its importers. Tests: no-op rebuild → all `fresh` + runs (42);
    edit one of two siblings → the other stays `fresh`, the edited module + `main` (imports it) recompile (the
    `!fresh main` assertion is the INVALIDATION-SOUNDNESS guard — a fingerprint omitting imports' sources would
    leave `main` STALE), edit takes effect (40+2→42); NOT Salsa-backed (`--separate` bypasses Salsa like merge,
    a content hash is the analogue); the cache lives beside the objects in the `-o` dir.
    ✅ **ITEM-GRANULAR fingerprint** (`63d93cc`): the fingerprint now hashes the imported ITEMS a unit uses
    (the matching `ExportedItem`s — `ExportedFn`/`ExportedItem` derive `Hash`; the AST decls already did), NOT
    whole module sources. PRECISION: an `ExportedItem::Fn` is SIGNATURE-only, so editing a non-generic imported
    fn's BODY no longer recompiles its importers (they extern-call it → relink picks up the new body); an
    inlined item (struct/enum/generic body/trait/effect) carries its full decl, so a change there does recompile
    importers. Tests cover both directions (body change → importer `fresh`; struct field reorder → importer
    recompiles). ⚠ remaining coarseness: a graph-wide effect change still recompiles every unit (the op-id base
    map is global). ▶ **NEXT (remaining tail, all LOWER value):** extend the type-tag fix to class /
    generic-instance type args (add their arms to `mangle_type_dedup` + a `NamedTypeOrigins` field — same shape,
    rarer) + dedup cross-module trait/class METHODS (currently importer-qualified; mirror the generic-fn
    `generic_origins` model with a method-origin map). Keep the merge path + both fixed points green (additive).
    Settled decisions: ADR 0037 **SETTLED DESIGN POINTS** +
    D5.1/D5.2/D7/D9; the 2/N cross-module-type-tag soundness fix is flagged there. CODE MAP: mangling site
    `crates/sentinel-codegen/src/lib.rs:385` (`add_function(&signature.name,…)`); the `fns` map `:327`;
    `collect_mono_instantiations` `:1738`; `mangle_mono_name`/`mangle_type` `:2049`/`:2067`; golden test
    `abi_v1_mangling_is_stable` `:7736`; driver `discover_module_graph`/`run_build`/`run_build_merged` =
    `crates/sentinel-driver/src/main.rs:663`/`:724`/`:837`; `merge_modules`/`resolve_imports` =
    `crates/sentinel-resolve/src/lib.rs:1499`/`:1416`; the FnId builtins `:56–135`.**
    **▶ OTHER OPEN TRACKS (owner's call — see [[sentinel_review_action_plan]] +
    `docs/REVIEW_ACTION_PLAN.md`, the owner's UNTRACKED plan):** (1) **P2.4** — a Linux `ubuntu-24.04` CI
    job (observe-only / continue-on-error; glibc surfaces heap bugs macOS masks; before abi-v1 ossifies;
    NOTE: configurable but only verifiable on GitHub, no Linux locally); (2) **rest of P3 — docs/DX**:
    P3.2 split STATE.md, P3.3 DESIGN vs DESIGN2 disposition, P3.4 a stale-reference sweep, P3.5 a minimal
    LSP (wire the salsa pipeline → publishDiagnostics — error-as-you-type, 3 reviewers flagged the stub),
    P3.6 a diagnostics-quality pass; (3) **P4** (external CT review of the now-calibrated claim +
    `docs/ct-model.md` + conformance suite — the artifacts were built to feed it; perf profile; deferred
    Polonius ergonomics); (4) a small follow-on: a FAITHFUL per-function parser depth limit (the `99d5ee1`
    guard covers bracket nesting only — operator-ambiguous unary/generic nesting is a residual). Detail:
    [[sentinel_review_action_plan]] (the full P0–P4 status) + [[sentinel_partial_move_fix]] +
    `docs/ct-model.md` + ADR 0037.

    --- BELOW: the Bar B historical record (the completed full-corpus codegen-parity work) ---
    **Bar B headline** (DONE): the Sentinel compiler fully compiles itself, via BOTH fixed-point paths —
    (b) 8g merge-to-source A18 + (a) the self-hosted merge A19–A20; `scg` discovers+merges+emits itself,
    `cc`→`scg'` self-reproduces. **METHOD** (the established per-slice
    lock-step): for each construct the corpus uses but the selfhost compiler doesn't, extend the oracle
    (`crates/sentinel-driver/src/llvm_dump.rs`) + the Sentinel mode-4 cg (`selfhost/types.sentinel`),
    **mirroring the inkwell backend** (`crates/sentinel-codegen/src/lib.rs`) for the layout/lowering;
    validate = the codegen differential (`sentinel_codegen_matches_oracle_on_corpus`, byte + behavioural) +
    modes 0–3 byte-identical + the `leaks` sweep + four-check; feat per construct, docs batched. To find the
    current emitting set + remaining blockers, run `snc llvm` over `tests/pass`+`tests/ui` and bucket the
    Errs (the categorize loop is trivial). **✅ DONE: `print` (FnId 0)** — emitting set **57 → 73** (feat
    `391ae58`). **✅ DONE: nullable `?T`** (A22, feat `8150ccc`) — emitting set **73 → 80** (the 6 `c15_*`
    fixtures + `c16_linked_list_node`, which reaches `?Struct` via `null`; c17 stays Err'd — needs mono).
    Layout `{ i1, T }` (`?primitive`) / `{ i1, ptr }` (`?Struct`, heap-indirect → recursive `struct Node {
    next: ?Node }` works) in `cgo_ty`/`ll_type_to`; `null` = operand kind 3 (`{ i1 0, <zero> }`); widen =
    `cg_widen` (mir_widen's cg twin, at every widen site) → `{ i1 1, T }` insertvalue; `x == null` =
    `cg_cmp_null` (extract+icmp valid bits); `is_some`/`unwrap_or` in `cg_emit_call` (FnId 2/1). ⚠ `?Struct`
    heap-box WidenToNullable + nullable drop DEFERRED (unexercised — corpus only widens primitives + `null`s
    `?Struct`); `cg_extract` now returns its dest reg. Byte-identical + behavioural + leak-free, both
    fixed-point paths preserved, 1476 tests, four-check green. **✅ DONE: secret + declassify** (A23, feat
    `7b471a9`) — emitting set **80 → 85** (`c31_secret_typing`, `c31_go_no_go`, `c52_secret_ct`, `c53_ct_eq`
    + the `c52_secret_leak` ui). STRIP-TO-INNER: `secret T` lowers identically to its inner. Oracle = a new
    `Emit::lty(&self, ty)` strips a top-level `Type::Secret` via `program.secrets` then calls `llvm_ty`
    (every Emit `llvm_ty(` → `self.lty(`; 4 non-Emit sites strip inline); Sentinel = `cgo_ty`/`ll_type_to`
    strip kind-3 at their ENTRY (a stripped `secret bool` → `i1` must route to the scalar arm). declassify /
    widen-secret are identity. **✅ DONE: generics (a) — generic STRUCT instances** (A24, feat `d3be39b`) —
    emitting set **85 → 87** (c17_box, c25). `Decl<args>` → a STRUCTURALLY-named `%Box_i64` aggregate (NOT
    interner-id — the two type-checkers may intern in different orders) + substituted-field layout/drop.
    `llvm_ty` now threads `&program` (unified the secret strip + the GenericInstance arm) +
    `mangle_type`/`mangle_instance`; Pass 0 emits concrete instance layouts; needs_drop/emit_drop substitute
    fields. Sentinel `cgo_ty`/`cg_pass0`/`cg_mangle_to`/`cg_has_typeparam`/`cg_struct_is_generic` mirror it
    (⚠ BIND `(*c).ta[idx]` to a local before a recursive `&mut c` call — the nested-`&mut`-ctx quirk). ⚠⚠ THE
    UN-PARSER (`merge.sentinel`) had to preserve generic `<T>`: `merge_source` ALWAYS re-emits (no raw
    passthrough), and `emit_struct_decl` did `skip_type_params` (dropping `<T>`) → fixed with a new
    `emit_type_params` (non-generic decls byte-unchanged → fixed point holds). **✅ DONE: generics (b) —
    generic fns / MONO** (A25, feat `170a13a`) — emitting set **87 → 92** (the 5 c17 generic-fn fixtures incl.
    `pick__i64`+`pick__bool`); GENERICS COMPLETE. A generic fn emits ONCE PER instance under `id__i64`. Oracle:
    reuses inkwell's `collect_mono_instantiations` (now `pub`) + substitutes each def (`TypedFnDef::substitute`)
    → `dump_fn_named` under `mangle_mono_name`; `lower_call` threads `type_args` → mangled callee. Sentinel
    (the hard half): pass-2 SKIPS generic fns in mode 4 (records `fn`-token positions); `dump_generic_call`
    records the instance (`cg_mono_record`) + stashes `cg_targs`; a MONO PASS re-walks each instance's body via
    `type_fn` with `cg_mono_on`+`cg_mono_args` (so `type_of_typeexpr` resolves `T`→concrete + `cg_emit_fn`
    mangles); `cg_emit_call` mangles a generic callee. ⚠ `dump_args_capture_all` had to `cg_collect` (was
    mir-only). Un-parser `emit_fn_decl` preserves fn `<T>`. Both fixed-point paths preserved (the mono
    machinery is inert for the generics-free selfhost — `nmono`=0). **✅ DONE: classes / traits / impls /
    delegates** (A26, feat `a1a3341`) — emitting set **92 → 98** (c41/c42/c43/c4_named_impl). Pointer ABI
    (mirror inkwell): a class is `%Class.N` held by value; `init` = `void @Class__init(ptr out,…)`, a method
    `@Class__m(ptr self,…)`, an impl method `<prefix>__<Type>__<Trait>__<m>` (`default` or the impl name).
    `self` binds to `%arg0` (no alloca — `Var(self)` loads `%Class.N`, `self.f` GEPs `%arg0`); the 4 forms
    (ClassInit alloca-call-load / MethodCall + ImplMethodCall self-ptr / QualifiedCall args[0]=recv) compose
    through the existing FieldAccess + call paths. **Delegates need NO special cg** — the type layer already
    synthesised them into ordinary `impl as Tr for C { fn m … { self.f.m(args) } }` (GEP the inline field →
    the field type's `default` impl). ⚠⚠ BOTH un-parsers (`source_dump.rs` AST-driven + `merge.sentinel`
    token-driven) REJECTED class/trait/impl DECLs — both now emit them so a re-parse is structurally identical
    (same ClassId / field+method indices / delegate ImplIds, all fixed by intra-kind source order). Sentinel
    side: a **`cgcls`** buffer for class/impl defines appended AFTER the fns (the group-ordered walk lands them
    in the oracle's order); operand **kind 4** = `%arg0`; `cg_self_var`/`cg_arg_base`; `cg_emit_method` + the
    call/symbol helpers; `synth_forward` emits the delegate forwarding. The 6 fixtures `scg` == `snc llvm`
    byte-identical + behaviourally == inkwell (7/42/42/42/42/42) + leak-free; modes 0–3 byte-identical; both
    fixed-point paths preserved (selfhost declares no classes → `cgcls` empty). **✅ EFFECTS c35a LANDED**
    (A27, feat `29e3027`, emitting set 98 → 101) — the restricted handler case (a `handle` body that IS a
    direct `perform`). The kont ABI + the c35a `.ll`/cg are IN the code (`llvm_dump.rs` lower_handle/
    lower_resume_kont/Perform + `types.sentinel` dump_tharms/Handle/Perform/resume-kont + the un-parser effect
    decls) — the reference for the next sub-slices. The if-else-chain finding is REALIZED (both backends
    chain, not `switch`; the Sentinel `HArms` is single-consumption). **✅ EFFECTS c35b LANDED** (A28, feat
    `02891fd`, emitting set **101 → 107**) — the **effecting-fn `Kont*` ABI + pure-return**: a fn with a
    non-`Async` effect row returns `ptr`, so a `handle` body that is a CALL to an effecting fn dispatches on
    the returned kont; a pure tail wraps via `sentinel_kont_pure`. SIX fixtures flip: c35b_handle_{fn_call_
    body,multi_arm,pure_return} + c32/c33_go_no_go (effecting fns, no handle) + **c5_go_no_go** (the C5
    phase-go). Oracle: lift `dump_fn`'s gate + `uses_kont_abi`/`validate_effecting_fn_body`/`produces_kont`
    (defer let-bound/embedded/chained perform) + `lower_call`→ptr. Un-parser (`merge.sentinel`):
    `emit_fn_decl` re-emits the `! { E }` row (new `emit_effect_row` — the A24 `<T>` analog). Sentinel: a
    per-FnId `ufeff` table (pass-1 `eff_row_is_kont`/`cg_is_async`) + `cg_eff`/`cg_tailk` driving `cg_emit_fn`
    (ptr ABI + kont_pure wrap) / `cg_emit_call` (ptr) / `cg_emit_perform`. ⚠ **CORRECTION:** `merge_source`
    ALWAYS re-emits (no raw passthrough — `merge_mode`=`has_use` only gates the rename map), so the
    `emit_fn_decl` effect-row re-emit was REQUIRED (dropping it silently lost the row → plain i64 ABI).
    ⚠ scg is run ONLY when the oracle SUCCEEDS (the differential `continue`s first), so a deferred fixture's
    scg output is never compared → the Sentinel side needs NO `validate_effecting_fn_body`. **✅ EFFECTS c35c
    LANDED** (A29, feat `96c54b9`, emitting set **107 → 110**) — **let-bound perform + the captured frame**: an
    effecting fn whose body is `let v: i64 = perform Op(); <pure tail>` reifies a captured eval frame, emitting
    **TWO defines per source fn** (the first sub-slice to) + the first use of **`sentinel_kont_push`**. THREE
    fixtures flip: c35c_let_bound_perform + _with_capture + **c37_go_no_go** (the C3.7 phase-go — perform-with-
    arg `Io.log(x)`, captured `x`, `x + logged`, + `print` → stdout 85). The PARENT (Kont* ABI) allocs the
    captured struct (`i64[N]` / null), lowers the RHS perform → kont, `kont_push(kont, @__resume_<name>,
    captured)`, ret; the RESUMER `@__resume_<name>(i64 %arg0, ptr %arg1)` binds the let var to %arg0 + captures
    from %arg1, lowers the pure tail, `kont_pure`-wraps, ret. Runtime owns kont/frame/captured (leak-free, no
    cg drops); the two defines share NO register counter (each `%v0`-fresh — `%vN` are NAMED locals).
    Oracle: `detect_let_shape` routes `dump_fn_named`→`dump_let_shape_fn` before `validate_effecting_fn_body`;
    `collect_captured_vars`; a `kont_push` RuntimeSym. Un-parsers: **NO change** (source_dump round-trips it;
    merge.sentinel already emits let/perform/handle/effect-row — no new syntax). Sentinel: `cg_emit_fn_eff`
    (an effecting fn — `cg_eff`, mode-4 only) detects the let-shape **structurally** (a single `SLet` — every
    EMITTED performing-statement effecting fn IS a let-shape, the oracle defers the rest) → `cg_letshape_emit`
    (parent reuses the param setup; resumer `cg_reset`s + rebinds the let var/captures + walks the tail), else
    `cg_eff_normal` (the c35b straight-line path). Capture set = the param range `[cg_pv0,cg_pvn)` (= the
    oracle's first-ref order for the ≤1-param c35c corpus; multi-param first-ref is c35d+). ⚠ The Sentinel
    match grammar has NO bind-the-whole-value pattern (only `Enum::Variant`/`_`), so non-let-shape branches
    re-wrap the moved-out `Stmts`/`Stmt` parts (fully enumerated). New: `cg_used_kontpush`. **✅ EFFECTS c35d
    LANDED** (A30, feat `ecd150c`, emitting set **110 → 113**) — **embedded perform via placeholder
    substitution**: a statement-free tail mixing exactly ONE perform into pure context (`perform Op()+1`,
    `f(perform Op())`) reuses the c35c two-define frame — the PARENT lowers JUST the perform + `kont_push`es;
    the RESUMER re-evaluates the FULL tail with the perform substituted by the resumed value. THREE fixtures
    flip: c35d_binop_with_perform / _perform_in_call_arg / _perform_with_capture_and_binop (exit 42 each).
    Oracle: `detect_embedded_shape` (`collect_performs` walker, exactly-one + i64, before
    `validate_effecting_fn_body`) → `dump_embedded_shape_fn`; NO substituted tree — a new `Emit::embed_ph`
    slot makes the Perform arm emit a `load` (byte-equal to the substituted `Var`); the captured walk skips
    the Perform subtree (= inkwell's substituted-tail walk). Un-parsers: NO change. Sentinel: move semantics
    bar inspect-then-reuse, so `type_fn` re-parses a disposable CLASSIFICATION COPY from the same tokens
    (mode-4 effecting fns only) → `eff_classify`/`efp_*` extract the perform as a 1-element `Args` list →
    `cg_embed_emit` (the letshape mirror; ANONYMOUS `TyCtx.cg_ph` slot, no `bind_name`) + `cg_emit_phload`
    in the Perform arm. The param-range capture heuristic stays exact (≤1 captured var). **✅ EFFECTS c35e
    LANDED** (A31, feat `6bdd23b`, emitting set **113 → 116**) — **chained effecting lets**: a body of 2+
    `let v: i64 = perform …` + a pure tail emits **N+1 defines** (parent + N resumers); each chaining
    resumer-i performs let-(i+1) + pushes resumer-(i+1) (the runtime BUBBLES the fresh kont so the handle
    re-dispatches — the c35c-wired bubble path), the last wraps the tail. THREE fixtures flip:
    c35e_chained_perform / _chained_dependent_perform (the 2nd perform's arg = the 1st let, forward capture
    flow) / _chained_perform_with_capture (an outer param carried through both resumers). Oracle:
    `detect_chained_lets_shape` → `dump_chained_lets_fn`; `compute_chained_captures(i)` = vars in
    (lets[i+1..].RHS + tail) minus lets[i..] (⚠ a chained RHS's perform args ARE captured —
    `walk_collect_rhs_var_refs` — the emitting resumer lowers them, unlike c35d). Sentinel: the 2+-stmt branch
    of `cg_emit_fn_eff` → **`cg_chained_emit`** (3 phases — re-parse-bind the let vids / consume the ORIGINAL
    body for the N+1 lowerings / on-demand capture sets from FRESH re-parses via `cg_chained_caps` + the new
    `cg_walk_ex` name-collector-cum-disposal-walk); `cg_chained_parent` reuses emit_tparams, `cg_chained_
    resumer` `cg_reset`s per define mirroring the oracle's alloca/fresh interleaving. ⚠⚠ THE SELF-COMPILE
    SURFACED A NEW SCG QUIRK (not the fixtures — the fixed-point capstone): an inline DISCARDED `match` (`match
    x { … };` statement) defaults its result type to **`ptr`** in scg vs the oracle's **`i64`**, diverging the
    self-compiled alloca; fixed by moving the navigate-collect into the **`cg_caps_collect`** helper (the
    `match` in TAIL position → the fn's i64 return directs inference). A discarded `if` does NOT hit this
    (infers from the then-branch). **REUSABLE RULE: a discarded `match` statement needs an i64-directing
    context (tail position / annotated binding) or scg allocas its result slot as `ptr`.** First Bar-B slice
    where the new SENTINEL SOURCE (not just its emitted output) had to self-compile identically.
    where the new SENTINEL SOURCE (not just its emitted output) had to self-compile identically. **✅ EFFECTS
    c36a LANDED** (A32, feat `caf4175`, emitting set **116 → 119**) — **handle `return` arm + pure-body wrap**:
    a non-identity `return v => body` transforms the pure value at each pure-drain site (the dispatch pure
    block AND each k(v) pure path — Phase B's re-wrap); a PURE body (`handle 42`) wraps via `kont_pure`. THREE
    fixtures flip: c36a_return_arm_transform / _return_arm_after_resume (the k(v)-path case — REQUIRED) /
    c37_handle_return (42→84). Oracle: `lower_handle` drops the return-arm Err + defers nested-Handle bodies +
    wraps pure; `apply_return_arm` inlines at both sites (arm carried in `handle_stack`). Sentinel (THE HARD
    HALF — multi-site lowering vs no-clone): **RE-PARSE** — `Ret::YesRet` gains the var+body token indices, the
    tokens are copied to `TyCtx` ONLY when mode 4 + a `return` token (so the fixed point pays nothing),
    `cg_apply_return_arm` re-parses via `parse_expr` at each site (the borrow `parse_expr(&(*c).cgtk, …)`
    de-risked in isolation first). The Handle arm reconstructs `hr` to set `cg_ret_*` before `dump_tharms`;
    `dump_tret` disposes the body in mode 4 / type-dumps in 0-3; pure-body via `cg_tailk`. ⚠ The `YesRet` AST
    change rippled to 4 stages (parser/resolve/effects/merge — bind+ignore, dumps byte-unchanged); `parse_expr`
    + `slice_of` made `pub`. Both fixed-point capstones pass (the AST change + new code self-compile). **✅
    EFFECTS c36b LANDED** (A33, feat `b63cc98`, emitting set **119 → 121**) — **nested handles; THE LAST
    HANDLER SLICE — ALL EFFECTS DONE.** A `handle` whose body is a handle: the inner (NESTED, depth>1) handle
    lowers to a **Kont\*-typed result** — arms wrap their i64 via `kont_pure`, the PURE_RETURN case passes the
    kont through (or re-wraps the return-arm'd value), and the switch DEFAULT **propagates** the un-caught kont
    to the merge so the OUTER handle dispatches it. TWO fixtures flip: c36b_nested_handle_basic (inner=Io,
    outer=Net; the Net kont propagates out) / _inner_full (inner fully discharges Io; outer gets PURE_RETURN).
    Oracle: `lower_handle` gains a `handle_depth` counter (is_nested=depth>1; split into `lower_handle` +
    `lower_handle_inner`) + a `ptr` result cell when nested + `store_handle_result` (wrap i64→ptr) + the
    propagate default + a nested-Handle body treated as kont-producing. Sentinel: a `cg_h_depth` counter;
    `is_nested` threads to `dump_tharms` (the arm store → `cg_store_hresult`) + the dispatch tail (ptr rslot,
    passthrough/wrap pure block, propagate default, ptr merge load); a nested handle sets `cg_tailk = is_nested`
    so the enclosing handle's body-kont detection sees the inner produced a Kont\*. **✅ STRUCTURED
    CONCURRENCY LANDED** (A34, feat `0f360cf`, emitting set **121 → 123**) — **FULL-CORPUS PARITY; BAR B
    COMPLETE; ADR 0045 ACCEPTED.** `scope concurrent { … }` / `spawn fn(args)` / `expr.await` lower to the ADR
    0024 runtime: scope → `sentinel_scope_enter`/`_exit`; spawn → pack args (heap buffer) + `sentinel_task_spawn`
    with a per-target `__spawn_wrapper_<id>` + `sentinel_scope_register`; await → `sentinel_task_await`; a
    `Task<T>` is an opaque `ptr`. TWO fixtures flip: c44_go_no_go (scope+spawn+await) + c4_go_no_go (the full C4
    surface). **ALL 123 PASS FIXTURES EMIT** (the 20 ui/ negatives Err by design). Oracle: 5 RuntimeSyms +
    `Emit::current_scope` + `collect_spawn_targets_*`/`dump_spawn_wrapper` + `Type::Task → ptr`; `lower_expr` is
    now EXHAUSTIVE (the catch-all deleted). Sentinel: the Scope/Spawn/Await arms gain cg (Spawn branches on
    `cg_on` — collects args via `dump_targs` + `cg_emit_spawn`, NOT a normal call), `cg_scope`/`cg_spawn_t` +
    `cg_emit_spawn_wrapper` (into `cgcls`, after the class/impl methods — the oracle's order), `cg_is_task →
    ptr`. ⚠ Un-parsers UNCHANGED: concurrency is out of `source_dump`'s Bar-A (selfhost) scope, like declassify
    (the selfhost compiler has no concurrency → the fixed-point un-parse is unaffected; the codegen differential
    parses fixtures directly). ⚠ The Sentinel collects-then-stores spawn args (vs the oracle's inline
    lower-then-store) — byte-identical for IMMEDIATE args (the corpus); a non-immediate spawn arg is a
    documented refinement.
    Sub-phase like the production C3.5(a)–(e)+C3.6 — done: **c35a** inline + **c35b** effecting-fn ABI +
    **c35c** let-bound perform + **c35d** embedded perform + **c35e** chained lets + **c36a** return arm +
    **c36b** nested handle + **structured concurrency**. ALL HANDLER + CONCURRENCY SLICES COMPLETE.
    **▶ BAR B IS DONE — full-corpus codegen parity reached; ADR 0045 ACCEPTED-WITH-AMENDMENTS (A1–A34). The
    self-host port's goal is MET.** All kont symbols
    (`perform_op`/`kont_resume`/`kont_consume_pure`/`kont_pure`/`kont_push`) + the `sentinel_task_*`/
    `sentinel_scope_*` set are USED. **Remaining deferred track: the per-unit separate-compilation back end
    (ADR 0037 (a))** — independent of the port (the headline self-host is complete). The path-(a) build
    record + design follows (COMPLETE):
    **✅ (a-1)+(a-1b) DONE (ADR 0045 A19):**
    `selfhost/merge.sentinel` — a Sentinel un-parser (port of `source_dump.rs`) re-emitting a parsed
    program as re-parseable source (fns + structs + enums + the full expr/stmt/type grammar);
    round-trips the real single-module stages BYTE-IDENTICAL (`snc llvm` unparsed == orig: `lexer`
    4,390 / `parser` 21,618 `.ll` lines), leak-free, guarded by
    `sentinel_merge_unparser_round_trips_single_module_stages`; 1474 tests, four-check green. **Spine
    PROVEN, NO blocker** (an earlier "heap crash" was a FALSE ALARM — exit 139 was a probe's correct return
    value 53+48+19+19, misread as SIGSEGV; lldb confirmed a clean exit). **DESIGN (settled):** each
    module's rename map is SELF-CONTAINED
    (`merge_modules` builds it from that module's own top-level names [qualified by its `$`-prefix] + its
    `use a::b::Item` → `a$b$Item`, never cross-referencing other modules) → `scg` processes ONE MODULE AT A
    TIME (re-parse per module; NO `Vec<Program>`/HashMap needed) and the rewrite FUSES into a Sentinel
    un-parser (port the PROVEN Rust `crates/sentinel-driver/src/source_dump.rs`: look up the current
    module's rename map at each name-emit site — Call callee / StructLit-ClassInit-Qcall names / type
    Idents / Pattern enum / Handle-Perform effects; NEVER `Var`/locals/field/method/variant/op names —
    mirror the Rust `rewrite_expr`/`rewrite_type_expr`). BFS discover+emit can fuse (one parse/module, BFS
    order = merge order). **INTEGRATION:** `codegen.sentinel main` does `merged = discover_and_merge(entry)`
    then `types::run(merged, 4, result)` (the existing pipeline, unchanged); `types::run` is TOKEN-DRIVEN
    (tokenize → walk), so the un-parser mirrors that. **SLICES:** ✅ (a-1)+(a-1b) DONE — the single-module
    un-parser (`selfhost/merge.sentinel`; round-trips `lexer`/`parser` byte-identical, leak-free);
    **▶ (a-2) NEXT** — per-module rename map (flat parallel pools, the resolve/types idiom) + the rewrite
    FUSED into `emit_expr` (look up the rename map at name-emit sites — Call callee / StructLit-ClassInit-
    Qcall names / type Idents / Pattern enum / Handle-Perform effects; NEVER `Var`/locals/field/method/
    variant/op names — mirror the Rust `rewrite_expr`); (a-3) BFS discovery (read entry, follow `use` edges,
    build `<root>/a/b.sentinel` paths, read each); (a-4) wire `discover_and_merge` into `codegen.sentinel`'s
    `main` (then `types::run(merged, 4, …)`) + the capstone: `scg` discovers+merges+emits == the `snc llvm`
    oracle, `cc`→`scg'` self-reproduces. ⚠ The leak GATE is the **`leaks --atExit` sweep** (codesign:
    entitlements plist w/ get-task-allow → `codesign -s - -f --entitlements ent.plist ./bin` →
    `leaks --atExit -- ./bin`), NOT the differential. ⚠⚠ **The behavioural test is ~84s** — SAMPLE on
    changed fixtures, full suite ONCE as the final gate. (Path (a)'s spec was the Rust `snc merge` +
    `source_dump.rs` + `merge_modules`/`Renamer`/`rewrite_*` (sentinel-resolve/src/lib.rs:1416-1949) +
    `discover_module_graph` (main.rs:607) — all now ported to `selfhost/merge.sentinel`.) The codegen +
    self-host history that built this is below.

    NEXT = **(8/N) CODEGEN — the GRAND FINALE + the bootstrap fixed-point — is OPENED; ADR
    0045 PROPOSED** (its own kickoff, the 0039–0044 cadence), the 3 design calls SETTLED WITH
    THE OWNER (as 5/N–7/N were): (1) emission target = **textual LLVM `.ll`** (write_file →
    external `clang`/`llc` → object → link `libsentinel_runtime.a`) — probe-validated; (2)
    oracle = a NEW Rust **`snc llvm` (`llvm_dump.rs`) canonical-`.ll` byte-parity differential**
    (the port's method — a canonical spec WE define, NOT inkwell `print_to_string`) + behavioural
    clang-run-parity + the fixed-point capstone; (3) scope = **fixed-point-first** (Bar A = the
    non-exotic core the selfhost sources use → the fixed-point; Bar B = effects/handlers/
    concurrency/classes/generics/nullable → full-corpus parity, after). 🔑 SCOUT+PROBE findings
    that shrink the 8263-line finale: **NO phi** — codegen is alloca/load-store at `-O0`, merges
    via memory cells, so the `.ll` needs no SSA merge (simpler than 7/N's MIR; a hand-written
    `.ll` in this style clang/llc-18 compiles + runs exit-correct at `-O0`, probe-validated);
    **secret codegen is a no-op** (strip-to-inner, codegen lib.rs:1594; the CT guarantee = the
    source rejections + the 7/N D5 verifier); **the bootstrap subset is small** (the selfhost
    sources declare 424 fns / 3 structs / 14 enums + 0 traits/impls/classes/effects/generics +
    no nullable → Bar A ≪ 8263 lines; the ~1500-line handler/kont + ~765-line shape-detection +
    concurrency/class machinery is Bar B). Reuse = the 6/N `types::run`-with-`mode` template, a
    new **`mode 4`** (emit-`.ll`); the 3-pass structure (type-decls / fn+runtime-symbol-decls /
    body-emission) reproduced; `compile_to_object` reads TypedProgram + DropPlan (codegen
    lib.rs:168); link = `cc obj libsentinel_runtime.a -o exe`. ⚠ **(8a) is PROBE-GATED:** settle
    fused `mode 4` vs a hybrid (the driving walk a `mode 4` in `types.sentinel`, the `.ll`-emit
    helpers in a `codegen.sentinel` module — keeps the monolith bounded), and re-verify modes
    0–3 (`snc types`/`borrow`/`mir`/`ctverify`) **byte-identical BEFORE bulk emission** (the
    0044 D3 gate, widened to FOUR accepted stages). ⚠ Precision: temporary/block numbering +
    GEP indices + `abi-v1` mangled names must match the Rust `llvm_dump` exactly (byte-for-byte
    at object scale). **✅ (8a-i) the `snc llvm` ORACLE LANDED** (`1931496`, ADR 0045 A1):
    `run_llvm` + `llvm_dump.rs` emit the canonical `.ll` (partial-by-Err); `tests/llvm.rs` =
    goldens + a 0-panics sweep (16 emit / 125 Err over 141) + **16/16 behavioural parity** (each
    emitted `.ll` via `cc` == inkwell `snc build`). AS-BUILT spec: NO phi (alloca/load-store,
    `%vN` counter, `%argN` params), `main`→i32-trunc, FnId order. **D4 reuse SETTLED = fused
    `mode 4`** (mirrors MIR `mode 2`: a `cgout` buffer + an operand-threading field like
    `lastval` + a VarId→slot append-only pool like `mvdv` + a value counter; `type_fn` emits the
    define header/footer; hybrid-with-a-separate-module deferred). **✅ (8a-ii)
    `selfhost/codegen.sentinel` LANDED (`2ed426a`, ADR 0045 A2) — (8a) COMPLETE:** the 8th +
    final Sentinel stage emits `.ll` straight-line, matching `snc llvm` byte-for-byte
    (`sentinel_codegen_matches_oracle_on_corpus`, 16/16 emitted), leak-free, modes 0–3
    byte-identical (all 8 corpus differentials green). The fused `mode 4` mirrors MIR `mode 2`
    1:1 (cgout + cglk/cglv≈lastval + cgsv/cgsr slot-pool≈var_defs + value counter; `mir_on`→2/3,
    `cg_on`=4). 🔑 KEY: the "no `&mut (*c).field` to a USER fn" rule is sidestepped by
    direct-to-`cgout` helpers using the BUILTIN `push` + consuming `[u8]` args by value (simpler
    than MIR's render-to-local-then-fold; NO phi — alloca/load/store). **✅ (8b) CONTROL FLOW
    COMPLETE (ADR 0045 A3; `c76db27` 8b-1 + `7b33d49` 8b-2):** if/else + while/break/continue +
    `&&`/`||`, byte-identical to `snc llvm` (26/26 emitted) + behavioural (cc==inkwell) +
    leak-free; modes 0–3 byte-identical. 🔑 THE ALLOCA HOIST (8b-1): every alloca is hoisted to
    the entry block (solves the if-result's late-known type — the parser AST has no precomputed
    types — AND ADR 0036 loop-stack growth); codegen.sentinel buffers the body in `cgbody`,
    records allocas as (slot,type) pairs, + a `cg_putc` router (cg_to_body → cgbody walk / cgout
    teardown). if/else + `&&`/`||` = no-phi memory-cell merges; while = the real loop CFG (a
    loop-target stack + a dead block after break/continue). **✅ (8c-1) STRUCTS COMPLETE (ADR
    0045 A4):** struct type decls + literals + field reads, byte-identical to `snc llvm` (32/32
    emitted) + behavioural (cc==inkwell) + leak-free; modes 0–3 + effects byte-identical. A struct
    is a first-class SSA VALUE — `insertvalue`/`extractvalue` over an aggregate `%vN` (NOT
    alloca/GEP), so let/var/param/return/call carry it via the EXISTING alloca/store/load once
    `cgo_ty` learns structs (kind-6 → `%Struct.N` via `struct_of_handle`); **Pass 0** (`cg_pass0`
    in the mode-4 preamble + `ll_type_to`) emits the `%Struct.N = type {…}` decls; struct-lit
    reuses the call-arg collect stacks (`cg_collecting`/`cgak`/`cgav`/`cgat` + `cg_emit_structlit`,
    the oracle switched interleave→collect to match); field read = `cg_extract` (`extractvalue`).
    Generic structs (8h/Bar B) + field-assign (`p.x=…`, non-Var-lvalue) deferred. **✅ (8c-2)
    ARRAYS COMPLETE (ADR 0045 A5):** array literals + indexing + `len`, byte-identical to `snc
    llvm` (42/42 emitted) + behavioural + leak-free; modes 0–3 + effects byte-identical. `[T]` =
    the abi-v1 `{ i64, ptr }` (ONE inline literal for every element type → NO Pass-0 name); a
    literal heap-allocs `n*sizeof(elem)` (GEP-sizeof idiom + `sentinel_alloc`) + GEP-stores +
    `insertvalue {len,ptr}` (`cg_emit_arraylit`); `a[i]` = extract len(0)/data(1) + bounds-check
    (`sge`/`slt`/`and`, br ok/oob, OOB=`sentinel_panic_oob`+`unreachable`) + GEP+load
    (`cg_emit_index`); `len`=`extractvalue 0`. First **runtime-symbol declares**, emitted only for
    symbols used (per-symbol `used_alloc`/`used_panic` → 8a–8c-1 byte-identical). ⚠ `len` (FnId 3,
    generic builtin) goes via `dump_gcall`/`dump_args_capture_first`, which DIDN'T `cg_collect` the
    first arg → SIGABRT; fixed (collect first arg in `dump_args_capture_first` + `dump_array_elems`).
    **✅ (8c-3) `[u8]`/STRING LITERALS COMPLETE (ADR 0045 A6) — slice (8c) aggregates DONE:** string
    literals + the char-literal cg operand, byte-identical to `snc llvm` (43/43 emitted —
    `c5d5_break_continue` joins, `len("tok")=3` drives the exit) + 2 seeds + 1 golden, behavioural +
    leak-free; modes 0–3 + effects byte-identical. A string IS a `[u8]` (ADR 0033) — decoded bytes
    heap-copied (`sentinel_alloc` + N `i8` stores) into `{ i64, ptr }`, EXACTLY a u8 array literal of
    byte constants → reuses the array machinery (oracle factored `emit_array_buffer` shared by
    ArrayLit+StringLit; Sentinel `Str` arm pushes bytes as `i8` operands + reuses `cg_emit_arraylit`,
    read before `sink_name`). Closed a latent gap: `Char` arm now sets the cg operand (u8 constant
    like `Int`). **✅ (8d, runtime builtins) COMPLETE (ADR 0045 A7) — byte-array builtins, FIRST
    within (8d):** `str_eq`/`print_bytes`/`read_file`/`write_file`, byte-identical to `snc llvm`
    (45/45 emitted — `c5d2_strings` (str_eq) + `c5d4_file_io` (read/write/print — REAL file I/O) join)
    + 2 seeds + 1 golden, behavioural + leak-free; modes 0–3 + effects byte-identical. Each
    `extractvalue`s its `[u8]` into len(0)+ptr(1) and calls `sentinel_*` as `(ptr, i64, …)`;
    `read_file` uses a hoisted out-len slot. Refactor: per-symbol declare bools → a **`RuntimeSyms`**
    struct (merge + emit_declares, fixed order); Sentinel `cg_used_*` + a `cg_lenptr` helper.
    **✅ (8d-refs) REFERENCES COMPLETE (ADR 0045 A8) — the Vec prerequisite:** `&`/`&mut`/`*`/`*p=x`,
    byte-identical to `snc llvm` (53/53 emitted — the 8 C2 ref fixtures light up) + 2 seeds + 1 golden
    + behavioural + leak-free; modes 0–3 + effects byte-identical. A ref is an opaque `ptr` (pointee
    from `program.refs` at the deref); `&v` = v's alloca slot (NO instruction); `*r` = load-through;
    `*r = x` = store-through (target ptr emitted FIRST). Sentinel REUSES `cg_suppress` (slot for &v,
    pointer for *r; `&*r` keeps the deref-place's pointer). **✅ (8d-Vec-1) Vec IN-PLACE OPS COMPLETE
    (ADR 0045 A9):** `vec_new`/`push`/`pop`/`len`/`v[i]`, byte-identical to `snc llvm` (54/54 emitted
    — `c5d5_loops` joins) + 1 seed + 1 golden + behavioural + leak-free; modes 0–3 + effects
    byte-identical. A `Vec<T>` is `{ i64 len, i64 cap, ptr data }` (data=FIELD 2 vs `[T]`'s field 1).
    `vec_new` = the constant `{0,0,null}` (new `cgo_operand` kind 2); `push(&mut v,x)` = the `len==cap`
    `sentinel_realloc` GROW CFG (select + GEP-sizeof) via the `&mut Vec` field GEPs, no phi, returns
    i64 0; `pop` = empty-check + decrement; `len`/`v[i]` use the arg's actual aggregate type
    (`cg_emit_index` gained it, data field via `cg_is_vec`). The grow-CFG matched the differential
    first try. **NEXT = (8d-Vec-2)** `vec_to_array` (extract len/data + `sentinel_alloc` + `llvm.memcpy`
    + build `[T]` → `c5d3_collections` emits) + **heap drops** (DropPlan `sentinel_free` —
    byte-parity-NEUTRAL for behaviour, needed for a clean fixed-point) → (8e) enums/match → (8f)
    calls/multi-module → **(8g) the bootstrap fixed-point**. ⚠⚠ behavioural test ~83s at 54 —
    SAMPLE/CACHE it now. Sub-slices 8a–8l in ADR 0045 D10 (Bar A 8a–8g → the FIXED-POINT (8g);
    Bar B 8h–8l → full corpus → ADR 0045 ACCEPTED). No in-flight slice.

    KEY REUSABLE FINDINGS (carry forward):
    - The back-half stages REUSE the typed program (they can't cheaply re-derive it).
      The template (6/N): `types.sentinel` exposes **`pub fn run(src, mode, result)`**
      (mode 0 = the `snc types` dump; mode 1 = `snc borrow` moved-sources); a new
      reusing stage is THIN (`use types::run;` via a D.6 chain stage→types→parser, OR
      add a new `mode`). HIR/MIR/codegen build on this.
    - The differential-oracle method: each Rust stage gets a `snc <stage>` dump
      subcommand; the Sentinel stage reproduces it byte-for-byte; a corpus differential
      test asserts equality (skipping fixtures the oracle rejects — error/diagnostic
      parity is OUT OF SCOPE every stage). Mirror `tests/selfhost_borrow.rs`.
    - Sentinel-language idioms/quirks (bite every stage): recursive ASTs = enums
      returned BY VALUE + consuming `match` (Vec<non-primitive> UNSUPPORTED → cons-list
      enums); flat parallel-`Vec<i64>` tables + a name blob (integer-indexed); an owned
      `[u8]` must be CONSUMED (sink it) or it leaks; refs index via `(*r)[i]`; FLAT
      per-fn var namespace (but MATCH arms are independent scopes — names recur across
      arms; only nested if-branches within one arm need unique names); `if` is an
      EXPRESSION (tail + mandatory else); Vec index-assign + loop-reassign-of-a-Move-
      binding are FORBIDDEN (use recursion / append-only / rebuild); NO `<<`/`>>`/`~`
      (use a multiply loop for `2^n`, `^ (0-1)` for bit-not); `vec_to_array` is
      Vec<u8>-only (Vec<i64> auto-drops via RAII).

    BUILD/RUN/LEAK: stage selfhost/*.sentinel for the target stage in /tmp/tb/ (the
    entry + its `use` deps), `target/debug/snc build /tmp/tb/<stage>.sentinel -o
    /tmp/tb/bin`, run reading ./input.sentinel (cp <fixture> /tmp/tb/input.sentinel; (cd
    /tmp/tb && ./bin)), diff vs `target/debug/snc <stage> <fixture>`. Sweep tests/pass +
    tests/ui, skip where the oracle exits nonzero. Leak-check (codesign trick for fresh
    binaries): an entitlements plist with com.apple.security.get-task-allow=true,
    `codesign -s - --entitlements ent.plist -f /tmp/tb/bin`, then `(cd /tmp/tb && leaks
    --atExit -- ./bin)` — want 0 leaks. ALWAYS full-corpus-diff for zero-regressions +
    full-leak-sweep every match.

    READ FIRST: docs/STATE.md top banner + docs/HANDOVER.md §0.1 (working norms) + §0.3
    (quick-status — the (8/N) NEXT block at the top) + **docs/decisions/0045-self-host-
    port-codegen.md (the (8/N) kickoff — the build plan: D2 emission target, D3 oracle, D4
    reuse-probe, D5 data model, D6 the 3 passes, D7 the two bars, D8 the fixed-point, D10
    sub-slices)** + docs/decisions/0044-self-host-port-mir.md (the just-closed stage + the
    reuse template) + docs/decisions/0026-hir-mir-pipeline-and-constant-time-secret-
    codegen.md (the codegen + CT-secret-codegen DESIGN being ported — the D3/D4 escape hatch
    = why secret codegen is a no-op) + docs/decisions/0029-stable-abi.md (abi-v1 +
    reproducible builds = the fixed-point substrate) + docs/decisions/0038-self-host-
    port-lexer.md (the port's spine) + docs/agent-protocol.md + auto-memory
    `sentinel_selfhost_port`.

    NORMS (HARD): never git push (dev pushes via GitHub Desktop); four-check green gates
    EVERY commit; feat+docs commit pairs per increment (docs = ADR amendment + STATE
    banner + HANDOVER §0.3 + auto-memory); never commit leaking code; commit messages
    backtick-free (zsh — `git commit -F /tmp/msg.txt`), end with the `Co-Authored-By:
    Claude Opus 4.8 (1M context)` trailer; no nested heredocs. Work on main (don't
    branch). Small reviewable patches; build between each. Add seeds to the stage's
    differential test per slice. Agents help at the EDGES (de-risking probes, corpus
    analysis, adversarial review) per docs/agent-protocol.md — NOT the sequential stage
    build; the orchestrator re-runs probe snippets (agent sandboxes deny `snc` exec).

---

