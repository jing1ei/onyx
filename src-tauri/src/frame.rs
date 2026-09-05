//! The 60 Hz pump.
//!
//! One thread does four jobs that all have to happen off the audio callback
//! and off the webview's main thread:
//!
//! * emit `onyx://frame` (transport + meters + per-deck decode progress),
//! * notice `take_ended()` and auto-advance the playlist,
//! * emit a debounced `onyx://state` when decoding progress changes something
//!   the snapshot carries (progress, waveform buckets, analysis),
//! * drain the engine's real-time fault counters and *log* them, which the
//!   audio callback is not allowed to do itself (see [`FaultLog`]).
//!
//! Frames are skipped entirely while every window is hidden or minimised:
//! there is no point serialising 60 payloads a second for nobody to draw. They
//! are emitted app-wide, so the detached EQ window (SPEC §12) receives the same
//! stream as the main one without a second pump.

use std::sync::Arc;
use std::time::{Duration, Instant};

use onyx_core::engine::{RtFaults, StreamFault};
use onyx_core::Deck;
use tauri::{AppHandle, Emitter, Manager};

use crate::loader;
use crate::state::{AppState, FrameDeck, FramePayload, FRAME_EVENT};

/// 60 Hz.
const FRAME_INTERVAL: Duration = Duration::from_micros(16_667);
/// `onyx://state` is never pushed faster than this (10 Hz).
const STATE_MIN_INTERVAL: Duration = Duration::from_millis(100);
/// Window visibility is asked for at most twice a second — it is a round trip
/// to the window system's main thread, so it must not happen per frame.
const VISIBILITY_POLL: Duration = Duration::from_millis(500);
/// How often the settings and the loudness cache are considered for a write.
/// The writer debounces on top of this (SPEC §8), so the disk is touched at
/// most once every couple of seconds even while a fader is being dragged.
const PERSIST_INTERVAL: Duration = Duration::from_secs(2);
/// How often the real-time fault counters are drained. Four times a second is
/// far more often than a human notices and costs two relaxed atomic swaps.
const FAULT_DRAIN_INTERVAL: Duration = Duration::from_millis(250);
/// A fault that keeps happening is reported at most this often, with a count.
const FAULT_REPORT_INTERVAL: Duration = Duration::from_secs(5);
/// Decode progress change that is worth a snapshot (2 %).
const PROGRESS_EPSILON: f32 = 0.02;
/// Waveform bucket growth that is worth a snapshot.
const BUCKET_EPSILON: usize = 24;

#[derive(Clone, Copy, Default)]
struct DeckMark {
    fraction: f32,
    buckets: usize,
    analysis_ready: bool,
}

impl DeckMark {
    /// Has this deck moved enough that the UI has to be told?
    fn material_change(&self, next: &FrameDeck) -> bool {
        (next.decoded_fraction - self.fraction).abs() >= PROGRESS_EPSILON
            || next.waveform_buckets.abs_diff(self.buckets) >= BUCKET_EPSILON
            || next.analysis_ready != self.analysis_ready
            // The transition to "fully decoded" flips `decoded`/`truncated` in
            // the snapshot even if the fraction barely moved.
            || (next.decoded_fraction >= 1.0 && self.fraction < 1.0)
    }

    fn update(&mut self, next: &FrameDeck) {
        self.fraction = next.decoded_fraction;
        self.buckets = next.waveform_buckets;
        self.analysis_ready = next.analysis_ready;
    }
}

/// Start the pump. Returns immediately; the thread lives as long as the app.
pub fn spawn(state: Arc<AppState>, app: AppHandle) {
    let spawned = std::thread::Builder::new()
        .name("onyx-frame".into())
        .spawn(move || run(state, app));
    if let Err(e) = spawned {
        // No pump means no meters, no playhead and no auto-advance: the window
        // would look frozen even though audio is fine.
        log::error!("could not start the frame thread: {e}");
    }
}

