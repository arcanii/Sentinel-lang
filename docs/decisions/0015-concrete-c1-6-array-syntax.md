# ADR 0015: Concrete C1.6 surface syntax — arrays, indexing, heap allocation, and the D10 unlock

Status: ACCEPTED-WITH-AMENDMENTS — D1-D5, D7-D13 all fully
exercised at C1.6.1 + C1.6.2-6 (commits 3cfd49f + 8c5bbbe). One
amendment uncovered during implementation:

  - **D6 amendment**: the ADR proposed extending NullableInner
    with `Array(ArrayElem)` and ArrayElem with
    `Nullable(NullableInner)` to represent `?[T]` and `[?T]`.
    Rust's mutual enum recursion forces Box indirection which
    breaks Type's Copy and would cascade through the codebase.
    The C1.6 implementation caps the depth at 1: NullableInner
    and ArrayElem stay as primitive-only subset enums (I64,
    I32, Bool, Struct). `?[T]` and `[?T]` are deferred — they
    become "not yet representable" until a future ADR adds them
    (likely alongside generics at C1.7).

  - **D11 unlock implemented**: the ADR 0014 D10 deferral
    retires here. `?Struct` codegen uses heap indirection
    (`{ i1 valid, ptr payload }`); the cycle detector relaxes
    to walk only direct struct edges. Recursive structs through
    `?T` now type-check, compile, and run.

Date: 2026-05-26
Last touched: 2026-05-26 (C1.6 landed; status flipped to
ACCEPTED-WITH-AMENDMENTS with the D6 amendment + D11 implementation
documented)
Related: 0011 (Phase C1 kickoff; D3 lists arrays as a C1
deliverable, D6 schedules C1.6 after C1.5), 0014 (concrete C1.5
surface — D10 deferred the recursive-struct cycle-check
relaxation to C1.6's heap arrival), 0013 (concrete C1.4 surface —
established the parallel-tree pattern for compound types that
arrays extend), 0012 (concrete C1 surface — D3 "primitives are
identifiers, not keywords" pattern continues to hold for
type-position; arrays use bracket syntax instead)

## Context

C1.5 landed nullable types `?T` end-to-end with the explicit
deferral of ADR 0014 D10 (the recursive-struct cycle-check
relaxation). The deferral was forced by C1.5's flat representation
`?T = { i1, T }` — recursive struct fields contain their parent
type by value, leading to infinite LLVM struct sizes. C1.6
introduces the first heap-allocation primitive in Sentinel, which
both:

  1. Backs arrays (the primary C1.6 deliverable), since arrays are
     variable-length and can't be sized at fn-frame stack-alloca
     time.
  2. Provides the indirection that breaks recursive-struct cycles,
     finally unlocking ADR 0014 D10.

The 1.0 target surface (SENTINEL_DESIGN2.md §4.4 + §15.1) uses
`[T]` for arrays with `a[i]` indexing and bounds-checked at
runtime. Sentinel arrays are 1.0-targeted to be growable
(Vec<T>-style) eventually, but at C1.6 we ship the simpler
fixed-after-creation form: an array literal `[1, 2, 3]` creates
a 3-element array; the length is captured at creation and
queryable via `len(a)`.

What this ADR explicitly does NOT commit to at C1.6: growable
arrays (push/pop/append), array slicing (`a[i..j]`), array
methods, multi-dimensional arrays (arrays-of-arrays), mutable
indexing (`a[i] = v`), generic `Array<T>` syntax. All of those
are later ADRs without conflicting with the C1.6 grammar.

The C1.5 lexer's current token set is:

  - **Keywords**: `let`, `fn`, `if`, `else`, `true`, `false`,
    `struct`, `null`
  - **Punctuation**: `+ - * / = ( ) { } , ; : . ? ->`
    `== != < <= > >= && || !`
  - **Identifiers**: `[A-Za-z_][A-Za-z0-9_]*`
  - **Integer literals**: `[0-9]+`
  - **Skipped**: `[ \t\r\n]+`, `//[^\n]*`

C1.6 extends this with the additions in D8 below.

## Decision

Thirteen D-numbered sub-decisions covering syntax (D1-D5), the
type-level rules (D6, D7), lexer additions (D8), the heap runtime
(D9), bounds-check semantics (D10), the D10-of-ADR-0014 unlock
(D11), and out-of-scope items (D12, D13).

