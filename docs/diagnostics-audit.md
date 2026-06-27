# Diagnostics Quality Audit (P3.6)

_Audit date: 2026-06-27. Read-only audit of the three highest-traffic diagnostic
families — borrow checker, effects/handlers, constant-time `secret_leak` — against
the actionability bar from [`REVIEW_ACTION_PLAN.md`](REVIEW_ACTION_PLAN.md): a good
diagnostic states (1) **what** was expected vs found, (2) **why** — the rule being
enforced, and (3) the **workaround/fix**._

> **Status:** audit complete; fixes are **not yet applied** — they require a build +
> `insta` snapshot re-bless (`just bless`), so they land on a machine with LLVM 18.
> The MIR `secret_leak` family is already exemplary — use it as the template; do not
> regress it.

## Headline finding — a plumbing bug, not a wording problem

Most of the help text the user *should* see **already exists** in the error enums
(`#[diagnostic(help(...))]` / `#[label(...)]`), but it is **silently dropped at
render time**. Two locations:

- **[`crates/sentinel-base/src/lib.rs:41-58`](../crates/sentinel-base/src/lib.rs)** —
  the salsa `Diagnostic` accumulator struct carries only `stage / severity / code /
  message / span`. **No `help` field and no label-text field**, so a variant's help
  and labels are discarded the moment it becomes a `Diagnostic`.
- **[`crates/sentinel-driver/src/main.rs:1793-1807`](../crates/sentinel-driver/src/main.rs)** —
  `render_diagnostics` reconstructs a miette diagnostic and hard-codes
  `.with_label(LabeledSpan::at(span, ""))` (empty label) with **no `.with_help()`
  call**. The function's own doc comment (lines 1785-1792) admits this is a follow-up.

Two render paths exist, and they are wildly unequal:

| Path | Used by | Renders `help:`? | Renders label text? |
|---|---|---|---|
| `Report::new(err)` direct (miette derive) | **MIR `secret_leak` only** | **Yes** | **Yes** |
| salsa accumulator → `render_diagnostics` | borrow, effects, all source-level type/CT checks, lex, parse, resolve | **No (dropped)** | **No (emptied)** |

**Fix (the single biggest lever):** add `help: Option<String>` and label text (e.g.
`Vec<(Range<usize>, String)>`) to the `Diagnostic` accumulator, and thread them through
`render_diagnostics` (`.with_help(...)`, real label text). This instantly upgrades ~25
diagnostics from "bare assertion" to "what / why / how" using wording that already
exists. Every wording item below marked **(plumbing)** is blocked only by this.

## Prioritized fixes

1. **(plumbing, whole-compiler)** Thread `help` + label text through the `Diagnostic`
   accumulator + `render_diagnostics`. Unblocks ~25 already-authored helps. *Do this first.*
2. **(semantics) Secret array index is mis-framed as a type error.** `a[secret_i]` is
   rejected by `IndexNotInt` as `array index must be \`i64\`, got \`<secret#0>\``
   (`sentinel-types`, ~lines 2921-2927; fixture `c52_secret_array_index`). It never
   mentions **timing/constant-time**, so a user may "fix" it by `declassify`-ing the
   index — silently defeating the CT guarantee. This is the worst failure mode in the
   audit (a CT violation steering toward an insecure fix). Give it a dedicated
   `SecretIndex`-style message: *"array index must not be `secret` — a secret index
   leaks via a cache-timing side channel; restructure to a constant-time table scan, or
   `declassify` only if you accept the leak."* Also render `secret i64`, not `<secret#0>`.
3. **(wording) `WriteWhileBorrowed`** — render the inner-block idiom as a `note:` (the
   exact lexical over-rejection named in [`borrow-check-limitations.md`](borrow-check-limitations.md) §1).
   Current help "introduce a new scope to bound the borrow" is too abstract; show:
   `let snapshot = { let r = &x; *r }; x = 10;  // r's scope ended at the brace`.
   (`sentinel-borrow-check`, ~line 257.)
4. **(wording+plumbing) `UseAfterMove`** — restore the dropped **"moved here"** label
   (today only the *use* caret renders, blank). The move site is the most useful pointer,
   especially for the partial-move sub-case. (`sentinel-borrow-check`, ~lines 292-305;
   fixture `c25_use_after_partial_move`.)
5. **(stale+wording) `UnhandledEffect`** — the authored help says *"handlers land at ADR
   0020"*, which is **stale** (handlers shipped). Replace with the rule + the concrete
   shape: *"`main` must have an empty effect row; wrap the effectful call in
   `handle <expr> with { Io.<op>(k) => … }`."* Consider listing *all* unhandled effects,
   not just the first. (`sentinel-effect-check`, ~lines 91-96; fixture `c37_perform_outside_handle`.)
6. **(wording) `undefined_handler_op`** — list the operations the effect *does* declare
   ("did you mean"), the highest-traffic handler typo. (`sentinel-resolve`; fixture
   `c34_handle_undefined_op`.)
7. **(wording) `SecretBranch`** — render its (already-good) help and add a masked-select /
   `declassify` note to unify framing with the MIR pass. (`sentinel-types`, ~lines
   3278-3287; fixtures `c52_secret_in_if`, `c52_secret_in_while`.)

## Already exemplary — do not regress

- **MIR `secret_leak`** (`sentinel-mir`, ~lines 784-799; `c52_secret_leak.snap`) — the gold
  standard: full what/why/how with a rendered `help:` block. **Template for the fix above.**
- **`ReturnsLocalRef`**, **`MovedInLoopBody`** (borrow) and **`SecretInRefDeref`** (types) —
  authored help already nails why+fix (e.g. the precise `& secret T` vs `secret &T`
  distinction); only the plumbing hides them.
- **`duplicate_handler_arm`** (`c34_handle_duplicate_arm`) — clear, well-scoped message.

## Note on scope

The "CT family" a user hits *first* is split: the direct `a[secret]` / `if secret` cases
are rejected in **`sentinel-types`** (accumulator path, help dropped), while the MIR
`secret_leak` pass (which renders well) catches flows that survive to MIR. Fixing the
plumbing makes the type-checker-side CT diagnostics as good as the MIR one.
