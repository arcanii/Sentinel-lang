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

### ▶ RESUME HERE (2026-06-29 — ADR 0065 EFFECT-FREE PATH COMPLETE + self-hosted (stage-4 codegen byte-identical, both fixed points green) + the two typing limitations CLOSED; ▶ NEXT: stage 3 — `return` crossing a `handle` (D6), which is BLOCKED on a pre-existing staged-effect-runtime gap, see the assessment below)

> **▶ ADR 0065 effect-free path is DONE and self-hosted** (commits `3bc85e95`, `6114dc5f`). The
> **real `return` control-flow text-IR** is byte-identical in BOTH text emitters — the `snc llvm`
> oracle (`crates/sentinel-driver/src/llvm_dump.rs`, was a stub) and the self-hosted `scg` (the
> `cg` mode of `dump_texpr` in `selfhost/types.sentinel`, was carrying just the operand): eval
> inner → drop every live binding to the fn floor (`emit_loop_exit_drops(0)` / `cg_drop_range(c,0)`)
> → `ret` w/ the epilogue ABI (main i64→i32; effecting → `sentinel_kont_pure`; ordinary) → park a
> dead block. Selfhost MIR mirrors the Rust `Return → Opaque([inner])`. Two single-file fixtures —
> `tests/pass/c65_return.sentinel` (return in an `if` + a heap binding live across the return +
> statement-position) and `tests/pass/c65_return_match.sentinel` (return in a `match` arm) — bring
> `return` INTO every differential corpus; `snc llvm` == `scg` byte-for-byte, **both bootstrap
> fixed points hold** (verified on Windows under vcvars). The two deferred typing limitations are
> **CLOSED** in `sentinel-types`: the **coerce-skip** (a divergent node returns early from
> `check_expr` before `coerce_to_expected`, so `return e` is valid in ANY expected context) and the
> **match-arm divergence** (the result-type join skips a diverging arm). Mismatched-divergent
> demonstrators are `examples/lang/early_return*.sentinel` (snc-only). The selfhost typer needs NO
> `expr_diverges` mirror — it is a pure dumper (never rejects), and the only difference is the node
> type for a *mismatched*-divergent join (snc-only).
>
> **▶ STAGE 3 (`return` crossing a `handle`, D6) — REMAINS, and is BLOCKED first.** Assessment
> (2026-06-29): the handle/`perform` codegen is staged — it supports only specific body *shapes*
> (direct `perform`, let-shape, embedded-perform, chained lets — `compile_effecting_fn_with_*`). A
> handle body whose **control flow reaches a `perform`** (an `if`/`match` whose branch performs) is
> NOT a supported shape and currently **SILENTLY MISCOMPILES** — it builds but returns the wrong
> value. This was confirmed INDEPENDENT of `return`: `handle if c { perform Io.read() } else {
> perform Io.read() } with { Io.read(k) => k(42) }` already returns 0 instead of 42 (test it with
> `snc build` on a scratch file). So a `return` inside a handle body cannot be done until that gap
> is closed. **Stage 3 = (3a) make control-flow handle bodies lower correctly** (frame reification
> at `if`/`match` sites — the incremental work ADR 0020 anticipated) **THEN (3b) the
> `return`-crossing-`handle` teardown** (a new runtime `sentinel_kont_free` that walks `frames_head`
> freeing each captured block/frame, + handle-region tracking in codegen so an early return tears
> down the in-flight kont) — both across the three emitters. The effect runtime is the youngest
> subsystem (`crates/sentinel-runtime/src/lib.rs`: `SentinelKont`/`SentinelFrame`/`kont_push`/
> `kont_resume`). **Safer interim (a small bounded prerequisite):** REJECT a `return` whose lexical
> parent is a `handle` body/arm with a clear diagnostic so the miscompile is never silent (the ADR
> open question). Weigh that against implementing D6 outright. Spec for the `return` lowering = the
> inkwell `crates/sentinel-codegen/src/lib.rs` (`emit_return_drops` + `build_fn_return` + the Return
> arm). Also pending: the final macOS cross-platform confirmation; then ADR 0065 ACCEPTED.

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
