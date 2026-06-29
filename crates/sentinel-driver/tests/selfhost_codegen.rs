//! Phase D self-host port (8/N) / ADR 0045 D11: the codegen differential test.
//! Compile `selfhost/codegen.sentinel` (which REUSES `selfhost/types.sentinel` via a
//! D.6 `use`, mode 4 — which in turn `use`s the parser, a 3-deep module chain) with
//! the Rust `snc`, then assert its emitted textual LLVM IR (`.ll`) is byte-identical
//! to the `snc llvm` oracle.
//!
//! (8a) covers the STRAIGHT-LINE subset (const, var, unary neg/not, binary and cmp,
//! `let`, assign-to-var, value-block, user-fn calls, the u8<->i64 width builtins);
//! (8b) adds control flow (if/else, while/break/continue, &&/||); (8c-1) adds
//! STRUCTS (Pass-0 `%Struct.N` type decls, `insertvalue` literals, `extractvalue`
//! field reads, pass-by-value params/returns); (8c-2) adds ARRAYS (`[T]` =
//! `{ i64, ptr }`, heap-alloc literals via `sentinel_alloc`, `a[i]` with a
//! `sentinel_panic_oob` bounds-check, `len`) — all alloca/load/store, NO phi. The
//! oracle is PARTIAL-by-Err (it Errs + exits nonzero on
//! a not-yet-ported construct), so the differential skips those fixtures, exactly as it
//! skips upstream parse/resolve/type rejects; the supported subset grows per sub-slice
//! (8b..8l). Behavioural correctness of the `.ll` is covered by `tests/llvm.rs` (the
//! oracle compiles + runs identically to inkwell); this test asserts the Sentinel side
//! reproduces the oracle's bytes.

use std::path::{Path, PathBuf};
use std::process::Command;

fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .canonicalize()
        .expect("canonicalize workspace root")
}

/// ADR 0067: `types` is a multi-file module — stage every file in its `types/`
/// parts dir alongside the staged `types.sentinel` so discovery finds the parts.
fn stage_types_parts(root: &Path, dst: &Path) {
    let pd = dst.join("types");
    std::fs::create_dir_all(&pd).expect("create types/ parts dir");
    for ent in std::fs::read_dir(root.join("selfhost/types")).expect("read types/ parts") {
        let p = ent.expect("dir entry").path();
        if p.extension().and_then(|x| x.to_str()) == Some("sentinel") {
            std::fs::copy(&p, pd.join(p.file_name().unwrap())).expect("stage a types part");
        }
    }
}

