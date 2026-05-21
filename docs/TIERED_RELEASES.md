# TIERED_RELEASES.md — Sentinel Tiered Release Model Design

This document specifies the proposed tiered release model for Sentinel:
a system where compiled artifacts move through graduated trust levels,
with runtime overhead calibrated to each level and earned through
signed profile data and deliberate promotion workflows.

The model addresses Sentinel's central honest cost — that its runtime
safety checks impose overhead Rust's compile-time-only checks do not —
in a way that is principled rather than ad hoc. Code that has not
earned trust pays full overhead. Code that has earned trust through
sustained observation, fuzzing, and explicit security review can have
specific checks elided, with the elisions documented, signed, and
auditable.

The model also turns Sentinel's runtime instrumentation from a pure
cost into a strategic asset. The recording mode that produces overhead
at lower tiers is also the mechanism that earns the right to lower
overhead at higher tiers. The investment in instrumentation pays for
itself.

## 1. Problem Statement

Sentinel's safety guarantees come from a combination of compile-time
and runtime enforcement. Bounds checks are elided where the compiler
can prove safety and inserted otherwise. Generational handle checks
run on every access through a broker handle. Constant-time operations
and speculation barriers protect `secret` data. Broker bookkeeping
records every allocation. Most of this overhead is small individually
but compounds in tight loops to a 1-3% baseline cost in typical code,
and substantially more in code that exercises broker handles or
`secret` data heavily.

A portion of this overhead is fundamental: the compiler cannot prove
every array access is in bounds at every call site, and a runtime
check is the honest answer. A larger portion is conservative: the
runtime checks exist because the compiler cannot prove they are
unnecessary, not because they are actually triggered in practice. Code
that has run for two years in production across hundreds of millions
of invocations without ever tripping a particular bounds check is,
empirically, code where that check is dead weight.

What is missing is a mechanism to convert *observation* into
*permission to elide*. Profile-guided optimization exists everywhere
in modern compilers but uses profile data only for performance choices
that preserve semantics (inlining, code layout, specialization). No
production language uses profile data to make safety-vs-performance
tradeoffs that change runtime behavior.

The opportunity is to extend profile-guided compilation to safety
checks themselves, with the discipline that the elisions are signed,
audited, demoteable, and statistically defensible.

## 2. Goals

The tiered release model should provide the following:

  - A defined set of release tiers spanning maximum instrumentation
    (development), full safety with optimization (experimental
    release), selective check elision based on earned trust (stable
    release), and additional hardening for security-critical
    deployments (hardened release).
  - Per-module tier assignment, not per-binary, so that mature and
    immature code in the same project can ship at different tiers.
  - A signed trust profile that records what was observed about the
    code over its operational life, with cryptographic integrity
    across the data supply chain.
  - A deliberate, human-signed promotion workflow from lower tiers
    to higher tiers, with defined criteria including production
    observation, fuzzing coverage, and security review.
  - A defined demotion workflow for when vulnerabilities are
    discovered, with fast-path infrastructure for emergency
    response.
  - Compatibility with Sentinel's existing reproducible-build
    commitment: given the same source and the same profile,
    different builders produce byte-identical binaries.
  - Integration with the signature, effect, and broker
    infrastructure already specified in the project documents.

The model should never sacrifice cryptographic correctness or
side-channel hardening regardless of tier. The `secret` qualifier's
protections remain at full strength at every tier.

## 3. Non-Goals

The tiered release model is not a general-purpose performance
optimization framework. PGO for inlining and code layout exists
separately and is unaffected by this work. The tier system addresses
*safety check overhead*, not performance more broadly.

The model is also not a substitute for sound engineering. Promoting
code to a higher tier does not make the code correct; it documents
that observation has not found incorrectness in defined ways. The
limits of that documentation must be honestly communicated.

The model does not attempt to verify that promotion decisions were
*correct*. A team that promotes inadequately tested code to Tier S has
made an operational mistake the tier infrastructure cannot prevent. The
infrastructure can require evidence and signatures; it cannot require
good judgment.

## 4. The Tiers

### 4.1 Tier D — Development

Maximum instrumentation. Every safety check enabled. Recording mode
active by default. Full broker bookkeeping for every allocation and
handle access. No optimization that would obscure debugging. The
compiler inserts diagnostic assertions for invariants that would
normally be implicit. Error messages include full provenance
information.

