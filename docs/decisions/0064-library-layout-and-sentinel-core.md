# ADR 0064: Library layout — a top-level `/sentinel_library/` + the `Sentinel::` core base

Status: **ACCEPTED — v1 implemented (2026-06-28, Windows-verified; macOS `just check-all` +
self-host differential backlogged like the rest of this session — not oracle-moving, so the
differential is expected byte-identical).** Give the repo a real home for *many* first-party libraries
(a top-level `/sentinel_library/`), and carve an identity-defining FOUNDATION out of `std`:
the `Sentinel::` core base — the small constant-time `secret` vocabulary the rest builds on.
`std::` stays the broad **batteries**, relocated under the new tree and `use`-ing `Sentinel::`
for the core. This is a **filesystem move + module-path edits + a harness search-root change —
NOT oracle-moving** (no lex/parse/IR change; no `selfhost/` file moves; module resolution is
generic path-mapping, so no compiler change), so no re-bless, no `selfhost/` mirror, both
bootstrap fixed points unaffected — the same property as ADR 0037 point 12 and ADR 0062.

Date: 2026-06-28
Related: **0037** (modules / `discover_module_graph` — `use a::b::Item` → `<base>/a/b.sentinel`,
the generic path-mapping this relies on; point 12 `--lib-path`), **0062** (file-level conditional
compilation — also a resolve/harness-only, not-oracle-moving change; the `_<os>` selector
composes unchanged under the new tree), **0061** (code signing & trust — `tools/trust/*`'s build
invocations are a touch point), **0059** (`snc build --lib` — the multi-module export path
`crypto_lib` rides, a touch point), **0047/0051/0052** (`[secret u8]`, the public→secret widen,
`Vec<secret u8>` — the secret-boundary machinery the core module wraps).

## Context

The examples-as-tests track has grown `std/` into a substantial, real library — ~30
`std/security` crypto modules, a networked `sshd`, the data/text/collections trio, math, sys.
Two structural pressures have built up:

1. **The repo has one library tree (`std/`) but wants many.** As patterns recur we want to
   refactor them into first-party libraries beyond `std` (e.g. a future `Sentinel::ct`-built
   crypto crate, app frameworks, the planned threading/GUI libraries). There is no top-level
   home that says "first-party libraries live here."

2. **A `widen`/`reveal` secret-boundary pair is copy-pasted across the codebase.** The exact
   same two helpers —
   `widen(src: &[u8]) -> [secret u8]` (lift public bytes into the secret domain) and
   `reveal(d: [secret u8]) -> [u8]` (declassify a buffer byte-by-byte) —
   appear **byte-identical** in `examples/export/crypto_lib.sentinel`,
   `tools/trust/sign_core.sentinel`, and `tools/trust/keygen_core.sentinel`. This is the
   single most fundamental operation in the language — crossing the `secret` boundary — and it
   has no library home, so every program that needs it reinvents it.

The insight that resolves both: **Sentinel's foundational data types (Vec / array / string /
`print` / `secret`) are COMPILER BUILTINS, not library code.** So the foundational *library*
layer is not "containers and strings" (those are builtin) — it is the **constant-time `secret`
vocabulary**, which is Sentinel's entire reason to exist. That deserves a first-class,
branded home: `Sentinel::` (capital, first-party), distinct from `std::` the batteries.

## Decision (proposed)

### D1. A top-level `/sentinel_library/` houses all first-party libraries.

Every first-party library tree moves under `/sentinel_library/`:

```
/sentinel_library/
  Sentinel/        # the core base (D2) — the identity-defining foundation
  std/             # the batteries (D4) — relocated, namespace unchanged
  <future libs>/   # room to grow: more first-party libraries land here
```

Module resolution is unchanged: `use a::b::Item` still maps to `<base>/a/b.sentinel`. The only
thing that moves is the *base directory* the harness/build points at — from the repo root to
`/sentinel_library/` (D6). Because resolution is pure path-mapping (ADR 0037), no compiler
code knows the name `std` or `Sentinel` — there is nothing to special-case.

### D2. The core base = `Sentinel::` — the constant-time `secret` foundation.

