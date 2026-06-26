//! Examples-as-tests: the `std/` core libraries + `examples/` programs that
//! `use` them, each wired as a pass-test.
//!
//! This is the examples-as-tests + core-library track. Each example is a real,
//! idiomatic Sentinel program that doubles as a feature test of the library it
//! exercises. Every example is built **twice** from the SAME source:
//!
//!   1. `snc build <entry> --separate` — the per-unit separate-compilation back
//!      end (ADR 0037 (a)): each module → its own object, linked via
//!      module-qualified `abi-v1` symbols. This dogfoods the module system +
//!      `--separate` on real multi-module programs (the best test of those
//!      features), including the incremental `.o.fp` cache.
//!   2. `snc build <entry>` — the default merge path.
//!
//! Both must succeed, and both must run to the SAME exit code as each other and
//! as the table's expected value (a free differential between the two back ends,
//! and the "exit-code-is-the-answer" convention). A successful build also means
//! the constant-time check (`sentinel::mir::secret_leak`) passed — so an example
//! that carries `secret` values (e.g. `secure_compare`) compiling at all is a
//! proof that the constant-time discipline held across the call graph.
//!
//! ## How a multi-file program is assembled here
//!
//! Module discovery roots at the entry file's parent directory, and
//! `use a::b::Item` maps to `<root>/a/b.sentinel` (no parent traversal). So an
//! entry that does `use std::security::ct::...` needs `std/` to sit next to it.
//! The repo keeps a clean top-level split (`std/` and `examples/`, each
//! subdivided by functional category), and this harness reconstructs a buildable
//! project in a temp dir: it copies the whole `std/` tree next to the example
//! entry (flattened to the temp root). The repo `.sentinel` files stay the
//! source of truth; the temp copy is only the build sandbox (and keeps the repo
//! free of `.o` / `.o.fp` / linked binaries).

use std::path::{Path, PathBuf};
use std::process::Command;

/// The repository root (two levels up from this crate's manifest dir,
/// `crates/sentinel-driver`).
fn repo_root() -> PathBuf {
    let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    manifest
        .join("..")
        .join("..")
        .canonicalize()
        .expect("canonicalize repo root")
}

/// A fresh temp project directory, unique per test + process so the parallel
/// runner never collides. Best-effort cleared first.
fn temp_project(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("snc_examples_{}_{}", std::process::id(), name));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("create temp project dir");
    dir
}

/// Recursively copy `src` into `dst` (creating `dst`), `.sentinel` files and
/// all. Used to drop the whole `std/` tree next to an example entry.
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

/// Assemble a buildable temp project for one example: copy the repo's `std/`
/// tree to `<temp>/std`, and the example entry to `<temp>/<stem>.sentinel`
/// (flattened to the root so `use std::...` resolves). Returns the temp entry.
fn assemble(rel_entry: &str, test_name: &str) -> PathBuf {
    let root = repo_root();
    let dir = temp_project(test_name);
    copy_dir_recursive(&root.join("std"), &dir.join("std"));
    let src_entry = root.join(rel_entry);
    let stem = src_entry
        .file_name()
        .expect("entry file name")
        .to_owned();
    let temp_entry = dir.join(stem);
    std::fs::copy(&src_entry, &temp_entry).expect("copy example entry");
    temp_entry
}

/// Run a compiled binary and return its exit code (the program's tail value).
fn run_exit(exe: &Path) -> i32 {
    let run = Command::new(exe).output().expect("run compiled binary");
    run.status.code().expect("process exited normally")
}

/// Build `entry` with `snc build <entry> --separate -o <exe>` (the per-unit
/// back end), run it, return its exit code.
fn build_and_run_separate(entry: &Path) -> i32 {
    let exe = entry.with_extension("sep");
    let out = Command::new(env!("CARGO_BIN_EXE_snc"))
        .arg("build")
        .arg(entry)
        .arg("--separate")
        .arg("-o")
        .arg(&exe)
        .output()
        .expect("run snc --separate");
    assert!(
        out.status.success(),
        "expected a successful `--separate` build of {}; stderr:\n{}",
        entry.display(),
        String::from_utf8_lossy(&out.stderr),
    );
    run_exit(&exe)
}

