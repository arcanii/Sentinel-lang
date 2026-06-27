//! ADR 0059 — the C-ABI EXPORT harness: build a Sentinel `export "C"` library
//! with `snc build --lib`, generate its C header, compile a C driver against
//! the library, run it, and assert the exit code. This is the cross-language
//! proof — a C program calls INTO Sentinel and gets the verified-constant-time
//! primitive over a plain C ABI (the inverse of the `extern "C"` FFI import).
//!
//! The demonstrator lives at `examples/export/{ct_select.sentinel, driver.c}`
//! (a library + its C driver, not a runnable `examples/` program — so it is
//! excluded from the build-and-run example harness).

use std::path::PathBuf;
use std::process::Command;

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .canonicalize()
        .expect("canonicalize repo root")
}

fn temp_dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("snc_export_{}_{}", std::process::id(), name));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("create temp dir");
    dir
}

#[test]
fn export_c_library_called_from_c() {
    let root = repo_root();
    let dir = temp_dir("ct_select");

    // Copy the demonstrator library + C driver into the sandbox.
    let lib_src = dir.join("ct_select.sentinel");
    std::fs::copy(root.join("examples/export/ct_select.sentinel"), &lib_src)
        .expect("copy ct_select.sentinel");
    let driver_c = dir.join("driver.c");
    std::fs::copy(root.join("examples/export/driver.c"), &driver_c).expect("copy driver.c");

    // `snc build --lib ct_select.sentinel -o libsentinelct.a --emit-header sentinelct.h`
    let lib_a = dir.join("libsentinelct.a");
    let header = dir.join("sentinelct.h");
    let build = Command::new(env!("CARGO_BIN_EXE_snc"))
        .arg("build")
        .arg("--lib")
        .arg(&lib_src)
        .arg("-o")
        .arg(&lib_a)
        .arg("--emit-header")
        .arg(&header)
        .output()
        .expect("run snc build --lib");
    assert!(
        build.status.success(),
        "snc build --lib failed; stderr:\n{}",
        String::from_utf8_lossy(&build.stderr)
    );
    assert!(lib_a.exists(), "the static library was not produced");
    assert!(header.exists(), "the C header was not produced");

    // The header declares the exports with their C value-ABI types.
    let h = std::fs::read_to_string(&header).expect("read header");
    assert!(
        h.contains("int64_t ct_choose(int64_t, int64_t, int64_t);"),
        "header missing ct_choose prototype:\n{h}"
    );
    assert!(
        h.contains("int64_t sentinel_add(int64_t, int64_t);"),
        "header missing sentinel_add prototype:\n{h}"
    );
    // ADR 0059 Phase 1b: a `&[u8]` param expands to a C `(const uint8_t*,
    // int64_t)` pair in the generated prototype.
    assert!(
        h.contains("int64_t ct_byte_eq(const uint8_t*, int64_t, const uint8_t*, int64_t, int64_t);"),
        "header missing the buffer-ABI ct_byte_eq prototype:\n{h}"
    );

    // Compile the C driver against the generated header + library, run it.
    let driver_bin = dir.join("driver");
    let cc = Command::new("cc")
        .arg(&driver_c)
        .arg(&lib_a)
        .arg("-I")
        .arg(&dir)
        .arg("-o")
        .arg(&driver_bin)
        .output()
        .expect("run cc on the C driver");
    assert!(
        cc.status.success(),
        "cc failed to build the C driver against the Sentinel library; stderr:\n{}",
        String::from_utf8_lossy(&cc.stderr)
    );

    let run = Command::new(&driver_bin).output().expect("run the C driver");
    let code = run.status.code().expect("driver exited normally");
    assert_eq!(
        code, 42,
        "C driver calling the Sentinel exports exited {code}, expected 42;\nstdout:\n{}",
        String::from_utf8_lossy(&run.stdout)
    );
}

