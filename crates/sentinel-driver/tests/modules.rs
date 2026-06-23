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

// ===== ADR 0037 (a): TRUE per-unit separate compilation (`--separate`) =====

/// Run `snc build <entry> --separate -o <exe>` and return (success, stderr).
fn build_separate(entry: PathBuf) -> (bool, String) {
    let out = Command::new(env!("CARGO_BIN_EXE_snc"))
        .arg("build")
        .arg(&entry)
        .arg("--separate")
        .arg("-o")
        .arg(entry.with_extension(""))
        .output()
        .expect("run snc");
    (out.status.success(), String::from_utf8_lossy(&out.stderr).into_owned())
}

/// Build `entry` with `--separate`, assert it compiled, run it, return exit.
fn build_and_run_separate(entry: PathBuf) -> i32 {
    let (ok, stderr) = build_separate(entry.clone());
    assert!(ok, "expected a successful --separate build; stderr:\n{stderr}");
    let exe = entry.with_extension("");
    let run = Command::new(&exe).output().expect("run compiled binary");
    run.status.code().expect("process exited normally")
}

#[test]
fn separate_cross_module_call_compiles_and_runs() {
    // ADR 0037 (a) D10 phase-go: TRUE per-unit separate compilation.
    // `main.sentinel` uses a `pub fn` from `util/math.sentinel`; EACH module
    // compiles to its OWN object independently, and the cross-module call
    // resolves at LINK time via the module-qualified abi-v1 symbol
    // (`_S4util4math3add`). add(40, 2) -> exit 42.
    let dir = temp_project("separate");
    write(dir.join("util/math.sentinel"), "pub fn add(a: i64, b: i64) -> i64 { a + b }\n");
    write(
        dir.join("main.sentinel"),
        "use util::math::add;\nfn main() -> i64 { add(40, 2) }\n",
    );
    assert_eq!(build_and_run_separate(dir.join("main.sentinel")), 42);
    // Two objects were emitted INDEPENDENTLY (per-unit, not one merged .o).
    assert!(dir.join("main.o").exists(), "the entry unit's object should exist");
    assert!(
        dir.join("util_math.o").exists(),
        "the util::math unit's object should exist"
    );
}

#[test]
fn separate_cross_module_struct_compiles_and_runs() {
    // ADR 0037 D4 cross-module TYPES: a `pub struct` imported across units is
    // LAYOUT-only (no link symbol) — the importer RE-MATERIALIZES the decl in
    // its own type space (emits its own `%Point`). main imports Point +
    // constructs/reads it locally. Point { x: 40, y: 2 }: x + y -> exit 42.
    let dir = temp_project("separate_struct");
    write(dir.join("util/geo.sentinel"), "pub struct Point { x: i64, y: i64 }\n");
    write(
        dir.join("main.sentinel"),
        "use util::geo::Point;\nfn main() -> i64 { let p = Point { x: 40, y: 2 }; p.x + p.y }\n",
    );
    assert_eq!(build_and_run_separate(dir.join("main.sentinel")), 42);
    // Both units emitted their own object (the importer re-materializes Point).
    assert!(dir.join("main.o").exists(), "the entry unit's object should exist");
    assert!(dir.join("util_geo.o").exists(), "the util::geo unit's object should exist");
}

#[test]
fn separate_cross_module_enum_compiles_and_runs() {
    // Cross-module `pub enum` (ADR 0037 D4): same layout-only re-materialization
    // as a struct (the importer re-emits the `{tag, ptr}` shape). main imports
    // Shape + constructs/matches it locally (scalar payloads re-resolve in the
    // importer). Shape::Square(42) -> the Square arm -> exit 42.
    let dir = temp_project("separate_enum");
    write(dir.join("util/geo.sentinel"), "pub enum Shape { Circle(i64), Square(i64) }\n");
    write(
        dir.join("main.sentinel"),
        "use util::geo::Shape;\n\
         fn main() -> i64 { let s = Shape::Square(42); match s { Shape::Circle(r) => r, Shape::Square(v) => v } }\n",
    );
    assert_eq!(build_and_run_separate(dir.join("main.sentinel")), 42);
    assert!(dir.join("util_geo.o").exists(), "the util::geo unit's object should exist");
}

