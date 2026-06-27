//! The signed-object format + the verification entry point (ADR 0061 D2/D3).
//!
//! The bytes that get signed are a **rigid canonical payload** — `domain || algo
//! || pubkey || grants || SHA-512(body)` — chosen so the signer and verifier agree
//! with *zero* canonicalization TCB (ADR 0061 D2). The body is bound by its hash
//! *inside* the signed payload; the verifier always recomputes `SHA-512(body)`
//! from the artifact on disk and never trusts a hash carried alongside it
//! ("verify the exact bytes you compile"). Because the Sentinel signer only ever
//! signs these opaque payload bytes, the entire format lives here in Rust — there
//! is no second format implementation to keep in sync.
//!
//! The **carrier** (the detached `.sig` file, or an in-file `//` header block)
//! is a small human-readable text holding the public key, the capability grants,
//! and the signature. The same parser reads both forms: it strips an optional
//! leading `//` from each line, so a detached carrier and a `//`-prefixed in-file
//! block parse identically.

use crate::ed25519::ed25519_verify;
use crate::sha512::sha512;

/// Domain-separation prefix (ADR 0061 D4): binds a signature to this scheme so it
/// can never be replayed as a signature in another protocol.
pub const DOMAIN: &[u8] = b"sentinel-sig-v1\0";

/// Algorithm tag for Ed25519 (the `algorithm` agility field, D4).
pub const ALGO_ED25519: u8 = 1;

/// The canonical signed bytes: `domain || algo || pubkey || grants || body_hash`.
///
/// `grants` is encoded order-independently: sorted, then a `u16` big-endian count
/// followed by each name as a `u16` big-endian length + its UTF-8 bytes. This is
/// the exact byte string the signer signs and the verifier reconstructs.
#[must_use]
pub fn canonical_payload(pubkey: &[u8; 32], grants: &[String], body_hash: &[u8; 64]) -> Vec<u8> {
    let mut sorted: Vec<&String> = grants.iter().collect();
    sorted.sort();
    sorted.dedup();

    let mut p = Vec::with_capacity(DOMAIN.len() + 1 + 32 + 2 + 64);
    p.extend_from_slice(DOMAIN);
    p.push(ALGO_ED25519);
    p.extend_from_slice(pubkey);
    p.extend_from_slice(&(sorted.len() as u16).to_be_bytes());
    for name in sorted {
        p.extend_from_slice(&(name.len() as u16).to_be_bytes());
        p.extend_from_slice(name.as_bytes());
    }
    p.extend_from_slice(body_hash);
    p
}

/// A parsed signature carrier (ADR 0061 D3): the public key that signed, the
/// capability grants the signer claims, and the Ed25519 signature.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SignedObject {
    pub pubkey: [u8; 32],
    pub grants: Vec<String>,
    pub signature: [u8; 64],
}

/// The result of a successful verification: who signed, and the capabilities the
/// signature authorizes (checked against the dependency's effect row by the gate,
/// D6).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Verified {
    pub pubkey: [u8; 32],
    pub grants: Vec<String>,
}

/// Why a signature did not verify.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VerifyError {
    /// The carrier text could not be parsed (missing/!hex field, bad magic, …).
    Malformed(String),
    /// The carrier parsed, but the signature does not verify over the body.
    BadSignature,
}

impl std::fmt::Display for VerifyError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            VerifyError::Malformed(why) => write!(f, "malformed signature carrier: {why}"),
            VerifyError::BadSignature => write!(f, "signature does not verify over the source bytes"),
        }
    }
}

impl std::error::Error for VerifyError {}

/// Serialize a [`SignedObject`] to the canonical carrier text (the detached `.sig`
/// form). The in-file block form is the same text with each line prefixed `// `.
#[must_use]
pub fn serialize_carrier(obj: &SignedObject) -> String {
    let mut s = String::new();
    s.push_str("sentinel-signature v1\n");
    s.push_str("algorithm: ed25519\n");
    s.push_str(&format!("key: {}\n", to_hex(&obj.pubkey)));
    s.push_str(&format!("grants: {}\n", obj.grants.join(",")));
    s.push_str(&format!("signature: {}\n", to_hex(&obj.signature)));
    s
}

/// Parse a carrier from either form (detached, or a `//`-prefixed in-file block —
/// each line's optional leading `//` is stripped first).
pub fn parse_carrier(text: &str) -> Result<SignedObject, VerifyError> {
    let bad = |m: &str| VerifyError::Malformed(m.to_string());

    let mut magic_seen = false;
    let mut key: Option<[u8; 32]> = None;
    let mut sig: Option<[u8; 64]> = None;
    let mut grants: Vec<String> = Vec::new();

    for raw in text.lines() {
        // Strip an optional leading `//` (the in-file block form), then trim.
        let line = raw.trim_start();
        let line = line.strip_prefix("//").unwrap_or(line).trim();
        if line.is_empty() {
            continue;
        }
        if !magic_seen {
            if line.starts_with("sentinel-signature") {
                magic_seen = true;
            }
            // Ignore anything before the magic line (e.g. a leading file comment).
            continue;
        }
        let Some((k, v)) = line.split_once(':') else { continue };
        let (k, v) = (k.trim(), v.trim());
        match k {
            "algorithm" => {
                if v != "ed25519" {
                    return Err(bad(&format!("unsupported algorithm `{v}`")));
                }
            }
            "key" => {
                let b = from_hex(v).ok_or_else(|| bad("`key` is not valid hex"))?;
                key = Some(b.try_into().map_err(|_| bad("`key` must be 32 bytes"))?);
            }
            "signature" => {
                let b = from_hex(v).ok_or_else(|| bad("`signature` is not valid hex"))?;
                sig = Some(b.try_into().map_err(|_| bad("`signature` must be 64 bytes"))?);
            }
            "grants" => {
                grants = v
                    .split(',')
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .map(String::from)
                    .collect();
            }
            _ => {} // forward-compatible: ignore unknown fields.
        }
    }

    if !magic_seen {
        return Err(bad("missing `sentinel-signature` header"));
    }
    Ok(SignedObject {
        pubkey: key.ok_or_else(|| bad("missing `key`"))?,
        grants,
        signature: sig.ok_or_else(|| bad("missing `signature`"))?,
    })
}