/// Stage `parser.sentinel` + `types.sentinel` + `codegen.sentinel` into `tmp` (so the
/// `use parser::…` / `use types::…` edges resolve) and compile the entry.
fn build_sentinel_codegen(tmp: &Path) -> PathBuf {
    let root = workspace_root();
    std::fs::copy(root.join("selfhost/parser.sentinel"), tmp.join("parser.sentinel"))
        .expect("stage parser.sentinel");
    std::fs::copy(root.join("selfhost/types.sentinel"), tmp.join("types.sentinel"))
        .expect("stage types.sentinel");
    stage_types_parts(&root, tmp);
    // (8g) path (a): codegen.sentinel now `use`s merge.sentinel (self-hosted discover+merge).
    std::fs::copy(root.join("selfhost/merge.sentinel"), tmp.join("merge.sentinel"))
        .expect("stage merge.sentinel");
    let entry = tmp.join("codegen.sentinel");
    std::fs::copy(root.join("selfhost/codegen.sentinel"), &entry).expect("stage codegen.sentinel");
    let bin = tmp.join("scg");
    let out = Command::new(env!("CARGO_BIN_EXE_snc"))
        .arg("build")
        .arg(&entry)
        .arg("-o")
        .arg(&bin)
        .output()
        .expect("run snc build");
    assert!(
        out.status.success(),
        "compiling selfhost/codegen.sentinel failed:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    bin
}

/// Compile the scg-emitted `.ll` (capstone 2's L1) into a runnable executable,
/// returning its path. On Unix this is one `cc` invocation. Windows has no
/// `cc`/clang DRIVER (the from-source LLVM at `$LLVM_SYS_180_PREFIX` ships the
/// `llvm-*` tools but not the clang driver), so we go `llc` (`.ll` -> `.obj`) +
/// the MSVC `link.exe` with the SAME runtime + native libs + 16 MB stack snc's
/// own link backend uses (ADR 0060). That makes the bootstrap fixed point
/// verifiable on Windows too — it was only ever blocked by the 1 MB default
/// stack (now fixed) plus this missing `cc`, not by anything fundamental.
fn compile_ll_to_exe(ll_path: &Path, out_stem: &Path) -> PathBuf {
    let snc_dir = Path::new(env!("CARGO_BIN_EXE_snc"))
        .parent()
        .expect("snc binary dir");
    if cfg!(target_os = "windows") {
        let obj = out_stem.with_extension("obj");
        let exe = out_stem.with_extension("exe");
        let llc = PathBuf::from(
            std::env::var("LLVM_SYS_180_PREFIX")
                .expect("LLVM_SYS_180_PREFIX set (for llc on Windows)"),
        )
        .join("bin")
        .join("llc.exe");
        // snc hardcodes `target triple = "arm64-apple-darwin"` in the TEXT IR
        // (macOS-first); inkwell overrides it to the host TargetMachine on a
        // real build, so `snc build` works on Windows. `llc` honors the text
        // triple literally, so we must tell it the host triple explicitly (the
        // same thing inkwell does) — else it emits a Mach-O object link.exe
        // rejects (LNK1107). The IR carries no datalayout, so llc uses the
        // x86_64-windows default.
        let llc_out = Command::new(&llc)
            .arg(ll_path)
            .arg("-mtriple=x86_64-pc-windows-msvc")
            .arg("-filetype=obj")
            .arg("-o")
            .arg(&obj)
            .output()
            .expect("run llc on the scg-emitted .ll");
        assert!(
            llc_out.status.success(),
            "llc of the scg-emitted .ll failed:\n{}",
            String::from_utf8_lossy(&llc_out.stderr)
        );
        // Mirror sentinel-driver's `link_exe` (ADR 0060): the runtime `.lib` +
        // the native deps + msvcrt + the 16 MB stack. Requires the MSVC env
        // (LIB paths) on PATH, exactly like a normal `snc build` on Windows.
        let runtime = snc_dir.join("sentinel_runtime.lib");
        let mut link = Command::new("link.exe");
        link.arg("/NOLOGO")
            .arg("/SUBSYSTEM:CONSOLE")
            .arg("/STACK:16777216")
            .arg(format!("/OUT:{}", exe.display()))
            .arg(&obj)
            .arg(&runtime);
        for lib in [
            "legacy_stdio_definitions.lib",
            "kernel32.lib",
            "ntdll.lib",
            "userenv.lib",
            "ws2_32.lib",
            "dbghelp.lib",
        ] {
            link.arg(lib);
        }
        link.arg("/DEFAULTLIB:msvcrt");
        let link_out = link.output().expect("run link.exe on the scg .obj");
        assert!(
            link_out.status.success(),
            "link.exe of the scg .obj failed:\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&link_out.stdout),
            String::from_utf8_lossy(&link_out.stderr)
        );
        exe
    } else {
        let runtime = snc_dir.join("libsentinel_runtime.a");
        let cc = Command::new("cc")
            .arg(ll_path)
            .arg(&runtime)
            .arg("-o")
            .arg(out_stem)
            .output()
            .expect("run cc on the scg-emitted .ll");
        assert!(
            cc.status.success(),
            "cc of the scg-emitted .ll failed:\n{}",
            String::from_utf8_lossy(&cc.stderr)
        );
        out_stem.to_path_buf()
    }
}

/// Straight-line seeds (8a): const + main-trunc, params + arith + call, a `let` chain,
/// cmp + unary-not + bool, unary negate, bitwise ops, and mut + assign-to-var.
const SEEDS: &[&str] = &[
    "fn main() -> i64 { 42 }\n",
    "fn add(a: i64, b: i64) -> i64 { a + b }\nfn main() -> i64 { add(20, 22) }\n",
    "fn compute(x: i64) -> i64 { let y: i64 = x * 3; let z: i64 = y - 4; z }\nfn main() -> i64 { compute(16) }\n",
    "fn f(a: i64, b: i64) -> bool { let c: bool = a < b; !c }\nfn main() -> i64 { 0 }\n",
    "fn neg(x: i64) -> i64 { -x }\nfn main() -> i64 { 0 }\n",
    "fn bits(a: i64, b: i64) -> i64 { a & b | (a ^ b) }\nfn main() -> i64 { 0 }\n",
    "fn m() -> i64 { let mut x: i64 = 1; x = x + 41; x }\nfn main() -> i64 { m() }\n",
    // (8b) control flow: if/else, a while loop, &&/||, and a conditional break.
    "fn pick(c: bool, a: i64, b: i64) -> i64 { if c { a } else { b } }\nfn main() -> i64 { pick(true, 7, 9) }\n",
    "fn nested(a: bool, b: bool) -> i64 { if a { if b { 1 } else { 2 } } else { 3 } }\nfn main() -> i64 { 0 }\n",
    "fn sum(n: i64) -> i64 { let mut i: i64 = 0; let mut s: i64 = 0; while i < n { i = i + 1; s = s + i; } s }\nfn main() -> i64 { sum(5) }\n",
    "fn t(a: bool, b: bool) -> bool { a && b || a }\nfn main() -> i64 { 0 }\n",
    "fn brk(n: i64) -> i64 { let mut i: i64 = 0; while i < 100 { i = i + 1; if i == n { break; 0 } else { 0 }; } i }\nfn main() -> i64 { brk(7) }\n",
    "fn cont(n: i64) -> i64 { let mut i: i64 = 0; let mut s: i64 = 0; while i < n { i = i + 1; if i == 2 { continue; 0 } else { 0 }; s = s + i; } s }\nfn main() -> i64 { cont(4) }\n",
    // (8c-1) aggregates — structs: Pass-0 `%Struct.N` decls, `insertvalue` literals,
    // `extractvalue` field reads, pass-by-value params/returns (alloca/store/load).
    "struct Point { x: i64, y: i64 }\nfn dist(p: Point) -> i64 { p.x + p.y }\nfn main() -> i64 { let p = Point { x: 30, y: 12 }; dist(p) }\n",
    "struct Inner { x: i64 }\nstruct Outer { inner: Inner, y: i64 }\nfn main() -> i64 { let o = Outer { inner: Inner { x: 7 }, y: 3 }; o.inner.x + o.y }\n",
    "struct Tagged { value: i64, valid: bool, count: i64 }\nfn main() -> i64 { let t = Tagged { value: 5, valid: true, count: 9 }; if t.valid { t.value + t.count } else { 0 } }\n",
    "struct Pair { a: i64, b: i64 }\nfn pick(c: bool) -> Pair { if c { Pair { a: 1, b: 2 } } else { Pair { a: 10, b: 20 } } }\nfn main() -> i64 { let p = pick(true); p.a + p.b }\n",
    // (8c-2) aggregates — arrays: `{ i64, ptr }`, heap alloc + GEP-stores +
    // insertvalue; `a[i]` bounds-check + GEP+load; `len` = extractvalue 0; the
    // module declares only the runtime symbols actually used.
    "fn main() -> i64 { let xs = [10, 20, 30]; xs[1] + len(xs) }\n",
    "fn first(xs: [i64]) -> i64 { xs[0] }\nfn main() -> i64 { first([7, 8, 9]) }\n",
    "fn main() -> i64 { let xs: [i64] = []; len(xs) }\n",
    // (8c-3) [u8]/string literals (heap-copied byte arrays — a u8 array literal of
    // constant bytes) + char literals (u8 constants).
    "fn main() -> i64 { let s: [u8] = \"hi\"; len(s) }\n",
    "fn main() -> i64 { let c: u8 = 'Z'; u8_to_i64(c) }\n",
    // (8d) runtime builtins: str_eq + print_bytes decompose a [u8] into (ptr, len)
    // and call the sentinel_* symbol. (read_file/write_file need a real file at
    // runtime → behaviourally covered by the c5d4_file_io corpus fixture, not a seed.)
    "fn eq(a: [u8], b: [u8]) -> bool { str_eq(a, b) }\nfn main() -> i64 { let x: [u8] = \"hi\"; let y: [u8] = \"hi\"; if eq(x, y) { 1 } else { 0 } }\n",
    "fn show(b: [u8]) -> i64 { print_bytes(b) }\nfn main() -> i64 { let s: [u8] = \"hi\"; show(s) }\n",
    // (8d-refs) &/&mut/*/*p=x: a ref is an opaque ptr; `&v` is v's slot (no
    // instruction), `*r` loads through it, `*x = v` stores through it.
    "fn add(a: &i64, b: &i64) -> i64 { *a + *b }\nfn main() -> i64 { let a: i64 = 10; let b: i64 = 32; add(&a, &b) }\n",
    "fn inc(x: &mut i64) -> i64 { let n: i64 = *x + 1; *x = n; *x }\nfn main() -> i64 { let mut a: i64 = 10; let r: i64 = inc(&mut a); a + r }\n",
    // (8d-Vec) Vec<T> = {len,cap,ptr}: vec_new (constant), push (the len==cap
    // realloc grow CFG via &mut Vec), v[i] (index, data = field 2), len, pop.
    "fn main() -> i64 { let mut v: Vec<i64> = vec_new(); push(&mut v, 10); push(&mut v, 20); let a: i64 = v[1]; let l: i64 = len(v); let p: i64 = pop(&mut v); a + l + p }\n",
    // (8d-Vec-2) vec_to_array(v): the Vec -> [T] bridge — extract len(0)/data(2),
    // sentinel_alloc(len*sizeof), llvm.memcpy the live prefix, build [T] {len,dest}.
    // A Vec<u8> bridge (elem = i8, the lexer/String use case) and a Vec<i64> bridge
    // (elem = i64, a wider GEP-sizeof stride).
    "fn main() -> i64 { let mut v: Vec<u8> = vec_new(); push(&mut v, 'h'); push(&mut v, 'i'); let a: [u8] = vec_to_array(v); len(a) }\n",
    "fn main() -> i64 { let mut v: Vec<i64> = vec_new(); push(&mut v, 10); push(&mut v, 20); push(&mut v, 30); let a: [i64] = vec_to_array(v); a[0] + a[2] }\n",
    // (8d-drops) scope-exit `sentinel_free`: a heap binding is freed in reverse decl
    // order at its block's exit, EXCEPT a moved-out one (consuming call) — the callee
    // frees its param. arr is moved into consume (no double-free); tmp drops at the
    // nested block; the [u8] param of show drops at show's exit.
    "fn consume(xs: [i64]) -> i64 { xs[0] }\nfn main() -> i64 { let arr: [i64] = [1, 2, 3]; let inner: i64 = { let tmp: [i64] = [4, 5]; tmp[0] }; consume(arr) + inner }\n",
    "fn show(b: [u8]) -> i64 { len(b) }\nfn main() -> i64 { let s: [u8] = \"hi\"; let v: Vec<i64> = vec_new(); show(s) + len(v) }\n",
    // (8d-drops-2) recursive struct-field drop: dropping the struct GEPs into each
    // heap-backed field and frees it (the [i64] field); scalar fields are skipped.
    "struct Box { data: [i64], tag: i64 }\nfn main() -> i64 { let b = Box { data: [9, 8], tag: 5 }; b.data[0] + b.tag }\n",
    // (8d-drops-3) loop-exit drops: a per-iteration heap binding is drained on the
    // break path (before branching to loop_after) AND on the fall-through (body end);
    // mutually exclusive blocks → freed once per runtime path.
    "fn main() -> i64 { let mut i: i64 = 0; let mut n: i64 = 0; while i < 3 { let s: [u8] = \"ab\"; n = n + len(s); if i == 1 { break; 0 } else { 0 }; i = i + 1; } n }\n",
    "fn main() -> i64 { let mut i: i64 = 0; let mut n: i64 = 0; while i < 4 { i = i + 1; let w: [u8] = \"xy\"; n = n + len(w); if (i / 2) * 2 != i { continue; 0 } else { 0 }; n = n + 1; } n }\n",
    // (8e-1) enum construction `{ i32 tag, ptr payload }` + enum drop (null-checked box
    // free): a payload variant heap-boxes its fields; a unit variant is a null payload.
    // (match — reading the value back — is 8e-2.)
    "enum E { A, B(i64) }\nfn main() -> i64 { let x = E::B(7); let y = E::A; 0 }\n",
    "enum P { Z, Two(i64, i64) }\nfn main() -> i64 { let p = P::Two(3, 4); let z = P::Z; 0 }\n",
    // (8e-2) match: an if-else chain over the variant arms (tag check + payload bind +
    // body + store-to-result + br merge), `unreachable` default, merge load. The param
    // enum is dropped at the callee's exit; a 2-payload variant binds two fields.
    "enum E { A, B(i64) }\nfn f(e: E) -> i64 { match e { E::A => 0, E::B(x) => x } }\nfn main() -> i64 { f(E::B(7)) }\n",
    "enum Shape { Unit, Circle(i64), Rect(i64, i64) }\nfn area(s: Shape) -> i64 { match s { Shape::Unit => 0, Shape::Circle(r) => r * r, Shape::Rect(w, h) => w * h } }\nfn main() -> i64 { area(Shape::Rect(5, 6)) }\n",
    // (8f-3) lvalue field places — `&mut (*c).f` (the selfhost ctx-field idiom, 236x) +
    // `(*c).f = x`: address-of / assign-to a struct field through a &mut pointer. GEP
    // into the target's pointer; the enclosing &mut/assign uses the field address.
    "struct Box { items: Vec<i64>, n: i64 }\nfn add(b: &mut Box, x: i64) -> i64 { push(&mut (*b).items, x); (*b).n = (*b).n + 1; 0 }\nfn main() -> i64 { let mut bx: Box = Box { items: vec_new(), n: 0 }; add(&mut bx, 10); add(&mut bx, 20); bx.n + len(bx.items) }\n",
];

#[test]
fn sentinel_codegen_matches_oracle_on_seeds() {
    let tmp = std::env::temp_dir().join(format!("snc_selfhost_cg_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(&tmp).expect("create temp dir");
    let cg = build_sentinel_codegen(&tmp);

    let work = tmp.join("work");
    std::fs::create_dir_all(&work).expect("create work dir");
    let input = work.join("input.sentinel");

    let mut mismatches: Vec<String> = Vec::new();
    for seed in SEEDS {
        std::fs::write(&input, seed).expect("stage seed");
        let oracle = Command::new(env!("CARGO_BIN_EXE_snc"))
            .arg("llvm")
            .arg(&input)
            .output()
            .expect("run snc llvm");
        assert!(
            oracle.status.success(),
            "snc llvm rejected a straight-line seed:\n{seed}\n{}",
            String::from_utf8_lossy(&oracle.stderr)
        );
        let sentinel = Command::new(&cg)
            .current_dir(&work)
            .output()
            .expect("run the Sentinel codegen");
        if oracle.stdout != sentinel.stdout {
            mismatches.push(format!(
                "  seed {seed:?}\n    oracle:\n{}\n    sentinel:\n{}",
                String::from_utf8_lossy(&oracle.stdout),
                String::from_utf8_lossy(&sentinel.stdout)
            ));
        }
    }
    assert!(
        mismatches.is_empty(),
        "the Sentinel codegen diverged from `snc llvm` on {}/{} seed(s):\n{}",
        mismatches.len(),
        SEEDS.len(),
        mismatches.join("\n")
    );
}

/// Every `.sentinel` fixture under tests/pass + tests/ui, sorted.
fn collect_fixtures() -> Vec<PathBuf> {
    let root = workspace_root();
    let mut fixtures = Vec::new();
    for sub in ["tests/pass", "tests/ui"] {
        for entry in std::fs::read_dir(root.join(sub)).expect("read fixture dir") {
            let path = entry.expect("dir entry").path();
            if path.extension().and_then(|e| e.to_str()) == Some("sentinel") {
                fixtures.push(path);
            }
        }
    }
    fixtures.sort();
    fixtures
}

/// (8a) the corpus differential: the Sentinel codegen matches `snc llvm` byte-for-byte
/// over every fixture the oracle EMITS (the straight-line subset at 8a — the oracle is
/// partial-by-Err, so fixtures using a not-yet-ported construct exit nonzero and are
/// skipped, as are upstream rejects). The emitting subset grows each sub-slice; the floor
/// guards against a regression that stops the Sentinel side from reproducing it.
#[test]
fn sentinel_codegen_matches_oracle_on_corpus() {
    let tmp =
        std::env::temp_dir().join(format!("snc_selfhost_cg_corpus_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(&tmp).expect("create temp dir");
    let cg = build_sentinel_codegen(&tmp);

    let work = tmp.join("work");
    std::fs::create_dir_all(&work).expect("create work dir");
    let input = work.join("input.sentinel");

    let fixtures = collect_fixtures();
    assert!(fixtures.len() > 100, "expected a substantial corpus, got {}", fixtures.len());

    let mut emitted = 0usize;
    let mut mismatches: Vec<String> = Vec::new();
    for fixture in &fixtures {
        let bytes = std::fs::read(fixture).expect("read fixture");
        std::fs::write(&input, &bytes).expect("stage input");
        let oracle = Command::new(env!("CARGO_BIN_EXE_snc"))
            .arg("llvm")
            .arg(&input)
            .output()
            .expect("run snc llvm");
        if !oracle.status.success() {
            continue; // not in the emitted subset (a deferred construct / upstream reject)
        }
        emitted += 1;
        let sentinel = Command::new(&cg)
            .current_dir(&work)
            .output()
            .expect("run the Sentinel codegen");
        if oracle.stdout != sentinel.stdout {
            mismatches.push(format!(
                "  {} (oracle {} bytes vs sentinel {} bytes)",
                fixture.file_name().unwrap().to_string_lossy(),
                oracle.stdout.len(),
                sentinel.stdout.len()
            ));
        }
    }

    assert!(
        emitted >= 15,
        "expected the straight-line subset (~16) to emit, got {emitted}"
    );
    assert!(
        mismatches.is_empty(),
        "the Sentinel codegen diverged from `snc llvm` on {}/{} emitted fixture(s):\n{}",
        mismatches.len(),
        emitted,
        mismatches.join("\n")
    );
}

/// (8f) The Sentinel codegen emits its OWN front-end stages byte-identically to `snc
/// llvm` — a step toward the bootstrap fixed-point: the compiler compiling its own
/// source. `lexer.sentinel` (390 lines) and `parser.sentinel` (2590 lines) are the
/// self-contained stages (no `use`), so they lower through the single-file pipeline;
/// they exercise the WHOLE Bar-A construct set at real scale (4.4k + 21.6k `.ll` lines)
/// far beyond the small corpus fixtures. The multi-module stages (types/codegen/…) need
/// the merged path (a later slice). This is the headline (8f) regression guard.
#[test]
fn sentinel_codegen_matches_oracle_on_selfhost_stages() {
    let tmp =
        std::env::temp_dir().join(format!("snc_selfhost_cg_stages_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(&tmp).expect("create temp dir");
    let cg = build_sentinel_codegen(&tmp);

    let work = tmp.join("work");
    std::fs::create_dir_all(&work).expect("create work dir");
    let input = work.join("input.sentinel");
    let root = workspace_root();

    for stage in ["lexer", "parser"] {
        let src = root.join("selfhost").join(format!("{stage}.sentinel"));
        let bytes = std::fs::read(&src).expect("read selfhost stage");
        std::fs::write(&input, &bytes).expect("stage input");
        let oracle = Command::new(env!("CARGO_BIN_EXE_snc"))
            .arg("llvm")
            .arg(&input)
            .output()
            .expect("run snc llvm");
        assert!(
            oracle.status.success(),
            "snc llvm rejected selfhost/{stage}.sentinel:\n{}",
            String::from_utf8_lossy(&oracle.stderr)
        );
        let sentinel = Command::new(&cg)
            .current_dir(&work)
            .output()
            .expect("run the Sentinel codegen");
        assert_eq!(
            oracle.stdout,
            sentinel.stdout,
            "selfhost/{stage}.sentinel: the Sentinel codegen diverged from `snc llvm` \
             (oracle {} bytes vs sentinel {} bytes)",
            oracle.stdout.len(),
            sentinel.stdout.len()
        );
    }
}

/// (8f-2/8f-3) `snc llvm` lowers the FULL multi-module self-hosting compiler via the
/// merged path (`run_llvm_merged` mirrors `run_build`'s D.6 discovery + `merge_modules`).
/// Each stage that `use`s others (`types` uses parser; `codegen` uses the 3-deep chain)
/// emits its `.ll` without Err — guarding the multi-module dispatch AND the complete
/// Bar-A construct coverage on the real compiler (the merged `codegen` is ~83k `.ll`
/// lines). Oracle-only: the Sentinel `scg` is single-file, so self-compiling the
/// multi-module compiler (it merging too) is the (8g) fixed-point. (The emitted `.ll` is
/// behaviourally validated cc==inkwell out of band.)
#[test]
fn snc_llvm_lowers_the_merged_compiler() {
    let root = workspace_root();
    for stage in ["types", "codegen"] {
        let src = root.join("selfhost").join(format!("{stage}.sentinel"));
        let out = Command::new(env!("CARGO_BIN_EXE_snc"))
            .arg("llvm")
            .arg(&src)
            .output()
            .expect("run snc llvm");
        assert!(
            out.status.success(),
            "snc llvm failed on the merged selfhost/{stage}.sentinel:\n{}",
            String::from_utf8_lossy(&out.stderr)
        );
        assert!(
            out.stdout.len() > 100_000,
            "expected a large `.ll` for the merged {stage} compiler, got {} bytes",
            out.stdout.len()
        );
    }
}

/// (8g) THE BOOTSTRAP FIXED POINT — the capstone of the self-host port (ADR 0045
/// D8(ii)). The Sentinel codegen (`scg`, built by `snc build` via inkwell) lowers the
/// WHOLE multi-module self-hosting compiler AND reproduces it. Two assertions:
///   1. **Self-compilation** — `scg` reads the MERGED compiler source (`snc merge`: the
///      D.6 module graph collapsed to one `$`-qualified `.sentinel`, the owner-chosen
///      path (b)) and emits `.ll` BYTE-IDENTICAL to the Rust `snc llvm` oracle.
///   2. **Fixed point** — `cc` that `.ll` into a fresh compiler `scg'`, which re-emits
///      the SAME `.ll` byte-for-byte: the compiler reproduces its own output (why C5
///      shipped `abi-v1` + reproducible builds, ADR 0029).
///
/// `scg` is single-file; the compiler is multi-module — so the merge runs in Rust
/// (`merge_modules` + `source_dump`) and feeds `scg` one file. Every compiler STAGE
/// (lex → resolve → types → effect → borrow → ctverify → codegen) runs inside `scg`.
#[test]
fn sentinel_codegen_reaches_the_bootstrap_fixed_point() {
    let tmp =
        std::env::temp_dir().join(format!("snc_fixedpoint_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(&tmp).expect("create temp dir");
    // Stage parser+types+codegen into `tmp` and build `scg` (inkwell). The entry the
    // merge/oracle read is the SAME staged source `scg` was built from.
    let scg = build_sentinel_codegen(&tmp);
    let entry = tmp.join("codegen.sentinel");

    // M = the merged single-file compiler source (Rust merge-to-source, ADR 0045 (8g)).
    let merged = Command::new(env!("CARGO_BIN_EXE_snc"))
        .arg("merge")
        .arg(&entry)
        .output()
        .expect("run snc merge");
    assert!(
        merged.status.success(),
        "snc merge failed:\n{}",
        String::from_utf8_lossy(&merged.stderr)
    );

    // The Rust oracle's canonical `.ll` for the full compiler.
    let oracle = Command::new(env!("CARGO_BIN_EXE_snc"))
        .arg("llvm")
        .arg(&entry)
        .output()
        .expect("run snc llvm");
    assert!(
        oracle.status.success(),
        "snc llvm failed:\n{}",
        String::from_utf8_lossy(&oracle.stderr)
    );

    // L1 = scg(M). `scg` reads `./input.sentinel` from its cwd.
    let r1 = tmp.join("r1");
    std::fs::create_dir_all(&r1).expect("create r1");
    std::fs::write(r1.join("input.sentinel"), &merged.stdout).expect("stage M for scg");
    let l1 = Command::new(&scg).current_dir(&r1).output().expect("run scg");
    assert!(l1.status.success(), "scg failed:\n{}", String::from_utf8_lossy(&l1.stderr));

    // Capstone 1: scg == the Rust oracle, byte-for-byte, over the full merged compiler.
    assert_eq!(
        l1.stdout,
        oracle.stdout,
        "scg diverged from `snc llvm` on the merged compiler (scg {} vs oracle {} bytes)",
        l1.stdout.len(),
        oracle.stdout.len()
    );

    // scg' = compile(L1) — `cc` on Unix, `llc` + `link.exe` on Windows. The
    // runtime sits beside the snc binary.
    let l1_path = tmp.join("L1.ll");
    std::fs::write(&l1_path, &l1.stdout).expect("write L1.ll");
    let scg_prime = compile_ll_to_exe(&l1_path, &tmp.join("scg_prime"));

    // L2 = scg'(M). Capstone 2: L2 == L1 — the bootstrap fixed point.
    let r2 = tmp.join("r2");
    std::fs::create_dir_all(&r2).expect("create r2");
    std::fs::write(r2.join("input.sentinel"), &merged.stdout).expect("stage M for scg'");
    let l2 = Command::new(&scg_prime).current_dir(&r2).output().expect("run scg'");
    assert!(l2.status.success(), "scg' failed:\n{}", String::from_utf8_lossy(&l2.stderr));
    assert_eq!(
        l2.stdout,
        l1.stdout,
        "the bootstrap fixed point does not hold: scg' re-emitted {} bytes vs scg's {}",
        l2.stdout.len(),
        l1.stdout.len()
    );

    let _ = std::fs::remove_dir_all(&tmp);
}

/// (8g) PATH (a) — THE TRUE FULL SELF-HOST. `scg` (`codegen.sentinel`, which now `use`s
/// the self-hosted `merge.sentinel`) DISCOVERS + MERGES + EMITS the whole multi-module
/// compiler ITSELF — no Rust merge pre-pass. It reads the multi-module entry directly,
/// follows the `use` edges, merges in Sentinel, and lowers the merged program to `.ll`
/// BYTE-IDENTICAL to the `snc llvm` oracle; `cc` that `.ll` → `scg'`, which re-emits the
/// same `.ll` byte-for-byte (the fixed point). The entry is staged as `input.sentinel`
/// (the name `merge_source` reads) so its stem matches the oracle's.
#[test]
fn sentinel_codegen_self_merges_the_compiler_and_reaches_fixed_point() {
    let tmp =
        std::env::temp_dir().join(format!("snc_patha_capstone_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(&tmp).expect("create temp dir");
    // Builds scg from codegen.sentinel + merge + types + parser (the self-merging compiler).
    let scg = build_sentinel_codegen(&tmp);

    // The run dir: the multi-module compiler, with `codegen.sentinel` as the entry
    // (`input.sentinel`) and its `use`-reachable deps alongside.
    let run = tmp.join("run");
    std::fs::create_dir_all(&run).expect("create run dir");
    let root = workspace_root();
    std::fs::copy(root.join("selfhost/codegen.sentinel"), run.join("input.sentinel"))
        .expect("stage codegen as input.sentinel");
    for dep in ["merge", "types", "parser"] {
        std::fs::copy(
            root.join("selfhost").join(format!("{dep}.sentinel")),
            run.join(format!("{dep}.sentinel")),
        )
        .expect("stage dependency");
    }
    // ADR 0067: `types` is multi-file — stage its `types/` parts dir (staged as
    // `types`, so its parts resolve under `run/types/`).
    stage_types_parts(&root, &run);

    // The Rust oracle: `snc llvm` on the entry (discovers + merge_modules + dumps).
    let oracle = Command::new(env!("CARGO_BIN_EXE_snc"))
        .arg("llvm")
        .arg(run.join("input.sentinel"))
        .output()
        .expect("run snc llvm (oracle)");
    assert!(
        oracle.status.success(),
        "snc llvm failed on the multi-module compiler entry:\n{}",
        String::from_utf8_lossy(&oracle.stderr)
    );

    // scg discovers + merges + emits ITSELF (reads ./input.sentinel, follows `use` edges).
    let l1 = Command::new(&scg).current_dir(&run).output().expect("run scg (self-merge)");
    assert!(l1.status.success(), "scg failed:\n{}", String::from_utf8_lossy(&l1.stderr));
    assert_eq!(
        l1.stdout,
        oracle.stdout,
        "scg's self-merge+emit diverged from the oracle (scg {} vs oracle {} bytes)",
        l1.stdout.len(),
        oracle.stdout.len()
    );

    // scg' = compile(L1); L2 = scg'(compiler); assert L2 == L1 (the fixed point).
    let l1_path = tmp.join("L1.ll");
    std::fs::write(&l1_path, &l1.stdout).expect("write L1.ll");
    let scg_prime = compile_ll_to_exe(&l1_path, &tmp.join("scg_prime"));
    let l2 = Command::new(&scg_prime).current_dir(&run).output().expect("run scg'");
    assert!(l2.status.success(), "scg' failed:\n{}", String::from_utf8_lossy(&l2.stderr));
    assert_eq!(
        l2.stdout,
        l1.stdout,
        "path (a) fixed point does not hold: scg' re-emitted {} bytes vs scg's {}",
        l2.stdout.len(),
        l1.stdout.len()
    );

    let _ = std::fs::remove_dir_all(&tmp);
}