#[test]
fn separate_cross_module_struct_in_signature_compiles_and_runs() {
    // The type-in-signature case: a cross-module fn whose SIGNATURE takes an
    // imported struct (`sum(p: Point) -> i64`). main imports BOTH Point and
    // sum; it constructs a Point and passes it across the unit boundary. The
    // extern `sum`'s `Point` param re-resolves to main's LOCAL Point id, and
    // the C ABI passes the struct by value (both units agree on its layout).
    // sum(Point { x: 40, y: 2 }) = 40 + 2 -> exit 42.
    let dir = temp_project("separate_sig");
    write(
        dir.join("util/geo.sentinel"),
        "pub struct Point { x: i64, y: i64 }\n\
         pub fn sum(p: Point) -> i64 { p.x + p.y }\n",
    );
    write(
        dir.join("main.sentinel"),
        "use util::geo::Point;\nuse util::geo::sum;\n\
         fn main() -> i64 { sum(Point { x: 40, y: 2 }) }\n",
    );
    assert_eq!(build_and_run_separate(dir.join("main.sentinel")), 42);
    assert!(dir.join("util_geo.o").exists(), "the util::geo unit's object should exist");
}

#[test]
fn separate_cross_module_generic_fn_compiles_and_runs() {
    // ADR 0037 D6 cross-module GENERICS (2/N): a `pub fn id<T>` is INLINED
    // into the importer + monomorphized LOCALLY. The i64 instance is
    // collision-safe, so it now emits under the ORIGIN-qualified symbol
    // (`_S4util4math…id__i64`) with `linkonce_odr` linkage (deduped across
    // importers; see `separate_cross_module_generic_is_linkonce_deduped_*`).
    // main imports id + instantiates id<i64> (inferred from `42`). id(42) -> 42.
    let dir = temp_project("separate_generic");
    write(dir.join("util/math.sentinel"), "pub fn id<T>(x: T) -> T { x }\n");
    write(
        dir.join("main.sentinel"),
        "use util::math::id;\nfn main() -> i64 { id(42) }\n",
    );
    assert_eq!(build_and_run_separate(dir.join("main.sentinel")), 42);
    assert!(dir.join("main.o").exists(), "the entry unit's object should exist");
}

#[test]
fn separate_cross_module_generic_over_cross_module_struct_compiles_and_runs() {
    // Generics over a CROSS-MODULE struct work in the inline-local model:
    // main imports both `Point` (a struct) and `id<T>` (a generic), then
    // instantiates id<Point>. Because the instance is qualified by the
    // IMPORTER's path (`_S4main…id__Point`) and main self-contains both the
    // inlined Point + the instance, the type-tag-collision concern (which
    // would bite an ORIGIN-qualified linkonce_odr model) does not arise.
    // id(Point { 40, 2 }) returns it; q.x + q.y -> exit 42.
    let dir = temp_project("separate_gen_struct");
    write(
        dir.join("util/geo.sentinel"),
        "pub struct Point { x: i64, y: i64 }\npub fn id<T>(x: T) -> T { x }\n",
    );
    write(
        dir.join("main.sentinel"),
        "use util::geo::Point;\nuse util::geo::id;\n\
         fn main() -> i64 { let p = Point { x: 40, y: 2 }; let q = id(p); q.x + q.y }\n",
    );
    assert_eq!(build_and_run_separate(dir.join("main.sentinel")), 42);
}