`/sentinel_library/Sentinel/` is the small, identity-defining layer the rest of the libraries
build on. It is **flat** (no sub-categories): the core is meant to stay small. v1 seeds it with
exactly two modules:

- **`Sentinel::ct`** — the branch-free constant-time primitives (`ct_diff`, `ct_combine`,
  `ct_not`, `ct_mask`, `ct_select`, `ct_memcmp`, `ct_vec_eq`, the `ct_rotl*`/`ct_rotr*`
  rotates). **Moved verbatim** from `std/security/ct` (no behavior change).
- **`Sentinel::secrets`** — the secret-boundary helpers (D3), DRY'd from the three copies.

Rationale for `Sentinel::` (capital, first-party branding): the constant-time `secret`
discipline IS Sentinel; the module path should say so. `std::` is the conventional
"everything else" namespace; `Sentinel::` is the thing only Sentinel has.

### D3. `Sentinel::secrets` — the DRY'd secret-boundary vocabulary.

The copy-pasted pair becomes two `pub fn`s in `Sentinel::secrets`:

```sentinel
// Sentinel::secrets — the public⇄secret boundary helpers.
pub fn widen(src: &[u8]) -> [secret u8] { … }   // public bytes → the secret domain
pub fn reveal(d: [secret u8]) -> [u8] { … }     // declassify a whole buffer, byte-by-byte
```

Bodies are the existing proven implementations (unchanged). `crypto_lib`, `sign_core`, and
`keygen_core` drop their local copies and `use Sentinel::secrets::widen; use
Sentinel::secrets::reveal;`. `reveal` is an array-level declassify built on the per-scalar
`declassify` special form — it does not add a new escape hatch; it packages the sanctioned one.

> **Naming note (the one real fork — RECOMMENDATION + flag).** The owner proposed
> `Sentinel::secret`, but **`secret` is a reserved keyword** (`TokenKind::Secret`), and a
> `use`-path segment must lex as an `Ident` (`parser.rs::parse_use_decl`). So `use
> Sentinel::secret::widen` **does not parse**. Options:
> - **(a) `Sentinel::secrets`** (plural — `secrets` lexes as an `Ident`; the lexer already
>   tests this). One-letter change from the owner's intent, reads naturally ("the secrets
>   module"), **zero compiler change. RECOMMENDED.**
> - (b) Allow keywords as `use`-path segments — a parser change (touches the front-end, larger
>   blast radius, marginal benefit). Rejected for a naming convenience.
> - (c) A different word (`Sentinel::boundary`, `Sentinel::taint`) — further from the owner's
>   intent than (a).
> **Owner-confirmed (2026-06-28): (a) `Sentinel::secrets`.**

### D4. `std::` stays the batteries, relocated and re-pointed at the core.

`std/` moves wholesale to `/sentinel_library/std/` with its **namespace unchanged** — every
existing `use std::…` outside the carved-out modules keeps working (it is the same module path,
only the base dir moved). The two carved-out modules are the exception:

- `std/security/ct` is **deleted** (it is now `Sentinel::ct`); its importers
  (`std/security/{siphash,sha256,sha512,sha3,chacha20,poly1305}`, `std/math/num`,
  `std/bytes/bytes`, `std/bits/bits`, and the `ct`-using `examples/*`) switch
  `use std::security::ct::X` → `use Sentinel::ct::X`.

No re-export shim: the move is clean and importers update. (A shim would keep two names for one
thing — exactly the drift this reorg removes.)

### D5. Not oracle-moving.

A filesystem move + `use`-path string edits + a harness search-root change. No lexer / parser /
AST / IR change; `discover_module_graph` resolves `Sentinel::ct` exactly as it resolves any
`a::b`. **No `selfhost/*.sentinel` file moves** (the self-hosted compiler does not depend on
`std`/`Sentinel`), so every selfhost differential dump and both bootstrap fixed points are
byte-identical. No re-bless, no mirror. (Same not-oracle-moving property as ADR 0037 pt 12 /
ADR 0062.)

### D6. Touch points (the complete list).

- **(a) The move.** `git mv std → sentinel_library/std`; `git mv
  sentinel_library/std/security/ct.sentinel → sentinel_library/Sentinel/ct.sentinel`; add
  `sentinel_library/Sentinel/secrets.sentinel` (new, D3).
