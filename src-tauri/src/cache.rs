//! Persistent loudness cache (SPEC §8).
//!
//! Integrated LUFS / LRA / true peak for files you have already played, so the
//! playlist shows real numbers on the first paint and A/B level matching is
//! ready before a decode finishes.
//!
//! # What the key has to cover
//!
//! A record describes **the PCM a decode produced**, not the bytes on disk, so
//! the key is everything that decides what those samples are:
//!
//! | part | why |
//! |---|---|
//! | canonical path, size, mtime | a different or edited file is a different measurement |
//! | `render_key` | a `.mid` has no loudness of its own — the SoundFont's does (SPEC §18) |
//! | decode rate | the engine follows the source rate (SPEC §9.6), so the same file decodes at 44.1 / 48 / 96 kHz on different devices, through the resampler or bit-transparently, and true peak in particular is *not* the same number |
//! | [`DECODE_SEMANTICS`] | what the decoder does to the samples: Opus pre-skip, AAC/MP4 edit-list priming, the length bound, the fold, the loudness maths |
//!
//! The last two are the ones that were missing. A file measured at 48 kHz was
//! served from cache on a 96 kHz device, and a change to priming handling would
//! have silently reused every measurement taken before it. If you are here
//! because you changed decode behaviour, the answer is in
//! [`DECODE_SEMANTICS`]'s own documentation: bump it.
//!
//! What else makes it safe to trust:
//!
//! * every record carries the [`SCHEMA`] the measurement was made with, so
//!   changing the *record layout* discards old values instead of silently
//!   believing them,
//! * the file is bounded at [`MAX_ENTRIES`] with least-recently-used eviction,
//! * a corrupt or unparseable file degrades to "no cache" with a warning and is
//!   never fatal,
//! * writes go through [`AtomicWriter`], so no command ever blocks on the disk
//!   and a crash cannot leave a half-written cache behind.
//!
//! Nothing on a command path ever serialises the cache either: [`store`] and
//! [`lookup`] only touch the map and set a flag, and [`LoudnessCache::tick`] —
//! called from the frame thread at the same low rate as the settings save —
//! does the JSON work. Otherwise adding 200 files would serialise 5 000 records
//! 200 times.
//!
//! Waveform peaks are deliberately **not** cached: they regenerate in
//! milliseconds and would dominate the file size. They do, however, come out of
//! the same decode, so a rate or semantics change invalidates them too — they
//! are simply rebuilt every time rather than remembered.
//!
//! [`store`]: LoudnessCache::store
//! [`lookup`]: LoudnessCache::lookup

use std::collections::HashMap;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};

use onyx_core::decode::DECODE_SEMANTICS;
use onyx_core::LoudnessAnalysis;
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};

use crate::persist::{read_optional, unix_secs, AtomicWriter, DEBOUNCE};

/// Bump whenever the *record layout* changes (SPEC §8). What the numbers mean
/// is [`DECODE_SEMANTICS`]' business, and it is part of the key rather than of
/// the record: two builds with different decode semantics can share a cache
/// file without either believing the other's measurements.
///
/// `2` since the key gained the decode rate and the semantics version — records
/// written by schema 1 are keyed on neither and are discarded on load.
pub const SCHEMA: u32 = 2;
/// Hard cap on records; the least recently used are evicted on insert.
pub const MAX_ENTRIES: usize = 5_000;
pub const FILE_NAME: &str = "loudness-cache.json";

/// What `cache_stats` / `cache_clear` return.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CacheStats {
    pub entries: usize,
    pub bytes: u64,
    pub path: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Record {
    /// Readable path, kept purely so the file can be debugged by eye.
    #[serde(default)]
    path: String,
    integrated_lufs: f32,
    lra: f32,
    true_peak_db: f32,
    sample_peak_db: f32,
    last_used_unix: u64,
    schema: u32,
    /// Insertion order within one run, used only to break `last_used_unix`
    /// ties (its resolution is a whole second). Not persisted.
    #[serde(skip)]
    seq: u64,
}

