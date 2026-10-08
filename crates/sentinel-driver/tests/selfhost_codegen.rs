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

mod common;

fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .canonicalize()
        .expect("canonicalize workspace root")
}

/// ADR 0067: stage every file in a multi-file module's `<module>/` parts dir
/// alongside the staged `<module>.sentinel` so discovery finds the parts. A no-op
/// if the module has no parts dir.
fn stage_module_parts(root: &Path, dst: &Path, module: &str) {
    let src = root.join("selfhost").join(module);
    if !src.is_dir() {
        return;
    }
    let pd = dst.join(module);
    std::fs::create_dir_all(&pd).expect("create parts dir");
    for ent in std::fs::read_dir(&src).expect("read parts dir") {
        let p = ent.expect("dir entry").path();
        if p.extension().and_then(|x| x.to_str()) == Some("sentinel") {
            std::fs::copy(&p, pd.join(p.file_name().unwrap())).expect("stage a part");
        }
    }
}

/// Stage `parser.sentinel` + `types.sentinel` + `codegen.sentinel` into `tmp` (so the
/// `use parser::…` / `use types::…` edges resolve) and compile the entry.
fn build_sentinel_codegen(tmp: &Path) -> PathBuf {
    let root = workspace_root();
    std::fs::copy(root.join("selfhost/parser.sentinel"), tmp.join("parser.sentinel"))
        .expect("stage parser.sentinel");
    stage_module_parts(&root, tmp, "parser");
    std::fs::copy(root.join("selfhost/types.sentinel"), tmp.join("types.sentinel"))
        .expect("stage types.sentinel");
    stage_module_parts(&root, tmp, "types");
    // (8g) path (a): codegen.sentinel now `use`s merge.sentinel (self-hosted discover+merge).
    std::fs::copy(root.join("selfhost/merge.sentinel"), tmp.join("merge.sentinel"))
        .expect("stage merge.sentinel");
    stage_module_parts(&root, tmp, "merge");
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

/// Compile an emitted `.ll` -- `scg`'s (capstone 2's L1), or the oracle's -- into a
/// runnable executable, returning its path. On Unix this is one `cc` invocation. Windows has no
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
            .expect("run llc on the emitted .ll");
        assert!(
            llc_out.status.success(),
            "llc of {} failed:\n{}",
            ll_path.display(),
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
        let link_out = link.output().expect("run link.exe on the .obj");
        assert!(
            link_out.status.success(),
            "link.exe of the .obj built from {} failed:\nstdout:\n{}\nstderr:\n{}",
            ll_path.display(),
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
            .expect("run cc on the emitted .ll");
        assert!(
            cc.status.success(),
            "cc of {} failed:\n{}",
            ll_path.display(),
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
    // ADR 0074 (register D79): every exit of a handler arm releases the continuation
    // the arm's slot still holds, and `k(v)` clears the slot after its argument. Seeds
    // rather than `tests/pass` fixtures because an `if` inside a handler arm makes the
    // two MIR lowerers diverge (register D83), and the MIR differential sweeps every
    // `tests/pass` file. `tests/handler_arm_exits.rs` runs them through `snc build`,
    // `oracle_ir_of_the_handler_arm_exit_programs_runs` below through the oracle's IR,
    // and `tests/llvm.rs` pins the oracle's side of each.
    include_str!("fixtures/handler_arm_exits/c74_arm_declines_to_resume.sentinel"),
    include_str!("fixtures/handler_arm_exits/c74_arm_return_leaves_the_arm.sentinel"),
    include_str!("fixtures/handler_arm_exits/c74_arm_break_continue.sentinel"),
    include_str!("fixtures/handler_arm_exits/c74_resume_arg_leaves_the_arm.sentinel"),
    include_str!("fixtures/handler_arm_exits/c74_two_open_arms.sentinel"),
    // ADR 0071 A4 (register D157): a struct and a class that hold each other by value are
    // accepted, and a class's drop walks its fields for handles, stopping at a type already
    // on its path. A seed rather than a fixture because the IR does not assemble (`llc`:
    // "Cannot allocate unsized type"); it holds `scg`'s walk, and where it stops, to the
    // oracle's. Remove it once D157's refusal exists.
    "struct S { k: K, n: i64 }\nclass K { let s: S; let h: Shared<i64>; pub init(s: S) { self.s = s; self.h = shared_new(1); 0 } }\nfn f(s: S) -> i64 { s.n }\nfn g(k: K) -> i64 { 1 }\nfn main() -> i64 { 7 }\n",
    // ADR 0077: a `handle`'s `return` arm is emitted once per pure-drain site; the oracle walks
    // one typed tree at each and `scg` re-parses the arm, so a binding the arm declares and
    // moves must get its own flag in each copy in both (`%mf0`, `%mf1`).
    "effect Io { read() -> i64; }\nfn consume(v: [i64]) -> i64 { v[0] }\nfn main() -> i64 { handle 40 with { Io.read(k) => k(1), return x => { let v: [i64] = [x, 2]; consume(v) + 2 } } }\n",
    // ADR 0077 D5: a moved field its binding's drop does nothing with gets no flag. A
    // compared `?Node` field in an `if` condition is a move site whose drop is empty, and so
    // is a type parameter's field at a Copy instance; neither gets a flag in either back end.
    "struct Node { val: i64 }\nstruct Holder { name: [u8], head: ?Node }\nfn main() -> i64 { let h: Holder = Holder { name: \"abc\", head: null }; if h.head == null { 42 } else { 0 } }\n",
    "struct Pair<A, B> { first: A, second: B }\nfn take<T>(x: T) -> i64 { 1 }\nfn g<T>(p: Pair<T, [i64]>, n: i64) -> i64 { if n > 5 { take(p.first) } else { 42 } }\nfn main() -> i64 { let p: Pair<i64, [i64]> = Pair { first: 5, second: [1] }; g(p, 1) }\n",
    // ADR 0077 D5: a moved field gets a flag only if its binding's drop does something with
    // it -- a struct's drop drops a field its type needs a drop for, and a class's releases
    // the handles its fields hold and nothing else. So none for a struct of scalars, a
    // compared `?Node` in a `let`, an array moved out of a class, or a type parameter's field
    // at a struct of scalars (whose numbering must not shift the whole binding's flag), and an
    // assignment after such a move drops nothing; one for a `secret` struct holding a
    // `Shared` moved out of a class.
    "struct Pt { x: i64, y: i64 }\nstruct Bag { buf: [i64], pt: Pt }\nfn use_pt(p: Pt) -> i64 { p.x + p.y }\nfn main() -> i64 { let b: Bag = Bag { buf: [7, 8, 9], pt: Pt { x: 20, y: 22 } }; let r: i64 = use_pt(b.pt); r }\n",
    "struct Node { val: i64 }\nstruct Holder { name: [u8], head: ?Node }\nfn main() -> i64 { let h: Holder = Holder { name: \"abc\", head: null }; let b: bool = h.head == null; if b { 42 } else { 0 } }\n",
    "class C { let a: [i64]; let n: i64; pub init(a: [i64]) { self.a = a; self.n = 1; 0 } }\nfn consume(v: [i64]) -> i64 { v[0] }\nfn f(i: i64) -> i64 { let c: C = C::init([1, 2]); if i > 5 { consume(c.a) } else { 1 } }\nfn main() -> i64 { f(9) + f(1) + 40 }\n",
    "struct P { x: i64 }\nstruct Pair<A, B> { first: A, second: B }\nfn take<T>(g: T) -> i64 { 1 }\nfn gp<A, B>(p: Pair<A, B>, n: i64) -> i64 { if n == 0 { take(p.first) } else { if n == 1 { take(p) } else { 1 } } }\nfn main() -> i64 { let p: Pair<P, [i64]> = Pair { first: P { x: 1 }, second: [1] }; gp(p, 0) + 41 }\n",
    "struct P { x: i64 }\nstruct HQ { q: P, a: [i64] }\nfn eat_p(p: P) -> i64 { p.x }\nfn f(i: i64) -> i64 { let mut h: HQ = HQ { q: P { x: 1 }, a: [1, 2, 3, 4] }; let r: i64 = if i > 5 { eat_p(h.q) } else { 1 }; h = HQ { q: P { x: 2 }, a: [5, 6, 7, 8] }; r }\nfn main() -> i64 { f(9) + f(1) + 40 }\n",
    "struct H { s: Shared<i64>, n: i64 }\nfn eat(h: secret H) -> i64 { 1 }\nclass C { let h: secret H; let z: i64; pub init(n: i64) { self.h = H { s: shared_new(n), n: n }; self.z = n; 0 } }\nfn f(i: i64) -> i64 { let c: C = C::init(40); if i > 5 { eat(c.h) } else { 0 } }\nfn main() -> i64 { f(9) + f(1) + 41 }\n",
    // Parity where D6 does not fire: after a class moved an array out (no flag), after a
    // compared `?Node` field (register D117's position) and after a type parameter's field at
    // a Copy instance, an assignment drops no old value in either back end (register D120).
    "fn consume(v: [i64]) -> i64 { v[0] }\nclass C { let a: [i64]; let h: Shared<i64>; pub init(n: i64) { self.a = [n, 2]; self.h = shared_new(n); 0 } }\nfn f(n: i64) -> i64 { let mut c: C = C::init(1); let r: i64 = if n > 0 { consume(c.a) } else { 1 }; c = C::init(2); r }\nfn main() -> i64 { f(1) + f(0) + 40 }\n",
    "struct Node { val: i64 }\nstruct Holder { buf: [i64], head: ?Node }\nfn f(i: i64) -> i64 { let mut h: Holder = Holder { buf: [1, 2], head: null }; let r: i64 = if h.head == null { 1 } else { 0 }; h = Holder { buf: [5, 6], head: null }; r }\nfn main() -> i64 { f(1) + 41 }\n",
    "struct Pair<A, B> { first: A, second: B }\nfn take_g<T>(x: T) -> i64 { 1 }\nfn mkp<A, B>(a: A, b: B) -> Pair<A, B> { Pair { first: a, second: b } }\nfn g<T>(x: T, y: T, n: i64) -> i64 { let mut s = mkp(x, [n, 2]); let r: i64 = if n > 0 { take_g(s.first) } else { 0 }; s = mkp(y, [n, 5]); r }\nfn main() -> i64 { g(1, 2, 1) + 41 }\n",
    // A field of a compound target is not a field of a binding: `{ s }.a` moves `s` whole,
    // and so does each arm of the `if`. No field flag, and neither binding's drop is elided.
    "struct S { a: [i64], b: [i64] }\nfn consume(v: [i64]) -> i64 { v[0] }\nfn f(n: i64) -> i64 { let s: S = S { a: [40], b: [2] }; consume({ s }.a) }\nfn main() -> i64 { f(1) + 2 }\n",
    "struct S { a: [i64], b: [i64] }\nfn consume(v: [i64]) -> i64 { v[0] }\nfn f(n: i64) -> i64 { let s: S = S { a: [40], b: [2] }; let t: S = S { a: [40], b: [3] }; consume((if n > 0 { s } else { t }).a) }\nfn main() -> i64 { f(1) + 2 }\n",
    // Registers D133 and D161: an effecting body with statements takes the let or chained
    // shape only where the oracle does, and is otherwise lowered straight-line, as the oracle
    // lowers it: statements that do not suspend (an enum value and a `match`, `declassify`)
    // before a `perform` tail, and `main`. A `let` with no annotation bound to a call of a
    // generic effecting fn still takes the let shape alone, and the chained shape in a chain: its
    // type is the instance's, which the classifier leaves to the shape's emitter. A `secret i64`
    // operation fits with no annotation, and under a `secret i64` one as a block's value, which
    // then takes no widen.
    "effect Io { read() -> i64; write(x: i64) -> i64; }\nenum E { A(i64), B }\nfn w() -> i64 ! { Io } { let e: E = E::A(40); let n: i64 = match e { E::A(x) => x, _ => 0 }; perform Io.write(n) }\nfn main() -> i64 { handle w() with { Io.read(k) => k(41), Io.write(x, k) => k(x + 2) } }\n",
    "effect Io { read() -> i64; write(x: i64) -> i64; }\nfn w() -> i64 ! { Io } { let s: secret i64 = 40; let a: i64 = declassify(s); perform Io.write(a) }\nfn main() -> i64 { handle w() with { Io.read(k) => k(41), Io.write(x, k) => k(x + 2) } }\n",
    "effect Io { read() -> i64; }\nfn main() -> i64 ! { Io } { let a: i64 = 1; a + 41 }\n",
    "effect Io { read() -> i64; }\nfn g<T>(x: T) -> T ! { Io } { x }\nfn w() -> i64 ! { Io } { let v = g(41); v + 1 }\nfn main() -> i64 { handle w() with { Io.read(k) => k(41) } }\n",
    "effect Io { read() -> i64; }\nfn g<T>(x: T) -> T ! { Io } { x }\nfn w() -> i64 ! { Io } { let a = g(20); let b = g(21); a + b + 1 }\nfn main() -> i64 { handle w() with { Io.read(k) => k(41) } }\n",
    "effect Io { sread() -> secret i64; }\nfn w() -> i64 ! { Io } { let v = perform Io.sread(); declassify(v) + 1 }\nfn main() -> i64 { handle w() with { Io.sread(k) => k(41) } }\n",
    "effect Io { sread() -> secret i64; }\nfn w() -> i64 ! { Io } { let v: secret i64 = { perform Io.sread() }; declassify(v) + 1 }\nfn main() -> i64 { handle w() with { Io.sread(k) => k(41) } }\n",
    // A `secret i64` `let` over a block ending in a call of a generic effecting fn takes the shape,
    // alone and chained: the oracle binds the type parameter to the expected `secret i64`, so the
    // block's value is not widened. And a `let` over a block that does not suspend, before a
    // `perform` tail, is lowered straight-line.
    "effect Io { read() -> i64; }\nfn g<T>(x: T) -> T ! { Io } { x }\nfn sec5() -> secret i64 { 41 }\nfn w() -> i64 ! { Io } { let v: secret i64 = { g(sec5()) }; declassify(v) + 1 }\nfn main() -> i64 { handle w() with { Io.read(k) => k(41) } }\n",
    "effect Io { read() -> i64; }\nfn g<T>(x: T) -> T ! { Io } { x }\nfn sec5() -> secret i64 { 41 }\nfn w() -> i64 ! { Io } { let a: i64 = perform Io.read(); let v: secret i64 = { g(sec5()) }; declassify(v) + a - 40 }\nfn main() -> i64 { handle w() with { Io.read(k) => k(41) } }\n",
    "effect Io { read() -> i64; write(x: i64) -> i64; }\nfn w() -> i64 ! { Io } { let z: i64 = { let q: i64 = 5; q }; perform Io.write(z + 35) }\nfn main() -> i64 { handle w() with { Io.read(k) => k(41), Io.write(x, k) => k(x + 2) } }\n",
    // A handler arm's names are unbound after the `handle`, so the call after it is the
    // effecting fn's. A tail of the fn's return type is not widened: a `secret i64` operation, or
    // a call of a generic effecting fn, whose type parameter the return type binds.
    "effect Io { read() -> i64; }\neffect Ask { q() -> i64; }\nfn pure1() -> i64 { 5 }\nfn work() -> i64 ! { Io } { perform Io.read() }\nfn w() -> i64 ! { Io } { let x: i64 = handle pure1() with { Ask.q(work) => 7 }; work() }\nfn main() -> i64 { handle w() with { Io.read(k) => k(41) } }\n",
    "effect Io { sread() -> secret i64; }\nfn w() -> secret i64 ! { Io } { let a: i64 = 1; perform Io.sread() }\nfn main() -> i64 { declassify(handle w() with { Io.sread(k) => k(40) }) }\n",
    "effect Io { read() -> i64; }\nfn g<T>(x: T) -> T ! { Io } { x }\nfn w() -> secret i64 ! { Io } { let a: i64 = 1; g(a) }\nfn main() -> i64 { declassify(handle w() with { Io.read(k) => k(40) }) }\n",
    "effect Io { read() -> i64; }\nfn g<T>(x: T) -> T ! { Io } { x }\nfn w() -> i64 ! { Io } { let a: i64 = 42; g(a) }\nfn main() -> i64 { handle w() with { Io.read(k) => k(41) } }\n",
    // The names the walk notes are the fn's own: a fn that binds a name does not make a later fn's
    // call of the fn of that name a call through a bound name, and a fn that calls a fn, or reads
    // one as a value, does not make a later fn's binding of that name refused. So is the refusal
    // it sets: a body without statements that holds a `handle` with a `return` arm does not refuse
    // a later fn's. And a use is refused only of a name the body binds, other than as a parameter,
    // that also names one of the program's fns: not of a fn the body does not bind, nor of a
    // parameter named like one that the body binds no other way, nor of a `let` named like a
    // builtin. A name a `let` inside a `while` binds that the body binds nowhere else stays
    // lowered, in the chained shape too; after a loop, at the top level or in a block, a `let` in
    // a block named like an earlier pattern is not taken for a loop's; nor is a name a pattern
    // binds twice in a loop, `_` among them, which the typer unbinds after each arm, nor a handler
    // arm's parameter bound in a loop and again. A pattern's `_` slot binds no name, so a `let _`
    // in a loop and a `_` slot after it stay lowered.
    "effect Io { read() -> i64; write(x: i64) -> i64; }\nfn work(x: i64) -> i64 ! { Io } { perform Io.write(x) }\nfn a() -> i64 ! { Io } { let work: i64 = perform Io.read(); 41 }\nfn w() -> i64 ! { Io } { let b: i64 = work(40); b + 0 }\nfn main() -> i64 { handle w() with { Io.read(k) => k(41), Io.write(x, k) => k(x + 2) } }\n",
    "effect Io { read() -> i64; write(x: i64) -> i64; }\nfn inc(x: i64) -> i64 { x + 1 }\nfn dbl(x: i64) -> i64 { x * 2 }\nfn a() -> i64 ! { Io } { perform Io.write(inc(apply(dbl, 20))) }\nfn w() -> i64 ! { Io } { let inc: i64 = perform Io.read(); let dbl: i64 = perform Io.read(); 42 }\nfn main() -> i64 { handle w() with { Io.read(k) => k(41), Io.write(x, k) => k(x + 2) } }\n",
    "effect Io { read() -> i64; write(x: i64) -> i64; }\neffect Ask { q(x: i64) -> i64; }\nfn ask1() -> i64 ! { Ask } { perform Ask.q(1) }\nfn work(x: i64) -> i64 ! { Io } { perform Io.write(x) }\nfn a() -> i64 ! { Io } { work(handle ask1() with { Ask.q(x, k) => k(x), return v => v + 1 }) }\nfn w() -> i64 ! { Io } { let b: i64 = perform Io.read(); b + 1 }\nfn main() -> i64 { handle w() with { Io.read(k) => k(41), Io.write(x, k) => k(x + 2) } }\n",
    "effect Io { read() -> i64; write(x: i64) -> i64; }\nfn inc(x: i64) -> i64 { x + 1 }\nfn w() -> i64 ! { Io } { let g: Fn<i64, i64> = inc; perform Io.write(apply(g, 39)) }\nfn main() -> i64 { handle w() with { Io.read(k) => k(41), Io.write(x, k) => k(x + 2) } }\n",
    "effect Io { read() -> i64; write(x: i64) -> i64; }\nfn w() -> i64 ! { Io } { let len: i64 = perform Io.read(); len + 1 }\nfn main() -> i64 { handle w() with { Io.read(k) => k(41), Io.write(x, k) => k(x + 2) } }\n",
    "effect Io { read() -> i64; write(x: i64) -> i64; }\nfn work(x: i64) -> i64 { x + 2 }\nfn w2(work: i64) -> i64 ! { Io } { let a: i64 = work + 1; perform Io.write(a + 37) }\nfn w() -> i64 ! { Io } { w2(1) }\nfn main() -> i64 { handle w() with { Io.read(k) => k(41), Io.write(x, k) => k(x + 2) } }\n",
    "effect Io { read() -> i64; write(x: i64) -> i64; }\nfn w() -> i64 ! { Io } { let a: i64 = { while false { let u: i64 = 7; } perform Io.read() }; let t: i64 = perform Io.read(); a + t - 40 }\nfn main() -> i64 { handle w() with { Io.read(k) => k(41), Io.write(x, k) => k(x + 2) } }\n",
    "effect Io { read() -> i64; write(x: i64) -> i64; }\nenum E { A(i64), B }\nfn w() -> i64 ! { Io } { let mut i: i64 = 0; while i < 1 { i = i + 1; } let n: i64 = { let mut j: i64 = 0; while j < 1 { j = j + 1; } j }; let a: i64 = match E::A(1) { E::A(t) => t, _ => 0 }; let b: i64 = { let t: i64 = 38; t }; perform Io.write(a + b + i + n - 1) }\nfn main() -> i64 { handle w() with { Io.read(k) => k(41), Io.write(x, k) => k(x + 2) } }\n",
    "effect Io { read() -> i64; write(x: i64) -> i64; }\nenum R { Ok(i64), Err(i64) }\nfn w() -> i64 ! { Io } { let mut i: i64 = 0; let mut s: i64 = 0; while i < 2 { let r: R = if i == 0 { R::Ok(20) } else { R::Err(1) }; s = s + (match r { R::Ok(v) => v, R::Err(v) => 0 - v }) + (match R::Ok(i) { R::Ok(_) => 1, R::Err(_) => 0 }); i = i + 1; } perform Io.write(s + 19) }\nfn main() -> i64 { handle w() with { Io.read(k) => k(41), Io.write(x, k) => k(x + 2) } }\n",
    "effect Io { read() -> i64; write(x: i64) -> i64; }\neffect Ask { q(x: i64) -> i64; }\nfn pure0() -> i64 { 5 }\nfn w() -> i64 ! { Io } { let mut i: i64 = 0; let mut s: i64 = 0; while i < 1 { s = s + handle pure0() with { Ask.q(x, k) => x }; i = i + 1; } let b: i64 = handle pure0() with { Ask.q(x, k) => x }; perform Io.write(s + b + 30) }\nfn main() -> i64 { handle w() with { Io.read(k) => k(41), Io.write(x, k) => k(x + 2) } }\n",
    "effect Io { read() -> i64; write(x: i64) -> i64; }\nenum E { A(i64), B }\nfn w() -> i64 ! { Io } { let mut i: i64 = 0; while i < 1 { let _: i64 = i; i = i + 1; } let a: i64 = match E::A(39) { E::A(_) => 39, _ => 0 }; perform Io.write(a + i) }\nfn main() -> i64 { handle w() with { Io.read(k) => k(41), Io.write(x, k) => k(x + 2) } }\n",
    // A handler arm's own continuation, named like the callee, resumed inside the callee's
    // argument, in the let shape and in the chained one: a name bound only as a continuation,
    // which the bound-name rule leaves to the shape, so the call through it resumes, as the
    // oracle's does, and the callee is the fn.
    "effect Io { read() -> i64; write(x: i64) -> i64; }\neffect Ask { q(x: i64) -> i64; }\nfn pure0() -> i64 { 5 }\nfn work(x: i64) -> i64 ! { Io } { perform Io.write(x) }\nfn w() -> i64 ! { Io } { let a: i64 = work(handle pure0() with { Ask.q(x, work) => work(x) } + 33); a + 0 }\nfn main() -> i64 { handle w() with { Io.read(k) => k(41), Io.write(x, k) => k(x + 2) } }\n",
    "effect Io { read() -> i64; write(x: i64) -> i64; }\neffect Ask { q(x: i64) -> i64; }\nfn pure0() -> i64 { 5 }\nfn work(x: i64) -> i64 ! { Io } { perform Io.write(x) }\nfn w() -> i64 ! { Io } { let a: i64 = work(handle pure0() with { Ask.q(x, work) => work(x) } + 33); let a0: i64 = perform Io.read(); a + a0 - 41 }\nfn main() -> i64 { handle w() with { Io.read(k) => k(41), Io.write(x, k) => k(x + 2) } }\n",
    // ADR 0077 A1 (register D139): the chained shape's parent drops its parameters after it pushes
    // the frame (`scg`'s let and embedded shapes refuse such a parameter, D159): a struct holding
    // a `Shared` that no resumer reads is released there, and an array moved into the first
    // `let`'s effecting call is not dropped again.
    "effect Io { read() -> i64; }\nstruct H { s: Shared<i64>, n: i64 }\nfn eff(h: H) -> i64 ! { Io } { let x: i64 = perform Io.read(); let y: i64 = perform Io.read(); x + y }\nfn main() -> i64 { handle eff(H { s: shared_new(5), n: 1 }) with { Io.read(k) => k(21) } }\n",
    "effect Io { read() -> i64; }\nfn take(v: [i64]) -> i64 ! { Io } { perform Io.read() }\nfn eff(a: [i64], c: i64) -> i64 ! { Io } { let x: i64 = take(a); let y: i64 = perform Io.read(); x + y + c }\nfn main() -> i64 { handle eff([1, 2], 0) with { Io.read(k) => k(21) } }\n",
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

/// Registers D150 and D152 (ADR 0041 A15): under codegen `scg` binds a handler arm's
/// parameters as `i64`. Bound as the declared `?i64`, this program's parameter was read as
/// a 16-byte `{ i1, i64 }` from its 8-byte slot, in a module `llvm-as` accepts, because
/// `scg`'s `perform` passes the `5` unwidened (register D132). The oracle's `main` makes the
/// same read, but its `perform` widens the `5` and its module fails there (register
/// D68(c)). This holds the `i64` binding until D132's `perform` half lands (register D152).
#[test]
fn sentinel_codegen_reads_an_arm_parameter_within_its_slot() {
    let tmp =
        std::env::temp_dir().join(format!("snc_selfhost_cg_arm_slot_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(&tmp).expect("create temp dir");
    let cg = build_sentinel_codegen(&tmp);

    let work = tmp.join("work");
    std::fs::create_dir_all(&work).expect("create work dir");
    std::fs::write(
        work.join("input.sentinel"),
        "effect Io { w(x: ?i64) -> i64; }\n\
         fn one() -> i64 ! { Io } { perform Io.w(5) }\n\
         fn main() -> i64 { handle one() with { Io.w(x, k) => if is_some(x) { k(1) } else { k(2) } } }\n",
    )
    .expect("stage the program");
    let out = Command::new(&cg).current_dir(&work).output().expect("run the Sentinel codegen");
    let ir = String::from_utf8_lossy(&out.stdout);
    assert!(ir.contains("@main("), "expected IR from the Sentinel codegen:\n{ir}");
    assert!(
        !ir.contains("load { i1, i64 }"),
        "the arm's parameter was read wider than its `i64` slot:\n{ir}"
    );
}

/// ADR 0072 D4, registers D67 and D69: a continuation frame carries each captured value in
/// one `i64`, so a param that is not `i64` or `secret i64` would be rebuilt in the resumer
/// from one word of itself, and a class or struct rebuilt that way releases words that were
/// never handles when the replay drops it (ADR 0071 A4). `scg` refuses such a fn, with the
/// code inkwell refuses it with: in the embedded and let shapes any non-word param, since they
/// copy every one (register D159), and in the chained shape one its frames carry: one a later
/// `let`'s value or the tail reads outside the forms its frame walk does not enter, such as
/// an `if`, a `match` or a `handle` (D67). The oracle refuses all four programs
/// (`tests/llvm.rs`). A chained fn whose non-word params only the first
/// `let` reads, or none, still lowers, to the oracle's bytes, and so does one where a `match`
/// arm binds a name a non-word param has.
#[test]
fn sentinel_codegen_refuses_a_non_word_param_a_continuation_would_capture() {
    let tmp =
        std::env::temp_dir().join(format!("snc_selfhost_cg_capture_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(&tmp).expect("create temp dir");
    let cg = build_sentinel_codegen(&tmp);
    let work = tmp.join("work");
    std::fs::create_dir_all(&work).expect("create work dir");
    let header = "class K { let n: i64; let s: Shared<i64>; pub init(s: Shared<i64>) { self.n = 5; self.s = s; 0 } }\n\
                  effect Io { read() -> i64; echo(x: i64) -> i64; }\n\
                  fn eat(k: K) -> i64 { 1 }\n";
    let main = "fn main() -> i64 { let s: Shared<i64> = shared_new(42); handle eff(K::init(s)) with { Io.read(kk) => kk(41), Io.echo(x, kk) => kk(x) } }\n";
    for (name, eff) in [
        ("embedded", "fn eff(k: K) -> i64 ! { Io } { eat(k) + perform Io.read() }\n"),
        ("let", "fn eff(k: K) -> i64 ! { Io } { let v: i64 = perform Io.read(); v + eat(k) }\n"),
        (
            "chained",
            "fn eff(k: K) -> i64 ! { Io } { let a: i64 = perform Io.read(); let b: i64 = perform Io.read(); a + b + eat(k) }\n",
        ),
        // Read only by a later `let`'s `perform` argument, in the resumer that evaluates it.
        (
            "chained_later_let",
            "fn eff(k: K) -> i64 ! { Io } { let a: i64 = perform Io.read(); let b: i64 = perform Io.echo(eat(k)); a + b }\n",
        ),
    ] {
        std::fs::write(work.join("input.sentinel"), format!("{header}{eff}{main}"))
            .expect("stage the program");
        let out = Command::new(&cg).current_dir(&work).output().expect("run the Sentinel codegen");
        let stdout = String::from_utf8_lossy(&out.stdout);
        let mut lines = stdout.lines();
        assert!(
            !out.status.success()
                && lines.next() == Some("sentinel::codegen::effecting_fn_body_not_direct")
                && lines.next().is_some_and(|l| l.contains("`k` is captured across the continuation")),
            "{name}: `scg` must refuse a class param its continuation would capture; got {:?}:\n{stdout}",
            out.status
        );
    }
    // The chained shape copies only what its frames carry, so a fn whose non-word params
    // nothing reads, or only the first `let`'s `perform` argument reads (in the parent), lowers,
    // to the oracle's bytes; so does one whose `match` arm binds the name a non-word param has.
    for (name, prog) in [
        (
            "unread",
            format!(
                "{header}fn eff(k: K, flag: bool) -> i64 ! {{ Io }} {{ let a: i64 = perform Io.read(); let b: i64 = perform Io.read(); a + b - 40 }}\n{}",
                main.replace("eff(K::init(s))", "eff(K::init(s), true)")
            ),
        ),
        (
            "first_let_arg",
            "class K { let n: i64; let s: Shared<i64>; pub init(s: Shared<i64>) { self.n = 5; self.s = s; 0 } }\n\
             effect Io { read() -> i64; echo(x: i64) -> i64; }\n\
             fn eat(k: K) -> i64 { 1 }\n\
             fn eff(k: K, flag: bool) -> i64 ! { Io } { let a: i64 = perform Io.echo(eat(k) + 40); let b: i64 = perform Io.read(); a + b - 40 }\n\
             fn main() -> i64 { let s: Shared<i64> = shared_new(42); handle eff(K::init(s), true) with { Io.read(kk) => kk(41), Io.echo(x, kk) => kk(x) } }\n"
                .to_string(),
        ),
        (
            "match_binding_shadows",
            format!(
                "{header}enum E {{ A(i64), B }}\nfn mk(v: i64) -> E {{ E::A(v) }}\n\
                 fn eff(x: K) -> i64 ! {{ Io }} {{ let a: i64 = perform Io.read(); let b: i64 = perform Io.read(); match mk(b) {{ E::A(x) => x + 1, _ => 0 }} }}\n{main}"
            ),
        ),
    ] {
        std::fs::write(work.join("input.sentinel"), prog).expect("stage the program");
        let oracle = Command::new(env!("CARGO_BIN_EXE_snc"))
            .arg("llvm")
            .arg(work.join("input.sentinel"))
            .output()
            .expect("run snc llvm");
        assert!(oracle.status.success(), "{name}: the oracle lowers the chained program");
        let out = Command::new(&cg).current_dir(&work).output().expect("run the Sentinel codegen");
        assert!(
            out.status.success() && out.stdout == oracle.stdout,
            "{name}: `scg` must lower a chained fn whose resumers read no non-word param, to the oracle's bytes; got {:?}:\n{}",
            out.status,
            String::from_utf8_lossy(&out.stdout)
        );
    }
}

/// Registers D133 and D161: `scg` routes an effecting fn whose body has statements as the oracle's
/// `dump_fn_named` routes it (`eff_route`), by all of the oracle's conditions but its capture
/// condition (registers D162 and D165). It takes the let shape or the chained shape only for `let`s
/// whose values produce a continuation and fit one `i64` slot, before a tail that does not suspend,
/// and never in `main`; otherwise it lowers the body straight-line where the oracle does (the seeds
/// above), and refuses it where the oracle does, with the oracle's code and either the oracle's
/// message or the part of it before the type it names. Each program below meets one of those
/// conditions, and `tests/ui`'s fixtures pinned with the code that the oracle refuses too are
/// checked the same way. The exceptions to the message register D161 lists, where `scg` gives the
/// general reason and the oracle names the type rule or a captured parameter, or fails with an
/// internal message, are pinned too. A body without statements is still classified by
/// `eff_classify` (register D160), so the one such fixture `scg` lowers is listed, and must stay
/// lowered until D160 is fixed. A body with statements that calls, anywhere in it, through a name
/// the fn binds anywhere other than only as a handler arm's continuation, or uses such a name
/// otherwise, other than a parameter, that also names a fn, is refused where the oracle lowers it,
/// wherever the binding's scope ends: pinned with a `Fn` value called directly (a parameter, bound
/// in a block, or a handler arm's operation parameter), which `scg` does not lower as the oracle
/// does (ADR 0070's direct-call syntax is not mirrored); with a name `scg`'s typer binds apart from
/// the resolver (register D164), bound in a `while` body and called, or read as a `Fn` value, after
/// it; with a chained `let`'s own or a later `let`'s name called in its value; with such calls
/// inside an argument of an effecting call or of a `perform`; and with calls `scg` lowered to the
/// oracle's bytes before the rule, which it refuses all the same (a fn named like a `let` its
/// argument binds or like a `match` arm's pattern, and a continuation named like an earlier `Fn`
/// value). A body with statements that holds a `handle` with a `return` arm is refused the same
/// way: pinned alone, and with four that also call through a name the `return` arm binds, in a
/// handler arm after a resume or after the `handle`. And so is one in which a `let` inside a
/// `while` binds a name that the body binds again, pinned with a chained `let` of that name after
/// the loop and around it, and with a `let` of that name after the loop in a body lowered
/// straight-line, which `scg` would lower to the oracle's bytes. Two bodies whose IR differs from
/// the oracle's (register D163) must stay lowered. And a ported type error inside a body the oracle
/// refuses is the answer, as it is the oracle's, whose typer runs before its code generation.
#[test]
fn sentinel_codegen_refuses_an_effecting_body_the_oracle_refuses() {
    const CODE: &str = "sentinel::codegen::effecting_fn_body_not_direct";
    // Register D160: a body without statements that the oracle refuses and `scg` lowers.
    const D160_LOWERED: &[&str] = &["c35_effecting_call_in_operand.sentinel"];
    let tmp = std::env::temp_dir().join(format!("snc_selfhost_cg_route_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(&tmp).expect("create temp dir");
    let cg = build_sentinel_codegen(&tmp);
    let work = tmp.join("work");
    std::fs::create_dir_all(&work).expect("create work dir");
    let input = work.join("input.sentinel");
    let oracle_says = |input: &Path| -> (bool, String) {
        let out = Command::new(env!("CARGO_BIN_EXE_snc"))
            .arg("llvm")
            .arg(input)
            .output()
            .expect("run snc llvm");
        let err = String::from_utf8_lossy(&out.stderr).to_string();
        let msg = err.lines().find_map(|l| l.strip_prefix("snc: llvm: ")).unwrap_or("").to_string();
        (out.status.success(), msg)
    };
    // `scg`'s refusal: the code, then the oracle's message or the part of it before "; ".
    let scg_refuses_as = |name: &str, oracle_msg: &str| {
        let out = Command::new(&cg).current_dir(&work).output().expect("run the Sentinel codegen");
        let stdout = String::from_utf8_lossy(&out.stdout);
        let mut lines = stdout.lines();
        let code = lines.next().unwrap_or("");
        let msg = lines.next().and_then(|l| l.strip_prefix("scg: ")).unwrap_or("");
        let matches = !msg.is_empty()
            && (oracle_msg == msg || oracle_msg.starts_with(&format!("{msg}; ")));
        assert!(
            !out.status.success() && code == CODE && matches && stdout.lines().count() == 2,
            "{name}: `scg` must refuse what the oracle refuses (`{oracle_msg}`); got {:?}:\n{stdout}",
            out.status
        );
    };

    let io = "effect Io { read() -> i64; write(x: i64) -> i64; flag() -> bool; }\n";
    let main = "fn main() -> i64 { handle w() with { Io.read(k) => k(41), Io.write(x, k) => k(x + 2), Io.flag(k) => k(true) } }\n";
    let g = "fn g<T>(x: T) -> T ! { Io } { x }\n";
    let ask = "effect Ask { q() -> i64; }\nfn pure1() -> i64 { 5 }\n";
    let programs: Vec<(&str, String)> = vec![
        // Two or more statements that are not all such `let`s, or whose tail suspends.
        ("plain_let_then_suspending_let", "fn w() -> i64 ! { Io } { let a: i64 = 1; let b: i64 = perform Io.read(); a + b }\n".into()),
        ("chain_then_suspending_tail", "fn w() -> i64 ! { Io } { let a: i64 = perform Io.read(); let b: i64 = perform Io.read(); perform Io.write(a + b - 84) }\n".into()),
        ("chain_with_nullable_let", "fn w() -> i64 ! { Io } { let a: i64 = perform Io.read(); let b: ?i64 = perform Io.read(); a + 1 }\n".into()),
        ("while_that_performs", "fn w() -> i64 ! { Io } { let mut i: i64 = 0; while i < 2 { i = i + perform Io.read(); } i }\n".into()),
        // One statement: not a `let` of that kind, a type that does not fit, a suspending tail.
        ("suspending_statement", "fn w() -> i64 ! { Io } { perform Io.write(1); 42 }\n".into()),
        ("nullable_let", "fn w() -> i64 ! { Io } { let v: ?i64 = perform Io.read(); 42 }\n".into()),
        ("bool_let", "fn w() -> i64 ! { Io } { let v: bool = perform Io.flag(); 42 }\n".into()),
        ("unannotated_bool_let", "fn w() -> i64 ! { Io } { let v = perform Io.flag(); 42 }\n".into()),
        ("let_then_suspending_tail", "fn w() -> i64 ! { Io } { let a: i64 = perform Io.read(); perform Io.write(a - 1) }\n".into()),
        ("plain_let_then_embedded_tail", "fn w() -> i64 ! { Io } { let s: i64 = 1; perform Io.read() + s }\n".into()),
        // A block's value is widened on its tail, not on itself, so the oracle gives its general
        // reason.
        ("nullable_let_over_block", "fn w() -> i64 ! { Io } { let v: ?i64 = { perform Io.read() }; 42 }\n".into()),
        // Under `secret i64` a block's value is widened on its tail, and produces no continuation;
        // nor does a block with a statement that suspends.
        ("secret_let_over_block", "fn w() -> i64 ! { Io } { let v: secret i64 = { perform Io.read() }; declassify(v) + 1 }\n".into()),
        ("block_with_suspending_statement", "fn w() -> i64 ! { Io } { let v: i64 = { perform Io.write(1); perform Io.read() }; v + 1 }\n".into()),
        // A suspension inside a construct: it may suspend, and produces no continuation.
        ("if_value", "fn w() -> i64 ! { Io } { let c: i64 = 1; let a: i64 = if c == 1 { perform Io.read() } else { 0 }; a + 1 }\n".into()),
        ("array_of_performs", "fn w() -> i64 ! { Io } { let a: [i64] = [perform Io.read()]; 42 }\n".into()),
        ("match_arm_that_performs", "enum E { A(i64), B }\nfn w() -> i64 ! { Io } { let e: E = E::A(40); let n: i64 = match e { E::A(x) => perform Io.write(x), _ => 0 }; n }\n".into()),
        ("return_that_performs", "fn w() -> i64 ! { Io } { let a: i64 = 1; let b: i64 = if a == 2 { return perform Io.read() } else { a }; perform Io.write(b + 39) }\n".into()),
        // A resume inside a `handle` whose body never performs, which the oracle counts; and the
        // same with the continuation named like an effecting fn, whose name is the arm's.
        ("resume_in_handle", format!("{ask}fn w() -> i64 ! {{ Io }} {{ let x: i64 = handle pure1() with {{ Ask.q(k) => k(1) }}; x + 37 }}\n")),
        ("resume_named_like_effecting_fn", format!("{ask}fn work() -> i64 ! {{ Io }} {{ perform Io.read() }}\nfn w() -> i64 ! {{ Io }} {{ let x: i64 = handle pure1() with {{ Ask.q(work) => work(1) }}; x + 37 }}\n")),
        // A `let` with no annotation bound to a generic effecting call, typed `bool` only once
        // lowered: in the let shape, first in a chain (the parent) and second (a resumer).
        ("generic_bool_let", format!("{g}fn w() -> i64 ! {{ Io }} {{ let v = g(true); 42 }}\n")),
        ("generic_bool_first_in_chain", format!("{g}fn w() -> i64 ! {{ Io }} {{ let a = g(true); let b = g(20); b + 22 }}\n")),
        ("generic_bool_second_in_chain", format!("{g}fn w() -> i64 ! {{ Io }} {{ let a = g(20); let b = g(true); a + 22 }}\n")),
    ];
    for (name, body) in &programs {
        std::fs::write(&input, format!("{io}{body}{main}")).expect("stage the program");
        let (ok, oracle_msg) = oracle_says(&input);
        assert!(!ok && oracle_msg.starts_with("effecting fn `"), "{name}: the oracle must refuse it: {oracle_msg}");
        scg_refuses_as(name, &oracle_msg);
    }
    // `main` takes neither shape.
    std::fs::write(&input, format!("{io}fn main() -> i64 ! {{ Io }} {{ let a: i64 = perform Io.read(); 42 }}\n"))
        .expect("stage the program");
    let (ok, oracle_msg) = oracle_says(&input);
    assert!(!ok && oracle_msg.starts_with("effecting fn `main`"), "main: the oracle must refuse it: {oracle_msg}");
    scg_refuses_as("main", &oracle_msg);
    // A tail of a type other than the fn's return type is widened to it, so it produces no
    // continuation, and still suspends.
    std::fs::write(
        &input,
        format!(
            "{io}fn w() -> secret i64 ! {{ Io }} {{ let a: i64 = 1; perform Io.write(a) }}\nfn main() -> i64 {{ declassify(handle w() with {{ Io.read(k) => k(41), Io.write(x, k) => k(x + 2), Io.flag(k) => k(true) }}) }}\n"
        ),
    )
    .expect("stage the program");
    let (ok, oracle_msg) = oracle_says(&input);
    assert!(!ok && oracle_msg.starts_with("effecting fn `w`"), "tail_widened: the oracle must refuse it: {oracle_msg}");
    scg_refuses_as("tail_widened", &oracle_msg);

    // A ported type error in the tail of a body the oracle refuses: `scg` records its refusal
    // once it has walked the body (`eff_route`'s refusals) or lowered it (a shape's emitter's,
    // for a `let` it could type only by lowering it), so the type error is recorded first.
    let p = "struct P { a: i64 }\n";
    let typed: Vec<(&str, String)> = vec![
        ("suspending_statement_then_type_error", format!("{p}fn w() -> i64 ! {{ Io }} {{ perform Io.write(1); {{ let q: P = P {{ a: 1 }}; q.zz }} }}\n")),
        ("bool_let_then_type_error", format!("{p}fn w() -> i64 ! {{ Io }} {{ let v: bool = perform Io.flag(); {{ let q: P = P {{ a: 1 }}; q.zz }} }}\n")),
        ("chain_with_bool_let_then_type_error", format!("{p}fn w() -> i64 ! {{ Io }} {{ let u: i64 = perform Io.read(); let v: bool = perform Io.flag(); {{ let q: P = P {{ a: u }}; q.zz }} }}\n")),
        ("generic_bool_let_then_type_error", format!("{p}{g}fn w() -> i64 ! {{ Io }} {{ let v = g(true); {{ let q: P = P {{ a: 1 }}; q.zz }} }}\n")),
        ("generic_bool_first_in_chain_then_type_error", format!("{p}{g}fn w() -> i64 ! {{ Io }} {{ let a = g(true); let b = g(20); {{ let q: P = P {{ a: b }}; q.zz }} }}\n")),
        ("generic_bool_second_in_chain_then_type_error", format!("{p}{g}fn w() -> i64 ! {{ Io }} {{ let a = g(20); let b = g(true); {{ let q: P = P {{ a: a }}; q.zz }} }}\n")),
    ];
    for (name, body) in &typed {
        std::fs::write(&input, format!("{io}{body}{main}")).expect("stage the program");
        let oracle = Command::new(env!("CARGO_BIN_EXE_snc"))
            .arg("types")
            .arg(&input)
            .output()
            .expect("run snc types");
        let oracle_err = String::from_utf8_lossy(&oracle.stderr).to_string();
        let oracle_msg = oracle_err.lines().find_map(|l| l.strip_prefix("snc: ")).unwrap_or("");
        assert!(
            !oracle.status.success() && oracle_msg == "struct `P` has no field `zz`",
            "{name}: the oracle must report the type error: {oracle_err}"
        );
        let out = Command::new(&cg).current_dir(&work).output().expect("run the Sentinel codegen");
        let stdout = String::from_utf8_lossy(&out.stdout);
        let mut lines = stdout.lines();
        let code = lines.next().unwrap_or("");
        let msg = lines.next().and_then(|l| l.strip_prefix("scg: ")).unwrap_or("");
        assert!(
            !out.status.success()
                && code == "sentinel::types::unknown_field"
                && msg == oracle_msg
                && stdout.lines().count() == 2,
            "{name}: `scg` must report the oracle's type error; got {:?}:\n{stdout}",
            out.status
        );
    }

    // Register D161's exceptions to the message. A `let` of a type that does not fit,
    // bound to a block that ends in a `perform`: the oracle names the type rule unless the
    // block's value is widened (as in `nullable_let_over_block` above), which `scg` does not
    // tell apart before typing, so it gives the general reason.
    std::fs::write(
        &input,
        format!("{io}fn w() -> i64 ! {{ Io }} {{ let v: bool = {{ perform Io.flag() }}; 42 }}\n{main}"),
    )
    .expect("stage the program");
    let (ok, oracle_msg) = oracle_says(&input);
    assert!(
        !ok && oracle_msg.starts_with("effecting fn `w` cannot be lowered: a `let` bound to a suspension must be"),
        "bool_let_over_block: the oracle names the type rule: {oracle_msg}"
    );
    scg_refuses_as(
        "bool_let_over_block",
        "effecting fn `w` cannot be lowered: a `perform` or a call to an effecting fn appears outside tail position, which needs a reified frame",
    );
    // In `main`, which takes no shape, a `let` with no annotation bound to a call of a generic
    // effecting fn whose return type is its type parameter, or to a block ending in one, at an
    // instance that does not fit. And a `let` of a type that does not fit in a body that calls
    // through a name the fn binds, which `scg` refuses with the general reason: a `Fn` value
    // called directly, or a fn named like a name a `while` body bound before the call (register
    // D164).
    let inc = "fn inc(x: i64) -> i64 { x + 1 }\n";
    for (name, prog, who) in [
        ("main_generic_instance_unfit", format!("{io}{g}fn main() -> i64 ! {{ Io }} {{ let v = g(true); 42 }}\n"), "main"),
        ("main_generic_instance_unfit_over_a_block", format!("{io}{g}fn main() -> i64 ! {{ Io }} {{ let v = {{ g(true) }}; 42 }}\n"), "main"),
        (
            "unfit_let_before_a_fn_value_tail",
            format!("{io}{inc}fn w2(f: Fn<i64, i64>) -> i64 ! {{ Io }} {{ let v: bool = perform Io.flag(); f(1) }}\nfn w() -> i64 ! {{ Io }} {{ w2(inc) }}\n{main}"),
            "w2",
        ),
        (
            "unfit_let_before_a_call_named_like_a_while_binding",
            format!("{io}{inc}fn work(x: i64) -> i64 {{ x + 41 }}\nfn w() -> i64 ! {{ Io }} {{ let v: bool = perform Io.flag(); {{ let mut i: i64 = 0; while i < 1 {{ let work: Fn<i64, i64> = inc; i = i + 1; }} work(1) }} }}\n{main}"),
            "w",
        ),
    ] {
        std::fs::write(&input, prog).expect("stage the program");
        let (ok, oracle_msg) = oracle_says(&input);
        assert!(
            !ok && oracle_msg.starts_with(&format!("effecting fn `{who}` cannot be lowered: a `let` bound to a suspension must be")),
            "{name}: the oracle names the type rule: {oracle_msg}"
        );
        scg_refuses_as(name, &format!("effecting fn `{who}` cannot be lowered: a `perform` or a call to an effecting fn appears outside tail position, which needs a reified frame"));
    }
    // And a body the oracle refuses for a parameter its frame would carry that is not `i64` or
    // `secret i64`, which also calls through a name the fn binds (here the parameter itself):
    // `scg` refuses it with the general reason before its shapes check their parameters.
    std::fs::write(
        &input,
        format!("{io}{inc}fn w2(f: Fn<i64, i64>) -> i64 ! {{ Io }} {{ let a: i64 = perform Io.read(); f(a) }}\nfn w() -> i64 ! {{ Io }} {{ w2(inc) }}\n{main}"),
    )
    .expect("stage the program");
    let (ok, oracle_msg) = oracle_says(&input);
    assert!(
        !ok && oracle_msg.starts_with("effecting fn `w2` cannot be lowered: `f` is captured across the continuation"),
        "fn_parameter_captured_and_called: the oracle names the captured parameter: {oracle_msg}"
    );
    scg_refuses_as(
        "fn_parameter_captured_and_called",
        "effecting fn `w2` cannot be lowered: a `perform` or a call to an effecting fn appears outside tail position, which needs a reified frame",
    );
    // And a body whose lowering the oracle fails with an internal message (register D165), which
    // holds a `handle` with a `return` arm: `scg` refuses it with the general reason.
    std::fs::write(
        &input,
        format!("{io}{ask}fn w2(n: i64) -> i64 ! {{ Io }} {{ let a: i64 = perform Io.read(); handle pure1() with {{ Ask.q(k) => 7, return u => u + a + n - 5 }} }}\nfn w() -> i64 ! {{ Io }} {{ w2(1) }}\n{main}"),
    )
    .expect("stage the program");
    let (ok, oracle_msg) = oracle_says(&input);
    assert!(
        !ok && oracle_msg == "read of an unbound var",
        "oracle_internal_failure_with_a_return_arm: the oracle fails with its internal message: {oracle_msg}"
    );
    scg_refuses_as(
        "oracle_internal_failure_with_a_return_arm",
        "effecting fn `w2` cannot be lowered: a `perform` or a call to an effecting fn appears outside tail position, which needs a reified frame",
    );

    // `tests/ui`'s fixtures pinned with the code that the oracle refuses too (`snc build`
    // refuses some the oracle lowers).
    let root = workspace_root();
    let snaps = root.join("crates/sentinel-driver/tests/snapshots");
    let mut fixtures: Vec<String> = Vec::new();
    for entry in std::fs::read_dir(&snaps).expect("read snapshots") {
        let path = entry.expect("dir entry").path();
        let text = std::fs::read_to_string(&path).unwrap_or_default();
        let fixture = text
            .lines()
            .find_map(|l| l.strip_prefix("expression: \"reject_stderr(\\\""))
            .and_then(|rest| rest.split("\\\"").next())
            .map(str::to_string);
        let code = text
            .splitn(3, "---")
            .nth(2)
            .and_then(|body| body.lines().map(str::trim).find(|l| !l.is_empty()));
        if let (Some(fixture), Some(CODE)) = (fixture, code) {
            fixtures.push(fixture);
        }
    }
    fixtures.sort();
    let mut checked = 0;
    for fixture in &fixtures {
        std::fs::copy(root.join("tests/ui").join(fixture), &input).expect("stage the fixture");
        let (ok, oracle_msg) = oracle_says(&input);
        if ok {
            continue;
        }
        if D160_LOWERED.contains(&fixture.as_str()) {
            let out = Command::new(&cg).current_dir(&work).output().expect("run the Sentinel codegen");
            assert!(
                out.status.success(),
                "{fixture}: `scg` now refuses it; register D160 is fixed for it, so delete its entry"
            );
            continue;
        }
        scg_refuses_as(fixture, &oracle_msg);
        checked += 1;
    }
    assert!(checked >= 3, "expected at least three refused `tests/ui` fixtures, got {checked}");

    // Bodies the oracle lowers that call through a name the fn binds other than as a handler arm's
    // continuation, which `scg` refuses wherever the binding's scope ends (`eff_check_called`): a
    // direct call through a `Fn` value, which `scg` does not lower as the oracle does (ADR 0070's
    // direct-call syntax is not mirrored), here a parameter, which the walk also counts as a call
    // that may suspend (`eff_kind`).
    let direct = format!(
        "{io}fn inc(x: i64) -> i64 {{ x + 1 }}\nfn w2(f: Fn<i64, i64>) -> i64 ! {{ Io }} {{ let a: i64 = f(1); perform Io.write(a + 38) }}\nfn w() -> i64 ! {{ Io }} {{ w2(inc) }}\n{main}"
    );
    std::fs::write(&input, direct).expect("stage the program");
    let (ok, _) = oracle_says(&input);
    assert!(ok, "fn_value_call: the oracle lowers it");
    scg_refuses_as(
        "fn_value_call",
        "effecting fn `w2` cannot be lowered: a `perform` or a call to an effecting fn appears outside tail position, which needs a reified frame",
    );

    // The same with the value bound in a block. A name `scg`'s typer binds apart from the resolver
    // (register D164), bound in a `while` body and called after it, which `scg` lowers apart from
    // the oracle, or read after it as a `Fn` value. A chained `let`'s own name, or a later `let`'s,
    // called in its value, which the chained shape binds before it lowers any value. And calls
    // `scg` lowered to the oracle's bytes before the rule, refused all the same: a fn named like a
    // `let` its argument binds, alone and chained, or like a `match` arm's pattern, and a
    // continuation named like an earlier `Fn` value. And a body that holds a `handle` with a
    // `return` arm, which `scg` lowers inside the handler arm at each resume site (`eff_ret`):
    // alone, and with a call through a name the `return` arm binds, which `scg`'s typer also binds
    // at each resume site, in a handler arm after a resume (the arm's own continuation when the
    // `return` arm's value shares its name, or a fn named like its value or like a `let` its body
    // binds) or after the `handle`. And a name a `let` inside a `while` binds that the body binds
    // again as a chained `let`, after the loop or around it, which the chained shape binds before
    // it lowers any value (`eff_check_while`), and, the rule being the simple one, as a `let` after
    // the loop in a body lowered straight-line, which `scg` would lower to the oracle's bytes.
    let work1 = "fn work(x: i64) -> i64 ! { Io } { perform Io.write(x) }\n";
    let askq = "effect Ask { q(x: i64) -> i64; }\nfn ask0() -> i64 ! { Ask } { perform Ask.q(0) }\nfn ask1() -> i64 ! { Ask } { perform Ask.q(1) }\n";
    for (name, body) in [
        ("fn_value_bound_in_a_block", "fn w() -> i64 ! { Io } { let z: i64 = { let f: Fn<i64, i64> = inc; 0 }; f(41) }\n".to_string()),
        (
            "continuation_named_like_the_return_arm_value",
            format!("{askq}{work1}fn w() -> i64 ! {{ Io }} {{ let a: i64 = work(handle ask0() with {{ Ask.q(x, k) => if x > 0 {{ k(x) }} else {{ k(x + 5) }}, return k => k + 1 }}); a + 0 }}\n"),
        ),
        (
            "fn_named_like_the_return_arm_value",
            format!("{askq}{work1}fn r(x: i64) -> i64 {{ x + 2 }}\nfn w() -> i64 ! {{ Io }} {{ let a: i64 = work(handle ask1() with {{ Ask.q(x, k) => {{ let b: i64 = k(x); r(b) }}, return r => r + 1 }}); a + 0 }}\n"),
        ),
        (
            "fn_named_like_a_let_the_return_arm_binds",
            format!("{askq}{work1}fn r(x: i64) -> i64 {{ x + 2 }}\nfn w() -> i64 ! {{ Io }} {{ let a: i64 = work(handle ask1() with {{ Ask.q(x, k) => {{ let b: i64 = k(x); r(b) }}, return v => {{ let r: i64 = v; r + 1 }} }}); a + 0 }}\n"),
        ),
        (
            "chained_let_named_like_its_own_callee",
            format!("{work1}fn w() -> i64 ! {{ Io }} {{ let work: i64 = work(40); let b: i64 = perform Io.read(); work + b - 41 }}\n"),
        ),
        (
            "chained_let_named_like_an_earlier_callee",
            format!("{work1}fn f(x: i64) -> i64 {{ x + 3 }}\nfn w() -> i64 ! {{ Io }} {{ let a: i64 = work(f(1)); let f: i64 = perform Io.read(); a + f - 47 }}\n"),
        ),
        (
            "callee_named_like_a_let_its_argument_binds",
            format!("{work1}fn w() -> i64 ! {{ Io }} {{ let a: i64 = work({{ let work: i64 = 38; work }}); a + 2 }}\n"),
        ),
        (
            "chained_callee_named_like_a_let_its_argument_binds",
            format!("{work1}fn w() -> i64 ! {{ Io }} {{ let a: i64 = perform Io.read(); let b: i64 = work({{ let work: i64 = 1; 1 }}); a + b - 2 }}\n"),
        ),
        (
            "fn_named_like_a_match_arm_pattern",
            format!("enum E {{ A(i64), B }}\n{work1}fn w() -> i64 ! {{ Io }} {{ let a: i64 = work(match E::A(40) {{ E::A(work) => work, _ => 0 }}); a + 0 }}\n"),
        ),
        (
            "continuation_named_like_an_earlier_fn_value",
            format!("{askq}{work1}fn pure0() -> i64 {{ 5 }}\nfn w() -> i64 ! {{ Io }} {{ let a: i64 = {{ let k1: Fn<i64, i64> = inc; work(handle pure0() with {{ Ask.q(x, k1) => k1(x) }} + 33) }}; a + 0 }}\n"),
        ),
        (
            "fn_value_bound_in_a_while_body",
            format!("{work1}fn w() -> i64 ! {{ Io }} {{ let b: i64 = {{ let mut i: i64 = 0; while i < 1 {{ let work: Fn<i64, i64> = inc; i = i + 1; }} work(1) }}; b + 40 }}\n"),
        ),
        (
            "fn_value_bound_in_a_top_level_while_body",
            format!("{work1}fn w() -> i64 ! {{ Io }} {{ let mut i: i64 = 0; while i < 1 {{ let work: Fn<i64, i64> = inc; i = i + 1; }} work(40) }}\n"),
        ),
        (
            "fn_value_bound_in_a_while_body_read_after_it",
            "fn dbl(x: i64) -> i64 { x * 2 }\nfn w() -> i64 ! { Io } { let mut i: i64 = 0; while i < 1 { let inc: Fn<i64, i64> = dbl; i = i + 1; } perform Io.write(apply(inc, 39)) }\n".to_string(),
        ),
        (
            "fn_value_bound_in_a_return_arm",
            format!("{ask}{work1}fn w() -> i64 ! {{ Io }} {{ let n: i64 = handle pure1() with {{ Ask.q(k) => 7, return v => {{ let work: Fn<i64, i64> = inc; v }} }}; work(n) }}\n"),
        ),
        (
            "while_name_bound_again_by_a_later_chained_let",
            "fn w() -> i64 ! { Io } { let a: i64 = { while false { let t: i64 = 7; } perform Io.read() }; let t: i64 = perform Io.read(); a + t - 40 }\n".to_string(),
        ),
        (
            "while_name_bound_again_by_the_chained_let_around_it",
            "fn w() -> i64 ! { Io } { let a: i64 = perform Io.read(); let t: i64 = { while false { let t: i64 = 7; } perform Io.read() }; a + t - 40 }\n".to_string(),
        ),
        (
            "while_name_bound_again_by_a_later_let",
            "fn w() -> i64 ! { Io } { let mut i: i64 = 0; while i < 1 { let t: i64 = 7; i = i + t; } let t: i64 = 34; perform Io.write(t + i - 1) }\n".to_string(),
        ),
        (
            "handle_with_a_return_arm",
            format!("{askq}{work1}fn w() -> i64 ! {{ Io }} {{ let z: i64 = 0; work(handle ask1() with {{ Ask.q(x, k) => k(x + 1), return v => v * 2 }}) }}\n"),
        ),
    ] {
        std::fs::write(&input, format!("{io}{inc}{body}{main}")).expect("stage the program");
        let (ok, _) = oracle_says(&input);
        assert!(ok, "{name}: the oracle lowers it");
        scg_refuses_as(name, "effecting fn `w` cannot be lowered: a `perform` or a call to an effecting fn appears outside tail position, which needs a reified frame");
    }

    // The same calls inside an argument of an effecting call or of a `perform`, whose kind the
    // walk does not weigh: the body is refused wherever the call is (`eff_check_called`), here
    // also through a handler arm's operation parameter, an arm parameter other than its
    // continuation.
    for (name, prog, who) in [
        (
            "fn_operation_parameter_called_in_a_perform_argument",
            format!("{io}{inc}effect H {{ app(f: Fn<i64, i64>) -> i64; }}\nfn pure0() -> i64 {{ 40 }}\nfn w() -> i64 ! {{ Io }} {{ let z: i64 = 0; perform Io.write(handle pure0() with {{ H.app(f, k) => f(1) }}) }}\n{main}"),
            "w",
        ),
        (
            "callee_binds_its_own_name_to_a_fn_value_in_its_argument",
            format!("{io}{inc}{work1}fn w() -> i64 ! {{ Io }} {{ let a: i64 = work({{ let work: Fn<i64, i64> = inc; work(37) }}); a + 0 }}\n{main}"),
            "w",
        ),
        (
            "fn_value_parameter_called_in_an_effecting_call_argument",
            format!("{io}{inc}{work1}fn w2(f: Fn<i64, i64>) -> i64 ! {{ Io }} {{ let z: i64 = 0; work(f(37)) }}\nfn w() -> i64 ! {{ Io }} {{ w2(inc) }}\n{main}"),
            "w2",
        ),
        (
            "fn_value_bound_in_a_block_called_in_a_perform_argument",
            format!("{io}{inc}fn w() -> i64 ! {{ Io }} {{ let z: i64 = 0; perform Io.write({{ let f: Fn<i64, i64> = inc; f(37) }}) }}\n{main}"),
            "w",
        ),
    ] {
        std::fs::write(&input, prog).expect("stage the program");
        let (ok, _) = oracle_says(&input);
        assert!(ok, "{name}: the oracle lowers it");
        scg_refuses_as(name, &format!("effecting fn `{who}` cannot be lowered: a `perform` or a call to an effecting fn appears outside tail position, which needs a reified frame"));
    }

    // Register D163: a chained `let` that reads an earlier `let` with no annotation through a
    // call of a generic effecting fn is typed from that untyped `let`, so its instance is named
    // apart from the oracle's; its type fits, so the body stays lowered (`cg_chained_unfit`).
    for (name, body) in [
        ("generic_call_reads_an_unannotated_let", "fn w() -> i64 ! { Io } { let a = perform Io.read(); let b = g(a); a + b - 40 }\n"),
        ("annotated_generic_call_reads_an_unannotated_let", "fn w() -> i64 ! { Io } { let a = perform Io.read(); let b: i64 = g(a); a + b - 40 }\n"),
    ] {
        std::fs::write(&input, format!("{io}{g}{body}{main}")).expect("stage the program");
        let (ok, _) = oracle_says(&input);
        assert!(ok, "{name}: the oracle lowers it");
        let out = Command::new(&cg).current_dir(&work).output().expect("run the Sentinel codegen");
        assert!(
            out.status.success(),
            "{name}: `scg` must lower it:\n{}",
            String::from_utf8_lossy(&out.stdout)
        );
    }
}

/// ADR 0074 (register D79): the five `handler_arm_exits` programs, built from the
/// `snc llvm` oracle's IR and run. `scg` emits that IR byte-for-byte (they are seeds
/// above), so this runs the text back ends' releases; `tests/handler_arm_exits.rs`
/// runs the same programs through `snc build`. Besides the exit values, it sees which
/// slot a release reads once its arm has resumed: had `after`'s `return` release read
/// the `handle`'s dispatch slot instead of the arm's, `c74_arm_return_leaves_the_arm`
/// would exit differently. Until an arm resumes the two slots hold the same kont, so it
/// cannot tell them apart there (every `break` / `continue` release here, for one);
/// `tests/llvm.rs` checks every release's slot.
#[test]
fn oracle_ir_of_the_handler_arm_exit_programs_runs() {
    let tmp = std::env::temp_dir().join(format!("snc_arm_exits_oracle_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(&tmp).expect("create temp dir");
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/handler_arm_exits");
    for (stem, want) in [
        ("c74_arm_declines_to_resume", 143),
        ("c74_arm_return_leaves_the_arm", 34),
        ("c74_arm_break_continue", 11),
        ("c74_resume_arg_leaves_the_arm", 13),
        ("c74_two_open_arms", 16),
    ] {
        let oracle = Command::new(env!("CARGO_BIN_EXE_snc"))
            .arg("llvm")
            .arg(dir.join(format!("{stem}.sentinel")))
            .output()
            .expect("run snc llvm");
        assert!(
            oracle.status.success(),
            "snc llvm failed on {stem}:\n{}",
            String::from_utf8_lossy(&oracle.stderr)
        );
        let ll = tmp.join(format!("{stem}.ll"));
        std::fs::write(&ll, &oracle.stdout).expect("write the oracle's IR");
        let exe = compile_ll_to_exe(&ll, &tmp.join(stem));
        let run = Command::new(&exe).output().expect("run the program built from the oracle's IR");
        assert_eq!(
            run.status.code(),
            Some(want),
            "{stem}: built from the oracle's IR it exits {:?}; stderr:\n{}",
            run.status,
            String::from_utf8_lossy(&run.stderr)
        );
    }
    let _ = std::fs::remove_dir_all(&tmp);
}

/// ADR 0075 D6 / A1: the arm-remainder programs, built from the `snc llvm` oracle's IR and
/// RUN.
///
/// The codegen differential holds `scg` to the oracle byte for byte, which proves they AGREE
/// — not that either is right. If both emit the same wrong IR the differential stays green,
/// and the corpus-wide behaviour check (`llvm_behaviour_matches_inkwell_over_emitted_subset`
/// in `tests/llvm.rs`) cannot run on a box without a `libsentinel_runtime.a`. So run these
/// directly. Each misses 42 if the oracle drops a remainder instead of replaying it. Beyond
/// that, in the ORACLE's output:
///
/// - `c75_remainder_moves_a_local` misses it if a resumer is named after a symbol two
///   defines share (`llc` rejects the redefinition), or if a resumer's drops disagree with
///   its parent's drop plan for a value the remainder moves.
/// - `c75_remainder_capture_types` misses it if a remainder whose only capture is an `i64`
///   is refused rather than replayed (its bubble then aborts).
///
/// Whether `scg` RECORDS a remainder's moves is `scg`'s mechanism, not the oracle's; the
/// corpus differential is what catches that.
#[test]
fn oracle_ir_of_the_arm_remainder_programs_runs() {
    let tmp = std::env::temp_dir().join(format!("snc_armrem_oracle_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(&tmp).expect("create temp dir");
    let dir = workspace_root().join("tests/pass");
    for (stem, want) in [
        ("c75_bubble_replays_the_remainder", 42),
        ("c75_remainder_moves_a_local", 42),
        ("c75_remainder_capture_types", 42),
    ] {
        let oracle = Command::new(env!("CARGO_BIN_EXE_snc"))
            .arg("llvm")
            .arg(dir.join(format!("{stem}.sentinel")))
            .output()
            .expect("run snc llvm");
        assert!(
            oracle.status.success(),
            "snc llvm failed on {stem}:\n{}",
            String::from_utf8_lossy(&oracle.stderr)
        );
        let ll = tmp.join(format!("{stem}.ll"));
        std::fs::write(&ll, &oracle.stdout).expect("write the oracle's IR");
        let exe = compile_ll_to_exe(&ll, &tmp.join(stem));
        let run = Command::new(&exe).output().expect("run the program built from the oracle's IR");
        assert_eq!(
            run.status.code(),
            Some(want),
            "{stem}: built from the oracle's IR it exits {:?}; stderr:\n{}",
            run.status,
            String::from_utf8_lossy(&run.stderr)
        );
    }
    let _ = std::fs::remove_dir_all(&tmp);
}

/// ADR 0071 D2 amendment A1, RUN from the oracle's IR, for the same reason as the test
/// above: the codegen differential holds `scg` to the oracle byte for byte, which proves they
/// agree, not that the count is right, and the corpus-wide behaviour check cannot run here.
/// `tests/pass/c71_shared_place_duplications` duplicates a `Shared` out of a place into a new
/// owner in every position the amendment counts, and a `Mutex` in two of them; a position the
/// oracle does not clone releases one unit too many, and the runtime's refcount check aborts
/// instead of the program answering 94.
#[test]
fn oracle_ir_of_the_shared_duplication_program_runs() {
    let tmp = std::env::temp_dir().join(format!("snc_shdup_oracle_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(&tmp).expect("create temp dir");
    let stem = "c71_shared_place_duplications";
    let src = workspace_root().join("tests/pass").join(format!("{stem}.sentinel"));
    let oracle = Command::new(env!("CARGO_BIN_EXE_snc"))
        .arg("llvm")
        .arg(&src)
        .output()
        .expect("run snc llvm");
    assert!(
        oracle.status.success(),
        "snc llvm failed on {stem}:\n{}",
        String::from_utf8_lossy(&oracle.stderr)
    );
    let ll = tmp.join(format!("{stem}.ll"));
    std::fs::write(&ll, &oracle.stdout).expect("write the oracle's IR");
    let exe = compile_ll_to_exe(&ll, &tmp.join(stem));
    let run = Command::new(&exe).output().expect("run the program built from the oracle's IR");
    assert_eq!(
        run.status.code(),
        Some(94),
        "{stem}: built from the oracle's IR it exits {:?}; stderr:\n{}",
        run.status,
        String::from_utf8_lossy(&run.stderr)
    );
    let _ = std::fs::remove_dir_all(&tmp);
}

/// ADR 0071 D2 amendment A4 (register D156): the oracle's IR of `c71_class_field_handles`
/// runs and answers 42. Before the amendment the text back ends failed eleven of its cases —
/// five of them only there, since inkwell's method and init frames keep their handles (A2) —
/// and `pass_c71_class_field_handles` builds the file through inkwell alone; the corpus-wide
/// behaviour check cannot run on Windows, and the codegen differential holds `scg` to the
/// oracle byte for byte, so this is what runs the oracle's output.
#[test]
fn oracle_ir_of_the_class_field_program_runs() {
    let tmp = std::env::temp_dir().join(format!("snc_clsfld_oracle_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(&tmp).expect("create temp dir");
    let stem = "c71_class_field_handles";
    let src = workspace_root().join("tests/pass").join(format!("{stem}.sentinel"));
    let oracle = Command::new(env!("CARGO_BIN_EXE_snc"))
        .arg("llvm")
        .arg(&src)
        .output()
        .expect("run snc llvm");
    assert!(
        oracle.status.success(),
        "snc llvm failed on {stem}:\n{}",
        String::from_utf8_lossy(&oracle.stderr)
    );
    let ll = tmp.join(format!("{stem}.ll"));
    std::fs::write(&ll, &oracle.stdout).expect("write the oracle's IR");
    let exe = compile_ll_to_exe(&ll, &tmp.join(stem));
    let run = Command::new(&exe).output().expect("run the program built from the oracle's IR");
    assert_eq!(
        run.status.code(),
        Some(42),
        "{stem}: built from the oracle's IR it exits {:?}; stderr:\n{}",
        run.status,
        String::from_utf8_lossy(&run.stderr)
    );
    let _ = std::fs::remove_dir_all(&tmp);
}

/// ADR 0075 A1, for the one effecting-fn shape the corpus differential cannot hold: an
/// EMBEDDED shape (`perform Op(arg) + rest`) with a bubbling `handle` in each of its two
/// defines -- one in the perform's argument, which the parent lowers, and one in the rest
/// of the tail, which the `__resume_` frame lowers. Each define is its own `Emit`, so the
/// two must continue ONE arm-remainder sequence (numbering each from 0 defines
/// `@__armrem_emb_0` twice) and emit their resumers after the LAST define, in the order they
/// were lowered (flushing each define's own puts the parent's between `@emb` and
/// `@__resume_emb`). `snc build`
/// refuses the program, and `scg` lowers a perform's argument a second time (register
/// D100), so it cannot sit in the corpus the differential compares: this pins the oracle
/// alone, and runs what it emits.
#[test]
fn oracle_ir_of_an_embedded_shape_with_a_handle_in_each_define_runs() {
    let src = concat!(
        "effect Io { read() -> i64; }\n",
        "effect Ask { get(x: i64) -> i64; }\n",
        "fn two() -> i64 ! { Io } {\n",
        "    let a: i64 = perform Io.read();\n",
        "    let b: i64 = perform Io.read();\n",
        "    a + b\n",
        "}\n",
        "fn emb() -> i64 ! { Ask } {\n",
        "    perform Ask.get(handle two() with { Io.read(k) => { let r: i64 = k(1); r + 1 } })\n",
        "        + (handle two() with { Io.read(k) => { let r: i64 = k(1); r + 10 } })\n",
        "}\n",
        "fn main() -> i64 { handle emb() with { Ask.get(x, k) => k(x) } }\n",
    );
    let tmp = std::env::temp_dir().join(format!("snc_armrem_embedded_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(&tmp).expect("create temp dir");
    let input = tmp.join("input.sentinel");
    std::fs::write(&input, src).expect("write the program");
    let oracle = Command::new(env!("CARGO_BIN_EXE_snc"))
        .arg("llvm")
        .arg(&input)
        .output()
        .expect("run snc llvm");
    assert!(
        oracle.status.success(),
        "snc llvm failed:\n{}",
        String::from_utf8_lossy(&oracle.stderr)
    );
    let text = String::from_utf8(oracle.stdout).expect("utf-8 IR");
    let defines: Vec<&str> = text.lines().filter(|l| l.starts_with("define ")).collect();
    let at = |sym: &str| {
        let head = format!("@{sym}(");
        let found: Vec<usize> = (0..defines.len()).filter(|&i| defines[i].contains(&head)).collect();
        assert_eq!(found.len(), 1, "@{sym} is defined {} times in:\n{text}", found.len());
        found[0]
    };
    let last_frame = at("__resume_emb");
    for sym in ["__armrem_emb_0", "__armrem_emb_1"] {
        assert!(at(sym) > last_frame, "@{sym} is emitted before the fn's last define:\n{text}");
    }
    assert!(
        at("__armrem_emb_0") < at("__armrem_emb_1"),
        "the resumers are out of lowering order:\n{text}"
    );
    let ll = tmp.join("emb.ll");
    std::fs::write(&ll, &text).expect("write the oracle's IR");
    let exe = compile_ll_to_exe(&ll, &tmp.join("emb"));
    let run = Command::new(&exe).output().expect("run the program built from the oracle's IR");
    // 4 + 22: the argument's `handle` answers 2 + 1 + 1, the tail's 2 + 10 + 10.
    assert_eq!(
        run.status.code(),
        Some(26),
        "built from the oracle's IR it exits {:?}; stderr:\n{}",
        run.status,
        String::from_utf8_lossy(&run.stderr)
    );
    let _ = std::fs::remove_dir_all(&tmp);
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
/// source. `lexer.sentinel` and `parser.sentinel` are the self-contained stages (no
/// `use`), so they lower through the single-file pipeline; they exercise the WHOLE
/// Bar-A construct set at real scale — thousands of `.ll` lines each — far beyond the
/// small corpus fixtures. (This sentence used to quote line counts for both, and both
/// had rotted: `parser.sentinel`'s was pre-ADR-0067, from before the file was split
/// into parts. A count asserted in prose is worth nothing here; `wc -l` is worth
/// everything.) The multi-module stages (types/codegen/…) need
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
        // ADR 0067: a multi-file stage (parser) is staged AS input.sentinel, so its
        // parts resolve under `input/` (the staged stem) for both the oracle + cg.
        let parts_src = root.join("selfhost").join(stage);
        if parts_src.is_dir() {
            let pd = work.join("input");
            std::fs::create_dir_all(&pd).expect("create input parts dir");
            for ent in std::fs::read_dir(&parts_src).expect("read stage parts") {
                let p = ent.expect("dir entry").path();
                if p.extension().and_then(|x| x.to_str()) == Some("sentinel") {
                    std::fs::copy(&p, pd.join(p.file_name().unwrap())).expect("stage a part");
                }
            }
        }
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
    stage_module_parts(&root, &run, "types");
    stage_module_parts(&root, &run, "merge");
    stage_module_parts(&root, &run, "parser");

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

// ---------------------------------------------------------------------------
// The EXTENDED program differential: real multi-module programs, not just the
// curated single-file fixture corpus.
//
// `collect_fixtures` above sweeps only `tests/pass` + `tests/ui` — single-file
// fixtures written to exercise one construct each. That left the harness
// structurally blind to divergence in REAL programs: a verification pass found
// that 7 of the 14 `examples/lang/` programs the oracle emits diverged from
// `scg`, ALL silently (both sides exit 0) — including a catastrophic miscompile
// (`fn_value.sentinel`: the oracle emits an indirect call, `scg` emitted an
// effect-continuation resume) and an ABI-shaped break (`sealed_*`: a
// `SealedChannel` param typed `i64` where the oracle uses `ptr`). Most of those
// are KNOWN-deferred features — but "known" lived only in prose, so a genuine
// REGRESSION in that surface would have been equally invisible.
//
// This closes the blind spot: it sweeps `examples/`, `sentinel_library/` and
// `tools/`, and every divergence must be either fixed or listed in
// `DEFERRED_PROGRAMS` with the ADR that defers it. The list IS the deliverable —
// it converts an invisible gap into an auditable one, and a deferred program
// that starts matching fails the test so the list cannot rot.

/// Programs whose `scg` divergence is a KNOWN, deliberately-deferred feature
/// gap, each with the ADR/decision that defers it. A listed program is still RUN
/// (so a crash, or the oracle ceasing to emit, is still noticed) but its
/// byte-difference is not a failure. Deleting an entry is how a mirror slice
/// records that it closed the gap.
///
/// NOTE the asymmetry that makes this list tolerable: `snc build` uses the
/// inkwell backend, so every program here COMPILES AND RUNS CORRECTLY today
/// (asserted end-to-end in `examples.rs`). The divergence is between the
/// hand-maintained `snc llvm` text ORACLE and `scg` — it constrains the
/// self-host story, not the shipped compiler.
const DEFERRED_PROGRAMS: &[(&str, &str)] = &[
    // ADR 0070 D3-revisit: a `Fn<T,R>`-typed local called DIRECTLY (`op(5)`).
    // scg's `dump_te_call` keeps ADR 0020 D5's "vars win over fns" dispatch
    // unconditionally, so it lowers the call as a kont RESUME rather than an
    // indirect call — the most severe entry here: wrong code, not a missing
    // feature.
    ("examples/lang/fn_value.sentinel", "ADR 0070 D3-revisit: direct call of a Fn-typed var is unmirrored in scg (lowers as a kont resume)"),
    ("examples/lang/fn_value_generic.sentinel", "ADR 0070 M-cont: generic Fn<T,R> instantiations are snc-only"),
    // (ADR 0069 / ADR 0066 M2.4a-c, register D51 — the THREE `sealed_*` entries that
    // used to sit here are GONE, deleted rather than re-labelled. They were all one
    // defect: `type_of_typeexpr` had no `SealedChannel` arm, so the annotation fell
    // through to `struct_lookup`, found nothing, and resolved to the i64 placeholder —
    // `scg` typed a `SealedChannel` param `i64` where the oracle types `ptr`. Adding the
    // arm made all three byte-identical to the oracle in one change. Deleting the
    // entries is the proof; do not re-add them.
    //
    // ⚠ Their stated reason — "an ABI-shaped divergence", i.e. valid IR on both sides —
    // was FALSE, and register D52 records why that mattered: all three emitted
    // `llvm-as`-INVALID IR from `scg` while the oracle was clean, which is exactly what
    // the `llvm_rejects` gate exists to catch, and `deferred_reason` switches that gate
    // OFF for any listed program. An invalid-IR case sat under two labels that each
    // asserted it could not be invalid, with the one automated check that would have
    // noticed disabled for precisely those programs.)
    // (ADR 0066 M1.2b-cont `channel_generic` + M1.2c `addressed_reply` were here —
    // generic word-scalar channel elements and channel-of-channels, both snc-only
    // because scg's channel typing hardcoded an i64 element and its lowering had no
    // encode/decode. Both are DELETED, not re-labelled: the scg mirror landed
    // (element-generic channel typing + send/recv encode/decode), so both are now
    // byte-identical to the oracle. The oracle itself was extended in the same slice
    // — its text backend had never learned to send a channel HANDLE, so it errored
    // on channel-of-channels where inkwell succeeded.)
    // (ADR 0066 M2.3b / register D34 — the PROCESS-channel entry that used to sit here
    // is GONE, deleted 2026-09-03 rather than re-labelled. `scg` now mirrors
    // process_send/process_recv (fids 29/30): the element ENCODE reuses D17's
    // `cg_container_encode`, the DECODE its twin, and `process_recv`'s result element
    // is context-typed off the EXPECTED type the way `channel_new` is — a `Process`
    // is anonymous and carries no element, so it cannot come from the argument.
    // `examples/lang/process_channel_typed.sentinel` is byte-identical to the oracle
    // and `llvm-as` clean. Deleting the entry is the proof; do not re-add it.)
    // Register D24/D25/D26, NOT D15 (which is fixed in both Rust back ends). THREE
    // independent scg gaps, each verified on this exact program:
    //   D24 — scg's monomorphisation worklist has ZERO transitive closure. It seeds
    //         only from NON-generic bodies, so a generic fn reached through another
    //         generic fn's body is CALLED and never DEFINED: `@wrap__i64` here. It
    //         needs no generic struct at all — `fn ident<T>(x: T) -> T` called from a
    //         generic `outer` is enough.
    //   D25 — scg takes no generic-struct FIELD closure, so it emits
    //         `%Wrap_bool = type { %Box_bool }` while never declaring `%Box_bool` —
    //         an undefined TYPE rather than an undefined function (`llvm-as`: "use of
    //         undefined type named 'Box_bool'").
    //   D26 — scg emits a spurious `%Struct.2 = type { i64 }`: a runtime layout for
    //         the generic DECL `Shelf<S>`, which has none. The Rust back ends skip
    //         generic decls in Pass 0.
    // All three are unrelated to the interner-staleness D15 fixed: they fire with no
    // substitution-born instance anywhere. ⚠ This entry also exempts the program from
    // the `llvm_rejects` validity gate below (it is guarded by `deferred_reason`), and
    // scg's IR for it IS invalid — that is disclosed here rather than discovered later.
    // Delete this entry when scg grows a real worklist, and re-check all three gaps.
    // (register D24/D25/D26 — the `generic_calls_generic` entry that used to sit here is
    // GONE, deleted 2026-09-03 rather than re-labelled. All three are fixed: `scg` now
    // takes a transitive mono closure (discovery pass under a LIFO stack, emission in
    // discovery order), re-reads the interner bound while laying out generic-instance
    // fields, and decides struct genericity from the DECL's type-param count instead of
    // scanning its fields. The program is byte-identical to the oracle and `llvm-as`
    // clean; `tests/pass/c16_transitive_mono_order.sentinel` additionally pins the
    // emission ORDER, which this program's single chain cannot distinguish. Do not
    // re-add it.)
];

/// Programs whose divergence is a REAL BUG in `scg`, not a deferred feature —
/// kept separate from `DEFERRED_PROGRAMS` on purpose. Conflating "we chose not
/// to port this yet" with "this is wrong" is precisely the invisible-gap problem
/// this test exists to end, so a bug listed here must carry its DIAGNOSIS, and
/// the entry is a debt marker to be deleted by a fix — never by a re-label.
///
/// Both entries below were found by this test on its first run; neither was
/// known before, and neither is reachable through `tests/pass`.
///
/// `examples/sys/process_ids.sentinel` used to head this list — the `extern "C"`
/// bug, closed across three commits (the merge rename half, the merge emitter
/// half, then ADR 0057 extern support in scg's types/codegen). It is deleted, not
/// re-labelled, which is the only way an entry may leave.
const KNOWN_SCG_BUGS: &[(&str, &str)] = &[
    // MOSTLY FIXED (36 diff lines -> 8). scg's merge never recorded trait (54)
    // or class (56) declarations in its rename map, so it emitted
    // `@Logged__init` / `@default__Logged__Meter__tick` where the oracle emits
    // `@input$Logged__init` / `@default__input$Logged__input$Meter__tick` — the
    // right instruction shape under the wrong symbol names, and a cross-module
    // collision hazard. Both kinds are now recorded.
    // WHAT REMAINS: a NAMED impl's own name (`impl Add as Meter for Dial`, which
    // codegen mangles into `<Name>__<Type>__<Trait>__<method>`) is still not
    // qualified — scg emits `@Add__input$Dial__input$Meter__tick` vs the
    // oracle's `@input$Add__…`. Recording it in `build_rename` IS the missing
    // half, but doing so alone makes scg CRASH downstream
    // ("index out of bounds: idx=-1, len=5"), so some later impl-name lookup
    // still expects the bare name and must be fixed in the same change. A
    // wrong-symbol divergence beats a crash, so it stays registered.
    ("examples/lang/delegation.sentinel", "scg does not qualify a NAMED impl's own name (`@Add__…` vs `@input$Add__…`); trait/class halves FIXED"),
];

/// The deferral reason for `rel` (a repo-relative, forward-slashed path).
fn deferred_reason(rel: &str) -> Option<&'static str> {
    DEFERRED_PROGRAMS
        .iter()
        .chain(KNOWN_SCG_BUGS.iter())
        .find(|(p, _)| *p == rel)
        .map(|(_, why)| *why)
}

/// Recursively collect `.sentinel` files under `dir`.
fn collect_under(dir: &Path, out: &mut Vec<PathBuf>) {
    if !dir.is_dir() {
        return;
    }
    for entry in std::fs::read_dir(dir).expect("read dir") {
        let path = entry.expect("dir entry").path();
        if path.is_dir() {
            collect_under(&path, out);
        } else if path.extension().and_then(|e| e.to_str()) == Some("sentinel") {
            out.push(path);
        }
    }
}

/// The real-program corpus: every `.sentinel` under `demos/`, `examples/`,
/// `sentinel_library/` and `tools/`. Library modules with no `main` simply fail
/// the oracle and are skipped, exactly like a deferred construct.
///
/// `demos/` was MISSING from this list until a review caught it — the same
/// species of hole the real-program sweeps exist to close, a directory of real
/// programs nothing compared. Kept in step with the seven stage differentials,
/// which enumerate the same four roots.
fn collect_programs() -> Vec<PathBuf> {
    let root = workspace_root();
    let mut out = Vec::new();
    for sub in ["demos", "examples", "sentinel_library", "tools"] {
        collect_under(&root.join(sub), &mut out);
    }
    out.sort();
    out
}

/// Does LLVM accept `ll`? Returns `Some(reason)` when it does NOT.
///
/// This exists because a byte-DIFFERENCE against the oracle is a tolerable,
/// registerable state — but INVALID IR never is, and the two are independent:
/// an adversarial review found scg emitting non-assemblable IR for a program
/// whose divergence was already registered, so the whole suite stayed green
/// while scg produced output no backend would accept. Byte-comparison alone
/// cannot see that; this can.
///
/// NOTE this used to say `llvm-as` exits 0 when verification fails, so that only
/// stderr was a usable signal. Measured on the LLVM 18.1.8 this repo builds
/// against, that is false: it exits **1** for a verifier failure ("assembly
/// parsed, but does not verify as correct!") exactly as it does for a parse
/// error, and writes no `.bc`. The stderr test below is kept as belt-and-braces —
/// it costs nothing and this claim has already been wrong once — but the exit
/// code is the signal. Skipped (returns `None`) when `LLVM_SYS_180_PREFIX` is
/// unset, so the test still runs without an LLVM install; `llvm_as()` in
/// `llvm.rs` deliberately does NOT make that concession, because a gate that
/// checks nothing is how the defect this one was written for stayed hidden.
fn llvm_rejects(ll: &Path) -> Option<String> {
    let prefix = std::env::var("LLVM_SYS_180_PREFIX").ok()?;
    let tool = PathBuf::from(prefix).join("bin").join(if cfg!(windows) {
        "llvm-as.exe"
    } else {
        "llvm-as"
    });
    if !tool.exists() {
        return None;
    }
    let out = Command::new(&tool)
        .arg(ll)
        .arg("-o")
        .arg(ll.with_extension("bc"))
        .output()
        .ok()?;
    let stderr = String::from_utf8_lossy(&out.stderr);
    if out.status.success() && !stderr.contains("does not verify") {
        return None;
    }
    Some(stderr.lines().take(2).collect::<Vec<_>>().join(" / "))
}

/// Copy `src`'s CONTENTS into `dst` recursively (so `sentinel_library/std` lands
/// at `<dst>/std`). Mirrors `examples.rs`'s `assemble()` staging, which is what
/// makes `use std::…` / `use Sentinel::…` resolve: module discovery roots at the
/// entry file's parent directory, for the oracle and for scg's own self-hosted
/// discover+merge alike.
fn copy_tree_contents(src: &Path, dst: &Path) {
    std::fs::create_dir_all(dst).expect("create dst");
    for entry in std::fs::read_dir(src).expect("read_dir") {
        let entry = entry.expect("dir entry");
        let from = entry.path();
        let to = dst.join(entry.file_name());
        if from.is_dir() {
            copy_tree_contents(&from, &to);
        } else {
            std::fs::copy(&from, &to).expect("copy file");
        }
    }
}

#[test]
fn sentinel_codegen_matches_oracle_on_real_programs() {
    let tmp = std::env::temp_dir().join(format!("snc_selfhost_cg_prog_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(&tmp).expect("create temp dir");
    let cg = build_sentinel_codegen(&tmp);

    let work = tmp.join("work");
    std::fs::create_dir_all(&work).expect("create work dir");
    // Stage the first-party libraries next to the entry so `use std::…` /
    // `use Sentinel::…` resolves for BOTH sides.
    copy_tree_contents(&workspace_root().join("sentinel_library"), &work);
    let input = work.join("input.sentinel");

    let programs = collect_programs();
    assert!(
        programs.len() > 50,
        "expected a substantial program corpus, got {}",
        programs.len()
    );

    let root = workspace_root();
    let mut emitted = 0usize;
    let mut mismatches: Vec<String> = Vec::new();
    let mut stale: Vec<String> = Vec::new();
    let mut unverifiable: Vec<String> = Vec::new();
    let scg_ll = tmp.join("scg_out.ll");
    let oracle_ll = tmp.join("oracle_out.ll");
    for program in &programs {
        let rel = program
            .strip_prefix(&root)
            .expect("under root")
            .to_string_lossy()
            .replace('\\', "/");
        let bytes = std::fs::read(program).expect("read program");
        std::fs::write(&input, &bytes).expect("stage input");
        let oracle = Command::new(env!("CARGO_BIN_EXE_snc"))
            .arg("llvm")
            .arg(&input)
            .output()
            .expect("run snc llvm");
        if !oracle.status.success() {
            continue; // not in the emitted subset (deferred construct / not a program)
        }
        emitted += 1;
        let sentinel = Command::new(&cg)
            .current_dir(&work)
            .output()
            .expect("run the Sentinel codegen");
        // Validity is checked INDEPENDENTLY of byte-equality: a registered
        // divergence excuses different BYTES, never invalid IR.
        //
        // It is a DIFFERENTIAL check — scg is only at fault when the oracle's
        // own IR verifies and scg's does not. That distinction is load-bearing:
        // the hand-maintained text oracle currently emits IR LLVM rejects for
        // ~30 real programs (dominated by shift-operand width mismatches such as
        // `shl i32 %v4, %v7` with an i64 amount, across every chacha20/ssh/ct
        // example), and scg faithfully MIRRORS it — which is byte-correct and
        // exactly what this differential asks of it. Those are oracle defects,
        // tracked separately; inkwell (`snc build`) is unaffected, so the
        // programs themselves compile and run correctly.
        // A REGISTERED program is exempt (its entry states the severity — two of
        // them record "scg emits INVALID IR" explicitly); an unregistered one is
        // not, so a NEW instance of this class fails loudly instead of hiding.
        std::fs::write(&scg_ll, &sentinel.stdout).expect("stage scg .ll");
        if deferred_reason(&rel).is_none() {
            if let Some(why) = llvm_rejects(&scg_ll) {
                std::fs::write(&oracle_ll, &oracle.stdout).expect("stage oracle .ll");
                if llvm_rejects(&oracle_ll).is_none() {
                    unverifiable.push(format!("  {rel}: {why}"));
                }
            }
        }
        if oracle.stdout == sentinel.stdout {
            if deferred_reason(&rel).is_some() {
                stale.push(format!(
                    "  {rel} now MATCHES the oracle — delete it from \
                     DEFERRED_PROGRAMS / KNOWN_SCG_BUGS"
                ));
            }
            continue;
        }
        if deferred_reason(&rel).is_some() {
            continue; // a registered gap: an ADR-deferred feature or a tracked bug
        }
        mismatches.push(format!(
            "  {rel} (oracle {} bytes vs sentinel {} bytes)",
            oracle.stdout.len(),
            sentinel.stdout.len()
        ));
    }

    assert!(
        emitted >= 10,
        "expected the oracle to emit for a meaningful number of real programs, got {emitted}"
    );
    assert!(stale.is_empty(), "DEFERRED_PROGRAMS is stale:\n{}", stale.join("\n"));
    assert!(
        unverifiable.is_empty(),
        "scg emitted IR that LLVM will not accept for {} program(s) where the ORACLE's \
         IR verifies cleanly — a registered byte-divergence excuses different bytes, \
         never IR no backend would accept:\n{}",
        unverifiable.len(),
        unverifiable.join("\n")
    );
    assert!(
        mismatches.is_empty(),
        "the Sentinel codegen diverged from `snc llvm` on {}/{} emitted real program(s) \
         NOT registered as deferred:\n{}",
        mismatches.len(),
        emitted,
        mismatches.join("\n")
    );

    let _ = std::fs::remove_dir_all(&tmp);
}

/// ADR 0041 A14 (register D97): the self-hosted code generator (`scg`) refuses what the oracle refuses,
/// for the refusals `scg` ports (`common::assert_refuses_what_the_oracle_refuses`).
#[test]
fn sentinel_codegen_refuses_what_the_oracle_refuses() {
    let tmp = std::env::temp_dir()
        .join(format!("snc_selfhost_codegen_refusals_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(&tmp).expect("create temp dir");
    let driver = build_sentinel_codegen(&tmp);
    let checked = common::assert_refuses_what_the_oracle_refuses(&driver, &tmp.join("refusals"));
    assert!(checked >= 10, "expected at least ten pinned refusals, got {checked}");
}
