//! A postfix method call on a REFERENCE-typed receiver, end-to-end.
//!
//! ADR 0022 D7's auto-deref resolves `c.m()` on a `c: &K` by dereferencing the
//! receiver to find the method — `check_method_call_expr`'s `recv_concrete` maps
//! `Type::Ref(rid)` to `refs[rid].inner`. Codegen passed the receiver's own
//! storage as `self`, which for a ref-typed place is the slot HOLDING the
//! pointer rather than the object: one indirection too many. A `&Self` method
//! then read the slot as an object, and a `&mut Self` method wrote into it.
//!
//! These complement `tests/pass/c22_ref_receiver_method_call.sentinel` rather
//! than duplicating it. That fixture is in the stage differentials' corpus, so
//! it pins the stronger property — `snc llvm` and the self-hosted `scg` emit
//! byte-identical IR for the shape — and asserts one combined exit code. These
//! three separate the receiver kinds, so a regression says WHICH one broke: a
//! shared reference, a mutable one, or the owned/`self` controls that must not
//! change. They were written before the `scg` mirror existed, when a corpus
//! fixture would have failed the codegen differential for a reason unrelated to
//! the program; nothing under `crates/sentinel-driver/tests/` is swept.
//!
//! Like `examples.rs` and `deadlock.rs`, the build links (needs the host link
//! toolchain).

use std::path::{Path, PathBuf};
use std::process::Command;

fn temp_dir() -> PathBuf {
    let dir = std::env::temp_dir().join(format!("snc_ref_receiver_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("create temp dir");
    dir
}

fn build_and_run(src: &str, dir: &Path, stem: &str) -> i32 {
    let entry = dir.join(format!("{stem}.sentinel"));
    std::fs::write(&entry, src).expect("write source");
    let exe = dir.join(format!("{stem}{}", std::env::consts::EXE_SUFFIX));
    let res = Command::new(env!("CARGO_BIN_EXE_snc"))
        .arg("build")
        .arg(&entry)
        .arg("-o")
        .arg(&exe)
        .output()
        .expect("run snc build");
    assert!(
        res.status.success(),
        "build of {stem} failed; stderr:\n{}",
        String::from_utf8_lossy(&res.stderr),
    );
    let run = Command::new(&exe).output().expect("run the built program");
    run.status.code().expect("program exited with a code")
}

/// A `&Self` method reached through a `&K` binding reads the OBJECT.
///
/// Before the fix this returned the address of `c`'s own slot reinterpreted as
/// the field — a frame pointer, so the value varied run to run with ASLR.
#[test]
fn shared_method_through_a_reference_receiver_reads_the_object() {
    let dir = temp_dir();
    let exit = build_and_run(
        r#"
class K {
    let v: i64;
    pub init(v: i64) { self.v = v; 0 }
    pub fn peek(self: &Self) -> i64 { self.v }
}
fn read(c: &K) -> i64 { c.peek() }
fn main() -> i64 {
    let k: K = K::init(42);
    read(&k)
}
"#,
        &dir,
        "shared_recv",
    );
    assert_eq!(exit, 42, "expected the field, got {exit}");
}

/// A `&mut Self` method reached through a `&mut K` binding writes the OBJECT.
///
/// The read case above is the visible half; this is the one that mattered. The
/// store landed at the wrong address, so `k.v` kept its old value: the program
/// answered 42 + 7 instead of 99 + 7. `guard_a` is a neighbouring local, asserted
/// intact so a fix that merely redirected the write somewhere else still fails.
#[test]
fn mutable_method_through_a_reference_receiver_writes_the_object() {
    let dir = temp_dir();
    let exit = build_and_run(
        r#"
class K {
    let v: i64;
    pub init(v: i64) { self.v = v; 0 }
    pub fn bump(self: &mut Self) -> i64 { self.v = 99; 0 }
    pub fn peek(self: &Self) -> i64 { self.v }
}
fn poke(c: &mut K) -> i64 { c.bump() }
fn main() -> i64 {
    let mut k: K = K::init(42);
    let guard_a: i64 = 7;
    let discard: i64 = poke(&mut k);
    k.peek() + guard_a
}
"#,
        &dir,
        "mut_recv",
    );
    assert_eq!(exit, 106, "expected 99 + 7; got {exit}");
}

/// The controls, in one program: a method on an OWNED receiver (the alloca IS
/// the object), a method on `self` from inside another method (`self` is bound
/// to the object pointer under the CLASS type, so it takes the lvalue path
/// unchanged), and a chain through both. A fix keyed on the wrong thing — the
/// expression kind rather than `Type::Ref` — breaks one of these.
#[test]
fn owned_and_self_receivers_are_unaffected() {
    let dir = temp_dir();
    let exit = build_and_run(
        r#"
class K {
    let v: i64;
    pub init(v: i64) { self.v = v; 0 }
    pub fn peek(self: &Self) -> i64 { self.v }
    pub fn twice(self: &Self) -> i64 { self.peek() + self.peek() }
}
fn read(c: &K) -> i64 { c.peek() }
fn main() -> i64 {
    let k: K = K::init(20);
    k.twice() + read(&k) + k.peek()
}
"#,
        &dir,
        "owned_recv",
    );
    assert_eq!(exit, 80, "expected 40 + 20 + 20; got {exit}");
}
