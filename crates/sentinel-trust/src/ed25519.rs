//! Ed25519 **verification** (RFC 8032 §5.1.7) — the build-time trust gate's
//! in-process verifier (ADR 0061 D4).
//!
//! This is the **Rust twin** of `std::security::ed25519`: the same TweetNaCl
//! subset (the `gf = [i64; 16]` radix-2^16 field, `unpackneg` decompression, the
//! `[S]B − [h]A` group equation, the `S < L` canonicality check), validated
//! against the **same** RFC 8032 §7.1 test vectors the Sentinel side reproduces.
//! Two implementations, one standard — the dual-impl/oracle discipline self-host
//! already uses (Rust `snc` vs Sentinel `scg`).
//!
//! It is **verify-only and operates entirely on public data** (a signature, a
//! public key, a message), so — unlike the Sentinel signer, which must be
//! constant-time in the secret key — this is plain Rust: the masked
//! conditional-swaps are kept only because they are the faithful transcription,
//! not for timing. (Rust's `i64 >>` is already arithmetic, so the Sentinel
//! `arith_shr` reconstruction collapses to plain `>>`.)

#![allow(clippy::needless_range_loop)] // index-parallel field arithmetic reads clearest as-is.

use crate::sha512::sha512;

/// A field element of GF(2^255-19): 16 limbs of radix 2^16 (TweetNaCl `gf`).
type Gf = [i64; 16];

const GF0: Gf = [0; 16];
const GF1: Gf = [1, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0];

// Edwards25519 constants as radix-2^16 field elements (identical limbs to
// std::security::fe25519 — the values are pinned by the RFC 8032 KATs below).
const D: Gf = [
    30883, 4953, 19914, 30187, 55467, 16705, 2637, 112, 59544, 30585, 16505, 36039, 65139, 11119,
    27886, 20995,
];
const D2: Gf = [
    61785, 9906, 39828, 60374, 45398, 33411, 5274, 224, 53552, 61171, 33010, 6542, 64743, 22239,
    55772, 9222,
];
const X: Gf = [
    54554, 36645, 11616, 51542, 42930, 38181, 51040, 26924, 56412, 64982, 57905, 49316, 21502,
    52590, 14035, 8553,
];
const Y: Gf = [
    26200, 26214, 26214, 26214, 26214, 26214, 26214, 26214, 26214, 26214, 26214, 26214, 26214,
    26214, 26214, 26214,
];
const SQRTM1: Gf = [
    41136, 18958, 6951, 50414, 58488, 44335, 6150, 12099, 55207, 15867, 153, 11085, 57099, 20417,
    9344, 11139,
];

/// The group order L = 2^252 + 27742317777372353535851937790883648493, 32
/// little-endian bytes (RFC 8032 §5.1).
const L: [i64; 32] = [
    237, 211, 245, 92, 26, 99, 18, 88, 214, 156, 247, 162, 222, 249, 222, 20, 0, 0, 0, 0, 0, 0, 0,
    0, 0, 0, 0, 0, 0, 0, 0, 16,
];

/// A curve point in extended coordinates (X, Y, Z, T): `[GF; 4]`.
type Point = [Gf; 4];

// ---- field arithmetic ----------------------------------------------------------

fn car25519(o: &mut Gf) {
    for i in 0..16 {
        let c = o[i] >> 16;
        o[i] -= c << 16;
        if i < 15 {
            o[i + 1] += c;
        } else {
            o[0] += 38 * c;
        }
    }
}

fn fadd(o: &mut Gf, a: &Gf, b: &Gf) {
    for i in 0..16 {
        o[i] = a[i] + b[i];
    }
}

fn fsub(o: &mut Gf, a: &Gf, b: &Gf) {
    for i in 0..16 {
        o[i] = a[i] - b[i];
    }
}

fn fmul(o: &mut Gf, a: &Gf, b: &Gf) {
    let mut t = [0i64; 31];
    for i in 0..16 {
        for j in 0..16 {
            t[i + j] += a[i] * b[j];
        }
    }
    for i in 0..15 {
        t[i] += 38 * t[i + 16];
    }
    o[..16].copy_from_slice(&t[..16]);
    car25519(o);
    car25519(o);
}

fn fsq(o: &mut Gf, a: &Gf) {
    fmul(o, a, a);
}

