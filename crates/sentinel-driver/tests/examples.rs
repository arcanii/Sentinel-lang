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
//! entry that does `use std::security::sha256::...` / `use Sentinel::ct::...`
//! needs `std/` / `Sentinel/` to sit next to it. The repo keeps the first-party
//! libraries under a top-level `sentinel_library/` tree (`std/` the batteries +
//! `Sentinel/` the core base, ADR 0064), and this harness reconstructs a
//! buildable project in a temp dir: it copies the whole `sentinel_library/` tree
//! next to the example entry (flattened to the temp root). The repo `.sentinel`
//! files stay the source of truth; the temp copy is only the build sandbox (and
//! keeps the repo free of `.o` / `.o.fp` / linked binaries).

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
/// all. Used to drop the whole `sentinel_library/` tree next to an example entry.
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

/// Assemble a buildable temp project for one example: copy the repo's
/// `sentinel_library/` trees (`std/` + `Sentinel/`, ADR 0064) to `<temp>/`, and
/// the example entry to `<temp>/<stem>.sentinel` (flattened to the root so
/// `use std::...` / `use Sentinel::...` resolves). Returns the temp entry.
fn assemble(rel_entry: &str, test_name: &str) -> PathBuf {
    let root = repo_root();
    let dir = temp_project(test_name);
    copy_dir_recursive(&root.join("sentinel_library"), &dir);
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
    // The `f64` float type (ADR 0058): the quadratic formula + 2-D geometry over
    // std::math::float (float literals, `+ - * /`, unary `-`, ordered `==`, the
    // `sqrt` intrinsic, int<->float `as` casts) — a PUBLIC-only numeric module
    // crossing the boundary (no `secret f64`). 42 = every result matched.
    ("examples/math/quadratic.sentinel", 42),
    // Float transcendentals (ADR 0058 f64 + ADR 0057 FFI): std::math::float's
    // sin/cos/exp/log/pow/floor/ceil wrappers forward to libm through the
    // `extern "C"` FFI. 42 = the special-point values + the sin^2+cos^2=1 /
    // exp(ln x)=x identities (epsilon-checked) all held.
    ("examples/math/transcendental.sentinel", 42),
    // The `extern "C"` FFI (ADR 0057): call real POSIX libc functions
    // (getpid/getppid/getuid/getgid) through the safe std::sys::posix
    // wrappers — the i64 value-ABI, resolved against libc by `cc` at link.
    // 42 = every process-identity invariant held.
    ("examples/sys/process_ids.sentinel", 42),
    // The buffer FFI (ADR 0057 Phase 1b): the `ptr` opaque type + `ptr_of` /
    // `ptr_of_mut` over pointer-taking libc calls (`getentropy`, `strlen`)
    // through std::sys::ffi. 42 = strlen("hello")==5 AND two getentropy draws
    // differ (the OS wrote randomness).
    ("examples/sys/ffi_buffers.sentinel", 42),
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
    // An SSH-2 "exec" request over a real socket (RFC 4254 §6.5): the client opens a
    // session channel and asks the server to run a command; the server replies
    // CHANNEL_SUCCESS, streams the command output as CHANNEL_DATA, sends a "exit-status"
    // CHANNEL_REQUEST, and closes — what `ssh user@host some-command` does. Each message
    // is a sealed chacha20-poly1305 record under a per-direction seqnr. 42 = the exec was
    // accepted, the output was the expected transform of the command, the exit status was
    // 0, and the channel closed cleanly.
    ("examples/net/ssh_exec.sentinel", 42),
    // The text/string library (std::text::str): ASCII case folding, trimming, substring
    // search, slicing, concat/repeat/replace, lexicographic compare, decimal parse/format
    // (with a parse∘format round-trip property), index-based splitting (split_count /
    // split_nth, since Vec<[u8]> is unavailable), and padding — ~45 assertions over `[u8]`.
    // 42 = every string operation behaved as specified.
    ("examples/text/str_demo.sentinel", 42),
    // The string-keyed hash map (std::collections::map): insert / update / lookup /
    // default-on-absence / presence / count-by-key, plus a RESIZE — 20 keys inserted past
    // the initial 8 buckets force a rehash and all stay retrievable. Parallel-array +
    // index-chaining storage (the only shape the D.3 Vec MVP allows) with a public FNV-1a
    // hash; keys built via std::text::str. 42 = every map operation behaved as specified.
    ("examples/collections/map_demo.sentinel", 42),
    // The JSON library (std::data::json): a parse∘serialize round-trip over numbers,
    // strings, booleans, null, and nested arrays + objects, plus whitespace insensitivity,
    // string escapes, fractional-drop (no float type), and a parse-then-match extraction.
    // The value is a cons-list recursive enum (no Vec<enum>); serialize consumes it. 42 =
    // every document round-tripped and parsed as specified.
    ("examples/data/json_demo.sentinel", 42),
    // Composition-by-delegation, the hard cases (ADR 0021 D6): (1) MULTIPLE LEVELS —
    // `Logged` delegates Meter to `Buffered` which delegates Meter to `Counter`, so one
    // `tick` forwards three deep to the real impl; (2) the MULTIPLE-INHERITANCE / diamond
    // problem — the same method `tick` from two sources, which Sentinel rejects as an
    // ambiguous bare call and resolves with NAMED impls (`Add`/`Scale`) + qualified
    // `Name::tick(..)` calls (composition + names, no diamond/MRO). 42 = the multi-level
    // tick (20) + the two named ticks (4 + 18).
    ("examples/lang/delegation.sentinel", 42),
    // Explicit early `return` (ADR 0065): `return expr` exits the enclosing
    // function early as a divergent branch tail (a guard `if b == 0 { return -1 }
    // else { a / b }`), including a heap binding live across the return (dropped
    // exactly once on the early-exit and fall-through paths). 42 = every check
    // held (and `main` itself early-returns 42 on success).
    ("examples/lang/early_return.sentinel", 42),
    // `return` inside a `match` arm (ADR 0065), the diverging-arm-type-join case: a
    // `match` yields a `bool`, but its `Halt` arm early-`return`s the function's `i64`
    // directly. A diverging arm yields no value, so it does not constrain the arm-type
    // join (pre-0065 this was a "match arms have incompatible types" reject). 42 = held.
    ("examples/lang/early_return_match.sentinel", 42),
    // `return` crossing a `handle` (ADR 0065 D6): a handler arm early-returns instead of
    // resuming `k` (abandoning the in-flight kont — the runtime `sentinel_kont_free` frees
    // it on the return path), and a handle body early-returns before any `perform`. Both
    // resume + early-return paths run leak-free with no double-free. 42 = held. snc-only.
    ("examples/lang/early_return_handle.sentinel", 42),
    // A handle body that performs through CONTROL FLOW (ADR 0065 stage 3a): a `perform`
    // in an `if`/`else` branch is normalized to a continuation the handler dispatches,
    // rather than being hoisted into a separate effecting fn. snc-only (the text oracle +
    // selfhost don't yet mirror the normalization). 42 = held.
    ("examples/lang/handle_control_flow.sentinel", 42),
    // ADR 0066 M1.1: generic `Task<T>` — an `i32`-arg, `f64`-result spawn.
    // The wrapper loads the arg with its real type + encodes the f64 result
    // into the Task's i64 slot; `.await` decodes it. snc-only (f64 is
    // snc-side; the selfhost mirror lands in M1.1 Phase B). 42 = decoded.
    ("examples/lang/task_generic.sentinel", 42),
    // ADR 0066 M1.2: channels — channel_new / send / channel_close / recv (the
    // ?i64 some-or-null) over a Channel<i64> queue. snc-only (the selfhost
    // mirror lands in M1.2 Phase B). 10 + 32 = 42.
    ("examples/lang/channel.sentinel", 42),
    // ADR 0066 M1.2b-cont: GENERIC word-scalar channels — Channel<u8> / Channel<bool>
    // (the element encoded/decoded through the i64 slot; the i64 case stays byte-
    // identical to M1.2). ⚠ This row used to say "snc-only (the scg mirror is deferred,
    // like M2.3b)" and BOTH halves were false: this program is deferred in no list, and
    // M2.3b's mirror landed as register D34. A Channel<u8> queue summing 30+12 + a
    // Channel<bool> round-trip → 42.
    ("examples/lang/channel_generic.sentinel", 42),
    // ADR 0066 M1.3: the worker-pool pattern — two long-lived workers spawned
    // with their `Channel<i64>` endpoints (the M1.2b channel-typed param), a
    // shared work queue (work-stealing via the mutex'd mpsc receiver) + a
    // results fan-in channel. Squares 1/4/5 across the pool. 1+16+25 = 42.
    ("examples/lang/worker_pool.sentinel", 42),
    // ADR 0071 M1.4-0 (the D5 gate) part 1: a shared counter as an owning task
    // (the ADR 0066 D2 actor pattern) — three workers fan increments into one
    // channel, the owner folds them into its private total. The shape channels
    // handle WELL (commutative accumulation). 3*14 = 42. See
    // docs/decisions/0071-m14-0-analysis.md.
    ("examples/lang/shared_counter_via_channel.sentinel", 42),
    // ADR 0071 M1.4-0 (the D5 gate) part 2: a shared sequence/ID allocator via
    // request-reply channels — the shape channels handle POORLY (replies are
    // unaddressed; no channel-of-channels, no select to correlate them). Runs
    // only because the checked result is order-independent. 7 requests *+6 = 42.
    ("examples/lang/shared_sequence_via_channel.sentinel", 42),
    // ADR 0066 M1.2c (D6a): channel-of-channels — the ADDRESSED reply, i.e. the
    // program the line above says cannot be written. Each claimer sends its OWN
    // reply-channel handle through a `Channel<Channel<i64>>`, so a reply reaches
    // exactly one claimer and correlation is structural. Needs no `select`: a
    // worker waits on one channel, its own. ⚠ This row used to say "snc-only (the scg
    // mirror is deferred, like process_channel_typed / channel_generic)" — stale three
    // times over: this program is deferred in no list, and BOTH cited exemplars are
    // mirrored (channel_generic already, process_channel_typed as register D34).
    // 6+12+18+6 = 42.
    ("examples/lang/addressed_reply.sentinel", 42),
    // ADR 0066 M2.3b: GENERIC word-scalar elements for the typed process channel —
    // `process_send`/`process_recv` over `u8` / `i32` / `bool` (not just `i64`),
    // encoded into / decoded from the 8-byte i64 frame (the M1.1 spawn encode). The
    // spawn is guarded for cross-platform determinism; the generic send/recv IR +
    // typing still lower. ⚠ This row used to end "snc-only (the scg mirror is deferred
    // … the differential corpus stays at the i64 `c66_process_channel`)". Both halves
    // are false since register D34: `scg` mirrors fids 29/30, this program is
    // byte-identical to the oracle at the types, MIR and codegen stages, and its
    // deferred entry is deleted from all THREE lists — so it is now a live differential
    // pin for the non-i64 elements, not a demonstrator. 42.
    ("examples/lang/process_channel_typed.sentinel", 42),
    // ADR 0066 M2.4a / ADR 0069: SealedChannel<secret i64> — the AEAD-encrypted
    // secret-cross-process path. An in-process `seal` -> `open` round-trip
    // (runtime-verified crypto: the secret re-emerges secret, authenticated) plus a
    // GUARDED pipe path (`sealed_channel` / `sealed_send` / `sealed_recv` over the
    // bridge builtins, compiled but not run for cross-platform determinism). Reuses
    // the verified-CT ssh record cipher (no new primitive). snc-only (the scg mirror
    // is deferred, like process_channel_typed / task_generic). 42 = the opened value.
    ("examples/lang/sealed_channel.sentinel", 42),
    // ADR 0066 M2.4b / ADR 0069 D3/D4: the authenticated x25519 KEX that establishes
    // a SealedChannel's per-direction session keys, + the counter-nonce sealed stream
    // — verified in-process (both halves derive matching keyc/keyd; the initiator
    // authenticates the host via ed25519 sig + a pinned host key; a 3-message stream
    // with distinct counter nonces re-emerges secret). Reuses the verified-CT ssh KEX
    // (no new primitive). The real-pipe transport is deferred (needs a self-stdin
    // builtin). snc-only. 5+15+20 + keyc_match + authed = 42.
    ("examples/lang/sealed_session.sentinel", 42),
    // ADR 0066 M2.4c / ADR 0069 D5: variable-length SECRET messages over a
    // SealedChannel with fixed-width padding — seal_bytes/open_bytes seal an arbitrary
    // [secret u8] message; the wire ciphertext length is constant for a given maxlen
    // (no length leak). Verified in-process: a variable-length message re-emerges
    // secret + authenticated (3 bytes summing to 42), and two different-length
    // messages frame to the same length. snc-only. 42.
    ("examples/lang/sealed_bytes.sentinel", 42),
    // ADR 0037 / BACKLOG §10.9: the worked MULTI-FILE example — a program using
    // a `pub class` (`Rect`) defined in another file, the library module
    // `std::math::rect`. The class crosses the module / separate-compilation
    // boundary. 6 × 7 = 42.
    ("examples/modules/rect_demo.sentinel", 42),
    // ADR 0070: non-capturing first-class function values — `Fn<i64,i64>`. A
    // bare top-level fn name used as a value (`let op = square;`), invoked
    // indirectly via the `apply(f, x)` builtin. Unblocks parameterizing a
    // worker body by its operation instead of hardcoding it (the worker_pool
    // motivation). Two different fns through the same `apply_to`: square(6) +
    // double(3) = 36 + 6 = 42. snc-only — this one IS genuinely still deferred, in all
    // three lists (see ADR 0070 D7). ⚠ Its old "like task_generic / f64 /
    // process_channel_typed" citation is dead: `task_generic` was deleted and
    // `process_channel_typed` was mirrored (register D34), so `f64` is the only live
    // companion left.
    ("examples/lang/fn_value.sentinel", 42),
    // ADR 0070 (generalized, M-cont): GENERIC word-scalar Fn<T,R> — not just
    // Fn<i64,i64>. Fn<u8,u8>/Fn<f64,f64>/Fn<bool,bool>, the signature id
    // computed arithmetically (no interner threading). inc_u8(29)=30 +
    // double_f64(5.0) as i64=10 + flip(false)-contributes-2 = 42. snc-only.
    ("examples/lang/fn_value_generic.sentinel", 42),
    // ADR 0016 A1 (register item D5): a PHANTOM type parameter — a generic struct
    // parameter no field uses, so it tags a type without adding layout. It is the
    // one position where a type reaches the name mangler with NO value of that type
    // anywhere. 20 markers -> 20 distinct, LLVM-legal, byte-identical type
    // declarations across all three back ends. 6+7+8+9+12 = 42.
    //
    // It sweeps the tag forms a phantom parameter can reach — bare scalar, nominal
    // name, one-element wrapper, handle-over-element, two-element signature, nested
    // composition — but NOT every form the scheme has: the index-bearing `T<n>` is
    // filtered out of Pass 0 by construction, and the zero-element atoms `process` /
    // `sealedchannel` are not writable in type position at all. Those two are covered
    // instead by `tests/pass/c16_mono_key_handle_tags.sentinel`, through the mono-key
    // route.
    ("examples/lang/phantom_type_param.sentinel", 42),
    // ADR 0016 D7 / register D15: a generic fn calling a generic fn, plus a generic
    // struct whose field is another generic instance reached only as a phantom type
    // argument. Both create instances the TYPE CHECKER never interned, which both Rust
    // back ends then indexed out of bounds and CRASHED on. Registered in
    // `DEFERRED_PROGRAMS` for `scg`'s own two gaps (no transitive mono worklist, no
    // struct-field closure), not for D15. 20 + 18 + 3 + 1 = 42.
    ("examples/lang/generic_calls_generic.sentinel", 42),
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
fn math_quadratic_f64() {
    check_example("examples/math/quadratic.sentinel", "quadratic", 42);
}

#[test]
fn sys_process_ids_ffi() {
    check_example("examples/sys/process_ids.sentinel", "process_ids", 42);
}

#[test]
fn sys_ffi_buffers_ptr() {
    check_example("examples/sys/ffi_buffers.sentinel", "ffi_buffers", 42);
}

#[test]
fn math_transcendental_libm() {
    check_example("examples/math/transcendental.sentinel", "transcendental", 42);
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

#[test]
fn ssh_exec_command_request() {
    check_example("examples/net/ssh_exec.sentinel", "ssh_exec", 42);
}

#[test]
fn text_str_library() {
    check_example("examples/text/str_demo.sentinel", "str_demo", 42);
}

#[test]
fn collections_hash_map() {
    check_example("examples/collections/map_demo.sentinel", "map_demo", 42);
}

#[test]
fn data_json_roundtrip() {
    check_example("examples/data/json_demo.sentinel", "json_demo", 42);
}

#[test]
fn lang_delegation_levels_and_multiple_inheritance() {
    check_example("examples/lang/delegation.sentinel", "delegation", 42);
}

#[test]
fn lang_early_return() {
    check_example("examples/lang/early_return.sentinel", "early_return", 42);
}

#[test]
fn lang_early_return_match() {
    check_example("examples/lang/early_return_match.sentinel", "early_return_match", 42);
}

#[test]
fn lang_early_return_handle() {
    check_example("examples/lang/early_return_handle.sentinel", "early_return_handle", 42);
}

#[test]
fn lang_handle_control_flow() {
    check_example("examples/lang/handle_control_flow.sentinel", "handle_control_flow", 42);
}

#[test]
fn lang_task_generic() {
    check_example("examples/lang/task_generic.sentinel", "task_generic", 42);
}

#[test]
fn lang_channel() {
    check_example("examples/lang/channel.sentinel", "channel", 42);
}

#[test]
fn lang_worker_pool() {
    check_example("examples/lang/worker_pool.sentinel", "worker_pool", 42);
}

#[test]
fn lang_shared_counter_via_channel() {
    // ADR 0071 M1.4-0 (the D5 gate) part 1: a shared counter as an owning task —
    // the shape channels handle WELL (commutative fan-in accumulation). 3*14 = 42.
    check_example(
        "examples/lang/shared_counter_via_channel.sentinel",
        "shared_counter_via_channel",
        42,
    );
}

#[test]
fn lang_addressed_reply() {
    // ADR 0066 M1.2c (D6a): channel-of-channels. The correlated request/reply
    // that `shared_sequence_via_channel` (below) documents as impossible —
    // each claimer passes its own reply channel through the request, so it
    // waits on exactly one channel and gets exactly its own answer.
    check_example(
        "examples/lang/addressed_reply.sentinel",
        "addressed_reply",
        42,
    );
}

#[test]
fn lang_shared_sequence_via_channel() {
    // ADR 0071 M1.4-0 (the D5 gate) part 2: a shared sequence allocator via
    // request-reply channels — the shape channels handle POORLY (replies
    // uncorrelated; no channel-of-channels, no select). Runs only because the
    // checked result is order-independent. 7 requests *+6 = 42.
    check_example(
        "examples/lang/shared_sequence_via_channel.sentinel",
        "shared_sequence_via_channel",
        42,
    );
}

#[test]
fn lang_channel_generic() {
    // ADR 0066 M1.2b-cont: generic word-scalar channels (Channel<u8> / Channel<bool>);
    // the element is encoded/decoded through the i64 slot. 30+12 + a bool round-trip = 42.
    check_example("examples/lang/channel_generic.sentinel", "channel_generic", 42);
}

#[test]
fn lang_sealed_channel() {
    // ADR 0066 M2.4a / ADR 0069: seal a `secret i64`, frame the public ciphertext,
    // open it back to `secret i64` (the AEAD-encrypted secret-cross-process path).
    // The in-process round-trip is runtime-verified (exit 42 = the opened+declassified
    // value); the pipe path is compiled but guarded. Built --separate and merged.
    check_example("examples/lang/sealed_channel.sentinel", "sealed_channel", 42);
}

#[test]
fn lang_sealed_session() {
    // ADR 0066 M2.4b / ADR 0069 D3/D4: the authenticated x25519 KEX + per-direction
    // keys + a counter-nonce sealed stream, verified in-process (matching keyc/keyd,
    // host authenticated via ed25519 sig + pinned host key, a 3-message stream re-
    // emerges secret). 5+15+20 + keyc_match + authed = 42. Built --separate and merged.
    check_example("examples/lang/sealed_session.sentinel", "sealed_session", 42);
}

#[test]
fn lang_sealed_bytes() {
    // ADR 0066 M2.4c / ADR 0069 D5: variable-length SECRET messages with fixed-width
    // padding — seal_bytes/open_bytes over a [secret u8] message; a 3-byte message
    // (sum 42) re-emerges secret + authenticated, and two different-length messages
    // frame to the same length (no length leak). Built --separate and merged.
    check_example("examples/lang/sealed_bytes.sentinel", "sealed_bytes", 42);
}

#[test]
fn modules_rect_demo() {
    // A program using a `pub class` defined in another file (the library module
    // std::math::rect) — built --separate (the class crosses a unit boundary)
    // and merged.
    check_example("examples/modules/rect_demo.sentinel", "rect_demo", 42);
}

#[test]
fn lang_fn_value() {
    // ADR 0070: non-capturing first-class function values (Fn<i64,i64>) — a bare
    // fn name used as a value, invoked indirectly via `apply(f, x)`. Two distinct
    // fns (square, double) through the same `apply_to` parameter: 36 + 6 = 42.
    check_example("examples/lang/fn_value.sentinel", "fn_value", 42);
}

#[test]
fn lang_fn_value_generic() {
    // ADR 0070 (generalized): Fn<T,R> over any word-scalar pair, not just
    // Fn<i64,i64> — Fn<u8,u8>/Fn<f64,f64>/Fn<bool,bool> via the same `apply`
    // builtin, context-typed from each Fn value's own signature.
    check_example("examples/lang/fn_value_generic.sentinel", "fn_value_generic", 42);
}

#[test]
fn lang_phantom_type_param() {
    // ADR 0016 A1: phantom type parameters over 20 distinct marker types. The
    // interesting assertion is NOT the exit code — it is that the same program is
    // byte-identical between `snc llvm` and `scg`, which `selfhost_codegen`'s
    // real-program sweep checks (this file lives under `examples/`, so it is in that
    // sweep's corpus). Before A1 the oracle emitted names `llvm-as` could not parse
    // and `scg` collapsed eight markers onto one name; only inkwell — which this test
    // exercises — was ever right, which is exactly why an exit-code test alone could
    // not have caught it.
    //
    // ⚠ Be precise about the `llvm-as` half, because the natural reading is wrong.
    // That sweep's `llvm_rejects` gate fires only when scg's IR is rejected AND the
    // ORACLE's is clean — it exists to catch scg diverging into invalid IR, not to
    // audit the oracle. In the exact pre-A1 state this fixture memorialises, BOTH
    // sides were rejected (scg: "redefinition of type"; oracle: "expected '=' after
    // name"), so the gate would have been SILENT. What would have caught it is the
    // byte comparison, and only because the two were wrong in different ways — had
    // they been wrong identically, nothing in CI would have noticed. (The gate is
    // also skipped entirely when `LLVM_SYS_180_PREFIX` is unset.)
    check_example("examples/lang/phantom_type_param.sentinel", "phantom_type_param", 42);
}

#[test]
fn lang_generic_calls_generic() {
    // ADR 0016 D7 / register D15 (defect alpha). The exit code proves inkwell — which
    // PANICKED on this shape in `TypedProgram::generic_instance` before the fix —
    // compiles and runs it. The byte comparison against `scg` is deliberately NOT
    // asserted here: the program is in `DEFERRED_PROGRAMS` because `scg` has no
    // transitive monomorphisation worklist and takes no generic-struct field closure,
    // both of which are its own defects rather than this one. 20 + 18 + 3 + 1 = 42.
    check_example("examples/lang/generic_calls_generic.sentinel", "generic_calls_generic", 42);
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
                // ADR 0059: `examples/export/` holds C-ABI LIBRARIES (no `main`)
                // + their C drivers — exercised by `tests/export.rs`, not the
                // build-and-run example harness. Skip it here.
                if path.file_name().and_then(|n| n.to_str()) == Some("export") {
                    continue;
                }
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
