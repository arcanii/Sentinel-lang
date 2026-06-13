# SECRETS_LIFECYCLE.md — Sentinel Secrets Lifecycle Design

> **Status: VISION / design exploration — NOT on the implementation
> roadmap.** None of the lifecycle stages (origin, rotation, custody, L1–L4)
> described below are scheduled or being built. The **shipped** secret
> surface is exactly: the `secret T` type qualifier (mlock'd, no-core-dump,
> zeroed on free, kept out of default serialization) **plus** the
> constant-time verification pass that statically rejects a `secret` reaching
> a branch / memory index / divisor (see the README headline section for the
> precise property and its boundaries). This document is a detailed *what-if*
> for a possible future extension, retained for design continuity — not a
> commitment. When it and [`STATE.md`](STATE.md) disagree about what exists,
> STATE.md wins.

This document specifies a proposed extension of Sentinel's `secret`
qualifier to track full secrets lifecycle. It is a focused design
proposal, not a high-level survey. It is intended to be detailed enough
for an engineer to begin prototyping — *if* the extension is ever scheduled.

The 1.0 design (SENTINEL_DESIGN2.md Section 4.5) defines `secret T` as
a type qualifier with memory-handling guarantees: mlock'd, no-core-dump,
zeroed on free, forbidden from default serialization, constant-time
operations. This document extends that foundation to handle the
*operational lifecycle* of the secret: its origin, its rotation, its
dependents, and its retirement.

## 1. Problem Statement

Every system that handles credentials, keys, tokens, or other sensitive
material faces the same operational discipline: secrets must be created
from authorized sources, used within their validity windows, rotated
according to policy, and destroyed when no longer needed. This
discipline is universally treated as out-of-band operational work —
runbooks, spreadsheets, secret managers, and tribal knowledge.

The discipline fails predictably. Equifax, Codecov, Heroku, Travis CI,
and countless smaller incidents involved credentials that should have
been rotated but were not, or were rotated incompletely because the
operator did not know which downstream systems depended on the secret.
The pattern is consistent: the language and runtime know nothing about
the secret beyond its bytes; everything else lives in human memory.

What is missing is a language-level model of the secret's lifecycle,
enforced by the type system and the broker, surfaced through tooling,
and integrated with the supply-chain infrastructure already proposed
for Sentinel.

## 2. Goals

The lifecycle extension should provide the following guarantees:

  - A secret's origin is recorded at creation and cannot be forged.
  - A secret's rotation policy is declared in its type and enforced
    by the broker.
  - Overdue rotations are surfaced at build time when statically
    determinable and at runtime when not.
  - The set of dependents of a secret (other secrets derived from
    it, signed artifacts produced with it, configuration that
    embeds it) is tracked, so rotation has a complete blast-radius
    assessment.
  - A secret's expiration is enforced by the type system; expired
    secrets cannot be used.
  - Destruction of a secret cascades to its dependents in a
    well-defined order.
  - The full lifecycle history is queryable and signable for audit.

The extension should not require every secret to opt into every
feature. Simple secrets (a development API key for a local test)
should remain ergonomic. Production secrets with complex requirements
should get the full machinery without imposing it elsewhere.

## 3. Non-Goals

The lifecycle extension is not a secret manager. It does not store
secrets at rest, distribute them between machines, or replace tools
like Vault, AWS KMS, or 1Password. It is the language-level
*runtime* discipline that those tools deliver secrets *into*. The
boundary is: secret managers are responsible for getting secrets to
the process; Sentinel is responsible for their behavior while inside
the process.

The extension also does not attempt to verify that a declared rotation
policy is the *right* policy. If a developer declares that a
database password rotates every five years, the language enforces the
five-year window; it does not argue with the choice.

## 4. Type System Extensions

### 4.1 Lifecycle-Tagged Secrets

The existing `secret T` becomes the base case. Lifecycle-tagged
secrets carry additional information in their type:

    secret T                              // base case, no lifecycle
    secret T @lifecycle(L)                // L is a lifecycle policy
    secret T @lifecycle(L) @origin(O)     // with explicit origin

The `@lifecycle` parameter binds to a policy type that declares
rotation, expiration, and destruction semantics. The `@origin`
parameter binds to a provenance assertion describing where the secret
came from.

