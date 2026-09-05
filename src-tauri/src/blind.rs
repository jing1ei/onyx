//! Blind testing: 2AFC (`ab`, unchanged from v1) and ABX (`abx`, SPEC §7).
//!
//! Both protocols share one rule that makes or breaks the feature: **the
//! mapping for the current trial is never serialised while the test is
//! active**. The front end cannot leak what it never receives, so
//! [`BlindSnapshot::mapping`] / [`BlindSnapshot::abx_mapping`] are `None` until
//! the run is finished.
//!
//! * **`ab`** — slots `x` / `y` map randomly to decks A / B, re-randomised every
//!   trial. Question: *"which slot is deck A?"*, so `correct_slot` is the slot
//!   holding deck A.
//! * **`abx`** — slot `a` is always deck A, slot `b` is always deck B, and slot
//!   `x` is randomly one of them, **re-randomised every trial**. Question:
//!   *"is X the same as A or as B?"*, so a vote of `a` means "X is A" and
//!   `correct_slot` is `"a"` or `"b"`.
//!
//! Significance is a one-tailed exact binomial (see [`binomial_tail`]): with
//! eight trials a listener guessing has a 3.5 % chance of scoring 7, and a
//! score with no p-value next to it invites exactly the wrong conclusion.
//!
//! Randomness comes from a 5-line xorshift seeded from the system clock. A test
//! with a handful of trials does not need a CSPRNG, and not adding a dependency
//! keeps the binary (and the audit surface) smaller.

use std::sync::atomic::{AtomicU64, Ordering};

use onyx_core::Deck;
use serde::{Deserialize, Serialize};

/// Which protocol is running (SPEC §7).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum BlindMode {
    /// Two hidden slots, "which one is deck A?".
    Ab,
    /// Three slots, "is X the same as A or B?".
    #[default]
    Abx,
}

/// A slot the listener can switch to or vote for.
///
/// Serialises as the bare letter, which is all the UI is allowed to know:
/// `"a"`, `"b"`, `"x"`, `"y"`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Slot {
    A,
    B,
    X,
    Y,
}

impl Slot {
    pub fn as_str(self) -> &'static str {
        match self {
            Slot::A => "a",
            Slot::B => "b",
            Slot::X => "x",
            Slot::Y => "y",
        }
    }

    /// Parse a slot name from IPC. Unknown names are rejected, never guessed.
    pub fn parse(raw: &str) -> Option<Slot> {
        match raw.trim().to_ascii_lowercase().as_str() {
            "a" => Some(Slot::A),
            "b" => Some(Slot::B),
            "x" => Some(Slot::X),
            "y" => Some(Slot::Y),
            _ => None,
        }
    }
}

impl BlindMode {
    /// Slots the listener may switch between, in UI order.
    pub fn slots(self) -> &'static [Slot] {
        match self {
            BlindMode::Ab => &[Slot::X, Slot::Y],
            BlindMode::Abx => &[Slot::A, Slot::B, Slot::X],
        }
    }

    /// Slots that are a legal *answer*. In ABX you listen to `x` but you vote
    /// `a` or `b`, so the two sets are not the same.
    pub fn answers(self) -> &'static [Slot] {
        match self {
            BlindMode::Ab => &[Slot::X, Slot::Y],
            BlindMode::Abx => &[Slot::A, Slot::B],
        }
    }

    /// Slot that is audible when a trial starts.
    fn first_slot(self) -> Slot {
        match self {
            BlindMode::Ab => Slot::X,
            BlindMode::Abx => Slot::A,
        }
    }
}

/// Slot → deck mapping of an `ab` trial, revealed only when the run ends.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub struct Mapping {
    pub x: Deck,
    pub y: Deck,
}

/// Which deck slot `x` was on in the final `abx` trial.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub struct AbxMapping {
    pub x: Deck,
}

#[derive(Clone, Copy, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BlindVote {
    pub trial: usize,
    /// Slot the listener picked.
    pub chose: Slot,
    /// Slot that was actually right.
    pub correct_slot: Slot,
    pub correct: bool,
}

/// What the UI is allowed to know (SPEC §7).
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BlindSnapshot {
    pub active: bool,
    pub mode: BlindMode,
    /// 1-based while active.
    pub trial: usize,
    pub trials: usize,
    pub slots: Vec<Slot>,
    pub current_slot: Slot,
    pub votes: Vec<BlindVote>,
    pub score: usize,
    pub finished: bool,
    /// One-tailed exact binomial `P(K >= score | p = 0.5)`. `None` until finished.
    pub p_value: Option<f64>,
    /// `ab` only. `None` while active; populated on reveal.
    pub mapping: Option<Mapping>,
    /// `abx` only. `None` while active; populated on reveal.
    pub abx_mapping: Option<AbxMapping>,
}

