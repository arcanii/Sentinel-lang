# DESIGN_IDEAS.md — Sentinel Forward-Looking Security Design Ideas

This document catalogs security-oriented design ideas for Sentinel that
are not yet committed to BACKLOG2.md but deserve consideration. The
ideas here range from "almost certainly worth doing" to "interesting but
probably wrong." Each entry is written to be evaluated on its merits,
not promoted.

The document's purpose is to capture thinking before it is lost. Ideas
move from here to BACKLOG2.md once they have a defensible shape and
clear value, or are removed once they have been considered and rejected.
The discipline of writing each idea down with its weaknesses is more
important than the discipline of pursuing them.

## Framing

Sentinel's purpose, restated: a flexible, high-security, high-safety
framework whose job is to eliminate as many unintentionally introduced
security bugs as the language layer can reach. Every idea below is
evaluated against the same test: does it make ordinary code more secure
by default, or does it only help when the programmer is already
thinking about security?

Ideas that only help careful programmers are de-prioritized. Ideas that
make insecure code fail to compile, fail to sign, or fail to run are
prioritized.

---

## 1. Secrets Lifecycle Tracking

**Problem.** Every language has libraries for handling secrets in
memory. None track the secret's *lifecycle*: when it was created, who
has touched it, when it should be rotated, when it should be destroyed,
what depends on it. Credential rotation is universally treated as
operational discipline, lives in spreadsheets and runbooks, and fails
predictably. The Equifax, Codecov, Heroku, and Travis incidents all
involved credentials that should have been rotated and were not.

**Shape.** Extend the `secret T` qualifier to carry lifecycle metadata
recorded by the broker: creation timestamp, source provenance, rotation
policy, dependents (other secrets derived from this one), and expected
destruction. The broker enforces rotation deadlines as typed runtime
errors and surfaces overdue rotations at build time.

**Fit.** Very close fit. Builds directly on existing `secret`
infrastructure and the broker's lifecycle tracking for memory.

**Hard parts.** The metadata schema must cover real-world rotation
patterns (some secrets rotate on a schedule, some on use, some on
revocation events) without becoming a configuration nightmare.
Cross-process and cross-restart persistence of lifecycle state needs
careful design.

**Assessment.** Strongest candidate in this document. Probably worth a
focused design document of its own; see `SECRETS_LIFECYCLE.md`.

---

## 2. Behavioral Attestation

**Problem.** Code signing tells you the bytes came from a signer. It
does not tell you the code behaves as expected. Two functionally
different versions can carry the same signature; bugs introduced
through legitimate development paths slip past signature-only
verification.

**Shape.** Functions carry signed behavioral profiles: declared bounds
on allocations, I/O operations, runtime, files touched, network
destinations contacted. The compiler verifies what it can statically;
the broker enforces what it cannot. Profiles are signed alongside the
binary; verification at load checks signature and at runtime checks
adherence.

**Fit.** Builds on the effect system (which declares what is possible)
by adding declarations of what is *typical*. Reuses signature
infrastructure from BACKLOG2.md Section 2.

**Hard parts.** Profiles drift under legitimate change. Tooling needs
to update profiles automatically during development and require
re-signing only for releases. False positives in production are
operationally expensive; the system must tolerate reasonable drift
without becoming meaningless.

**Assessment.** Genuinely novel. No production language has this. Worth
prototyping in a research mode before committing.

---

## 3. Permissioned Data Lineage

**Problem.** Regulations require knowing where personal data came from,
where it went, and what was done to it. Implementation is universally
through application-level logging that is inconsistent and incomplete.

**Shape.** When data carrying a flow label (user identity, classification
level, tenant ID) passes through a function, the runtime records the
transformation in a signed, tamper-evident audit log. The log is
queryable per-subject for compliance reporting and per-incident for
forensic response.

**Fit.** Builds on the flow-label work reserved in SENTINEL_DESIGN2.md
Section 11. Reuses broker recording infrastructure.