/// Conditionally swap `p` and `q` when `b == 1` (branch-free mask; faithful to
/// the constant-time signer — here `b` is public).
fn sel25519(p: &mut Gf, q: &mut Gf, b: i64) {
    let c = -b;
    for i in 0..16 {
        let t = c & (p[i] ^ q[i]);
        p[i] ^= t;
        q[i] ^= t;
    }
}

/// z^((p-5)/8) = z^(2^252-3) — the exponent point decompression's square root needs.
fn pow2523(o: &mut Gf, a: &Gf) {
    let mut c = *a;
    let mut i = 250i64;
    while i >= 0 {
        let mut t = GF0;
        fsq(&mut t, &c);
        c = t;
        if i != 1 {
            let mut t2 = GF0;
            fmul(&mut t2, &c, a);
            c = t2;
        }
        i -= 1;
    }
    *o = c;
}

/// The multiplicative inverse z^(2^255-21) = z^-1 (Fermat).
fn finv(o: &mut Gf, a: &Gf) {
    let mut c = *a;
    let mut i = 253i64;
    while i >= 0 {
        let mut t = GF0;
        fsq(&mut t, &c);
        c = t;
        if i != 2 && i != 4 {
            let mut t2 = GF0;
            fmul(&mut t2, &c, a);
            c = t2;
        }
        i -= 1;
    }
    *o = c;
}

fn unpack25519(o: &mut Gf, n: &[u8; 32]) {
    for i in 0..16 {
        o[i] = i64::from(n[2 * i]) + (i64::from(n[2 * i + 1]) << 8);
    }
    o[15] &= 32767;
}

fn pack25519(n: &Gf) -> [u8; 32] {
    let mut o = *n;
    car25519(&mut o);
    car25519(&mut o);
    car25519(&mut o);
    for _ in 0..2 {
        let mut m = GF0;
        m[0] = o[0] - 65517;
        for i in 1..15 {
            m[i] = o[i] - 65535 - ((m[i - 1] >> 16) & 1);
            m[i - 1] &= 65535;
        }
        m[15] = o[15] - 32767 - ((m[14] >> 16) & 1);
        let b = (m[15] >> 16) & 1;
        m[14] &= 65535;
        sel25519(&mut o, &mut m, 1 - b);
    }
    let mut out = [0u8; 32];
    for i in 0..16 {
        out[2 * i] = (o[i] & 255) as u8;
        out[2 * i + 1] = ((o[i] >> 8) & 255) as u8;
    }
    out
}

/// 1 iff the two field elements differ (compare canonical packings).
fn neq25519(a: &Gf, b: &Gf) -> i64 {
    let pa = pack25519(a);
    let pb = pack25519(b);
    let mut acc = 0i64;
    for i in 0..32 {
        acc |= i64::from(pa[i]) ^ i64::from(pb[i]);
    }
    ((0 - acc) >> 63) & 1
}

/// The low bit (parity) of a field element.
fn par25519(a: &Gf) -> i64 {
    i64::from(pack25519(a)[0]) & 1
}

// ---- the Edwards group law -----------------------------------------------------

/// `p += q` (unified twisted-Edwards addition, extended coordinates).
fn point_add(p: &mut Point, q: &Point) {
    let (mut a, mut b, mut c) = (GF0, GF0, GF0);
    let (mut e, mut f, mut g, mut h) = (GF0, GF0, GF0, GF0);
    let mut t = GF0;
    let mut t2 = GF0;

    fsub(&mut a, &p[1], &p[0]); // a = (Y1-X1)*(Y2-X2)
    fsub(&mut t, &q[1], &q[0]);
    fmul(&mut t2, &a, &t);
    a = t2;

    fadd(&mut b, &p[0], &p[1]); // b = (X1+Y1)*(X2+Y2)
    fadd(&mut t, &q[0], &q[1]);
    fmul(&mut t2, &b, &t);
    b = t2;

    fmul(&mut c, &p[3], &q[3]); // c = T1*T2*(2d)
    fmul(&mut t2, &c, &D2);
    c = t2;

    let mut dd = GF0; // d = 2*(Z1*Z2)
    fmul(&mut dd, &p[2], &q[2]);
    let td = dd;
    for i in 0..16 {
        dd[i] += td[i];
    }

    fsub(&mut e, &b, &a); // e=b-a; f=d-c; g=d+c; h=b+a
    fsub(&mut f, &dd, &c);
    fadd(&mut g, &dd, &c);
    fadd(&mut h, &b, &a);

    fmul(&mut p[0], &e, &f); // X3=e*f; Y3=h*g; Z3=g*f; T3=e*h
    fmul(&mut p[1], &h, &g);
    fmul(&mut p[2], &g, &f);
    fmul(&mut p[3], &e, &h);
}

