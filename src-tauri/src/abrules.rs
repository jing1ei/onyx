//! The A/B assignment rules, as pure functions.
//!
//! SPEC §2.8 ("A/B assignment") makes assignment a contract with four routes in
//! the front end — a drag onto a lane, the per-row `A` / `B` chips, `⇧A` / `⇧B`
//! and the row context menu — but only one rule underneath all of them:
//! **putting material on deck B is a request to compare, so it turns A/B on.**
//!
//! It lives here, on its own, for one reason: the front end has a second
//! implementation of this backend (`src/lib/mock.ts`, which the browser preview
//! and the screenshot harness run against), and the two drifted. The mock
//! assigned deck B without enabling A/B, so every verification done in the
//! preview passed while the real macOS build looked like "deck B is not
//! assignable" — the track *was* on deck B, and nothing showed it because the
//! comparison was still off.
//!
//! `tests/ab_assign_contract.rs` pins this function against a checked-in
//! fixture and `scripts/check-ab-parity.mjs` drives the real `src/lib/mock.ts`
//! through the same fixture, so neither side can move alone again.

use onyx_core::Deck;

/// Whether A/B is on after assigning `deck`, given whether it was on before.
///
/// Assigning deck B enables the comparison; assigning deck A never *disables*
/// it — turning A/B off is `ab_set_enabled`'s job and nothing else's.
pub const fn ab_enabled_after_assign(deck: Deck, was_enabled: bool) -> bool {
    was_enabled || matches!(deck, Deck::B)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deck_b_enables_and_deck_a_never_disables() {
        assert!(ab_enabled_after_assign(Deck::B, false));
        assert!(ab_enabled_after_assign(Deck::B, true));
        assert!(!ab_enabled_after_assign(Deck::A, false));
        assert!(ab_enabled_after_assign(Deck::A, true));
    }
}
