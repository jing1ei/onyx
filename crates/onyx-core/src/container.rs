//! Container detection by content (SPEC §17).
//!
//! Symphonia already probes by content — its `Hint` is explicitly documented as
//! an optimisation that "won't lead the probe astray" — so the decoder does not
//! need this module to pick a demuxer. Two other things do:
//!
//! * **MIDI** (§18) is not a Symphonia container at all. It has to be
//!   recognised before the file reaches the probe, and it has to be recognised
//!   by content, because `.mid` is not the only name a MIDI file ever has.
//! * **The badge the UI shows.** "`MP4 · AAC`" has to be honest, and the file
//!   extension is not evidence: a `.wav` that is really an MP3 must read as
//!   `MP3`, not as `WAV`.
//!
//! Everything here works on a byte prefix, is bounds-checked, and returns
//! `None` rather than guessing. Nothing in this module can panic on hostile
//! input — the malformed-input corpus in `tests/` leans on that.

/// How many bytes of the file the sniffer wants. Enough for an ID3v2 tag of
/// typical size plus the frame header that follows it.
pub const SNIFF_BYTES: usize = 8 * 1024;

/// A container Onyx can name. This is a *labelling* type: the decoder is still
/// Symphonia's, and an [`Unknown`](Container::Unknown) file is handed to the
/// probe exactly like any other.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Container {
    Wav,
    Aiff,
    Caf,
    Flac,
    /// Ogg: Vorbis, Opus or FLAC inside.
    Ogg,
    /// ISO base media, MPEG-4 branded.
    Mp4,
    /// ISO base media, QuickTime branded.
    Mov,
    Matroska,
    WebM,
    /// A bare MPEG audio stream (layer I/II/III), with or without an ID3 tag.
    Mpeg,
    /// A bare AAC stream (ADTS or ADIF framing).
    Aac,
    /// A Standard MIDI File — rendered, not decoded (§18).
    Midi,
}

impl Container {
    /// Short upper-case label for the UI, e.g. the `MP4` of `MP4 · AAC`.
    pub fn label(self) -> &'static str {
        match self {
            Container::Wav => "WAV",
            Container::Aiff => "AIFF",
            Container::Caf => "CAF",
            Container::Flac => "FLAC",
            Container::Ogg => "OGG",
            Container::Mp4 => "MP4",
            Container::Mov => "MOV",
            Container::Matroska => "MKV",
            Container::WebM => "WEBM",
            Container::Mpeg => "MPEG",
            Container::Aac => "AAC",
            Container::Midi => "MIDI",
        }
    }

    /// True for the containers that carry video as well as audio, where the
    /// decoder has to pick the first audio track and ignore the picture.
    pub fn may_carry_video(self) -> bool {
        matches!(
            self,
            Container::Mp4 | Container::Mov | Container::Matroska | Container::WebM
        )
    }

    /// True when this is a MIDI file, which is *rendered* rather than decoded.
    pub fn is_midi(self) -> bool {
        matches!(self, Container::Midi)
    }
}

/// Identify the container from a prefix of the file.
///
/// `buf` should be the first [`SNIFF_BYTES`] bytes (fewer is fine — a short
/// file is simply less identifiable). Returns `None` when nothing matches,
/// which is not an error: Symphonia may still know the format.
pub fn sniff(buf: &[u8]) -> Option<Container> {
    if starts_with(buf, b"MThd") {
        return Some(Container::Midi);
    }
    if starts_with(buf, b"RIFF") && has_at(buf, 8, b"WAVE") {
        return Some(Container::Wav);
    }
    if starts_with(buf, b"RF64") && has_at(buf, 8, b"WAVE") {
        return Some(Container::Wav);
    }
    if starts_with(buf, b"FORM") && (has_at(buf, 8, b"AIFF") || has_at(buf, 8, b"AIFC")) {
        return Some(Container::Aiff);
    }
    if starts_with(buf, b"caff") {
        return Some(Container::Caf);
    }
    if starts_with(buf, b"fLaC") {
        return Some(Container::Flac);
    }
    if starts_with(buf, b"OggS") {
        return Some(Container::Ogg);
    }
    if starts_with(buf, &[0x1A, 0x45, 0xDF, 0xA3]) {
        // EBML. WebM declares itself in the DocType element near the start;
        // anything else that is EBML we call Matroska.
        let head = &buf[..buf.len().min(256)];
        return Some(if find(head, b"webm").is_some() {
            Container::WebM
        } else {
            Container::Matroska
        });
    }
    if has_at(buf, 4, b"ftyp") {
        return Some(iso_brand(buf));
    }
    if starts_with(buf, b"ADIF") {
        return Some(Container::Aac);
    }
    mpeg_or_adts(buf)
}