/// Build `entry` with `snc build <entry> -o <exe>` (the merge path), run it,
/// return its exit code.
fn build_and_run_merge(entry: &Path) -> i32 {
    let exe = entry.with_extension("mrg");
    let out = Command::new(env!("CARGO_BIN_EXE_snc"))
        .arg("build")
        .arg(entry)
        .arg("-o")
        .arg(&exe)
        .output()
        .expect("run snc build");
    assert!(
        out.status.success(),
        "expected a successful merge build of {}; stderr:\n{}",
        entry.display(),
        String::from_utf8_lossy(&out.stderr),
    );
    run_exit(&exe)
}

/// Build one example both ways from the same source, assert both back ends agree
/// with each other and with `expected` exit.
fn check_example(rel_entry: &str, test_name: &str, expected: i32) {
    let entry = assemble(rel_entry, test_name);
    let sep = build_and_run_separate(&entry);
    let mrg = build_and_run_merge(&entry);
    assert_eq!(
        sep, mrg,
        "{rel_entry}: --separate exit {sep} != merge exit {mrg} (back-end differential)"
    );
    assert_eq!(
        sep, expected,
        "{rel_entry}: exit {sep} != expected {expected}"
    );
}

/// The example corpus: `(repo-relative entry, expected exit code)`. Every
/// `.sentinel` file under `examples/` must appear here (enforced by
/// `every_example_is_registered` below) so no example is silently untested.
const EXAMPLES: &[(&str, i32)] = &[
    // Constant-time fixed-width secure compare over `secret` scalars
    // (std::security::ct). 42 = equal-compared-equal AND tampered-compared-unequal.
    ("examples/security/secure_compare.sentinel", 42),
    // Integer clamp/abs helpers (std::math::num) — a non-`secret` module
    // crossing the boundary. 42 = every helper returned its expected result.
    ("examples/math/clamp.sentinel", 42),
    // In-place element updates via `&mut a[i]` element borrows (ADR 0054): clamps
    // array + Vec elements in place by passing them by mutable reference to an
    // imported helper (std::math::num::clamp_assign). 42 = every element landed at
    // its clamped value.
    ("examples/math/inplace.sentinel", 42),
    // The flagship: variable-length constant-time memcmp over `[secret u8]`
    // secret byte buffers (std::security::ct::ct_memcmp, ADR 0047). 42 =
    // equal-compared-equal AND tampered-compared-unequal.
    ("examples/security/ct_memcmp.sentinel", 42),
    // Bit rotation via std::bits::bits (ADR 0048 shift operators). 42 = rotl/
    // rotr and the masked rotate-by-0 / rotate-by-width identities all hold.
    ("examples/bits/rotate.sentinel", 42),
    // One SipHash ARX round over four SECRET 64-bit words (the recognizable
    // branch-free primitive) — add/rotate-by-public-constant/xor, constant-time
    // via std::security::ct::ct_rotl64. 42 = reproduced the reference output.
    ("examples/security/siphash_round.sentinel", 42),
    // Ordinary (public) `[u8]` scanning + building via std::bytes::bytes
    // (find/count/contains/starts_with/eq/repeat over `&[u8]` borrows). 42 =
    // every utility returned its expected result.
    ("examples/bytes/scan.sentinel", 42),
    // A true 32-bit ChaCha quarter-round over SECRET i32 words (ADR 0049 `as`
    // cast + ADR 0048 shifts) — add/rotate/xor, constant-time via ct_rotl32.
    // 42 = reproduced the RFC 8439 §2.1.1 test vector.
    ("examples/security/chacha_qr.sentinel", 42),
    // A full ChaCha20 block: 20 rounds + feed-forward over a SECRET 16-word
    // `[secret i32]` state permuted IN PLACE via index assignment (ADR 0050),
    // constant-time via ct_rotl32. 42 = reproduced the RFC 8439 §2.3.2 block.
    ("examples/security/chacha20_block.sentinel", 42),
    // SipHash-2-4 keyed MAC over a public message + 128-bit secret key
    // (std::security::siphash) — branch-free ARX over `secret i64` state. 42 =
    // reproduced the canonical 0xa129ca6149be45e5 vector AND a tampered message
    // produced a different tag.
    ("examples/security/siphash24.sentinel", 42),
    // The ChaCha20 stream cipher (std::security::chacha20::chacha20_xor) — the
    // shipped block keystream XORed into a `[secret u8]` message in place. 42 =
    // encrypting "Ladies and Gentl" reproduced the RFC 8439 §2.4.2 ciphertext AND
    // decrypting recovered the plaintext.
    ("examples/security/chacha20_stream.sentinel", 42),
    // Poly1305 one-time MAC (std::security::poly1305) — a polynomial in the secret
    // clamped key over GF(2^130-5), as radix-2^26 `secret i64` limbs with a
    // constant-time freeze; only the tag is declassified. 42 = reproduced the
    // RFC 8439 §2.5.2 tag.
    ("examples/security/poly1305.sentinel", 42),
    // In-place insertion sort + binary search over public `[i64]`
    // (std::algorithms::seq) — the sort permutes via index assignment (ADR 0050),
    // the search over a `&[i64]` borrow. 42 = sorted ascending + found a present
    // value + returned -1 for an absent one.
    ("examples/algorithms/sort_search.sentinel", 42),
    // ChaCha20-Poly1305 AEAD (std::security::aead) — composes the shipped ChaCha20
    // + Poly1305 (refactored to a secret `&[secret u8]` key) per RFC 8439 §2.8:
    // OTK gen + encrypt + mac-data + tag. 42 = both the ciphertext and the tag
    // matched the reference for the §2.8.2 key/nonce.
    ("examples/security/chacha20poly1305.sentinel", 42),
    // The FULL RFC 8439 §2.8.2 AEAD vector (114-byte plaintext + 12-byte AAD): the
    // secret plaintext is built up by `push` into a `Vec<secret u8>` (ADR 0052) and
    // bridged to `[secret u8]` via `vec_to_array` (ADR 0053). 42 = the full 114-byte
    // ciphertext AND the 16-byte tag matched the published vector byte-for-byte.
    ("examples/security/chacha20poly1305_full.sentinel", 42),
    // Constant-time SHA-256 over a SECRET message (std::security::sha256, FIPS
    // 180-4) — the compression is branch-free over `secret i32` words; the schedule
    // is a Vec<secret i32> (ADR 0052) and the padded message a Vec<secret u8> ->
    // [secret u8] (ADR 0053). 42 = SHA-256("abc"), "", and 100*'a' (multi-block) all
    // matched their NIST digest.
    ("examples/security/sha256.sentinel", 42),
    // HMAC-SHA256 over a SECRET key (std::security::hmac, RFC 2104 / RFC 4231,
    // composing sha256) — two branch-free SHA-256 passes over the secret key blocks.
    // 42 = RFC 4231 TC1/TC2/TC6 (TC6's 131-byte key exercises the hash-first path)
    // all matched their published tag.
    ("examples/security/hmac.sentinel", 42),
    // Variable-length constant-time compare over GROWABLE secret byte buffers
    // (std::security::ct::ct_vec_eq, ADR 0052 `Vec<secret u8>`) — each buffer is
    // built up by `push` at run time, then reduced branch-free. 42 = two
    // identically-built buffers compared equal AND a one-byte-shifted buffer
    // compared unequal.
    ("examples/security/ct_vec_eq.sentinel", 42),
    // Constant-time AES-128 block encryption (std::security::aes, FIPS-197) over a
    // SECRET key + plaintext. The textbook table-lookup S-box (a secret index into
    // a 256-byte table) is rejected by the constant-time check, so the library
    // computes the S-box arithmetically (GF(2^8) field inversion + affine),
    // branch-free; the build succeeding is the constant-time proof. 42 = the
    // FIPS-197 §C.1 vector AND the AES-128(0,0) vector matched byte-for-byte.
    ("examples/security/aes128.sentinel", 42),
    // Constant-time X25519 (std::security::x25519, RFC 7748) — the ECDH primitive
    // over a SECRET scalar. A textbook Montgomery ladder branches on the secret
    // scalar bits (a key-leaking timing side channel); that is rejected by the
    // constant-time check, so the ladder's conditional swap is branch-free and the
    // build succeeding is the constant-time proof. 42 = the RFC 7748 §5.2 vector AND
    // the §6.1 Diffie-Hellman shared secret matched (and Alice and Bob agreed).
    ("examples/security/x25519.sentinel", 42),
    // Constant-time SHA-512 over a SECRET message (std::security::sha512, FIPS
    // 180-4) — the 64-bit-word twin of SHA-256, branch-free over `secret i64` words
    // (no width casts). 42 = SHA-512("abc"), "", and 200*'a' (multi-block) all
    // matched their NIST digest.
    ("examples/security/sha512.sentinel", 42),
    // Constant-time AES-128-GCM AEAD (std::security::aes_gcm, NIST SP 800-38D) over a
    // SECRET key + plaintext. Composes the AES block with GHASH, whose GF(2^128)
    // multiply is the new branch-free field primitive (the textbook version branches
    // on bits of the secret auth key H). 42 = the McGrew/OpenSSL "TC4" vector (60-byte
    // plaintext + 20-byte AAD, partial blocks) AND the same input without AAD matched
    // ciphertext + tag byte-for-byte.
    ("examples/security/aes_gcm.sentinel", 42),
    // Constant-time SHA-3 over a SECRET message (std::security::sha3, FIPS 202) — the
    // Keccak sponge (a different construction from SHA-2's Merkle-Damgård): the
    // Keccak-f[1600] permutation is branch-free bitwise ops over 25 `secret i64`
    // lanes. 42 = SHA3-256("abc"/""/200*'a'/135*'a' padding-edge) and SHA3-512("abc"/
    // 100*'a') all matched their NIST digest (both rates, single + multi-block).
    ("examples/security/sha3.sentinel", 42),
    // Constant-time Ed25519 SIGNING + VERIFICATION (std::security::ed25519, RFC 8032)
    // over a SECRET seed. Composes the shared field layer (std::security::fe25519) +
    // SHA-512; the Edwards scalar multiplication runs over the secret scalar via a
    // branch-free ladder (a textbook double-and-add branches on the key bits, which
    // the CT check rejects), and verify decompresses a point via a field square root.
    // 42 = three RFC 8032 / pyca-cryptography vectors (incl. the empty message) matched
    // both the public key and the signature, AND each verified its valid signature and
    // rejected a one-bit-tampered one.
    ("examples/security/ed25519.sentinel", 42),
    // HKDF-SHA256 key derivation (std::security::hkdf, RFC 5869) — extract-then-
    // expand built purely from std::security::hmac (→ sha256). Every salt / IKM /
    // info byte is `[secret u8]`, so the whole derivation is constant-time; only
    // the output length and the public block counter drive control flow. 42 = the
    // three RFC 5869 Appendix A vectors (incl. A.3's empty salt + info) reproduced
    // both the PRK and the OKM byte-for-byte.
    ("examples/security/hkdf.sentinel", 42),
    // SP 800-185 SHA-3 derived functions (std::security::sha3) — cSHAKE128,
    // TupleHash128/256, and ParallelHash128 over the published NIST sample vectors.
    // Every message / tuple element is `[secret u8]`, so the data flows only through
    // the branch-free Keccak permutation; only the public framing (tuple lengths,
    // block size, output length, length encodings) drives control flow. 42 = the
    // cSHAKE / TupleHash / ParallelHash sample outputs reproduced byte-for-byte.
    ("examples/security/sp800185.sentinel", 42),
    // Constant-time X448 ECDH (std::security::x448, RFC 7748) over the larger
    // Curve448 field (std::security::fe448, GF(2^448-2^224-1) as 28 radix-2^16
    // limbs). The secret scalar drives the Montgomery ladder through branch-free
    // sel448 swaps over a fixed 448 iterations. 42 = the RFC 7748 §5.2 vector and a
    // full ECDH exchange (both sides agree on the shared secret) matched byte-for-byte.
    ("examples/security/x448.sentinel", 42),
    // Constant-time Ed448 signatures (std::security::ed448, RFC 8032) over the
    // edwards448 curve (the shared std::security::fe448 field) + SHAKE256. Signing
    // runs the secret scalar through a branch-free Montgomery ladder and a branch-free
    // Barrett reduction mod the group order. 42 = the RFC 8032 §7.4 vector (public key
    // + signature) reproduced, a non-empty-message vector reproduced, both verified,
    // and a tampered signature rejected.
    ("examples/security/ed448.sentinel", 42),
    // X25519 at radix 2^51 (std::security::x25519_64 over std::security::fe25519_64),
    // the demonstrator for the `u128` type (ADR 0055 — the 128-bit numeric gap): the
    // field multiply runs in `secret u128` because a 51-bit limb product is 102 bits.
    // 42 = the RFC 7748 §5.2 vector reproduced AND the radix-2^51 and radix-2^16
    // implementations agreed on the same shared secret (an internal consistency proof).
    ("examples/security/x25519_64.sentinel", 42),
    // A complete SSH-2 transport key exchange (std::net::ssh — curve25519-sha256 +
    // ssh-ed25519, RFC 4253 / RFC 8731) run loopback in memory: the constant-time
    // crypto core of an sshd, composing X25519 / Ed25519 / SHA-256 through the SSH wire
    // protocol. 42 = the ECDH agreed, the exchange hash matched an independent model,
    // the host-key signature verified, and all six derived session keys matched.
    ("examples/net/ssh_handshake.sentinel", 42),
    // The SSH encrypted channel (std::net::ssh_cipher — chacha20-poly1305@openssh.com):
    // the binary packet AEAD that protects every SSH packet after NEWKEYS, composing
    // ChaCha20 + Poly1305. 42 = a sealed packet matched an independent model byte-for-
    // byte, its tag verified, the payload round-tripped, and a tampered record was
    // rejected.
    ("examples/net/ssh_channel.sentinel", 42),
    // A complete SSH-2 transport session end-to-end (std::net::ssh + ssh_cipher): the
    // curve25519-sha256 KEX, the ssh-ed25519 host-key auth, the §7.2 key derivation
    // (the 64-byte chacha20-poly1305 key), and an encrypted application packet — all
    // stitched in memory as a real sshd does after NEWKEYS. 42 = the KEX agreed and
    // matched H, the signature verified, the derived key matched, and the encrypted
    // packet round-tripped.
    ("examples/net/ssh_session.sentinel", 42),
    // A real localhost TCP echo over the ADR 0056 socket builtins: bind 127.0.0.1:0,
    // `spawn` a concurrent server task (an OS thread, via structured concurrency), and
    // from the main task connect to it, send bytes, and read the echo back — actual
    // kernel sockets, not in-memory buffers. The substrate a networked sshd runs on.
    // 42 = the byte round-trip matched and the server task joined cleanly.
    ("examples/net/tcp_echo.sentinel", 42),
    // The SSH-2 transport handshake run over a REAL TCP socket (std::net::ssh +
    // ssh_cipher + ADR 0056 sockets): the client and server are separate concurrent
    // tasks that exchange the curve25519-sha256 KEX values over loopback — each holds
    // only its own ephemeral secret and derives the shared secret K independently (a
    // real distributed Diffie-Hellman). The wire-public values (ephemeral pubkeys, host
    // key, signature, ciphertext record) are declassified to send and widened back to
    // the secret domain on receipt; K and the session key never cross the socket. 42 =
    // the two peers agreed on the exchange hash H, the host-key signature verified, the
    // derived session key matched, and the encrypted application packet round-tripped.
    ("examples/net/ssh_over_tcp.sentinel", 42),
    // A bidirectional, multi-packet SSH encrypted channel over a real TCP socket (the
    // data phase after NEWKEYS): both peers derive the two directional keys ('C' c->s,
    // 'D' s->c) from the shared session state, then a request/response ping-pong streams
    // N chacha20-poly1305@openssh.com records each way, each under its per-direction
    // sequence number (which drives a fresh nonce per packet). Every record is
    // tag-verified before it is decrypted (verify-then-decode). 42 = all N packets
    // round-tripped — every tag verified under the right key+seqnr and every response
    // was the expected transform of its request (so the seqnr stream stayed in lockstep).
    ("examples/net/ssh_channel_stream.sentinel", 42),
    // A complete SSH-2 session over one real connection — the handshake AND the data
    // phase — using the reusable std::net::ssh_handshake halves: the client and server
    // each call one library function (ssh_client_handshake / ssh_server_handshake) that
    // runs its side of the curve25519-sha256 KEX over the socket and returns the two
    // directional keys (in an SshSessionKeys struct), then they exchange N encrypted
    // packets each way. 42 = the handshake authenticated, the derived key matched the
    // known-good vector, and every data packet round-tripped.
    ("examples/net/ssh_full_session.sentinel", 42),
    // SSH-2 "publickey" user authentication (RFC 4252 §7) over a real socket, on top of
    // the transport handshake: the client signs a request bound to the session id (the
    // first exchange hash H) with its Ed25519 user key, and the server verifies the
    // signature AND checks the offered key is authorized before replying SUCCESS. Binding
    // to H means a captured signature can't be replayed on another session. 42 = the
    // transport authenticated and the publickey userauth succeeded.
    ("examples/net/ssh_pubkey_auth.sentinel", 42),
    // The SSH-2 connection layer (RFC 4254) over a real socket: a "session" channel with
    // windowed flow control, riding inside the encrypted transport. After the handshake,
    // CHANNEL_OPEN/CONFIRMATION negotiate a 16-byte window, then a 32-byte payload crosses
    // in two CHANNEL_DATA chunks — the client must wait for a CHANNEL_WINDOW_ADJUST after
    // the first 16 bytes before sending the rest — and the channel CLOSEs. Each message is
    // a sealed chacha20-poly1305 record under a per-direction seqnr. 42 = the channel
    // opened, both windowed chunks reassembled intact, and it closed cleanly.
    ("examples/net/ssh_channel_window.sentinel", 42),
];