### D1. Array type syntax: `[T]`.

A `[` token followed by a base type followed by `]` denotes an
array of that type. Grammar extension at the type-expr level:

    type         = base_type | '?' type | '[' type ']'
    base_type    = Ident          // i64 / i32 / bool / struct name

The C1.5 `?T` form (per ADR 0014 D1) still parses; `[T]` adds a
parallel parser path. Nested arrays (`[[T]]`) and array-of-
nullable (`[?T]`) and nullable-of-array (`?[T]`) all parse at
the grammar level; the type checker accepts some combinations
and rejects others (see D7).

Notably absent: a length parameter (no `[T; N]` like Rust). C1.6
arrays carry their length as runtime data, not in the type. This
matches Sentinel's preference for fewer compile-time-parameterised
type forms (generics in general are C1.7). When fixed-size arrays
become useful (e.g., for cryptographic key buffers), a future ADR
adds `[T; N]` alongside `[T]`.

### D2. Array literal syntax: `[e1, e2, …]`.

A `[` token followed by a comma-separated list of expressions
followed by `]` denotes an array literal:

    array_lit    = '[' expr_list ']'
    expr_list    = (expr (',' expr)*)? ','?

All elements must type to the same T; the resulting type is
`[T]`. Trailing comma is permitted (consistent with fn parameter
lists per ADR 0010 D5 and struct field lists per ADR 0013 D1).

Empty array literals (`[]`) require explicit type annotation
because the element type is unrecoverable from the literal alone:

    let xs: [i64] = [];       // OK — annotation provides T
    let ys = [];              // TypeError::AmbiguousEmptyArray

Same pattern as null literal needing context (ADR 0014 D2).

### D3. Indexing syntax: postfix `a[i]`.

A postfix `[expr]` after an atom denotes indexing. Extends
parse_postfix alongside `.field` (precedent from ADR 0013 D2):

    postfix      = atom postfix_op*
    postfix_op   = '.' Ident          // field access (C1.4)
                 | '[' expr ']'        // array index (C1.6)

Indexing returns the element type T. The index must be `i64`
(C1.7 with generics may extend to other integer types). At
runtime, the index is bounds-checked per D10.

Indexing on a non-array surfaces as
`TypeError::IndexOnNonArray`. Index of non-i64 surfaces as
`TypeError::IndexNotInt`.

### D4. Array length: `len(a)` builtin function.

A new builtin function, registered alongside `print`,
`unwrap_or`, `is_some`:

  - `len(a: [T]) -> i64` — returns the array's length.

Generic over T, special-cased at the type-check stage same as
the C1.5 builtins per ADR 0014 D9. The implementation lowers
inline: extract field 0 of the LLVM `{ i64 len, T* data }`
struct.

A method-call form `a.len` is NOT in scope at C1.6 because
methods are a C4 concern. The function-call form is a natural
fit for the existing call grammar.

### D5. Empty arrays require annotation.

Per D2, `let xs = []` is a `TypeError::AmbiguousEmptyArray`
because the type checker has no way to infer T. The fix is
either an annotation (`let xs: [i64] = []`) or a non-empty
literal (`let xs = [0]`).

This mirrors ADR 0014 D2's null-literal-needs-context rule —
both `null` and `[]` are "polymorphic" literals that bidirectional
checking resolves against an expected `?T` or `[T]`.

### D6. Type-level representation: `Type::Array(ArrayElem)`.

The internal representation extends the C1.5 universe:

    enum Type {
        I64, I32, Bool, Struct(StructId),
        Nullable(NullableInner),
        Array(ArrayElem),
    }

    enum ArrayElem {
        I64, I32, Bool, Struct(StructId),
        Nullable(NullableInner),
    }

Follows the C1.5 pattern (`Type::Nullable(NullableInner)`) for
the same reasons: keeps `Type` `Copy`, makes some nestings
structurally unrepresentable. `ArrayElem` includes `Nullable`
(so `[?T]` is valid) but NOT `Array` (so `[[T]]` is rejected at
the type level — multi-dimensional arrays land in a later ADR).

Conversion helpers `ArrayElem::to_type()` and
`Type::to_array_elem()` mirror the C1.5 NullableInner conversion.
The non-nesting rule is enforced at `resolve_type_expr` (rejects
`[[T]]` as `TypeError::NestedArray`) and at the parser /
constructor level redundantly for defense in depth.

