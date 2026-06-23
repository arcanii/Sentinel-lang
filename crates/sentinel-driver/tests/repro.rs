//! Reproducible-build regression tests — ADR 0025 D8 / HANDOVER §7.
//!
//! Compiling the same source in two independent `snc` processes must
//! produce a byte-identical object file. This is a hard prerequisite
//! for the Phase D self-hosting fixed point (a compiler that, fed its
//! own source, yields a byte-identical binary).
//!
//! Why this can regress: `sentinel-codegen` uses `std::HashMap`, whose
//! iteration order is randomized per process. Today every such map is
//! a *lookup* table — emission walks the source-ordered `Vec`s in the
//! typed program (`fns`, `structs`, `classes`, `impls`, monomorphized
//! instances, captured-var lists), so two processes emit identical IR
//! and LLVM lowers it identically (mach-O objects embed no timestamp).
//! The C5.0 D8 audit confirmed this empirically across the full
//! C0–C4 surface. If a future stage (e.g. the C5.1 HIR/MIR lowering)
//! iterates a `HashMap`/`HashSet` to *emit* declarations, two
//! processes will pick different orders and these tests fail — surfacing
//! the nondeterminism before it ships. The fix in that case is to emit
//! from a sorted/`BTreeMap` order or a deterministically-ordered `Vec`.
//!
//! The object (`.o`) is compared, not the linked executable: the object
//! is the codegen artifact under test, whereas the linker may add its
//! own metadata. snc writes `<output>.o` beside the executable and
//! leaves it on disk.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Command;

fn workspace_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("crate dir has a parent")
        .parent()
        .expect("crates/ has a parent")
        .to_path_buf()
}

fn snc_binary() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_snc"))
}

/// Compile `tests/pass/<fixture>` in a fresh `snc` process (hence a
/// fresh HashMap seed) and return the emitted object bytes. The output
/// path varies per `tag` but does not affect the object — confirmed by
/// the D8 audit, since the source path is identical across the pair and
/// the object is purely a function of source content.
fn compile_object(fixture: &str, tag: &str) -> Vec<u8> {
    let root = workspace_root();
    let out_dir = root.join("target/sentinel-repro");
    std::fs::create_dir_all(&out_dir).expect("create build dir");
    let out = out_dir.join(tag);
    let status = Command::new(snc_binary())
        .current_dir(&root)
        .arg("build")
        .arg(format!("tests/pass/{fixture}"))
        .arg("-o")
        .arg(&out)
        .status()
        .expect("snc invocation failed");
    assert!(status.success(), "snc build {fixture} failed");
    std::fs::read(out.with_extension("o")).expect("read object file")
}

/// Two independent compilations of the same source must be byte-identical.
fn assert_reproducible(fixture: &str) {
    let stem = fixture.trim_end_matches(".sentinel");
    let a = compile_object(fixture, &format!("{stem}_a"));
    let b = compile_object(fixture, &format!("{stem}_b"));
    assert_eq!(
        a, b,
        "object for `{fixture}` is not byte-identical across two \
         independent compilations — codegen is nondeterministic \
         (most likely a HashMap/HashSet iterated to emit declarations; \
         emit from a sorted order instead)"
    );
}

/// Declare a reproducibility test for one `tests/pass/` fixture.
macro_rules! repro {
    ($name:ident, $fixture:literal) => {
        #[test]
        fn $name() {
            assert_reproducible($fixture);
        }
    };
}

// A cross-section of the codegen surface — each exercises a different
// family of codegen tables (per-class/impl method maps, the
// monomorphization worklist + `visited` set, per-resumer captured-state
// layout, drop-plan emission):
repro!(repro_c4_full_surface, "c4_go_no_go.sentinel");
repro!(repro_c17_generics_monomorphization, "c17_go_no_go.sentinel");
repro!(repro_c35e_chained_perform_with_capture, "c35e_chained_perform_with_capture.sentinel");
repro!(repro_c36b_nested_handle, "c36b_nested_handle_basic.sentinel");
repro!(repro_c24_raii_drop, "c24_go_no_go.sentinel");