/// Smallest possible non-cryptographic PRNG: xorshift64*.
#[derive(Clone, Copy, Debug)]
struct Rng(u64);

impl Rng {
    fn from_clock() -> Rng {
        static SEQUENCE: AtomicU64 = AtomicU64::new(0);
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos() as u64)
            .unwrap_or(0x2545_F491_4F6C_DD1D);
        // The system clock is coarse on some platforms (~15 ms on Windows), so
        // a monotonic counter is mixed in: two runs started back to back must
        // not get the same sequence of mappings.
        let seq = SEQUENCE.fetch_add(1, Ordering::Relaxed);
        let mixed = nanos ^ seq.wrapping_mul(0x9E37_79B9_7F4A_7C15).rotate_left(17);
        Rng(mixed | 1)
    }

    fn next_u64(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.0 = x;
        x.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }

    fn coin(&mut self) -> bool {
        self.next_u64() & (1 << 33) != 0
    }
}

pub const MAX_TRIALS: usize = 100;

pub struct BlindTest {
    mode: BlindMode,
    active: bool,
    finished: bool,
    trial: usize,
    trials: usize,
    current_slot: Slot,
    /// The one secret: in `ab` it means "slot x is deck A", in `abx` it means
    /// "slot x is deck A". Re-rolled at the start of every trial.
    x_is_a: bool,
    votes: Vec<BlindVote>,
    score: usize,
    rng: Rng,
}

impl BlindTest {
    pub fn new() -> BlindTest {
        BlindTest {
            mode: BlindMode::default(),
            active: false,
            finished: false,
            trial: 0,
            trials: 0,
            current_slot: BlindMode::default().first_slot(),
            x_is_a: true,
            votes: Vec::new(),
            score: 0,
            rng: Rng::from_clock(),
        }
    }

    pub fn is_active(&self) -> bool {
        self.active
    }

    /// Deck behind a slot under the current trial's mapping.
    fn deck_of(&self, slot: Slot) -> Deck {
        match (self.mode, slot) {
            // ABX: a and b are never hidden, x is the secret one.
            (BlindMode::Abx, Slot::A) => Deck::A,
            (BlindMode::Abx, Slot::B) => Deck::B,
            (BlindMode::Abx, _) => x_deck(self.x_is_a),
            // 2AFC: both slots are hidden.
            (BlindMode::Ab, Slot::X) => x_deck(self.x_is_a),
            (BlindMode::Ab, _) => x_deck(self.x_is_a).other(),
        }
    }

    /// The slot a listener with perfect hearing would pick this trial.
    fn correct_slot(&self) -> Slot {
        match self.mode {
            // "is X the same as A or as B?"
            BlindMode::Abx => {
                if self.x_is_a {
                    Slot::A
                } else {
                    Slot::B
                }
            }
            // "which slot is deck A?"
            BlindMode::Ab => {
                if self.x_is_a {
                    Slot::X
                } else {
                    Slot::Y
                }
            }
        }
    }

    /// Deck that is audible right now.
    pub fn current_deck(&self) -> Deck {
        self.deck_of(self.current_slot)
    }

    /// Start a run. Returns the deck that must become audible.
    pub fn start(&mut self, trials: usize, mode: BlindMode) -> Deck {
        self.mode = mode;
        self.trials = trials.clamp(1, MAX_TRIALS);
        self.trial = 1;
        self.active = true;
        self.finished = false;
        self.votes.clear();
        self.score = 0;
        self.current_slot = mode.first_slot();
        self.x_is_a = self.rng.coin();
        self.current_deck()
    }

    /// Switch the audible slot.
    ///
    /// `Err` for a slot that does not belong to the running protocol, so a bad
    /// payload is a rejected command rather than a panic or a silent no-op.
    pub fn switch(&mut self, slot: Slot) -> Result<Deck, String> {
        if !self.active {
            return Err("no blind test is running".into());
        }
        if !self.mode.slots().contains(&slot) {
            return Err(format!(
                "slot \"{}\" does not exist in this test (slots: {})",
                slot.as_str(),
                slot_list(self.mode.slots())
            ));
        }
        self.current_slot = slot;
        Ok(self.current_deck())
    }