### 4.2 Lifecycle Policy Types

Policies are first-class types declared by the standard library or
user code:

    policy ShortLived {
        rotate_every: 1.hour;
        expire_after: 24.hours;
        on_expire:    Destroy;
    }

    policy LongLived {
        rotate_every: 90.days;
        expire_after: 1.year;
        on_expire:    RequireRotation;
        notify_before_expire: 30.days;
    }

    policy NoExpiration {
        rotate_every: never;
        expire_after: never;
    }

The compiler treats policies as types in their own right; a function
can be polymorphic over policy:

    fn store<T, P: Policy>(secret: secret T @lifecycle(P)) uses storage
        -> Handle<secret T @lifecycle(P)>;

### 4.3 Origin Provenance

Origin types record where a secret came from:

    origin EnvironmentVariable(name: str);
    origin Generated(by: Function, at: Timestamp);
    origin Derived(from: Vec<SecretId>, via: DerivationFunction);
    origin Loaded(from: VaultPath, at: Timestamp, by: Identity);

Origin is set at creation and cannot be modified. The broker records
it; tooling can query it; audit logs include it. The type system uses
origin to enforce constraints: a function might accept only secrets
with `origin: Loaded(from: VaultPath, ...)` to refuse environment-
variable-sourced secrets in production paths.

### 4.4 Validity Windows

Secrets carry validity windows enforced by the type system:

    secret Token @valid_from(t1) @valid_until(t2)

Operations on the secret are statically checked against compile-time-
known windows where possible and dynamically checked otherwise. An
expired secret has type `expired secret T`, which has only the
destruction operations available. The compiler refuses to use an
`expired secret T` as `secret T` without an explicit re-validation
step (which fails at runtime if validation is not possible).

## 5. The Broker's Role

The broker, already responsible for memory protection of `secret`
data, takes on additional responsibilities for lifecycle.

### 5.1 Registration

Every lifecycle-tagged secret registers with the broker at creation.
The registration includes origin, policy, validity window, and a
unique secret ID. The broker maintains the secret's lifecycle state in
a protected region of its own metadata.

### 5.2 Rotation Tracking

The broker tracks rotation deadlines per secret. A secret approaching
its rotation deadline raises a build-time warning (when statically
determinable from policy constants) and a runtime warning (when not).
A secret past its deadline raises a typed runtime error on next access,
configurable per policy: hard failure for high-security secrets, soft
warning for development secrets.

### 5.3 Dependency Tracking

When a secret is used to derive another secret (key derivation, signing
a token, encrypting a configuration value), the broker records the
derivation relationship. The relationship graph is queryable: "what
depends on this root key?" returns the full transitive set.

When a secret is rotated or destroyed, the broker can identify all
dependents and surface them. A planned rotation produces a report:
"rotating this secret will invalidate these 47 derived secrets and
require these 12 signing operations to be redone."

### 5.4 Destruction

Destruction is staged: a secret marked for destruction first enters a
`destroying` state in which it can still satisfy reads from existing
holders but cannot be used to derive new dependents. Dependents are
notified through the broker's event system. Once all dependents are
themselves resolved (rotated, destroyed, or explicitly waived),
destruction completes: the memory is zeroed, the registration is
removed, and the lifecycle event is recorded.

A forced destruction path exists for incident response: all dependents
are marked invalid immediately, holders receive errors on next access,
and the lifecycle log records the forced destruction with reason.

### 5.5 Persistence

Lifecycle state must survive process restarts for long-lived secrets.
The broker writes lifecycle metadata to a protected store (the file
system in development, a secret manager in production). The store is
authenticated; tampering with lifecycle metadata is detectable.

## 6. Tooling

### 6.1 Build-Time Reporting

The compiler produces a lifecycle report per build:

  - Secrets declared in this codebase with their policies.
  - Secrets approaching rotation deadlines (based on static
    information).
  - Secrets whose policies have changed since the last build.
  - Dependency graphs of secrets, where statically determinable.

The report is signed alongside the binary using the supply-chain
infrastructure from BACKLOG2.md Section 2.

### 6.2 Runtime Reporting

The broker exposes a lifecycle query API:

    let report = broker.lifecycle_report();
    for secret in report.overdue() {
        log.warn("secret {} is {} past rotation", secret.id, secret.overdue_by);
    }
    for secret in report.expiring_within(7.days) {
        log.info("secret {} expires in {}", secret.id, secret.expires_in);
    }

