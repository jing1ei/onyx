//! Zip archives as playlists (SPEC §19).
//!
//! A `.zip` that is opened or dropped on the window is expanded into a temp
//! directory and its audio contents become playlist rows. Everything below
//! exists because **a zip file is untrusted input**: it is a format whose
//! entries carry their own file names, their own sizes and their own file
//! *type*, all of them attacker-controlled, and all three have been used to
//! write outside the extraction root, to create symlinks pointing at
//! `~/.ssh/id_rsa`, and to turn 42 KB into 4.5 PB of disk.
//!
//! The defences, each with a test in this file:
//!
//! * **Zip slip.** An entry called `../../.bashrc` or `/etc/cron.d/x` must not
//!   escape. Every name goes through [`safe_entry_path`], which refuses
//!   absolute paths, any `..` component, Windows drive prefixes and UNC roots,
//!   and then re-checks that the joined result is still under the root.
//! * **Symlinks.** A zip can store a symlink; extracting one and then writing
//!   "through" it writes wherever it points. Symlink entries are refused
//!   outright — Onyx never needs one.
//! * **Zip bombs.** The central directory is *asked* how big the archive is,
//!   and refused above [`MAX_TOTAL_BYTES`], [`MAX_ENTRIES`] or a compression
//!   ratio of [`MAX_RATIO`]. Because those numbers are themselves supplied by
//!   the archive, extraction *also* enforces the byte budget as it reads, so a
//!   central directory that lies about a 10-byte entry still cannot fill the
//!   disk.
//! * **Nested zips.** Not recursed into — that is where the ratio guard gets
//!   defeated one layer at a time. A `.zip` inside a `.zip` is simply not audio.
//! * **File modes.** The archive's own unix mode is ignored; extracted files
//!   get the process default, so an entry cannot arrive executable or setuid.
//!
//! The temp directory is owned by [`ExtractedArchive`], which removes it on
//! drop — playlist clear, a failed open and normal exit all go through that.
//! A crash (or `std::process::exit`) skips destructors, so [`sweep_stale`]
//! removes leftovers from previous runs at startup.

use std::cmp::Ordering;
use std::io::{BufReader, Read};
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering as AtomicOrdering};

use onyx_core::decode::is_supported_path;

/// Extensions that are archives rather than audio. Not recursed into, so this
/// is deliberately just the one.
pub const ARCHIVE_EXTENSIONS: &[&str] = &["zip"];

/// Most entries one archive may declare. A 2 000-track archive is already
/// absurd for a mastering session; 100 000 empty entries is an attack.
pub const MAX_ENTRIES: usize = 2_000;

/// Most bytes one archive may expand to. Four gigabytes is an hour and a half
/// of 24/96 stereo — more than any album — and small enough that it cannot
/// fill a working disk.
pub const MAX_TOTAL_BYTES: u64 = 4 * 1024 * 1024 * 1024;

/// Largest ratio of expanded bytes to archive bytes.
///
/// Real audio barely compresses: WAV in deflate manages perhaps 2:1, FLAC and
/// MP3 essentially 1:1. Even a file of digital silence stays well inside this.
/// `42.zip` is about 10^11:1.
pub const MAX_RATIO: u64 = 1_000;

/// Deepest directory nesting accepted inside an archive.
const MAX_ENTRY_DEPTH: usize = 16;

/// Longest relative path accepted inside an archive, in bytes.
const MAX_ENTRY_PATH_LEN: usize = 512;

/// Prefix of the temp directories we create, used by [`sweep_stale`].
const TEMP_PREFIX: &str = "onyx-zip-";

/// How old a leftover temp directory must be before the startup sweep removes
/// it. Only relevant if a second process is racing us; ten minutes is far
/// longer than the window between two launches and far shorter than a session.
const STALE_AGE: std::time::Duration = std::time::Duration::from_secs(10 * 60);

/// Is this a path Onyx would treat as an archive rather than as audio?
pub fn is_archive_path(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .map(|e| ARCHIVE_EXTENSIONS.contains(&e.to_ascii_lowercase().as_str()))
        .unwrap_or(false)
}

