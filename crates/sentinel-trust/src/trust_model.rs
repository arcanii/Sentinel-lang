//! The consumer trust manifest (ADR 0061 D5) and the build-time gate's per-module
//! evaluation (D7).
//!
//! *Signed ≠ trusted* (D1): a valid signature only says "this key vouches for
//! these bytes". Trust is the **consumer's** decision, expressed in a
//! `sentinel-trust.toml` that lists the keys this project relies on — and,
//! optionally, the capability ceiling each key is granted (D6). The gate verifies
//! a module's signature, then resolves it against the manifest; the driver maps
//! the [`GateOutcome`] to an action per the active [`SigPolicy`].

use serde::Deserialize;

use crate::format::{to_hex, verify_signed, VerifyError, Verified};

/// A consumer trust manifest: the keys this project trusts to sign its modules.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TrustManifest {
    #[serde(default)]
    pub keys: Vec<TrustedKey>,
}

/// One trusted key entry.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TrustedKey {
    /// The Ed25519 public key as 64 hex chars.
    pub pubkey: String,
    /// An optional human label (for diagnostics).
    #[serde(default)]
    pub name: Option<String>,
    /// The capability ceiling this key may publish (D6). Empty = no manifest
    /// ceiling (the signature's own grants apply unrestricted).
    #[serde(default)]
    pub grants: Vec<String>,
}

impl TrustManifest {
    /// Parse a manifest from TOML.
    pub fn parse(text: &str) -> Result<Self, String> {
        toml::from_str(text).map_err(|e| format!("invalid trust manifest: {e}"))
    }

    /// The trusted entry for `pubkey`, if any (hex compare, case-insensitive).
    #[must_use]
    pub fn trusted(&self, pubkey: &[u8; 32]) -> Option<&TrustedKey> {
        let want = to_hex(pubkey);
        self.keys.iter().find(|k| k.pubkey.trim().eq_ignore_ascii_case(&want))
    }
}

/// How strictly the build enforces signatures (ADR 0061 D7).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SigPolicy {
    /// No enforcement (today's default).
    Off,
    /// Verify; warn on any problem but build anyway.
    Warn,
    /// A module that is unsigned, signed by an untrusted key, or whose signature
    /// does not verify fails the build.
    Strict,
}

impl SigPolicy {
    /// Parse `off` / `warn` / `strict`.
    #[must_use]
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "off" => Some(SigPolicy::Off),
            "warn" => Some(SigPolicy::Warn),
            "strict" => Some(SigPolicy::Strict),
            _ => None,
        }
    }
}

/// The verdict for one module at the trust gate.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GateOutcome {
    /// Verified by a key in the trust manifest. Carries the **effective** grants
    /// (the signature's grants, narrowed by the key's manifest ceiling) for the
    /// capability check (D6).
    Trusted { pubkey: [u8; 32], grants: Vec<String> },
    /// No signature carrier was found.
    Unsigned,
    /// A carrier was present but did not verify over the bytes.
    BadSignature(VerifyError),
    /// The signature verified, but the signing key is not in the trust manifest.
    Untrusted { pubkey: [u8; 32] },
}

/// Evaluate one module: verify `carrier` (if present) over `body`, then resolve
/// the signing key against `manifest`.
#[must_use]
pub fn evaluate(body: &[u8], carrier: Option<&str>, manifest: &TrustManifest) -> GateOutcome {
    let Some(carrier) = carrier else {
        return GateOutcome::Unsigned;
    };
    let v: Verified = match verify_signed(body, carrier) {
        Ok(v) => v,
        Err(e) => return GateOutcome::BadSignature(e),
    };
    match manifest.trusted(&v.pubkey) {
        Some(key) => GateOutcome::Trusted {
            pubkey: v.pubkey,
            grants: effective_grants(&v.grants, &key.grants),
        },
        None => GateOutcome::Untrusted { pubkey: v.pubkey },
    }
}

/// The capabilities actually in force for a trusted module: the signature's
/// grants, intersected with the key's manifest ceiling when the manifest sets one
/// (D6 — both the author's self-declaration and the consumer's ceiling must
/// permit a capability).
fn effective_grants(sig_grants: &[String], manifest_grants: &[String]) -> Vec<String> {
    if manifest_grants.is_empty() {
        sig_grants.to_vec()
    } else {
        sig_grants
            .iter()
            .filter(|g| manifest_grants.iter().any(|m| m == *g))
            .cloned()
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ed25519::test_sign::{ed25519_pubkey, ed25519_sign};
    use crate::format::{canonical_payload, serialize_carrier, SignedObject};
    use crate::sha512;

    fn sign(seed: &[u8; 32], body: &[u8], grants: &[&str]) -> String {
        let pubkey = ed25519_pubkey(seed);
        let grants: Vec<String> = grants.iter().map(|s| (*s).to_string()).collect();
        let payload = canonical_payload(&pubkey, &grants, &sha512(body));
        let signature = ed25519_sign(seed, &payload);
        serialize_carrier(&SignedObject { pubkey, grants, signature })
    }

    fn manifest_trusting(seed: &[u8; 32], grants: &[&str]) -> TrustManifest {
        let pk: String = ed25519_pubkey(seed).iter().map(|b| format!("{b:02x}")).collect();
        let grants = grants.iter().map(|g| format!("\"{g}\"")).collect::<Vec<_>>().join(", ");
        TrustManifest::parse(&format!("[[keys]]\npubkey = \"{pk}\"\ngrants = [{grants}]\n")).unwrap()
    }

    #[test]
    fn trusted_key_with_valid_signature() {
        let seed = [1u8; 32];
        let body = b"library bytes\n";
        let carrier = sign(&seed, body, &["alloc", "secret"]);
        let m = manifest_trusting(&seed, &[]); // no ceiling → sig grants apply
        match evaluate(body, Some(&carrier), &m) {
            GateOutcome::Trusted { grants, .. } => assert_eq!(grants, vec!["alloc", "secret"]),
            other => panic!("expected Trusted, got {other:?}"),
        }
    }

    #[test]
    fn manifest_ceiling_narrows_grants() {
        let seed = [2u8; 32];
        let body = b"x\n";
        let carrier = sign(&seed, body, &["alloc", "network", "secret"]);
        let m = manifest_trusting(&seed, &["alloc", "secret"]); // ceiling forbids network
        match evaluate(body, Some(&carrier), &m) {
            GateOutcome::Trusted { grants, .. } => assert_eq!(grants, vec!["alloc", "secret"]),
            other => panic!("expected Trusted, got {other:?}"),
        }
    }

    #[test]
    fn untrusted_key_is_flagged() {
        let seed = [3u8; 32];
        let body = b"x\n";
        let carrier = sign(&seed, body, &[]);
        let m = TrustManifest::default(); // trusts nobody
        assert!(matches!(evaluate(body, Some(&carrier), &m), GateOutcome::Untrusted { .. }));
    }

    #[test]
    fn unsigned_is_flagged() {
        assert_eq!(evaluate(b"x", None, &TrustManifest::default()), GateOutcome::Unsigned);
    }

    #[test]
    fn tampered_body_is_bad_signature() {
        let seed = [4u8; 32];
        let carrier = sign(&seed, b"original\n", &[]);
        let m = manifest_trusting(&seed, &[]);
        assert!(matches!(evaluate(b"tampered\n", Some(&carrier), &m), GateOutcome::BadSignature(_)));
    }

    #[test]
    fn unknown_manifest_field_is_rejected() {
        assert!(TrustManifest::parse("[[keys]]\npubkey=\"00\"\nbogus=1\n").is_err());
    }
}
