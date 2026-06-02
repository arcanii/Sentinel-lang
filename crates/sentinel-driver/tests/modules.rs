//! Phase D.6 (1/N) / ADR 0037: multi-file module-graph discovery.
//!
//! `snc build <entry>` follows `use` edges from the entry file, mapping
//! each `use a::b::Item;` to module `a::b` → file `<root>/a/b.sentinel`
//! (the source root is the entry's directory; the last path segment is
//! the imported item). These tests drive the real `snc` binary on temp
//! multi-file projects. Multi-module compilation (per-unit resolve +
//! separate codegen + link) is the NEXT D.6 (1/N) increment, so for now
//! a discovered multi-module graph is reported + gated; this verifies the
//! discovery itself — the path→file mapping and the ModuleNotFound edge.

use std::path::PathBuf;
use std::process::Command;

/// A fresh temp project directory (unique per test + process, so the
/// parallel test runner never collides). Best-effort cleared first.
fn temp_project(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("snc_d6_{}_{}", std::process::id(), name));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("create temp project dir");
    dir
}

fn write(path: PathBuf, contents: &str) {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).expect("create parent dir");
    }
    std::fs::write(path, contents).expect("write source file");
}

/// Run `snc build <entry>` (output next to the entry) and return
/// (success, stderr).
fn build(entry: PathBuf) -> (bool, String) {
    let out = Command::new(env!("CARGO_BIN_EXE_snc"))
        .arg("build")
        .arg(&entry)
        .arg("-o")
        .arg(entry.with_extension(""))
        .output()
        .expect("run snc");
    (out.status.success(), String::from_utf8_lossy(&out.stderr).into_owned())
}

/// Build `entry`, assert it compiled, run the binary, and return its exit
/// code (the program's tail value, the "exit-code-is-the-answer" rule).
fn build_and_run(entry: PathBuf) -> i32 {
    let (ok, stderr) = build(entry.clone());
    assert!(ok, "expected a successful multi-file build; stderr:\n{stderr}");
    let exe = entry.with_extension("");
    let run = Command::new(&exe).output().expect("run compiled binary");
    run.status.code().expect("process exited normally")
}

#[test]
fn cross_module_call_compiles_and_runs() {
    // The payoff: `use util::add;` + a cross-module call compiles to a
    // binary and runs. add(2, 3) -> exit 5.
    let dir = temp_project("multi");
    write(dir.join("util.sentinel"), "pub fn add(a: i64, b: i64) -> i64 { a + b }\n");
    write(dir.join("main.sentinel"), "use util::add;\nfn main() -> i64 { add(2, 3) }\n");
    assert_eq!(build_and_run(dir.join("main.sentinel")), 5);
}

#[test]
fn nested_module_path_compiles_and_runs() {
    // ADR 0037 open point 4: `use util::math::add;` → module `util::math`
    // → file `util/math.sentinel` (the last segment, `add`, is the item).
    let dir = temp_project("nested");
    write(dir.join("util/math.sentinel"), "pub fn add(a: i64, b: i64) -> i64 { a + b }\n");
    write(dir.join("main.sentinel"), "use util::math::add;\nfn main() -> i64 { add(10, 5) }\n");
    assert_eq!(build_and_run(dir.join("main.sentinel")), 15);
}

#[test]
fn private_names_across_modules_do_not_collide() {
    // The point of qualification: two modules each declare a private
    // `helper`; they must not clash, and a `pub fn` calls its OWN module's
    // private. util::compute(4) = helper(4)*10 + 1 = 41; main's own
    // (unused) `helper` returns 99.
    let dir = temp_project("collide");
    write(
        dir.join("util.sentinel"),
        "pub fn compute(x: i64) -> i64 { helper(x) + 1 }\nfn helper(x: i64) -> i64 { x * 10 }\n",
    );
    write(
        dir.join("main.sentinel"),
        "use util::compute;\nfn helper() -> i64 { 99 }\nfn main() -> i64 { compute(4) }\n",
    );
    assert_eq!(build_and_run(dir.join("main.sentinel")), 41);
}

