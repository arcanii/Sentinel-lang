# ADR 0038: Phase D self-host port (movement 2) — kickoff + (1/N) lexer-in-Sentinel

Status: **ACCEPTED-WITH-AMENDMENTS** — opens **movement 2** of Phase D (ADR 0031
D5): porting the `snc` compiler to Sentinel, stage by stage, each differentially
validated against the Rust `snc` oracle, converging on the bootstrap fixed-point.
This ADR opens the port and designs its **first sub-phase — the lexer**, which has
**LANDED**: `snc lex` (the oracle) + `selfhost/lexer.sentinel` (all 69 `TokenKind`s)
+ a corpus-wide differential test — the Sentinel lexer matches `snc lex` on every
clean-lexing fixture (139/139 in `tests/pass` + `tests/ui`). Each later stage
(parser → resolve → types → HIR/MIR → codegen) gets its own ADR. See ## Amendments.

## Amendments (at lexer (1/N) close)

- **A1 — direct emission, no Token enum (D7).** (1/N) emits the canonical dump
  *directly* into an output `Vec<u8>` rather than building a `Token` enum +
  `Vec<Token>` as D7 sketched. The dump is the contract; the internal token model
  is deferred to (2/N), where the parser actually needs the lexer to *return* a
  token list. Smaller, and it validates the hard part (spans / longest-match /
  keyword classification) without the extra machinery.
- **A2 — Sentinel-language workarounds the port surfaced.** Two quirks of the
  current language shaped the implementation (and are noted for future ergonomics):
  (i) a **flat per-fn variable namespace** — no shadowing or re-declaration, even
  across disjoint `if`-arms — so each branch uses uniquely-named locals; (ii) a
  deep `if`-chain with a borrowing call (`emit`) in each arm's **tail position**
  makes the lexical borrow checker treat the sibling `&mut Vec` borrows as
  overlapping, so the operator dispatch computes the kind + length first and emits
  in a single statement. Neither is a blocker; both are candidates for later
  language polish.
- **A3 — lex-error parity deferred (D6/D8).** The differential corpus excludes the
  one deliberate lex-error fixture (`tests/ui/lex_invalid_char.sentinel`, `let x =
  @`); (1/N) validates happy-path token production. Lexer error parity (bad escapes,
  unterminated literals, invalid bytes) is a follow-on slice.
- **A4 — input via a fixed relative path.** With no argv access yet, the Sentinel
  lexer reads `./input.sentinel`; the differential test stages each fixture there +
  runs the lexer with that cwd. An argv/stdin surface is a later convenience.

Date: 2026-06-02
Related:
  - **0031** (Phase D kickoff): D2 "three movements" — movement 1 (the
    language/stdlib build-out) is **complete** (D.1 sum types, D.2 strings + `u8`,
    D.3 `Vec`, D.4 file I/O, D.5 loops, D.6 modules); this ADR is the start of
    **movement 2** (D5, the self-host sequence). D3 names the Rust `snc` as the
    differential oracle; D5 fixes the stage order (lexer → parser → resolve →
    types → HIR/MIR → codegen); D7's "first sub-phase" was sum types — this ADR is
    the analogous "first sub-phase" of the *port*.
  - **0037** (modules): the last movement-1 prerequisite. The self-host compiler
    will be many `.sentinel` files compiled via the Path A merge. The port is
    **back-end-agnostic** — it is Sentinel *source*, indifferent to whether `snc`
    lowers it via the merge or per-unit objects — so the per-unit separate-comp
    back end (ADR 0037 follow-up) is an **independent** track the port does not
    depend on or gate.
  - **0029 / 0025 D8** (frozen `abi-v1` + reproducible builds): Phase D's
    substrate. The bootstrap fixed-point — the Sentinel compiler compiling itself
    **byte-identically** — is only well-defined against a frozen ABI + deterministic
    emission, which is *why* C5 shipped them.
  - **0001** (staged validation): the project's spine. Each ported stage is
    independently verified against the oracle, not a big-bang leap.

## Context

Of Phase D's three movements (ADR 0031 D2), the **language/stdlib build-out**
(movement 1) is done: the 1.0 language was recursion-only, string-less,
collection-less, single-file; D.1–D.6 added sum types + `match`, strings + a `u8`
byte type, growable `Vec<T>`, file I/O, loops (`while` + `break`/`continue`), and
modules (`use`/`pub`, multi-file). The readiness gate ADR 0031 named is therefore
**cleared** — a lexer in Sentinel needs exactly these, and they now exist:

| a lexer needs… | provided by |
|----------------|-------------|
| read the source file's bytes | `read_file([u8]) -> [u8]` (D.4) over `[u8]` / `u8` (D.2) |
| a `Token` kind sum type | `enum` + `match` (D.1) |
| a growable token stream | `Vec<T>` (D.3) |
| scan bytes with early exit | `while` + `break` / `continue` (D.5) |
| dispatch on byte / state | `match` (D.1) |
| organise into many files | modules — `use` / `pub` (D.6) |