Intended for use during development, in test suites, and in CI runs.
Performance overhead is substantial (typically 30-100% over Tier E,
sometimes more) and is acceptable in this tier because the goal is
diagnosis, not throughput.

### 4.2 Tier E — Experimental Release

All safety checks enabled but optimizations applied. Bounds checks
vectorized where possible. Generational handle checks inlined.
`secret` operations fully hardened. Constant-time guarantees enforced.
Speculation barriers in place. Recording mode available but optional;
typically off in latency-sensitive paths and on in long-running
services where the data is valuable.

Intended for staging environments, beta deployments, and early
production. Also the default for new code, code that has changed
recently, and code that has not yet earned promotion.

Most current production code in Rust corresponds roughly to Tier E.
Performance is 1-3% lower than equivalent Rust for typical code,
larger for code that exercises broker handles or `secret` data
heavily.

### 4.3 Tier S — Stable Release

Selective check elision based on signed trust profile data. Code paths
that have accumulated sufficient observation without ever triggering a
particular safety check, and that have been exercised by fuzzing
across their documented input space, are candidates for elision of
that specific check. Code paths without sufficient observation, or
that have triggered checks (legitimately or not), retain the check.

The compiler reads the trust profile alongside the source and produces
a binary calibrated to that profile. The produced binary carries a
manifest listing every elision and the profile data justifying it,
signed by the parties who authorized the promotion.

`secret` operations are not eligible for elision. Cryptographic
correctness and side-channel hardening remain at full strength
regardless of tier.

Performance approaches Rust release-build performance for
well-exercised code while remaining higher for newly-added paths in
the same module.

Intended for mature, performance-critical production code that has
been in operation long enough to have meaningful observation data.

### 4.4 Tier H — Hardened Release

The opposite direction from Tier S. Additional checks beyond Tier E,
not fewer. Used for security-critical deployments where the threat
model justifies additional cost.

Hardening includes: memory poisoning on free with detectable patterns,
anti-tampering checks on the running binary against signed expected
hashes, lifecycle audit logging for every secret operation, side-
channel information budget enforcement, resource-use anomaly
detection, optional periodic broker integrity self-verification, and
optional control-flow integrity checks beyond what the underlying
hardware provides.

Performance is lower than Tier E (typically 5-15% slower depending on
which hardening features are enabled). Used where that cost is
acceptable in exchange for runtime integrity properties Tier E does
not provide.

Tier H and Tier S are not mutually exclusive at the project level:
some modules can ship at Tier S (mature, performance-critical) while
others ship at Tier H (security-critical, performance-tolerant). The
tier assignment is per-module.

### 4.5 Tier Selection

Tier selection is declared in the build manifest per module:

    [module.crypto-primitives]
    tier = "hardened"
    hardening = [ "memory-poison", "anti-tamper", "ct-audit" ]

    [module.request-parser]
    tier    = "stable"
    profile = "profiles/request-parser-2.7.sigprofile"

    [module.experimental-feature]
    tier = "experimental"

    [module.new-code]
    tier = "experimental"  # default

The build refuses to produce a binary if any module is marked stable
without a valid signed profile, or if any module declares hardening
features inconsistent with its tier (Tier S cannot declare Tier H
hardening features; Tier H cannot declare Tier S elisions).

## 5. The Trust Profile

The mechanism that makes Tier S possible is the trust profile: a
signed artifact recording observations about the code over its
operational life.

### 5.1 Profile Contents

Per check site, the profile records:

  - **Reach count.** Number of times this check was reached.
    Indicates exposure.
  - **Trigger count.** Number of times this check would have failed.
    Must be zero for elision eligibility.
  - **Input distribution.** Summary statistics about the inputs that
    reached this check, including ranges, distributions, and
    coverage of the documented input space.
  - **Coverage attribution.** Which tests, fuzz runs, and production
    deployments contributed to this site's data.
  - **Observation window.** Start and end timestamps of the data
    collection period.
  - **Platform context.** Architecture, OS, CPU generation,
    microcode version where relevant.
  - **Signatures.** Cryptographic signatures from the parties who
    observed each datum, supporting independent verification.

The profile is itself a typed Sentinel data structure, not an opaque
binary blob. It can be inspected, queried, and audited.

### 5.2 Profile Aggregation

