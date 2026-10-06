//! Background processing of uploaded audio files.
//!
//! [`JobQueue`] is the sender side handed to API handlers; [`spawn_worker`]
//! runs the receiving loop that turns each freshly uploaded `processing` file
//! into a `new` (peaks ready) or `error` (cause recorded) row.

use tokio::sync::mpsc;
use uuid::Uuid;

use sqlx::PgPool;

use crate::media::peaks;
use crate::media::probe::decode_to_i16;
use crate::media::store::MediaStore;
use crate::models::{AudioFile, FileStatus};

/// Depth of the pending-work channel between handlers and the worker.
const QUEUE_DEPTH: usize = 64;

/// Handle for enqueueing files onto the processing pipeline.
#[derive(Debug, Clone)]
pub struct JobQueue {
    tx: mpsc::Sender<Uuid>,
}

impl JobQueue {
    /// Create the queue together with the receiving end for [`spawn_worker`].
    pub fn new() -> (Self, mpsc::Receiver<Uuid>) {
        let (tx, rx) = mpsc::channel(QUEUE_DEPTH);
        (Self { tx }, rx)
    }

    /// Enqueue one file; fails only if the worker is no longer running.
    pub async fn enqueue(&self, file_id: Uuid) -> anyhow::Result<()> {
        self.tx
            .send(file_id)
            .await
            .map_err(|_| anyhow::anyhow!("processing worker is not running"))
    }
}

/// Spawn the processing worker that drains `rx` until the queue is dropped.
pub fn spawn_worker(
    pool: PgPool,
    store: MediaStore,
    mut rx: mpsc::Receiver<Uuid>,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        while let Some(file_id) = rx.recv().await {
            if let Err(err) = process_file(&pool, &store, file_id).await {
                tracing::error!(file_id = %file_id, error = ?err, "file processing crashed");
            }
        }
    })
}

/// Run one file through the pipeline: decode → peaks → `new`, or `error` with
/// the recorded cause. Infrastructure failures (not decode failures) propagate
/// to the worker loop.
async fn process_file(pool: &PgPool, store: &MediaStore, file_id: Uuid) -> anyhow::Result<()> {
    let file: Option<AudioFile> = sqlx::query_as("SELECT * FROM audio_files WHERE id = $1")
        .bind(file_id)
        .fetch_optional(pool)
        .await?;
    let Some(file) = file else {
        tracing::debug!(file_id = %file_id, "file deleted before processing started");
        return Ok(());
    };
    if file.status != FileStatus::Processing {
        tracing::debug!(file_id = %file_id, "file is not awaiting processing");
        return Ok(());
    }

    let ext = file
        .storage_key
        .rsplit('.')
        .next()
        .unwrap_or("bin")
        .to_owned();
    let path = store.media_path(file.id, &ext);

    let worker_store = store.clone();
    let outcome = tokio::task::spawn_blocking(move || -> Result<(i32, i32), String> {
        let (samples, sample_rate) = match decode_to_i16(&path) {
            Ok(decoded) => decoded,
            Err(err) => return Err(err.to_string()),
        };
        let duration_ms = (samples.len() as u64 * 1000 / u64::from(sample_rate)) as i32;
        let encoded = peaks::encode(&peaks::compute(&samples, sample_rate));
        worker_store
            .write_peaks(file.id, &encoded)
            .map_err(|err| format!("failed to write peaks: {err}"))?;
        Ok((duration_ms, sample_rate as i32))
    })
    .await
    .map_err(|err| format!("processing task panicked: {err}"))
    .and_then(|inner| inner);

    match outcome {
        Ok((duration_ms, sample_rate)) => {
            sqlx::query(
                "UPDATE audio_files
                 SET duration_ms = $2, sample_rate = $3, channels = 1,
                     status = 'new', error_message = NULL
                 WHERE id = $1 AND status = 'processing'",
            )
            .bind(file.id)
            .bind(duration_ms)
            .bind(sample_rate)
            .execute(pool)
            .await?;
        }
        Err(cause) => {
            sqlx::query(
                "UPDATE audio_files SET status = 'error', error_message = $2
                 WHERE id = $1 AND status = 'processing'",
            )
            .bind(file.id)
            .bind(&cause)
            .execute(pool)
            .await?;
            tracing::warn!(file_id = %file.id, cause = %cause, "file processing failed");
        }
    }
    Ok(())
}