The Nullable + Array combinations:

  - `?[T]` — nullable array. Represented as
    `Type::Nullable(NullableInner::Array(ArrayElem))`. **BUT**
    NullableInner doesn't currently have an Array variant. We
    EXTEND `NullableInner` to add `Array(ArrayElem)`:

        enum NullableInner {
            I64, I32, Bool, Struct(StructId),
            Array(ArrayElem),       // ADR 0015 D6 extension
        }

    This keeps the no-nested-`??T` rule from ADR 0014 D6 while
    allowing `?[T]`.

  - `[?T]` — array of nullables. Represented as
    `Type::Array(ArrayElem::Nullable(NullableInner))`. Valid.

  - `[[T]]` — nested array. Rejected at type-resolution (no
    `Array` constructor on `ArrayElem`).

  - `[?[T]]` — array of nullable-array. Valid as
    `Type::Array(ArrayElem::Nullable(NullableInner::Array(...)))`.

  - `?[?T]` — nullable array of nullables. Valid as
    `Type::Nullable(NullableInner::Array(ArrayElem::Nullable(...)))`.

So one extra constructor in each of `NullableInner` and a new
flat `ArrayElem` enum captures everything C1.6 needs.

### D7. Bidirectional checking extends to array literals.

The ADR 0014 D5 bidirectional infrastructure extends to array
literals: when the expected type is `[T]`, each element is
checked against `T` (with widening per ADR 0014 D3 if T is `?U`
and the element is `U`).

For empty array literals, the expected type IS the array type
(per D5). For non-empty literals, the first element's type
synthesizes T if no expected is provided; subsequent elements
are checked against that T.

Bidirectional pushdown sites for arrays:

  - `let xs: [T] = [...]` — annotation pushes [T] down to the
    literal.
  - `fn f(xs: [T])` — call arg position pushes [T] down to the
    literal at call sites.
  - `fn f() -> [T] { [...] }` — fn return type pushes [T] down
    to the body tail.
  - Inside `[...]`, each element gets T as its expected type.

### D8. Lexer additions at C1.6.

Two new tokens. No new keywords (D4's `len` and D11's heap
primitive are registered builtins, not lexer keywords):

| Token   | Notes                                       |
| ------- | ------------------------------------------- |
| `[`     | LBracket — opens array type / literal / index |
| `]`     | RBracket — closes array type / literal / index |

Two new tokens total. Empty position to overload for ranges
(`..`, `..=`) and similar future operators stays clean.

### D9. Heap allocation runtime.

C1.6 introduces the first heap-allocated values in Sentinel
(arrays, recursively-nullable structs). The implementation uses
the C runtime's `malloc` / `free`, exposed via two
sentinel-runtime symbols:

  - `sentinel_alloc(size: i64) -> ptr` — wraps `malloc`; panics
    on allocation failure (returning `null` from `malloc`).
  - `sentinel_panic_oob(idx: i64, len: i64) -> never` — called
    on out-of-bounds access; prints a diagnostic and aborts.

These are NOT exposed at the language level — they're called
internally by codegen for array literal construction and bounds
checking. Sentinel users can't directly invoke them.

There is NO `free` at C1.6. Arrays leak; this is an explicit
scope-cut because lifetime management is C2's region work. The
leak is acceptable for the bootstrap compiler's typical usage
(short-lived programs) and is documented as a known limitation.

Future ADRs (C2+) will add reference counting, regions, or
some other resource-management strategy. C1.6 punts.

### D10. Bounds-check semantics.

Every `a[i]` access at runtime checks `0 <= i < len(a)`. On
failure, the program calls `sentinel_panic_oob(i, len)` which
prints a diagnostic and aborts (non-zero exit code).

The check lowers to:

    %len = load %array.len
    %ok = and (icmp sge %i, 0) (icmp slt %i, %len)
    br %ok, %access_bb, %panic_bb
  access_bb:
    %data = load %array.data
    %elem_ptr = gep %data, %i
    %elem = load %elem_ptr
  panic_bb:
    call @sentinel_panic_oob(%i, %len)
    unreachable