#[test]
fn import_of_private_item_is_rejected() {
    // ADR 0037 D3: importing a non-`pub` item is a visibility error,
    // surfaced before the compilation gate.
    let dir = temp_project("private");
    write(dir.join("util.sentinel"), "fn add(a: i64, b: i64) -> i64 { a + b }\n");
    write(dir.join("main.sentinel"), "use util::add;\nfn main() -> i64 { 0 }\n");
    let (ok, stderr) = build(dir.join("main.sentinel"));
    assert!(!ok, "a private import should fail the build; stderr:\n{stderr}");
    assert!(
        stderr.contains("private to module `util`"),
        "expected a PrivateItem error naming `util`; stderr:\n{stderr}"
    );
}

#[test]
fn use_of_missing_module_is_module_not_found() {
    // A `use` whose module file does not exist is surfaced at discovery
    // (before resolve) as a clear ModuleNotFound, naming the expected file.
    let dir = temp_project("missing");
    write(dir.join("main.sentinel"), "use absent::thing;\nfn main() -> i64 { 0 }\n");
    let (ok, stderr) = build(dir.join("main.sentinel"));
    assert!(!ok, "a missing module should fail the build; stderr:\n{stderr}");
    assert!(
        stderr.contains("module `absent` not found"),
        "expected a ModuleNotFound naming `absent`; stderr:\n{stderr}"
    );
}

// --- D.6 cross-module TYPES (structs + enums) -------------------------------
// merge_modules now qualifies struct + enum names by module path and
// rewrites every type reference (annotations, struct literals, enum
// construction / patterns, fn signatures), so a `pub` data type can cross
// a module boundary and same-named types coexist.

#[test]
fn cross_module_struct_compiles_and_runs() {
    // A `pub struct` defined in `geo` is imported into the entry and used
    // as a `let` annotation, a struct literal, and a cross-module fn arg;
    // `geo::sum` reads its fields. Point{30,12} -> 30+12 -> exit 42.
    let dir = temp_project("xstruct");
    write(
        dir.join("geo.sentinel"),
        "pub struct Point { x: i64, y: i64 }\npub fn sum(p: Point) -> i64 { p.x + p.y }\n",
    );
    write(
        dir.join("main.sentinel"),
        "use geo::Point;\nuse geo::sum;\n\
         fn main() -> i64 {\n    let p: Point = Point { x: 30, y: 12 };\n    sum(p)\n}\n",
    );
    assert_eq!(build_and_run(dir.join("main.sentinel")), 42);
}

#[test]
fn cross_module_enum_with_match_compiles_and_runs() {
    // A `pub enum` crosses the boundary: the entry constructs its variants
    // (`Shape::Circle(3)` / `Shape::Square(5)`) and `shape::area` matches
    // over the qualified enum. 3*3*3 + 5*5 = 27 + 25 -> exit 52.
    let dir = temp_project("xenum");
    write(
        dir.join("shape.sentinel"),
        "pub enum Shape { Circle(i64), Square(i64) }\n\
         pub fn area(s: Shape) -> i64 {\n\
         \x20   match s {\n\
         \x20       Shape::Circle(r) => r * r * 3,\n\
         \x20       Shape::Square(side) => side * side,\n\
         \x20   }\n\
         }\n",
    );
    write(
        dir.join("main.sentinel"),
        "use shape::Shape;\nuse shape::area;\n\
         fn main() -> i64 {\n\
         \x20   let c: Shape = Shape::Circle(3);\n\
         \x20   let sq: Shape = Shape::Square(5);\n\
         \x20   area(c) + area(sq)\n\
         }\n",
    );
    assert_eq!(build_and_run(dir.join("main.sentinel")), 52);
}

