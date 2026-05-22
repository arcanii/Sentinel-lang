//! sentinel-mir
//!
//! SSA-form mid-level IR with optimizations
//!
//! Scaffold stub. Real implementation begins in the phase described
//! in HANDOVER.md Section 6.2.

/// Returns the crate name as a sanity-check that the build is wired up.
pub fn crate_name() -> &'static str {
    "sentinel-mir"
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn smoke() {
        assert_eq!(crate_name(), "sentinel-mir");
    }
}