Bounds-check elision (when the index is statically provably
in-range) is NOT in scope at C1.6. Adding it is a codegen pass
that arrives at C2+ when the type checker has range analysis
(or sooner if a clear case appears).

### D11. ADR 0014 D10 unlock: recursive structs via `?T` with heap indirection.

C1.5 deferred the recursive-struct cycle-check relaxation
because `?T` was represented as `{ i1, T }` flat-inline. C1.6's
heap arrival provides the missing indirection for the struct
case:

  - `?T` for T = primitive (I64, I32, Bool) stays
    `{ i1 valid, T payload }` flat — no indirection needed,
    bounded size.
  - `?T` for T = struct (`Struct(StructId)`) becomes
    `{ i1 valid, T* payload }` where the payload is a
    heap-allocated `T`. The null case has `valid: false` and
    `payload: undef` (the pointer is never read when valid is
    false).

Construction of `?Foo` for a struct Foo allocates space for one
Foo on the heap via `sentinel_alloc`, copies the Foo value in,
and stores the pointer. The cycle is broken because the field's
storage is now pointer-sized (8 bytes on 64-bit), regardless of
the inner struct's size.

The cycle detector in sentinel-types relaxes per ADR 0014 D10's
original spec: cycles that include at least one nullable-struct
edge are accepted; cycles consisting only of direct-struct
edges still error as `TypeError::RecursiveStruct`.

After C1.6:

    struct Node { value: i64, next: ?Node }     // NOW OK
    struct Tree { l: ?Tree, r: ?Tree }          // NOW OK
    struct Bad { x: Bad }                       // STILL ERROR

The C1.5 STATE.md decision 72 ("ADR 0014 D10 deferral") and the
ADR 0014 D10 amendment are both retired with this C1.6
implementation.

### D12. Out of scope at C1.6.

The following array-related features are explicitly deferred:

  - **Mutable indexing** (`a[i] = v`). Out of scope because
    Sentinel has no mutation semantics yet. C2 introduces
    mutability with regions; assignment lands then.
  - **Array slicing** (`a[i..j]`). Requires the `..` lexer
    token; defer to a future ADR.
  - **Growable arrays** (`push`, `pop`, `append`, etc.). Sentinel
    1.0 will have `Vec<T>`-style growable arrays; at C1.6
    arrays are fixed-after-creation.
  - **Multi-dimensional arrays** (`[[T]]`). Rejected at the
    type level per D6.
  - **Array methods** (`a.map(f)`, etc.). Methods are C4.
  - **Array comparison** (`==` on arrays). Element-wise compare
    is doable but expensive; defer to a structural-equality
    revisit at C1.7+.
  - **`free` / RAII / explicit deallocation**. Arrays leak at
    C1.6; resource management is C2+.

### D13. `fn main() -> i64` invariant stays.

Same as ADR 0012 D11 / 0013 D11 / 0014 D11. Main returns i64;
codegen truncates to i32 for the C ABI. Arrays can be returned
from non-main fns but main returns scalar.

## Reasoning

The decisions cluster around four themes:

**Minimum-viable array surface.** D2 (positional literals, no
named-init), D3 (positional indexing, no slicing), D4 (`len` as
fn-call, not method), D5 (annotation-required empty arrays), D12
(deferred features list) all keep C1.6 scope-tight. The C1.6 win
is "the type universe gains a variable-length compound type plus
heap allocation"; everything else waits.

**Match Rust where it doesn't conflict.** D1 (`[T]` syntax),
D2 (`[1, 2, 3]` literals), D3 (postfix `a[i]`), D10 (bounds-
checked indexing with abort on OOB) all match Rust's
fundamentals. Sentinel diverges by using `[T]` for the
heap-backed runtime-length type instead of Rust's
`Vec<T>` — a simpler surface for the language's intended
bootstrap-then-self-host trajectory.

**The D11 unlock pays for the rest.** Implementing heap for
arrays naturally provides heap for `?Struct`, which closes
the ADR 0014 D10 deferral. Two language features for the price
of one runtime addition. Same pattern as C1.4's
StructValue-via-build_insert_value paving the way for C1.5's
nullable struct value lowering.

**Leak now, manage later.** D9's "no free at C1.6" is honest
about the lifetime-management gap. C2's region work is the
right place to introduce a sound resource-management story.
Until then, leaks are documented and acceptable for
bootstrap-compiler use cases.