/// ISO base media: the major brand says whether this is QuickTime or MPEG-4.
///
/// QuickTime (`.mov`) is `qt  `; everything else in the wild that we care
/// about (`isom`, `mp41/2`, `M4A `, `M4V `, `dash`, `avc1`) is MPEG-4. An
/// unrecognised brand is reported as MP4 rather than as nothing: the file *is*
/// ISO base media, and Symphonia will tell us if it cannot read it.
fn iso_brand(buf: &[u8]) -> Container {
    match buf.get(8..12) {
        Some(b"qt  ") => Container::Mov,
        _ => Container::Mp4,
    }
}

/// A bare MPEG audio or ADTS AAC stream, possibly behind an ID3v2 tag.
fn mpeg_or_adts(buf: &[u8]) -> Option<Container> {
    let start = id3v2_len(buf).unwrap_or(0);
    // A tag that claims to be longer than the file is a damaged (or hostile)
    // file, not a reason to give up: there is an ID3 header, so this is an
    // MPEG audio file as far as anyone can tell.
    let Some(rest) = buf.get(start..) else {
        return Some(Container::Mpeg);
    };
    // Scan a short window: some files pad a few bytes before the first frame.
    let window = &rest[..rest.len().min(4_096)];
    for i in 0..window.len().saturating_sub(3) {
        let (b0, b1, b2) = (window[i], window[i + 1], window[i + 2]);
        if b0 != 0xFF || (b1 & 0xE0) != 0xE0 {
            continue;
        }
        // MPEG version/layer bits. `01` in the version field is reserved, and
        // layer `00` means this is ADTS AAC rather than MPEG audio.
        let version = (b1 >> 3) & 0x03;
        let layer = (b1 >> 1) & 0x03;
        if layer == 0 {
            // ADTS: 12-bit sync, and the sampling-frequency index is 0..=12.
            if (b1 & 0xF0) == 0xF0 && ((b2 >> 2) & 0x0F) <= 12 {
                return Some(Container::Aac);
            }
            continue;
        }
        // Reserved version, "bad" bitrate index and reserved sample-rate index
        // all mean we matched noise, not a frame header. Without these checks
        // a run of 0xFF bytes reads as MPEG audio.
        let bitrate_index = b2 >> 4;
        let rate_index = (b2 >> 2) & 0x03;
        if version == 1 || bitrate_index == 0x0F || bitrate_index == 0 || rate_index == 3 {
            continue;
        }
        return Some(Container::Mpeg);
    }
    // An ID3 tag with nothing recognisable after it is still almost certainly
    // an MPEG audio file that we could not see far enough into.
    if start > 0 {
        return Some(Container::Mpeg);
    }
    None
}

/// Length of a leading ID3v2 tag (header included), if there is one.
fn id3v2_len(buf: &[u8]) -> Option<usize> {
    if !starts_with(buf, b"ID3") {
        return None;
    }
    let size = buf.get(6..10)?;
    // Syncsafe integer: seven bits per byte.
    let len = size
        .iter()
        .fold(0usize, |acc, b| (acc << 7) | (*b as usize & 0x7F));
    Some(10 + len)
}

fn starts_with(buf: &[u8], marker: &[u8]) -> bool {
    buf.len() >= marker.len() && &buf[..marker.len()] == marker
}

fn has_at(buf: &[u8], at: usize, marker: &[u8]) -> bool {
    buf.get(at..at + marker.len()) == Some(marker)
}