The trust profile is cumulative across deployments. Production
telemetry from version 1.4 feeds the trust calculation for version 1.5,
provided the relevant code did not change.

The compiler tracks which code paths changed between versions through
content-addressed identifiers derived from the typed HIR. Profile data
for unchanged paths carries forward; profile data for changed paths is
invalidated and must be re-earned.

Aggregation respects cryptographic provenance: data from a deployment
signed by key K is attributed to that deployment, and the promotion
review can require minimum diversity of contributing keys (no single
deployment can authorize Tier S promotion alone).

### 5.3 Profile Integrity

The trust profile is the security-critical input to Tier S
compilation. Its integrity must be defended.

Profile data is signed at collection time, before transmission to any
aggregation infrastructure. The signing identity is the broker on the
deployment producing the data; the broker's identity is in turn
attested through the platform attestation mechanism (BACKLOG2.md
Section 2.7).

Transmitted data is signed end-to-end; aggregation infrastructure
cannot modify data without invalidating signatures. Aggregation
produces a merged profile signed by the aggregator, but each
contributing datum retains its original signature for audit.

The promotion review process verifies the full signature chain. A
promotion based on improperly signed profile data is rejected at the
build stage.

### 5.4 Profile Schema Evolution

The profile schema evolves over the language's lifetime. Old profiles
can be read by new compilers (with profile fields the new compiler
does not understand treated as advisory only). New profiles cannot be
read by old compilers. Schema versions are part of the profile and
verified during promotion.

## 6. Promotion Workflow

Code starts at Tier E. Promotion to Tier S is a deliberate,
human-signed event with defined criteria.

### 6.1 Promotion Criteria

The default criteria for promotion are:

  - **Observation window.** Minimum six months of operational
    deployment at Tier E (project-configurable, with a minimum the
    language enforces).
  - **Coverage thresholds.** Every check site that is a candidate for
    elision must have reach count exceeding a minimum threshold
    (default one million for low-traffic sites, ten million for
    typical sites, one billion for hot paths). All reach must be
    with trigger count zero.
  - **Fuzzing coverage.** Fuzz harnesses must have exercised the
    code paths in question across their documented input space, with
    coverage measured against the adversarial input distribution as
    well as typical inputs.
  - **Test coverage.** Unit and integration tests covering the
    elided check sites must pass at the version being promoted.
  - **No outstanding advisories.** No public or known-internal
    security advisory affects the code paths in question.
  - **Reviewer signatures.** A defined set of designated reviewers
    have inspected the trust profile and the proposed elisions and
    signed a promotion attestation.

Projects can tighten these criteria but cannot weaken the language-
level minimums.

### 6.2 The Promotion Review

The promotion review is a human process supported by tooling. The
`snc promote` command produces a review package containing:

  - The current trust profile for the module being promoted.
  - The list of check sites proposed for elision and the data
    supporting each elision.
  - A diff against the previous tier assignment showing what changes
    in runtime behavior.
  - The fuzzing and test coverage reports for the affected paths.
  - Any outstanding warnings, anomalies, or borderline cases.

Reviewers examine the package and either approve, request changes, or
reject. Approval produces a signed promotion attestation that becomes
input to the build. Rejection is recorded with reasoning for future
reference.

The review can be split among multiple reviewers with different
roles: a performance reviewer focused on whether the elisions deliver
meaningful value, a security reviewer focused on whether the
elisions create attack surface, and an operations reviewer focused
on whether the observation window represents realistic deployment
conditions. The promotion attestation can require signatures from
each role.

### 6.3 Promotion Granularity

Promotion is per check site, not per function or per module.
A function may have ten bounds checks, of which six have earned
elision and four have not. The compiler emits the function with the
six elided and four retained. The granularity matters because real
code has rare branches that may not accumulate sufficient observation
even after long deployment.

### 6.4 The Built Artifact

A Tier S binary contains, alongside the executable code, a manifest
listing every elision: which check site, what type of check, the
profile data that justified elision, the promotion attestation
signatures. The manifest is signed as part of the build's overall
signature.

Security review of a Tier S binary can inspect the manifest to
understand exactly what runtime checks have been removed. The
manifest is human-readable through standard tooling and machine-
processable for automated audit.

## 7. Demotion Workflow

Demotion is the inverse of promotion: returning code to a lower tier
because circumstances have invalidated the trust that supported the
promotion.

