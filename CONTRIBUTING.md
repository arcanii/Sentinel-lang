# Contributing to Sentinel

Thanks for your interest. Sentinel is an early-stage research project
([Anie Ltd.](https://aniesolutions.ai)), and its design is still fluid —
language semantics, the compiler's internal APIs, and the ABI surface all
change as the work proceeds. To keep that exploration coherent, the
contribution posture is deliberately narrow right now.

When this file and [`docs/STATE.md`](docs/STATE.md) disagree about current
status, STATE.md wins.

## Not (yet) accepting general feature PRs

We are **not** taking unsolicited feature or refactor pull requests at this
stage. The architecture is driven by an ADR trail (see
[`docs/decisions/`](docs/decisions/)) and a per-stage differential-validation
discipline; a feature PR that hasn't been designed through that process
usually can't be merged without redesign, so opening one is likely to waste
your time. If you have a design idea, open an issue describing the *problem*
first — not a finished patch.

## What is welcome

- **Bug reports.** A minimal `.sentinel` reproducer + the `snc` command you
  ran + expected vs actual (exit code / diagnostic / panic). Compiler crashes
  and miscompiles are especially valuable. The corpus under
  [`tests/pass/`](tests/pass/) and [`tests/ui/`](tests/ui/) shows the fixture
  shape we use.
- **Security-relevant findings.** See *Reporting security issues* below.
- **Documentation fixes.** Typos, stale claims, broken links, clarity. The
  docs are large and parts lag the code; corrections are appreciated. If a
  doc overclaims (especially around the `secret` / constant-time guarantee),
  that is a bug — report it.
- **Test cases.** A `.sentinel` program that *should* compile-and-run (or
  *should* be rejected) but isn't handled as expected — as a bug report or,
  if you're set up to build, a fixture + the expected exit code / diagnostic.

## Reporting security issues

Please **do not** open a public issue for an exploitable finding — for
example, a way to make the borrow checker accept use-after-free / double-free,
or a `secret` value that reaches a branch / index / divisor without a
`sentinel::mir::secret_leak` rejection (a constant-time-verification bypass).

Report these privately: use GitHub's **"Report a vulnerability"** (Security →
Advisories) on this repository, or email the maintainer
(`bryan.mark@gmail.com`). Include a reproducer and the impact. We'll
acknowledge and coordinate a fix before any public disclosure.

The constant-time guarantee has known, documented boundaries (the type system
is the taint oracle; verification runs pre-LLVM-optimization — see the README
"headline" section). A finding *within* those stated boundaries is a
documentation/scope matter, not a vulnerability; a finding that breaks the
guarantee *as stated* is a security bug.

## Ground rules for any change

- Every change lands **four-check green**: `cargo build --workspace`,
  `cargo nextest run --workspace`, `cargo test --doc --workspace`, and
  `cargo clippy --workspace --all-targets -- -D warnings`.
- Oracle-moving changes (anything that alters `snc`'s stage dumps or emitted
  IR) follow the established rhythm: ADR PROPOSED → Rust `snc` + fixtures →
  re-bless the per-stage differential → mirror into `selfhost/*.sentinel` →
  both bootstrap fixed-point paths green → ADR ACCEPTED. See
  [`docs/HANDOVER.md`](docs/HANDOVER.md) for the method.
- Match the surrounding code's style; no new dependencies without discussion.

## Where to look first

- [`docs/STATE.md`](docs/STATE.md) — authoritative current status.
- [`docs/project-context.md`](docs/project-context.md) — condensed rules of the road
  for AI agents (and a fast orientation for humans): stack pins, pipeline invariants,
  the constant-time `secret` discipline, and the ADR + dual-bootstrap workflow.
- [`docs/decisions/`](docs/decisions/) — the ADR trail (the "why").
- [`README.md`](README.md) — what Sentinel is, and explicitly *is not*.
- [`docs/borrow-check-limitations.md`](docs/borrow-check-limitations.md) —
  known borrow-checker edges (all currently over-rejections).