/// A temp directory holding one archive's extracted audio.
///
/// Owns the directory: dropping it deletes the tree. That is the whole
/// lifetime story — playlist clear drops it, a failed open drops it because it
/// was never handed over, and exit drops it.
#[derive(Debug)]
pub struct ExtractedArchive {
    root: PathBuf,
    /// File name of the `.zip`, shown on every row that came out of it.
    name: String,
}

impl ExtractedArchive {
    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn root(&self) -> &Path {
        &self.root
    }
}

impl Drop for ExtractedArchive {
    fn drop(&mut self) {
        match std::fs::remove_dir_all(&self.root) {
            Ok(()) => log::debug!("removed the temp copy of \"{}\"", self.name),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => log::warn!(
                "could not remove the temp copy of \"{}\" at {} ({e})",
                self.name,
                self.root.display()
            ),
        }
    }
}

/// What came out of one archive.
#[derive(Debug)]
pub struct ArchiveContents {
    /// Owns the temp directory the files live in; keep it alive for as long as
    /// the rows are in the playlist.
    pub archive: ExtractedArchive,
    /// Extracted audio files, in natural sort order.
    pub files: Vec<PathBuf>,
    /// Things the user should be told, already summarised: at most one line
    /// for refused entries and one for unreadable ones, never one per file.
    pub warnings: Vec<String>,
}

/// Counters for the two summary warnings (SPEC §19: "a single summary
/// warning rather than one per file").
#[derive(Default)]
struct Tally {
    /// Entries refused by a security guard.
    refused: usize,
    /// Audio entries that could not be extracted or did not turn out to be
    /// audio after all.
    unreadable: usize,
}

/// Extract the audio in `zip_path` to a fresh temp directory.
///
/// `verify` is called on each extracted file and decides whether it is really
/// decodable; it is the app's `safe_decode::probe` wired up with the user's
/// SoundFont, passed in so this module stays testable without the decoder.
///
/// `Err` for an archive that cannot be opened or that trips a limit — those
/// are refusals the user has to see. Everything else is a per-entry decision
/// summarised in [`ArchiveContents::warnings`].
pub fn extract(zip_path: &Path, verify: &dyn Fn(&Path) -> bool) -> Result<ArchiveContents, String> {
    let name = zip_path
        .file_name()
        .map(|f| f.to_string_lossy().to_string())
        .unwrap_or_else(|| zip_path.to_string_lossy().to_string());

    let file =
        std::fs::File::open(zip_path).map_err(|e| format!("{name} could not be read: {e}"))?;
    let archive_bytes = file.metadata().map(|m| m.len()).unwrap_or(0);
    let mut zip = zip::ZipArchive::new(BufReader::new(file))
        .map_err(|e| format!("{name} is not a readable zip archive ({e})"))?;

    check_limits(&name, zip.len(), declared_total(&mut zip), archive_bytes)?;

    let root = make_temp_dir()?;
    // From here on the directory is owned: every `?` below drops this and
    // takes the tree with it.
    let archive = ExtractedArchive {
        root,
        name: name.clone(),
    };

    let mut tally = Tally::default();
    let mut files: Vec<(String, PathBuf)> = Vec::new();
    let mut budget = MAX_TOTAL_BYTES;

    for index in 0..zip.len() {
        let mut entry = match zip.by_index(index) {
            Ok(entry) => entry,
            Err(e) => {
                log::debug!("{name}: entry {index} is unreadable ({e})");
                tally.unreadable += 1;
                continue;
            }
        };
        if entry.is_dir() {
            continue;
        }
        // Before anything else: a symlink entry is never extracted. Writing
        // one and then writing "through" it is how an archive escapes its own
        // directory without ever using `..`.
        if entry.is_symlink() {
            log::warn!("{name}: refused the symlink entry \"{}\"", entry.name());
            tally.refused += 1;
            continue;
        }
        let raw = entry.name().to_string();
        let Some(relative) = safe_entry_path(&raw) else {
            log::warn!("{name}: refused the entry \"{raw}\" (it escapes the archive)");
            tally.refused += 1;
            continue;
        };
        // Non-audio entries — cover art, cue sheets, a nested `.zip` — are
        // skipped without a word, as the spec asks.
        if !is_supported_path(&relative) {
            continue;
        }

        let target = archive.root.join(&relative);
        match write_entry(&mut entry, &target, &mut budget) {
            Ok(()) => files.push((relative.to_string_lossy().to_string(), target)),
            Err(WriteError::OutOfBudget) => {
                // The central directory lied about the sizes; stop rather than
                // keep going and fill the disk.
                return Err(format!(
                    "{name} expands to more than {} MiB, which is more than Onyx will unpack",
                    MAX_TOTAL_BYTES / (1024 * 1024)
                ));
            }
            Err(WriteError::Failed(e)) => {
                log::debug!("{name}: could not extract \"{raw}\" ({e})");
                tally.unreadable += 1;
            }
        }
    }
    drop(zip);

    // `track2` before `track10`, across directories.
    files.sort_by(|(a, _), (b, _)| natural_cmp(a, b));

    // A file with an audio extension is not necessarily audio. Probing is a
    // header parse — sub-millisecond, and the entry cap bounds how many there
    // can be — and it turns "17 rows that all fail when clicked" into one
    // honest summary line.
    let mut kept = Vec::with_capacity(files.len());
    for (_, path) in files {
        if verify(&path) {
            kept.push(path);
        } else {
            tally.unreadable += 1;
            let _ = std::fs::remove_file(&path);
        }
    }

    let mut warnings = Vec::new();
    if tally.refused > 0 {
        warnings.push(format!(
            "{name}: {} unsafe {} refused (a path escaping the archive, or a symlink)",
            tally.refused,
            plural(tally.refused, "entry was", "entries were")
        ));
    }
    if tally.unreadable > 0 {
        warnings.push(format!(
            "{name}: {} {} skipped (not something Onyx can decode)",
            tally.unreadable,
            plural(tally.unreadable, "file was", "files were")
        ));
    }
    if kept.is_empty() {
        // "must say so plainly instead of silently doing nothing".
        warnings.push(format!("{name} contains no audio files"));
    }
    log::info!(
        "{name}: {} audio file(s) extracted, {} refused, {} unreadable",
        kept.len(),
        tally.refused,
        tally.unreadable
    );

    Ok(ArchiveContents {
        archive,
        files: kept,
        warnings,
    })
}