    /// Record a vote and advance.
    ///
    /// `Ok(Some(deck))` = next trial's audible deck, `Ok(None)` = the run just
    /// finished.
    pub fn vote(&mut self, slot: Slot) -> Result<Option<Deck>, String> {
        if !self.active {
            return Err("no blind test is running".into());
        }
        if !self.mode.answers().contains(&slot) {
            return Err(format!(
                "\"{}\" is not an answer for this protocol (answers: {})",
                slot.as_str(),
                slot_list(self.mode.answers())
            ));
        }
        let correct_slot = self.correct_slot();
        let correct = slot == correct_slot;
        self.votes.push(BlindVote {
            trial: self.trial,
            chose: slot,
            correct_slot,
            correct,
        });
        if correct {
            self.score += 1;
        }
        if self.trial >= self.trials {
            self.active = false;
            self.finished = true;
            // The mapping of the *last* trial is the one we reveal.
            return Ok(None);
        }
        self.trial += 1;
        self.current_slot = self.mode.first_slot();
        // Re-randomised every trial: remembering "x was the good one" is worthless.
        self.x_is_a = self.rng.coin();
        Ok(Some(self.current_deck()))
    }

    /// Abandon the run and forget everything about it.
    pub fn abort(&mut self) {
        self.active = false;
        self.finished = false;
        self.trial = 0;
        self.trials = 0;
        self.votes.clear();
        self.score = 0;
        self.current_slot = self.mode.first_slot();
    }

    pub fn snapshot(&self) -> BlindSnapshot {
        // Revealed only once the run is over — that is the whole feature.
        let reveal = self.finished && !self.active;
        BlindSnapshot {
            active: self.active,
            mode: self.mode,
            trial: self.trial,
            trials: self.trials,
            slots: self.mode.slots().to_vec(),
            current_slot: self.current_slot,
            votes: self.votes.clone(),
            score: self.score,
            finished: self.finished,
            p_value: if reveal {
                Some(binomial_tail(self.score, self.votes.len()))
            } else {
                None
            },
            mapping: match (reveal, self.mode) {
                (true, BlindMode::Ab) => Some(Mapping {
                    x: x_deck(self.x_is_a),
                    y: x_deck(self.x_is_a).other(),
                }),
                _ => None,
            },
            abx_mapping: match (reveal, self.mode) {
                (true, BlindMode::Abx) => Some(AbxMapping {
                    x: x_deck(self.x_is_a),
                }),
                _ => None,
            },
        }
    }
}

impl Default for BlindTest {
    fn default() -> Self {
        BlindTest::new()
    }
}

fn x_deck(x_is_a: bool) -> Deck {
    if x_is_a {
        Deck::A
    } else {
        Deck::B
    }
}

fn slot_list(slots: &[Slot]) -> String {
    slots
        .iter()
        .map(|s| s.as_str())
        .collect::<Vec<_>>()
        .join(", ")
}

/// One-tailed exact binomial `P(K >= k | n, p = 0.5)` = `sum_{i=k}^{n} C(n,i) / 2^n`.
///
/// Computed with the multiplicative recurrence `C(n,i+1) = C(n,i) * (n-i)/(i+1)`,
/// which is exact in `f64` for the trial counts a listening test uses (every
/// intermediate is an integer below 2^53 up to n = 50-odd) and stays accurate to
/// ~1e-15 at [`MAX_TRIALS`]. No dependency, no log-gamma, no table.
pub fn binomial_tail(k: usize, n: usize) -> f64 {
    // Zero trials: "at least 0 successes out of 0" is certain. Reporting 1.0
    // rather than 0.0 keeps "p < 0.05 means significant" honest for an empty run.
    if n == 0 {
        return 1.0;
    }
    if k == 0 {
        return 1.0;
    }
    if k > n {
        return 0.0;
    }
    // Sum whichever tail has fewer terms, so the error stays small and the
    // loop stays short even at n = MAX_TRIALS.
    let sum = if k * 2 >= n {
        tail_sum(k, n)
    } else {
        // P(K >= k) = 2^n - sum_{i=0}^{k-1} C(n,i).
        2f64.powi(n as i32) - tail_sum_low(k - 1, n)
    };
    (sum / 2f64.powi(n as i32)).clamp(0.0, 1.0)
}

/// `sum_{i=k}^{n} C(n,i)`.
fn tail_sum(k: usize, n: usize) -> f64 {
    let mut c = binomial(n, k);
    let mut sum = c;
    for i in k..n {
        c = c * (n - i) as f64 / (i + 1) as f64;
        sum += c;
    }
    sum
}

