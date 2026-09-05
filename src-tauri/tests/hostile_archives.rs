//! The hostile-archive corpus (SPEC §19).
//!
//! `src/archive.rs` unit-tests its guards one pure function at a time — the
//! entry-name grammar, the limit arithmetic, the byte budget. That is not the
//! same as proving the guards are *wired up*: a `safe_entry_path` that refuses
//! `../evil.wav` is worthless if the extraction loop never calls it, and every
//! one of those unit tests would still pass. So this file builds real zip files,
//! hands them to the real [`onyx_lib::archive::extract`] — the same entry point
//! `loader::open_paths` uses — and asserts on what ends up on disk.
//!
//! Kept out of `src/` deliberately: an integration test can only reach what the
//! crate makes public, which is exactly the surface the app itself drives.
//!
//! Nothing here needs an audio device, a decoder or a window. `verify` (the
//! decodability probe the app passes in as `safe_decode::probe`) is a closure,
//! so "is this really audio" is decided by the test rather than by Symphonia.

use std::io::{Cursor, Write};
use std::path::{Path, PathBuf};

use onyx_lib::archive::{self, MAX_ENTRIES};
use zip::write::SimpleFileOptions;
use zip::{CompressionMethod, ZipWriter};

/* ── building hostile archives ───────────────────────────────────────────── */

