//! The load pipeline: playlist entry → decoder → deck → engine.
//!
//! Three things here are subtle enough to deserve the comments they have:
//!
//! 1. The engine follows the **source** sample rate so deck A is never
//!    resampled, therefore [`AudioEngine::request_rate`] has to happen *before*
//!    `decode::open`, otherwise the decoder would convert to the old rate.
//! 2. Re-clocking the device invalidates the *other* deck: its PCM is stored at
//!    the previous rate, so playing it would transpose it. Anything left at the
//!    wrong rate is re-decoded — see [`resync_decks_to_engine_rate`].
//! 3. Because of (1) and (2), a load that fails *after* the device has been
//!    re-clocked would leave every deck holding PCM at a rate the engine no
//!    longer runs at. That path unwinds the rate change instead of returning an
//!    error over a transposed player, and while it is unwinding, every deck that
//!    does not match the engine is taken away from the audio callback rather
//!    than left playing — see [`rate_change_plan`] and [`apply_rate_agreement`].
//!    A rate change is therefore atomic as far as the listener is concerned:
//!    they hear the old rate, or the new one, or silence, never a transposition.
//!
//! [`AudioEngine::request_rate`]: onyx_core::engine::AudioEngine::request_rate

use std::path::Path;
use std::sync::Arc;

use onyx_core::deck::DeckSlot;
use onyx_core::decode::DecodeHandle;
use onyx_core::Deck;

use crate::playlist::{collect_sources, Collected, Source};
use crate::state::{AppState, LevelMatch};

/// How often a load watcher checks on its decode.
const WATCH_INTERVAL_MS: u64 = 40;

/// How long a decode may sit with a closed PCM buffer and no "finished" flag
/// before the watcher declares the decode thread dead. A healthy decode passes
/// through that state for microseconds; two seconds is only reached when the
/// thread really is gone.
const DECODER_GONE_GRACE_MS: u64 = 2_000;

#[derive(Clone, Copy, Debug)]
pub struct LoadOpts {
    /// Start playing as soon as the deck is armed.
    pub autoplay: bool,
    /// May this load re-clock the output device to the source rate?
    pub allow_rate_change: bool,
    /// Restart the shared playhead from zero.
    pub seek_to_zero: bool,
    /// Keep the level-match trim already applied to this deck.
    pub keep_trim: bool,
    /// This load re-decodes the material the deck already holds (a device or
    /// rate change), so it does not change *what* the listener is comparing.
    ///
    /// Only such a load may proceed while a blind test is running: everything
    /// else would put different audio behind a slot the listener has already
    /// voted on. See [`AppState::blind_guard`].
    pub same_material: bool,
}

impl LoadOpts {
    /// "One tap = play": load into a deck and play it from the top.
    pub fn play() -> LoadOpts {
        LoadOpts {
            autoplay: true,
            allow_rate_change: true,
            seek_to_zero: true,
            keep_trim: false,
            same_material: false,
        }
    }

    /// A/B assignment: arm the deck without disturbing the playhead.
    pub fn assign() -> LoadOpts {
        LoadOpts {
            autoplay: false,
            allow_rate_change: true,
            seek_to_zero: false,
            keep_trim: false,
            same_material: false,
        }
    }

    /// Re-decode of a deck that is already where it should be, at a new rate.
    fn redecode() -> LoadOpts {
        LoadOpts {
            autoplay: false,
            allow_rate_change: false,
            seek_to_zero: false,
            keep_trim: true,
            same_material: true,
        }
    }
}

/// Rate each deck's PCM is stored at, `None` when the deck is empty.
fn stored_rates(state: &AppState) -> [Option<u32>; 2] {
    let decks = state.decks.lock();
    [
        decks[0].handle.as_ref().map(|h| h.stored_rate),
        decks[1].handle.as_ref().map(|h| h.stored_rate),
    ]
}

/// What has to happen to one deck's engine slot when the engine runs at a given
/// rate. See [`rate_change_plan`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DeckAction {
    /// The deck already agrees with the engine (or there is nothing to judge).
    Leave,
    /// Take this deck's PCM away from the audio callback: it is at another rate,
    /// and playing it would transpose it.
    Silence,
    /// Give the engine this deck's PCM back: it matches the engine again.
    Attach,
}