fn run(state: Arc<AppState>, app: AppHandle) {
    let mut deadline = Instant::now();
    let mut last_state = Instant::now() - STATE_MIN_INTERVAL;
    let mut last_persist = Instant::now();
    let mut last_visibility = Instant::now() - VISIBILITY_POLL;
    let mut last_faults = Instant::now();
    let mut faults = FaultLog::default();
    let mut visible = true;
    let mut marks = [DeckMark::default(); 2];
    let mut state_pending = false;

    loop {
        deadline += FRAME_INTERVAL;
        let now = Instant::now();
        if let Some(wait) = deadline.checked_duration_since(now) {
            std::thread::sleep(wait);
        } else if now.duration_since(deadline) > Duration::from_millis(250) {
            // The machine went to sleep, or we were descheduled for a long
            // time: resynchronise instead of spinning to catch up.
            deadline = now;
        }

        // The window is gone (app quitting): stop pumping.
        if app.webview_windows().is_empty() {
            state.persist_flush();
            return;
        }

        if last_visibility.elapsed() >= VISIBILITY_POLL {
            visible = window_visible(&app);
            last_visibility = Instant::now();
        }

        // Diagnostics the audio callback could only count, never log (it may
        // not format, allocate or take the logger's lock). This is the only
        // place they are drained, and it is deliberately not tied to window
        // visibility: a device that dies while the window is hidden still has
        // to appear in the log.
        if last_faults.elapsed() >= FAULT_DRAIN_INTERVAL {
            let now = Instant::now();
            last_faults = now;
            if let Some(report) = faults.note(state.engine.take_faults(), now) {
                report_faults(&state, &report);
            }
        }

        // End of track → next entry (the engine handles whole-file looping
        // itself and does not report an end in that case).
        if state.engine.shared().take_ended() {
            handle_ended(&state);
        }

        let (deck_a, deck_b) = state.frame_decks();

        if visible {
            let payload = FramePayload {
                transport: state.transport(),
                meters: state.engine.meters(),
                deck_a,
                deck_b,
                // Read from the engine so the EQ window's sweep can light the
                // main window's badge; neither window can see the other's
                // JavaScript.
                audition: state.audition_frame(),
            };
            if app.emit(FRAME_EVENT, payload).is_err() {
                // The webview is tearing down; nothing to do but stop.
                return;
            }
        }

        if mirror_finished_analysis(&state) {
            // Only reachable when a load watcher could not be spawned; the
            // watcher normally does this (and the caching) itself.
            loader::recompute_trims(&state);
            state.mark_state_dirty();
        }

        for (mark, next) in marks.iter_mut().zip([&deck_a, &deck_b]) {
            if mark.material_change(next) {
                mark.update(next);
                state_pending = true;
            }
        }
        if state.take_state_dirty() {
            state_pending = true;
        }
        if state_pending && last_state.elapsed() >= STATE_MIN_INTERVAL {
            state.emit_state();
            state_pending = false;
            last_state = Instant::now();
        }

        if last_persist.elapsed() >= PERSIST_INTERVAL {
            // The frame thread is the only writer of the settings and the
            // loudness cache: never the audio callback, never a decode thread,
            // and never a command (SPEC §8).
            state.remember_settings();
            state.persist_tick();
            last_persist = Instant::now();
        }
    }
}

/// One coalesced report about the real-time path.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct FaultReport {
    dropped_commands: u64,
    stream_faults: u64,
    /// The most recent kind of stream fault in this report, if any.
    last_stream_fault: Option<StreamFault>,
    /// The interval the counts cover. Zero on the first report of an episode,
    /// which goes out immediately rather than being hidden for five seconds.
    over: Duration,
    /// True when nothing had been reported for a while before this one. Used to
    /// toast the user once per episode instead of once per report.
    first_of_episode: bool,
}