- **(b) The example harness** — `crates/sentinel-driver/tests/examples.rs::assemble()` copies
  `repo_root().join("std")` next to each flattened entry. It must copy the whole
  `/sentinel_library/` *contents* (so both `std/` and `Sentinel/` sit next to the entry) →
  `copy_dir_recursive(&root.join("sentinel_library"), &dir)`.
- **(c) The export harness** — `crates/sentinel-driver/tests/export.rs` (the multi-module
  `crypto_lib` test) copies `root.join("std")`; same fix as (b) (it now also needs `Sentinel/`,
  since `crypto_lib` will `use Sentinel::secrets`).
- **(d) `--lib-path` / build-root references** → point at `/sentinel_library`:
  - `crates/sentinel-driver/tests/sign_core.rs::build_core` (`--lib-path <root>` →
    `<root>/sentinel_library`).
  - `crates/sentinel-driver/src/trust_tools.rs` (the error-hint string `--lib-path <repo-root>`).
  - `demos/win32/messagebox_compact.sentinel` build comment + `demos/win32/README.md`
    (`--lib-path .` → `--lib-path sentinel_library`).
- **(e) `use` edits** — `use std::security::ct::X` → `use Sentinel::ct::X` (the D4 importers);
  `fn widen`/`fn reveal` local defs removed in favor of `use Sentinel::secrets::{widen,reveal}`
  (crypto_lib, sign_core, keygen_core).
- **Docs** — `docs/STATE.md` + `docs/HANDOVER.md` updated to record the new layout (and that
  `std::security::ct` is now `Sentinel::ct`).

`conditional.rs` and the `--lib-path` tests in `modules.rs` use **synthetic** modules in temp
dirs (not the repo `std/`), so they are **not** touch points.

### D7. Phasing (incremental, four-check green at each step).

1. **Relocate `std/` → `/sentinel_library/std/`** (namespace unchanged) + repoint the two
   harnesses (b)(c) + the `--lib-path` references (d). Four-check green. This is a pure move:
   no `use` edits, smallest possible diff, easy to verify.
2. **Carve out `Sentinel::`** — create `Sentinel/`, move `ct`, add `secrets` (D2/D3), switch
   the D4 importers and the three copy-paste sites (e). Four-check green.

Each phase is independently green so a regression bisects cleanly.

## Self-host

**Not oracle-moving** (D5). No `selfhost/` mirror, no re-bless; both bootstrap fixed points are
byte-identical before and after.

## Constant-time guarantee

**Untouched.** Moving `ct` and packaging the secret-boundary helpers changes no `secret` rule
and no `sentinel::mir::secret_leak` behavior. `Sentinel::secrets::reveal` is built on the
existing per-scalar `declassify` special form — it is the sanctioned escape, packaged, not a
new one. `widen` is the existing public→secret operand widen (ADR 0051), packaged. Every moved
module is CT-checked exactly as before.

## Non-goals (v1)

- **New core modules beyond `ct` + `secrets`.** The core stays minimal; more lands when a
  genuine foundation-level need recurs.
- **A `prelude` / auto-import.** Every use is still explicit `use Sentinel::…`. Implicit
  preludes are a separate, larger decision.
- **Splitting `std` further** (e.g. `std::crypto` vs `std::security`) — out of scope; `std`
  relocates with its current shape.
- **A package/manifest format** for third-party libraries — `/sentinel_library/` is the
  first-party tree; external packages are the ADR 0061/0063 trust+descriptor track.

## Open questions

- ~~**The `Sentinel::secrets` name**~~ — RESOLVED (2026-06-28): owner-confirmed
  `Sentinel::secrets` (D3 naming note); both phases (D7) landed Windows-verified.
- **Flat vs nested core.** v1 keeps `Sentinel::` flat (`Sentinel::ct`, `Sentinel::secrets`). If
  the core grows, a shallow grouping may help — deferred until it does.
- **Should `std::security` re-export `Sentinel::ct`** for discoverability (so crypto authors
  find it under `security`)? v1 says no (one name per thing); revisit if it bites.