/// Make the engine's decks agree with the engine's rate — as a pure function.
///
/// `stored[i]` is the rate deck `i`'s PCM is stored at (`None` = empty deck),
/// `attached[i]` whether the engine currently holds that PCM, and `rate` the
/// rate the device is running at now. The rule is one line: **a deck is audible
/// only while its PCM is at the engine's rate.**
///
/// This exists because the interesting cases are the ones no test with an output
/// device could reach. A load that re-clocks the device and then fails has to
/// put the rate back, and the device can refuse *that* too — at which point the
/// engine is at 96 kHz with two decks decoded at 44.1 kHz, and the old code's
/// answer was to re-decode them, which is right, but only after they had already
/// been playing a major sixth sharp and 2.18× too fast for as long as a decoder
/// open takes. In a mastering tool that is not a glitch, it is a lie: you cannot
/// hear "this is the wrong rate", you hear a different record. So the deck goes
/// quiet first and comes back when it is correct, and if it cannot be made
/// correct it is cleared with an error (see [`resync_decks_to_engine_rate`]).
///
/// A zero on either side means "we do not know" — no device rate yet, or a
/// decode that never reported one — and guessing is how a deck gets silenced for
/// no reason, so an unknown rate is left alone.
pub fn rate_change_plan(
    stored: [Option<u32>; 2],
    attached: [bool; 2],
    rate: u32,
) -> [DeckAction; 2] {
    let one = |stored: Option<u32>, attached: bool| match stored {
        // Nothing loaded: nothing to attach, and nothing that could sound.
        None => DeckAction::Leave,
        Some(stored) if stored == 0 || rate == 0 => DeckAction::Leave,
        Some(stored) if stored != rate => {
            if attached {
                DeckAction::Silence
            } else {
                DeckAction::Leave
            }
        }
        Some(_) => {
            if attached {
                DeckAction::Leave
            } else {
                DeckAction::Attach
            }
        }
    };
    [one(stored[0], attached[0]), one(stored[1], attached[1])]
}

/// Which decks hold PCM at a rate the engine no longer runs at.
///
/// `None` = nothing loaded on that deck, so nothing to fix. A zero rate on
/// either side means "we do not know", and re-decoding at an unknown rate would
/// be worse than leaving it alone. Pure so the rate-change logic — the most
/// intricate thing in this file, and impossible to exercise here without an
/// output device — can be tested directly.
pub fn stale_at_rate(stored: [Option<u32>; 2], engine_rate: u32) -> [bool; 2] {
    let one = |stored: Option<u32>| match stored {
        None => false,
        Some(rate) => rate != 0 && engine_rate != 0 && rate != engine_rate,
    };
    [one(stored[0]), one(stored[1])]
}

/// Carry out [`rate_change_plan`] against the real engine, and report which
/// decks are now silent because their PCM is at the wrong rate.
///
/// Called on every rate change, in both directions: after a successful
/// re-clocking (silencing whatever no longer matches) and after an unwind
/// (attaching whatever matches again). Idempotent — a second call with nothing
/// to do pushes nothing, so it cannot click.
fn apply_rate_agreement(state: &AppState, rate: u32) -> [bool; 2] {
    // The plan is computed under the deck lock and *executed* outside it: the
    // engine calls are queue pushes, but the rule in this file is that no lock
    // is held across one.
    let (plan, work) = {
        let mut decks = state.decks.lock();
        let stored = [0usize, 1].map(|i| decks[i].handle.as_ref().map(|h| h.stored_rate));
        let attached = [decks[0].attached, decks[1].attached];
        let plan = rate_change_plan(stored, attached, rate);
        let work = [0usize, 1].map(|i| match plan[i] {
            DeckAction::Silence => {
                decks[i].attached = false;
                None
            }
            DeckAction::Attach => {
                decks[i].attached = true;
                decks[i]
                    .handle
                    .as_ref()
                    .map(|h| (Arc::clone(&h.pcm), decks[i].trim_db))
            }
            DeckAction::Leave => None,
        });
        (plan, work)
    };
    for (index, action) in plan.iter().enumerate() {
        let deck = Deck::from_index(index);
        match action {
            DeckAction::Silence => {
                log::info!(
                    "deck {} is silent until it has been re-decoded at {rate} Hz",
                    deck_label(deck)
                );
                state.engine.clear_deck(deck);
            }
            DeckAction::Attach => {
                if let Some((pcm, trim)) = work[index].clone() {
                    log::info!("deck {} is audible again at {rate} Hz", deck_label(deck));
                    state.engine.load_deck(deck, pcm, trim);
                }
            }
            DeckAction::Leave => {}
        }
    }
    [0usize, 1].map(|i| plan[i] == DeckAction::Silence)
}

/// Recompute both level-match trims and push them to the engine (SPEC §10).
///
/// Sources of loudness, in order: the deck's own finished analysis, then the
/// playlist row — which the loudness cache of §8 fills in on probe, so a file
/// you have played before is matched immediately instead of after a decode. A
/// deck that is empty or not yet measured reports `ready: false` and unity, and
/// the watcher calls this again the moment the measurement lands.
pub fn recompute_trims(state: &AppState) {
    let enabled = state.ab.lock().level_match.enabled;
    let decks = state.deck_analysis();
    let loudness: [Option<f32>; 2] = {
        let playlist = state.playlist.lock();
        [0usize, 1].map(|i| {
            let (entry_id, analysis) = &decks[i];
            match analysis {
                Some(a) => Some(a.integrated_lufs),
                None => entry_id
                    .and_then(|id| playlist.get(id))
                    .and_then(|e| e.analysis)
                    .map(|a| a.integrated_lufs),
            }
        })
    };

    let matched = LevelMatch::compute(enabled, loudness[0], loudness[1]);
    {
        let mut decks = state.decks.lock();
        for deck in [Deck::A, Deck::B] {
            decks[deck.index()].trim_db = matched.trim.get(deck);
        }
    }
    // Outside the lock: `set_trim_db` is a queue push, but the rule is the rule.
    for deck in [Deck::A, Deck::B] {
        state.engine.set_trim_db(deck, matched.trim.get(deck));
    }
    let changed = {
        let mut ab = state.ab.lock();
        let changed = ab.level_match != matched;
        ab.level_match = matched;
        changed
    };
    // Only when something actually moved: this runs on every decode tick and an
    // unconditional `mark_state_dirty` would push a snapshot at 10 Hz for ever.
    if changed {
        state.mark_state_dirty();
    }
}