#[test]
fn same_named_structs_across_modules_do_not_collide() {
    // The struct analog of the private-fn collision test: modules `a` and
    // `b` each declare a private `struct Item` with DIFFERENT fields. They
    // must coexist after qualification (`a$Item` vs `b$Item`); each fn
    // builds its own. from_a()=2, from_b()=40 -> exit 42.
    let dir = temp_project("xcollide");
    write(
        dir.join("a.sentinel"),
        "struct Item { v: i64 }\npub fn from_a() -> i64 { let it: Item = Item { v: 2 }; it.v }\n",
    );
    write(
        dir.join("b.sentinel"),
        "struct Item { w: i64 }\npub fn from_b() -> i64 { let it: Item = Item { w: 40 }; it.w }\n",
    );
    write(
        dir.join("main.sentinel"),
        "use a::from_a;\nuse b::from_b;\nfn main() -> i64 { from_a() + from_b() }\n",
    );
    assert_eq!(build_and_run(dir.join("main.sentinel")), 42);
}

#[test]
fn cross_module_struct_as_field_type_compiles_and_runs() {
    // Three modules deep: `inner::Inner` is used as a FIELD TYPE inside
    // `outer::Outer` (so the field's TypeExpr must be qualified across the
    // boundary), and the entry constructs the nested literal + reads
    // `o.inner.n`. Outer{Inner{40},2} -> 40+2 -> exit 42.
    let dir = temp_project("xfield");
    write(dir.join("inner.sentinel"), "pub struct Inner { n: i64 }\n");
    write(
        dir.join("outer.sentinel"),
        "use inner::Inner;\npub struct Outer { inner: Inner, extra: i64 }\n\
         pub fn total(o: Outer) -> i64 { o.inner.n + o.extra }\n",
    );
    write(
        dir.join("main.sentinel"),
        "use outer::Outer;\nuse outer::total;\nuse inner::Inner;\n\
         fn main() -> i64 {\n\
         \x20   let o: Outer = Outer { inner: Inner { n: 40 }, extra: 2 };\n\
         \x20   total(o)\n\
         }\n",
    );
    assert_eq!(build_and_run(dir.join("main.sentinel")), 42);
}

// --- D.6 cross-module TRAITS / EFFECTS / CLASSES ----------------------------
// merge_modules now qualifies trait / effect / class / named-impl names too
// and rewrites their reference sites (impl `as Trait for Type` heads,
// `perform`/`handle` effect names, effect rows, delegate trait names), so a
// `pub` trait/effect crosses a boundary and same-named ones coexist.

#[test]
fn cross_module_trait_compiles_and_runs() {
    // A `pub trait` defined in `counter` is imported into `tally`, which
    // impls it for its own class and dispatches `t.tick(42)` via the
    // receiver-typed default impl. The trait ref (`impl as Counter …`)
    // must resolve to the qualified `counter$Counter`. run(42) -> exit 42.
    let dir = temp_project("xtrait");
    write(
        dir.join("counter.sentinel"),
        "pub trait Counter {\n    fn tick(self: &mut Self, n: i64) -> i64;\n}\n",
    );
    write(
        dir.join("tally.sentinel"),
        "use counter::Counter;\n\
         class Tally {\n    let count: i64;\n    pub init() { self.count = 0; 0 }\n}\n\
         impl as Counter for Tally {\n\
         \x20   fn tick(self: &mut Self, n: i64) -> i64 { self.count = self.count + n; self.count }\n\
         }\n\
         pub fn run(n: i64) -> i64 { let mut t: Tally = Tally::init(); t.tick(n) }\n",
    );
    write(
        dir.join("main.sentinel"),
        "use tally::run;\nfn main() -> i64 { run(42) }\n",
    );
    assert_eq!(build_and_run(dir.join("main.sentinel")), 42);
}

#[test]
fn same_named_classes_across_modules_do_not_collide() {
    // Classes can't be `pub` (module-local), but same-named ones in two
    // modules must coexist after qualification (`a$Holder` vs `b$Holder`),
    // each with its own `init` + method. from_a()=2, from_b()=40 -> exit 42.
    let dir = temp_project("xclass");
    write(
        dir.join("a.sentinel"),
        "class Holder { let v: i64; pub init() { self.v = 2; 0 } \
         pub fn get(self: &Self) -> i64 { self.v } }\n\
         pub fn from_a() -> i64 { let h: Holder = Holder::init(); h.get() }\n",
    );
    write(
        dir.join("b.sentinel"),
        "class Holder { let w: i64; pub init() { self.w = 40; 0 } \
         pub fn get(self: &Self) -> i64 { self.w } }\n\
         pub fn from_b() -> i64 { let h: Holder = Holder::init(); h.get() }\n",
    );
    write(
        dir.join("main.sentinel"),
        "use a::from_a;\nuse b::from_b;\nfn main() -> i64 { from_a() + from_b() }\n",
    );
    assert_eq!(build_and_run(dir.join("main.sentinel")), 42);
}

