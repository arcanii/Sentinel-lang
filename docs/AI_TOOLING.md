# AI_TOOLING.md — Sentinel and AI Code Assistance

This document covers how Sentinel can be designed to work well with AI
code-assistance tooling without compromising its security thesis or
chasing trends that may age badly. The position taken throughout is
deliberately measured: features that help AI are mostly the same
features that help human reasoning, and Sentinel should be ruthless
about both rather than designing AI-specific machinery.

The document is a design reference rather than a roadmap. Its purpose
is to ensure that decisions made for other reasons (security, clarity,
tooling) accidentally accommodate AI assistance well, and that the
small number of AI-specific enhancements worth making are identified
and prioritized.

## 1. Framing

AI assistance is genuinely changing how code gets written. A language
designed in 2026 that ignores this is missing a real shift in its
development context. But the field is moving fast enough that
confident predictions about what helps AI today may be wrong in two
years. A language designed primarily for current AI capabilities
risks being optimized for a moving target.

The defensible bet is that structured information in canonical forms
is good for AI for the same reason it is good for humans, and
Sentinel should be ruthless about both. Effects in signatures, errors
as values, regions made explicit, secrets typed, signatures bounded
— these were designed for human reasoning and security, and they
happen to be exactly what makes AI assistance work well. Doubling
down on these is safer than chasing AI-specific features.

The strategic position: Sentinel is designed for security and
clarity, and AI tooling works well on it as a natural consequence.
The features that help AI are the features that help humans too. The
language should not be branded as "AI-first" or "designed for AI" —
that positioning ages badly and signals to skeptics that the language
is chasing trends.

## 2. Modes of AI Assistance

AI code assistance falls into several modes with different needs.
Worth defining them before designing for them, because a feature that
helps one mode may hurt another.

**Completion** predicts the next tokens given context. It benefits
from regular syntax, predictable patterns, and unambiguous keywords.

**Generation from intent** produces code from natural-language
description. It benefits from semantic primitives that map cleanly to
common task descriptions, so that "read a file safely with a timeout"
has a recognizable canonical form.

**Review and verification** checks that code does what was claimed.
It benefits from explicit types, declared effects, traceable
provenance — the same things human reviewers benefit from.

**Refactoring** changes code while preserving behavior. It benefits
from local reasoning, explicit dependencies, and unambiguous scope.

**Repair** fixes broken code. It benefits from precise error
messages, clear failure modes, and a small set of canonical patterns.

These modes overlap but are not identical. Sentinel cannot optimize
for all of them; it can be designed to not actively fight AI
assistance, which is a more achievable bar.

## 3. Sentinel Features That Already Help

Several aspects of Sentinel's existing design happen to be
AI-friendly. This is not coincidence: features that surface
structured information to human reviewers also surface it to AI
consumers.

### 3.1 Explicit Type Qualifiers

When a function declares `uses io, network, throw -> Response`, an
AI assistant has reliable information about what the function does
without needing to read its body or guess from naming conventions.
This is the same information that helps human reviewers, and it is
what current languages mostly lack: Python and JavaScript provide
almost no signal, Java provides type signatures but not effects,
Rust provides types but not effects.

Sentinel's effect system is a structured channel of information that
AI can consume reliably. The implication: maximize what is
expressible in the signature, minimize what requires reading the
body. Every property a function commits to (region, effects,
secrecy, lifetime, side-channel behavior) should be in the signature
where AI can see it without inference.

### 3.2 Errors as Values

Algebraic error types via the effect system mean errors are typed
values that flow through code in declared ways. This is much easier
for AI to reason about than exception-based code, where any function
call might transfer control somewhere unexpected. Generation,
review, and repair all benefit. This is already in the design and
worth keeping prominent.

### 3.3 Effect Declarations as Documentation

When `uses network<host: "api.x.com">` appears in a signature, it
is both a security boundary and a piece of structured documentation.
AI generating code that calls this function knows it makes network
calls to a specific host. AI reviewing code can verify the call site
matches the constraint. This is documentation that cannot go stale
because the compiler enforces it.

### 3.4 Canonical Forms

Languages where there are five ways to do the same thing are harder
for AI to generate code in because the training data is split across
the forms. Languages with one obvious way to do each thing produce
more consistent generated code and are easier to verify.

Sentinel commits to canonical forms ruthlessly: one way to declare a
variable, one way to handle errors, one way to define a class, one
way to express asynchrony. This is a real cost (some developers want
flexibility) but the payoff is consistency that benefits both AI and
human readers.