/// Coalescer for the engine's real-time fault counters.
///
/// The engine can only *count* faults (`log::` on the audio thread formats,
/// allocates and locks), so the counts have to be turned into log lines here.
/// The rule that matters: a fault that keeps firing — a permanently full command
/// queue, a device erroring on every callback — must not be able to write a line
/// per drain. The first occurrence is reported at once, everything after it is
/// accumulated and reported as a *count over an interval*.
#[derive(Debug, Default)]
struct FaultLog {
    dropped_commands: u64,
    stream_faults: u64,
    last_stream_fault: Option<StreamFault>,
    /// When the current quiet-down window opened; `None` = nothing pending.
    window: Option<Instant>,
}

impl FaultLog {
    fn pending(&self) -> bool {
        self.dropped_commands > 0 || self.stream_faults > 0
    }

    fn add(&mut self, faults: RtFaults) {
        self.dropped_commands += u64::from(faults.dropped_commands);
        self.stream_faults += u64::from(faults.stream_faults);
        if faults.last_stream_fault.is_some() {
            self.last_stream_fault = faults.last_stream_fault;
        }
    }

    /// Empty the accumulator into a report.
    fn take(&mut self, over: Duration, first_of_episode: bool) -> FaultReport {
        FaultReport {
            dropped_commands: std::mem::take(&mut self.dropped_commands),
            stream_faults: std::mem::take(&mut self.stream_faults),
            last_stream_fault: self.last_stream_fault.take(),
            over,
            first_of_episode,
        }
    }

    /// Fold one drain in and decide whether anything should be logged now.
    fn note(&mut self, faults: RtFaults, now: Instant) -> Option<FaultReport> {
        self.add(faults);
        match self.window {
            // Quiet, and still quiet.
            None if !self.pending() => None,
            // First fault after a quiet spell: say so straight away, then hold
            // the door shut for FAULT_REPORT_INTERVAL.
            None => {
                self.window = Some(now);
                Some(self.take(Duration::ZERO, true))
            }
            Some(opened) => {
                if now.duration_since(opened) < FAULT_REPORT_INTERVAL {
                    return None;
                }
                if !self.pending() {
                    // Nothing repeated in the whole window: the episode is over,
                    // so the next fault is news again.
                    self.window = None;
                    return None;
                }
                let over = now.duration_since(opened);
                self.window = Some(now);
                Some(self.take(over, false))
            }
        }
    }
}

/// Turn a coalesced report into log lines, and — when the user has to do
/// something about it — one toast per episode.
fn report_faults(state: &Arc<AppState>, report: &FaultReport) {
    let device = state
        .engine
        .current_device()
        .unwrap_or_else(|| "system default".to_string());
    let rate = state.engine.engine_rate();
    let window = if report.over.is_zero() {
        "just now".to_string()
    } else {
        format!("in the last {:.0} s", report.over.as_secs_f64())
    };

    if report.dropped_commands > 0 {
        // Degraded but continuing: audio is still playing, but a control change
        // (a fader move, an EQ edit) never reached the callback.
        log::warn!(
            "{} engine command(s) were dropped {window}: the real-time queue was full, \
             so a control change did not take effect (output \"{device}\", {rate} Hz)",
            report.dropped_commands
        );
    }

    if report.stream_faults == 0 {
        return;
    }
    match report.last_stream_fault {
        Some(StreamFault::DeviceUnavailable) => {
            // The user's audio is gone and only they can fix it.
            log::error!(
                "output \"{device}\" failed {} time(s) {window}: {} - there is no audio until \
                 an output device is selected again",
                report.stream_faults,
                StreamFault::DeviceUnavailable.as_str()
            );
            if report.first_of_episode {
                state.error("Output device disconnected — choose another output in Settings");
            }
        }
        // A backend hiccup drops audio but the stream survives; it already shows
        // up in the transport's underrun count, so it is a log line, not a toast.
        Some(StreamFault::Backend) => log::warn!(
            "output \"{device}\" reported {} stream error(s) {window}: {} - audio dropped out",
            report.stream_faults,
            StreamFault::Backend.as_str()
        ),
        None => log::warn!(
            "output \"{device}\" reported {} stream error(s) {window} of an unknown kind",
            report.stream_faults
        ),
    }
}

