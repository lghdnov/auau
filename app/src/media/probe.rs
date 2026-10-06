use std::error::Error;
use std::fmt;
use std::fs::File;
use std::io;
use std::path::Path;

use symphonia::core::audio::SampleBuffer;
use symphonia::core::codecs::{DecoderOptions, CODEC_TYPE_NULL};
use symphonia::core::errors::Error as SymphoniaError;
use symphonia::core::formats::FormatOptions;
use symphonia::core::io::MediaSourceStream;
use symphonia::core::meta::MetadataOptions;
use symphonia::core::probe::Hint;

/// Audio stream properties extracted by [`probe`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProbeInfo {
    pub duration_ms: i32,
    pub sample_rate: i32,
    pub channels: i32,
}

/// Failure modes of [`probe`].
#[derive(Debug)]
pub enum ProbeError {
    /// The file could not be opened or read.
    Io(io::Error),
    /// No enabled format reader or decoder accepts the container/codec
    /// (e.g. AAC inside an MP4 container).
    Unsupported(String),
    /// The container could not be parsed or a packet failed to decode.
    Decode(String),
    /// The container holds no audio track.
    NoAudioTrack,
    /// The audio track does not declare a usable (positive) sample rate.
    MissingSampleRate,
}

impl fmt::Display for ProbeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ProbeError::Io(err) => write!(f, "i/o error while probing: {err}"),
            ProbeError::Unsupported(reason) => {
                write!(f, "unsupported container or codec: {reason}")
            }
            ProbeError::Decode(reason) => write!(f, "failed to decode audio: {reason}"),
            ProbeError::NoAudioTrack => write!(f, "no audio track found"),
            ProbeError::MissingSampleRate => write!(f, "audio track has no usable sample rate"),
        }
    }
}

impl Error for ProbeError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            ProbeError::Io(err) => Some(err),
            _ => None,
        }
    }
}

impl From<SymphoniaError> for ProbeError {
    fn from(err: SymphoniaError) -> Self {
        match err {
            SymphoniaError::IoError(err) => ProbeError::Io(err),
            SymphoniaError::DecodeError(reason) => ProbeError::Decode(reason.into()),
            SymphoniaError::Unsupported(reason) => ProbeError::Unsupported(reason.into()),
            SymphoniaError::LimitError(reason) => ProbeError::Unsupported(reason.into()),
            SymphoniaError::SeekError(_) => ProbeError::Decode("seek failed while probing".into()),
            SymphoniaError::ResetRequired => ProbeError::Decode("decoder reset required".into()),
        }
    }
}

/// Probe the audio file at `path`: open it, demux, and fully decode it, then
/// derive duration and stream basics from the result. A full decode is fine
/// for our file sizes (≤2 h); the frame count equals the mono sample count
/// produced by the shared decode loop.
///
/// The container is hinted from the file extension, so probe the stored file
/// (`<uuid>.<ext>`) rather than the client-supplied original name.
///
/// Symphonia panics on some malformed-but-parseable headers (e.g. a WAV
/// declaring a zero sample rate panics inside `TimeBase::new` during demuxer
/// init). Since probe runs on user uploads, malformed input must surface as
/// `Err`, so the symphonia interaction is fenced with `catch_unwind` and any
/// internal panic is reported as [`ProbeError::Decode`].
pub fn probe(path: &Path) -> Result<ProbeInfo, ProbeError> {
    let decoded = decode_fenced(path)?;
    let frames = decoded.samples.len() as u64;
    Ok(ProbeInfo {
        duration_ms: (frames * 1000 / u64::from(decoded.sample_rate)) as i32,
        sample_rate: decoded.sample_rate as i32,
        channels: decoded.channels as i32,
    })
}

/// Decode the audio file at `path` into mono i16 samples.
///
/// Multi-channel audio is mixed down to mono (channels averaged per frame);
/// returns `(samples, sample_rate, channels)`, where `channels` is the source
/// stream's channel count *before* the mix-down. Shares the panic-fenced
/// decode loop of [`probe`], so malformed uploads surface as `Err` instead of
/// panicking.
pub fn decode_to_i16(path: &Path) -> Result<(Vec<i16>, u32, u16), ProbeError> {
    let decoded = decode_fenced(path)?;
    Ok((
        decoded.samples,
        decoded.sample_rate,
        decoded.channels as u16,
    ))
}

/// A fully decoded audio stream mixed down to mono.
struct Decoded {
    /// One i16 sample per frame (channels averaged, or the sole channel).
    samples: Vec<i16>,
    sample_rate: u32,
    /// Channel count of the source stream (before the mono mix-down).
    channels: u32,
}

/// Run [`decode_inner`] behind the `catch_unwind` fence described in [`probe`],
/// so every symphonia call in both public paths is covered.
fn decode_fenced(path: &Path) -> Result<Decoded, ProbeError> {
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| decode_inner(path)))
        .map_err(|payload| ProbeError::Decode(panic_message(&payload)))
        .and_then(|result| result)
}