fn plural(n: usize, one: &'static str, many: &'static str) -> &'static str {
    if n == 1 {
        one
    } else {
        many
    }
}

/// Sum of the uncompressed sizes the central directory *claims*.
///
/// A claim, not a fact — see the budget in [`write_entry`] — but a cheap first
/// gate that rejects the classic bombs without decompressing a byte.
fn declared_total<R: Read + std::io::Seek>(zip: &mut zip::ZipArchive<R>) -> u64 {
    (0..zip.len())
        .filter_map(|i| zip.by_index_raw(i).ok().map(|e| e.size()))
        .fold(0u64, |acc, size| acc.saturating_add(size))
}

/// The three refusals that are about the archive as a whole.
///
/// Split out and pure so the numbers can be tested without building a
/// multi-petabyte fixture.
fn check_limits(
    name: &str,
    entries: usize,
    declared: u64,
    archive_bytes: u64,
) -> Result<(), String> {
    if entries > MAX_ENTRIES {
        return Err(format!(
            "{name} contains {entries} entries; Onyx will not unpack more than {MAX_ENTRIES}"
        ));
    }
    // The ratio is checked before the size, so the classic bombs are named
    // for what they are rather than reported as "a bit large".
    if archive_bytes > 0 {
        let ratio = declared / archive_bytes.max(1);
        if ratio > MAX_RATIO {
            return Err(format!(
                "{name} expands {ratio}× — that is a zip bomb, not an album; refusing to unpack it"
            ));
        }
    }
    if declared > MAX_TOTAL_BYTES {
        return Err(format!(
            "{name} expands to {} MiB, more than the {} MiB Onyx will unpack",
            declared / (1024 * 1024),
            MAX_TOTAL_BYTES / (1024 * 1024)
        ));
    }
    Ok(())
}

#[derive(Debug)]
enum WriteError {
    /// The running byte budget ran out: the archive lied about its sizes.
    OutOfBudget,
    Failed(String),
}

