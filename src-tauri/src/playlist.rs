//! The playlist model plus lazy, off-thread metadata probing.
//!
//! Adding 200 files must never block the UI, so `add_path` only records the
//! path and the file name and hands the id to a small worker pool. The workers
//! probe headers/tags and fill the entry in, marking the state dirty so the
//! frame thread pushes an `onyx://state` as results land.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use crossbeam_channel::{unbounded, Sender};
use onyx_core::decode::is_supported_path;
use onyx_core::{Deck, LoudnessAnalysis, TrackInfo};
use serde::Serialize;

use crate::archive::{self, ExtractedArchive};
use crate::state::AppState;

/// How many files may be probed in parallel. Probing is IO bound, but a header
/// parse on a cold spinning disk is slow enough that four helps a lot and more
/// than four only thrashes the drive.
const PROBE_WORKERS: usize = 4;
/// Directories dropped on the window are walked, but not forever.
const MAX_WALK_DEPTH: usize = 8;
/// Hard cap on how many files one open/drop may add.
pub const MAX_FILES_PER_OPEN: usize = 5_000;

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PlaylistEntry {
    pub id: u64,
    pub path: String,
    pub file_name: String,
    pub title: Option<String>,
    pub artist: Option<String>,
    pub duration_secs: f64,
    pub sample_rate: u32,
    pub channels: u16,
    pub codec: String,
    pub bits_per_sample: Option<u32>,
    pub is_lossless: bool,
    /// Loudness analysis, cached from the decoder once the track has played.
    pub analysis: Option<LoudnessAnalysis>,
    /// Which deck currently holds this entry.
    pub deck: Option<Deck>,
    /// Name of the `.zip` this row was extracted from (SPEC §19), so the
    /// playlist can show where a temp-directory path really came from.
    pub archive: Option<String>,
    /// SoundFont a MIDI row is rendered through (SPEC §18), for the
    /// `MIDI · GM · <bank>` badge. `None` for ordinary audio.
    pub synth_bank: Option<String>,
    /// Bank identity for the loudness cache key. Host-side only: the front end
    /// has no use for it and it is not part of the IPC contract.
    #[serde(skip)]
    pub render_key: Option<String>,
    /// File disappeared, is not audio, or failed to probe.
    pub missing: bool,
    /// Has a probe already run? Not sent to the UI: it shows up as a row with
    /// no duration/format until the probe lands.
    #[serde(skip)]
    pub probed: bool,
}

impl PlaylistEntry {
    fn new(id: u64, path: &Path, archive: Option<&str>) -> PlaylistEntry {
        PlaylistEntry {
            id,
            path: path.to_string_lossy().to_string(),
            file_name: path
                .file_name()
                .map(|f| f.to_string_lossy().to_string())
                .unwrap_or_else(|| path.to_string_lossy().to_string()),
            title: None,
            artist: None,
            duration_secs: 0.0,
            sample_rate: 0,
            channels: 0,
            codec: String::new(),
            bits_per_sample: None,
            is_lossless: false,
            analysis: None,
            deck: None,
            archive: archive.map(|a| a.to_string()),
            synth_bank: None,
            render_key: None,
            missing: false,
            probed: false,
        }
    }

    /// Fill the technical fields in from a probe.
    pub fn apply_probe(&mut self, info: &TrackInfo) {
        self.title = info.title.clone();
        self.artist = info.artist.clone();
        self.duration_secs = info.duration_secs;
        self.sample_rate = info.sample_rate;
        self.channels = info.channels;
        self.codec = info.codec.clone();
        self.bits_per_sample = info.bits_per_sample;
        self.is_lossless = info.is_lossless;
        self.synth_bank = info.synth_bank.clone();
        self.render_key = info.render_key.clone();
        self.missing = false;
        self.probed = true;
    }

    pub fn mark_missing(&mut self) {
        self.missing = true;
        self.probed = true;
    }

    pub fn path_buf(&self) -> PathBuf {
        PathBuf::from(&self.path)
    }
}

#[derive(Debug, Default)]
pub struct Playlist {
    pub entries: Vec<PlaylistEntry>,
    next_id: u64,
}

