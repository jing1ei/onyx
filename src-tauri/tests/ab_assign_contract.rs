//! The A/B assignment contract, engine half (SPEC §2.8).
//!
//! Onyx has two implementations of its own backend: the real one in Rust, and
//! `src/lib/mock.ts`, which the browser preview and the screenshot harness run
//! against. They drifted on one rule — assigning a track to deck B turns A/B on
//! — and the drift is exactly what hid a shipped bug: in the preview deck B
//! assignment "worked" (the mock left A/B off, so lane B was never drawn and
//! nobody noticed), while on the real macOS build a user reported "deck b is
//! not assignable, only a".
//!
//! So the rule is a checked-in fixture rather than a sentence in a comment.
//! This test holds the Rust side to it; `scripts/check-ab-parity.mjs`
//! (`npm run check:ab`, wired into `npm run build` and `npm run build:mock`)
//! drives the real TypeScript mock through the same file. Neither side can be
//! changed alone.
//!
//! What this cannot cover here: `commands::ab_assign` itself needs a live
//! `AppState`, which needs an `AudioEngine`, which needs an output device. This
//! machine has none, so the command's *other* effects (the load, the trim
//! recompute) are out of reach; what is pinned is the rule the command calls.

use std::collections::BTreeSet;
use std::fs;
use std::path::PathBuf;

use onyx_core::Deck;
use onyx_lib::abrules::ab_enabled_after_assign;
use serde::Deserialize;

#[derive(Deserialize)]
struct Contract {
    cases: Vec<Case>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Case {
    name: String,
    deck: String,
    ab_enabled_before: bool,
    ab_enabled_after: bool,
}

fn fixture_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/ab_assign_contract.json")
}

fn load() -> Contract {
    let path = fixture_path();
    let text = fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("cannot read the A/B contract at {}: {e}", path.display()));
    serde_json::from_str(&text).unwrap_or_else(|e| panic!("cannot parse {}: {e}", path.display()))
}

fn deck_of(name: &str) -> Deck {
    match name {
        "a" => Deck::A,
        "b" => Deck::B,
        other => panic!("the contract names a deck that does not exist: {other:?}"),
    }
}

#[test]
fn the_engine_obeys_the_assignment_contract() {
    let contract = load();
    assert!(!contract.cases.is_empty(), "the contract is empty");
    for case in &contract.cases {
        let got = ab_enabled_after_assign(deck_of(&case.deck), case.ab_enabled_before);
        assert_eq!(
            got,
            case.ab_enabled_after,
            "{}: assigning deck {} with A/B {} left it {}, contract says {}",
            case.name,
            case.deck.to_uppercase(),
            case.ab_enabled_before,
            got,
            case.ab_enabled_after,
        );
    }
}

/// A contract that only described the case that broke would let the opposite
/// regression through, so it has to cover every (deck, prior state) pair.
#[test]
fn the_contract_covers_every_combination() {
    let contract = load();
    let seen: BTreeSet<(String, bool)> = contract
        .cases
        .iter()
        .map(|c| (c.deck.clone(), c.ab_enabled_before))
        .collect();
    for deck in ["a", "b"] {
        for before in [false, true] {
            assert!(
                seen.contains(&(deck.to_string(), before)),
                "the contract says nothing about assigning deck {} with A/B {before}",
                deck.to_uppercase(),
            );
        }
    }
}
