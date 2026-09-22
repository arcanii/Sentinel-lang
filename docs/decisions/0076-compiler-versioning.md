# ADR 0076: The compiler carries a semantic version, starting at 0.1.0

Status: **APPROVED** (2026-09-22) — all six decisions ratified by the maintainer, D1 with
the clarification that the number is for public communication. Moves to **ACCEPTED** when the
Implementation list lands, which is this repo's convention for what ACCEPTED means.
Closes register item **D73** (D5).

Nothing in the tree is a release version today. `snc` identifies itself as `C1.0b`, a Phase
C sub-slice code from [ADR 0011](0011-phase-c1-kickoff-and-type-system-plan.md), in a
compiler that has since self-hosted; the workspace version is the cargo-init placeholder
`0.0.1`; there are no git tags and no changelog. This ADR gives the toolchain a number, says
what moves it, and says where it may not appear.

## Related

- [ADR 0011](0011-phase-c1-kickoff-and-type-system-plan.md) — where `C1.0b` comes from: a
  phase code for the salsa front-end retrofit, not a release label.
- [`abi-v1.md`](../abi-v1.md) — the emitted-code contract, **frozen and stable at 1.0**,
  with its own version in its own name. D1 keeps it independent.
- [ADR 0063](0063-prebuilt-library-consumption.md) — the `.sif` descriptor, whose
  header carries `sentinel-interface v1` (a FORMAT version) and `compiler <version>` (this
  ADR's number). Two different things on adjacent lines; D1 keeps them apart.
- [ADR 0037](0037-modules.md) — `snc build --separate` and the per-unit
  fingerprint cache that D5 fixes.
- [ADR 0025](0025-phase-c5-kickoff-and-productionization-plan.md) D8 — reproducible builds, pinned by
  `crates/sentinel-driver/tests/repro.rs`. D5 is written to stay inside it.
- Register **D73** — `--separate` reuses per-unit objects an older `snc` compiled, because
  the fingerprint's only compiler identity is a version that has never moved.

## Context

### What carries a version today

| Surface | Value | Read by |
|---|---|---|
| `snc --help` banner, [`main.rs:141`](../../crates/sentinel-driver/src/main.rs) | `C1.0b` | a human, and nothing else |
| root `Cargo.toml` `[workspace.package]` | `0.0.1` | inherited by all 16 member crates |
| `.sif` header, [`descriptor.rs:95`](../../crates/sentinel-driver/src/descriptor.rs) | `compiler 0.0.1` | `env!("CARGO_PKG_VERSION")` |
| `unit_fingerprint`, [`main.rs:2162`](../../crates/sentinel-driver/src/main.rs) | hashed | `env!("CARGO_PKG_VERSION")` |

Verified while writing this, rather than assumed:

- **Nothing asserts `C1.0b`.** It appears in five places: the banner and four doc comments.
  No test, snapshot or fixture reads it.
- **Nothing asserts `0.0.1`.** Every apparent hit in the tree is the IPv4 literal `127.0.0.1`.
- **`InterfaceHeader::compiler` is parsed and never read.** `read_interface` stores it in a
  field that has no reader anywhere in the workspace, and the module's own tests assert
  `module`, `object_sha512` and `version` but not `compiler`. Changing the version's text
  moves no behaviour there. (`render_interface` / `read_interface` also have no callers
  outside those tests yet — ADR 0063's format is built and tested, not yet wired to a
  command.)
- **`snc llvm` output contains no version string**, which is load-bearing — see D6.
- **`repro.rs` compiles one fixture twice with the SAME binary** and asserts the two objects
  are byte-identical. A value fixed at compile time is identical in both processes, which is
  why D5 is safe there.
- **The workspace has no `build.rs`,** and D5 is written to keep it that way.
- **`std::env::current_exe()` already has two callers in the driver** (locating the runtime
  library, and `trust_tools`), so D5 needs no new API surface.

### Why the placeholder is not merely untidy

`CARGO_PKG_VERSION` is already wired into the incremental-compilation cache key, and its own
doc comment says the version is there to "invalidate on upgrade". Because the version has
never moved, that invalidation has never fired. Register **D73**: a `--separate` rebuild into
a directory holding a `<unit>.o` / `.o.fp` pair from an earlier `snc` prints `snc: fresh` and
links the old object — keeping whatever that compiler got wrong, including miscompiles later
commits fixed. The workaround in the register is "build `--separate` into a fresh directory
after upgrading `snc`", which is a thing a person has to remember.

So this is not only a marketing question. The version is a load-bearing input to a cache that
is currently constant.

## Decisions

### D1. The number versions the COMPILER and the LANGUAGE SURFACE. It does not version the ABI or the descriptor format.

`snc` gets a `major.minor.patch` semantic version. It covers what a user writes and runs: the
language accepted, the diagnostics that reject, the CLI, and the runtime a compiled program
links.

**This is the number for public communication — it answers "what version of Sentinel do I
have?"** That question has no answer today, and it is the one a user, a bug report or a
release note actually asks. So the scope is deliberately the pairing a user experiences as
one thing: the language and the compiler that implements it. Versioning only the toolchain
and leaving the language unnumbered would keep the question unanswered; versioning them
separately would make a person carry two numbers to describe one install.

The other two versions in the system are contracts between ARTIFACTS, not things a user
names, which is why they stay separate.

It does **not** cover:

- **`abi-v1`** — the emitted-code contract, deliberately frozen at 1.0, versioned in its own
  name and amended under its own rules (ADR 0016 A1 is the precedent). Coupling the two would
  force an `abi` bump on every language change, which is exactly what freezing it was for.
- **`sentinel-interface v1`** — the `.sif` FORMAT version. A descriptor written by a newer
  compiler in the same format is still `v1`.

The `.sif` header already shows the split, one line apart:

```
sentinel-interface v1
abi 1
compiler 0.1.0
```

Three versions, three contracts, three lifetimes. That is correct and this ADR preserves it.

### D2. The scheme is pre-1.0 semver, starting at **0.1.0**, and the MINOR is the breaking slot.

| Bump | What it means | Examples from the current register |
|---|---|---|
| **patch** — `0.1.0` → `0.1.1` | A fix with no user-visible surface change and no emitted-IR change for a program that already compiled. | D95 (a `scg`-only transposition), D96, D97 |
| **minor** — `0.1.x` → `0.2.0` | Anything user-visible. In 0.x this is where breaking changes live: a new language feature, a new or changed CLI flag, a new runtime symbol, an `abi-v1` amendment, or a rule that now REJECTS a program that previously compiled. | ADR 0075 (a new symbol and a changed answer), ADR 0074, ADR 0071's milestones |
| **major** — `0.x` → `1.0.0` | Reserved. See D3. | — |

**The bump test rides vocabulary the project already has: an ORACLE-MOVING change is at least
a minor.** `CLAUDE.md` and `docs/project-context.md` already define oracle-moving as anything
that alters `snc`'s stage dumps or emitted IR, and already give it a fixed landing rhythm. A
change that moves the oracle has, by construction, changed what a user's program compiles to.
Reusing that test means there is one definition of "user-visible", not two that can drift.

A rejection that is a FIX is still a minor: ADR 0075 D3's `MovedInHandlerArm` refuses programs
that used to compile, and the fact that those programs were unsound does not help someone
whose build stops.

### D3. `1.0.0` is reserved for the production bar. The Phase C milestone loses its number.

Calling the Phase C close "Sentinel 1.0" was aggressive. It was the proof that the design
works — the full language compiles and runs with machine-verified constant-time `secret` —
which is a proof-of-concept milestone, not a release. `README.md` already says so in its own
words: it "was **not** a production release: single-process, single-file, loop-free-by-design,
no standard library at the 1.0 close."

So the rename is not just collision-avoidance. **The milestone drops the number entirely** —
"the Phase C bootstrap close", or "the bootstrap proof of concept". Keeping a number in it
(an earlier draft of this ADR proposed "the 1.0 bootstrap milestone") would leave the string
`1.0` attached to a proof of concept and only half-solve the problem. Seven occurrences
across four files: `README.md` (3), `docs/STATE.md` (2), `docs/project-context.md` (1),
`docs/HANDOVER.md` (1).

`1.0.0` becomes a **destination**, and the project is on a course to it rather than past it.

**What the bar is.** Not invented here — it is `README.md`'s own "What this is not",
minus what has since closed. Phase D has already retired several of the 1.0-close limits
(self-hosting, per-unit separate compilation, loops, modules, sum types, strings, `Vec`, I/O,
threads and channels, multi-processing). What that section still says is open:

- **API stability.** "Every API can change." The `abi-v1` compiled-artifact contract is frozen
  and layout-tested; the Rust crate APIs are not.
- **The constant-time guarantee's remaining reach.** The check runs pre-LLVM and does not yet
  FORCE constant-time emission: speculation-barrier / `cmov` emission, post-codegen assembly
  verification, an independent secret-dataflow oracle, `[secret T]` arrays.
- **The borrow checker is lexical and over-rejects.** The Polonius migration is the
  post-1.0-close item; every current limitation is an over-rejection, so sound but not
  ergonomic.
- **Tooling.** `sentinel-lsp` is a stub.
- **Contribution posture.** "Not accepting general contributions yet — the design is still
  fluid."

Two judgement calls this ADR deliberately does NOT make, because they are the maintainer's:
whether a standard library is required for `1.0.0` or belongs to the ecosystem (the README
argues the latter for cipher suites), and whether Polonius must land before `1.0.0` or an
over-rejecting borrow checker is acceptable in a 1.0. Both change the shape of the road, not
the scheme, so they can be settled later without reopening this.

### D4. The version has TWO parts: a hand-maintained semver and a computed build id.

```
snc 0.1.0 (0x3f2a1c9d4e5b6a70)
```

- **`0.1.0`** is hand-maintained in the root `Cargo.toml` and moves by D2's rules. It is the
  number a human says out loud, and nothing computes it.
- **`0x3f2a…`** is the build id from D5: computed, 16 hex digits, identifying *this build of
  the compiler* rather than its source revision.

The two answer different questions and neither substitutes for the other. "Which release is
this?" is the semver. "Is this the same compiler that produced that object?" is the build id,
and only the build id can answer it — see D5.

`--version` and `-V` slot in beside the existing `-h | --help | help` arm of `main`'s
slice-pattern match; no existing single-flag pattern conflicts. It prints that one line to
**stdout** and exits **0**. The usage banner keeps going to stderr with exit 2 on a bad
invocation, which is right for usage and wrong for a version query. The same string replaces
`C1.0b` in the banner's first line.

Two variations available without reopening this: appending the artifact contract
(`snc 0.1.0 (0x3f2a…, abi-v1)`), which helps when a bug report is about a compiled artifact
rather than the compiler; and truncating the displayed id to 8 digits. Both are display-only
and change nothing else.

### D5. The build id is the executable's own size and mtime, hashed. It feeds `--version` AND `unit_fingerprint`, which closes D73.

```rust
// once per process
let meta = std::env::current_exe().and_then(std::fs::metadata);
// hash (len, modified) with DefaultHasher -> format!("{:016x}", ..)
```

`DefaultHasher` is already the fingerprint's hasher and is process-stable by fixed keys (its
own doc comment in `main.rs` says so); `{:016x}` is already the format `unit_fingerprint`
writes into a `.o.fp` sidecar. This reuses both.