So movement 2 can begin. Per ADR 0031 D5 the order is **lexer first** (the most
self-contained stage: its input is bytes, its output a flat token stream — the
cleanest thing to diff against the oracle).

Two facts shape the design. (1) The Rust `snc` already exposes **stage-dump
subcommands** (`snc parse <file>` pretty-prints the AST); the lexer oracle is the
parallel `snc lex <file>`. (2) The Rust lexer's token set is a fixed, moderate
**69-variant `TokenKind`** (`crates/sentinel-syntax/src/lexer.rs`): keywords
(`Let`…`Match`), punctuation/operators (`Plus`…`ColonColon`), and the three
literal + `Ident` variants. That is the exact behaviour the Sentinel lexer must
reproduce.

## Decision

### D1. Goal.

Begin movement 2: port `snc` to Sentinel stage by stage, each **differentially
validated against the Rust `snc`** on the fixture corpus, converging on the
bootstrap fixed-point (the Sentinel compiler compiles itself byte-identically,
then becomes the reference; the Rust bootstrap retires to a maintenance oracle —
ADR 0031 D1/D6). This ADR opens the port and designs **sub-phase (1/N), the
lexer**; later stages get their own ADRs.

### D2. The differential-oracle method (the spine of the whole port).

The Rust `snc` is ground truth (ADR 0031 D3). For each ported stage:
  1. The Rust `snc` gains a **canonical stage-dump subcommand** emitting that
     stage's output as deterministic text.
  2. The **Sentinel** stage, compiled by the current `snc` + run on the same
     input, emits the **byte-identical** dump.
  3. A **differential test** runs both over the whole `tests/pass` + `tests/ui`
     corpus and asserts the dumps match. A mismatch means the port is wrong (the
     Rust stage is correct by definition).

This makes each stage independently verifiable instead of a leap of faith, and
reuses the existing corpus as the harness (ADR 0031 D3).

### D3. The lexer oracle — `snc lex <file>`.

Add a `lex` subcommand to the Rust `snc` that lexes the source and prints the
token stream in the **canonical token-dump format** (D4), exit 0 on a clean lex.
It mirrors the existing `snc parse`. This is a dev/validation surface, **not** part
of `abi-v1` (no runtime/link contract); but the dump format is pinned by a golden
test so it can't drift silently under the Sentinel lexer.

### D4. The canonical token-dump format.

A stable, line-oriented serialisation both lexers emit, one token per line:

```
<KIND> <start> <end> [<lexeme>]
```

where `<KIND>` is the variant *name* (e.g. `Let`, `Plus`, `Ident`), `<start>`/
`<end>` are byte offsets into the source, and `<lexeme>` is present only for the
value-bearing variants (`Ident`, `IntLit`, `StringLit`, `CharLit`). A trailing
`EOF` line terminates the dump. The variant **name** (not the numeric
discriminant) is emitted, so the two lexers need not agree on enum ordering — only
on the name set + spans + lexemes, which is the behaviour that actually matters.
The format is the contract between the two implementations; the Rust side is
golden-tested, the Sentinel side is diffed against it.

### D5. Where the self-host compiler source lives.

A new top-level **`selfhost/`** directory of `.sentinel` files, compiled by the
current (Rust) `snc`. The lexer starts as `selfhost/lexer.sentinel`; as the port
grows it becomes a module graph dogfooding D.6 (`selfhost/token.sentinel`,
`selfhost/lexer.sentinel`, `selfhost/main.sentinel`, …, with `use`/`pub`). The
differential tests compile these with `snc build` and run the resulting binary.
(Building the port exercises the Path A merge — the first real multi-file Sentinel
program beyond test fixtures.)

### D6. Sub-phase (1/N) scope — the lexer.

Reproduce the Rust lexer's token production: read a source file, scan its bytes,
and emit the 69-variant `TokenKind` stream with byte spans + lexemes, matching
`snc lex` exactly. The token model is a Sentinel `enum` for the kind plus a span
(and a lexeme slice for the value-bearing variants), collected into a
`Vec<Token>`. **Staged within (1/N) if needed:** start with a token subset + a
seed set of fixtures, grow to the full set + the whole corpus — landing only when
it matches the oracle corpus-wide.

**Out of scope at (1/N):** the parser and later stages (their own sub-phases);
**lexer error/diagnostic parity** beyond clean-token production (the Rust lexer's
error cases — bad escapes, unterminated strings — are a follow-on slice once the
happy path matches); performance (correctness-first, like every prior sub-phase).

### D7. Token representation in Sentinel (indicative; settled in implementation).