#[test]
fn cross_module_effect_compiles_and_runs() {
    // A `pub effect` crosses the boundary: `io::source` performs `Io.read`
    // (its effect row + perform name qualify to `io$Io`), and the entry
    // `handle`s the same qualified effect, resuming with 42 -> exit 42.
    // (The merged path skips effect-check, but a well-formed effectful
    // program still resolves + lowers + runs.)
    let dir = temp_project("xeffect");
    write(
        dir.join("io.sentinel"),
        "pub effect Io {\n    read() -> i64;\n}\n\
         pub fn source() -> i64 ! { Io } { perform Io.read() }\n",
    );
    write(
        dir.join("main.sentinel"),
        "use io::Io;\nuse io::source;\n\
         fn main() -> i64 {\n\
         \x20   handle source() with {\n\
         \x20       Io.read(k) => k(42)\n\
         \x20   }\n\
         }\n",
    );
    assert_eq!(build_and_run(dir.join("main.sentinel")), 42);
}

// --- D.6 cross-module GENERICS (Path A whole-program mono) -------------------
// Under Path A the merged graph is one Program, so `collect_mono_instantiations`
// runs whole-program and a generic instantiated in a *different* module than its
// definition is discovered + emitted like any single-file instance. These pin
// that it works (the true per-unit `linkonce_odr` story is ADR 0037 (2/N)).

#[test]
fn cross_module_generic_struct_compiles_and_runs() {
    // `pub struct Box<T>` defined in `boxmod`, instantiated `Box<i64>` in the
    // entry (annotation + literal + field access). Box{42} -> 42 -> exit 42.
    let dir = temp_project("xgenstruct");
    write(dir.join("boxmod.sentinel"), "pub struct Box<T> { value: T }\n");
    write(
        dir.join("main.sentinel"),
        "use boxmod::Box;\n\
         fn main() -> i64 { let b: Box<i64> = Box { value: 42 }; b.value }\n",
    );
    assert_eq!(build_and_run(dir.join("main.sentinel")), 42);
}

#[test]
fn cross_module_generic_fn_compiles_and_runs() {
    // `pub fn id<T>` defined in `gen`, instantiated at i64 from the entry.
    // The whole-program mono pass must emit `gen$id` for i64. id(42) -> 42.
    let dir = temp_project("xgenfn");
    write(dir.join("gen.sentinel"), "pub fn id<T>(x: T) -> T { x }\n");
    write(
        dir.join("main.sentinel"),
        "use gen::id;\nfn main() -> i64 { id(42) }\n",
    );
    assert_eq!(build_and_run(dir.join("main.sentinel")), 42);
}

#[test]
fn cross_module_generic_fns_over_generic_struct_compile_and_run() {
    // Multi-param generics across the boundary: `Pair<A, B>` + `make_pair` +
    // `fst` in `pairmod`, instantiated `Pair<i64, i64>` from the entry.
    // fst(make_pair(42, 99)) -> 42 -> exit 42.
    let dir = temp_project("xgenpair");
    write(
        dir.join("pairmod.sentinel"),
        "pub struct Pair<A, B> { first: A, second: B }\n\
         pub fn make_pair<A, B>(a: A, b: B) -> Pair<A, B> { Pair { first: a, second: b } }\n\
         pub fn fst<A, B>(p: Pair<A, B>) -> A { p.first }\n",
    );
    write(
        dir.join("main.sentinel"),
        "use pairmod::Pair;\nuse pairmod::make_pair;\nuse pairmod::fst;\n\
         fn main() -> i64 { let p: Pair<i64, i64> = make_pair(42, 99); fst(p) }\n",
    );
    assert_eq!(build_and_run(dir.join("main.sentinel")), 42);
}