#[test]
fn secure_compare_constant_time() {
    check_example("examples/security/secure_compare.sentinel", "secure_compare", 42);
}

#[test]
fn math_clamp() {
    check_example("examples/math/clamp.sentinel", "clamp", 42);
}

#[test]
fn math_inplace_element_borrow() {
    check_example("examples/math/inplace.sentinel", "inplace", 42);
}

#[test]
fn ct_memcmp_secret_bytes() {
    check_example("examples/security/ct_memcmp.sentinel", "ct_memcmp", 42);
}

#[test]
fn bits_rotate() {
    check_example("examples/bits/rotate.sentinel", "rotate", 42);
}

#[test]
fn siphash_round_constant_time() {
    check_example("examples/security/siphash_round.sentinel", "siphash_round", 42);
}

#[test]
fn bytes_scan() {
    check_example("examples/bytes/scan.sentinel", "scan", 42);
}

#[test]
fn chacha_quarter_round() {
    check_example("examples/security/chacha_qr.sentinel", "chacha_qr", 42);
}

#[test]
fn chacha20_block_constant_time() {
    check_example("examples/security/chacha20_block.sentinel", "chacha20_block", 42);
}

#[test]
fn siphash24_keyed_mac() {
    check_example("examples/security/siphash24.sentinel", "siphash24", 42);
}