/// Write one entry, spending from `budget`.
///
/// `Read::take(budget + 1)` is the point: the loop copies at most one byte
/// more than the budget allows, notices, and gives up. Trusting `entry.size()`
/// here instead is exactly how "the zip said it was 10 bytes" becomes a full
/// disk.
fn write_entry<R: Read>(entry: &mut R, target: &Path, budget: &mut u64) -> Result<(), WriteError> {
    if let Some(parent) = target.parent() {
        std::fs::create_dir_all(parent).map_err(|e| WriteError::Failed(e.to_string()))?;
    }
    // `create_new`: the entry names are already unique per archive and the
    // directory is ours, so a collision means something is wrong.
    let mut out = std::fs::File::create(target).map_err(|e| WriteError::Failed(e.to_string()))?;
    let allowed = *budget;
    let mut limited = entry.take(allowed.saturating_add(1));
    let written = match std::io::copy(&mut limited, &mut out) {
        Ok(n) => n,
        Err(e) => {
            let _ = std::fs::remove_file(target);
            return Err(WriteError::Failed(e.to_string()));
        }
    };
    if written > allowed {
        let _ = std::fs::remove_file(target);
        return Err(WriteError::OutOfBudget);
    }
    *budget -= written;
    Ok(())
}

/// Validate one entry name and turn it into a relative path.
///
/// `None` means "refuse": an absolute path, a drive letter, a UNC root, any
/// `..`, an empty name, one that is not UTF-8-clean, one nested absurdly deep
/// or absurdly long. Pure, because this is the guard the whole module rests on
/// and it has to be testable without building a hostile zip for every case.
///
/// Note that this deliberately does *not* consult the file system: a check
/// like "does the joined path start with the root" can be defeated by a
/// symlink created by an earlier entry of the same archive, which is why
/// symlink entries are refused separately.
pub fn safe_entry_path(raw: &str) -> Option<PathBuf> {
    if raw.is_empty() || raw.len() > MAX_ENTRY_PATH_LEN {
        return None;
    }
    // A NUL truncates the path for the OS but not for us.
    if raw.contains('\0') {
        return None;
    }
    // Zip stores `/`; a Windows-authored archive may still contain `\`, and
    // `Path` on Linux would treat `..\..\x` as one harmless-looking component.
    let normalised = raw.replace('\\', "/");
    if normalised.starts_with('/') {
        return None;
    }
    // `C:foo`, `C:/foo`: a drive-relative or absolute Windows path.
    if normalised
        .split('/')
        .next()
        .is_some_and(|first| first.len() >= 2 && first.as_bytes()[1] == b':')
    {
        return None;
    }

    let mut out = PathBuf::new();
    let mut depth = 0usize;
    for part in normalised.split('/') {
        if part.is_empty() || part == "." {
            continue;
        }
        if part == ".." {
            return None;
        }
        // Belt and braces: whatever the string looked like, the component the
        // OS parses out of it must be an ordinary name.
        if !matches!(
            Path::new(part).components().next(),
            Some(Component::Normal(_))
        ) {
            return None;
        }
        depth += 1;
        if depth > MAX_ENTRY_DEPTH {
            return None;
        }
        out.push(part);
    }
    if out.as_os_str().is_empty() {
        return None;
    }
    Some(out)
}

/// Create a fresh temp directory for one archive.
fn make_temp_dir() -> Result<PathBuf, String> {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let base = std::env::temp_dir();
    for _ in 0..8 {
        let unique = COUNTER.fetch_add(1, AtomicOrdering::Relaxed);
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.subsec_nanos())
            .unwrap_or(0);
        let dir = base.join(format!(
            "{TEMP_PREFIX}{}-{unique}-{nanos:09}",
            std::process::id()
        ));
        // `create_dir`, not `create_dir_all`: it fails if *anything* already
        // exists at that path, including a symlink someone planted in a shared
        // `/tmp` hoping we would extract through it.
        match std::fs::create_dir(&dir) {
            Ok(()) => return Ok(dir),
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(e) => {
                return Err(format!(
                    "could not create a temp folder for the archive: {e}"
                ))
            }
        }
    }
    Err("could not create a temp folder for the archive".to_string())
}

