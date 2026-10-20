//! Shared TTL constants re-exported from the workspace-level `clearledger-ttl`
//! crate so the bump policy stays consistent across all contracts.
//!
//! `TTL_THRESHOLD` is the minimum number of ledgers an entry must have
//! remaining before it is extended (25% of `TTL_EXTEND_TO`), and
//! `TTL_EXTEND_TO` is the number of ledgers the entry is extended to.

pub use clearledger_ttl::EXTEND_TO as TTL_EXTEND_TO;
pub use clearledger_ttl::THRESHOLD as TTL_THRESHOLD;