#[test]
fn separate_cross_module_generic_is_linkonce_deduped_across_importers() {
    // ADR 0037 (2/N) `linkonce_odr`: TWO units both instantiate `util::id<i64>`.
    // A collision-safe instance is emitted under the ORIGIN-qualified symbol
    // (`_S4util4math…id__i64`) with `linkonce_odr` linkage, so the linker keeps
    // ONE definition. This is LOAD-BEARING two ways: (a) two EXTERNAL defs of
    // the same symbol would be a duplicate-symbol LINK ERROR, so a successful
    // link proves the linkage; (b) `nm` shows exactly one such symbol in the
    // exe — the inline-local model would emit two importer-qualified copies
    // (`_S4main…` + `_S6helper…`). id(0) + (id(21)+id(21)) -> 0 + 42 -> exit 42.
    let dir = temp_project("separate_linkonce");
    write(dir.join("util/math.sentinel"), "pub fn id<T>(x: T) -> T { x }\n");
    write(
        dir.join("helper.sentinel"),
        "use util::math::id;\npub fn doubled() -> i64 { id(21) + id(21) }\n",
    );
    write(
        dir.join("main.sentinel"),
        "use util::math::id;\nuse helper::doubled;\nfn main() -> i64 { id(0) + doubled() }\n",
    );
    let entry = dir.join("main.sentinel");
    assert_eq!(build_and_run_separate(entry.clone()), 42);
    // Exactly ONE definition of the origin-qualified instance survives the link.
    let exe = entry.with_extension("");
    let nm = Command::new("nm").arg(&exe).output().expect("run nm");
    let syms = String::from_utf8_lossy(&nm.stdout);
    let count = syms.lines().filter(|l| l.contains("id__i64")).count();
    assert_eq!(count, 1, "expected one linkonce_odr-deduped id<i64> symbol; nm:\n{syms}");
}

#[test]
fn separate_cross_module_generic_over_struct_is_linkonce_deduped() {
    // ADR 0037 (2/N) point 8: two units both instantiate `util::id<geo::Point>`
    // — a generic over a CROSS-MODULE struct. The mono key ORIGIN-qualifies the
    // struct tag (`id__geo$Point`), so the instance dedups under ONE
    // origin-qualified `linkonce_odr` symbol; a same-named `Point` from another
    // module would get a DISTINCT tag, so the dedup stays sound. The `nm`
    // assertion is load-bearing for the point-8 fix: WITHOUT it a struct arg is
    // not dedup-safe → two importer-qualified copies and NO `id__geo$Point`.
    // (19+2) + (20+1) -> 21 + 21 -> exit 42.
    let dir = temp_project("separate_linkonce_struct");
    write(dir.join("util/math.sentinel"), "pub fn id<T>(x: T) -> T { x }\n");
    write(dir.join("geo.sentinel"), "pub struct Point { x: i64, y: i64 }\n");
    write(
        dir.join("helper.sentinel"),
        "use util::math::id;\nuse geo::Point;\n\
         pub fn h() -> i64 { let p: Point = Point { x: 20, y: 1 }; let q: Point = id(p); q.x + q.y }\n",
    );
    write(
        dir.join("main.sentinel"),
        "use util::math::id;\nuse geo::Point;\nuse helper::h;\n\
         fn main() -> i64 { let p: Point = Point { x: 19, y: 2 }; let q: Point = id(p); q.x + q.y + h() }\n",
    );
    let entry = dir.join("main.sentinel");
    assert_eq!(build_and_run_separate(entry.clone()), 42);
    let exe = entry.with_extension("");
    let nm = Command::new("nm").arg(&exe).output().expect("run nm");
    let syms = String::from_utf8_lossy(&nm.stdout);
    let count = syms.lines().filter(|l| l.contains("id__geo$Point")).count();
    assert_eq!(count, 1, "expected one origin-qualified id__geo$Point symbol; nm:\n{syms}");
}