fn point_cswap(p: &mut Point, q: &mut Point, b: i64) {
    for i in 0..4 {
        sel25519(&mut p[i], &mut q[i], b);
    }
}

/// `out = [scalar] * q` by the double-and-add ladder (scalar = 32 little-endian bytes).
fn scalarmult(out: &mut Point, q: &Point, scalar: &[u8; 32]) {
    *out = [GF0, GF1, GF1, GF0]; // identity (X=0,Y=1,Z=1,T=0)
    let mut w = *q;
    let mut i = 255i64;
    while i >= 0 {
        let b = (i64::from(scalar[(i / 8) as usize]) >> (i & 7)) & 1;
        point_cswap(out, &mut w, b);
        point_add(&mut w, out); // q += out
        let s = *out; // out += out
        point_add(out, &s);
        point_cswap(out, &mut w, b);
        i -= 1;
    }
}

/// `out = [scalar] * B`, B the Ed25519 base point.
fn scalarbase(out: &mut Point, scalar: &[u8; 32]) {
    let mut base: Point = [X, Y, GF1, GF0];
    let mut bt = GF0;
    fmul(&mut bt, &X, &Y); // T = Bx*By
    base[3] = bt;
    scalarmult(out, &base, scalar);
}

/// Compress a point to 32 little-endian bytes (RFC 8032 §5.1.2).
fn point_pack(p: &Point) -> [u8; 32] {
    let mut zi = GF0;
    finv(&mut zi, &p[2]);
    let mut tx = GF0;
    let mut ty = GF0;
    fmul(&mut tx, &p[0], &zi);
    fmul(&mut ty, &p[1], &zi);
    let yb = pack25519(&ty);
    let xb = pack25519(&tx);
    let mut out = yb;
    out[31] |= (xb[0] & 1) << 7; // fold x-parity into the top bit
    out
}

/// Decompress the **negation** of a point from its 32-byte encoding (TweetNaCl
/// `unpackneg`): recover x from y + the sign bit. Returns 1 if `pbytes` is a
/// valid curve point, 0 otherwise; the point (which is -P) lands in `r`.
fn point_decode_neg(r: &mut Point, pbytes: &[u8; 32]) -> i64 {
    r[2] = GF1; // Z = 1
    unpack25519(&mut r[1], pbytes); // Y (top sign bit cleared)

    let mut num = GF0;
    let mut den = GF0;
    fsq(&mut num, &r[1]); // num = y^2
    fmul(&mut den, &num, &D); // den = y^2 * d
    let mut tmp = GF0;
    fsub(&mut tmp, &num, &GF1); // num = y^2 - 1
    num = tmp;
    let mut tmp2 = GF0;
    fadd(&mut tmp2, &GF1, &den); // den = 1 + y^2*d
    den = tmp2;

    // x = num * den^3 * (num * den^7)^((p-5)/8)
    let mut d2 = GF0;
    fsq(&mut d2, &den);
    let mut d4 = GF0;
    fsq(&mut d4, &d2);
    let mut d6 = GF0;
    fmul(&mut d6, &d4, &d2);
    let mut t = GF0;
    fmul(&mut t, &d6, &num);
    let mut tt = GF0;
    fmul(&mut tt, &t, &den);
    t = tt; // num * den^7
    let mut tp = GF0;
    pow2523(&mut tp, &t);
    t = tp;
    let mut ta = GF0;
    fmul(&mut ta, &t, &num);
    t = ta;
    let mut tb = GF0;
    fmul(&mut tb, &t, &den);
    t = tb;
    let mut tc = GF0;
    fmul(&mut tc, &t, &den);
    t = tc;
    fmul(&mut r[0], &t, &den); // rx

    // if x^2*den != num, multiply x by sqrt(-1) (the other root branch).
    let mut chk = GF0;
    fsq(&mut chk, &r[0]);
    let mut chkd = GF0;
    fmul(&mut chkd, &chk, &den);
    chk = chkd;
    let m1 = neq25519(&chk, &num);
    let mut xi = GF0;
    fmul(&mut xi, &r[0], &SQRTM1);
    sel25519(&mut r[0], &mut xi, m1);

    // valid iff x^2*den == num after the correction.
    let mut chk2 = GF0;
    fsq(&mut chk2, &r[0]);
    let mut chk2d = GF0;
    fmul(&mut chk2d, &chk2, &den);
    chk2 = chk2d;
    let valid = 1 - neq25519(&chk2, &num);

    // if parity(x) == the encoded sign bit, negate x (→ -P).
    let sign = (i64::from(pbytes[31]) >> 7) & 1;
    let par = par25519(&r[0]);
    let negmask = 1 - (par ^ sign);
    let mut negx = GF0;
    fsub(&mut negx, &GF0, &r[0]);
    sel25519(&mut r[0], &mut negx, negmask);

    let mut rt = GF0;
    fmul(&mut rt, &r[0], &r[1]); // T = X*Y
    r[3] = rt;
    valid
}