/// ADR 0059 Phase 1b (A7): the owned-`[u8]` RETURN ABI. A `[u8]`-returning
/// export hands C a heap buffer via the out-param pair `(uint8_t** out_data,
/// int64_t* out_len)`, freed with `sentinel_free_bytes`. The demonstrator
/// exports a verified-constant-time SHA-256 (the headline — crypto callable from
/// C) and a variable-length `repeat_byte`; the C driver checks the digest
/// against the NIST "abc" vector, frees each buffer, and exits 42.
#[test]
fn export_owned_bytes_return_from_c() {
    let root = repo_root();
    let dir = temp_dir("digest");

    let lib_src = dir.join("digest_lib.sentinel");
    std::fs::copy(root.join("examples/export/digest_lib.sentinel"), &lib_src)
        .expect("copy digest_lib.sentinel");
    let driver_c = dir.join("digest_driver.c");
    std::fs::copy(root.join("examples/export/digest_driver.c"), &driver_c)
        .expect("copy digest_driver.c");

    let lib_a = dir.join("libsentineldigest.a");
    let header = dir.join("sentineldigest.h");
    let build = Command::new(env!("CARGO_BIN_EXE_snc"))
        .arg("build")
        .arg("--lib")
        .arg(&lib_src)
        .arg("-o")
        .arg(&lib_a)
        .arg("--emit-header")
        .arg(&header)
        .output()
        .expect("run snc build --lib");
    assert!(
        build.status.success(),
        "snc build --lib failed; stderr:\n{}",
        String::from_utf8_lossy(&build.stderr)
    );
    assert!(lib_a.exists(), "the static library was not produced");
    assert!(header.exists(), "the C header was not produced");

    // The header renders a `[u8]` return as `void name(<inputs>, uint8_t**,
    // int64_t*)` and declares the free function once.
    let h = std::fs::read_to_string(&header).expect("read header");
    assert!(
        h.contains("void sha256_oneshot(const uint8_t*, int64_t, uint8_t**, int64_t*);"),
        "header missing the owned-return sha256_oneshot prototype:\n{h}"
    );
    assert!(
        h.contains("void repeat_byte(int64_t, int64_t, uint8_t**, int64_t*);"),
        "header missing the variable-length repeat_byte prototype:\n{h}"
    );
    assert!(
        h.contains("void sentinel_free_bytes(uint8_t* data);"),
        "header missing the sentinel_free_bytes declaration:\n{h}"
    );

    // Compile + run the C driver against the generated header + library.
    let driver_bin = dir.join("digest_driver");
    let cc = Command::new("cc")
        .arg(&driver_c)
        .arg(&lib_a)
        .arg("-I")
        .arg(&dir)
        .arg("-o")
        .arg(&driver_bin)
        .output()
        .expect("run cc on the C driver");
    assert!(
        cc.status.success(),
        "cc failed to build the C driver against the Sentinel library; stderr:\n{}",
        String::from_utf8_lossy(&cc.stderr)
    );

    let run = Command::new(&driver_bin).output().expect("run the C driver");
    let code = run.status.code().expect("driver exited normally");
    assert_eq!(
        code, 42,
        "C driver calling the owned-return exports exited {code}, expected 42;\nstdout:\n{}",
        String::from_utf8_lossy(&run.stdout)
    );
}

/// `snc build --lib` requires at least one `export "C"` and rejects `main`-less
/// libraries that export nothing (there is nothing to put in the archive).
#[test]
fn lib_without_exports_is_rejected() {
    let dir = temp_dir("no_exports");
    let src = dir.join("empty.sentinel");
    std::fs::write(&src, "fn helper(x: i64) -> i64 { x + 1 }\n").expect("write source");
    let out = Command::new(env!("CARGO_BIN_EXE_snc"))
        .arg("build")
        .arg("--lib")
        .arg(&src)
        .arg("-o")
        .arg(dir.join("out.a"))
        .output()
        .expect("run snc build --lib");
    assert!(!out.status.success(), "a library with no exports should be rejected");
    assert!(
        String::from_utf8_lossy(&out.stderr).contains("no `export"),
        "expected a no-exports diagnostic; got:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
}

/// The export path is unused by the build-and-run example harness; this asserts
/// the demonstrator source files exist (so a rename can't silently orphan them).
#[test]
fn export_demonstrator_files_present() {
    let root = repo_root();
    assert!(root.join("examples/export/ct_select.sentinel").exists());
    assert!(root.join("examples/export/driver.c").exists());
    assert!(root.join("examples/export/digest_lib.sentinel").exists());
    assert!(root.join("examples/export/digest_driver.c").exists());
}