**Hard parts.** Performance overhead is real; lineage recording cannot
be free. Storage cost of lineage logs grows linearly with data
operations. Privacy implications of the lineage log itself need
thought (it contains information about every operation on every
user's data).

**Assessment.** High value in regulated domains, low value elsewhere.
Should be opt-in per flow-label rather than universal. Worth pursuing
once flow labels are implemented.

---

## 4. Time-Based Access Control Types

**Problem.** Validity windows for tokens, signed URLs, session
identifiers, and OAuth assertions are implemented as application logic.
Implementation is inconsistent, often incorrect, and a frequent source
of replay and reuse vulnerabilities.

**Shape.** A type `valid_during<R: TimeRange> Credential` that the
compiler refuses to use outside its window. The broker refuses to load
expired credentials. Signed assertions carry verifiable validity
windows enforced at the language level.

**Fit.** Natural extension of the type system. Composes with the
`secret` qualifier and with signature verification.

**Hard parts.** Clock skew is a real-world problem; the system needs
explicit tolerance windows. Distributed systems with disagreeing clocks
need a defined behavior. Long-lived credentials with sliding renewal
have complex state.

**Assessment.** Worth doing. Probably part of a broader credential-
handling effort alongside secrets lifecycle.

---

## 5. Cryptographic Agility as Language Concern

**Problem.** Cryptographic algorithm migrations (MD5 to SHA-1 to SHA-256,
RSA to ECC to post-quantum) are universally painful because algorithm
choices are baked into application code, file formats, and protocols
with no clean evolution path. The PQ transition over the next decade
will surface this as an acute problem industry-wide.

**Shape.** Cryptographic types carry algorithm identity as part of
their type. Format versioning is built into serialization. Tooling
identifies all cryptographic decision points in a codebase. Automatic
dual-write during migration windows is a standard pattern, not a custom
implementation per project.

**Fit.** Extension of the cryptographic type system reserved in
BACKLOG2.md Section 4.2.

**Hard parts.** Algorithm identity in the type system makes signatures
verbose. Migrations involve operational state (which keys have been
rotated, which protocols negotiated) that the language alone cannot
manage.

**Assessment.** Probably essential for the PQ transition. Should be
designed before that transition becomes urgent rather than during.

---

## 6. Authenticated Configuration

**Problem.** Configuration files are one of the largest unaddressed
attack surfaces. A compromised config changes program behavior
arbitrarily. Signature verification on configs is rare; the assumption
that "configuration is not code" leaves the gap that attackers exploit.

**Shape.** Configuration is treated as code: signed, versioned,
capability-bounded, verified at load. The same supply-chain rigor as
source code applies. Schema enforcement at load prevents structural
attacks. Capability declarations in config (this config is allowed to
enable network access; this config is not) extend the effect system to
runtime configuration.

**Fit.** Direct extension of the signature work in BACKLOG2.md
Section 2. The infrastructure already exists; this is mostly tooling.

**Hard parts.** Configuration is often modified by operators without
re-signing access. Workflows for legitimate operational change must
remain practical. Emergency configuration changes during incidents
need a defined process.

**Assessment.** Worth doing. Relatively low-effort given the existing
signature infrastructure. High value because the attack surface is real
and currently unaddressed.

---

## 7. Trustworthy Compilation in Untrusted Environments

**Problem.** CI/CD compromises (CircleCI, Travis, GitHub Actions
incidents) have demonstrated that trusting the build environment is
dangerous. The build is currently the largest attack surface most
organizations have, and the smallest one they verify.

**Shape.** Attestable remote compilation. A build runs in an attested
environment (TEE, signed container, reproducible deterministic
builder), produces a signed proof that the build was performed
correctly with declared inputs, and the proof is verifiable
independently of trusting the build infrastructure.

**Fit.** Combines reproducible builds (already committed in
SENTINEL_DESIGN2.md Section 13.1), TEE attestation (BACKLOG2.md
Section 4.3), and build environment attestation (BACKLOG2.md
Section 2.7) into a unified primitive.

**Hard parts.** TEE attestation is hardware-dependent and the hardware
trust roots are themselves controversial. Reproducible builds across
heterogeneous environments are hard. Caching and incremental builds
interact awkwardly with attestation.

**Assessment.** Genuinely important. Probably requires a separate
design effort once the simpler signature work is mature.

---

## 8. Resource Exhaustion Budgets

**Problem.** Denial-of-service through resource exhaustion (allocation
bombs, regex DoS, ZIP bombs, billion-laughs XML, hash collision floods)
is a category no current language addresses systematically. Sentinel's
memory budgets help with memory, but CPU time, file descriptors,
network bandwidth, and entropy are all exhaustible without typed
bounds.

**Shape.** Generalize the memory budget concept to all bounded
resources: `within cpu_budget(100ms)`, `within fd_budget(8)`, `within
bandwidth_budget(10.MiB/sec)`. The runtime tracks; the type system
enforces graceful degradation at the budget boundary.

**Fit.** Direct extension of existing broker budget infrastructure.

**Hard parts.** Some resources (CPU time) cannot be cleanly bounded
without preemption support that may not exist on all targets.
Composition of budgets across nested scopes needs careful semantics.

**Assessment.** Worth doing. Closes a real category of vulnerability
with relatively modest additional complexity.

---

## 9. Defense Against Developer Mistakes

**Problem.** A surprising fraction of real-world breaches come from
developer own-goals: secrets committed to git, tokens logged
accidentally, internal state serialized into responses, debug code in
production builds. These are not exploits but they account for
significant breach volume.

**Shape.** Language-level protections against the developer themselves.
The broker refuses to serialize `secret` data to disk in git-tracked
locations. The type system refuses to write `secret` data to the
default logging path. Debug-only code is statically separated and the
compiler refuses to link it in release builds. Internal types are
explicitly distinguished from API types and cannot be returned across
API boundaries.

**Fit.** Several small extensions of existing infrastructure rather
than one large feature.

**Hard parts.** Distinguishing legitimate "I really do want this in
logs" from accidental disclosure requires escape hatches that are hard
to make right. Too restrictive and developers fight the language; too
permissive and the protections are meaningless.

**Assessment.** High cumulative value, low individual feature cost.
Should be designed as a set of small additions rather than one large
project.

---

## 10. Machine-Readable Vulnerability Response

**Problem.** When a security bug is found in a dependency, response is
manual: humans read the advisory, humans update the version, humans
verify the fix. The window between disclosure and remediation is
measured in days for the best-run organizations and months for most.

**Shape.** Vulnerability advisories are language-aware: they identify
the specific functions affected, not just package versions. The build
tool reports "you call this vulnerable function at these locations" not
"you depend on this vulnerable package." Signed advisories from
maintainers integrate with the trust infrastructure.

**Fit.** Extends signature and SBOM infrastructure from BACKLOG2.md
Section 2.

**Hard parts.** Requires ecosystem-wide buy-in to a structured advisory
format. The OSV format is moving in this direction but is not yet
universal.

**Assessment.** Worth pursuing once the basic supply-chain infrastructure
is mature. Probably requires partnership with vulnerability database
operators.

---

## 11. Forensically Robust Incident Response

**Problem.** Security incidents happen even with the best prevention.
The forensic phase is universally painful because the necessary data is
scattered, incomplete, and partially destroyed by the incident itself.

**Shape.** Language-level support for incident response. Tamper-evident
audit logs that survive process compromise. Broker recordings archivable
and replayable by responders. Signed attestations of code running at
incident time. SBOM snapshots tied to specific running processes.

**Fit.** Builds on the recording mode (SENTINEL_DESIGN2.md Section
5.7), signatures (BACKLOG2.md Section 2), and SBOM (BACKLOG2.md
Section 2.9).

**Hard parts.** Tamper-evidence requires careful cryptographic design
to survive process compromise. Storage and replication of forensic
data have real costs.

**Assessment.** Important but probably not 1.0. Should be designed once
incident response patterns with Sentinel are observed in practice.

---

## 12. Better Defaults for the Boring Stuff

**Problem.** A surprising amount of security work is choosing the right
library because defaults are wrong. Python's `random` is not crypto-
secure but is the obvious choice. Java's `ObjectInputStream`
deserializes arbitrary classes. PHP's `==` for strings is variable-time.
Each instance seems minor; cumulatively they account for enormous
numbers of CVEs.

**Shape.** Sentinel's defaults are the secure choice. Random number
generation is cryptographic unless explicitly weakened. Serialization
is structural and authenticated. String comparison on `secret` data is
constant-time. Hash maps use randomized seeding by default. The wrong
choice is harder than the right choice, always.

**Fit.** Pervasive throughout the standard library design rather than a
single feature.

**Hard parts.** Performance-sensitive code sometimes legitimately needs
the fast-but-unsafe defaults. The escape hatches must exist and must
be visible.

**Assessment.** Essential. This is design discipline more than a
discrete feature, but it has to be enforced consistently throughout
1.0 development.

---

## 13. Per-Function Capability Granularity

**Problem.** Effects are declared per-function in 1.0. In practice,
many security failures come from functions that declare a broad effect
(say, `network`) but only need a narrow one (`network to api.x.com on
port 443`). The unused capability is attacker-accessible.

**Shape.** Effects support parameters: `network<host: "api.x.com",
port: 443>` rather than just `network`. The compiler tracks parameters;
the runtime enforces them. Combined with capability attenuation across
the dependency graph, this gives least-privilege at fine granularity.

**Fit.** Refinement of the existing effect system.

**Hard parts.** Effect parameters make type signatures verbose. The
type system needs to handle parameterized effect composition cleanly.
Inference must keep call-site annotation minimal.

**Assessment.** Probably worth doing. Should be designed alongside the
effect system rather than retrofitted.

---

## 14. Schema-Bound Data Types

**Problem.** Data parsed from untrusted input is the primary attack
surface for most applications. Type-level guarantees end at the parse
boundary; everything after is "string that we hope is a URL" or "byte
array that we hope is valid JSON." Most parsing-related vulnerabilities
exploit the gap between "we have bytes" and "we have validated data."

**Shape.** First-class schema types where parsing produces a value that
carries its schema in the type. `Json<UserProfile>` is structurally
different from `Json<RawValue>`; operations on the former are
type-checked against the schema. Schema migration is a typed operation,
not string-replacement.

**Fit.** Generalization of refinement types from BACKLOG2.md Section
8.2.

**Hard parts.** Schema language design is itself a research problem.
Performance of validation must remain reasonable. Integration with
streaming parsers needs care.

**Assessment.** Interesting but large. Probably a separate research
direction rather than a 1.0 commitment.

---

## 15. Anti-Patterns Made Visually Distinct

**Problem.** Some patterns are individually legitimate but
disproportionately associated with vulnerabilities: string concatenation
to build SQL, exec with shell expansion, dynamic dispatch with
attacker-controlled types, deserialization of arbitrary objects. The
patterns exist in safe forms but the safe forms are visually identical
to the unsafe ones.

**Shape.** The language and editor make dangerous patterns visually
distinct from safe ones. String concatenation building queries shows a
warning icon in the editor. Exec without explicit argument-array form
is highlighted. Deserialization without a target type is flagged. The
discipline is mechanical rather than relying on review.

**Fit.** Mostly tooling rather than language design. The LSP from 1.0
already has the necessary infrastructure.

**Hard parts.** False positives are expensive in terms of developer
trust. The signal must be precise.

**Assessment.** Worth doing as tooling polish, probably post-1.0.

---

## 16. Honest Limits (Things Not to Build)

Several plausible-sounding ideas have been considered and rejected.

**Code obfuscation, anti-debugging, and runtime self-modification**
belong to DRM, not security. A language marketing these as security
helps vendors fight users, not the other way around. Sentinel refuses
this category entirely.

**ML-based anomaly detection at the language level** imports the
unsolved problems of that field (false positives, adversarial inputs,
training-data dependencies) into the language runtime. Better to leave
to specialized tools that can be replaced when better ones arrive.

**"Zero trust" marketing** covers a real idea (every interaction
authenticates and authorizes explicitly) wrapped in slogans. The real
idea fits Sentinel naturally through capabilities and signatures; the
marketing version should be ignored to keep the design honest.

**Compliance-as-feature** is the failure mode where features exist to
satisfy auditors rather than attackers. Compliance is a byproduct of
good design, not a goal.

**Mandatory cloud connections** for any security feature. Every feature
in Sentinel must work without contacting any vendor's server.
Operational independence is a security property; phoning home is not.

---

## 17. Strategic Pattern

The pattern across the strongest ideas in this document is that they
treat security as a property of *operational practice embedded in the
language* rather than as a *separate concern delegated to libraries
and tooling*. Current ecosystems treat security as something added to
working systems. Sentinel's strategic position is to be the first
systems language built around the inversion: the language itself is
responsible for making secure code easy and insecure code hard.

Memory safety, effects, signatures, secrets lifecycle, authenticated
configuration, resource budgets, lineage tracking, and forensic
readiness are not independent features. They are facets of a single
claim: that operational security disciplines belong in the type system
and the runtime, not in the runbook.

Most language design treats that as overreach. Sentinel's pitch is that
the overreach is the point.

---

## 18. Process

Ideas in this document move to BACKLOG2.md once they have a defensible
shape, a clear value proposition, and identified foundational
requirements. Ideas that have been considered and rejected move to
`docs/decisions/` with reasoning recorded.

The document is revisited quarterly. The bar for inclusion is "worth
thinking about." The bar for promotion to backlog is "worth committing
to think about further." The bar for the roadmap is "worth committing
to ship."

New ideas are added freely. The document being large is not a problem;
the document being a graveyard of half-considered notions is. Quarterly
review prunes ruthlessly.

*End of document.*