// ---- scalar reduction mod L ----------------------------------------------------

/// Reduce a 64-limb little-endian value `x` modulo L → 32 little-endian bytes
/// (TweetNaCl `modL`). All inputs here originate as bytes (verify), so the i64
/// intermediates never approach overflow.
fn modl(x: &mut [i64; 64]) -> [u8; 32] {
    let mut i = 63usize;
    while i >= 32 {
        let mut carry = 0i64;
        let lo = i - 32;
        for j in lo..(i - 12) {
            x[j] += carry - 16 * x[i] * L[j - lo];
            carry = (x[j] + 128) >> 8;
            x[j] -= carry << 8;
        }
        x[i - 12] += carry;
        x[i] = 0;
        i -= 1;
    }
    let mut carry = 0i64;
    for j in 0..32 {
        x[j] += carry - ((x[31] >> 4) * L[j]);
        carry = x[j] >> 8;
        x[j] &= 255;
    }
    for j in 0..32 {
        x[j] -= carry * L[j];
    }
    let mut out = [0u8; 32];
    for i in 0..32 {
        x[i + 1] += x[i] >> 8;
        out[i] = (x[i] & 255) as u8;
    }
    out
}

/// SHA-512 digest (64 bytes) → a scalar mod L (32 bytes).
fn reduce(h: &[u8; 64]) -> [u8; 32] {
    let mut x = [0i64; 64];
    for i in 0..64 {
        x[i] = i64::from(h[i]);
    }
    modl(&mut x)
}

/// 1 iff `s` (32 little-endian bytes) is `< L` — the RFC 8032 §5.1.7 step-1
/// canonicality check (else `(R, S+L)` would be a second accepted signature).
fn s_lt_l(s: &[u8]) -> i64 {
    let mut slt = 0i64;
    let mut still_eq = 1i64;
    let mut bi = 31i64;
    while bi >= 0 {
        let sb = i64::from(s[bi as usize]);
        let lb = L[bi as usize];
        let lt = ((sb - lb) >> 63) & 1; // 1 iff s[bi] < L[bi]
        let gt = ((lb - sb) >> 63) & 1; // 1 iff s[bi] > L[bi]
        slt |= still_eq & lt;
        still_eq &= 1 - (lt | gt);
        bi -= 1;
    }
    slt
}

// ---- public API ----------------------------------------------------------------

/// Verify an Ed25519 signature: `true` iff `sig` (R‖S, 64 bytes) is a valid
/// signature of `msg` under public key `pk` (32 bytes) — RFC 8032 §5.1.7.
/// Decompresses −A, computes `[S]B − [h]A` with `h = reduce(SHA-512(R‖A‖msg))`,
/// accepts iff it re-encodes to R **and** the decompression was valid **and**
/// `S < L`.
#[must_use]
pub fn ed25519_verify(pk: &[u8; 32], msg: &[u8], sig: &[u8; 64]) -> bool {
    let mut a: Point = [GF0; 4];
    let dvalid = point_decode_neg(&mut a, pk);

    // h = reduce(SHA-512(R || A || msg)), R = sig[0..32], A = pk.
    let mut buf = Vec::with_capacity(64 + msg.len());
    buf.extend_from_slice(&sig[..32]);
    buf.extend_from_slice(pk);
    buf.extend_from_slice(msg);
    let h = reduce(&sha512(&buf));

    // p = [h](-A) ; q = [S]B ; p = p + q = [S]B - [h]A.
    let mut p: Point = [GF0; 4];
    scalarmult(&mut p, &a, &h);
    let mut s_scalar = [0u8; 32];
    s_scalar.copy_from_slice(&sig[32..64]);
    let mut q: Point = [GF0; 4];
    scalarbase(&mut q, &s_scalar);
    point_add(&mut p, &q);

    // accept iff pack(p) == R, decompression valid, and S < L.
    let packed = point_pack(&p);
    let mut acc = 0i64;
    for i in 0..32 {
        acc |= i64::from(packed[i]) ^ i64::from(sig[i]);
    }
    let eq = 1 - (((0 - acc) >> 63) & 1);
    (eq & dvalid & s_lt_l(&sig[32..64])) == 1
}

