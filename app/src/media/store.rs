use std::fs;
use std::io;
use std::path::PathBuf;

use uuid::Uuid;

/// Local-filesystem storage for uploaded originals and precomputed peaks.
///
/// Layout under the store root (typically `Config.storage_dir`):
/// - `media/<uuid>.<ext>` — the untouched uploaded original;
/// - `peaks/<uuid>.bin` — the derived waveform peaks.
#[derive(Debug, Clone)]
pub struct MediaStore {
    root: PathBuf,
}

impl MediaStore {
    /// Create a store rooted at `root`.
    pub fn new(root: PathBuf) -> Self {
        Self { root }
    }

    /// Path of the stored original: `<root>/media/<id>.<ext>`.
    pub fn media_path(&self, id: Uuid, ext: &str) -> PathBuf {
        self.root.join("media").join(format!("{id}.{ext}"))
    }

    /// Path of the peaks file: `<root>/peaks/<id>.bin`.
    pub fn peaks_path(&self, id: Uuid) -> PathBuf {
        self.root.join("peaks").join(format!("{id}.bin"))
    }

    /// Persist an uploaded original, creating `media/` if needed.
    pub fn save_upload(&self, id: Uuid, ext: &str, bytes: &[u8]) -> io::Result<()> {
        let path = self.media_path(id, ext);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::write(path, bytes)
    }

    /// Delete the stored original. Fails with `io::ErrorKind::NotFound` if absent.
    pub fn delete_media(&self, id: Uuid, ext: &str) -> io::Result<()> {
        fs::remove_file(self.media_path(id, ext))
    }

    /// Persist precomputed peaks, creating `peaks/` if needed.
    pub fn write_peaks(&self, id: Uuid, bytes: &[u8]) -> io::Result<()> {
        let path = self.peaks_path(id);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::write(path, bytes)
    }

    /// Read the precomputed peaks back.
    pub fn read_peaks(&self, id: Uuid) -> io::Result<Vec<u8>> {
        fs::read(self.peaks_path(id))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn store_paths_and_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().to_path_buf();
        let store = MediaStore::new(root.clone());
        let id = Uuid::new_v4();

        assert_eq!(
            store.media_path(id, "wav"),
            root.join("media").join(format!("{id}.wav"))
        );
        assert_eq!(
            store.peaks_path(id),
            root.join("peaks").join(format!("{id}.bin"))
        );

        let media = crate::media::testutil::sine_wav(440.0, 0.01, 8000);
        store.save_upload(id, "wav", &media).unwrap();
        assert_eq!(fs::read(store.media_path(id, "wav")).unwrap(), media);
        store.delete_media(id, "wav").unwrap();
        assert!(!store.media_path(id, "wav").exists());

        let peaks = vec![1u8, 2, 3, 4];
        store.write_peaks(id, &peaks).unwrap();
        assert_eq!(store.read_peaks(id).unwrap(), peaks);
    }
}