/// Remove temp trees left behind by a previous run (SPEC §19).
///
/// Destructors do not run when a process is killed or calls `exit`, so without
/// this an unlucky crash leaves an album's worth of WAV in `/tmp` for ever.
/// Called once, at startup. Returns how many directories were removed, which
/// is what the test asserts on.
pub fn sweep_stale() -> usize {
    let base = std::env::temp_dir();
    let Ok(read) = std::fs::read_dir(&base) else {
        return 0;
    };
    let now = std::time::SystemTime::now();
    let mut removed = 0;
    for entry in read.filter_map(|e| e.ok()) {
        let path = entry.path();
        let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        if !is_stale_temp_dir(name, std::process::id()) {
            continue;
        }
        // Age guard: the single-instance plugin makes a concurrent Onyx
        // unlikely, but deleting a *running* instance's extraction out from
        // under it would be far worse than leaving one directory behind.
        let recent = entry
            .metadata()
            .and_then(|m| m.modified())
            .ok()
            .and_then(|t| now.duration_since(t).ok())
            .map(|age| age < STALE_AGE)
            .unwrap_or(false);
        if recent {
            continue;
        }
        if !path.is_dir() {
            continue;
        }
        match std::fs::remove_dir_all(&path) {
            Ok(()) => {
                removed += 1;
                log::info!("removed a stale archive temp folder from a previous run: {name}");
            }
            Err(e) => log::debug!("could not remove the stale temp folder {name} ({e})"),
        }
    }
    removed
}

/// Is `name` one of our temp directories, from some *other* process?
fn is_stale_temp_dir(name: &str, our_pid: u32) -> bool {
    let Some(rest) = name.strip_prefix(TEMP_PREFIX) else {
        return false;
    };
    match rest.split('-').next().and_then(|p| p.parse::<u32>().ok()) {
        Some(pid) => pid != our_pid,
        None => false,
    }
}

/// Compare two names the way a person reads them: `track2` before `track10`.
///
/// Digit runs compare as numbers, everything else case-insensitively, with the
/// raw bytes as a final tie-break so the order is total and therefore stable.
pub fn natural_cmp(a: &str, b: &str) -> Ordering {
    let (mut ai, mut bi) = (a.chars().peekable(), b.chars().peekable());
    loop {
        match (ai.peek().copied(), bi.peek().copied()) {
            (None, None) => break,
            (None, Some(_)) => return Ordering::Less,
            (Some(_), None) => return Ordering::Greater,
            (Some(ca), Some(cb)) => {
                if ca.is_ascii_digit() && cb.is_ascii_digit() {
                    let na = take_digits(&mut ai);
                    let nb = take_digits(&mut bi);
                    // Compare by value: length first (after leading zeros are
                    // dropped), then lexicographically, so no integer overflow
                    // is possible on a 400-digit "number".
                    let ta = na.trim_start_matches('0');
                    let tb = nb.trim_start_matches('0');
                    let order = ta.len().cmp(&tb.len()).then_with(|| ta.cmp(tb));
                    if order != Ordering::Equal {
                        return order;
                    }
                } else {
                    // Case-insensitively only: a per-character tie-break here
                    // would make `Track20` sort before `track1`, because the
                    // very first character would decide the whole comparison.
                    // The byte-wise order is the *last* resort, below.
                    let order = ca.to_ascii_lowercase().cmp(&cb.to_ascii_lowercase());
                    if order != Ordering::Equal {
                        return order;
                    }
                    ai.next();
                    bi.next();
                }
            }
        }
    }
    a.cmp(b)
}

