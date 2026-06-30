//! ADR 0066 M2.4b real-pipe end-to-end test: a parent (initiator/client) spawns a
//! child (responder/server) and runs the full `SealedChannel` flow over a REAL
//! process pipe — the authenticated x25519 handshake (parent `process_send` ↔ child
//! `stdin_recv`, child `stdout_send` ↔ parent `process_recv`), then a sealed record
//! that re-emerges `secret` on the verified child.
//!
//! Unlike the in-process `examples/lang/sealed_session.sentinel` (which runs both KEX
//! halves in one program), this drives the handshake across two real processes. It is
//! the consumer that exercises the M2.4b self-stdin/stdout builtins (`stdin_recv` /
//! `stdout_send`) end-to-end — the differential corpus never calls them.
//!
//! Mechanics: both halves hardcode the SAME deterministic test vectors; the parent
//! pins the child's ed25519 host key. The child exits with the recovered sealed value
//! (42); the parent `process_wait`s it and exits 42. The parent's `process_spawn`
//! target is templated in (`__CHILD_PATH__` → the compiled child's absolute path)
//! since Sentinel has no argv/own-path reflection yet.
//!
//! Like `examples.rs`, this build links, so it needs the host link toolchain on PATH
//! (the MSVC env on Windows). It is gated on the child binary actually building.

use std::path::{Path, PathBuf};
use std::process::Command;

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .canonicalize()
        .expect("canonicalize repo root")
}

fn fixture_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/sealed_pipe")
}

fn copy_dir_recursive(src: &Path, dst: &Path) {
    std::fs::create_dir_all(dst).expect("create dst dir");
    for entry in std::fs::read_dir(src).expect("read_dir") {
        let entry = entry.expect("dir entry");
        let from = entry.path();
        let to = dst.join(entry.file_name());
        if from.is_dir() {
            copy_dir_recursive(&from, &to);
        } else {
            std::fs::copy(&from, &to).expect("copy file");
        }
    }
}

/// `snc build <entry> -o <out>` (the merge path); assert success.
fn snc_build(entry: &Path, out: &Path) {
    let res = Command::new(env!("CARGO_BIN_EXE_snc"))
        .arg("build")
        .arg(entry)
        .arg("-o")
        .arg(out)
        .output()
        .expect("run snc build");
    assert!(
        res.status.success(),
        "snc build of {} failed; stderr:\n{}",
        entry.display(),
        String::from_utf8_lossy(&res.stderr),
    );
}

#[test]
fn sealed_pipe_handshake_round_trip() {
    let root = repo_root();
    let fixtures = fixture_dir();
    let dir = std::env::temp_dir().join(format!("snc_sealed_pipe_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("create temp project dir");

    // The library tree must sit next to the entries so `use std::security::...`
    // resolves (the examples.rs convention).
    copy_dir_recursive(&root.join("sentinel_library"), &dir);

    // Build the CHILD (server). Its absolute path is what the parent spawns.
    std::fs::copy(fixtures.join("child.sentinel"), dir.join("child.sentinel"))
        .expect("copy child source");
    let child_exe = dir.join(format!("sealed_pipe_child{}", std::env::consts::EXE_SUFFIX));
    snc_build(&dir.join("child.sentinel"), &child_exe);
    assert!(child_exe.exists(), "child binary was not produced");

    // Template the PARENT (client): substitute the child's absolute path (forward-
    // slashed — accepted by Command::new on every host, and free of backslash-escape
    // ambiguity in the Sentinel string literal).
    let child_fwd = child_exe.to_string_lossy().replace('\\', "/");
    let parent_src = std::fs::read_to_string(fixtures.join("parent.sentinel.in"))
        .expect("read parent template")
        .replace("__CHILD_PATH__", &child_fwd);
    let parent_path = dir.join("parent.sentinel");
    std::fs::write(&parent_path, parent_src).expect("write parent source");
    let parent_exe = dir.join(format!("sealed_pipe_parent{}", std::env::consts::EXE_SUFFIX));
    snc_build(&parent_path, &parent_exe);

    // Run the parent: it spawns the child, runs the handshake over the real pipe,
    // seals 42 and sends it; the child opens it and exits 42; the parent waits + exits
    // 42 iff it authenticated the child AND the value round-tripped.
    let run = Command::new(&parent_exe).output().expect("run parent binary");
    let code = run.status.code().expect("parent exited normally");
    assert_eq!(
        code, 42,
        "sealed-pipe round-trip exit {code} != 42; parent stderr:\n{}",
        String::from_utf8_lossy(&run.stderr),
    );
}
