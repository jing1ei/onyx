use std::path::PathBuf;

/// Errors surfaced by the audio core.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("unsupported or unreadable file: {0}")]
    Unsupported(String),

    #[error("no audio track found in {0}")]
    NoAudioTrack(PathBuf),

    #[error("decode failed: {0}")]
    Decode(String),

    #[error("io: {0}")]
    Io(#[from] std::io::Error),

    #[error("no output device available")]
    NoOutputDevice,

    #[error("output device error: {0}")]
    Device(String),

    #[error("the requested stream format is not supported by the device")]
    UnsupportedStreamConfig,

    #[error("resampler: {0}")]
    Resample(String),

    #[error("engine is not running")]
    NotRunning,

    #[error("{0}")]
    Other(String),
}

impl From<symphonia::core::errors::Error> for Error {
    fn from(e: symphonia::core::errors::Error) -> Self {
        use symphonia::core::errors::Error as SE;
        match e {
            SE::IoError(io) => Error::Io(io),
            SE::Unsupported(m) => Error::Unsupported(m.to_string()),
            other => Error::Decode(other.to_string()),
        }
    }
}

impl From<cpal::DevicesError> for Error {
    fn from(e: cpal::DevicesError) -> Self {
        Error::Device(e.to_string())
    }
}

impl From<cpal::DeviceNameError> for Error {
    fn from(e: cpal::DeviceNameError) -> Self {
        Error::Device(e.to_string())
    }
}

impl From<cpal::SupportedStreamConfigsError> for Error {
    fn from(e: cpal::SupportedStreamConfigsError) -> Self {
        Error::Device(e.to_string())
    }
}

impl From<cpal::BuildStreamError> for Error {
    fn from(e: cpal::BuildStreamError) -> Self {
        Error::Device(e.to_string())
    }
}

impl From<cpal::PlayStreamError> for Error {
    fn from(e: cpal::PlayStreamError) -> Self {
        Error::Device(e.to_string())
    }
}

pub type Result<T> = std::result::Result<T, Error>;