/// A **test-only** Ed25519 signer (and pubkey derivation), reusing the field /
/// group machinery above. It is NOT part of the shipped surface — production
/// signing is pure Sentinel (`std::security::ed25519`, dogfooded by `snc sign`);
/// this exists only to generate signature vectors for the format/verify tests
/// self-containedly. It is itself validated against the RFC 8032 §7.1 vectors
/// (seed → pk, and `sign(seed, msg)` == the published signature), so a vector it
/// produces is a genuine Ed25519 signature the shipped verifier accepts.
#[cfg(test)]
pub(crate) mod test_sign {
    use super::{modl, point_pack, reduce, scalarbase, sha512, Point, GF0};

    fn clamped_scalar(seed: &[u8; 32]) -> ([u8; 32], [u8; 32]) {
        let h = sha512(seed);
        let mut a = [0u8; 32];
        a.copy_from_slice(&h[..32]);
        a[0] &= 248;
        a[31] &= 127;
        a[31] |= 64;
        let mut prefix = [0u8; 32];
        prefix.copy_from_slice(&h[32..64]);
        (a, prefix)
    }

    pub(crate) fn ed25519_pubkey(seed: &[u8; 32]) -> [u8; 32] {
        let (a, _) = clamped_scalar(seed);
        let mut p: Point = [GF0; 4];
        scalarbase(&mut p, &a);
        point_pack(&p)
    }

    pub(crate) fn ed25519_sign(seed: &[u8; 32], m: &[u8]) -> [u8; 64] {
        let (a, prefix) = clamped_scalar(seed);
        let pk = ed25519_pubkey(seed);

        // r = reduce(SHA-512(prefix || m)); R = [r]B.
        let mut rbuf = Vec::with_capacity(32 + m.len());
        rbuf.extend_from_slice(&prefix);
        rbuf.extend_from_slice(m);
        let r = reduce(&sha512(&rbuf));
        let mut rp: Point = [GF0; 4];
        scalarbase(&mut rp, &r);
        let rpack = point_pack(&rp);

        // k = reduce(SHA-512(R || A || m)); S = (r + k*a) mod L.
        let mut kbuf = Vec::with_capacity(64 + m.len());
        kbuf.extend_from_slice(&rpack);
        kbuf.extend_from_slice(&pk);
        kbuf.extend_from_slice(m);
        let k = reduce(&sha512(&kbuf));

        let mut x = [0i64; 64];
        for i in 0..32 {
            x[i] = i64::from(r[i]);
        }
        for i in 0..32 {
            for j in 0..32 {
                x[i + j] += i64::from(k[i]) * i64::from(a[j]);
            }
        }
        let s = modl(&mut x);

        let mut sig = [0u8; 64];
        sig[..32].copy_from_slice(&rpack);
        sig[32..].copy_from_slice(&s);
        sig
    }

    #[test]
    fn matches_rfc8032_test2() {
        let mut seed = [0u8; 32];
        let bytes = b"\x4c\xcd\x08\x9b\x28\xff\x96\xda\x9d\xb6\xc3\x46\xec\x11\x4e\x0f\x5b\x8a\x31\x9f\x35\xab\xa6\x24\xda\x8c\xf6\xed\x4f\xb8\xa6\xfb";
        seed.copy_from_slice(bytes);
        let pk = ed25519_pubkey(&seed);
        assert_eq!(
            pk,
            *b"\x3d\x40\x17\xc3\xe8\x43\x89\x5a\x92\xb7\x0a\xa7\x4d\x1b\x7e\xbc\x9c\x98\x2c\xcf\x2e\xc4\x96\x8c\xc0\xcd\x55\xf1\x2a\xf4\x66\x0c"
        );
        let sig = ed25519_sign(&seed, b"\x72");
        let expected = b"\x92\xa0\x09\xa9\xf0\xd4\xca\xb8\x72\x0e\x82\x0b\x5f\x64\x25\x40\xa2\xb2\x7b\x54\x16\x50\x3f\x8f\xb3\x76\x22\x23\xeb\xdb\x69\xda\x08\x5a\xc1\xe4\x3e\x15\x99\x6e\x45\x8f\x36\x13\xd0\xf1\x1d\x8c\x38\x7b\x2e\xae\xb4\x30\x2a\xee\xb0\x0d\x29\x16\x12\xbb\x0c\x00";
        assert_eq!(&sig[..], &expected[..]);
    }
}