// ===== ADR 0037 (3/N): per-unit `.o` reproducibility (`--separate`) =====
//
// Incremental caching (3/N) reuses an unchanged unit's cached object on the
// next build, so EACH separately-compiled unit's `.o` must be byte-identical
// across two independent compilations of the same source — the multi-file
// analogue of the single-file checks above. If a future change emits a unit
// from a HashMap/HashSet iteration order, two `snc` processes (two map seeds)
// diverge and this fails, surfacing the nondeterminism before caching ships.

/// Write a small multi-file project under `root` exercising the `--separate`
/// per-unit surface: a cross-module extern fn (`add`), a cross-module struct
/// (`Point`), and a generic instantiated over both a primitive and that struct
/// (`id<i64>` / `id<Point>` → origin-qualified `linkonce_odr` symbols, the
/// newest per-unit codegen path). Returns the entry path.
fn write_separate_project(root: &Path) -> PathBuf {
    let w = |rel: &str, src: &str| {
        let p = root.join(rel);
        std::fs::create_dir_all(p.parent().expect("rel has a parent")).expect("mkdir");
        std::fs::write(&p, src).expect("write source");
    };
    w(
        "util/math.sentinel",
        "pub fn id<T>(x: T) -> T { x }\npub fn add(a: i64, b: i64) -> i64 { a + b }\n",
    );
    w("geo.sentinel", "pub struct Point { x: i64, y: i64 }\n");
    w(
        "main.sentinel",
        "use util::math::id;\nuse util::math::add;\nuse geo::Point;\n\
         fn main() -> i64 {\n\
         \x20   let p: Point = Point { x: 1, y: 1 };\n\
         \x20   let q: Point = id(p);\n\
         \x20   add(id(20), q.x + q.y + 18) + 2\n\
         }\n",
    );
    root.join("main.sentinel")
}

/// Build `entry` with `--separate` into a fresh `out` dir and return every
/// emitted per-unit object keyed by file name (`main.o`, `util_math.o`, …).
/// Each build is a fresh `snc` process (hence a fresh HashMap seed).
fn compile_separate_objects(entry: &Path, out: &Path) -> BTreeMap<String, Vec<u8>> {
    std::fs::create_dir_all(out).expect("create out dir");
    let status = Command::new(snc_binary())
        .arg("build")
        .arg(entry)
        .arg("--separate")
        .arg("-o")
        .arg(out.join("main"))
        .status()
        .expect("snc invocation failed");
    assert!(status.success(), "snc build --separate failed");
    let mut objs = BTreeMap::new();
    for ent in std::fs::read_dir(out).expect("read out dir") {
        let path = ent.expect("dir entry").path();
        if path.extension().and_then(|e| e.to_str()) == Some("o") {
            let name = path.file_name().expect("file name").to_string_lossy().into_owned();
            objs.insert(name, std::fs::read(&path).expect("read object"));
        }
    }
    objs
}

#[test]
fn repro_separate_per_unit_objects() {
    let base = workspace_root().join("target/sentinel-repro-separate");
    let _ = std::fs::remove_dir_all(&base);
    let entry = write_separate_project(&base.join("proj"));
    let a = compile_separate_objects(&entry, &base.join("out_a"));
    let b = compile_separate_objects(&entry, &base.join("out_b"));

    assert_eq!(
        a.keys().collect::<Vec<_>>(),
        b.keys().collect::<Vec<_>>(),
        "the two builds emitted different per-unit object sets"
    );
    assert!(a.len() >= 3, "expected ≥3 units (main + util_math + geo); got {:?}", a.keys());
    for (name, bytes_a) in &a {
        assert_eq!(
            bytes_a,
            &b[name],
            "unit `{name}` is not byte-identical across two independent `--separate` \
             builds — per-unit codegen is nondeterministic (likely a HashMap/HashSet \
             iterated to emit; emit from a sorted order). This blocks (3/N) caching."
        );
    }
}