impl Playlist {
    /// Append a path. Ids are monotonic for the life of the process so a stale
    /// id from the UI can never resolve to a different track.
    ///
    /// `archive` is the file name of the `.zip` the file was unpacked from
    /// (SPEC §19), so the row can say where a temp-directory path came
    /// from; `None` for a file that is where the user put it.
    pub fn add_source(&mut self, path: &Path, archive: Option<&str>) -> u64 {
        self.next_id += 1;
        let id = self.next_id;
        self.entries.push(PlaylistEntry::new(id, path, archive));
        id
    }

    pub fn get(&self, id: u64) -> Option<&PlaylistEntry> {
        self.entries.iter().find(|e| e.id == id)
    }

    pub fn get_mut(&mut self, id: u64) -> Option<&mut PlaylistEntry> {
        self.entries.iter_mut().find(|e| e.id == id)
    }

    pub fn index_of(&self, id: u64) -> Option<usize> {
        self.entries.iter().position(|e| e.id == id)
    }

    pub fn id_at(&self, index: usize) -> Option<u64> {
        self.entries.get(index).map(|e| e.id)
    }

    pub fn remove(&mut self, id: u64) -> Option<PlaylistEntry> {
        let idx = self.index_of(id)?;
        Some(self.entries.remove(idx))
    }

    pub fn clear(&mut self) {
        self.entries.clear();
    }

    /// Move the entry at `from` so that it lands at `to`.
    pub fn move_entry(&mut self, from: usize, to: usize) -> bool {
        if from >= self.entries.len() {
            return false;
        }
        let entry = self.entries.remove(from);
        let to = to.min(self.entries.len());
        self.entries.insert(to, entry);
        true
    }

    /// Neighbour of `id`, wrapping around; `None` for an empty playlist.
    pub fn step(&self, id: Option<u64>, delta: isize, wrap: bool) -> Option<u64> {
        if self.entries.is_empty() {
            return None;
        }
        let len = self.entries.len() as isize;
        let current = id.and_then(|i| self.index_of(i)).map(|i| i as isize);
        let next = match current {
            Some(cur) => cur + delta,
            // Nothing playing: `next` starts at the top, `prev` at the bottom.
            None => {
                if delta >= 0 {
                    0
                } else {
                    len - 1
                }
            }
        };
        let next = if wrap {
            ((next % len) + len) % len
        } else if next < 0 || next >= len {
            return None;
        } else {
            next
        };
        self.id_at(next as usize)
    }

    /// Record which deck an entry sits on, clearing the previous occupant.
    pub fn assign_deck(&mut self, deck: Deck, id: Option<u64>) {
        for e in self.entries.iter_mut() {
            if e.deck == Some(deck) {
                e.deck = None;
            }
        }
        if let Some(id) = id {
            if let Some(e) = self.get_mut(id) {
                e.deck = Some(deck);
            }
        }
    }
}

/// Background probe pool. Dropping it closes the channel and the workers exit.
pub struct ProbePool {
    tx: Sender<u64>,
}

impl ProbePool {
    pub fn spawn(state: Arc<AppState>) -> ProbePool {
        let (tx, rx) = unbounded::<u64>();
        for n in 0..PROBE_WORKERS {
            let rx = rx.clone();
            let state = Arc::clone(&state);
            let spawned = std::thread::Builder::new()
                .name(format!("onyx-probe-{n}"))
                .spawn(move || {
                    while let Ok(id) = rx.recv() {
                        probe_one(&state, id);
                    }
                });
            if let Err(e) = spawned {
                // One worker short is slower metadata, not a broken playlist;
                // the load path probes inline when a row is still unresolved.
                log::warn!(
                    "could not spawn probe worker {n} ({e}); playlist metadata will fill in \
                     more slowly"
                );
            }
        }
        ProbePool { tx }
    }

    pub fn enqueue(&self, id: u64) {
        if let Err(e) = self.tx.send(id) {
            // Normal at shutdown, when the workers are already gone: the row
            // simply gets probed inline the first time it is loaded.
            log::debug!("probe queue closed ({e}); entry {id} will be probed on load");
        }
    }
}