## Consequences

### Positive

- C1.6's parser extension is bounded: `[T]` in parse_type
  (one new parallel-to-`?T` arm), `[...]` in parse_atom (one
  new arm), `[i]` in parse_postfix (one new arm alongside
  `.field`). ~30-50 LOC.

- C1.6's type-check extension reuses the C1.5 bidirectional
  infrastructure cleanly. The element-type pushdown into
  array-literal element checking is a natural extension. New
  TypeError variants: `IndexOnNonArray`, `IndexNotInt`,
  `AmbiguousEmptyArray`, `NestedArray`. ~80-120 LOC.

- C1.6's codegen extension is the substantive layer: pass 0
  adds heap-allocation calls; pass 2 emits malloc + element-
  store loops for array literals + bounds-check basic blocks
  for indexing + GEP-based element access. Runtime gains two
  symbols (`sentinel_alloc`, `sentinel_panic_oob`). ~150-200
  LOC.

- The ADR 0014 D10 deferral closes. Recursive structures
  (linked lists, trees) become representable. The C1.5 STATE
  decision 72 "deferred" status updates to "implemented."

- Empty-array + null-literal both use the same bidirectional
  infrastructure. Consistent ergonomic story for context-
  inferred literals.

### Negative

- D9's "no free" means C1.6 programs leak memory. For
  test fixtures (small, short-lived programs) this is fine;
  for "real" programs eventually built with Sentinel it's not.
  Mitigation: documented limitation; C2 will introduce the
  region-based resource management story.

- D10's bounds check adds runtime cost per access. Acceptable
  at C1.6's "correctness first" stance; future ADRs may add
  bounds-check elision based on type-checker range analysis.

- D6's `ArrayElem` (and the `NullableInner::Array` extension)
  duplicates the constructor list TWICE more on top of C1.5's
  NullableInner. C1.7's generics may push us back to a
  Box-based representation that handles arbitrary nesting; the
  flat-subset approach is C1-specific.

- D11's "?Struct uses heap indirection" creates an asymmetry
  with `?{primitive}` which stays flat-inline. The codegen has
  two branches per `?T` operation. Slightly more complex than
  a uniform representation but the asymmetry is real: primitives
  are tiny, structs can be arbitrarily large, the inline-vs-
  pointer choice is fundamentally type-driven.

### Neutral

- D4's `len` as a fn-call rather than `.len` field/method is
  a C1.6 ergonomic choice. C4 (methods) may add `.len()` as
  a method form; the fn-call form stays for backwards-compat.

- D8's `[` / `]` tokens are not reserved for anything else
  in the existing grammar — adding them doesn't cause
  ambiguity. Lists / tuples / other bracket-using constructs
  would need a separate ADR.

## Alternatives considered

- **`Vec<T>` syntax with generics.** Rejected per D1: requires
  generics (C1.7) and is heavier than `[T]` for the same
  semantics. C1.6 ships the simpler surface.

- **`[T; N]` fixed-size arrays.** Rejected for C1.6: requires
  const-expression handling in type position, adds a separate
  array shape, and isn't load-bearing for the C1.6 deliverable.
  Future ADR may add for crypto buffers etc.

- **Methods on arrays (`a.len`, `a.iter`, etc.).** Rejected per
  D4: methods are C4. The fn-call form covers C1.6's needs.

- **Reference-counted arrays.** Rejected per D9: would require a
  drop-trait-like system; resource management is C2's
  region work. Leak now, manage later is the honest scope-cut.

- **No bounds checking at C1.6 (defer to a debug-mode
  toggle).** Rejected per D10: HANDOVER §6.2's "obvious memory-
  safety violations rejected at the end of C1" criterion
  requires bounds checking by default. Performance-mode
  elision is fine to defer.

- **Single `[T]` representation with always-heap allocation,
  no inline `?T = { i1, T }` for primitives.** Rejected per
  D11: forces ALL `?T` to allocate, even `?i64`. Wasteful for
  short-lived programs and adds malloc churn. The mixed
  representation (inline for primitives, indirect for structs)
  is the right cost-benefit point.

- **Array index returns `?T` for safe indexing.** Rejected:
  ergonomically heavy (every access needs unwrap), diverges
  from Rust, and the "OOB aborts" semantic is well-understood.
  Future safe-indexing helpers can live as builtins/methods.