#[test]
fn separate_same_named_cross_module_structs_dont_alias_in_dedup() {
    // ADR 0037 (2/N) point 8 SOUNDNESS: two modules each define a DIFFERENT
    // `pub struct Point` (different layouts), and two importers instantiate
    // `util::id<Point>` over them. The origin-qualified mono tag keeps the two
    // instances DISTINCT (`id__a$Point` vs `id__b$Point`), so the linker does
    // NOT merge an 8-byte `id` with a 16-byte one — the exact unsoundness a
    // bare `id__Point` tag would cause. The `nm` assertion is load-bearing for
    // the fix: those origin-qualified tags exist ONLY when point 8 is active
    // (else struct args don't dedup → importer-qualified, no `id__a$Point`).
    // fa()=5, fb()=3+4 -> 5 + 7 -> exit 12.
    let dir = temp_project("separate_struct_tag_collision");
    write(dir.join("util/math.sentinel"), "pub fn id<T>(x: T) -> T { x }\n");
    write(dir.join("a.sentinel"), "pub struct Point { x: i64 }\n");
    write(dir.join("b.sentinel"), "pub struct Point { x: i64, y: i64 }\n");
    write(
        dir.join("usea.sentinel"),
        "use util::math::id;\nuse a::Point;\n\
         pub fn fa() -> i64 { let p: Point = Point { x: 5 }; let q: Point = id(p); q.x }\n",
    );
    write(
        dir.join("useb.sentinel"),
        "use util::math::id;\nuse b::Point;\n\
         pub fn fb() -> i64 { let p: Point = Point { x: 3, y: 4 }; let q: Point = id(p); q.x + q.y }\n",
    );
    write(
        dir.join("main.sentinel"),
        "use usea::fa;\nuse useb::fb;\nfn main() -> i64 { fa() + fb() }\n",
    );
    let entry = dir.join("main.sentinel");
    assert_eq!(build_and_run_separate(entry.clone()), 12);
    let exe = entry.with_extension("");
    let nm = Command::new("nm").arg(&exe).output().expect("run nm");
    let syms = String::from_utf8_lossy(&nm.stdout);
    assert!(
        syms.contains("id__a$Point") && syms.contains("id__b$Point"),
        "the two same-named structs must keep DISTINCT origin-qualified tags; nm:\n{syms}"
    );
}

#[test]
fn separate_cross_module_generic_over_enum_is_linkonce_deduped() {
    // ADR 0037 (2/N) point 8 (ENUMS): two units both instantiate
    // `util::id<shape::Shape>` — a generic over a CROSS-MODULE enum. The mono
    // key origin-qualifies the enum tag (`id__shape$Shape`), so the instance
    // dedups under ONE origin-qualified `linkonce_odr` symbol (the same
    // mechanism as structs). `nm`=1 is load-bearing: without the fix an enum
    // arg isn't dedup-safe → two importer-qualified copies, no `id__shape$Shape`.
    // 21 + 21 -> exit 42.
    let dir = temp_project("separate_linkonce_enum");
    write(dir.join("util/math.sentinel"), "pub fn id<T>(x: T) -> T { x }\n");
    write(dir.join("shape.sentinel"), "pub enum Shape { Circle(i64), Square(i64) }\n");
    write(
        dir.join("helper.sentinel"),
        "use util::math::id;\nuse shape::Shape;\n\
         pub fn h() -> i64 {\n\
         \x20   let s: Shape = Shape::Circle(20);\n\
         \x20   let r: Shape = id(s);\n\
         \x20   match r { Shape::Circle(n) => n + 1, Shape::Square(n) => n }\n\
         }\n",
    );
    write(
        dir.join("main.sentinel"),
        "use util::math::id;\nuse shape::Shape;\nuse helper::h;\n\
         fn main() -> i64 {\n\
         \x20   let s: Shape = Shape::Square(21);\n\
         \x20   let r: Shape = id(s);\n\
         \x20   let v: i64 = match r { Shape::Circle(n) => n, Shape::Square(n) => n };\n\
         \x20   v + h()\n\
         }\n",
    );
    let entry = dir.join("main.sentinel");
    assert_eq!(build_and_run_separate(entry.clone()), 42);
    let exe = entry.with_extension("");
    let nm = Command::new("nm").arg(&exe).output().expect("run nm");
    let syms = String::from_utf8_lossy(&nm.stdout);
    let count = syms.lines().filter(|l| l.contains("id__shape$Shape")).count();
    assert_eq!(count, 1, "expected one origin-qualified id__shape$Shape symbol; nm:\n{syms}");
}