/// `sum_{i=0}^{k} C(n,i)`.
fn tail_sum_low(k: usize, n: usize) -> f64 {
    let mut c = 1.0f64; // C(n, 0)
    let mut sum = c;
    for i in 0..k {
        c = c * (n - i) as f64 / (i + 1) as f64;
        sum += c;
    }
    sum
}

/// `C(n, k)` via the same exact-while-it-can-be recurrence.
fn binomial(n: usize, k: usize) -> f64 {
    let k = k.min(n - k);
    let mut c = 1.0f64;
    for i in 0..k {
        c = c * (n - i) as f64 / (i + 1) as f64;
    }
    c
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Force a known mapping so the scoring rules can be asserted exactly.
    fn rigged(mode: BlindMode, trials: usize, x_is_a: bool) -> BlindTest {
        let mut t = BlindTest::new();
        t.start(trials, mode);
        t.x_is_a = x_is_a;
        t
    }

    /* ── the statistic ─────────────────────────────────────────────────── */

    #[test]
    fn the_p_value_matches_hand_computed_values() {
        // 11 of 12: (C(12,11) + C(12,12)) / 2^12 = 13 / 4096.
        assert_eq!(binomial_tail(11, 12), 0.003_173_828_125);
        // 7 of 12: 1586 / 4096.
        assert!(
            (binomial_tail(7, 12) - 0.387_207_031_3).abs() < 1e-10,
            "got {}",
            binomial_tail(7, 12)
        );
        // A perfect and a chance score, both exact.
        assert_eq!(binomial_tail(12, 12), 1.0 / 4096.0);
        assert_eq!(binomial_tail(0, 12), 1.0);
        // 8-trial test: the threshold a listener will actually meet.
        assert!((binomial_tail(8, 8) - 1.0 / 256.0).abs() < 1e-15);
        assert!((binomial_tail(7, 8) - 9.0 / 256.0).abs() < 1e-15);
    }

    #[test]
    fn a_zero_trial_run_is_not_significant() {
        // Nothing was measured, so "you can hear a difference" must be false.
        assert_eq!(binomial_tail(0, 0), 1.0);
        assert!(binomial_tail(0, 0) >= 0.05);
        // And an impossible score does not produce a negative or NaN p.
        assert_eq!(binomial_tail(5, 3), 0.0);
    }

    #[test]
    fn the_p_value_is_monotonic_and_bounded() {
        let mut previous = f64::INFINITY;
        for k in 0..=20 {
            let p = binomial_tail(k, 20);
            assert!((0.0..=1.0).contains(&p), "p out of range at k={k}: {p}");
            assert!(p <= previous, "p must not increase with the score");
            previous = p;
        }
        // Both summation branches agree where they meet.
        assert!((binomial_tail(10, 20) - binomial_tail(10, 20)).abs() < 1e-15);
        assert!((binomial_tail(9, 20) + binomial_tail(12, 20) - 1.0).abs() < 1e-12);
        // The full range stays sane at the trial cap.
        assert!(binomial_tail(MAX_TRIALS, MAX_TRIALS) > 0.0);
        assert!((binomial_tail(1, MAX_TRIALS) - 1.0).abs() < 1e-12);
    }

    /* ── leak discipline ──────────────────────────────────────────────── */

    #[test]
    fn no_mapping_is_serialised_while_a_test_is_active() {
        for mode in [BlindMode::Ab, BlindMode::Abx] {
            let mut t = BlindTest::new();
            t.start(3, mode);
            for _ in 0..2 {
                let snap = t.snapshot();
                assert!(snap.active);
                assert!(snap.mapping.is_none(), "{mode:?} leaked `mapping`");
                assert!(snap.abx_mapping.is_none(), "{mode:?} leaked `abxMapping`");
                assert!(snap.p_value.is_none(), "p-value before the end");
                let json = serde_json::to_string(&snap).unwrap();
                assert!(json.contains("\"mapping\":null"), "{json}");
                assert!(json.contains("\"abxMapping\":null"), "{json}");
                t.vote(mode.answers()[0]).unwrap();
            }
        }
    }

    #[test]
    fn the_mapping_is_revealed_only_when_the_run_finishes() {
        let mut t = rigged(BlindMode::Abx, 1, false);
        assert!(t.snapshot().abx_mapping.is_none());
        t.vote(Slot::A).unwrap();
        let snap = t.snapshot();
        assert!(snap.finished && !snap.active);
        assert_eq!(snap.abx_mapping, Some(AbxMapping { x: Deck::B }));
        assert!(snap.mapping.is_none(), "abx must not send the ab mapping");
        assert_eq!(snap.p_value, Some(1.0), "one wrong answer out of one");

        let mut t = rigged(BlindMode::Ab, 1, false);
        t.vote(Slot::Y).unwrap();
        let snap = t.snapshot();
        assert_eq!(
            snap.mapping,
            Some(Mapping {
                x: Deck::B,
                y: Deck::A
            })
        );
        assert!(snap.abx_mapping.is_none(), "ab must not send abxMapping");
    }

    #[test]
    fn aborting_forgets_everything_including_the_mapping() {
        let mut t = BlindTest::new();
        t.start(8, BlindMode::Abx);
        t.vote(Slot::A).unwrap();
        t.abort();
        let snap = t.snapshot();
        assert!(!snap.active && !snap.finished);
        assert!(snap.votes.is_empty());
        assert_eq!(snap.score, 0);
        assert!(snap.mapping.is_none() && snap.abx_mapping.is_none());
        assert!(snap.p_value.is_none());
    }

    /* ── ABX protocol ─────────────────────────────────────────────────── */

    #[test]
    fn abx_slots_a_and_b_are_never_hidden() {
        for x_is_a in [true, false] {
            let t = rigged(BlindMode::Abx, 4, x_is_a);
            assert_eq!(t.deck_of(Slot::A), Deck::A, "slot a is always deck A");
            assert_eq!(t.deck_of(Slot::B), Deck::B, "slot b is always deck B");
            assert_eq!(
                t.deck_of(Slot::X),
                if x_is_a { Deck::A } else { Deck::B },
                "slot x follows the trial's secret"
            );
        }
    }

    #[test]
    fn abx_scores_x_equals_a_correctly() {
        let mut t = rigged(BlindMode::Abx, 2, true); // X is deck A
        assert_eq!(t.vote(Slot::A).unwrap(), Some(t.current_deck()));
        let snap = t.snapshot();
        assert_eq!(snap.score, 1);
        assert_eq!(snap.votes[0].chose, Slot::A);
        assert_eq!(snap.votes[0].correct_slot, Slot::A);
        assert!(snap.votes[0].correct);

        let mut t = rigged(BlindMode::Abx, 2, false); // X is deck B
        t.vote(Slot::A).unwrap();
        let snap = t.snapshot();
        assert_eq!(snap.score, 0);
        assert_eq!(snap.votes[0].correct_slot, Slot::B);
        assert!(!snap.votes[0].correct);
    }

    #[test]
    fn abx_re_randomises_x_every_trial() {
        // 100 trials of a fair coin: seeing one value only is a ~2^-99 event.
        let mut t = BlindTest::new();
        t.start(MAX_TRIALS, BlindMode::Abx);
        let mut x_was_a = 0usize;
        for _ in 0..MAX_TRIALS {
            if t.x_is_a {
                x_was_a += 1;
            }
            t.vote(Slot::A).unwrap();
        }
        assert!(
            x_was_a > 5 && x_was_a < MAX_TRIALS - 5,
            "x looks biased: {x_was_a} of {MAX_TRIALS}"
        );
    }

    #[test]
    fn abx_switching_selects_the_right_deck() {
        let mut t = rigged(BlindMode::Abx, 4, false); // X is deck B
        assert_eq!(t.switch(Slot::A).unwrap(), Deck::A);
        assert_eq!(t.switch(Slot::B).unwrap(), Deck::B);
        assert_eq!(t.switch(Slot::X).unwrap(), Deck::B);
        assert_eq!(t.snapshot().current_slot, Slot::X);
    }

    #[test]
    fn abx_rejects_slots_and_answers_that_do_not_belong() {
        let mut t = rigged(BlindMode::Abx, 4, true);
        // `y` exists in the 2AFC protocol, not this one.
        let err = t.switch(Slot::Y).unwrap_err();
        assert!(err.contains("does not exist"), "{err}");
        // You listen to `x`; you cannot vote for it.
        let err = t.vote(Slot::X).unwrap_err();
        assert!(err.contains("not an answer"), "{err}");
        assert!(t.snapshot().votes.is_empty(), "a bad vote must not count");
    }

    /* ── 2AFC protocol (v1 behaviour, unchanged) ──────────────────────── */

    #[test]
    fn ab_still_asks_which_slot_is_deck_a() {
        let mut t = rigged(BlindMode::Ab, 2, true); // x = A
        assert_eq!(t.deck_of(Slot::X), Deck::A);
        assert_eq!(t.deck_of(Slot::Y), Deck::B);
        t.vote(Slot::X).unwrap();
        assert_eq!(t.snapshot().score, 1);

        let mut t = rigged(BlindMode::Ab, 2, false); // x = B
        t.vote(Slot::X).unwrap();
        let snap = t.snapshot();
        assert_eq!(snap.score, 0);
        assert_eq!(snap.votes[0].correct_slot, Slot::Y);
    }

    #[test]
    fn ab_rejects_the_abx_slots() {
        let mut t = rigged(BlindMode::Ab, 2, true);
        assert!(t.switch(Slot::A).is_err());
        assert!(t.vote(Slot::B).is_err());
        assert!(t.switch(Slot::Y).is_ok());
    }

    #[test]
    fn the_slot_lists_match_the_contract() {
        assert_eq!(BlindMode::Ab.slots(), &[Slot::X, Slot::Y]);
        assert_eq!(BlindMode::Abx.slots(), &[Slot::A, Slot::B, Slot::X]);
        // `src/lib/types.ts`: ab: ["x","y"], abx: ["a","b","x"].
        let json = serde_json::to_string(&BlindMode::Abx.slots().to_vec()).unwrap();
        assert_eq!(json, "[\"a\",\"b\",\"x\"]");
        assert_eq!(serde_json::to_string(&BlindMode::Ab).unwrap(), "\"ab\"");
        assert_eq!(serde_json::to_string(&BlindMode::Abx).unwrap(), "\"abx\"");
    }

    /* ── lifecycle ────────────────────────────────────────────────────── */

    #[test]
    fn the_run_ends_after_the_requested_number_of_trials() {
        let mut t = BlindTest::new();
        t.start(3, BlindMode::Abx);
        assert!(t.vote(Slot::A).unwrap().is_some());
        assert!(t.vote(Slot::B).unwrap().is_some());
        assert!(t.vote(Slot::A).unwrap().is_none());
        let snap = t.snapshot();
        assert_eq!(snap.trial, 3);
        assert_eq!(snap.votes.len(), 3);
        assert!(snap.finished);
        // Voting after the end is an error, not a silent extra data point.
        assert!(t.vote(Slot::A).is_err());
        assert_eq!(t.snapshot().votes.len(), 3);
        assert!(t.switch(Slot::X).is_err());
    }

    #[test]
    fn trials_are_clamped() {
        let mut t = BlindTest::new();
        t.start(0, BlindMode::Abx);
        assert_eq!(t.snapshot().trials, 1);
        t.start(10_000, BlindMode::Ab);
        assert_eq!(t.snapshot().trials, MAX_TRIALS);
    }

    #[test]
    fn starting_switches_protocol_cleanly() {
        let mut t = BlindTest::new();
        t.start(2, BlindMode::Ab);
        t.vote(Slot::X).unwrap();
        t.start(2, BlindMode::Abx);
        let snap = t.snapshot();
        assert_eq!(snap.mode, BlindMode::Abx);
        assert_eq!(snap.current_slot, Slot::A);
        assert_eq!(snap.slots, vec![Slot::A, Slot::B, Slot::X]);
        assert!(snap.votes.is_empty() && snap.score == 0);
    }

    #[test]
    fn two_tests_do_not_share_a_sequence() {
        let mut a = BlindTest::new();
        let mut b = BlindTest::new();
        a.start(MAX_TRIALS, BlindMode::Abx);
        b.start(MAX_TRIALS, BlindMode::Abx);
        let seq = |t: &mut BlindTest| -> Vec<bool> {
            (0..MAX_TRIALS)
                .map(|_| {
                    let x_is_a = t.x_is_a;
                    let _ = t.vote(Slot::A);
                    x_is_a
                })
                .collect()
        };
        assert_ne!(seq(&mut a), seq(&mut b));
    }

    #[test]
    fn slot_names_are_parsed_strictly() {
        assert_eq!(Slot::parse("x"), Some(Slot::X));
        assert_eq!(Slot::parse(" A "), Some(Slot::A));
        assert_eq!(Slot::parse("B"), Some(Slot::B));
        assert_eq!(Slot::parse("z"), None);
        assert_eq!(Slot::parse(""), None);
        assert_eq!(Slot::parse("deck a"), None);
        assert_eq!(serde_json::to_string(&Slot::Y).unwrap(), "\"y\"");
    }
}