/// SPEC §2 rule 1/2: `replace` clears the playlist and plays the first file,
/// otherwise the files are appended and playback is left alone (unless nothing
/// is loaded, in which case the first new file starts).
pub fn open_paths(state: &Arc<AppState>, inputs: Vec<String>, replace: bool) -> Result<(), String> {
    // `replace` stops the engine and clears both decks *before* anything is
    // loaded, so the guard has to be here rather than in `load`: by the time a
    // load ran, the blind test would already have lost its material.
    if replace {
        state.blind_guard("Opening files")?;
    }
    // Expanding a `.zip` costs a decode-header probe per entry, so the
    // verifier is the same `safe_decode::probe` the playlist uses, with the
    // user's SoundFont in force (a `.mid` inside an archive is a real row).
    let options = state.decode_options();
    let collected = collect_sources(&inputs, &|path| {
        crate::safe_decode::probe_with(path, &options).is_ok()
    });
    let Collected {
        files,
        archives,
        warnings,
    } = collected;
    // Said before the early return: "that archive contains no audio files" is
    // the whole answer for an audio-free zip, and it must not be swallowed by
    // the generic refusal below.
    for warning in warnings {
        state.warn(warning);
    }
    if files.is_empty() {
        return Err("no supported audio files in that selection".to_string());
    }

    if replace {
        state.engine.stop();
        clear_deck(state, Deck::A);
        clear_deck(state, Deck::B);
        state.playlist.lock().clear();
        // The rows that pointed into the old temp trees are gone, so the trees
        // go too (SPEC §19). After the decks are cleared: a deck holding a
        // file from one of them is cancelled first.
        state.clear_archives();
    }
    // Adopted *before* the rows are added, so a failure between here and the
    // first load cannot leave rows pointing at a directory nobody owns.
    state.adopt_archives(archives);

    let ids: Vec<u64> = {
        let mut playlist = state.playlist.lock();
        files
            .iter()
            .map(|Source { path, archive }| playlist.add_source(path, archive.as_deref()))
            .collect()
    };
    // Metadata (and the cached loudness of §8) resolves in the background so
    // 200 files do not stall the UI.
    match state.probes.get() {
        Some(pool) => {
            for id in &ids {
                pool.enqueue(*id);
            }
        }
        None => log::warn!("no probe pool: playlist rows will fill in on load instead"),
    }
    // Rows appear immediately, unprobed.
    state.emit_state();

    let deck_a_loaded = state.deck_loaded(Deck::A);
    let count = ids.len();
    let first = ids.first().copied();
    if let Some(id) = first {
        if replace || !deck_a_loaded {
            // The message is the caller's to report: every path into here ends
            // at either a command (which returns it) or a toast, and reporting
            // it twice was how the same failure showed up as two toasts.
            load(state, Deck::A, id, LoadOpts::play())?;
        }
    }
    state.info(format!(
        "{count} file{} {}",
        if count == 1 { "" } else { "s" },
        if replace { "opened" } else { "added" }
    ));
    Ok(())
}

/// Load a playlist entry into a deck. `autoplay` implies "from the top".
pub fn load_entry_into_deck(
    state: &Arc<AppState>,
    deck: Deck,
    entry_id: u64,
    autoplay: bool,
) -> Result<(), String> {
    let opts = if autoplay {
        LoadOpts::play()
    } else {
        // Assigning to the deck we are listening to would otherwise leave the
        // playhead past the end of a shorter file.
        let mut opts = LoadOpts::assign();
        let was_loaded = state.deck_loaded(deck);
        opts.seek_to_zero = !was_loaded && state.engine.active_deck() == deck;
        opts
    };
    load(state, deck, entry_id, opts)
}

/// Play the neighbour of whatever deck A currently holds.
pub fn play_step(state: &Arc<AppState>, delta: isize, wrap: bool) -> Result<(), String> {
    let current = state.decks.lock()[Deck::A.index()].entry_id;
    let next = {
        let playlist = state.playlist.lock();
        playlist.step(current, delta, wrap)
    };
    match next {
        Some(id) => load(state, Deck::A, id, LoadOpts::play()),
        None => Ok(()),
    }
}

