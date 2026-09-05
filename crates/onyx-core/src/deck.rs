//! Host-side view of the two comparison decks.

use serde::Serialize;

use crate::decode::DecodeHandle;
use crate::types::{LoudnessAnalysis, TrackInfo};

/// A deck as the host (UI) sees it.
#[derive(Clone, Default)]
pub struct DeckSlot {
    pub handle: Option<DecodeHandle>,
    /// Level-match trim currently applied, in dB.
    pub trim_db: f32,
    /// Playlist entry id this deck was loaded from.
    pub entry_id: Option<u64>,
    /// Does the *engine* currently hold this deck's PCM?
    ///
    /// Normally the same answer as `handle.is_some()`, but not while a rate
    /// change is in flight: PCM decoded at 44.1 kHz plays a fifth-and-a-bit
    /// sharp on a device that has just been re-opened at 96 kHz, so the host
    /// takes it away from the audio callback until it has been re-decoded
    /// (`loader::apply_rate_agreement`). The deck is still loaded — the file,
    /// the trim and the playlist row are all still here — it is simply silent
    /// for the moment, which is the only honest thing to be.
    pub attached: bool,
}

impl DeckSlot {
    pub fn is_loaded(&self) -> bool {
        self.handle.is_some()
    }

    pub fn info(&self) -> Option<&TrackInfo> {
        self.handle.as_ref().map(|h| &h.info)
    }

    pub fn duration_secs(&self) -> f64 {
        self.handle
            .as_ref()
            .map(|h| h.duration_secs())
            .unwrap_or(0.0)
    }

    pub fn analysis(&self) -> Option<LoudnessAnalysis> {
        self.handle.as_ref().and_then(|h| h.status.analysis())
    }

    pub fn state(&self) -> DeckState {
        match &self.handle {
            None => DeckState::default(),
            Some(h) => DeckState {
                loaded: true,
                entry_id: self.entry_id,
                info: Some(h.info.clone()),
                duration_secs: h.duration_secs(),
                decoded_fraction: h.pcm.progress(),
                decoded: h.pcm.is_complete(),
                truncated: h.status.is_truncated(),
                analysis: h.status.analysis(),
                trim_db: self.trim_db,
                bit_transparent: h.bit_transparent,
                error: h.status.error(),
                waveform_buckets: h.waveform.len(),
            },
        }
    }
}

/// Serialisable deck state for the UI.
#[derive(Clone, Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DeckState {
    pub loaded: bool,
    pub entry_id: Option<u64>,
    pub info: Option<TrackInfo>,
    pub duration_secs: f64,
    pub decoded_fraction: f32,
    pub decoded: bool,
    pub truncated: bool,
    pub analysis: Option<LoudnessAnalysis>,
    pub trim_db: f32,
    pub bit_transparent: bool,
    pub error: Option<String>,
    pub waveform_buckets: usize,
}