/// Extract a human-readable reason from a panic payload.
fn panic_message(payload: &Box<dyn std::any::Any + Send>) -> String {
    if let Some(message) = payload.downcast_ref::<&'static str>() {
        format!("internal decoder panic: {message}")
    } else if let Some(message) = payload.downcast_ref::<String>() {
        format!("internal decoder panic: {message}")
    } else {
        "internal decoder panic".to_string()
    }
}

fn decode_inner(path: &Path) -> Result<Decoded, ProbeError> {
    let mut hint = Hint::new();
    if let Some(ext) = path.extension().and_then(|ext| ext.to_str()) {
        hint.with_extension(ext);
    }

    let file = File::open(path).map_err(ProbeError::Io)?;
    let source = MediaSourceStream::new(Box::new(file), Default::default());

    let probed = symphonia::default::get_probe().format(
        &hint,
        source,
        &FormatOptions::default(),
        &MetadataOptions::default(),
    )?;
    let mut format = probed.format;

    let track = format
        .tracks()
        .iter()
        .find(|track| track.codec_params.codec != CODEC_TYPE_NULL)
        .ok_or(ProbeError::NoAudioTrack)?;

    let track_id = track.id;
    let codec_params = track.codec_params.clone();

    let sample_rate = codec_params
        .sample_rate
        .filter(|rate| *rate > 0)
        .ok_or(ProbeError::MissingSampleRate)?;
    let mut channels = codec_params.channels.map(|channels| channels.count());

    let mut decoder =
        symphonia::default::get_codecs().make(&codec_params, &DecoderOptions::default())?;

    let mut samples: Vec<i16> = Vec::new();
    loop {
        let packet = match format.next_packet() {
            Ok(packet) => packet,
            Err(SymphoniaError::IoError(err)) if err.kind() == io::ErrorKind::UnexpectedEof => {
                break
            }
            Err(err) => return Err(err.into()),
        };

        if packet.track_id() != track_id {
            continue;
        }

        let decoded = match decoder.decode(&packet) {
            Ok(decoded) => decoded,
            Err(SymphoniaError::DecodeError(_)) => continue,
            Err(err) => return Err(err.into()),
        };

        if channels.is_none() {
            channels = Some(decoded.spec().channels.count());
        }

        let channels_count = decoded.spec().channels.count().max(1);
        let mut interleaved = SampleBuffer::<i16>::new(decoded.frames() as u64, *decoded.spec());
        interleaved.copy_interleaved_ref(decoded);
        mix_to_mono(interleaved.samples(), channels_count, &mut samples);
    }

    Ok(Decoded {
        samples,
        sample_rate,
        channels: channels.unwrap_or(1) as u32,
    })
}

/// Fold interleaved samples of `channels` channels into mono, appending one
/// sample per frame to `out`. Mono input is copied as-is.
fn mix_to_mono(interleaved: &[i16], channels: usize, out: &mut Vec<i16>) {
    if channels <= 1 {
        out.extend_from_slice(interleaved);
        return;
    }
    out.extend(interleaved.chunks_exact(channels).map(|frame| {
        let sum: i32 = frame.iter().map(|&sample| i32::from(sample)).sum();
        (sum / channels as i32) as i16
    }));
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use std::path::PathBuf;

    fn write_file(dir: &Path, name: &str, bytes: &[u8]) -> PathBuf {
        let path = dir.join(name);
        File::create(&path).unwrap().write_all(bytes).unwrap();
        path
    }

    #[test]
    fn probe_reads_sine_wav() {
        let dir = tempfile::tempdir().unwrap();
        let path = write_file(
            dir.path(),
            "sine.wav",
            &crate::media::testutil::sine_wav(440.0, 1.0, 8000),
        );

        let info = probe(&path).unwrap();

        assert_eq!(info.sample_rate, 8000);
        assert_eq!(info.channels, 1);
        assert!(
            (995..=1005).contains(&info.duration_ms),
            "duration_ms {} outside 995..=1005",
            info.duration_ms
        );
    }

    #[test]
    fn probe_rejects_zero_sample_rate() {
        let dir = tempfile::tempdir().unwrap();
        let mut wav = crate::media::testutil::sine_wav(440.0, 0.01, 8000);
        wav[24..28].copy_from_slice(&0u32.to_le_bytes());
        let path = write_file(dir.path(), "zero-rate.wav", &wav);

        let result = probe(&path);

        assert!(result.is_err(), "expected Err, got {result:?}");
    }

    #[test]
    fn probe_rejects_garbage() {
        let dir = tempfile::tempdir().unwrap();
        let path = write_file(dir.path(), "garbage.wav", b"not audio");

        assert!(probe(&path).is_err());
    }

    #[test]
    fn decode_to_i16_reads_sine_wav() {
        let dir = tempfile::tempdir().unwrap();
        let path = write_file(
            dir.path(),
            "sine.wav",
            &crate::media::testutil::sine_wav(440.0, 1.0, 8000),
        );

        let (samples, sample_rate, channels) = decode_to_i16(&path).unwrap();

        assert_eq!(sample_rate, 8000);
        assert_eq!(channels, 1);
        assert_eq!(samples.len(), 8000);
        assert!(*samples.iter().max().unwrap() > 29000);
        assert!(*samples.iter().min().unwrap() < -29000);
    }
}
