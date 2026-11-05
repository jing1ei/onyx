//! Assignment preserves the comparison switch. Only explicit user actions enable A/B.
use onyx_core::Deck;

pub const fn ab_enabled_after_assign(_deck: Deck, was_enabled: bool) -> bool {
    was_enabled
}

pub const fn playback_deck(comparison: bool, active: Deck) -> Deck {
    if comparison {
        active
    } else {
        Deck::A
    }
}

pub fn require_comparison(enabled: bool) -> Result<(), String> {
    if enabled {
        Ok(())
    } else {
        Err("Enable A/B comparison to switch decks.".into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn assignment_preserves_comparison_switch() {
        for deck in [Deck::A, Deck::B] {
            assert!(!ab_enabled_after_assign(deck, false));
            assert!(ab_enabled_after_assign(deck, true));
        }
    }

    #[test]
    fn playback_targets_only_the_audible_deck() {
        assert_eq!(playback_deck(false, Deck::A), Deck::A);
        assert_eq!(playback_deck(false, Deck::B), Deck::A);
        assert_eq!(playback_deck(true, Deck::A), Deck::A);
        assert_eq!(playback_deck(true, Deck::B), Deck::B);
    }

    #[test]
    fn switching_requires_explicit_comparison() {
        assert!(require_comparison(true).is_ok());
        assert_eq!(
            require_comparison(false).unwrap_err(),
            "Enable A/B comparison to switch decks."
        );
    }
}