/// Probe one entry and write the result back. Runs on a probe worker.
fn probe_one(state: &Arc<AppState>, id: u64) {
    let path = {
        let playlist = state.playlist.lock();
        match playlist.get(id) {
            // Already resolved (a load beat us to it) or gone from the list.
            Some(e) if !e.probed => e.path_buf(),
            _ => return,
        }
    };

    // Through `safe_decode`, never `decode::probe` directly: this is a worker
    // thread parsing an arbitrary file, and a panic here would silently retire
    // one of the four workers for the rest of the session.
    let result = crate::safe_decode::probe_with(&path, &state.decode_options());
    // The lookup stats the file, so it happens before the playlist lock is
    // taken: no IO of any kind inside a critical section the load path waits
    // on. It is keyed on the render bank for a MIDI file (SPEC §18) and on the
    // rate this file would be decoded at, both of which are only known once
    // the probe has run — hence the order.
    let cached = result.as_ref().ok().and_then(|info| {
        state.cache.lookup(
            &path,
            info.render_key.as_deref(),
            state.expected_decode_rate(info.sample_rate),
        )
    });
    let mut playlist = state.playlist.lock();
    let Some(entry) = playlist.get_mut(id) else {
        return;
    };
    match result {
        Ok(info) => {
            entry.apply_probe(&info);
            // SPEC §8: a file measured in an earlier session shows its
            // loudness on the first paint instead of after a decode.
            if let Some(analysis) = cached {
                entry.analysis = Some(analysis);
            }
        }
        Err(e) => {
            // The row is marked unreadable, so the user can see it in the list;
            // it only becomes an error when they try to play it.
            log::warn!(
                "could not read \"{}\" ({e}); the playlist row is marked unreadable",
                entry.file_name
            );
            log::debug!("unreadable playlist entry at {}", path.display());
            entry.mark_missing();
        }
    }
    let on_deck = entry.deck.is_some();
    drop(playlist);
    if on_deck {
        // A cache hit on a track that is already on a deck can complete the
        // level match before its decode finishes (SPEC §8 wire-up).
        crate::loader::recompute_trims(state);
    }
    state.mark_state_dirty();
}

/// Fill an entry's loudness in from the cache, if we have measured that exact
/// file before. Returns `true` when something was filled in.
///
/// Used by the load path, which probes inline when the background worker has
/// not reached the row yet. `render_key` is `TrackInfo::render_key` — the
/// SoundFont a MIDI row is rendered through, `None` for ordinary audio —
/// and `source_rate` the file's own rate, from which
/// [`AppState::expected_decode_rate`] derives the rate the cache is keyed on.
pub fn warm_analysis_from_cache(
    state: &AppState,
    id: u64,
    path: &Path,
    render_key: Option<&str>,
    source_rate: u32,
) -> bool {
    let rate = state.expected_decode_rate(source_rate);
    let Some(analysis) = state.cache.lookup(path, render_key, rate) else {
        return false;
    };
    let mut playlist = state.playlist.lock();
    match playlist.get_mut(id) {
        Some(entry) if entry.analysis.is_none() => {
            entry.analysis = Some(analysis);
            drop(playlist);
            state.mark_state_dirty();
            true
        }
        _ => false,
    }
}

/// One playable file, and the archive it was unpacked from (SPEC §19).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Source {
    pub path: PathBuf,
    /// File name of the `.zip`, or `None` for a file that was already on disk
    /// where the user pointed at it.
    pub archive: Option<String>,
}

/// What one open/drop expanded into.
#[derive(Default)]
pub struct Collected {
    pub files: Vec<Source>,
    /// Temp directories to keep alive for as long as the rows exist. The
    /// caller hands these to [`AppState::adopt_archives`]; dropping them
    /// instead deletes the extracted audio, which is exactly what should
    /// happen if the open goes no further.
    pub archives: Vec<ExtractedArchive>,
    /// Summary lines for the user — refused entries, undecodable ones, an
    /// archive with no audio in it. One line per archive per category, never
    /// one per file.
    pub warnings: Vec<String>,
}