`enum TokenKind { Let, Fn, …, Ident, IntLit, StringLit, CharLit }` (the 69
variants) + a `Token` carrying `kind` + `start` + `end` (+ a `[u8]` lexeme slice
for the value-bearing kinds), streamed as `Vec<Token>`. Lexemes are byte ranges
into the source `[u8]`. The dump (D4) is produced by `match`-ing each `TokenKind`
to its canonical name. Exact field shape (payload vs parallel arrays) is an
implementation detail of (1/N).

### D8. Out of scope here / honest sizing.

This ADR opens movement 2 and designs the lexer only; parser/resolve/types/
HIR-MIR/codegen each get their own ADR. The **bootstrap fixed-point** (the end
state) is not this sub-phase. The Path A merge backs the build; the per-unit
separate-comp back end (ADR 0037 follow-up) is **independent** and does not gate
the port. **No timeline** — the port is the longest stretch of the longest phase
(ADR 0031 D6).

### D9. Sub-phase split (indicative; reordered if a stage needs it).

| Sub        | Stage (in Sentinel)                                  | Oracle dump |
|------------|------------------------------------------------------|-------------|
| SH (1/N)   | **lexer** — bytes → token stream                     | `snc lex`   |
| SH (2/N)   | parser (+ AST) — tokens → AST                         | `snc parse` |
| SH (3/N)   | resolve — AST → resolved program                     | `snc resolve` (new) |
| SH (4/N)   | types (+ borrow / effect checks)                     | `snc check` (new) |
| SH (5/N)   | HIR / MIR                                             | dumps (new) |
| SH (6/N)   | codegen / runtime glue → object → link               | object diff (repro) |

Each lands only when it matches the Rust stage on the corpus; the final cutover
(D1) is when the whole compiled-by-Sentinel pipeline reproduces the Rust `snc`'s
output — and ultimately its own source — byte-for-byte.

### D10. Phase-go for (1/N).

`selfhost/lexer.sentinel`, compiled by `snc` and run over the `tests/pass` +
`tests/ui` corpus, emits token dumps **byte-identical** to `snc lex` for every
fixture (a differential test asserts it). Seed milestone en route: a clean match
on a small hand-picked fixture set (covering every `TokenKind`), then the full
corpus.

## Reasoning

**Why the lexer first.** Of all stages it has the simplest contract: bytes in, a
flat token stream out, no symbol tables or types. That makes the differential dump
trivial to define + diff, so the *method* (D2) is proven cheaply before the harder
stages lean on it.

**Why a `snc lex` dump rather than calling the Rust lexer lib in-test.** Symmetry:
both sides become text-emitting binaries, so the diff is a plain string compare and
the dump doubles as a dev tool (`snc lex foo.sentinel`). It also forces the
canonical format (D4) to be a real, golden-pinned artifact rather than an in-test
ad-hoc.

**Why names, not discriminants, in the dump.** Pinning the numeric enum order
across two independent implementations is brittle and couples them needlessly; the
*behaviour* that matters is "same kinds, same spans, same lexemes," which the
variant name + span + lexeme captures.

**Why the port is back-end-agnostic (and so doesn't wait on the per-unit back
end).** The self-host compiler is Sentinel *source*; whether `snc` lowers it via
the Path A merge or per-unit objects + link is invisible to that source. Building
the port on the merge incurs no port-level rework when the per-unit back end lands.
This is what lets movement 2 start now rather than after the (large, cohesive)
separate-comp effort.

## Consequences

### Positive
- Movement 2 begins — the project's defining goal — with the same staged,
  oracle-validated discipline (ADR 0001) as everything before it.
- The first real multi-file Sentinel program (beyond fixtures) dogfoods D.6
  modules; `snc lex` is a useful standalone tool.
- Each ported stage is independently shippable + verifiable; no big-bang.

### Negative
- The longest stretch of the longest phase; the lexer is one of ~6 stage
  sub-phases, and the fixed-point is many sub-phases away.
- A second lexer to keep in sync with the Rust one until cutover (mitigated: the
  differential test fails the moment they diverge).

### Neutral
- The Rust `snc` remains the production compiler throughout movement 2 (ADR 0031
  D6); nothing about the 1.0 toolchain changes until the fixed-point bakes.
- `snc lex` adds a dev surface, not a runtime/ABI one.

## Revisit

PROPOSED until the lexer sub-phase (1/N) lands, then ACCEPTED-WITH-AMENDMENTS as
the port's sub-phases close. Triggers:
- **D4 dump format**: if span/lexeme encoding proves awkward to emit identically
  from Sentinel, refine the canonical format (it's a dev contract, freely
  amendable — unlike `abi-v1`).
- **D6 scope**: if matching the full corpus needs lexer *error* parity sooner than
  expected, pull it into (1/N) instead of a follow-on.
- **D9 ordering**: if a later stage wants an earlier one's Sentinel form first,
  reorder (per ADR 0031 Revisit).
- **Reduced subset**: if the full `snc` proves too large to port wholesale, weigh
  a "Sentinel-0" metacircular subset (ADR 0031 Revisit) as an intermediate target.
