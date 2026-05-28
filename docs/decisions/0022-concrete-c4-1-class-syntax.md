# ADR 0022: Concrete C4.1 class surface — declarations, methods, init

Status: ACCEPTED-WITH-AMENDMENTS — flipped at C4.1 close after
the two-iteration landing (C4.1 (1/N) AST + parser; C4.1 (2/N)
resolve / types / codegen + method-call + Name::init). Eleven
D-decisions exercised; two amendments documented below covering
the partial definite-assignment dataflow (D4) and the deferred
`Self` in general type position (D8). This ADR details the
concrete surface syntax + typing rules for the Phase C4.1
class sub-phase per ADR 0021 D1-D3. ADR 0021 is the phase
kickoff (PROPOSED); ADR 0022 mirrors the role of ADR 0013
(concrete C1.4 struct syntax) within the larger phase.

Date: 2026-05-28
Related:
  - **0021** (Phase C4 kickoff — PROPOSED): the umbrella ADR.
    D1 picks `class Name { ... }` as the user-facing surface
    that desugars to struct + methods + impls; D2 picks
    `self: &Self` / `self: &mut Self` method signatures; D3
    picks `init(args)` with definite-assignment. This ADR
    fills in the syntax + typing detail.
  - **0013** (Concrete C1.4 struct syntax — ACCEPTED): the
    existing `struct Name { field: T, ... }` declaration +
    `Name { field: value }` literal + postfix `.field`
    access. C4.1 builds the class machinery on top of (and
    leaving intact) the C1.4 struct surface.
  - **0016** (C1.7 generics — ACCEPTED): generic structs +
    monomorphisation. C4.1 inherits the pattern; methods on
    generic classes monomorphise alongside the class's
    generic instances.
  - **0017** (Phase C2 kickoff — ACCEPTED-WITH-AMENDMENTS):
    `self: &Self` / `self: &mut Self` reuse the C2 borrow
    rules. DropPlan integrates with init's definite-assignment
    check so a partially-constructed `self` correctly skips
    drop of unassigned fields.

## Context

C4.0 reserved the twelve Phase C4 keywords. C4.1 ships the
**class** surface end-to-end: declarations, fields, methods,
init constructors, and the typing rules that make
construction safe (no half-constructed objects observable).

Sentinel's class model per SENTINEL_DESIGN §7:

  - No class-based inheritance — composition with explicit
    delegation replaces it (C4.3).
  - Construction via `init(args)`; every field definite-
    assigned before the constructor returns.
  - Methods inside `fn name(self: &Self, ...) -> R { ... }`
    syntax; the receiver kind (`&Self` vs `&mut Self`)
    determines borrow-check posture per ADR 0017 D6.
  - Trait impls land at C4.2; delegation at C4.3.

At C4.1 minimum, classes are *self-contained* — they have
fields + methods + an init constructor, but no traits and no
delegation yet. Calling a method on a class instance is
statically dispatched against the declaring class.

The C4.1 lexer state (from C4.0):

  - **Keywords**: `let, fn, if, else, true, false, struct,
    null, mut, effect, secret, declassify, handle, with,
    perform, return, class, trait, impl, init, delegate,
    scope, spawn, await, Self, self, as, for`.
  - **Punctuation**: unchanged from C3.

C4.1 picks up the class-related keywords (`class`, `init`,
`Self`, `self`) and weaves them through AST + parser +
resolve + types + codegen.

## Decision

Eleven D-numbered sub-decisions covering surface (D1-D5),
typing (D6-D8), codegen (D9), lexer recap (D10 — no new
tokens), and the C4.1 phase-go (D11).

### D1. `class Name { items }` declaration grammar.

The class declaration is a top-level item alongside `fn` and
`struct`:

    class Point {
        let x: i64;
        let y: i64;
        pub init(x: i64, y: i64) {
            self.x = x;
            self.y = y;
        }
        pub fn manhattan(self: &Self) -> i64 {
            self.x + self.y
        }
    }

Grammar:

    class_decl    = 'class' Ident '{' class_item* '}'
    class_item    = field_decl | init_decl | method_decl
    field_decl    = ('pub')? 'let' Ident ':' type_expr ';'
    init_decl     = ('pub')? 'init' '(' params? ')' block
    method_decl   = ('pub')? 'fn' Ident '(' self_param (',' param)* ')'
                    ('->' type_expr)? ('!' effect_row)? block
    self_param    = 'self' ':' ('&' 'mut'? 'Self')