#[test]
fn chacha20_stream_cipher() {
    check_example("examples/security/chacha20_stream.sentinel", "chacha20_stream", 42);
}

#[test]
fn poly1305_one_time_mac() {
    check_example("examples/security/poly1305.sentinel", "poly1305", 42);
}

#[test]
fn algorithms_sort_search() {
    check_example("examples/algorithms/sort_search.sentinel", "sort_search", 42);
}

#[test]
fn chacha20poly1305_aead() {
    check_example("examples/security/chacha20poly1305.sentinel", "chacha20poly1305", 42);
}

#[test]
fn chacha20poly1305_aead_full_vector() {
    check_example("examples/security/chacha20poly1305_full.sentinel", "chacha20poly1305_full", 42);
}

#[test]
fn ct_vec_eq_secret_vec() {
    check_example("examples/security/ct_vec_eq.sentinel", "ct_vec_eq", 42);
}

#[test]
fn sha256_digest() {
    check_example("examples/security/sha256.sentinel", "sha256", 42);
}

#[test]
fn hmac_sha256_rfc4231() {
    check_example("examples/security/hmac.sentinel", "hmac", 42);
}

#[test]
fn aes128_block_constant_time() {
    check_example("examples/security/aes128.sentinel", "aes128", 42);
}

#[test]
fn x25519_scalarmult_constant_time() {
    check_example("examples/security/x25519.sentinel", "x25519", 42);
}