/// Expand what the user handed us into a list of playable files: files pass
/// through the extension filter, directories are walked, and a `.zip` is
/// unpacked into a temp directory (SPEC §19).
///
/// `verify` decides whether an extracted file is really decodable; it is
/// `safe_decode::probe` wired up with the user's SoundFont, passed in so this
/// stays testable and so the archive module never has to know about settings.
pub fn collect_sources(inputs: &[String], verify: &dyn Fn(&Path) -> bool) -> Collected {
    let mut out = Collected::default();
    for raw in inputs {
        if out.files.len() >= MAX_FILES_PER_OPEN {
            break;
        }
        let path = PathBuf::from(raw);
        if archive::is_archive_path(&path) && path.is_file() {
            // A refusal here — a bomb, a corrupt archive — is the user's
            // business, so it becomes a warning rather than being swallowed.
            match archive::extract(&path, verify) {
                Ok(contents) => {
                    let name = contents.archive.name().to_string();
                    out.warnings.extend(contents.warnings);
                    for file in contents.files {
                        if out.files.len() >= MAX_FILES_PER_OPEN {
                            break;
                        }
                        out.files.push(Source {
                            path: file,
                            archive: Some(name.clone()),
                        });
                    }
                    // Kept even when it produced nothing: dropping it here
                    // would delete a directory the rows may point into, and
                    // an empty one costs a `remove_dir_all` at clear time.
                    out.archives.push(contents.archive);
                }
                Err(e) => out.warnings.push(e),
            }
            continue;
        }
        let mut plain: Vec<PathBuf> = Vec::new();
        if path.is_dir() {
            walk_dir(&path, 0, &mut plain);
        } else if is_supported_path(&path) && is_representable(&path) {
            plain.push(path);
        }
        out.files.extend(plain.into_iter().map(|path| Source {
            path,
            archive: None,
        }));
    }
    out.files.truncate(MAX_FILES_PER_OPEN);
    out
}

/// Everything Onyx will accept from a drop, a dialog or the command line:
/// audio extensions plus the archive extensions of SPEC §19.
///
/// This is the single list behind the file dialog's filter and the
/// `supportedExtensions` the snapshot hands the front end for drag-and-drop
/// acceptance, so the two can never disagree about `.zip`.
pub fn openable_extensions() -> Vec<&'static str> {
    onyx_core::decode::SUPPORTED_EXTENSIONS
        .iter()
        .copied()
        .chain(archive::ARCHIVE_EXTENSIONS.iter().copied())
        .collect()
}

/// Can this path survive the round trip to the webview and back?
///
/// A `PlaylistEntry` stores its path as a `String` and rebuilds a `PathBuf`
/// from it, and the whole IPC surface is JSON, which is UTF-8 by definition. A
/// path that is not valid UTF-8 — ordinary on Linux, possible on Windows with
/// an unpaired surrogate, and reachable on macOS over an SMB share — would be
/// lossily converted on the way in and would then name a file that does not
/// exist: a permanent "missing" row the user cannot act on. Refusing it with a
/// log line is the honest outcome. (Emoji, CJK and combining accents are all
/// valid UTF-8 and are unaffected; this rejects only genuinely ill-formed
/// names.)
fn is_representable(path: &Path) -> bool {
    if path.to_str().is_some() {
        return true;
    }
    log::warn!(
        "skipping \"{}\": the file name is not valid UTF-8 and cannot be opened",
        path.to_string_lossy()
    );
    false
}