#[test]
fn separate_cross_module_trait_compiles_and_runs() {
    // Cross-module `pub trait`: defined in `counter`, IMPORTED into the entry
    // which impls it for its OWN class + dispatches. The trait decl is
    // re-materialized (no link symbol); the entry's class init/method + impl
    // method emit under its module-qualified symbols (`_S4main…`). The defining
    // `counter` unit has no code (a trait is a decl). t.tick(42): 0 + 42 -> 42.
    let dir = temp_project("separate_trait");
    write(
        dir.join("counter.sentinel"),
        "pub trait Counter {\n    fn tick(self: &mut Self, n: i64) -> i64;\n}\n",
    );
    write(
        dir.join("main.sentinel"),
        "use counter::Counter;\n\
         class Tally {\n    let count: i64;\n    pub init() { self.count = 0; 0 }\n}\n\
         impl as Counter for Tally {\n\
         \x20   fn tick(self: &mut Self, n: i64) -> i64 { self.count = self.count + n; self.count }\n\
         }\n\
         fn main() -> i64 { let mut t: Tally = Tally::init(); t.tick(42) }\n",
    );
    assert_eq!(build_and_run_separate(dir.join("main.sentinel")), 42);
}

#[test]
fn separate_cross_module_effect_decl_compiles_and_runs() {
    // Cross-module `pub effect` DECL (first cut): `io` defines the effect;
    // the entry imports it and both PERFORMS (in `do_work`) + HANDLES it.
    // Because perform + handle live in the SAME unit, the effect's EffectId
    // (→ runtime `op_id = (eid<<16)|op`) is consistent — pure inline, no
    // codegen/eid-portability change. (Cross-UNIT perform/handle — a library
    // performs, the entry handles — needs eid portability, a later piece.)
    // The handler resumes Io.read with 42 -> exit 42.
    let dir = temp_project("separate_effect");
    write(dir.join("io.sentinel"), "pub effect Io {\n    read() -> i64;\n}\n");
    write(
        dir.join("main.sentinel"),
        "use io::Io;\n\
         fn do_work() -> i64 ! { Io } { perform Io.read() }\n\
         fn main() -> i64 { handle do_work() with { Io.read(k) => k(42) } }\n",
    );
    assert_eq!(build_and_run_separate(dir.join("main.sentinel")), 42);
}

#[test]
fn separate_cross_unit_perform_handle_compiles_and_runs() {
    // The HARD case (ADR 0037 2/N): a library `io::source` PERFORMS `Io.read`
    // in ITS OWN unit; the entry HANDLES it in ANOTHER unit. The two units
    // compile to independent objects and number their effects LOCALLY, so the
    // runtime op id can only agree via the build-wide op-id base map.
    //
    // This test is deliberately LOAD-BEARING (the recipe's TEST-TRAP: a naive
    // single-arm handler passes even with a WRONG op id, because its `default`
    // is `unreachable` and LLVM collapses the lone arm to run unconditionally,
    // ignoring the id). Here the entry handler lists arms for TWO effects: its
    // OWN `Local` (local EffectId 0) and the imported `Io` (local EffectId 1,
    // since `Local` is declared first). The performed kont carries `io`'s
    // encoding of `Io.read`. WITHOUT the base map, the entry would encode
    // `Local.tick` as op id 0 and `Io.read` as `1<<16`, so the kont's id (0)
    // would COLLIDE with the `Local` arm and resume `k(7)` -> exit 7 (a silent
    // miscompile). WITH the base map, every unit maps `Io` -> base 0 and
    // `Local` -> base 1, so the kont selects the `Io.read` arm -> `k(40)` ->
    // exit 40. (Verified by hand: removing the base map flips 40 -> 7.)
    let dir = temp_project("separate_xunit_effect");
    write(
        dir.join("io.sentinel"),
        "pub effect Io {\n    read() -> i64;\n}\n\
         pub fn source() -> i64 ! { Io } { perform Io.read() }\n",
    );
    write(
        dir.join("main.sentinel"),
        "use io::Io;\nuse io::source;\n\
         effect Local {\n    tick() -> i64;\n}\n\
         fn main() -> i64 {\n\
         \x20   handle source() with {\n\
         \x20       Local.tick(k) => k(7),\n\
         \x20       Io.read(k) => k(40),\n\
         \x20   }\n\
         }\n",
    );
    assert_eq!(build_and_run_separate(dir.join("main.sentinel")), 40);
}