#[test]
fn sha512_digest() {
    check_example("examples/security/sha512.sentinel", "sha512", 42);
}

#[test]
fn aes_gcm_aead_constant_time() {
    check_example("examples/security/aes_gcm.sentinel", "aes_gcm", 42);
}

#[test]
fn sha3_keccak_digest() {
    check_example("examples/security/sha3.sentinel", "sha3", 42);
}

#[test]
fn ed25519_signing_constant_time() {
    check_example("examples/security/ed25519.sentinel", "ed25519", 42);
}

#[test]
fn hkdf_sha256_rfc5869() {
    check_example("examples/security/hkdf.sentinel", "hkdf", 42);
}

#[test]
fn sp800185_derived_functions() {
    check_example("examples/security/sp800185.sentinel", "sp800185", 42);
}

#[test]
fn x448_scalarmult_constant_time() {
    check_example("examples/security/x448.sentinel", "x448", 42);
}

#[test]
fn ed448_signing_constant_time() {
    check_example("examples/security/ed448.sentinel", "ed448", 42);
}

#[test]
fn x25519_64_radix_2_51_u128() {
    check_example("examples/security/x25519_64.sentinel", "x25519_64", 42);
}

#[test]
fn ssh_handshake_curve25519_sha256() {
    check_example("examples/net/ssh_handshake.sentinel", "ssh_handshake", 42);
}