fn walk_dir(dir: &Path, depth: usize, out: &mut Vec<PathBuf>) {
    if depth > MAX_WALK_DEPTH || out.len() >= MAX_FILES_PER_OPEN {
        return;
    }
    let Ok(read) = std::fs::read_dir(dir) else {
        return;
    };
    // Deterministic order: the same folder always produces the same playlist.
    let mut entries: Vec<PathBuf> = read.filter_map(|e| e.ok()).map(|e| e.path()).collect();
    entries.sort();
    for path in entries {
        if out.len() >= MAX_FILES_PER_OPEN {
            return;
        }
        if path.is_dir() {
            walk_dir(&path, depth + 1, out);
        } else if is_supported_path(&path) && is_representable(&path) {
            out.push(path);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `collect_sources` with everything accepted, reduced to plain paths:
    /// what the pre-archive `collect_files` used to return.
    fn collect_files(inputs: &[String]) -> Vec<PathBuf> {
        collect_sources(inputs, &|_| true)
            .files
            .into_iter()
            .map(|s| s.path)
            .collect()
    }

    fn list(paths: &[&str]) -> Playlist {
        let mut pl = Playlist::default();
        for p in paths {
            pl.add_source(Path::new(p), None);
        }
        pl
    }

    #[test]
    fn ids_are_monotonic_and_stable_across_removal() {
        let mut pl = list(&["/a.wav", "/b.wav", "/c.wav"]);
        assert_eq!(
            pl.entries.iter().map(|e| e.id).collect::<Vec<_>>(),
            [1, 2, 3]
        );
        pl.remove(2);
        let id = pl.add_source(Path::new("/d.wav"), None);
        assert_eq!(id, 4, "ids must never be reused");
        assert_eq!(
            pl.entries.iter().map(|e| e.id).collect::<Vec<_>>(),
            [1, 3, 4]
        );
    }

    #[test]
    fn move_entry_matches_drag_and_drop_semantics() {
        let mut pl = list(&["/a.wav", "/b.wav", "/c.wav"]);
        assert!(pl.move_entry(0, 2));
        assert_eq!(
            pl.entries
                .iter()
                .map(|e| e.file_name.clone())
                .collect::<Vec<_>>(),
            ["b.wav", "c.wav", "a.wav"]
        );
        assert!(!pl.move_entry(9, 0));
        // Clamping, not panicking, when the target is past the end.
        assert!(pl.move_entry(0, 99));
        assert_eq!(pl.entries.last().unwrap().file_name, "b.wav");
    }

    #[test]
    fn playlist_operations_on_an_empty_list_are_refusals_not_panics() {
        // SPEC §9.6. Every one of these is reachable from the UI: a drag on
        // an empty list, a stale row id, `playlist_move` with the row already
        // removed by another gesture.
        let mut pl = Playlist::default();
        assert!(!pl.move_entry(0, 0), "empty list has no row 0");
        assert!(!pl.move_entry(0, 5));
        assert!(!pl.move_entry(usize::MAX, 0));
        assert!(pl.remove(1).is_none());
        assert!(pl.get(1).is_none() && pl.get_mut(1).is_none());
        assert!(pl.id_at(0).is_none() && pl.index_of(7).is_none());
        assert!(pl.step(None, 1, true).is_none());
        assert!(pl.step(Some(3), -1, false).is_none());
        // ...and on a one-row list, where wrapping is the interesting case.
        let mut pl = list(&["/only.wav"]);
        assert!(pl.move_entry(0, 0));
        assert_eq!(pl.step(Some(1), 1, true), Some(1), "wraps onto itself");
        assert_eq!(pl.step(Some(1), 1, false), None, "and stops when told to");
        assert!(pl.remove(99).is_none(), "unknown id");
        assert_eq!(pl.entries.len(), 1);
        // A stale id must not resolve to a different track after a removal.
        pl.remove(1);
        assert!(pl.get(1).is_none());
        assert!(pl.step(Some(1), 1, true).is_none());
    }

    #[test]
    fn step_wraps_or_stops_as_asked() {
        let pl = list(&["/a.wav", "/b.wav"]);
        assert_eq!(pl.step(Some(1), 1, true), Some(2));
        assert_eq!(pl.step(Some(2), 1, true), Some(1));
        assert_eq!(pl.step(Some(2), 1, false), None);
        assert_eq!(pl.step(Some(1), -1, true), Some(2));
        assert_eq!(pl.step(None, 1, true), Some(1));
        assert_eq!(pl.step(None, -1, true), Some(2));
        assert_eq!(Playlist::default().step(None, 1, true), None);
    }

    #[test]
    fn assign_deck_moves_the_badge() {
        let mut pl = list(&["/a.wav", "/b.wav"]);
        pl.assign_deck(Deck::A, Some(1));
        assert_eq!(pl.get(1).unwrap().deck, Some(Deck::A));
        pl.assign_deck(Deck::A, Some(2));
        assert_eq!(pl.get(1).unwrap().deck, None);
        assert_eq!(pl.get(2).unwrap().deck, Some(Deck::A));
        pl.assign_deck(Deck::A, None);
        assert_eq!(pl.get(2).unwrap().deck, None);
    }

    #[test]
    fn collect_files_filters_and_walks() {
        let root = std::env::temp_dir().join(format!("onyx-pl-{}", std::process::id()));
        let nested = root.join("nested");
        std::fs::create_dir_all(&nested).unwrap();
        std::fs::write(root.join("one.wav"), b"x").unwrap();
        std::fs::write(root.join("notes.txt"), b"x").unwrap();
        std::fs::write(nested.join("two.flac"), b"x").unwrap();

        let found = collect_files(&[root.to_string_lossy().to_string()]);
        let names: Vec<String> = found
            .iter()
            .map(|p| p.file_name().unwrap().to_string_lossy().to_string())
            .collect();
        // Depth-first, alphabetical: `nested/` sorts before `one.wav`.
        assert_eq!(names, ["two.flac", "one.wav"]);
        let _ = std::fs::remove_dir_all(&root);
    }

    /// Names a mastering engineer really does use — spaces, accents, CJK,
    /// emoji, and a very long one — must all survive `collect_files`, which is
    /// the only place a path is filtered before it becomes a playlist row.
    #[test]
    fn awkward_but_legal_file_names_are_kept() {
        let root = std::env::temp_dir().join(format!("onyx-names-{}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        let names = [
            "Bj\u{f6}rk \u{2013} Jo\u{301}ga (24-96 master).wav",
            "\u{4e2d}\u{6587}\u{6a19}\u{984c}.flac",
            "\u{1f3b5} rough mix \u{1f525}.wav",
            "  leading and trailing spaces  .wav",
            &format!("{}.wav", "long".repeat(40)),
        ];
        for name in names {
            std::fs::write(root.join(name), b"x").unwrap();
        }
        let found = collect_files(&[root.to_string_lossy().to_string()]);
        assert_eq!(found.len(), names.len(), "{found:?}");
        for path in &found {
            // Every kept path must round-trip through the String the playlist
            // stores, or the row can never be loaded.
            let round_tripped = PathBuf::from(path.to_string_lossy().to_string());
            assert_eq!(&round_tripped, path);
            assert!(
                path.exists(),
                "{path:?} does not exist after the round trip"
            );
        }
        let _ = std::fs::remove_dir_all(&root);
    }

    /// A name that is not valid UTF-8 cannot be a JSON string, so it is
    /// refused at the gate rather than becoming a row that is permanently
    /// "missing".
    ///
    /// Linux only, and not because the filter is: APFS and HFS+ *validate*
    /// file names as UTF-8 and refuse `\xff` with EILSEQ, so the fixture this
    /// test needs cannot be created on macOS at all. The behaviour under test
    /// is `collect_files`' `to_str()` gate, which is platform-independent; on
    /// macOS the kernel enforces the same rule one layer lower down. Skipping
    /// rather than tolerating the write failure keeps the test honest — a
    /// silent early return would go on passing if the gate were deleted.
    #[test]
    #[cfg(all(unix, not(target_os = "macos")))]
    fn a_non_utf8_file_name_is_refused() {
        use std::ffi::OsString;
        use std::os::unix::ffi::OsStringExt;

        let root = std::env::temp_dir().join(format!("onyx-badname-{}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        let bad = root.join(OsString::from_vec(b"take\xff.wav".to_vec()));
        assert!(bad.to_str().is_none());
        std::fs::write(&bad, b"x").unwrap();
        std::fs::write(root.join("good.wav"), b"x").unwrap();

        let found = collect_files(&[root.to_string_lossy().to_string()]);
        let names: Vec<String> = found
            .iter()
            .map(|p| p.file_name().unwrap().to_string_lossy().to_string())
            .collect();
        assert_eq!(names, ["good.wav"]);
        // The direct-argument path cannot be tested the same way: by the time
        // a path is a `String` the damage is already done, which is precisely
        // why the OS-supplied argv is filtered as `OsString` in `lib.rs`.
        let _ = std::fs::remove_dir_all(&root);
    }
}