**Why this and not a git SHA.** D73 bites hardest during compiler development, when `snc` is
rebuilt repeatedly without committing. A commit SHA does not move across an uncommitted
change, so the stale object would still be reused — the case that matters most is the one a
SHA misses. Appending `-dirty` does not rescue it, because two different dirty trees share
the marker. Size and mtime move on every rebuild, which is exactly the signal wanted.

It also costs nothing: one `stat`, no `build.rs` (the workspace still has none), no `git` at
build time, no new dependency. `std::env::current_exe()` already has two callers in the
driver.

**Fail-closed on error.** If `current_exe()` or `metadata()` fails, the id must fall back to a
value that ALWAYS invalidates — never to the semver alone. Falling back to a constant
silently restores D73 on exactly the platforms where the lookup is flaky, which is the worst
place to be quiet. A spurious rebuild is the acceptable failure; a stale object is not.

**What it identifies, and what it does not.** This is a build-INSTANCE id, not a
build-CONTENT id: the same binary copied to another machine, or restored from a backup, gets
a different id, and two byte-identical builds never share one. For invalidation that is the
safe direction (it over-invalidates, never under-invalidates). For display it means "same id"
proves "same build", while "different id" does not prove "different compiler". If content
identity is ever wanted, hashing the executable's bytes slots in behind the same interface
— a localized change, at the cost of hashing 36 MB per invocation with an in-tree SHA-512
written for ed25519 rather than for throughput.