/// Re-arm the decks so that the engine rate and both decks' stored rates agree
/// again. Used when the output device changes and when "follow source sample
/// rate" is switched back on.
pub fn rearm_decks(state: &Arc<AppState>) {
    /* First, before anything that reads a file: by the time we are called the
    engine is already running at the new device's rate, and both decks still
    hold PCM at the old one. Re-arming deck A opens a decoder, which is a file
    read — long enough to hear. Anything that does not match the engine goes
    quiet now and comes back below, correct. */
    apply_rate_agreement(state, state.engine.engine_rate());
    let (a_id, b_id) = {
        let decks = state.decks.lock();
        (
            decks[Deck::A.index()].entry_id,
            decks[Deck::B.index()].entry_id,
        )
    };
    let mut opts = LoadOpts::assign();
    // Keep playing if we were playing, and never move the playhead.
    opts.autoplay = state.engine.shared().is_playing();
    opts.keep_trim = true;
    // Re-arming loads each deck with the material it already holds, so it is the
    // one load a running blind test must *not* refuse: refusing would leave the
    // deck decoded at a rate the engine no longer runs at, i.e. transposed.
    opts.same_material = true;

    // Deck A decides the rate, so re-arm it first.
    if let Some(id) = a_id {
        if let Err(e) = load(state, Deck::A, id, opts) {
            state.warn(format!("could not re-arm deck A: {e}"));
        }
    } else if let Some(id) = b_id {
        if let Err(e) = load(state, Deck::B, id, opts) {
            state.warn(format!("could not re-arm deck B: {e}"));
        }
    }
    // After all that, no deck may still hold PCM at another rate.
    resync_decks_to_engine_rate(state);
}

/// Detach a deck: cancel its decode, free the engine slot, clear the badge.
pub fn clear_deck(state: &AppState, deck: Deck) {
    let previous = {
        let mut decks = state.decks.lock();
        std::mem::take(&mut decks[deck.index()])
    };
    if let Some(handle) = previous.handle {
        // Without this the decode thread keeps a whole file's worth of memory
        // alive and competes for IO with whatever the user is waiting for.
        handle.status.cancel();
    }
    state.engine.clear_deck(deck);
    state.playlist.lock().assign_deck(deck, None);
    // A blind test with an empty slot is not a blind test: "which one can you
    // hear at all" is not the question, so stop it rather than let the listener
    // record meaningless votes. A *finished* test keeps its reveal.
    let aborted = {
        let mut blind = state.blind.lock();
        let running = blind.is_active();
        if running {
            blind.abort();
        }
        running
    };
    if aborted {
        state.engine.select_deck(Deck::A);
        state.warn(format!(
            "Blind test stopped: deck {} was cleared",
            deck_label(deck)
        ));
    }
    // The surviving deck must not keep an attenuation that was calculated
    // against a track that is no longer loaded.
    recompute_trims(state);
    state.mark_state_dirty();
}