### 3.5 The Compiler as Library

The compiler is built as a salsa-style query engine exposed as a
public library API (HANDOVER.md Section 6.3). An AI assistant that
can ask the compiler "what effects does this function have?", "what
implementations of this trait exist?", "what calls this function?",
"what would this type check as?" can produce dramatically better
results than one limited to text completion.

The compiler-as-library design already serves human tooling needs.
Making it explicit that the library is intended for AI consumption
alongside human tooling is a small but meaningful framing.

## 4. Additions Worth Considering

A few features that would specifically help AI assistance and that
are not in the current design.

### 4.1 Structured Documentation Comments

Doc comments should have a defined schema — sections for parameters,
return values, effects, preconditions, postconditions, examples,
security notes — rather than free-form text. Rust's doc comments are
good but unstructured; Java's Javadoc has structure but is verbose.
Sentinel could specify a Markdown-derived structured format that AI
can parse reliably for context and that humans can read naturally.

Example:

    /// Authenticates a user with the given credentials.
    ///
    /// effects:   network, crypto, throw
    /// security:  requires constant-time credential comparison
    /// errors:    AuthError::Rejected if credentials are invalid
    ///            NetworkError if the auth service is unreachable
    /// example:
    ///     let session = authenticate(&creds).await?;
    fn authenticate(creds: &Credentials)
        uses network, crypto, throw -> Session { ... }

The structure makes AI consumption reliable without making the
documentation worse for humans.

### 4.2 Test Cases as Part of the Function

Rust has doc tests, which run code in documentation. Sentinel could
make this more first-class: a function carries test cases directly
in its signature block, executed by the test runner, used as
examples by tooling, and providing AI with a concrete behavioral
specification.

    fn parse_email(input: &str) -> ?Email
        examples {
            parse_email("alice@example.com") == some(Email { ... });
            parse_email("not-an-email")      == none;
            parse_email("")                  == none;
        }
    { ... }

The examples are real code that runs, validates, and documents.
They are also a structured signal AI can use for generating callers,
suggesting edge cases, or verifying alternative implementations.

### 4.3 Intent Annotations

Some research languages experiment with comments that describe what
a function should do at a higher level than its implementation.
Sentinel could adopt a lightweight version: a one-line intent
statement that AI uses for high-level reasoning.

    @intent("Validate that an email address is well-formed per RFC 5322")
    fn parse_email(input: &str) -> ?Email { ... }

This is gentle convention rather than language mandate, but if the
convention is established early, AI assistants can rely on it.

### 4.4 Refactoring-Safe Identifiers

When AI refactors code, renaming variables, functions, and types is
constant. Sentinel's stable ABI and explicit module exports help
because the rename can be scoped precisely. A small additional
feature would help more: every declaration could implicitly carry a
stable identity that survives renames, so refactoring tools can
track what was renamed to what across history.

This is the kind of feature that costs little but enables
sophisticated tooling. The identity is generated at first
declaration, preserved through renames, and surfaced through the
compiler-as-library API.

### 4.5 Semantic Patches

Some languages support structured diffs: patches that describe
semantic changes rather than line-by-line text. Sentinel could
specify a patch format from the start that expresses operations like
"rename function X to Y, change all callers to pass an additional
parameter, add an effect declaration" as semantic operations.

This is much easier for AI to generate correctly than textual diffs,
and it integrates with the signature infrastructure: semantic
patches can be signed, attributed, and reviewed at the semantic
level.

### 4.6 Compile-Time Code Generation

When AI generates boilerplate (serialization, builders, equality,
hashing), the language should support compile-time evaluation rich
enough that the boilerplate can be derived rather than emitted.
Rust's derive macros are an example; Sentinel could be more
ambitious.

This reduces both the volume of AI-generated code and the surface
area for bugs. The AI generates the declarative trigger (the derive
annotation); the compiler generates the implementation deterministically.

### 4.7 Conversation Context in the Build System

A small but useful addition: the build system records, optionally,
which code regions were authored by AI assistance and which were
human-authored, along with the prompts and context that produced the
AI sections. This is metadata, not enforcement: it does not change
what the code does, but it provides traceability for security
review.

A function whose body was generated by AI based on a particular
prompt can be audited differently from one written by hand. The
audit need not be more or less stringent; it just has different
relevant questions.

### 4.8 Type-Driven Stub Generation