#[test]
fn separate_effecting_import_without_its_effect_is_rejected() {
    // Importing an effecting `pub fn` requires importing its effect too (so
    // it is in scope to be handled) — the effect analogue of importing a type
    // a signature references. `main` imports `io::source` (effect `Io`) but
    // not `io::Io`, so re-resolving the extern's row in check_module fails
    // with a clean `UnknownImportedEffect`, not a panic or a silent miscompile.
    let dir = temp_project("separate_xunit_effect_missing");
    write(
        dir.join("io.sentinel"),
        "pub effect Io {\n    read() -> i64;\n}\n\
         pub fn source() -> i64 ! { Io } { perform Io.read() }\n",
    );
    write(dir.join("main.sentinel"), "use io::source;\nfn main() -> i64 { source() }\n");
    let (ok, stderr) = build_separate(dir.join("main.sentinel"));
    assert!(!ok, "an effecting import without its effect should fail; stderr:\n{stderr}");
    assert!(
        stderr.contains("uses effect `Io`, which is not in scope"),
        "expected an UnknownImportedEffect naming `Io`; stderr:\n{stderr}"
    );
}

#[test]
fn separate_import_of_private_item_is_rejected() {
    // The visibility gate (PrivateItem) runs in --separate too, before any
    // per-unit compilation.
    let dir = temp_project("separate_private");
    write(dir.join("util.sentinel"), "fn add(a: i64, b: i64) -> i64 { a + b }\n");
    write(dir.join("main.sentinel"), "use util::add;\nfn main() -> i64 { 0 }\n");
    let (ok, stderr) = build_separate(dir.join("main.sentinel"));
    assert!(!ok, "a private import should fail --separate; stderr:\n{stderr}");
    assert!(
        stderr.contains("private to module `util`"),
        "expected a PrivateItem error naming `util`; stderr:\n{stderr}"
    );
}

#[test]
fn separate_use_of_missing_module_is_module_not_found() {
    // A missing `use`d module file is ModuleNotFound at discovery, in
    // --separate too.
    let dir = temp_project("separate_missing");
    write(dir.join("main.sentinel"), "use absent::thing;\nfn main() -> i64 { 0 }\n");
    let (ok, stderr) = build_separate(dir.join("main.sentinel"));
    assert!(!ok, "a missing module should fail --separate; stderr:\n{stderr}");
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

#[test]
fn cross_module_unhandled_effect_in_main_is_rejected() {
    // Effect-check parity for the merged path: `main` calls the effecting
    // `io::source` WITHOUT a `handle`, so `Io` bubbles unhandled to `main` —
    // rejected (ADR 0019 D13). Before effect-check was wired into the merged
    // path this slipped through to codegen; now it's a clean build failure.
    let dir = temp_project("xeffect_unhandled");
    write(
        dir.join("io.sentinel"),
        "pub effect Io {\n    read() -> i64;\n}\n\
         pub fn source() -> i64 ! { Io } { perform Io.read() }\n",
    );
    write(
        dir.join("main.sentinel"),
        "use io::source;\nfn main() -> i64 { source() }\n",
    );
    let (ok, stderr) = build(dir.join("main.sentinel"));
    assert!(
        !ok,
        "an unhandled effect in main should fail the merged build; stderr:\n{stderr}"
    );
    assert!(
        stderr.contains("unhandled effect"),
        "expected an UnhandledEffect diagnostic; stderr:\n{stderr}"
    );
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
