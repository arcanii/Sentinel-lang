# Sentinel — Claude Code guide

Sentinel is a security-focused language; its compiler is a 15-crate Rust workspace
whose reason to exist is **machine-verified constant-time `secret`** handling. Most
rules exist to protect that guarantee or the self-hosting bootstrap — treat breaking
either as a serious regression, not a style nit.

## Read this first

The canonical, agent-facing implementation rules live in the project context file.
It is imported below so it loads every session — keep it as the single source of these
rules (edit it there, not here):

@docs/project-context.md

## Authority — when sources disagree

- **[`docs/STATE.md`](docs/STATE.md)** is the source of truth for current status; it
  wins over `docs/HANDOVER.md` and this file.
- Architecture is governed by the **ADR trail** in [`docs/decisions/`](docs/decisions/) —
  find and read the ADR before changing behavior it ratifies.
- Contributor process and the security-reporting policy: **[`CONTRIBUTING.md`](CONTRIBUTING.md)**.

## Highest-frequency commands

- `just check-all` — the mandatory four-check (build · nextest · doctests · `clippy -D warnings`)
- `just test` / `just test-all` (incl. doctests) · `just snc <args>` (run the `snc` compiler)
- `just bless` — update `insta` snapshots, then **review the diff** before committing

> Oracle-moving changes (anything altering `snc`'s stage dumps or emitted IR) must keep
> both bootstrap fixed points green — mirror Rust `snc` changes into `selfhost/*.sentinel`.
> See `docs/project-context.md` and `CONTRIBUTING.md` for the full rhythm.