The compiler can produce stub implementations from signatures alone:
given a function declaration with types, effects, preconditions, and
examples, generate a stub body that satisfies the signature even if
trivially (returning a default value, raising "not implemented",
etc.). This is useful for AI workflows where the signature is the
specification and the implementation is generated separately.

The stub is type-correct by construction; the AI fills in the
behavior. This separation makes reviewing AI-generated code easier
because the type contract is established before the body is
considered.

## 5. The Broker's Recording Mode as Training Signal

This is more speculative but worth mentioning. The broker captures
structured runtime traces (SENTINEL_DESIGN2.md Section 5.7).
Aggregated across many programs with appropriate privacy
protections, these traces are a richer training signal than source
code alone — they show what programs do, not just what they look
like.

A future where Sentinel's runtime data trains better Sentinel-
specific AI is plausible. The current design does not preclude this;
it might even enable it. Two design choices keep this option open:

  - The recording format is structured and self-describing, so it
    can be processed by tools without requiring source.
  - The recording includes effect-level information, so it can be
    correlated with declared behavior rather than just observed
    behavior.

This is not a commitment to ship AI training infrastructure. It is
a commitment to not preclude it through short-sighted design
choices.

## 6. What Not to Do

Some features that sound AI-friendly are actually counterproductive.

### 6.1 Do Not Make the Language Verbose for AI Benefit

Verbose languages do not help AI; they just take more tokens. Java
is verbose and AI handles it fine because the patterns are regular,
not because verbose is good for AI. Conciseness with regularity is
what helps; ceremony for its own sake does not.

### 6.2 Do Not Constrain Expressiveness for AI Convenience

A language stripped of features so AI can handle it better is a
worse language for humans too. AI capabilities are improving
rapidly; designing around current limitations is a losing strategy.
Aim for clarity of semantics, not simplicity of syntax.

### 6.3 Do Not Bet on Specific AI Architectures

A feature that helps current transformer-based code models
specifically may not help whatever comes next. Stay at the level of
"structured information in regular forms" rather than "designed for
sequence prediction" or any other architecture-specific assumption.

### 6.4 Do Not Sacrifice Security for AI Generation Ease

This is the most important constraint. Sentinel exists to make code
more secure. If a feature makes code easier to generate but harder
to verify, harder to audit, or harder to constrain, it cuts against
the language's purpose. The bar for AI-friendly features is that
they help generation without weakening security properties —
ideally, they help both.

### 6.5 Do Not Brand the Language as "AI-First"

The positioning ages badly and signals to skeptics that the language
is chasing trends. The right framing is that Sentinel is designed
for security and clarity, and that AI tooling works well on it as a
natural consequence. The features that help AI are the features that
help humans — which is what they should be.

## 7. Security Considerations for AI-Generated Code

AI-generated code introduces specific security risks that Sentinel's
infrastructure is well-positioned to address.

### 7.1 Hallucinated Dependencies

AI assistants sometimes invent packages that do not exist (or worse,
that exist and are malicious). Sentinel's signature infrastructure
(BACKLOG2.md Section 2) catches this naturally: a dependency that is
not in the trust manifest fails to compile. AI cannot introduce a
package by guessing its name; the package must be explicitly
authorized.

### 7.2 Hallucinated APIs

AI sometimes calls functions that do not exist with parameters that
do not match. Sentinel's type system catches this at compile time.
The cost of hallucinated APIs is reduced from "production bug" to
"failed build."

### 7.3 Insecure-by-Default Patterns

AI trained on the wider ecosystem may reproduce insecure patterns
(string-concatenated SQL, weak random number generation, plaintext
secret handling). Sentinel's secure defaults (BACKLOG2.md
Section 12, DESIGN_IDEAS.md Section 12) make these patterns harder
to write, so AI is less likely to produce them and the cost is
higher when it does.

### 7.4 Capability Escalation

AI generating code that suddenly needs new effects (network calls
in a parser, file access in a serializer) triggers compile-time
errors because the effect declarations propagate. AI cannot quietly
introduce new capabilities; the effect system makes them visible.

### 7.5 Plausible-Looking Wrong Code

The hardest case: AI generates code that compiles, type-checks,
matches declared effects, and is wrong in some subtle behavioral
way. Sentinel's pre/post conditions, refinement types, and
property-based testing (BACKLOG2.md Section 8) catch some of this
at the boundaries; integrated fuzzing catches more. But this is the
class of bug where the language cannot fully protect against AI
mistakes, and human review remains essential.