/// The real thing. Every public entry point funnels through here.
fn load(state: &Arc<AppState>, deck: Deck, entry_id: u64, opts: LoadOpts) -> Result<(), String> {
    // The one choke point every load funnels through, which makes it the right
    // place for the blind-test guard: the IPC commands, the OS "open with"
    // handler and the end-of-track auto-advance all arrive here.
    if !opts.same_material {
        state.blind_guard(&format!("Loading a track into deck {}", deck_label(deck)))?;
    }

    // -- 1. where is it, and what rate is it? -------------------------------
    let (path, cached_rate, resolved) = {
        let playlist = state.playlist.lock();
        let entry = playlist
            .get(entry_id)
            .ok_or_else(|| "that track is no longer in the playlist".to_string())?;
        (
            entry.path_buf(),
            entry.sample_rate,
            entry.probed && !entry.missing,
        )
    };

    let mut source_rate = cached_rate;
    if !resolved || source_rate == 0 {
        // The background probe has not reached this entry yet. A header parse is
        // sub-millisecond, so doing it inline is cheaper than waiting.
        match crate::safe_decode::probe_with(&path, &state.decode_options()) {
            Ok(info) => {
                source_rate = info.sample_rate;
                if let Some(entry) = state.playlist.lock().get_mut(entry_id) {
                    entry.apply_probe(&info);
                }
                // A file we have measured before shows its loudness now, and
                // level matching (if on) can use it before the decode ends.
                // The key includes the SoundFont for a MIDI render and the rate
                // the decode will run at, so neither a bank change nor a device
                // at another rate hands back a measurement that is not ours.
                crate::playlist::warm_analysis_from_cache(
                    state,
                    entry_id,
                    &path,
                    info.render_key.as_deref(),
                    info.sample_rate,
                );
            }
            Err(e) => return Err(fail(state, entry_id, &path, e)),
        }
    }

    let engine = &state.engine;
    let previous_rate = engine.engine_rate();

    // -- 2. re-clock the device *before* decoding ---------------------------
    let target_rate = if opts.allow_rate_change {
        match engine.request_rate(source_rate) {
            Ok(rate) => rate,
            Err(e) => {
                // The file still plays, resampled to the current rate: degraded
                // but continuing. `state.warn` logs it at `warn` for us.
                state.warn(format!(
                    "output device would not run at {source_rate} Hz ({e}); staying at {previous_rate} Hz"
                ));
                engine.engine_rate()
            }
        }
    } else {
        previous_rate
    };
    let rate_changed = target_rate != previous_rate;
    if rate_changed {
        // The device is clocked at `target_rate` from here on, and *everything*
        // already loaded — including this deck's outgoing PCM — is at
        // `previous_rate` until it has been re-decoded. Silence it now rather
        // than at step 5: a decoder open is a file read, and until this deck is
        // installed the audio callback would be mixing the old rate's samples at
        // the new rate's clock.
        apply_rate_agreement(state, target_rate);
    }

    // -- 3. open the decoder (returns after the header parse) ---------------
    //
    // The budget is the smaller of what the user allowed and what this file
    // could plausibly hold: see `plausible_budget`. `safe_decode::open` rather
    // than `decode::open`, because the header parse happens on this thread and
    // symphonia panics on some malformed input.
    let file_bytes = std::fs::metadata(&path)
        .map(|m| m.len())
        .unwrap_or(u64::MAX);
    let budget = crate::safe_decode::plausible_budget(state.budget_bytes(), file_bytes);
    let handle =
        match crate::safe_decode::open_with(&path, target_rate, budget, &state.decode_options()) {
            Ok(h) => h,
            Err(e) => {
                let message = fail(state, entry_id, &path, e);
                if rate_changed {
                    // The device is now clocked for a file we could not open, so
                    // every loaded deck holds PCM at the wrong rate. Returning here
                    // without unwinding that would leave the player transposed.
                    unwind_rate_change(state, previous_rate, target_rate);
                }
                return Err(message);
            }
        };
    if handle.status.is_truncated() {
        let minutes = handle.pcm.capacity_frames() as f64 / target_rate.max(1) as f64 / 60.0;
        let budget_mib = budget / (1024 * 1024);
        state.warn(format!(
            "{} exceeds the {budget_mib} MiB deck budget — only the first {minutes:.0} min is \
             loaded on deck {}",
            handle.info.file_name,
            deck_label(deck)
        ));
    }

    // -- 4. install it, cancelling whatever this deck was doing -------------
    let file_name = handle.info.file_name.clone();
    let trim = if opts.keep_trim {
        state.ab.lock().level_match.trim.get(deck)
    } else {
        0.0
    };
    let previous = {
        let mut decks = state.decks.lock();
        std::mem::replace(
            &mut decks[deck.index()],
            DeckSlot {
                handle: Some(handle.clone()),
                trim_db: trim,
                entry_id: Some(entry_id),
                // `engine.load_deck` below is what makes this true; the flag and
                // the engine's slot are set in the same breath so nothing can
                // read one without the other (see `apply_rate_agreement`).
                attached: true,
            },
        )
    };
    if let Some(old) = previous.handle {
        // The old decode thread would otherwise keep a whole file's worth of
        // memory alive and compete for IO with the one the user is waiting for.
        old.status.cancel();
    }
    state.playlist.lock().assign_deck(deck, Some(entry_id));

    engine.load_deck(deck, handle.pcm.clone(), trim);
    if opts.seek_to_zero {
        engine.seek_frames(0);
    }
    // Nothing else has to be re-derived after a rate change: everything the
    // realtime core holds in *frames* (playhead, loop bounds, A/B offset,
    // crossfade and every ramp) is re-expressed by `RtCore::set_rate` while the
    // stream is down. The host copies are all in seconds or milliseconds. This
    // used to re-seek to the position captured *before* the device rebuild,
    // which rewound playback by however long the rebuild took.
    if opts.autoplay {
        engine.play();
    }
    // A cached measurement for the *newly loaded* file may already be sitting in
    // the playlist row, so the trims can be right before the decode finishes.
    recompute_trims(state);
    state.emit_state();

    spawn_watcher(Arc::clone(state), deck, entry_id, handle);

    // -- 5. any other deck is now decoded at the wrong rate ----------------
    if rate_changed {
        resync_decks_to_engine_rate(state);
    }
    // A load is exactly the kind of lifecycle event a bug report needs: which
    // deck, which file, which rate. The file name only — never the directory.
    log::info!(
        "deck {} loaded \"{}\" ({} Hz source, engine at {target_rate} Hz{})",
        deck_label(deck),
        file_name,
        source_rate,
        if rate_changed {
            format!(", re-clocked from {previous_rate} Hz")
        } else {
            String::new()
        }
    );
    Ok(())
}

/// A load re-clocked the device and then failed. Put the rate back if the
/// device will still take it; otherwise re-decode what is loaded at whatever
/// rate we ended up with. Either way no deck is left playing transposed.
///
/// Every branch ends in [`apply_rate_agreement`] or in
/// [`resync_decks_to_engine_rate`] (which begins with it), because by the time we
/// get here the decks have already been silenced by step 2 of `load`: a
/// successful revert has to give them *back*, and a failed one has to keep them
/// quiet until they have been re-decoded. This is the failure path that used to
/// leave deck B audible at the old rate on a device now clocked at the new one —
/// wrong pitch and wrong speed, with nothing on screen to say so.
fn unwind_rate_change(state: &Arc<AppState>, previous_rate: u32, target_rate: u32) {
    match state.engine.request_rate(previous_rate) {
        Ok(rate) if rate == previous_rate => {
            log::info!("load failed at {target_rate} Hz; device restored to {previous_rate} Hz");
            // The decks are correct again, so hand their PCM back to the engine.
            apply_rate_agreement(state, previous_rate);
        }
        Ok(rate) => {
            log::warn!("could not restore {previous_rate} Hz after a failed load (got {rate} Hz)");
            state.error(format!(
                "the output device would not go back to {previous_rate} Hz and is running at \
                 {rate} Hz; anything loaded is being re-decoded and is silent until it is"
            ));
            resync_decks_to_engine_rate(state);
        }
        Err(e) => {
            log::warn!("could not restore {previous_rate} Hz after a failed load ({e})");
            state.error(format!(
                "the output device would not go back to {previous_rate} Hz ({e}); anything loaded \
                 is being re-decoded and is silent until it is"
            ));
            resync_decks_to_engine_rate(state);
        }
    }
}

