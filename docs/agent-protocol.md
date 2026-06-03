# Agent Protocol — the Sentinel self-host port

How we use multi-agent orchestration on Phase D movement 2 (the self-host port,
ADR 0038/0039/0040). Records **where agents help** (and where they don't), the
**briefing kit** every port-agent needs, and the **conventions** (isolation,
findings shape). The Rust `snc` stays the oracle; agents accelerate the *Sentinel
side* of each stage.

## Where agents help — and where they don't

The port's **core** — writing each `selfhost/*.sentinel` stage — is a single-file,
**sequential** loop: compile with `snc` → run → diff vs the `snc <stage>` oracle →
fix. It does not parallelize and needs sustained context. **Do not fan out the
stage build itself.** Agents help at the **edges**:

1. **Parallel de-risking probes** (HIGH value, *before* a big build). Every open
   design question — a new Sentinel idiom, a language-limit unknown — is an
   independent throwaway experiment. Run them concurrently. This is the ADR's
   "probe-first" discipline (ADR 0039 A3/A5/A11 settled `Vec<non-primitive>`, the
   `&mut Ret` out-param, the cons-list shapes this way), parallelized.
2. **Corpus analysis** (medium). Categorize `tests/pass` + `tests/ui` fixtures by
   which stage-features they exercise, to plan the seed progression (3a → 3e).
3. **Adversarial review** (medium-high, *after* a stage lands). An independent
   agent reviews the diff for leaks / borrow-quirk regressions / oracle
   divergence the differential test might not surface (it only covers the
   corpus). Diverse lenses: a `leaks` re-sweep, a borrow-rule audit, a
   "what corpus shape isn't covered" critic.

## Probe protocol

- **Isolation.** Each agent works in its OWN `/tmp/<probe-name>/` dir and **never
  touches the repo working tree**. Probes are **throwaway** — the *finding* + the
  *minimal working idiom* is the deliverable, not committed code. Parallel agents
  must use distinct dirs (the `snc` binary is read-only and safe to share).
- **Narrow question.** One viability question ("does X work in Sentinel?") + "what
  is the minimal idiom that works?". Keep the probe program tiny.
- **Structured finding.** Report: **WORKS / FAILS / WORKS-WITH-CAVEAT**; the exact
  minimal Sentinel snippet that compiled + ran + leak-checked clean; the
  recommended idiom for the real build; any gotcha hit on the way.
- **Verify-cheap.** The orchestrator re-runs the probe's snippet to confirm before
  acting on a finding (a wrong "it works" would misguide the build).

## Briefing kit (paste into every port-agent prompt)

**Build/run/leak.** `snc` = `target/debug/snc` (relative to the repo root
`/Users/bryan/Desktop/github_repos/Sentinel-lang`). Compile one file:
`snc build f.sentinel -o /tmp/p/bin`. Run: `(cd /tmp/p && ./bin)` (exit code = the
`main() -> i64` return). Leak-check: `(cd /tmp/p && leaks --atExit -- ./bin)` —
want `0 leaks for 0 total leaked bytes`. A program is a set of `fn`s with a
`fn main() -> i64`; output via `print(i64)` or building a `Vec<u8>` +
`print_bytes(vec_to_array(v))`.

**D.6 multi-file.** `use mod::Item;` imports `Item` from module `mod` = the file
`<entry-dir>/mod.sentinel` (source root = the entry file's dir; the last `::`
segment is the item, earlier segments are the path). Build the **entry** file;
the driver follows `use` edges. `pub` exports an item across modules.

**Sentinel idioms + quirks (ADR 0038 A2 / ADR 0039 A3 — these bite every stage):**
- Recursive ASTs are **recursive `enum`s returned BY VALUE** + a **consuming
  `match` dump**. `Vec<non-primitive>` is **unsupported** (`Vec<Expr>` /
  `Vec<[u8]>` / `Vec<struct>` → `vec_element_not_supported`) — variadic lists are
  **cons-list enums** (`enum L { End | Cell(T, L) }`). Only `Vec<i64>` / `Vec<u8>`
  (scalar element) is supported.
- Enum values are **Move-owned; there is NO clone**. To "copy" a cons-list you
  rebuild it by recursion.
- An **owned `[u8]` payload must be CONSUMED** (moved into a by-value fn, e.g.
  `append_str(out, bytes)`) to be dropped — merely **indexing** it (`b[i]`) leaks.
- Refs are indexed via **`(*r)[i]`** (auto `r[i]` fails on `&Vec`/`&[u8]`); a
  shared `&mut i64` **cursor** is threaded down and advanced (`*cur = *cur + 1`).
- **FLAT per-fn variable namespace**: every local needs a unique name even across
  disjoint `if`-arms (no shadowing / re-declaration).
- **Sibling-`if`-tail `&mut`-of-a-LOCAL borrows conflict** (`if c { f(&mut x) }
  else { g(&mut x) }` where `x` is a local → borrow_conflict). Fix: a SINGLE
  dispatch helper that re-passes the `&mut` **param** (re-passing a `&mut` *param*
  across if-tails is fine — cf. `parse_postfix_rest`).
- A **`&mut Enum` out-param's default must be a NULLARY variant** (assigning the
  real value through the ref does not free a boxed default → leak).
- `match` arms need **comma separators even with block bodies** (`Pat => { … },`).
- **`if` is an EXPRESSION**: every branch needs a tail value + a mandatory `else`
  (no bare `if` statement; loops/`break`/`continue` are statements).
- Left-assoc folds + accumulator threading go via **recursion** (a `while`-loop
  accumulator reassigning a Move binding trips moved-in-loop).

**Reference Sentinel stages** (read for idioms): `selfhost/lexer.sentinel`,
`selfhost/parser.sentinel` (the 2nd stage — the canonical idiom source).

## Models

Inherit the session model for subtle-semantics probes (scope/Move/borrow);
Sonnet is fine for mechanical ones (e.g. a 2-file module test). The orchestrator
verifies findings cheaply regardless of model.

## Status

First applied at **resolve (3a)** (2026-06-03): three parallel probes — D3
parse-sharing, the (3a)-core resolve mini-pipeline (fn-table + flat scope + RExpr
+ dump), and the D5 scope snapshot/restore (the 3c risk). See ADR 0040
amendments for the settled decisions.