The API is itself effect-scoped: querying lifecycle data requires the
`lifecycle_query` effect, which is granted sparingly.

### 6.3 Rotation Workflows

A `snc rotate` tool integrates with the broker to perform staged
rotations:

  - Identify all dependents of the secret being rotated.
  - Generate the new secret value (or accept it from a secret
    manager).
  - Update dependents in dependency order.
  - Verify each update succeeded.
  - Mark the old secret for destruction.
  - Wait for the destruction grace period.
  - Complete destruction and record the audit event.

The workflow is interruptible and resumable: a partial rotation
records its progress so it can be completed after a failure.

### 6.4 Audit Output

The lifecycle log is queryable and exportable in formats suitable for
compliance reporting. Each event is signed; the log is tamper-evident.
Standard query patterns include "show all operations on secrets of
type X in the last 90 days" and "show the lineage of this specific
secret from creation to destruction."

## 7. Integration with Existing Sentinel Features

### 7.1 Effects

Lifecycle operations require effects. Reading a secret's lifecycle
metadata requires `lifecycle_query`. Rotating a secret requires
`lifecycle_rotate`. Forced destruction requires `lifecycle_destroy`.
These compose with the existing effect system; modules can be denied
these capabilities through standard effect masking.

### 7.2 Signatures and Capability-Bounded Trust

A signing key authorized to publish a package can be additionally
constrained by which secret lifecycle operations it can grant. A
package whose declared effects include `lifecycle_destroy` requires a
signing key authorized to grant that capability.

### 7.3 Information Flow

When information flow control is implemented, secrets carry flow
labels that interact with lifecycle. A secret labeled `phi` cannot be
destroyed without recording a `phi`-destruction event for compliance.
A secret derived from a `phi`-labeled source inherits the label and
its lifecycle constraints.

### 7.4 Cross-Process Sharing

Secrets shared across processes (using the `@shared` region) carry
their lifecycle across the boundary. The broker on each process
participates in the lifecycle protocol; rotation in one process
notifies holders in other processes. Forced destruction propagates
across the participating process set.

## 8. Examples

### 8.1 Database Password with Standard Rotation

    let db_password: secret str @lifecycle(LongLived) @origin(Loaded(
        from: vault::path("/prod/db/password"),
        at:   now(),
        by:   identity::current()
    )) = vault::load("/prod/db/password")?;

    let conn = db::connect(db_password)?;
    // 89 days later, build emits warning: rotation due in 1 day
    // 91 days later, runtime emits error if accessed

### 8.2 Short-Lived API Token

    let token: secret Token @lifecycle(ShortLived) = api::authenticate(
        credentials: load_credentials()
    )?;

    api::call(token, request)?;
    // 24 hours later, token has type `expired secret Token`
    // Attempts to use it produce a compile error if statically
    // detectable, runtime error otherwise

### 8.3 Derived Key with Dependency Tracking

    let master_key: secret [u8; 32] @lifecycle(LongLived) @origin(...) =
        kms::load_master_key()?;

    let session_key: secret [u8; 32] @lifecycle(ShortLived)
                                     @origin(Derived(
                                         from: vec![master_key.id()],
                                         via:  hkdf::derive
                                     )) = hkdf::derive(master_key, ctx)?;

    // The broker records that session_key depends on master_key.
    // Rotating master_key will identify session_key as a dependent.

### 8.4 Forced Destruction During Incident

    // Operator detects compromise; runs `snc rotate --force /prod/db/password`
    // The broker:
    //   1. Marks the password as destroying
    //   2. Identifies all current holders
    //   3. Sends invalidation notices to each holder
    //   4. On next access, holders receive `SecretDestroyed` errors
    //   5. Records the forced destruction in the audit log with reason

## 9. Implementation Considerations

### 9.1 Performance

Lifecycle tracking adds overhead per secret operation. The overhead
must remain small for common cases. The broker uses lock-free data
structures for the common-path operations (access, validity check) and
acquires locks only for state transitions (rotation, destruction).
Cached state in thread-local storage avoids broker round-trips for
read-heavy workloads.

### 9.2 Memory