/// A scratch directory of our own, removed and recreated per test.
fn scratch(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("onyx-hostile-{}-{tag}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("a scratch dir");
    dir
}

/// Write a zip made of `(name, bytes)` pairs, deflated.
fn zip_of(dir: &Path, file: &str, entries: &[(&str, &[u8])]) -> PathBuf {
    let path = dir.join(file);
    let mut zip = ZipWriter::new(Cursor::new(Vec::new()));
    let options = SimpleFileOptions::default().compression_method(CompressionMethod::Deflated);
    for (name, bytes) in entries {
        zip.start_file(*name, options).expect("an entry");
        zip.write_all(bytes).expect("entry bytes");
    }
    let bytes = zip.finish().expect("a finished zip").into_inner();
    std::fs::write(&path, bytes).expect("the zip on disk");
    path
}

/// Enough of a RIFF header that nothing has to guess; the contents are never
/// decoded here, only counted.
fn wav(seconds: u8) -> Vec<u8> {
    let mut out = b"RIFF....WAVEfmt ".to_vec();
    out.extend(std::iter::repeat_n(0u8, 64 * seconds as usize));
    out
}

/// Accept everything as decodable — the archive guards are what is under test.
fn all_audio(_: &Path) -> bool {
    true
}

/// Relative paths of what was extracted, in the order `extract` returned them.
fn names(contents: &archive::ArchiveContents) -> Vec<String> {
    contents
        .files
        .iter()
        .map(|p| {
            p.strip_prefix(contents.archive.root())
                .unwrap_or(p)
                .to_string_lossy()
                .replace('\\', "/")
        })
        .collect()
}

/* ── the happy path, so the refusals below mean something ────────────────── */

#[test]
fn an_album_extracts_in_natural_order_and_skips_what_is_not_audio() {
    let dir = scratch("album");
    let inner = zip_of(&dir, "inner.zip", &[("take.wav", &wav(1))]);
    let nested = std::fs::read(&inner).expect("the inner zip");
    let path = zip_of(
        &dir,
        "album.zip",
        &[
            ("album/track10.wav", &wav(1)),
            ("album/track2.wav", &wav(1)),
            ("album/track1.wav", &wav(1)),
            ("album/cover.jpg", b"not audio"),
            ("album/notes.txt", b"cue sheet"),
            // A zip inside a zip is not recursed into: it is simply not audio.
            ("album/more.zip", &nested),
        ],
    );

    let contents = archive::extract(&path, &all_audio).expect("a readable album");
    assert_eq!(
        names(&contents),
        ["album/track1.wav", "album/track2.wav", "album/track10.wav"],
        "natural order, and only the audio"
    );
    // Silently skipped, as the spec asks: no warning for cover art or a cue
    // sheet, and none for the nested archive either.
    assert!(
        contents.warnings.is_empty(),
        "unexpected warnings: {:?}",
        contents.warnings
    );
    assert_eq!(contents.archive.name(), "album.zip");
    for file in &contents.files {
        assert!(file.is_file(), "{file:?} was not written");
        assert!(
            file.starts_with(contents.archive.root()),
            "{file:?} is outside the extraction root"
        );
    }
    // Nothing was recursed into: the inner archive is not on disk at all.
    assert!(!contents.archive.root().join("album/more.zip").exists());

    // The temp tree is owned by the extraction and dies with it.
    let root = contents.archive.root().to_path_buf();
    drop(contents);
    assert!(!root.exists(), "the temp tree outlived the extraction");
    let _ = std::fs::remove_dir_all(&dir);
}

/* ── zip slip ────────────────────────────────────────────────────────────── */

#[test]
fn nothing_escapes_the_extraction_root() {
    let dir = scratch("slip");
    let path = zip_of(
        &dir,
        "slip.zip",
        &[
            ("../escaped.wav", &wav(1)),
            ("album/../../escaped-too.wav", &wav(1)),
            ("..\\windows-escaped.wav", &wav(1)),
            ("/absolute.wav", &wav(1)),
            ("C:/drive.wav", &wav(1)),
            ("good.wav", &wav(1)),
        ],
    );

    let contents = archive::extract(&path, &all_audio).expect("the archive itself is readable");
    // The one honest entry is kept; the five escapes are refused.
    assert_eq!(names(&contents), ["good.wav"]);
    assert_eq!(contents.warnings.len(), 1, "{:?}", contents.warnings);
    let warning = &contents.warnings[0];
    assert!(warning.contains("slip.zip"), "{warning}");
    assert!(
        warning.contains('5'),
        "one summary line, five entries: {warning}"
    );

    // The point of the exercise: nothing was written beside the root, which is
    // where `..` from inside it lands.
    let root = contents.archive.root();
    let beside = root.parent().expect("a temp parent");
    for escaped in ["escaped.wav", "escaped-too.wav", "windows-escaped.wav"] {
        assert!(
            !beside.join(escaped).exists(),
            "{escaped} was written outside the root"
        );
    }
    // ...and everything that *was* written is under the root, one level deep.
    let written: Vec<PathBuf> = std::fs::read_dir(root)
        .expect("the root")
        .filter_map(|e| e.ok().map(|e| e.path()))
        .collect();
    assert_eq!(written.len(), 1, "{written:?}");
    let _ = std::fs::remove_dir_all(&dir);
}

/* ── symlink entries ─────────────────────────────────────────────────────── */

#[test]
fn a_symlink_entry_is_refused_rather_than_followed() {
    let dir = scratch("symlink");
    let secret = dir.join("secret.wav");
    std::fs::write(&secret, wav(1)).expect("a file to point at");

    let path = dir.join("symlink.zip");
    let mut zip = ZipWriter::new(Cursor::new(Vec::new()));
    let options = SimpleFileOptions::default().compression_method(CompressionMethod::Deflated);
    // A symlink whose target is a real file outside the archive. Extracting it
    // and writing "through" it is how an archive escapes without a `..`.
    zip.add_symlink("link.wav", secret.to_string_lossy(), options)
        .expect("a symlink entry");
    zip.start_file("real.wav", options).expect("an entry");
    zip.write_all(&wav(1)).expect("entry bytes");
    let bytes = zip.finish().expect("a finished zip").into_inner();
    std::fs::write(&path, bytes).expect("the zip on disk");

    let contents = archive::extract(&path, &all_audio).expect("readable");
    assert_eq!(names(&contents), ["real.wav"]);
    assert_eq!(contents.warnings.len(), 1, "{:?}", contents.warnings);
    assert!(
        contents.warnings[0].contains("symlink"),
        "the refusal must say why: {:?}",
        contents.warnings
    );
    let link = contents.archive.root().join("link.wav");
    assert!(!link.exists(), "the symlink was extracted");
    assert!(
        std::fs::symlink_metadata(&link).is_err(),
        "a dangling link was left behind"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/* ── bombs ───────────────────────────────────────────────────────────────── */

/// A real bomb, built rather than described: 32 MiB of zeros, which deflate
/// squeezes past the ratio guard's 1 000:1. Refused from the central directory,
/// before a byte is written — so the assertion is that no temp tree was made.
#[test]
fn a_compression_bomb_is_refused_before_anything_is_unpacked() {
    let dir = scratch("bomb");
    let zeros = vec![0u8; 32 * 1024 * 1024];
    let path = zip_of(&dir, "42.zip", &[("bomb.wav", &zeros)]);
    let on_disk = std::fs::metadata(&path).expect("the bomb").len();
    let ratio = zeros.len() as u64 / on_disk.max(1);
    assert!(
        ratio > 1_000,
        "the fixture is not a bomb: {ratio}× ({on_disk} bytes)"
    );

    let before = temp_dirs_of_ours();
    let error = archive::extract(&path, &all_audio).expect_err("a bomb must be refused");
    assert!(error.contains("zip bomb"), "{error}");
    assert!(
        error.contains("42.zip"),
        "the refusal must name it: {error}"
    );
    assert_eq!(
        temp_dirs_of_ours(),
        before,
        "a refused archive must not leave a temp directory behind"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn an_archive_with_absurdly_many_entries_is_refused() {
    let dir = scratch("many");
    let path = dir.join("many.zip");
    let mut zip = ZipWriter::new(Cursor::new(Vec::new()));
    let options = SimpleFileOptions::default().compression_method(CompressionMethod::Deflated);
    for i in 0..=MAX_ENTRIES {
        zip.start_file(format!("t{i}.wav"), options)
            .expect("an entry");
        zip.write_all(b"RIFF").expect("entry bytes");
    }
    let bytes = zip.finish().expect("a finished zip").into_inner();
    std::fs::write(&path, bytes).expect("the zip on disk");

    let error = archive::extract(&path, &all_audio).expect_err("too many entries");
    assert!(error.contains("many.zip"), "{error}");
    assert!(
        error.contains(&MAX_ENTRIES.to_string()),
        "the limit has to be in the message: {error}"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/* ── malformed and empty archives ────────────────────────────────────────── */

#[test]
fn garbage_that_calls_itself_a_zip_is_an_error_not_a_panic() {
    let dir = scratch("garbage");
    for (name, bytes) in [
        ("empty.zip", Vec::new()),
        ("text.zip", b"this is not a zip file at all".to_vec()),
        // A plausible local header and then nothing.
        (
            "truncated.zip",
            b"PK\x03\x04\x14\x00\x00\x00\x08\x00".to_vec(),
        ),
        ("nul.zip", vec![0u8; 512]),
    ] {
        let path = dir.join(name);
        std::fs::write(&path, &bytes).expect("the fixture");
        let error = archive::extract(&path, &all_audio)
            .expect_err(&format!("{name} is not a readable archive"));
        assert!(
            error.contains(name),
            "the message must name the file: {error}"
        );
    }
    // A missing file is a refusal too, not a panic.
    let error = archive::extract(&dir.join("gone.zip"), &all_audio).expect_err("no such file");
    assert!(error.contains("gone.zip"), "{error}");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn an_archive_with_no_audio_in_it_says_so() {
    let dir = scratch("audio-free");
    // Structurally valid, and completely useless to a player.
    let path = zip_of(
        &dir,
        "photos.zip",
        &[("a.jpg", b"jpeg"), ("notes/readme.txt", b"text")],
    );
    let contents = archive::extract(&path, &all_audio).expect("readable");
    assert!(contents.files.is_empty());
    assert_eq!(contents.warnings.len(), 1, "{:?}", contents.warnings);
    assert!(
        contents.warnings[0].contains("no audio files"),
        "{:?}",
        contents.warnings
    );

    // An archive with no entries at all reads the same way.
    let empty = zip_of(&dir, "nothing.zip", &[]);
    let contents = archive::extract(&empty, &all_audio).expect("readable");
    assert!(contents.files.is_empty());
    assert!(contents.warnings[0].contains("no audio files"));
    let _ = std::fs::remove_dir_all(&dir);
}

/// The verification pass: entries with an audio extension that are not audio.
/// One summary line for all of them, and the files are not left in the temp
/// tree to be clicked on.
#[test]
fn files_that_only_look_like_audio_are_summarised_once() {
    let dir = scratch("undecodable");
    let path = zip_of(
        &dir,
        "mixed.zip",
        &[
            ("real.wav", &wav(1)),
            ("fake1.wav", b"not audio at all"),
            ("fake2.flac", b"nor this"),
            ("fake3.mp3", b"nor this either"),
        ],
    );
    let contents = archive::extract(&path, &|p: &Path| {
        p.file_name().is_some_and(|n| n == "real.wav")
    })
    .expect("readable");
    assert_eq!(names(&contents), ["real.wav"]);
    assert_eq!(
        contents.warnings.len(),
        1,
        "one line, not one per file: {:?}",
        contents.warnings
    );
    assert!(
        contents.warnings[0].contains('3'),
        "{:?}",
        contents.warnings
    );
    assert!(
        !contents.archive.root().join("fake1.wav").exists(),
        "an undecodable file was left in the temp tree"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// Deep nesting and absurd names are refused by the same summary path, and the
/// directories that *are* accepted are walked.
#[test]
fn nested_directories_are_walked_and_absurd_ones_refused() {
    let dir = scratch("nesting");
    let deep = format!("{}take.wav", "d/".repeat(40));
    let long = format!("{}.wav", "n".repeat(600));
    let path = zip_of(
        &dir,
        "nested.zip",
        &[
            ("disc 2/track 1.wav", &wav(1)),
            ("disc 1/track 2.wav", &wav(1)),
            ("disc 1/track 10.wav", &wav(1)),
            ("disc 1/bonus/track 1.wav", &wav(1)),
            (deep.as_str(), &wav(1)),
            (long.as_str(), &wav(1)),
        ],
    );
    let contents = archive::extract(&path, &all_audio).expect("readable");
    assert_eq!(
        names(&contents),
        [
            "disc 1/bonus/track 1.wav",
            "disc 1/track 2.wav",
            "disc 1/track 10.wav",
            "disc 2/track 1.wav"
        ]
    );
    assert_eq!(contents.warnings.len(), 1, "{:?}", contents.warnings);
    assert!(
        contents.warnings[0].contains('2'),
        "{:?}",
        contents.warnings
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// Our own temp directories, so a refusal can be shown to have created none.
fn temp_dirs_of_ours() -> Vec<String> {
    let prefix = format!("onyx-zip-{}-", std::process::id());
    let Ok(read) = std::fs::read_dir(std::env::temp_dir()) else {
        return Vec::new();
    };
    let mut out: Vec<String> = read
        .filter_map(|e| e.ok())
        .filter_map(|e| e.file_name().into_string().ok())
        .filter(|n| n.starts_with(&prefix))
        .collect();
    out.sort();
    out
}