/// Silence, then re-decode, every deck whose PCM is stored at a rate the engine
/// no longer runs at. Idempotent, and safe to call after any rate change.
///
/// The silencing comes first and is not conditional on the re-decode working: a
/// deck that cannot be re-decoded is cleared with an error, and a deck that is
/// waiting for one is quiet. Nothing between the two states is audible.
pub fn resync_decks_to_engine_rate(state: &Arc<AppState>) {
    let rate = state.engine.engine_rate();
    // Takes the stale decks away from the audio callback and gives back any that
    // are correct again; the `[bool; 2]` it returns is "silenced, needs a decode".
    let silenced = apply_rate_agreement(state, rate);
    let stale = stale_at_rate(stored_rates(state), rate);
    for (index, stale) in stale.iter().enumerate() {
        if !stale && !silenced[index] {
            continue;
        }
        redecode(state, Deck::from_index(index), rate);
    }
}

/// Decode one deck again at `target_rate`, or clear it if that is impossible.
fn redecode(state: &Arc<AppState>, deck: Deck, target_rate: u32) {
    let entry_id = state.decks.lock()[deck.index()].entry_id;
    match entry_id {
        Some(id) => {
            if let Err(e) = load(state, deck, id, LoadOpts::redecode()) {
                state.warn(format!(
                    "deck {} could not be re-decoded at {target_rate} Hz: {e}",
                    deck_label(deck)
                ));
                clear_deck(state, deck);
            }
        }
        None => {
            // A handle with no playlist row should not be reachable; clearing is
            // still better than leaving audio at the wrong rate playing.
            log::warn!(
                "deck {} holds PCM at the wrong rate with no playlist row; clearing",
                deck_label(deck)
            );
            clear_deck(state, deck);
        }
    }
}

/// Watch one decode to completion, then record the analysis in the loudness
/// cache, refresh the level-match trims and push a snapshot.
fn spawn_watcher(state: Arc<AppState>, deck: Deck, entry_id: u64, handle: DecodeHandle) {
    let reporter = Arc::clone(&state);
    let spawned = std::thread::Builder::new()
        .name(format!("onyx-load-{}", deck_label(deck)))
        .spawn(move || {
            let mut closed_ticks = 0u32;
            loop {
                if handle.status.is_cancelled() || !is_current(&state, deck, &handle) {
                    return;
                }
                if handle.status.is_finished() {
                    break;
                }
                // The decode thread is detached and it runs third-party codec
                // code, so it can die without ever setting `finished` — a panic
                // unwinding out of symphonia would do it. When it does,
                // `PcmWriter::drop` still closes the buffer, so "the buffer is
                // complete but the decoder never reported finished" is the
                // signal that the thread is gone. It is also, briefly, what a
                // *healthy* decode looks like between `writer.finish()` and the
                // status store, hence the grace period. Without this the
                // watcher span at 25 Hz for ever, the deck sat on a partial
                // decode, and nothing ever told the user.
                if handle.pcm.is_complete() {
                    closed_ticks += 1;
                    if closed_ticks as u64 * WATCH_INTERVAL_MS >= DECODER_GONE_GRACE_MS {
                        log::error!(
                            "the decoder for \"{}\" stopped without finishing",
                            handle.info.file_name
                        );
                        state.error(format!(
                            "{}: the decoder stopped unexpectedly — only the first {:.1} s \
                             was read",
                            handle.info.file_name,
                            handle.pcm.frames_ready() as f64 / handle.stored_rate.max(1) as f64
                        ));
                        state.emit_state();
                        return;
                    }
                }
                std::thread::sleep(std::time::Duration::from_millis(WATCH_INTERVAL_MS));
            }
            if handle.status.is_cancelled() || !is_current(&state, deck, &handle) {
                return;
            }
            if let Some(error) = handle.status.error() {
                state.error(format!("{}: {error}", handle.info.file_name));
                state.emit_state();
                return;
            }
            if let Some(analysis) = handle.status.analysis() {
                if let Some(entry) = state.playlist.lock().get_mut(entry_id) {
                    // The decode is authoritative: overwrite whatever the cache
                    // said, so a schema change actually takes effect.
                    entry.analysis = Some(analysis);
                }
                // Only truncated-free decodes describe the whole file, and a
                // partial measurement must never be cached as if it were.
                if !handle.status.is_truncated() {
                    // SPEC §18: for a MIDI render the key carries the
                    // SoundFont's identity, so the measurement belongs to the
                    // bank it was made with. `stored_rate` — the rate this
                    // decode actually ran at, not the one that was asked for —
                    // keys it to the PCM that was measured.
                    state.cache.store(
                        Path::new(&handle.info.path),
                        handle.info.render_key.as_deref(),
                        handle.stored_rate,
                        &analysis,
                    );
                }
            }
            recompute_trims(&state);
            state.emit_state();
        });
    if let Err(e) = spawned {
        // No watcher means no analysis and no trim refresh for this deck, which
        // is degraded but not broken; the audio is already playing.
        log::warn!("could not spawn load watcher: {e}");
        reporter.warn(format!(
            "deck {} will not report its loudness (out of threads)",
            deck_label(deck)
        ));
    }
}