### 7.1 Triggers for Demotion

Demotion can be triggered by:

  - **Discovery of vulnerability.** A security advisory affecting
    code paths that were elided in a Tier S binary triggers
    immediate demotion of those paths.
  - **Discovery of new attack surface.** Research that demonstrates
    a previously-unknown class of attack affecting elided checks
    triggers demotion across all affected codebases.
  - **Profile integrity failure.** Discovery that profile data
    supporting a promotion was forged, corrupted, or based on
    inadequate observation triggers demotion.
  - **Voluntary demotion.** A project can demote at any time as a
    conservative response to operational changes, threat-model
    updates, or organizational decisions.

### 7.2 Fast-Path Demotion

Demotion latency is itself a security concern. The window between
demotion trigger and demoted binary deployment is a window of
exposure to the vulnerability that triggered the demotion.

The infrastructure must support fast-path demotion:

  - Pre-built Tier E variants of every Tier S binary, refreshed
    automatically when source changes, available for immediate
    deployment.
  - Traffic-shifting mechanisms (load balancer integration,
    feature flags) that can move production traffic to Tier E
    binaries within minutes.
  - Standing arrangements with deployment infrastructure to make
    emergency rollback an O(minutes) operation, not O(days).

The fast-path infrastructure is part of operational maturity, not
just the language. Sentinel provides the binary variants and the
manifest information; operators provide the deployment plumbing.

### 7.3 Profile Invalidation

When demotion occurs, the profile data supporting the demoted
elisions is invalidated. The data is not deleted (audit may require
it later) but is marked as superseded. Future promotion attempts
cannot use invalidated data.

If the underlying issue is corrected and re-promotion is desired,
fresh observation must be accumulated. The promotion workflow runs
again from scratch.

### 7.4 Communication

Demotion is a communication event as well as a technical one.
Downstream consumers of a demoted binary need to know that the binary
they are running has had elisions reversed and may now perform
differently. The signature infrastructure carries this information:
demoted binaries have updated signatures, and downstream verification
detects the change automatically.

For supply-chain-published packages, demotion produces a security
advisory through the same channel as other security advisories, with
recommended remediation (typically updating to the demoted version).

## 8. Integration with Existing Features

### 8.1 Signatures and Provenance

Trust profiles are signed using the same infrastructure as code
signatures (BACKLOG2.md Section 2). Signing keys for profiles can be
the same as code-signing keys or can be separate. The capability-
bounded trust model extends naturally: a signing key authorized to
sign code may not be authorized to sign Tier S promotions, requiring
a separate security reviewer's signature.

### 8.2 The Broker

The broker is the source of profile data. Its recording mode
(SENTINEL_DESIGN2.md Section 5.7) captures the events that feed the
trust profile. The broker also enforces elisions at runtime: a
binary's manifest tells the broker which checks have been elided, and
the broker omits the corresponding bookkeeping operations.

The broker's policy for `secret` data is invariant across tiers. Tier
S optimizations apply only to memory-safety checks; cryptographic and
side-channel operations remain at full strength.

### 8.3 The Effect System

The effect system is the static safety net that runtime elision
relies on. Code at Tier S that suddenly needs to perform an operation
outside its observed effect set fails closed: the elided checks do
not matter because the operation itself is refused by the effect
system. This composition is critical to the security model. The
elisions are safe only because the effect system bounds what the
elided code can do.

### 8.4 Reproducible Builds

Tier S builds must be reproducible: given the same source and the
same trust profile, two builders produce byte-identical binaries. The
trust profile is part of the build inputs that get attested
(BACKLOG2.md Section 2.7).

Multi-party reproducible builds extend naturally: independent
builders fed the same source and the same profile must produce the
same Tier S binary, providing the cross-verification that supports
high-trust deployments.

### 8.5 Secrets Lifecycle

The lifecycle work (SECRETS_LIFECYCLE.md) interacts with tiered
releases at the audit boundary. Tier H deployments enable full
lifecycle audit logging by default; Tier S deployments may reduce
lifecycle logging to summary form for performance, subject to
project-defined compliance requirements. The interaction must be
specified per project; the language enforces consistency rather than
mandating a particular tradeoff.

## 9. Examples

### 9.1 A Mature Parser Promoted to Tier S