/// Is *any* window on screen?
///
/// This used to ask only about the main window, which was the same question
/// while there was only one. The EQ window draws the 60 Hz analyser and can
/// live on another monitor, so minimising the main window must not freeze the
/// curve in front of the engineer's face.
fn window_visible(app: &AppHandle) -> bool {
    app.webview_windows()
        .values()
        .any(|w| w.is_visible().unwrap_or(true) && !w.is_minimized().unwrap_or(false))
}

/// Auto-advance. Loading touches the disk, so it happens on its own thread —
/// the pump must not miss frames because a file was slow to open.
fn handle_ended(state: &Arc<AppState>) {
    if state.engine.loop_enabled() {
        state.engine.seek_frames(0);
        state.engine.play();
        return;
    }
    let worker = Arc::clone(state);
    let spawned = std::thread::Builder::new()
        .name("onyx-advance".into())
        .spawn(move || {
            // No wrap: reaching the end of the playlist stops, it does not
            // silently start the first track again.
            if let Err(e) = loader::play_step(&worker, 1, false) {
                // The end of a playlist is not a failure, and neither is a
                // blind test refusing to have its material swapped: detail.
                log::debug!("auto-advance stopped: {e}");
            }
        });
    if let Err(e) = spawned {
        // The track ended and the next one will not start: the user is sitting
        // in silence wondering why, so say it in the window too.
        log::error!("could not start the auto-advance thread: {e}");
        state.error("Could not start the next track: the system refused a new thread");
    }
}

