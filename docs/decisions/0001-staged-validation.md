# ADR 0001: Staged validation before full bootstrap

## Status
Accepted, 2026-05.

## Context
Sentinel is an ambitious language with several novel pillars (runtime
memory broker, region-based safety with second-class refs, algebraic
effects, secret qualifier, signature infrastructure). Building all of
it in one push risks years of work before the foundational ideas are
validated.

## Decision
Follow the four-phase plan in HANDOVER.md:

  - Phase A: prototype the broker as a standalone Rust crate (3-6 mo)
  - Phase B: prototype the effects system as a research compiler (6-9 mo)
  - Phase C: build the production bootstrap compiler (12-18 mo)
  - Phase D: self-host (9-12 mo)

Each phase has a defined go/no-go criterion. Phase C does not begin
until Phase A and Phase B have validated their core ideas.

## Consequences
Slower path to a complete language; faster path to validated ideas.
Phase A and Phase B produce standalone value (a usable broker crate,
a publishable research artifact) even if Sentinel never proceeds.