//! Test-only audio fixtures shared across the crate's `#[cfg(test)]` modules.

/// Encode a mono 16-bit PCM sine wave as a complete in-memory RIFF/WAVE file.
///
/// The file starts with a 44-byte header, followed by `secs * sample_rate`
/// samples of `sin(2*pi*freq_hz*t)` at amplitude `0.9 * 32767`. It is a valid,
/// fully decodable upload payload — used by media tests here and by the
/// upload/peaks tests of later tasks.
pub fn sine_wav(freq_hz: f32, secs: f32, sample_rate: u32) -> Vec<u8> {
    let sample_count = (secs * sample_rate as f32).round() as u32;
    let data_len = sample_count * 2;

    let mut wav = Vec::with_capacity(44 + data_len as usize);
    wav.extend_from_slice(b"RIFF");
    wav.extend_from_slice(&(36 + data_len).to_le_bytes());
    wav.extend_from_slice(b"WAVE");
    wav.extend_from_slice(b"fmt ");
    wav.extend_from_slice(&16u32.to_le_bytes());
    wav.extend_from_slice(&1u16.to_le_bytes());
    wav.extend_from_slice(&1u16.to_le_bytes());
    wav.extend_from_slice(&sample_rate.to_le_bytes());
    wav.extend_from_slice(&(sample_rate * 2).to_le_bytes());
    wav.extend_from_slice(&2u16.to_le_bytes());
    wav.extend_from_slice(&16u16.to_le_bytes());
    wav.extend_from_slice(b"data");
    wav.extend_from_slice(&data_len.to_le_bytes());

    for n in 0..sample_count {
        let t = n as f32 / sample_rate as f32;
        let sample = (0.9 * 32767.0 * (std::f32::consts::TAU * freq_hz * t).sin()) as i16;
        wav.extend_from_slice(&sample.to_le_bytes());
    }

    wav
}