Lifecycle metadata is small per secret (estimated 200-400 bytes
including origin and policy references) but scales linearly with the
number of secrets. For systems with millions of secrets (rare but real
in some token-management workloads), the metadata may need to live in
a dedicated store rather than in-process.

### 9.3 Persistence Backend

The lifecycle metadata persistence backend is pluggable. Default
implementations are provided for: in-memory (development), local file
(single-process production), and a network-attached service (multi-
process production). The interface is small enough that integrating
with existing secret managers (Vault, AWS KMS, GCP Secret Manager) as
backends is straightforward.

### 9.4 Bootstrap

The first secret in a system has no origin to point to. The bootstrap
case is handled by treating the platform's hardware-rooted identity
(TPM, Secure Enclave, HSM) as the implicit root origin. Secrets
loaded at process startup from this root have origin
`Bootstrap(hardware_id)`; their authenticity is rooted in the
platform attestation.

## 10. Open Questions

The interaction between forced destruction and structured concurrency
needs careful design. If a secret is forcibly destroyed while a
concurrent task is mid-operation with it, the task must observe the
destruction in a defined way (probably by receiving a typed error on
its next access). The cancellation semantics need to integrate with
the structured concurrency model.

The granularity of lifecycle policies is open. The examples above
treat policies as small named bundles, but real-world rotation
requirements vary widely; a policy framework rich enough to express
realistic constraints without becoming a programming language unto
itself needs careful design. The current sketch treats policies as
ordinary types with declared fields, but this may need to evolve.

The relationship to external secret managers needs definition. The
broker's persistence backend can delegate to external services, but
the trust relationship (does Sentinel trust the secret manager's
lifecycle records, or does Sentinel maintain its own?) shapes the
design substantially.

The audit log's privacy properties need analysis. The log records
every operation on every secret, which is itself sensitive data. The
log itself needs lifecycle management, and the recursion (lifecycle
log of the lifecycle log) needs a defined terminator.

## 11. Path to Implementation

This work depends on several 1.0 features being complete: the `secret`
qualifier, the broker, the effect system, and the signature
infrastructure. It is therefore post-1.0 work.

Within post-1.0, the implementation phases are:

**Phase L1**: Type system extensions and broker registration. The
`@lifecycle` and `@origin` qualifiers are added to the type system.
The broker tracks registered secrets but does not yet enforce
rotation or destruction. Tooling produces reports but takes no
actions. This phase validates the type system design without
operational risk.

**Phase L2**: Rotation and validity enforcement. The broker enforces
validity windows and rotation deadlines. Tooling supports rotation
workflows. Existing applications can opt into lifecycle tracking
incrementally.

**Phase L3**: Dependency tracking and cascading operations. The
broker records derivation relationships and supports cascading
rotation and destruction. Audit log infrastructure is added.

**Phase L4**: Cross-process and persistent lifecycle. Lifecycle state
persists across restarts. Cross-process secrets carry lifecycle
across the boundary. Integration with external secret managers as
persistence backends.

Each phase is independently valuable; later phases are not required
for earlier phases to be useful. Phase L1 alone provides build-time
visibility into secret usage that no current language offers.

## 12. Summary

Secrets lifecycle tracking extends Sentinel's existing `secret`
qualifier from a memory-protection concern to a full operational-
lifecycle concern. The extension adds typed policies for rotation and
expiration, recorded provenance for origin, tracked dependencies for
blast-radius assessment, and integrated tooling for audit and
rotation workflows.

The design fits Sentinel's existing architecture: the broker is
already the lifecycle authority for memory, and extending it to
secrets lifecycle is a natural generalization. The effect system
provides capability scoping. The signature infrastructure provides
audit-log integrity. The information-flow work, when complete, will
provide policy composition.

The extension addresses a real and consistent class of breach: secrets
that should have been rotated but were not, because the discipline
lived in spreadsheets rather than in the language. By moving the
discipline into the type system and the runtime, Sentinel removes the
operational gap that produces these incidents.

This is the kind of feature that justifies a security-first systems
language. It is not a small addition; it is a substantial body of work
spanning the type system, the broker, the tooling, and the operational
model. But it addresses a problem that has no good solution today, and
solving it well would be one of the strongest differentiating
contributions Sentinel could make.

*End of document.*