A JSON parser that has been in production for two years across many
deployments. The trust profile shows ten billion reach events on the
core parsing loops with zero triggers, full fuzz coverage including
adversarial corpora from the OSS-Fuzz project, and unanimous test
suite passing across versions.

The promotion review approves elision of bounds checks in the inner
parsing loop. The Tier S binary's manifest records each elision. The
runtime performance approaches hand-written C, with the safety
guarantees preserved by the effect system (the parser cannot, by
its declared effects, do anything that would matter if the elided
checks were wrong).

### 9.2 A Cryptographic Module Pinned at Tier H

A cryptographic primitive library. Despite mature deployment, the
project pins the module at Tier H because the threat model includes
adversarial inputs and side-channel attackers.

Tier H adds memory poisoning, anti-tampering checks, lifecycle audit
logging on every key operation, and side-channel information budget
enforcement. Performance is 10% lower than Tier E for this module,
which is acceptable given the security gain.

### 9.3 An Emergency Demotion

A vulnerability is reported in the parser's handling of deeply nested
inputs: a path that had elided depth checking can be triggered to
allocate excessive memory under adversarial input. The vulnerability
was missed by fuzzing because the adversarial input distribution did
not include sufficient nesting depths.

The project responds with immediate demotion of the affected paths
back to Tier E. The pre-built Tier E variant is deployed within
fifteen minutes through the fast-path infrastructure. The profile
data supporting the demoted elisions is invalidated. A security
advisory is published. Re-promotion will require fresh observation
including the now-expanded adversarial input distribution.

### 9.4 A New Feature at Tier E

A new feature is added to the parser. Its code paths have no trust
profile yet. The build assigns the new module to Tier E by default.
The rest of the parser remains at Tier S. The binary contains a mix:
elided checks where profile data justifies elision, full checks where
profile data is absent. The mix is documented in the binary's
manifest.

Over the next six months, the new feature accumulates observation.
After meeting the promotion criteria, it can be promoted to Tier S in
its own right. Until then, it pays the small Tier E overhead while
the rest of the codebase runs at Tier S speed.

## 10. Hard Problems

I want to flag the real difficulties rather than pretend the design
is clean.

### 10.1 Trust Profile Poisoning

If an attacker can influence the profile data, they can manipulate
which checks get elided. Threat-model defenses:

  - Profile data is signed at collection by the broker, whose
    identity is hardware-attested.
  - Aggregation infrastructure cannot modify data without
    invalidating signatures.
  - Promotion requires minimum diversity of contributing
    deployments, so a single compromised deployment cannot
    authorize promotion.
  - The promotion review is an adversarial process: reviewers
    actively look for ways the profile data could have been
    manipulated.

These defenses raise the bar significantly but do not eliminate the
risk. A sophisticated attacker with deep access could still influence
the data. The model trades absolute security for graduated trust;
this tradeoff must be communicated honestly.

### 10.2 Statistical Validity

"Reached ten billion times with zero triggers" sounds like strong
evidence, but it depends on whether those ten billion invocations
covered the input space an attacker might explore. A bounds check
never tripped because production input is benign may trip
immediately on adversarial input.

The model requires fuzzing coverage as a promotion criterion, with
coverage measured against an adversarial input distribution. This is
research territory: defining "adversarial input distribution"
precisely is hard, and proving fuzzing has adequately covered it is
harder.

Phase T4 of the implementation (Section 11) treats this as research
rather than committing to a particular formulation. The early phases
support the infrastructure; the statistical-validity work happens
incrementally as the field develops.

### 10.3 Cognitive Load

A four-tier system is conceptually more complex than the standard
debug/release split. Developers must understand which tier their
code is in, what the implications are, and what promotion requires.
Documentation, tooling, and defaults need careful design to keep
this manageable.

Defaults help. New code starts at Tier E; staying at Tier E requires
no special action. Promotion is opt-in and requires deliberate
effort. Most developers never need to think about Tier S unless they
are explicitly working on mature performance-critical code.

Tooling helps. The build output reports the tier assignment of every
module. The LSP shows the current tier inline. Errors and warnings
reference the tier when relevant. The cognitive load is real but
manageable with good tooling discipline.

### 10.4 Auditing the Elided Checks

Security reviewers need to understand which checks have been elided
and why. The binary manifest carries this information, but it
expands the audit surface significantly. A Tier S binary with 10,000
elisions is harder to audit than a Tier E binary with none.