/// Is `handle` still the decode this deck is playing?
fn is_current(state: &AppState, deck: Deck, handle: &DecodeHandle) -> bool {
    state.decks.lock()[deck.index()]
        .handle
        .as_ref()
        .map(|h| Arc::ptr_eq(&h.pcm, &handle.pcm))
        .unwrap_or(false)
}

/// Mark an entry as unreadable and hand the message back.
///
/// Deliberately silent: the caller reports it exactly once, either as the
/// `Err` of a command or as its own toast. Doing both is how one unreadable
/// file produced two identical error toasts.
fn fail(state: &AppState, entry_id: u64, path: &Path, error: String) -> String {
    if let Some(entry) = state.playlist.lock().get_mut(entry_id) {
        entry.mark_missing();
    }
    let name = path
        .file_name()
        .map(|f| f.to_string_lossy().to_string())
        .unwrap_or_else(|| path.to_string_lossy().to_string());
    // The user asked for this file and did not get it: that is an error, even
    // though the app carries on. The file name is enough for a bug report; the
    // directory it came from is nobody's business above `debug`.
    log::error!("could not load \"{name}\": {error}");
    log::debug!("failed load: {}", path.display());
    state.emit_state();
    format!("{name}: {error}")
}

pub fn deck_label(deck: Deck) -> &'static str {
    match deck {
        Deck::A => "A",
        Deck::B => "B",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn load_opts_encode_the_behaviour_rules() {
        // "One tap = play" restarts from the top and may re-clock the device.
        let play = LoadOpts::play();
        assert!(play.autoplay && play.seek_to_zero && play.allow_rate_change);
        // Assigning a deck for A/B must not move the playhead.
        let assign = LoadOpts::assign();
        assert!(!assign.autoplay && !assign.seek_to_zero);
        // A re-decode after a rate change keeps position *and* trim, and must
        // never itself re-clock the device — that would recurse.
        let redecode = LoadOpts::redecode();
        assert!(!redecode.allow_rate_change && redecode.keep_trim && !redecode.seek_to_zero);
        // Blind-test integrity: only a re-decode of the material a deck already
        // holds may run during a test. `play`/`assign` change what is behind a
        // slot, so `load` refuses them (see `AppState::blind_guard`).
        assert!(
            redecode.same_material,
            "a re-decode must survive a blind test, or a device change clears the decks"
        );
        assert!(!play.same_material && !assign.same_material);
    }

    /* ── the sample-rate-change path (SPEC §9.6) ──────────────────────── */

    #[test]
    fn a_deck_at_another_rate_is_stale() {
        // Loading a 96 kHz file into A re-clocks the device, so B (44.1 kHz)
        // has to be re-decoded or it would play transposed.
        assert_eq!(
            stale_at_rate([Some(96_000), Some(44_100)], 96_000),
            [false, true]
        );
        assert_eq!(
            stale_at_rate([Some(44_100), Some(96_000)], 96_000),
            [true, false]
        );
        // Both wrong: a device change can do this, and both must be fixed.
        assert_eq!(
            stale_at_rate([Some(44_100), Some(44_100)], 48_000),
            [true, true]
        );
    }

    #[test]
    fn nothing_is_stale_when_the_rates_agree() {
        assert_eq!(
            stale_at_rate([Some(48_000), Some(48_000)], 48_000),
            [false, false]
        );
        // An empty deck has nothing to re-decode.
        assert_eq!(stale_at_rate([None, None], 48_000), [false, false]);
        assert_eq!(stale_at_rate([None, Some(44_100)], 48_000), [false, true]);
        assert_eq!(stale_at_rate([Some(44_100), None], 48_000), [true, false]);
    }

    #[test]
    fn an_unknown_rate_is_left_alone() {
        // Re-decoding "at 0 Hz" would be worse than the mismatch: a rate we do
        // not know is a reason to do nothing, not a reason to guess.
        assert_eq!(stale_at_rate([Some(0), Some(0)], 48_000), [false, false]);
        assert_eq!(
            stale_at_rate([Some(44_100), Some(48_000)], 0),
            [false, false]
        );
    }

    /* ── the rate-change *unwind*, which is where it used to go wrong ─────
    No output device exists here (and none exists on CI), so the sequence is
    driven through `rate_change_plan`, which is the whole decision. */

    use DeckAction::{Attach, Leave, Silence};

    #[test]
    fn a_deck_at_the_wrong_rate_is_taken_off_the_output() {
        // Loading a 96 kHz file into A re-clocks the device: B, decoded at
        // 44.1 kHz and still attached, has to stop sounding.
        assert_eq!(
            rate_change_plan([Some(96_000), Some(44_100)], [true, true], 96_000),
            [Leave, Silence]
        );
        // A device change can leave both wrong.
        assert_eq!(
            rate_change_plan([Some(44_100), Some(44_100)], [true, true], 48_000),
            [Silence, Silence]
        );
        // Already silent: nothing to push, so a second call cannot click.
        assert_eq!(
            rate_change_plan([Some(44_100), Some(44_100)], [false, false], 48_000),
            [Leave, Leave]
        );
        // Agreeing and attached is the ordinary case, and is left alone.
        assert_eq!(
            rate_change_plan([Some(48_000), Some(48_000)], [true, true], 48_000),
            [Leave, Leave]
        );
        // An empty deck, and an unknown rate on either side, are not judged.
        assert_eq!(
            rate_change_plan([None, None], [false, false], 48_000),
            [Leave, Leave]
        );
        assert_eq!(
            rate_change_plan([Some(0), Some(44_100)], [true, true], 0),
            [Leave, Leave]
        );
    }

    #[test]
    fn a_deck_that_matches_again_is_given_back() {
        // The successful unwind: silenced at 96 kHz, and the device took
        // 44.1 kHz back, so the PCM in hand is correct and must be re-attached
        // rather than left silent (or, worse, re-decoded for nothing).
        assert_eq!(
            rate_change_plan([Some(44_100), Some(44_100)], [false, false], 44_100),
            [Attach, Attach]
        );
    }

    /// The finding this was written for: a load re-clocks the device to a new
    /// rate, fails, and the device then refuses to go back. The engine ends up
    /// at a rate no deck holds PCM for, and deck B used to keep playing —
    /// audibly transposed, with nothing on screen to say why.
    ///
    /// Walked step by step, asserting the invariant after each: **no deck is
    /// attached whose stored rate differs from the engine's.**
    #[test]
    fn a_failed_revert_never_leaves_a_deck_playing_at_the_wrong_rate() {
        let mut stored = [Some(44_100), Some(44_100)];
        let mut attached = [true, true];
        // Both decks are correct and audible: the engine and the PCM agree.
        no_deck_is_transposed(stored, attached, 44_100);

        // 1. `load` asks for 96 kHz for a new file on deck A and gets it.
        let engine_rate = 96_000;
        let plan = rate_change_plan(stored, attached, engine_rate);
        assert_eq!(plan, [Silence, Silence], "both decks are at 44.1 kHz");
        apply(&mut attached, plan);
        assert_eq!(attached, [false, false]);
        no_deck_is_transposed(stored, attached, engine_rate);

        // 2. the decode fails, and the device refuses to go back to 44.1 kHz,
        //    so the engine is stuck at 96 kHz. `unwind_rate_change` → `resync`.
        let plan = rate_change_plan(stored, attached, engine_rate);
        assert_eq!(plan, [Leave, Leave], "nothing may be re-attached at 96 kHz");
        apply(&mut attached, plan);
        assert_eq!(
            stale_at_rate(stored, engine_rate),
            [true, true],
            "both decks still have to be re-decoded"
        );
        no_deck_is_transposed(stored, attached, engine_rate);

        // 3. deck A's file cannot be re-decoded, so `redecode` clears it; deck
        //    B's can, and comes back at 96 kHz and audible.
        stored[0] = None;
        stored[1] = Some(96_000);
        let plan = rate_change_plan(stored, attached, engine_rate);
        assert_eq!(plan, [Leave, Attach]);
        apply(&mut attached, plan);
        assert_eq!(attached, [false, true]);
        no_deck_is_transposed(stored, attached, engine_rate);
        assert_eq!(stale_at_rate(stored, engine_rate), [false, false]);
    }

    /// Every combination of two decks, two attach states and a set of plausible
    /// rates: applying the plan must always end with the invariant true, and one
    /// pass must be enough (the second pass has nothing left to do).
    #[test]
    fn the_plan_reaches_agreement_in_one_pass_from_any_state() {
        let rates = [None, Some(0), Some(44_100), Some(48_000), Some(96_000)];
        for a in rates {
            for b in rates {
                for attached_a in [false, true] {
                    for attached_b in [false, true] {
                        for engine_rate in [0, 44_100, 48_000, 96_000] {
                            let stored = [a, b];
                            let mut attached = [attached_a, attached_b];
                            let plan = rate_change_plan(stored, attached, engine_rate);
                            apply(&mut attached, plan);
                            no_deck_is_transposed(stored, attached, engine_rate);
                            assert_eq!(
                                rate_change_plan(stored, attached, engine_rate),
                                [Leave, Leave],
                                "not settled: {stored:?} {attached:?} at {engine_rate}"
                            );
                        }
                    }
                }
            }
        }
    }

    /// What `apply_rate_agreement` does to the flags, without an engine.
    fn apply(attached: &mut [bool; 2], plan: [DeckAction; 2]) {
        for (flag, action) in attached.iter_mut().zip(plan) {
            match action {
                DeckAction::Silence => *flag = false,
                DeckAction::Attach => *flag = true,
                DeckAction::Leave => {}
            }
        }
    }

    /// The invariant the whole mechanism exists for.
    fn no_deck_is_transposed(stored: [Option<u32>; 2], attached: [bool; 2], engine_rate: u32) {
        for i in 0..2 {
            if let (Some(rate), true) = (stored[i], attached[i]) {
                // A zero on either side is "unknown", which the plan leaves
                // alone by design; it is also never audible-and-wrong, because
                // an engine at 0 Hz is not running a stream.
                if rate == 0 || engine_rate == 0 {
                    continue;
                }
                assert_eq!(
                    rate, engine_rate,
                    "deck {i} is attached at {rate} Hz while the engine runs at {engine_rate} Hz"
                );
            }
        }
    }
}