fn find(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    if needle.is_empty() || haystack.len() < needle.len() {
        return None;
    }
    haystack.windows(needle.len()).position(|w| w == needle)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recognises_the_containers_we_ship_fixtures_for() {
        assert_eq!(
            sniff(b"MThd\0\0\0\x06\0\x01\0\x01\x01\xe0"),
            Some(Container::Midi)
        );
        assert_eq!(sniff(b"RIFF\x24\0\0\0WAVEfmt "), Some(Container::Wav));
        assert_eq!(sniff(b"FORM\0\0\0\x12AIFFCOMM"), Some(Container::Aiff));
        assert_eq!(sniff(b"caff\0\x01\0\0desc"), Some(Container::Caf));
        assert_eq!(sniff(b"fLaC\0\0\0\x22"), Some(Container::Flac));
        assert_eq!(sniff(b"OggS\0\x02\0\0"), Some(Container::Ogg));
        assert_eq!(sniff(b"\0\0\0\x20ftypqt  \0\0\x02\0"), Some(Container::Mov));
        assert_eq!(sniff(b"\0\0\0\x20ftypisom\0\0\x02\0"), Some(Container::Mp4));
        assert_eq!(sniff(b"\0\0\0\x20ftypM4A \0\0\x02\0"), Some(Container::Mp4));
        assert_eq!(
            sniff(b"\x1a\x45\xdf\xa3\x01\0\0\0webm"),
            Some(Container::WebM)
        );
        assert_eq!(
            sniff(b"\x1a\x45\xdf\xa3\x01\0\0\0matroska"),
            Some(Container::Matroska)
        );
    }

    #[test]
    fn finds_mpeg_audio_behind_a_tag_and_at_the_head() {
        // Bare MPEG-1 layer III frame header.
        assert_eq!(sniff(&[0xFF, 0xFB, 0x90, 0x00]), Some(Container::Mpeg));
        // ADTS AAC: layer bits are zero.
        assert_eq!(sniff(&[0xFF, 0xF1, 0x50, 0x80]), Some(Container::Aac));
        // ID3v2 header declaring a 10-byte tag, then the frame.
        let mut tagged = vec![b'I', b'D', b'3', 4, 0, 0, 0, 0, 0, 10];
        tagged.extend_from_slice(&[0u8; 10]);
        tagged.extend_from_slice(&[0xFF, 0xFB, 0x90, 0x00]);
        assert_eq!(sniff(&tagged), Some(Container::Mpeg));
    }

    /// The sniffer is the first thing a hostile file meets. Truncated magic,
    /// an empty buffer, a syncsafe length that overflows the file and a tag
    /// with nothing behind it must all be answers rather than panics.
    #[test]
    fn hostile_prefixes_never_panic() {
        assert_eq!(sniff(b""), None);
        assert_eq!(sniff(b"R"), None);
        assert_eq!(sniff(b"RIFF"), None);
        assert_eq!(sniff(b"RIFF\0\0\0\0AVI "), None);
        assert_eq!(sniff(b"MTh"), None);
        assert_eq!(sniff(b"\0\0\0\x20ftyp"), Some(Container::Mp4));
        assert_eq!(sniff(&[0x1A, 0x45, 0xDF, 0xA3]), Some(Container::Matroska));
        // ID3 tag claiming 256 MiB inside a 12-byte file.
        assert_eq!(
            sniff(&[b'I', b'D', b'3', 4, 0, 0, 0x7F, 0x7F, 0x7F, 0x7F, 0, 0]),
            Some(Container::Mpeg)
        );
        // 0xFF bytes that are not a frame sync.
        assert_eq!(sniff(&[0xFF; 64]), None);
        for len in 0..32usize {
            let noise: Vec<u8> = (0..len).map(|i| (i as u8).wrapping_mul(37)).collect();
            let _ = sniff(&noise);
        }
    }

    #[test]
    fn labels_are_what_the_badge_shows() {
        assert_eq!(Container::Mp4.label(), "MP4");
        assert_eq!(Container::Midi.label(), "MIDI");
        assert!(Container::Mov.may_carry_video());
        assert!(!Container::Wav.may_carry_video());
        assert!(Container::Midi.is_midi());
    }
}