/// Copy a finished decode's loudness analysis into its playlist entry so the
/// row keeps showing the LUFS value after the deck moves on.
///
/// This is a safety net: the load watcher in [`crate::loader`] normally does it
/// (and stores the measurement in the persistent cache) the instant the decode
/// finishes. It only has anything to do when that watcher thread could not be
/// spawned, which is why it is cheap and idempotent — it acts only on a row that
/// has no analysis yet.
fn mirror_finished_analysis(state: &AppState) -> bool {
    let mut changed = false;
    for deck in [Deck::A, Deck::B] {
        // The decks lock is released before the playlist lock is taken: the two
        // are never held together anywhere in the app.
        let (entry_id, analysis) = {
            let decks = state.decks.lock();
            let slot = &decks[deck.index()];
            (slot.entry_id, slot.analysis())
        };
        let (Some(entry_id), Some(analysis)) = (entry_id, analysis) else {
            continue;
        };
        let mut playlist = state.playlist.lock();
        if let Some(entry) = playlist.get_mut(entry_id) {
            if entry.analysis.is_none() {
                entry.analysis = Some(analysis);
                changed = true;
            }
        }
    }
    changed
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dropped(n: u32) -> RtFaults {
        RtFaults {
            dropped_commands: n,
            ..RtFaults::default()
        }
    }

    fn stream(n: u32, kind: StreamFault) -> RtFaults {
        RtFaults {
            stream_faults: n,
            last_stream_fault: Some(kind),
            ..RtFaults::default()
        }
    }

    /* ── coalescing the real-time fault counters ──────────────────────── */

    #[test]
    fn a_quiet_engine_never_logs() {
        let mut log = FaultLog::default();
        let start = Instant::now();
        for tick in 0..40 {
            let now = start + FAULT_DRAIN_INTERVAL * tick;
            assert_eq!(log.note(RtFaults::default(), now), None, "tick {tick}");
        }
    }

    #[test]
    fn the_first_fault_is_reported_immediately() {
        let mut log = FaultLog::default();
        let now = Instant::now();
        let report = log
            .note(stream(1, StreamFault::DeviceUnavailable), now)
            .expect("the first fault must not wait for the window");
        assert_eq!(report.stream_faults, 1);
        assert_eq!(
            report.last_stream_fault,
            Some(StreamFault::DeviceUnavailable)
        );
        assert!(report.over.is_zero(), "the first report covers no interval");
        assert!(report.first_of_episode, "a toast is owed exactly once");
    }

    #[test]
    fn a_permanent_fault_reports_counts_not_lines() {
        // The case that matters: a command queue that is full on every drain.
        // Twelve seconds at 4 Hz is 48 drains; unbatched that is 48 log lines.
        let mut log = FaultLog::default();
        let start = Instant::now();
        let mut reports = Vec::new();
        for tick in 0..48 {
            let now = start + FAULT_DRAIN_INTERVAL * tick;
            if let Some(report) = log.note(dropped(7), now) {
                reports.push(report);
            }
        }
        // One immediate report, then one per FAULT_REPORT_INTERVAL: at most
        // three over twelve seconds.
        assert_eq!(reports.len(), 3, "{reports:#?}");
        assert!(reports[0].first_of_episode);
        assert!(!reports[1].first_of_episode && !reports[2].first_of_episode);
        // Nothing is lost: what was reported plus what is still accumulating
        // accounts for every dropped command the engine counted.
        let counted: u64 = reports.iter().map(|r| r.dropped_commands).sum();
        assert_eq!(counted + log.dropped_commands, 48 * 7);
        // The later reports carry the interval they cover, so the line can say
        // "N in the last 5 s" instead of implying "N right now".
        assert!(reports[1].over >= FAULT_REPORT_INTERVAL);
    }

    #[test]
    fn a_fault_that_stops_ends_the_episode_so_the_next_one_is_news_again() {
        let mut log = FaultLog::default();
        let start = Instant::now();
        // One fault, reported at once.
        assert!(log.note(dropped(1), start).is_some());
        // Quiet for longer than the report interval: the window closes without
        // emitting an empty report.
        assert_eq!(
            log.note(RtFaults::default(), start + FAULT_REPORT_INTERVAL * 2),
            None
        );
        // ...and the next fault is a fresh episode, so it toasts again.
        let report = log
            .note(dropped(1), start + FAULT_REPORT_INTERVAL * 3)
            .expect("a new episode must report immediately");
        assert!(report.first_of_episode);
        assert!(report.over.is_zero());
    }

    #[test]
    fn a_mixed_episode_keeps_the_latest_stream_fault_kind() {
        let mut log = FaultLog::default();
        let start = Instant::now();
        // First report drains the backend hiccup.
        let first = log.note(stream(1, StreamFault::Backend), start).unwrap();
        assert_eq!(first.last_stream_fault, Some(StreamFault::Backend));
        // Then the device goes away and commands start piling up.
        log.note(
            stream(2, StreamFault::Backend),
            start + FAULT_DRAIN_INTERVAL,
        );
        log.note(
            stream(1, StreamFault::DeviceUnavailable),
            start + FAULT_DRAIN_INTERVAL * 2,
        );
        log.note(dropped(9), start + FAULT_DRAIN_INTERVAL * 3);
        let report = log
            .note(RtFaults::default(), start + FAULT_REPORT_INTERVAL)
            .expect("the accumulated episode must be reported");
        assert_eq!(report.stream_faults, 3, "counts add up across drains");
        assert_eq!(report.dropped_commands, 9);
        // The worst/most recent kind is what the user is told about.
        assert_eq!(
            report.last_stream_fault,
            Some(StreamFault::DeviceUnavailable)
        );
        assert!(!report.first_of_episode, "still the same episode");
    }

    #[test]
    fn taking_a_report_empties_the_accumulator() {
        let mut log = FaultLog::default();
        log.add(dropped(3));
        log.add(stream(2, StreamFault::Backend));
        assert!(log.pending());
        let report = log.take(Duration::from_secs(5), false);
        assert_eq!(report.dropped_commands, 3);
        assert_eq!(report.stream_faults, 2);
        assert!(!log.pending(), "a drained log must not report twice");
        assert_eq!(log.last_stream_fault, None);
    }
}
