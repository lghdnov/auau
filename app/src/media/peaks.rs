//! Waveform peak computation and its little-endian binary encoding.
//!
//! The peaks format (version 1):
//!
//! ```text
//! magic u32 = 0x6175_6175 | version u16 = 1 | overview_count u32 | detail_count u32
//! overview pairs (min i16, max i16) | detail pairs (min i16, max i16)
//! ```
//!
//! All integers are little-endian. `detail` holds one min/max pair per 10 ms
//! bucket (`sample_rate / 100` samples); `overview` is an even aggregation of
//! `detail` down to at most [`OVERVIEW_MAX_BUCKETS`] pairs for whole-file
//! rendering.

use std::error::Error;
use std::fmt;

/// Format magic: `"auau"` in little-endian byte order.
const MAGIC: u32 = 0x6175_6175;
/// Current peaks format version.
const VERSION: u16 = 1;
/// Header size in bytes: magic (4) + version (2) + two counts (4 + 4).
const HEADER_LEN: usize = 14;
/// Size of one encoded `(min, max)` pair.
const PAIR_LEN: usize = 4;
/// Maximum number of overview buckets (`min(4096, detail_len)`).
const OVERVIEW_MAX_BUCKETS: usize = 4096;
/// Detail buckets per second of audio (one bucket = 10 ms).
const BUCKETS_PER_SEC: u32 = 100;

/// Minimum/maximum sample of one bucket.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MinMax {
    pub min: i16,
    pub max: i16,
}

/// Precomputed waveform peaks at two zoom levels.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PeaksData {
    pub overview: Vec<MinMax>,
    pub detail: Vec<MinMax>,
}

/// Failure modes of [`decode`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PeaksError {
    /// The buffer does not start with the peaks magic.
    BadMagic,
    /// The buffer was written by an incompatible format version.
    BadVersion,
    /// The buffer is shorter than its header or declared payload.
    Truncated,
}

impl fmt::Display for PeaksError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            PeaksError::BadMagic => write!(f, "peaks data has a bad magic value"),
            PeaksError::BadVersion => write!(f, "peaks data has an unsupported version"),
            PeaksError::Truncated => write!(f, "peaks data is truncated"),
        }
    }
}

impl Error for PeaksError {}

/// Compute waveform peaks: detail min/max pairs at 100 buckets per second
/// (`sample_rate / 100` samples per bucket), plus an overview that evenly
/// aggregates detail down to `min(4096, detail_len)` pairs.
pub fn compute(samples: &[i16], sample_rate: u32) -> PeaksData {
    let bucket = (sample_rate / BUCKETS_PER_SEC).max(1) as usize;

    let mut detail = Vec::with_capacity(samples.len().div_ceil(bucket));
    for chunk in samples.chunks(bucket) {
        let mut min = i16::MAX;
        let mut max = i16::MIN;
        for &sample in chunk {
            min = min.min(sample);
            max = max.max(sample);
        }
        detail.push(MinMax { min, max });
    }

    let overview_len = detail.len().min(OVERVIEW_MAX_BUCKETS);
    PeaksData {
        overview: aggregate(&detail, overview_len),
        detail,
    }
}

/// Encode peaks into the version-1 binary format described in the module docs.
pub fn encode(data: &PeaksData) -> Vec<u8> {
    let mut out =
        Vec::with_capacity(HEADER_LEN + (data.overview.len() + data.detail.len()) * PAIR_LEN);
    out.extend_from_slice(&MAGIC.to_le_bytes());
    out.extend_from_slice(&VERSION.to_le_bytes());
    out.extend_from_slice(&(data.overview.len() as u32).to_le_bytes());
    out.extend_from_slice(&(data.detail.len() as u32).to_le_bytes());
    for pair in data.overview.iter().chain(data.detail.iter()) {
        out.extend_from_slice(&pair.min.to_le_bytes());
        out.extend_from_slice(&pair.max.to_le_bytes());
    }
    out
}

/// Decode peaks written by [`encode`].
pub fn decode(bytes: &[u8]) -> Result<PeaksData, PeaksError> {
    if bytes.len() < HEADER_LEN {
        return Err(PeaksError::Truncated);
    }
    if u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]) != MAGIC {
        return Err(PeaksError::BadMagic);
    }
    if u16::from_le_bytes([bytes[4], bytes[5]]) != VERSION {
        return Err(PeaksError::BadVersion);
    }
    let overview_count = u32::from_le_bytes([bytes[6], bytes[7], bytes[8], bytes[9]]) as usize;
    let detail_count = u32::from_le_bytes([bytes[10], bytes[11], bytes[12], bytes[13]]) as usize;

    let declared =
        HEADER_LEN as u64 + (overview_count as u64 + detail_count as u64) * PAIR_LEN as u64;
    if (bytes.len() as u64) < declared {
        return Err(PeaksError::Truncated);
    }

    let overview_end = HEADER_LEN + overview_count * PAIR_LEN;
    Ok(PeaksData {
        overview: read_pairs(&bytes[HEADER_LEN..overview_end]),
        detail: read_pairs(&bytes[overview_end..]),
    })
}

/// Evenly aggregate `detail` pairs into exactly `target` pairs.
///
/// Requires `target <= detail.len()` so every output bucket stays non-empty;
/// with `target == detail.len()` this is the identity mapping.
fn aggregate(detail: &[MinMax], target: usize) -> Vec<MinMax> {
    let len = detail.len();
    if target == 0 {
        return Vec::new();
    }

    let mut out = Vec::with_capacity(target);
    for bucket in 0..target {
        let start = bucket * len / target;
        let end = (bucket + 1) * len / target;
        let mut min = i16::MAX;
        let mut max = i16::MIN;
        for pair in &detail[start..end] {
            min = min.min(pair.min);
            max = max.max(pair.max);
        }
        out.push(MinMax { min, max });
    }
    out
}