#[cfg(test)]
mod tests {
    use super::ed25519_verify;

    fn unhex(s: &str) -> Vec<u8> {
        (0..s.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(&s[i..i + 2], 16).unwrap())
            .collect()
    }

    fn arr32(s: &str) -> [u8; 32] {
        unhex(s).try_into().unwrap()
    }
    fn arr64(s: &str) -> [u8; 64] {
        unhex(s).try_into().unwrap()
    }

    // RFC 8032 §7.1 test vectors (public key, message, signature).
    #[test]
    fn rfc8032_test1_empty_message() {
        let pk = arr32("d75a980182b10ab7d54bfed3c964073a0ee172f3daa62325af021a68f707511a");
        let sig = arr64(
            "e5564300c360ac729086e2cc806e828a84877f1eb8e5d974d873e065224901555fb8821590a33bacc61e39701cf9b46bd25bf5f0595bbe24655141438e7a100b",
        );
        assert!(ed25519_verify(&pk, b"", &sig));
    }

    #[test]
    fn rfc8032_test2() {
        let pk = arr32("3d4017c3e843895a92b70aa74d1b7ebc9c982ccf2ec4968cc0cd55f12af4660c");
        let msg = unhex("72");
        let sig = arr64(
            "92a009a9f0d4cab8720e820b5f642540a2b27b5416503f8fb3762223ebdb69da085ac1e43e15996e458f3613d0f11d8c387b2eaeb4302aeeb00d291612bb0c00",
        );
        assert!(ed25519_verify(&pk, &msg, &sig));
    }

    #[test]
    fn rfc8032_test3() {
        let pk = arr32("fc51cd8e6218a1a38da47ed00230f0580816ed13ba3303ac5deb911548908025");
        let msg = unhex("af82");
        let sig = arr64(
            "6291d657deec24024827e69c3abe01a30ce548a284743a445e3680d7db5ac3ac18ff9b538d16f290ae67f760984dc6594a7c15e9716ed28dc027beceea1ec40a",
        );
        assert!(ed25519_verify(&pk, &msg, &sig));
    }

    #[test]
    fn tampered_message_rejected() {
        let pk = arr32("fc51cd8e6218a1a38da47ed00230f0580816ed13ba3303ac5deb911548908025");
        let sig = arr64(
            "6291d657deec24024827e69c3abe01a30ce548a284743a445e3680d7db5ac3ac18ff9b538d16f290ae67f760984dc6594a7c15e9716ed28dc027beceea1ec40a",
        );
        assert!(!ed25519_verify(&pk, b"\xaf\x83", &sig)); // af82 → af83
    }

    #[test]
    fn tampered_signature_rejected() {
        let pk = arr32("fc51cd8e6218a1a38da47ed00230f0580816ed13ba3303ac5deb911548908025");
        let msg = unhex("af82");
        // flip the last byte of S.
        let sig = arr64(
            "6291d657deec24024827e69c3abe01a30ce548a284743a445e3680d7db5ac3ac18ff9b538d16f290ae67f760984dc6594a7c15e9716ed28dc027beceea1ec40b",
        );
        assert!(!ed25519_verify(&pk, &msg, &sig));
    }

    #[test]
    fn wrong_key_rejected() {
        // test2's signature under test3's key.
        let pk = arr32("fc51cd8e6218a1a38da47ed00230f0580816ed13ba3303ac5deb911548908025");
        let msg = unhex("72");
        let sig = arr64(
            "92a009a9f0d4cab8720e820b5f642540a2b27b5416503f8fb3762223ebdb69da085ac1e43e15996e458f3613d0f11d8c387b2eaeb4302aeeb00d291612bb0c00",
        );
        assert!(!ed25519_verify(&pk, &msg, &sig));
    }
}