**This closes register D73**, and is that entry's own stated fix direction: "fold the
compiler's own identity (a build id, or the executable's size and modification time) into the
fingerprint."

Safe under [ADR 0025](0025-phase-c5-kickoff-and-productionization-plan.md) D8: `repro.rs`
compares two runs of the SAME binary, whose size and mtime are identical, so the id is too.

### D6. The version MUST NOT appear in emitted LLVM IR. This is a fail-closed rule, not a preference.

`snc llvm` emits no version string today, and it must stay that way.

The codegen differential compares the text oracle's IR against the self-hosted `scg`'s **byte
for byte**, and both bootstrap fixed points depend on that equality. `scg` is built from
`selfhost/*.sentinel` and has no access to a Rust crate's `CARGO_PKG_VERSION`. A version
stamp in emitted IR therefore breaks `sentinel_codegen_matches_oracle_on_corpus` and both
fixed points on the first build — loudly, which is the good case. The bad case is a stamp
added somewhere the differential does not reach.

Permitted: the `.sif` header, `unit_fingerprint`, `--version` and the usage banner, the
`--emit-header` C comment. Forbidden: anything `snc llvm` or `scg` prints — and that now
covers D5's build id too, which is the more tempting of the two to stamp into a header
comment for provenance.

This rule belongs in `docs/project-context.md` beside the other footguns, because it is
exactly the kind of thing a later agent adds helpfully.