/// Read `(min, max)` pairs from a little-endian byte slice.
///
/// The caller has already validated that `bytes` holds whole pairs; any
/// trailing remainder is ignored (`chunks_exact`).
fn read_pairs(bytes: &[u8]) -> Vec<MinMax> {
    bytes
        .chunks_exact(PAIR_LEN)
        .map(|pair| MinMax {
            min: i16::from_le_bytes([pair[0], pair[1]]),
            max: i16::from_le_bytes([pair[2], pair[3]]),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encode_decode_roundtrip() {
        let data = PeaksData {
            overview: vec![MinMax {
                min: i16::MIN,
                max: i16::MAX,
            }],
            detail: vec![
                MinMax { min: -5, max: 10 },
                MinMax { min: 0, max: 0 },
                MinMax {
                    min: -1,
                    max: 32767,
                },
            ],
        };

        let bytes = encode(&data);

        assert_eq!(&bytes[0..4], &MAGIC.to_le_bytes());
        assert_eq!(&bytes[4..6], &VERSION.to_le_bytes());
        assert_eq!(&bytes[6..10], &1u32.to_le_bytes());
        assert_eq!(&bytes[10..14], &3u32.to_le_bytes());
        assert_eq!(bytes.len(), HEADER_LEN + 4 * 4);

        assert_eq!(decode(&bytes).unwrap(), data);

        let empty = PeaksData {
            overview: Vec::new(),
            detail: Vec::new(),
        };
        assert_eq!(decode(&encode(&empty)).unwrap(), empty);
    }

    #[test]
    fn decode_rejects_bad_magic() {
        let bytes = encode(&PeaksData {
            overview: vec![MinMax { min: 0, max: 0 }],
            detail: Vec::new(),
        });
        let mut bad = bytes;
        bad[0] ^= 0xFF;

        assert_eq!(decode(&bad), Err(PeaksError::BadMagic));
    }

    #[test]
    fn decode_rejects_bad_version() {
        let bytes = encode(&PeaksData {
            overview: vec![MinMax { min: 0, max: 0 }],
            detail: Vec::new(),
        });
        let mut bad = bytes;
        bad[4..6].copy_from_slice(&2u16.to_le_bytes());

        assert_eq!(decode(&bad), Err(PeaksError::BadVersion));
    }

    #[test]
    fn decode_rejects_truncated() {
        let bytes = encode(&PeaksData {
            overview: vec![MinMax { min: -1, max: 1 }; 3],
            detail: vec![MinMax { min: 0, max: 0 }; 5],
        });

        assert_eq!(decode(&[]), Err(PeaksError::Truncated));
        assert_eq!(decode(&bytes[..HEADER_LEN - 1]), Err(PeaksError::Truncated));
        assert_eq!(
            decode(&bytes[..bytes.len() - 1]),
            Err(PeaksError::Truncated)
        );

        let mut lying = bytes;
        lying[6..10].copy_from_slice(&100u32.to_le_bytes());
        assert_eq!(decode(&lying), Err(PeaksError::Truncated));
    }

    #[test]
    fn sine_one_sec_detail_is_100_buckets() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("sine.wav");
        std::fs::write(&path, crate::media::testutil::sine_wav(440.0, 1.0, 8000)).unwrap();

        let (samples, sample_rate, _channels) = crate::media::probe::decode_to_i16(&path).unwrap();
        assert_eq!(sample_rate, 8000);
        assert_eq!(samples.len(), 8000);

        let peaks = compute(&samples, 8000);
        assert_eq!(peaks.detail.len(), 100);
        for (bucket, pair) in peaks.detail.iter().enumerate() {
            assert!(
                pair.max > 29000,
                "bucket {bucket}: max {} not > 29000",
                pair.max
            );
            assert!(
                pair.min < -29000,
                "bucket {bucket}: min {} not < -29000",
                pair.min
            );
        }
    }

    #[test]
    fn short_file_overview_matches_detail() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("short.wav");
        std::fs::write(&path, crate::media::testutil::sine_wav(440.0, 0.3, 8000)).unwrap();

        let (samples, sample_rate, _channels) = crate::media::probe::decode_to_i16(&path).unwrap();
        let peaks = compute(&samples, sample_rate);

        assert_eq!(peaks.detail.len(), 30);
        assert_eq!(peaks.overview.len(), 30);
        assert_eq!(peaks.overview, peaks.detail);
    }

    #[test]
    fn long_file_overview_capped() {
        let samples: Vec<i16> = (0..60 * 44100).map(|n| (n % 4096 - 2048) as i16).collect();
        let peaks = compute(&samples, 44100);

        assert_eq!(peaks.detail.len(), 6000);
        assert_eq!(peaks.overview.len(), 4096);

        let detail_min = peaks.detail.iter().map(|p| p.min).min().unwrap();
        let overview_min = peaks.overview.iter().map(|p| p.min).min().unwrap();
        let detail_max = peaks.detail.iter().map(|p| p.max).max().unwrap();
        let overview_max = peaks.overview.iter().map(|p| p.max).max().unwrap();
        assert_eq!(overview_min, detail_min);
        assert_eq!(overview_max, detail_max);
    }
}