impl Record {
    fn analysis(&self) -> LoudnessAnalysis {
        LoudnessAnalysis {
            integrated_lufs: self.integrated_lufs,
            lra: self.lra,
            true_peak_db: self.true_peak_db,
            sample_peak_db: self.sample_peak_db,
        }
    }
}

#[derive(Debug, Default, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
struct CacheFile {
    schema: u32,
    entries: HashMap<String, Record>,
}

#[derive(Default)]
struct Inner {
    entries: HashMap<String, Record>,
    /// Size of the file as last read or written, so `cache_stats` can answer
    /// without touching the disk.
    bytes: u64,
    seq: u64,
}

pub struct LoudnessCache {
    inner: Mutex<Inner>,
    /// Set by `store`/`lookup`/`clear`, consumed by [`LoudnessCache::tick`].
    dirty: AtomicBool,
    writer: AtomicWriter,
}

impl LoudnessCache {
    /// Load the cache from `dir` (usually `app_cache_dir()`). Never fails: a
    /// missing directory means "in-memory only", a broken file means "empty".
    pub fn open(dir: Option<&Path>) -> LoudnessCache {
        let path = dir.map(|d| d.join(FILE_NAME));
        let mut inner = Inner::default();
        if let Some(path) = path.as_deref() {
            match read_optional(path) {
                Ok(None) => {}
                Ok(Some(text)) => {
                    inner.bytes = text.len() as u64;
                    match serde_json::from_str::<CacheFile>(&text) {
                        Ok(file) if file.schema == SCHEMA => {
                            inner.entries = file
                                .entries
                                .into_iter()
                                .filter(|(_, r)| r.schema == SCHEMA)
                                .collect();
                        }
                        Ok(file) => {
                            // Expected right after an upgrade, not a problem:
                            // the measurements are simply recomputed on demand.
                            log::info!(
                                "{FILE_NAME} was written by schema {} (this build is {SCHEMA}); \
                                 discarding {} stale measurement(s)",
                                file.schema,
                                file.entries.len()
                            );
                            log::debug!("stale loudness cache at {}", path.display());
                            inner.bytes = 0;
                        }
                        Err(e) => {
                            log::warn!(
                                "loudness cache {FILE_NAME} is corrupt ({e}); starting from empty, \
                                 so loudness will be re-measured on first play"
                            );
                            log::debug!("unreadable loudness cache at {}", path.display());
                            inner.bytes = 0;
                        }
                    }
                }
                Err(e) => {
                    log::warn!(
                        "could not read the loudness cache ({e}); loudness will be re-measured \
                         and nothing will be cached this session"
                    );
                    log::debug!("unreadable loudness cache at {}", path.display());
                }
            }
        }
        LoudnessCache {
            inner: Mutex::new(inner),
            dirty: AtomicBool::new(false),
            writer: AtomicWriter::spawn(path, "loudness-cache", DEBOUNCE),
        }
    }

    /// A cache with nowhere to write, for tests and for a system where the
    /// cache directory cannot be resolved.
    #[cfg(test)]
    fn memory_only() -> LoudnessCache {
        LoudnessCache {
            inner: Mutex::new(Inner::default()),
            dirty: AtomicBool::new(false),
            writer: AtomicWriter::spawn(None, "loudness-cache-test", DEBOUNCE),
        }
    }

    /// [`LoudnessCache::lookup`] as a build with a different
    /// [`DECODE_SEMANTICS`] would perform it.
    #[cfg(test)]
    fn lookup_as(
        &self,
        path: &Path,
        render_key: Option<&str>,
        rate: u32,
        semantics: u32,
    ) -> Option<LoudnessAnalysis> {
        let (key, _) = key_for(path, render_key, rate, semantics)?;
        self.lookup_key(&key)
    }

    /// [`LoudnessCache::store`] as a build with a different
    /// [`DECODE_SEMANTICS`] would perform it.
    #[cfg(test)]
    fn store_as(
        &self,
        path: &Path,
        render_key: Option<&str>,
        rate: u32,
        semantics: u32,
        analysis: &LoudnessAnalysis,
    ) {
        let Some((key, readable)) = key_for(path, render_key, rate, semantics) else {
            return;
        };
        self.store_key(key, readable, analysis);
    }