## Consequences

- **The first `--separate` build after an upgrade recompiles everything.** That is the point
  of D5, and it is the correct cost: today it silently links objects from a compiler that had
  known miscompiles.
- **No `build.rs`, no build-time `git`, no new dependency.** D5 is one `stat` and a hash of
  two integers, in code that already exists.
- **`snc build --separate` stops reusing objects while the compiler itself is under
  development** — every `snc` rebuild changes the build id, so every `.o.fp` goes stale.
  That is correct, and it is a visible change in behaviour: the incremental cache now pays
  off when `snc` is FIXED and the user's sources change, which is what it was for. That it
  appeared to pay off during compiler work too is the bug being fixed.
- **Seven prose edits** rename the Phase C milestone, dropping its number. The project's
  biggest published claim changes shape: what was "Sentinel 1.0, reached" becomes "the
  bootstrap proof of concept, reached; 1.0.0 is ahead." That reads as a step back and is
  a step forward — it is the accurate one, and it is the maintainer's call, taken here.
- **A bump becomes part of landing an oracle-moving change**, alongside the ADR and the
  fixture. This adds a step to the rhythm in `CLAUDE.md`.
- **`abi-v1` and `sentinel-interface v1` are unaffected**, by construction (D1).
- Register **D73** closes with D5.

## Implementation

1. Root `Cargo.toml` — `version = "0.1.0"`. All 16 member crates inherit it.
2. `crates/sentinel-driver/src/main.rs` — a `build_id()` behind a `OnceLock`, so the `stat`
   happens once and `--version` and the fingerprint cannot disagree.
3. `main.rs` arg match — a `--version` / `-V` arm beside the `-h | --help | help` arm;
   stdout, exit 0.
4. `print_usage()` first line — `C1.0b` becomes the D4 string.
5. `unit_fingerprint` — hash `build_id()` alongside `CARGO_PKG_VERSION`. **Closes D73.**
6. Docs — D3's seven renames; a short "Versioning" section in `README.md` and
   `CONTRIBUTING.md`; D6's rule added to `docs/project-context.md` beside the other footguns;
   D2's bump rule added to the oracle-moving rhythm in `CLAUDE.md`.