At C4.1 minimum:
- Generic classes (`class Pair<A, B>`) are deferred to a later
  sub-phase (the C1.7 generic machinery is in place, but
  the AST + parser + monomorphisation extension for class
  generics is its own work).
- Visibility is parsed (`pub`) but the visibility check
  itself is a stub at C4.1 — Sentinel's module system arrives
  at Phase C5, so all items are effectively module-private
  with no enforcement difference today.

### D2. Field declarations: `let name: T;` inside class blocks.

Fields use the existing `let` keyword + the C1.2 annotation
grammar. Differences vs free-`let`:

- Mandatory type annotation (no type inference — fields
  are part of the class's stored type).
- No initializer expression at the declaration site
  (initialization happens in `init`).
- Terminated with `;`.
- Optional `pub` visibility prefix.

Field order in the source determines memory layout order, same
as C1.4 structs. The desugaring at the typed-AST layer
generates a `TypedStructDecl { fields: Vec<...> }` underneath
the class.

### D3. Method declarations: `fn name(self: &Self, ...)` inside class blocks.

Methods reuse the existing `fn` keyword. The first parameter
*must* be `self: &Self` or `self: &mut Self` — methods
without a `self` receiver are deferred (static methods land
when traits arrive at C4.2 or in a later sub-phase).

The `Self` type refers to the class being defined; it's a
synthetic alias resolved at type-check time. Method bodies
follow the same effect-row + return-type conventions as free
fns. The receiver's borrow kind (`&Self` vs `&mut Self`)
determines what the method can do per ADR 0017 D6:

- `&Self`: shared read access to fields. Can call other
  `&Self`-taking methods on `self`.
- `&mut Self`: exclusive write access. Can assign to fields
  + call `&mut Self`-taking methods.

Method names share a namespace per class (no overloading at
C4.1). Self-recursive + mutually-recursive methods inside
the same class compile via the existing free-fn machinery.

### D4. `init(args) { body }` constructor + definite-assignment.

The `init` declaration is the constructor:

    init(x: i64, y: i64) {
        self.x = x;
        self.y = y;
    }

Inside `init`, `self` refers to the partially-initialised
class — fields are *unassigned* until written. The body:

- May read `self.field` *only after* the field has been
  assigned on every path leading to the read.
- Must definite-assign every field declared on the class
  before returning on every control-flow path.
- Has no explicit return value — `init` returns the
  fully-constructed `Self`.

Definite-assignment is a new dataflow analysis in
`sentinel-types`. It mirrors the existing borrow-check CFG
(per ADR 0017 D6) — per-branch snapshots + merge at if/else
join points. A field "assigned in both arms after the merge"
is treated as assigned post-merge; a field "assigned in only
one arm" surfaces as `InitFieldMaybeUnassigned`.

Fields read inside `init` before being assigned surface as
`InitFieldReadBeforeAssign`. Both errors carry a three-label
miette diagnostic (field decl + init body + the offending
read or post-return point).

At least one `init` is **mandatory** for a class with any
field; classes with zero fields can omit `init` (in which
case the constructor is a synthetic `init() { }`).

Multiple `init` overloads are deferred — a future ADR can
generalise.

### D5. Class instantiation: `Name::init(args)`.

To construct a class:

    let p: Point = Point::init(3, 4);

The `Class::init(args)` form mirrors the C1.4 free-fn call
shape (`do_work(args)` per ADR 0010). The `::` notation is a
new token sequence in the parser; codegen treats `Name::init`
as a synthetic FnId pointing at the init body.

ADR 0021 D3 floated `new Name(args)` as an alternative sugar;
**rejected at C4.1**. The `::init` form is consistent with
the existing fn-call discipline + parser doesn't need a new
keyword. If user testing surfaces friction, a follow-on ADR
can add `new` sugar.

The C1.4 struct-literal form `Name { field: value, ... }`
remains valid for plain structs (those declared via the
existing `struct` keyword). For classes (declared via
`class`), the struct-literal form is **rejected** — classes
must go through `init` per D4's no-half-constructed
invariant. Type-check surfaces `ClassConstructionMustUseInit`
when a class is the target of a struct-literal.

### D6. Field access + assignment: `obj.field` / `obj.field = v`.

Reuses the existing C1.4 postfix `.field` access (ADR 0013
D2) + C2.0.2 lvalue assignment (ADR 0017 D11). Inside method
bodies, `self.field` is the canonical pattern; the lvalue is
`self` (a `&Self` or `&mut Self` reference) followed by
`.field`.

Mutability rules:

- `self.field = v` requires `self: &mut Self`.
- `self.field` (read) works under both `&Self` and `&mut Self`.

The existing C2 borrow checker enforces these — no new
rules at C4.1.

### D7. Method dispatch: static against the receiver's class type.

A method call `obj.method(args)` lowers to:

  1. Resolve `obj`'s static type to a `Type::Class(ClassId)`.
  2. Look up `method` in the class's method table.
  3. Emit a direct fn call `@<mangled-class-method>(obj_ptr,
     args)`.

The auto-reference rule from ADR 0021 D2: if `method` takes
`self: &Self` and `obj`'s static type is `Class`, the call
auto-references `&obj`. If method takes `self: &mut Self`,
auto-references `&mut obj` (and the borrow check enforces
that `obj` is mutable).

No virtual dispatch at C4.1 — that's a `dyn Trait` matter for
post-C4.2 follow-on ADRs. Calling a method on `&Class` or
`&mut Class` lvalues directly (no auto-ref needed) works
identically.

### D8. `Self` and `self` resolution.

The `Self` type is a synthetic alias resolved at type-check
time. Inside a class block, `Self` resolves to
`Type::Class(this_class_id)`. Outside a class block, `Self`
is a resolve error (`SelfOutsideClassContext`).

The `self` value is the receiver binding inside method
bodies. It's a parameter (the first one) with a synthetic
VarId. Type: `&Self` or `&mut Self` depending on the method
signature. Inside `init`, `self` is also a parameter but its
type is `&mut PartiallyInitClass(class_id)` — a synthetic
type carrying per-field assigned-state for the definite-
assignment analysis. After `init` returns, the type erases
to `Class(class_id)`.

Both `Self` and `self` are syntactically distinct tokens (D2
of ADR 0021) — case-distinguished by the lexer. No ambiguity
at parse time.

### D9. Codegen: class as LLVM struct + per-class method namespace.

A class declaration compiles to:

  1. **An LLVM struct type** mirroring the field layout
     (reuses C1.4's struct-type cache).
  2. **A `Name__init` LLVM function** that takes the init's
     params + emits the body. The fn returns the constructed
     class by value; the caller `let p = Name::init(args)`
     binds it normally.
  3. **One LLVM function per method**, mangled as
     `Name__method`. Methods take their declared params
     (with `self` as a `ptr` to the class struct).

Method calls `obj.method(args)`:

  - Lower `obj`'s lvalue to a pointer via the existing C2.0.2
    `lower_lvalue_ptr` helper.
  - Emit `call @Name__method(obj_ptr, args)`.

Init calls `Name::init(args)`:

  - Allocate stack slot for the result class (alloca).
  - Pass alloca pointer as a synthetic first argument
    `out_ptr` (a pointer-passing convention to avoid
    returning structs by value across the ABI).
  - The init's emitted body writes through `out_ptr` for
    field assignments (`self.field = v` becomes
    `store v, gep(out_ptr, field_idx)`).
  - After init returns, the value at `out_ptr` is the
    constructed class. Load it into a temporary if needed.

Drop emission at scope-exit for class-valued bindings
follows the C2.4 / C2.5 rules — recursive field drops via
`emit_drop_struct_fields` already exists. Class drop is a
no-op extension since the underlying struct machinery
already runs.

### D10. Lexer (no new tokens at C4.1).

C4.0 reserved `class`, `init`, `Self`, `self`. C4.1 activates
them at the parser layer. No lexer changes.

### D11. Phase-go program.

At `tests/pass/c41_go_no_go.sentinel`. The full C4.1 surface
in one program:

    class Point {
        let x: i64;
        let y: i64;
        pub init(x: i64, y: i64) {
            self.x = x;
            self.y = y;
        }
        pub fn manhattan(self: &Self) -> i64 {
            self.x + self.y
        }
        pub fn translate(self: &mut Self, dx: i64, dy: i64) -> i64 {
            self.x = self.x + dx;
            self.y = self.y + dy;
            self.manhattan()
        }
    }

    fn main() -> i64 {
        let mut p: Point = Point::init(10, 20);
        p.translate(3, 9)
    }

Expected: exit code 42, no stdout.

Exercises:
- class declaration with multiple fields + init + two methods
  (one `&Self`, one `&mut Self`).
- init constructor with definite-assignment (both fields
  assigned on the single path).
- method call via `obj.method(args)` auto-ref + auto-mut-ref.
- self.field read + assign inside method bodies.
- inter-method calls (`self.manhattan()` from `translate`).
- the C2 borrow check on `&mut Self` methods (`let mut p`
  is required so `&mut p` is valid).

A secondary fixture `tests/pass/c41_class_basic.sentinel`
exercises the smallest possible class (single field, no
methods) to pin the minimum machinery.

A UI fixture `tests/ui/c41_init_field_unassigned.sentinel`
exercises the definite-assignment rejection — an init that
fails to assign one declared field surfaces
`InitFieldMaybeUnassigned`.

## Reasoning

The decisions cluster around three themes.

**Classes desugar to struct + methods + init.** D1-D3 + D9
keep the underlying typed AST compatible with C1.4 structs
+ free fns. The class block is a syntactic regrouping; the
compile pipeline reuses existing machinery wherever possible
(struct types, fn dispatch, postfix field access).

**Definite-assignment via dataflow.** D4 introduces the
first dataflow analysis specific to type-check (the borrow
checker has its own CFG). Init bodies are walked with a
per-field "assigned?" bitmap; if/else branches snapshot +
merge identically to the borrow-check pattern. The diagnostic
surfaces `InitFieldMaybeUnassigned` (post-return + per-arm
merge) and `InitFieldReadBeforeAssign` (mid-body reads).

**No-half-constructed via mandatory init.** D5's rejection of
struct-literal syntax for classes is the key invariant: every
class instance flows through `init`. Combined with D4's
definite-assignment, half-constructed instances cannot
materialise.

## Consequences

### Positive

- The class surface is the canonical declaration form for
  C4 and beyond (HANDOVER §6.2 lists class as the C4 surface
  marker). C4.1 ships the foundational piece on which traits
  (C4.2) + delegation (C4.3) build.
- Definite-assignment is a small, well-known dataflow
  analysis. The implementation reuses the existing CFG
  vocabulary from C2's borrow checker; the marginal cost is
  the per-field bitmap.
- Inter-method calls (`self.manhattan()` from `translate`)
  fall out of the existing method-dispatch machinery — no
  special-case typing.
- The class instance ABI (passed by pointer via `out_ptr` to
  init + as `ptr` self to methods) avoids large-struct-by-
  value perf concerns + matches how Rust's `&mut self`
  methods lower at LLVM IR.

### Negative

- ~600-900 LOC across crates (AST, parser, resolve, types,
  codegen). The largest single-sub-phase change since C1.7.
- The definite-assignment analysis adds typing-pass
  complexity. Generic classes will need to interact with
  monomorphisation (handled cleanly per C1.7 patterns but
  not free).
- The synthetic `PartiallyInitClass` type for the init body
  is a new type kind. The existing `Type` enum gains a
  variant — this expands the `Copy + Hash` invariant
  carefully (the new variant carries a `ClassId(u32)`, so
  Copy is preserved).
- Visibility (`pub`) is parsed but not enforced at C4.1. A
  later sub-phase or Phase C5 module-system work activates it;
  diagnostic for "private field accessed externally" is
  deferred.

### Neutral

- The existing struct surface (`struct Name { ... }`) stays
  valid. Classes are additive; existing programs continue
  to work.
- Method dispatch is static — no runtime overhead beyond
  a direct fn call. `dyn Trait` follow-on ADRs add dynamic
  dispatch with a witness-table indirection.
- The `::init` invocation form is consistent with potential
  future static methods (`Name::method(args)` for
  associated fns).

## Alternatives considered

- **`new Name(args)` sugar instead of `Name::init(args)`**
  (D5). Considered: aligns with most OOP languages' surface
  ergonomics. Rejected at C4.1: adds a new keyword + parser
  complexity for no semantic gain. The `::init` form
  composes with associated fns naturally.

- **Struct-literal syntax for classes** (D5). Rejected: the
  `Name { field: value }` form bypasses init, defeating the
  no-half-constructed invariant. Keeping it valid for
  classes would require parallel typing rules.

- **Optional `init` with implicit default** (D4). Considered
  for classes-with-all-defaultable-fields. Rejected: every
  field would need a default-expression (i64::default() = 0
  etc.), which is its own design surface. C4.1 requires
  explicit init.

- **Multiple `init` overloads** (D4). Considered: useful for
  classes with multiple construction paths. Rejected at C4.1:
  fn overloading is not in scope at the bootstrap-language
  level. Workaround: `Name::init_with_default()` /
  `Name::init_from_str()` style associated-fn naming once
  associated fns land.

- **Method dispatch via vtables at C4.1** (D7). Rejected:
  static dispatch covers C4.1's class-method-only surface.
  Virtual dispatch arrives with `dyn Trait` post-C4.2.

- **`Self` as a free identifier** (D8). Rejected: `Self` is
  a reserved keyword from C4.0; the parser-context check
  (`Self only inside class/trait blocks`) keeps the lexer
  simple while preserving the case-distinguished surface.

- **Lifting visibility (`pub`) enforcement at C4.1** (D2).
  Deferred: Sentinel's module system is Phase C5 work.
  Without modules, visibility has no enforcement substrate.
  Parsing `pub` reserves the surface for later.

## Amendments at C4.1 close

Two amendments document the gap between the as-drafted ADR
and what shipped at C4.1 (2/N). Both are carry-overs to a
future iteration; neither blocks Phase C4.2.

### A1. D4 definite-assignment is partial (flat any-assigned)

The drafted D4 specifies a dataflow analysis with per-arm
snapshots merged at if/else join points + a separate
`InitFieldReadBeforeAssign` check for mid-body reads. C4.1
(2/N) ships a **simpler check**: walk the init body, collect
the set of field names assigned via `self.field = expr`
anywhere in the body (stmts, tail, branches of conditionals),
and reject any declared field not in the set with
`InitFieldMaybeUnassigned`. This catches the obvious case
(field never written) — sufficient for the
`c41_init_field_unassigned` UI fixture and the phase-go's
linear init body. The branch-aware merge + the
`InitFieldReadBeforeAssign` mid-body check are deferred as
C4.1 follow-ons; the type-check pass surfaces neither at
C4.1 close. Reopen via a follow-on iteration when conditional-
init bodies surface in practice.

### A2. D8 `Self` in general type position deferred

The drafted D8 specifies `Self` as a synthetic alias that
resolves to `Type::Class(this_class_id)` anywhere inside a
class block — including method return types and field type
annotations. C4.1 (2/N) only supports `Self` positionally
inside `parse_self_param` (`self: &Self` / `self: &mut Self`).
General `Self` in type position is rejected at parse time
because `parse_type` doesn't handle the `SelfTy` token.

The phase-go fixture and c41_class_basic both use concrete
return types (i64), so the gap doesn't surface. Reopen when
a return-Self method shape surfaces (e.g., builder pattern,
chainable mutators).

## Revisit

This ADR is **ACCEPTED-WITH-AMENDMENTS at C4.1 close**.
Per-D status:

- **D1 (class declaration grammar)**: exercised. Generic
  classes (`class Pair<A, B>`) still deferred to a follow-on
  per the original D1 deferral; AST shape already supports
  the surface, only resolve+types+codegen monomorphisation
  is missing.
- **D2 (field declarations)**: exercised. Visibility parsed
  but not enforced — module system at Phase C5 will activate
  per D2.
- **D3 (method declarations)**: exercised. Methods support
  `&Self` + `&mut Self` receivers; method-call dispatch is
  static (D7).
- **D4 (init constructor + definite-assignment)**: exercised
  with amendment A1 — full dataflow deferred.
- **D5 (Name::init form)**: exercised. Struct-literal syntax
  for classes (`Point { x: 1 }`) NOT reached at type-check
  because resolve catches it as UndefinedStruct first;
  `ClassConstructionMustUseInit` declared but unreachable in
  practice — promote to a resolve-level detection in a
  follow-on iteration for clearer diagnostics.
- **D6 (field access + assignment)**: exercised. `self.field`
  + `self.field = v` reuse the C1.4 + C2 lvalue machinery
  via the FieldAccess Class arm.
- **D7 (method dispatch)**: exercised. Static dispatch only;
  dyn Trait post-C4.2.
- **D8 (Self / self resolution)**: exercised with amendment
  A2 — `self` (the value) fully supported including
  SelfOutsideClassContext; `Self` (the type) only via
  parse_self_param's positional form.
- **D9 (codegen out_ptr ABI + per-method namespace)**:
  exercised. Direct-return for small classes considered as a
  follow-on perf optimisation; out_ptr is the safe baseline.
- **D10 (no new lexer tokens at C4.1)**: exercised.
  `ColonColon` landed at C4.1 (1/N); no further tokens
  needed in (2/N).
- **D11 (phase-go program)**: exercised by
  `c41_go_no_go.sentinel` running end-to-end at exit 42.

Future revisit triggers (carried forward):

- **D1 (generic classes)**: revisit if user testing surfaces
  the need before C4.2 (the typing layer can accept
  `class Pair<A, B>` with modest work).
- **D4 (definite-assignment)**: revisit when branch-aware
  merge becomes necessary in practice. Candidate for
  refactoring into a shared analysis core with the C2
  borrow CFG.
- **D5 (Name::init form)**: revisit at first user-reported
  ergonomic friction. `new Name(args)` sugar remains the
  natural alternative.
- **D9 (codegen out_ptr ABI)**: revisit if struct-return-by-
  value performs adequately for classes-with-few-fields.

## Appendix: estimated implementation footprint

For session-budget planning. Numbers are rough; actual is
unknown until C4.1 ships.

  - **AST** (~150 LOC):
    - `ClassDecl { name, fields, init, methods, span }`.
    - `Init { params, body, span }`.
    - `Method { name, params, return_type, effect_row, body }`.
    - `Program.classes: Vec<ClassDecl>` alongside
      `program.fns` + `program.structs`.

  - **Parser** (~250 LOC):
    - `parse_class_decl` + `parse_class_item` (dispatching
      to field / init / method).
    - `parse_init_decl` + `parse_method_decl`.
    - `parse_self_param` for the first-param `self:
      &mut? Self` form.
    - `parse_postfix_method_call` extension (the existing
      postfix `.field` handler adds a path for `.name(args)`).
    - `parse_class_init_call` for `Name::init(args)` — a new
      `::` token sequence; the lexer currently has no `::`
      token, so add `ColonColon` at C4.1.0 OR parse two
      consecutive `Colon` tokens (cheaper, no lexer
      change). **Decision deferred to implementation** —
      probably ColonColon for clarity.

  - **Resolve** (~100 LOC):
    - `ClassId(u32)` interner + per-class scope.
    - Method-table population.
    - `Self` resolution inside class bodies.
    - `self` parameter binding to a synthetic VarId.

  - **Types** (~250-300 LOC):
    - `Type::Class(ClassId)` interner extension (preserves
      Copy + Hash).
    - Method signature check + self-param requirement.
    - Definite-assignment analysis for init bodies.
    - `InitFieldMaybeUnassigned` /
      `InitFieldReadBeforeAssign` /
      `ClassConstructionMustUseInit` /
      `SelfOutsideClassContext` error variants.

  - **Codegen** (~150 LOC):
    - LLVM struct type for each class (reuses struct cache).
    - Per-method LLVM fn generation.
    - Init lowering with `out_ptr` ABI.
    - Method call lowering with auto-ref of the receiver.

  - **Tests + fixtures** (~80 LOC):
    - c41_go_no_go.sentinel (D11).
    - c41_class_basic.sentinel.
    - c41_init_field_unassigned.sentinel (UI).
    - +unit tests across AST / parser / resolve / types /
      codegen.

  - **Total at C4.1 minimum**: ~700-1000 LOC across crates.
    Aligned with ADR 0021's D14 estimate of "2-3 sessions"
    for C4.1 — comparable to C1.4 (structs) or C1.7
    (generics) in volume.

After C4.1: **ADR 0023 PROPOSED** at C4.2 open — concrete
trait + impl surface syntax + dispatch resolution rules.