## Revisit

This ADR is **PROPOSED** until C1.6 lands the syntax decisions
herein. Per-D revisit triggers:

- **D1** (`[T]` syntax): revisit at C1.7 (generics) — `[T]`
  may need to coexist with `Vec<T>` or be subsumed by it.
- **D4** (`len` as fn-call): revisit at C4 when methods land.
- **D6** (Type::Array(ArrayElem)): revisit at C1.7 when
  generics need fully-recursive Type — likely move to Box<Type>
  with a deeper refactor.
- **D8** (`[` / `]` tokens): revisit when tuple types
  (`(T1, T2)` vs `[T1, T2]`?) or list comprehensions arrive.
- **D9** (heap leak): revisit at C2 when regions land. This is
  THE big language-level resource-management story.
- **D10** (bounds-check semantics): revisit at C2+ when type-
  checker range analysis enables elision.
- **D11** (recursive struct unlock): once landed, no revisit.
  The cycle detector + heap indirection co-evolves with C2's
  references.
- **D12** (out-of-scope list): each item gets its own future
  ADR.

## Appendix: C1.6 phase-go programs

For reference, three canonical C1.6 acceptance programs.

### Phase-go 1: Array sum via recursive fn

    fn sum_from(a: [i64], i: i64) -> i64 {
        if i == len(a) {
            0
        } else {
            a[i] + sum_from(a, i + 1)
        }
    }

    fn main() -> i64 {
        let arr: [i64] = [1, 2, 3, 4, 5];
        print(sum_from(arr, 0))
    }

Expected: stdout `15\n`, exit 0. Exercises:

  - Array type syntax (D1) — `[i64]` in two positions.
  - Array literal (D2) — `[1, 2, 3, 4, 5]`.
  - Indexing (D3) — `a[i]`.
  - `len(a)` builtin (D4).
  - Bounds-check termination (D10) — the `i == len(a)` guard
    keeps the recursion from going OOB.
  - Heap allocation backed array values (D9).

### Phase-go 2: Linked list via `?Node` (D11 unlock)

    struct Node { value: i64, next: ?Node }

    fn list_sum(node: ?Node) -> i64 {
        if is_some(node) {
            // Awkward at C1.5/C1.6 because we don't have flow
            // typing or pattern matching — fn boundary
            // unpacks via unwrap_or_else equivalents.
            sum_unwrapped(node, 0)
        } else {
            0
        }
    }

    fn sum_unwrapped(node: ?Node, acc: i64) -> i64 {
        // ... (uses unwrap_or-style access; concrete shape
        // depends on what other builtins C1.6 adds)
    }

    fn main() -> i64 {
        let three = Node { value: 3, next: null };
        let two = Node { value: 2, next: three };
        let one = Node { value: 1, next: two };
        print(list_sum(one))
    }

Expected: stdout `6\n`, exit 0. **This program is intentionally
incomplete** in the ADR — its exact shape depends on what
unwrap-style helpers C1.6 adds beyond `unwrap_or`. The point is
to exercise:

  - Recursive struct via `?T` (D11) — the ADR 0014 D10 unlock.
  - Heap allocation for `?Node` payloads.
  - Cycle detector accepts the cycle.

### Phase-go 3: Empty array with annotation

    fn fill(n: i64) -> [i64] {
        // C1.6 doesn't yet have a way to construct
        // dynamically-sized arrays. This phase-go is illustrative
        // only — concrete construction requires either an
        // initializer-fn pattern or a fixed-size literal.
        [0, 0, 0]
    }

    fn main() -> i64 {
        let xs: [i64] = [];                  // empty-with-annotation (D5)
        let ys = fill(3);
        print(len(xs) + len(ys))
    }

Expected: stdout `3\n`, exit 0. Exercises:

  - Empty array annotation (D5).
  - `len` returning 0 for empty array.
  - `fill` returning an array.

The full c16_go_no_go fixture will likely combine phase-go 1
and 3 elements. The linked-list case (phase-go 2) lives as a
separate fixture `c16_linked_list.sentinel`.

`tests/pass/c16_go_no_go.sentinel` and
`tests/pass/c16_linked_list.sentinel` (paired with their test
functions in pass.rs) will be the concrete acceptance fixtures
for C1.6.