7. Register — D73 marked DONE.

Deliberately NOT in this list: a `build.rs`, a `git` invocation, a new dependency, a
`CHANGELOG.md`, and git tags. The last two are release process rather than versioning scheme;
they can follow once the scheme exists, and neither is needed for D73 or `--version`.

## Verification

- **`--version`**: a driver test asserting the exact line shape, that it goes to stdout, and
  that the exit code is 0 — the last because the current code path for an unrecognized flag
  exits 2, and a version query landing there would look like it worked.
- **D6, the fail-closed rule**: the existing differentials already enforce it. Worth a
  deliberate check once — add the version to an emitted line, confirm
  `sentinel_codegen_matches_oracle_on_corpus` and both fixed points fail, revert. A rule
  nobody has watched fail is a rule nobody knows is wired up.
- **D5 / D73**: build `--separate` into a directory, rebuild `snc` at a different commit,
  rebuild `--separate` into the SAME directory, and assert the unit recompiles rather than
  printing `snc: fresh`. Mutation: revert the build-id hash line and watch it go stale
  again. Second mutation: force the `current_exe()` lookup to fail and assert the id still
  invalidates rather than collapsing to the semver — the fail-closed half of D5 is the
  part that will be quietly wrong if nobody checks it.
- **ADR 0025 D8**: `repro.rs` green, unchanged.
- **Four-check** and the full `selfhost_*` set, as for any change.
- The `.sif` unit tests are expected to pass untouched; `compiler` is asserted by none of
  them, which was verified rather than assumed.

## Alternatives considered

- **CalVer (`2026.9.0`).** Rejected: it says when, not what changed. The project's entire
  vocabulary — oracle-moving, `abi-v1` frozen, fixed points — is about compatibility, and a
  date expresses none of it.
- **Separate language and compiler versions.** Rejected as premature. There is one language
  with one implementation; `scg` is the same language, not a second. Revisit if a second
  front end ever exists.
- **Keep phase codes (`C1.0b`, then `D…`).** Rejected: they encode internal project phases,
  are meaningless outside the repo, and the current one is already stale by several
  milestones.
- **Tie the compiler version to `abi-v1`.** Rejected: the ABI is deliberately frozen and
  separately versioned. Coupling would either force spurious ABI bumps or freeze the compiler
  version, and both defeat one of the two.
- **Start at `1.0.0`** on the grounds that the Phase C milestone was called 1.0. Rejected: it
  would claim production readiness the `README`'s own "Not production-ready" section denies,
  and it would leave no room below for the alpha the project is actually in.


### For the build id specifically (D5)

- **A git short SHA via `build.rs`.** Rejected, and not on cost. A commit SHA does not move
  across an uncommitted change, so it misses the case D73 actually bites in — a compiler
  rebuilt repeatedly while being worked on. `-dirty` does not rescue it: two different dirty
  trees share the marker. It would also add the workspace's first build script and a
  build-time `git` dependency to buy a weaker signal.
- **Hashing the executable's bytes.** Deferred, not rejected. It is the only option that gives
  a build-CONTENT id, so identical binaries on two machines would agree. The cost is hashing
  36 MB per invocation with an in-tree SHA-512 written for ed25519 rather than throughput,
  which wants measuring first. It slots in behind D5's interface if that identity is ever
  needed.
- **A hand-bumped build constant.** Rejected for the BUILD id: it fails silently the first
  time someone forgets, and a stale-object bug that reappears intermittently is worse than one
  that is simply open. Note the distinction from D4 — the SEMVER is deliberately
  hand-maintained, because it encodes a judgement (what changed, and how much) that nothing
  can compute. The build id encodes a fact, so it should be computed.
- **Leave D73 open** and keep its documented workaround. Rejected: the workaround is "remember
  to use a fresh directory after upgrading `snc`", and the failure when it is forgotten is
  silent and links known-bad code.