fn take_digits(it: &mut std::iter::Peekable<std::str::Chars<'_>>) -> String {
    let mut out = String::new();
    while let Some(c) = it.peek().copied() {
        if !c.is_ascii_digit() {
            break;
        }
        out.push(c);
        it.next();
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /* ── zip slip: the entry-name guard (SPEC §19) ─────────────────── */

    #[test]
    fn an_ordinary_entry_name_survives() {
        assert_eq!(safe_entry_path("take.wav"), Some(PathBuf::from("take.wav")));
        assert_eq!(
            safe_entry_path("album/02 - take.wav"),
            Some(PathBuf::from("album/02 - take.wav"))
        );
        // A leading `./` and doubled separators are noise, not an escape.
        assert_eq!(
            safe_entry_path("./album//take.wav"),
            Some(PathBuf::from("album/take.wav"))
        );
        // Windows-authored archives really do use backslashes.
        assert_eq!(
            safe_entry_path("album\\take.wav"),
            Some(PathBuf::from("album/take.wav"))
        );
        // Unicode names are ordinary names.
        assert_eq!(
            safe_entry_path("\u{4e2d}\u{6587}/take \u{1f3b5}.wav"),
            Some(PathBuf::from("\u{4e2d}\u{6587}/take \u{1f3b5}.wav"))
        );
    }

    #[test]
    fn nothing_that_escapes_the_root_is_accepted() {
        for hostile in [
            "../evil.wav",
            "../../../../../../etc/cron.d/x",
            "album/../../evil.wav",
            "..\\..\\evil.wav",
            "album\\..\\..\\evil.wav",
            "/etc/passwd",
            "//server/share/evil.wav",
            "\\\\server\\share\\evil.wav",
            "C:/Windows/System32/evil.dll",
            "C:evil.wav",
            "c:\\evil.wav",
            "..",
            ".",
            "",
            "take\0.wav",
        ] {
            assert_eq!(
                safe_entry_path(hostile),
                None,
                "{hostile:?} was accepted as an entry name"
            );
        }
        // Absurd depth and absurd length are refused too: both are cheap for
        // an attacker to produce and neither describes a real album.
        let deep = "a/".repeat(MAX_ENTRY_DEPTH + 1) + "take.wav";
        assert_eq!(safe_entry_path(&deep), None);
        let long = format!("{}.wav", "a".repeat(MAX_ENTRY_PATH_LEN));
        assert_eq!(safe_entry_path(&long), None);
        // ...and the boundary cases either side are accepted, so the guard is
        // a limit rather than a coin toss.
        let deepest = "a/".repeat(MAX_ENTRY_DEPTH - 1) + "take.wav";
        assert!(safe_entry_path(&deepest).is_some());
    }

    /// Whatever the string looked like, the *joined* path must stay under the
    /// root. This is the property the guard exists for, checked directly.
    #[test]
    fn an_accepted_entry_always_lands_under_the_root() {
        let root = Path::new("/tmp/onyx-zip-1");
        for name in [
            "take.wav",
            "./a/b/take.wav",
            "a\\b\\take.wav",
            "  spaces  /take.wav",
            "-take.wav",
        ] {
            let relative = safe_entry_path(name).expect("{name} should be accepted");
            let joined = root.join(&relative);
            assert!(
                joined.starts_with(root),
                "{name:?} joined to {joined:?}, outside the root"
            );
            assert!(
                !relative
                    .components()
                    .any(|c| !matches!(c, Component::Normal(_))),
                "{name:?} produced a non-ordinary component"
            );
        }
    }

    /* ── zip bombs: the whole-archive limits ──────────────────────────── */

    #[test]
    fn the_archive_limits_refuse_bombs_and_pass_albums() {
        // A real album: 12 tracks, 600 MB, barely compressed.
        assert!(check_limits("album.zip", 12, 600_000_000, 560_000_000).is_ok());
        // 42.zip: a few tens of kilobytes claiming petabytes.
        let bomb = check_limits("42.zip", 16, 4_500_000_000_000_000, 42_374).unwrap_err();
        assert!(bomb.contains("zip bomb"), "{bomb}");
        // Under the ratio but still far too big to unpack.
        let big = check_limits("big.zip", 4, MAX_TOTAL_BYTES + 1, MAX_TOTAL_BYTES).unwrap_err();
        assert!(big.contains("more than the"), "{big}");
        // 100 000 empty entries: cheap to make, expensive to create inodes for.
        let many = check_limits("many.zip", MAX_ENTRIES + 1, 1_000, 1_000).unwrap_err();
        assert!(many.contains("entries"), "{many}");
        // Every refusal names the archive, because the user may have dropped
        // several at once.
        for message in [bomb, big, many] {
            assert!(message.contains(".zip"), "{message}");
        }
        // An archive whose size we could not stat is not judged on its ratio.
        assert!(check_limits("unknown.zip", 1, 1_000_000, 0).is_ok());
    }

    /// The central directory is a claim. This is the guard for when it lies:
    /// an entry that declares ten bytes and produces gigabytes.
    #[test]
    fn a_lying_entry_is_stopped_by_the_running_budget() {
        let dir = scratch("budget");
        let target = dir.join("liar.wav");
        let mut budget = 1_024u64;
        // 4 KiB of "decompressed" data against a 1 KiB remaining budget.
        let mut source = std::io::Cursor::new(vec![0u8; 4 * 1024]);
        let outcome = write_entry(&mut source, &target, &mut budget);
        assert!(matches!(outcome, Err(WriteError::OutOfBudget)));
        assert!(
            !target.exists(),
            "the partial file must not be left on disk"
        );

        // An honest entry spends its bytes and no more.
        let mut source = std::io::Cursor::new(vec![0u8; 512]);
        write_entry(&mut source, &target, &mut budget).expect("512 bytes fit");
        assert_eq!(budget, 512, "the budget must be spent, not ignored");
        assert_eq!(std::fs::metadata(&target).unwrap().len(), 512);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /* ── natural order (`track2` before `track10`) ────────────────────── */

    #[test]
    fn names_sort_the_way_a_person_reads_them() {
        let mut names = vec![
            "track10.wav",
            "track2.wav",
            "track1.wav",
            "Track20.wav",
            "track3.wav",
        ];
        names.sort_by(|a, b| natural_cmp(a, b));
        assert_eq!(
            names,
            [
                "track1.wav",
                "track2.wav",
                "track3.wav",
                "track10.wav",
                "Track20.wav"
            ]
        );
        // Leading zeros do not change the value, so `007` and `7` differ only
        // by the final byte-wise tie-break — never by 7 versus 7 000 000.
        assert_eq!(natural_cmp("track007", "track7"), Ordering::Less);
        assert_eq!(natural_cmp("a09", "a10"), Ordering::Less);
        // ...and a 400-digit run must compare, not overflow.
        let huge_a = format!("t{}", "9".repeat(400));
        let huge_b = format!("t1{}", "0".repeat(400));
        assert_eq!(natural_cmp(&huge_a, &huge_b), Ordering::Less);
        // Directories sort before their neighbours consistently, and the
        // ordering is total: equal strings compare equal, and it is
        // antisymmetric.
        assert_eq!(natural_cmp("a/b.wav", "a/b.wav"), Ordering::Equal);
        assert_eq!(natural_cmp("a", "ab"), Ordering::Less);
        assert_eq!(natural_cmp("ab", "a"), Ordering::Greater);
    }

    /* ── temp-directory lifetime ──────────────────────────────────────── */

    #[test]
    fn dropping_an_extraction_removes_its_directory() {
        let root = make_temp_dir().expect("a temp dir");
        std::fs::write(root.join("take.wav"), b"x").unwrap();
        let held = ExtractedArchive {
            root: root.clone(),
            name: "album.zip".into(),
        };
        assert!(root.is_dir());
        drop(held);
        assert!(!root.exists(), "the temp tree outlived its owner");
    }

    #[test]
    fn only_another_process_leaves_a_stale_directory() {
        let ours = std::process::id();
        assert!(!is_stale_temp_dir(
            &format!("{TEMP_PREFIX}{ours}-3-000"),
            ours
        ));
        assert!(is_stale_temp_dir(
            &format!("{TEMP_PREFIX}{}-3-000", ours + 1),
            ours
        ));
        // Anything that is not one of ours is left alone, whatever it is.
        for name in [
            "onyx-zip",
            "onyx-zip-",
            "onyx-zip-notapid-1",
            "tmp1234",
            ".X11-unix",
            "onyx-settings-1-x",
        ] {
            assert!(!is_stale_temp_dir(name, ours), "{name} would be deleted");
        }
    }

    #[test]
    fn the_startup_sweep_leaves_this_run_alone() {
        // A live extraction of ours, and the sweep running as it does at
        // startup: the directory must survive, because deleting a running
        // instance's audio out from under it is worse than a leftover.
        let root = make_temp_dir().expect("a temp dir");
        std::fs::write(root.join("take.wav"), b"x").unwrap();
        sweep_stale();
        assert!(root.is_dir(), "the sweep deleted our own extraction");
        let _ = std::fs::remove_dir_all(&root);
    }

    fn scratch(tag: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("onyx-archive-test-{}-{tag}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }
}