/// Verify a carrier over `body`: recompute `SHA-512(body)`, rebuild the canonical
/// payload, and Ed25519-verify. On success returns the signer key + the granted
/// capabilities (for the gate's D6 capability check).
pub fn verify_object(body: &[u8], obj: &SignedObject) -> Result<Verified, VerifyError> {
    let body_hash = sha512(body);
    let payload = canonical_payload(&obj.pubkey, &obj.grants, &body_hash);
    if ed25519_verify(&obj.pubkey, &payload, &obj.signature) {
        Ok(Verified { pubkey: obj.pubkey, grants: obj.grants.clone() })
    } else {
        Err(VerifyError::BadSignature)
    }
}

/// Parse a carrier text and verify it over `body` (the common entry point).
pub fn verify_signed(body: &[u8], carrier_text: &str) -> Result<Verified, VerifyError> {
    let obj = parse_carrier(carrier_text)?;
    verify_object(body, &obj)
}

// ---- hex (no base64 dependency; the carrier is hex-encoded) ---------------------

fn to_hex(bytes: &[u8]) -> String {
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        s.push_str(&format!("{b:02x}"));
    }
    s
}

fn from_hex(s: &str) -> Option<Vec<u8>> {
    if s.len() % 2 != 0 {
        return None;
    }
    (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&s[i..i + 2], 16).ok())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ed25519::test_sign::{ed25519_pubkey, ed25519_sign};

    /// Sign `body` with a test seed + grants → a valid carrier text.
    fn sign_carrier(seed: &[u8; 32], body: &[u8], grants: &[&str]) -> String {
        let pubkey = ed25519_pubkey(seed);
        let grants: Vec<String> = grants.iter().map(|s| (*s).to_string()).collect();
        let payload = canonical_payload(&pubkey, &grants, &sha512(body));
        let signature = ed25519_sign(seed, &payload);
        serialize_carrier(&SignedObject { pubkey, grants, signature })
    }

    #[test]
    fn carrier_round_trips() {
        let obj = SignedObject {
            pubkey: [7u8; 32],
            grants: vec!["secret".into(), "constant_time".into()],
            signature: [9u8; 64],
        };
        let parsed = parse_carrier(&serialize_carrier(&obj)).unwrap();
        assert_eq!(parsed, obj);
    }

    #[test]
    fn in_file_block_form_parses_the_same() {
        let obj = SignedObject { pubkey: [3u8; 32], grants: vec![], signature: [4u8; 64] };
        // Prefix every line with `// ` and bury it under a leading file comment.
        let block: String = serialize_carrier(&obj)
            .lines()
            .map(|l| format!("// {l}\n"))
            .collect();
        let with_lead = format!("// demo program\n{block}");
        assert_eq!(parse_carrier(&with_lead).unwrap(), obj);
    }

    #[test]
    fn canonical_payload_is_grant_order_independent() {
        let pk = [1u8; 32];
        let h = [2u8; 64];
        let a = canonical_payload(&pk, &["secret".into(), "alloc".into()], &h);
        let b = canonical_payload(&pk, &["alloc".into(), "secret".into()], &h);
        assert_eq!(a, b, "grant order must not change the signed bytes");
    }

    #[test]
    fn valid_signature_verifies() {
        let seed = [42u8; 32];
        let body = b"verified constant-time crypto, as a library\n";
        let carrier = sign_carrier(&seed, body, &["secret", "constant_time"]);
        let v = verify_signed(body, &carrier).unwrap();
        assert_eq!(v.pubkey, ed25519_pubkey(&seed));
        assert_eq!(v.grants, vec!["secret".to_string(), "constant_time".to_string()]);
    }

    #[test]
    fn a_changed_source_byte_fails() {
        let seed = [42u8; 32];
        let carrier = sign_carrier(&seed, b"the original body\n", &[]);
        // One byte different (D2: any body byte change must fail).
        assert_eq!(verify_signed(b"the 0riginal body\n", &carrier), Err(VerifyError::BadSignature));
    }

    #[test]
    fn a_changed_comment_byte_fails() {
        // D2's headline: comments are signed. The body here *is* a comment line;
        // flipping a byte in it must fail just like code.
        let seed = [7u8; 32];
        let body = b"// SAFETY: declassify only the public tag\nlet t = declassify(tag);\n";
        let carrier = sign_carrier(&seed, body, &[]);
        let tampered = b"// SAFETY: declassify only the secret key\nlet t = declassify(tag);\n";
        assert_eq!(verify_signed(tampered, &carrier), Err(VerifyError::BadSignature));
    }

    #[test]
    fn tampered_grants_fail() {
        // The grants are part of the signed payload (D3/D6): editing them in the
        // carrier breaks the signature, so a key can't have its capabilities widened.
        let seed = [42u8; 32];
        let body = b"some library\n";
        let carrier = sign_carrier(&seed, body, &["secret"]);
        let widened = carrier.replace("grants: secret", "grants: secret,network");
        assert_eq!(verify_signed(body, &widened), Err(VerifyError::BadSignature));
    }

    #[test]
    fn malformed_carrier_is_reported() {
        assert!(matches!(verify_signed(b"x", "not a signature"), Err(VerifyError::Malformed(_))));
    }
}