    /// Analysis for `path`, if we measured *this* version of the file, at
    /// *this* rate, through *this* renderer and this build's decode semantics.
    ///
    /// `render_key` is `TrackInfo::render_key`: `None` for an ordinary audio
    /// file, and the SoundFont's identity for a MIDI render (SPEC §18). A
    /// `.mid` has no inherent loudness — what was measured is what the bank
    /// produced — so leaving it out of the key would serve the old bank's
    /// measurement after the user picks a new one.
    ///
    /// `rate` is the rate the caller is about to decode at (see
    /// [`AppState::expected_decode_rate`]). A rate the file has not been
    /// measured at is a miss, which costs one measurement; the alternative —
    /// answering with a measurement taken at another rate — is a wrong number
    /// in a mastering tool.
    ///
    /// [`AppState::expected_decode_rate`]: crate::state::AppState::expected_decode_rate
    pub fn lookup(
        &self,
        path: &Path,
        render_key: Option<&str>,
        rate: u32,
    ) -> Option<LoudnessAnalysis> {
        let (key, _) = key_for(path, render_key, rate, DECODE_SEMANTICS)?;
        self.lookup_key(&key)
    }

    fn lookup_key(&self, key: &str) -> Option<LoudnessAnalysis> {
        let mut inner = self.inner.lock();
        let now = unix_secs();
        let seq = inner.next_seq();
        let record = inner.entries.get_mut(key)?;
        if record.schema != SCHEMA {
            return None;
        }
        // Touch it so a file you keep coming back to is not evicted.
        record.last_used_unix = now;
        record.seq = seq;
        let analysis = record.analysis();
        drop(inner);
        self.dirty.store(true, Ordering::Release);
        Some(analysis)
    }

    /// Record (or refresh) the analysis of `path`. `render_key` and `rate` as
    /// in [`LoudnessCache::lookup`] — `rate` must be the rate the decode that
    /// produced `analysis` actually ran at, not the rate that was asked for.
    pub fn store(
        &self,
        path: &Path,
        render_key: Option<&str>,
        rate: u32,
        analysis: &LoudnessAnalysis,
    ) {
        let Some((key, readable)) = key_for(path, render_key, rate, DECODE_SEMANTICS) else {
            return;
        };
        self.store_key(key, readable, analysis);
    }

    fn store_key(&self, key: String, readable: String, analysis: &LoudnessAnalysis) {
        {
            let mut inner = self.inner.lock();
            let seq = inner.next_seq();
            inner.entries.insert(
                key,
                Record {
                    path: readable,
                    integrated_lufs: analysis.integrated_lufs,
                    lra: analysis.lra,
                    true_peak_db: analysis.true_peak_db,
                    sample_peak_db: analysis.sample_peak_db,
                    last_used_unix: unix_secs(),
                    schema: SCHEMA,
                    seq,
                },
            );
            inner.evict_to(MAX_ENTRIES);
        }
        self.dirty.store(true, Ordering::Release);
    }

    pub fn stats(&self) -> CacheStats {
        let inner = self.inner.lock();
        CacheStats {
            entries: inner.entries.len(),
            bytes: inner.bytes,
            path: self
                .writer
                .path()
                .map(|p| p.to_string_lossy().to_string())
                .unwrap_or_default(),
        }
    }

    pub fn clear(&self) -> CacheStats {
        {
            let mut inner = self.inner.lock();
            inner.entries.clear();
            // Report zero bytes straight away: the file is about to be an empty
            // cache and the settings panel must not show a stale size.
            inner.bytes = 0;
        }
        self.dirty.store(true, Ordering::Release);
        // `cache_clear` is the one place a user expects the file to change now,
        // and the panel re-reads the stats from the return value.
        self.tick();
        self.stats()
    }

