#![no_std]

//! Shared TTL (time-to-live) constants for all storage entries across the
//! ClearLedger contract workspace.
//!
//! `THRESHOLD` is the minimum remaining ledger count before an entry is
//! eligible for extension; `EXTEND_TO` is the ledger count the entry's TTL is
//! extended to when that threshold is crossed.
//!
//! With `EXTEND_TO = 2_000_000`, a `THRESHOLD` of `500_000` (25% of
//! `EXTEND_TO`) ensures renewals happen well before expiry rather than at the
//! last second.
//!
//! Every `extend_ttl` call across all contracts should go through these
//! constants so the bump policy stays consistent.

/// Threshold (minimum remaining ledgers before extension triggers): 25% of
/// `EXTEND_TO`, so renewals fire with plenty of headroom.
pub const THRESHOLD: u32 = 500_000;
/// Ledger count the entry is extended to when `THRESHOLD` is crossed.
pub const EXTEND_TO: u32 = 2_000_000;

// Compile-time invariants: a future edit that breaks these fails the build
// immediately, before any test even runs.
const _: () = assert!(THRESHOLD < EXTEND_TO);
const _: () = assert!(THRESHOLD > 0);
const _: () = assert!(EXTEND_TO > 0);

#[cfg(test)]
mod test {
    use super::*;

    /// Pins the invariant `THRESHOLD < EXTEND_TO` as a `cargo test` target
    /// (in addition to the `const _: ()` compile-time check above), so it
    /// shows up in `cargo test --workspace` output.
    #[test]
    fn threshold_is_below_extend_to() {
        let threshold: u32 = THRESHOLD;
        let extend_to: u32 = EXTEND_TO;
        assert!(threshold < extend_to);
    }

    #[test]
    fn constants_are_non_zero() {
        let threshold: u32 = THRESHOLD;
        let extend_to: u32 = EXTEND_TO;
        assert!(threshold > 0);
        assert!(extend_to > 0);
    }
}