#[test]
fn ssh_channel_chacha20poly1305_openssh() {
    check_example("examples/net/ssh_channel.sentinel", "ssh_channel", 42);
}

#[test]
fn ssh_session_end_to_end() {
    check_example("examples/net/ssh_session.sentinel", "ssh_session", 42);
}

#[test]
fn tcp_echo_loopback() {
    check_example("examples/net/tcp_echo.sentinel", "tcp_echo", 42);
}

#[test]
fn ssh_over_tcp_distributed_handshake() {
    check_example("examples/net/ssh_over_tcp.sentinel", "ssh_over_tcp", 42);
}

#[test]
fn ssh_channel_stream_multipacket() {
    check_example("examples/net/ssh_channel_stream.sentinel", "ssh_channel_stream", 42);
}

#[test]
fn ssh_full_session_handshake_plus_data() {
    check_example("examples/net/ssh_full_session.sentinel", "ssh_full_session", 42);
}

#[test]
fn ssh_pubkey_auth_userauth() {
    check_example("examples/net/ssh_pubkey_auth.sentinel", "ssh_pubkey_auth", 42);
}

#[test]
fn ssh_channel_window_flow_control() {
    check_example("examples/net/ssh_channel_window.sentinel", "ssh_channel_window", 42);
}

/// Coverage guard: every `.sentinel` program under `examples/` must be
/// registered in `EXAMPLES`, so adding an example file without wiring it as a
/// test fails CI rather than going silently unexercised.
#[test]
fn every_example_is_registered() {
    fn collect(dir: &Path, root: &Path, out: &mut Vec<String>) {
        for entry in std::fs::read_dir(dir).expect("read_dir examples") {
            let path = entry.expect("dir entry").path();
            if path.is_dir() {
                collect(&path, root, out);
            } else if path.extension().and_then(|e| e.to_str()) == Some("sentinel") {
                let rel = path
                    .strip_prefix(root)
                    .expect("strip repo root")
                    .to_string_lossy()
                    .replace('\\', "/");
                out.push(rel);
            }
        }
    }
    let root = repo_root();
    let mut found = Vec::new();
    collect(&root.join("examples"), &root, &mut found);
    found.sort();

    let mut registered: Vec<String> = EXAMPLES.iter().map(|(p, _)| (*p).to_owned()).collect();
    registered.sort();

    assert_eq!(
        found, registered,
        "every example under examples/ must be registered in EXAMPLES (and vice versa)"
    );
}