    /// Serialise and hand over to the writer, but only if something changed.
    /// Called from the frame thread, never from a command or a decode thread.
    pub fn tick(&self) {
        if !self.dirty.swap(false, Ordering::AcqRel) {
            return;
        }
        if self.writer.path().is_none() {
            return;
        }
        // The map is cloned under the lock and serialised outside it: with 5 000
        // records the JSON pass is milliseconds, and holding the lock across it
        // would stall every probe worker's `lookup` behind the frame thread.
        let file = CacheFile {
            schema: SCHEMA,
            entries: self.inner.lock().entries.clone(),
        };
        let text = match serde_json::to_string(&file) {
            Ok(text) => text,
            Err(e) => {
                log::error!(
                    "could not serialise the loudness cache ({e}); this session's measurements \
                     will not be kept"
                );
                return;
            }
        };
        self.inner.lock().bytes = text.len() as u64;
        self.writer.queue(text);
    }

    /// Write anything outstanding. Called once, at exit.
    pub fn flush(&self) {
        self.tick();
        self.writer.flush_blocking();
    }
}

impl Inner {
    fn next_seq(&mut self) -> u64 {
        self.seq = self.seq.wrapping_add(1);
        self.seq
    }

    /// Drop least-recently-used records until at most `max` remain.
    fn evict_to(&mut self, max: usize) {
        if self.entries.len() <= max {
            return;
        }
        let mut order: Vec<(u64, u64, String)> = self
            .entries
            .iter()
            .map(|(k, r)| (r.last_used_unix, r.seq, k.clone()))
            .collect();
        order.sort_unstable();
        let excess = self.entries.len() - max;
        for (_, _, key) in order.into_iter().take(excess) {
            self.entries.remove(&key);
        }
    }
}

/// `(hashed key, readable path)` for a file that exists, or `None` when it
/// cannot be stat-ed — an unreadable file has no cacheable identity.
///
/// The parts, and why each is in there, are the table in the module
/// documentation. `rate` is the sample rate the measurement was (or would be)
/// taken at and `render_key` the SoundFont a MIDI row is rendered through.
///
/// `semantics` is [`DECODE_SEMANTICS`] at every real callsite; it is a
/// parameter only so the tests can prove that bumping it misses the cache
/// without having to edit the constant.
///
/// A rate of `0` means "unknown", which is a key of its own rather than a
/// wildcard: guessing would be the false hit this function exists to prevent.
fn key_for(
    path: &Path,
    render_key: Option<&str>,
    rate: u32,
    semantics: u32,
) -> Option<(String, String)> {
    let meta = std::fs::metadata(path).ok()?;
    if !meta.is_file() {
        return None;
    }
    let canonical = de_verbatim(
        &path
            .canonicalize()
            .unwrap_or_else(|_| path.to_path_buf())
            .to_string_lossy(),
    );
    let mtime_ms = meta
        .modified()
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_millis())
        .unwrap_or(0);
    let render = render_key.unwrap_or("");
    let raw = format!(
        "{canonical}|{}|{mtime_ms}|{rate}|d{semantics}|{render}",
        meta.len()
    );
    Some((format!("{:016x}", fnv1a(raw.as_bytes())), canonical))
}