The mitigation is that audits do not have to be exhaustive: tooling
can summarize elisions by category, flag elisions that affect
security-relevant code paths, and prioritize review based on risk.
The manifest is structured to support this analysis.

### 10.5 Profile Portability

Profile data collected on one platform may not apply to another.
ARM and x86 deployments may need separate profiles. Even within x86,
microcode versions and CPU generations affect what paths are
exercised. The profile schema captures platform context, and
promotions are platform-scoped.

In practice this means a project deployed across multiple platforms
needs to accumulate profiles separately for each. The infrastructure
supports this but the operational complexity is real.

## 11. Implementation Phases

The full vision is substantial work. Phasing keeps each step
independently valuable.

### Phase T1 — Tier Discipline (1.0)

Add Tier D and Tier E as the two production tiers, with the existing
implicit "release" tier becoming Tier E explicitly. This is mostly
renaming, but it establishes the discipline that production code
carries full runtime checks. Tier D enables recording mode by default.

Belongs in 1.0 because it shapes how all subsequent work fits.

### Phase T2 — Hardened Releases (post-1.0)

Add Tier H, building on the runtime integrity features from
BACKLOG2.md Section 3. Straightforward extension: more checks, not
fewer. Immediate value to constituencies that have asked for
hardening modes.

Probably the second item promoted from BACKLOG2.md after the
signature infrastructure stabilizes.

### Phase T3 — Trust Profile Infrastructure (post-1.0)

Build the trust profile data structures, signature integration,
aggregation tooling, and profile inspection tools. Tier S exists as a
target but only with manually-curated profiles (security reviewers
explicitly mark check sites for elision based on their own analysis).

This validates the infrastructure without requiring the statistical-
validity problem to be solved. Manual curation is operationally
plausible for security-critical libraries where a small number of
hot paths justify careful review.

### Phase T4 — Automatic Profile Generation (research)

Add automatic profile generation from production telemetry, with the
statistical-validity work as a research effort. This is the full
vision and is genuinely difficult. Treating it as research means it
can be deferred until the foundations are solid.

Substantial collaboration with the fuzzing, formal-methods, and
operational-security communities is appropriate at this phase.

## 12. Open Questions

The minimum observation window for promotion is set at six months in
this draft. Real-world calibration may suggest different defaults.
Empirical study is needed.

The minimum reach-count thresholds (one million, ten million, one
billion for low-traffic, typical, and hot paths) are intuitions
without empirical backing. Calibration against real codebases is
needed before these become defaults.

The relationship between profile aggregation and federated trust is
open. If multiple organizations contribute profile data to the same
library, how is that data combined? How is the trust relationship
established? This may require its own design effort.

The interaction with the editions system (BACKLOG2.md Section 13.1)
needs specification. A language edition change may invalidate
profile data wholesale; the rules for when and how need definition.

The exact format of the binary manifest for elisions needs
specification. The format must be human-readable, machine-
processable, signature-compatible, and stable enough to evolve. This
is straightforward design work but needs to happen before Phase T3
ships.

## 13. Summary

The tiered release model converts Sentinel's runtime overhead from a
uniform cost into a graduated investment that can be reduced as code
earns trust through observation. Four tiers (Development,
Experimental, Stable, Hardened) cover the spectrum from maximum
instrumentation to selective elision to additional hardening.

A signed trust profile records what was observed about the code over
its operational life. A deliberate, human-signed promotion workflow
moves code from Tier E to Tier S based on observation, coverage, and
review. A defined demotion workflow handles the inevitable case
where promoted code turns out to need its checks back.

The model composes with Sentinel's existing infrastructure:
signatures provide profile integrity, the broker generates profile
data and enforces elisions, the effect system provides the static
safety net that makes runtime elision safe. Cryptographic correctness
and side-channel hardening are never elided regardless of tier.

This is one of the strongest distinguishing features Sentinel could
offer. No production language combines runtime observation, signed
trust profiles, and tier-based safety check elision in this way. The
combination addresses the central honest cost of Sentinel — runtime
overhead compared to Rust — in a way that sacrifices nothing in code
that has not earned trust while rewarding code that has.

The work is substantial. The phasing keeps each step independently
valuable. Even Phase T1 alone, which mostly establishes naming
discipline, prepares the ground for everything that follows.

*End of document.*
