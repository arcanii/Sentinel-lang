//! `sentinel-trust` — code signing & supply-chain trust (ADR 0061).
//!
//! The "trusted" half of the pre-built-libraries rock: signed artifacts, a
//! consumer trust manifest, and capability-bounded keys, gating compilation so
//! that *"the code you build is the code you reviewed, signed by an authorized
//! party"* (BACKLOG2 §2 / AI_TOOLING §7.1).
//!
//! This first v1 increment ships the cryptographic foundation: the **in-process
//! Ed25519 verifier** the build-time gate runs (ADR 0061 D4 — the Rust twin of
//! `std::security::ed25519`, validated on the same RFC 8032 vectors) and the
//! **SHA-512** it composes. The signed-manifest format (D3), the consumer trust
//! manifest (D5), the build-time gate (D7), and capability bounding against the
//! effect row (D6) land in the following increments. Authoring (`snc keygen` /
//! `snc sign`) stays pure Sentinel (dogfooding `std::security::ed25519` +
//! `getentropy`); only the verify side lives here, inside `snc`'s trust boundary.

mod ed25519;
mod format;
mod sha512;

pub use ed25519::ed25519_verify;
pub use format::{
    canonical_payload, parse_carrier, serialize_carrier, verify_object, verify_signed, SignedObject,
    Verified, VerifyError, ALGO_ED25519, DOMAIN,
};
pub use sha512::sha512;