/// Undo Windows' `\\?\` verbatim prefix.
///
/// `Path::canonicalize` returns `\\?\C:\Music\take.wav` on Windows, and
/// `\\?\UNC\nas\masters\take.wav` for a share. That form is part of the cache
/// key and is written into the file as the human-readable path, so without
/// this the key depends on whether `canonicalize` succeeded (it falls back to
/// the plain path), and the file is unreadable by eye. A no-op everywhere else.
fn de_verbatim(path: &str) -> String {
    if let Some(unc) = path.strip_prefix(r"\\?\UNC\") {
        return format!(r"\\{unc}");
    }
    path.strip_prefix(r"\\?\").unwrap_or(path).to_string()
}

/// FNV-1a, 64 bit. Keeps the file small without pulling in a hash crate.
fn fnv1a(bytes: &[u8]) -> u64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for b in bytes {
        hash ^= *b as u64;
        hash = hash.wrapping_mul(0x1000_0000_01b3);
    }
    hash
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn windows_verbatim_prefixes_are_stripped_from_cache_keys() {
        assert_eq!(de_verbatim(r"\\?\C:\Music\take.wav"), r"C:\Music\take.wav");
        assert_eq!(
            de_verbatim(r"\\?\UNC\nas\masters\take.wav"),
            r"\\nas\masters\take.wav"
        );
        // Everything else, including every POSIX path, is untouched.
        assert_eq!(
            de_verbatim("/Users/me/Music/take.wav"),
            "/Users/me/Music/take.wav"
        );
        assert_eq!(de_verbatim(r"C:\Music\take.wav"), r"C:\Music\take.wav");
        assert_eq!(
            de_verbatim(r"\\nas\masters\take.wav"),
            r"\\nas\masters\take.wav"
        );
    }

    fn analysis(lufs: f32) -> LoudnessAnalysis {
        LoudnessAnalysis {
            integrated_lufs: lufs,
            lra: 6.0,
            true_peak_db: -0.8,
            sample_peak_db: -1.2,
        }
    }

    fn scratch(tag: &str) -> PathBuf {
        static N: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
        let n = N.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!("onyx-cache-{}-{tag}-{n}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// A device rate, so the tests read as "measured at 48 kHz".
    const R48: u32 = 48_000;
    const R96: u32 = 96_000;

    #[test]
    fn a_round_trip_returns_the_measurement() {
        let dir = scratch("roundtrip");
        let file = dir.join("take.wav");
        std::fs::write(&file, b"0123456789").unwrap();

        let cache = LoudnessCache::memory_only();
        assert!(
            cache.lookup(&file, None, R48).is_none(),
            "empty cache must not answer"
        );
        cache.store(&file, None, R48, &analysis(-14.2));
        let got = cache
            .lookup(&file, None, R48)
            .expect("stored value must come back");
        assert_eq!(got.integrated_lufs, -14.2);
        assert_eq!(got.true_peak_db, -0.8);
        assert_eq!(cache.stats().entries, 1);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The engine follows the source rate (SPEC §9.6), so the same file is
    /// decoded at whatever the device runs at — through the resampler on one
    /// machine and bit-transparently on another. True peak especially is not
    /// the same number, so a measurement taken at another rate is not an
    /// answer to this question.
    #[test]
    fn a_measurement_belongs_to_the_rate_it_was_taken_at() {
        let dir = scratch("rate");
        let file = dir.join("take.wav");
        std::fs::write(&file, b"0123456789").unwrap();
        let cache = LoudnessCache::memory_only();

        cache.store(&file, None, R48, &analysis(-14.0));
        assert_eq!(
            cache.lookup(&file, None, R48).unwrap().integrated_lufs,
            -14.0,
            "the same rate must hit"
        );
        assert!(
            cache.lookup(&file, None, R96).is_none(),
            "a 96 kHz device must not be served a 48 kHz measurement"
        );
        // An unknown rate is its own key, never a wildcard that matches one.
        assert!(cache.lookup(&file, None, 0).is_none());

        cache.store(&file, None, R96, &analysis(-13.4));
        assert_eq!(
            cache.lookup(&file, None, R96).unwrap().integrated_lufs,
            -13.4
        );
        assert_eq!(
            cache.lookup(&file, None, R48).unwrap().integrated_lufs,
            -14.0,
            "each rate keeps its own measurement"
        );
        assert_eq!(cache.stats().entries, 2);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The tripwire for the requirement documented on
    /// [`DECODE_SEMANTICS`]: change what a decode produces — AAC edit-list
    /// priming, Opus pre-skip, the fold, the loudness maths — bump the
    /// constant, and every measurement taken by the old behaviour is a miss
    /// rather than a number nobody can reproduce.
    #[test]
    fn a_decode_semantics_bump_misses_and_an_unrelated_change_still_hits() {
        let dir = scratch("semantics");
        let file = dir.join("take.m4a");
        std::fs::write(&file, b"pretend aac").unwrap();
        let cache = LoudnessCache::memory_only();

        cache.store_as(&file, None, R48, DECODE_SEMANTICS, &analysis(-9.5));
        assert_eq!(
            cache
                .lookup_as(&file, None, R48, DECODE_SEMANTICS)
                .unwrap()
                .integrated_lufs,
            -9.5,
            "this build must find its own measurement"
        );
        assert!(
            cache
                .lookup_as(&file, None, R48, DECODE_SEMANTICS + 1)
                .is_none(),
            "a build with new decode semantics must re-measure"
        );
        // ...and something that does not change the decoded samples — a second
        // look at the same file, same rate, same semantics — still hits, so the
        // key is not simply invalidating everything.
        assert!(
            cache
                .lookup_as(&file, None, R48, DECODE_SEMANTICS)
                .is_some(),
            "an unrelated repeat must stay a hit"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn editing_a_file_invalidates_its_entry() {
        let dir = scratch("mtime");
        let file = dir.join("take.wav");
        std::fs::write(&file, b"0123456789").unwrap();
        let cache = LoudnessCache::memory_only();
        cache.store(&file, None, R48, &analysis(-14.0));
        assert!(cache.lookup(&file, None, R48).is_some());

        // Same path, different size: a different key, so no stale answer.
        std::fs::write(&file, b"0123456789-and-more").unwrap();
        assert!(
            cache.lookup(&file, None, R48).is_none(),
            "an edited file must be re-measured"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// SPEC §18: a `.mid` has no inherent loudness — the number describes
    /// what the SoundFont produced. Two banks must not share a cache entry, or
    /// changing bank shows the old bank's measurement for ever.
    #[test]
    fn a_midi_render_is_cached_per_soundfont() {
        let dir = scratch("bank");
        let file = dir.join("suite.mid");
        std::fs::write(&file, b"MThd-pretend").unwrap();
        let cache = LoudnessCache::memory_only();

        let bundled = Some("bundled:generaluser-gs-2.0.3");
        let user = Some("user:/banks/Arachno.sf2|12345|999");
        cache.store(&file, bundled, R48, &analysis(-18.0));
        assert_eq!(
            cache.lookup(&file, bundled, R48).unwrap().integrated_lufs,
            -18.0,
            "the same bank must hit"
        );
        assert!(
            cache.lookup(&file, user, R48).is_none(),
            "a different SoundFont must not serve the previous bank's measurement"
        );
        // ...and the plain audio key is a third, separate identity, so an
        // existing cache written before v3 is neither read nor corrupted.
        assert!(cache.lookup(&file, None, R48).is_none());
        // The render rate scopes it too: the synth renders at the device rate.
        assert!(cache.lookup(&file, bundled, R96).is_none());

        cache.store(&file, user, R48, &analysis(-11.0));
        assert_eq!(
            cache.lookup(&file, user, R48).unwrap().integrated_lufs,
            -11.0
        );
        assert_eq!(
            cache.lookup(&file, bundled, R48).unwrap().integrated_lufs,
            -18.0,
            "switching back must find the first measurement again"
        );
        assert_eq!(cache.stats().entries, 2);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_missing_file_has_no_identity() {
        let cache = LoudnessCache::memory_only();
        let gone = std::env::temp_dir().join("onyx-definitely-not-here.wav");
        assert!(cache.lookup(&gone, None, R48).is_none());
        // Storing must not panic or create a bogus record either.
        cache.store(&gone, None, R48, &analysis(-9.0));
        assert_eq!(cache.stats().entries, 0);
    }

    #[test]
    fn the_cache_is_bounded_and_evicts_the_oldest() {
        let mut inner = Inner::default();
        for i in 0..(MAX_ENTRIES + 25) {
            let seq = inner.next_seq();
            inner.entries.insert(
                format!("k{i}"),
                Record {
                    path: format!("/music/{i}.wav"),
                    integrated_lufs: -14.0,
                    lra: 5.0,
                    true_peak_db: -1.0,
                    sample_peak_db: -1.0,
                    last_used_unix: 1_700_000_000,
                    schema: SCHEMA,
                    seq,
                },
            );
            inner.evict_to(MAX_ENTRIES);
        }
        assert_eq!(inner.entries.len(), MAX_ENTRIES);
        assert!(!inner.entries.contains_key("k0"), "oldest must be evicted");
        assert!(
            inner
                .entries
                .contains_key(&format!("k{}", MAX_ENTRIES + 24)),
            "newest must survive"
        );
    }

    #[test]
    fn a_corrupt_file_degrades_to_no_cache() {
        let dir = scratch("corrupt");
        std::fs::write(dir.join(FILE_NAME), b"{ this is not json").unwrap();
        let cache = LoudnessCache::open(Some(&dir));
        let stats = cache.stats();
        assert_eq!(stats.entries, 0, "a broken file must not be trusted");
        assert!(stats.path.ends_with(FILE_NAME));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_stale_schema_is_discarded_rather_than_believed() {
        let dir = scratch("schema");
        let body = format!(
            "{{\"schema\":{},\"entries\":{{\"abc\":{{\"path\":\"/x.wav\",\
             \"integratedLufs\":-9.0,\"lra\":1.0,\"truePeakDb\":0.0,\"samplePeakDb\":0.0,\
             \"lastUsedUnix\":1,\"schema\":{}}}}}}}",
            SCHEMA + 1,
            SCHEMA + 1
        );
        std::fs::write(dir.join(FILE_NAME), body).unwrap();
        assert_eq!(LoudnessCache::open(Some(&dir)).stats().entries, 0);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn entries_survive_a_reload_and_clear_empties_the_file() {
        let dir = scratch("reload");
        let file = dir.join("take.flac");
        std::fs::write(&file, b"some bytes here").unwrap();
        {
            let cache = LoudnessCache::open(Some(&dir));
            cache.store(&file, None, R48, &analysis(-11.5));
            cache.flush();
        }
        let cache = LoudnessCache::open(Some(&dir));
        assert_eq!(cache.stats().entries, 1);
        assert_eq!(
            cache.lookup(&file, None, R48).unwrap().integrated_lufs,
            -11.5
        );
        assert!(cache.stats().bytes > 0);

        let after = cache.clear();
        assert_eq!(after.entries, 0);
        // An empty cache is a few bytes of JSON, not the old size.
        assert!(after.bytes < 64, "stale size reported: {}", after.bytes);
        cache.flush();
        assert_eq!(LoudnessCache::open(Some(&dir)).stats().entries, 0);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_lookup_or_store_never_serialises_on_the_calling_thread() {
        let dir = scratch("nowrite");
        let file = dir.join("take.wav");
        std::fs::write(&file, b"bytes").unwrap();
        let cache = LoudnessCache::open(Some(&dir));
        cache.store(&file, None, R48, &analysis(-12.0));
        assert!(cache.lookup(&file, None, R48).is_some());
        // Nothing is written and nothing is even serialised until the frame
        // thread ticks: `bytes` is still the size we loaded (zero here).
        assert_eq!(cache.stats().bytes, 0);
        assert!(!dir.join(FILE_NAME).exists());
        cache.tick();
        assert!(cache.stats().bytes > 0);
        // A second tick with nothing new to say does no work.
        let before = cache.stats();
        cache.tick();
        assert_eq!(cache.stats(), before);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_cache_without_a_directory_still_works_in_memory() {
        let dir = scratch("nodir");
        let file = dir.join("take.wav");
        std::fs::write(&file, b"bytes").unwrap();
        let cache = LoudnessCache::open(None);
        cache.store(&file, None, R48, &analysis(-16.0));
        assert_eq!(
            cache.lookup(&file, None, R48).unwrap().integrated_lufs,
            -16.0
        );
        assert_eq!(cache.stats().path, "");
        cache.flush();
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_hash_is_stable_and_distinguishes_inputs() {
        assert_eq!(fnv1a(b""), 0xcbf2_9ce4_8422_2325);
        assert_ne!(fnv1a(b"/a.wav|10|1"), fnv1a(b"/a.wav|10|2"));
        assert_ne!(fnv1a(b"/a.wav|10|1"), fnv1a(b"/a.wav|11|1"));
    }
}
