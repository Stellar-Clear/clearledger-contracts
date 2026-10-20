#![no_std]

//! Shared emergency-pause state for all storage entries across the ClearLedger
//! contract workspace.
//!
//! There is a single process-wide circuit-breaker flag that every contract
//! consults from its state-changing entry points. Rather than duplicating the
//! storage key, defaults, and panic behaviour in all four contracts (registry,
//! invoice, escrow, pool), they share this tiny crate — exactly the pattern
//! already used by [`clearledger-ttl`](https://docs.rs/clearledger-ttl).
//!
//! # Usage
//!
//! ```ignore
//! use clearledger_pause::{require_not_paused, set_paused};
//!
//! // Guard a state-changing entry point:
//! pub fn create(env: Env, ..) -> bool {
//!     clearledger_pause::require_not_paused(&env);
//!     // .. proceed with the write
//! }
//!
//! // Flip the breaker from an admin-only entry point:
//! pub fn admin_pause(env: Env) {
//!     admin.require_auth();
//!     set_paused(&env, true);
//! }
//! ```
//!
//! The flag lives in **instance** storage: it is read on nearly every
//! entry point, and instance storage is loaded and TTL-managed together with
//! the rest of a contract's configuration.

use soroban_sdk::{contracterror, contracttype, panic_with_error, Env};

/// Instance-storage key under which the shared emergency-pause flag is stored.
///
/// Only one key is ever used, so the value (`bool`) unambiguously identifies
/// the pause state within a contract's instance storage.
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum PauseState {
    /// The single `bool` pause flag (`true` = paused, `false`/absent = live).
    Paused,
}

/// Error returned by [`require_not_paused`] when the breaker is engaged.
///
/// Consuming contracts surface this as `Error(Contract, #1)` within their own
/// error space, mirroring every other shared-helper failure in the workspace.
#[contracterror]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PauseError {
    /// The contract is paused; the guarded operation must not proceed.
    ContractPaused = 1,
}

/// Returns `true` when the contract is currently paused.
///
/// An absent flag is treated as *not paused*, so a freshly deployed contract
/// starts live and only ever becomes paused through an explicit
/// [`set_paused`] call.
pub fn is_paused(env: &Env) -> bool {
    env.storage()
        .instance()
        .get(&PauseState::Paused)
        .unwrap_or(false)
}

/// Sets the emergency-pause flag to `paused`.
///
/// Callers are expected to gate this behind an admin authorization check;
/// this crate deliberately does not impose an auth model so each contract can
/// wire it into its own admin path.
pub fn set_paused(env: &Env, paused: bool) {
    env.storage().instance().set(&PauseState::Paused, &paused);
}

/// Panics with [`PauseError::ContractPaused`] if the contract is paused.
///
/// Returns `()` (and never panics) when the contract is live, so it can be
/// called unconditionally at the top of a state-changing entry point.
pub fn require_not_paused(env: &Env) {
    if is_paused(env) {
        panic_with_error!(env, PauseError::ContractPaused);
    }
}

#[cfg(test)]
mod test {
    use super::*;
    use soroban_sdk::{contract, contractimpl, Env};

    /// Minimal harness so the helpers run inside a real contract frame — the
    /// same context they see in production — where instance storage exists.
    #[contract]
    struct PauseHarness;

    #[contractimpl]
    impl PauseHarness {
        pub fn pause(env: Env) {
            set_paused(&env, true);
        }

        pub fn unpause(env: Env) {
            set_paused(&env, false);
        }

        pub fn paused(env: Env) -> bool {
            is_paused(&env)
        }

        pub fn guarded(env: Env) {
            require_not_paused(&env);
        }
    }

    fn harness(env: &Env) -> PauseHarnessClient<'_> {
        let id = env.register_contract(None, PauseHarness);
        PauseHarnessClient::new(env, &id)
    }

    #[test]
    fn defaults_to_unpaused() {
        let env = Env::default();
        let client = harness(&env);
        assert!(!client.paused());
    }

    #[test]
    fn set_paused_round_trips_for_both_states() {
        let env = Env::default();
        let client = harness(&env);

        client.pause();
        assert!(client.paused());

        client.unpause();
        assert!(!client.paused());
    }

    #[test]
    fn require_not_paused_passes_when_unpaused() {
        let env = Env::default();
        let client = harness(&env);
        client.unpause();
        // Must not panic while live.
        client.guarded();
    }

    #[test]
    #[should_panic(expected = "Error(Contract, #1)")]
    fn require_not_paused_panics_when_paused() {
        let env = Env::default();
        let client = harness(&env);
        client.pause();
        client.guarded();
    }
}