The honest position: Sentinel reduces but does not eliminate the
risks of AI-generated code. The features that catch AI mistakes are
the same features that catch human mistakes. AI assistance does not
change the security model, but it does increase the volume of code
that needs to be vetted, which makes the language's static
guarantees more valuable in proportion to the AI velocity.

## 8. Tooling Vision

The full vision for Sentinel-aware AI tooling integrates several
features that already exist or are planned.

An AI assistant working on Sentinel code would:

  - Query the compiler-as-library for typed semantic information
    (types, effects, callers, callees, implementations) rather than
    inferring from text.
  - Read structured documentation (Section 4.1) and examples
    (Section 4.2) for behavioral context.
  - Use semantic patches (Section 4.5) to express changes rather
    than producing textual diffs.
  - Have its proposed changes type-checked, effect-checked, and
    signature-verified before presentation to the developer.
  - Have its contributions recorded with provenance (Section 4.7)
    for audit.
  - Be unable to introduce unsigned dependencies (Section 7.1).
  - Be unable to escalate capabilities without effect declarations
    (Section 7.4).
  - Run its generated code against fuzz harnesses and property
    tests integrated into the build (Section 7.5).

None of this requires AI-specific language features. It requires the
language and tooling to be built on structured semantic information
from the start. Sentinel's existing design supports it; the
additions in Section 4 enhance it; the security infrastructure in
Section 7 provides the safety net.

## 9. Implementation Priority

The AI-tooling considerations in this document do not require their
own phase of work. They are guidance for decisions made in other
contexts:

  - Section 3 features are already in the design and should be
    preserved as the language evolves.
  - Section 4 additions (structured docs, in-line examples, intent
    annotations, refactoring identifiers, semantic patches) are
    small enough to fold into 1.0 tooling work as low-priority
    items. None blocks 1.0.
  - Section 5 (broker recording as training signal) is a
    preservation-of-options decision, requiring only that recording
    formats stay structured. No active work.
  - Section 7 (security considerations) is already addressed by
    other security infrastructure; this document just makes the
    AI-specific implications explicit.
  - Section 8 (tooling vision) is achieved as a byproduct of the
    compiler-as-library design and the broader ecosystem maturing.

The net additional work specifically for AI tooling is modest. The
document exists to ensure that the modest work happens and that
larger temptations (designing AI-specific machinery, branding the
language as AI-first) are resisted.

## 10. Open Questions

The structured documentation schema (Section 4.1) needs concrete
specification. The candidate fields (parameters, returns, effects,
preconditions, postconditions, examples, security notes) are
reasonable but the exact format affects how AI tooling consumes it
and should be designed once rather than evolved.

The semantic patch format (Section 4.5) is an open research area.
Several language ecosystems have attempted versions of this; none
has converged on a standard. Sentinel could either adopt an existing
format (if one becomes credible) or specify its own (with the cost
of one more thing to maintain).

The interaction between AI-generated code provenance (Section 4.7)
and the signature infrastructure (BACKLOG2.md Section 2) needs
design. Specifically: can AI-generated code be signed by the
human who reviewed it, by the AI service that produced it, or both?
What does the trust model look like when a human signs code that an
AI wrote?

The training-signal possibility (Section 5) has significant privacy
implications. Recording mode produces detailed execution traces;
using them as training data requires either anonymization (which is
hard to do well) or opt-in (which limits the data). The privacy
design needs work before any training use becomes plausible.

## 11. Summary

Sentinel does not need to be designed for AI assistance to work well
with AI assistance. The features that make Sentinel a good language
for security and human reasoning — explicit types, declared effects,
errors as values, structured signatures, canonical forms, compiler
as library — are the features that make it a good language for AI
consumption. This is the right outcome: durable features serve
multiple constituencies.

The small set of AI-specific enhancements worth making (structured
documentation, in-line examples, intent annotations, refactoring
identifiers, semantic patches, optional generation provenance) are
inexpensive additions that compose with the existing design without
distorting it. They should be folded into 1.0 tooling work where
convenient and not allowed to delay other priorities.

What Sentinel should not do is brand itself as AI-first, design
AI-specific syntax or semantics, sacrifice expressiveness for
AI-consumption ease, or bet on the characteristics of any particular
AI architecture. These are temptations to resist.

The honest one-line summary: AI tooling will work well on Sentinel
because Sentinel is well-designed, not because Sentinel was designed
for AI tooling. The distinction matters, both for what we build and
for how we describe what we built.

*End of document.*
