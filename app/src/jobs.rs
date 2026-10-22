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

/// Crash recovery at startup: the pending-work channel lives in memory, so a
/// restart or crash loses whatever had not been processed yet and the rows
/// would stay `processing` forever. This feeds every such row back through
/// the pipeline (oldest first), where `process_file` decides `new` vs
/// `error` again — decoding is idempotent, so a second instance racing on
/// the same rows is harmless (the status UPDATE only matches `processing`).
/// Returns the number of re-enqueued files.
pub async fn reenqueue_stuck(pool: &PgPool, queue: &JobQueue) -> anyhow::Result<u64> {
    let ids: Vec<Uuid> = sqlx::query_scalar(
        "SELECT id FROM audio_files WHERE status = 'processing' ORDER BY created_at",
    )
    .fetch_all(pool)
    .await?;
    for id in &ids {
        queue.enqueue(*id).await?;
    }
    Ok(ids.len() as u64)
}

/// Return expired queue leases: `in_progress` files whose `claim_expires_at`
/// has passed go back to `new`, unclaimed. Annotations are deliberately kept
/// — the next annotator continues the partial work. Returns the number of
/// freed files.
pub async fn sweep_expired_leases(pool: &PgPool) -> anyhow::Result<u64> {
    let result = sqlx::query(
        "UPDATE audio_files SET status = 'new', claimed_by = NULL, claim_expires_at = NULL
             WHERE status = 'in_progress' AND claim_expires_at < now()",
    )
    .execute(pool)
    .await?;
    Ok(result.rows_affected())
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
    let outcome = tokio::task::spawn_blocking(move || -> Result<(i32, i32, i32), String> {
        let (samples, sample_rate, channels) = match decode_to_i16(&path) {
            Ok(decoded) => decoded,
            Err(err) => return Err(err.to_string()),
        };
        let duration_ms = (samples.len() as u64 * 1000 / u64::from(sample_rate)) as i32;
        let encoded = peaks::encode(&peaks::compute(&samples, sample_rate));
        worker_store
            .write_peaks(file.id, &encoded)
            .map_err(|err| format!("failed to write peaks: {err}"))?;
        Ok((duration_ms, sample_rate as i32, i32::from(channels)))
    })
    .await
    .map_err(|err| format!("processing task panicked: {err}"))
    .and_then(|inner| inner);

    match outcome {
        Ok((duration_ms, sample_rate, channels)) => {
            sqlx::query(
                "UPDATE audio_files
                 SET duration_ms = $2, sample_rate = $3, channels = $4,
                     status = 'new', error_message = NULL
                 WHERE id = $1 AND status = 'processing'",
            )
            .bind(file.id)
            .bind(duration_ms)
            .bind(sample_rate)
            .bind(channels)
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::test_support;
    use crate::media::testutil::sine_wav;

    /// Create a user and a project for them — the FK anchors of a file row.
    async fn user_and_project(pool: &PgPool) -> (Uuid, Uuid) {
        let (user, _cookie) = test_support::signed_in(pool).await.unwrap();
        let project_id = Uuid::new_v4();
        sqlx::query("INSERT INTO projects (id, name, created_by) VALUES ($1, $2, $3)")
            .bind(project_id)
            .bind(format!("проект-{}", project_id.simple()))
            .bind(user.id)
            .execute(pool)
            .await
            .unwrap();
        (user.id, project_id)
    }

    /// Insert a file row in the given status and store its media on disk —
    /// the exact state an upload leaves behind before the worker runs.
    async fn insert_file_with_media(
        pool: &PgPool,
        project_id: Uuid,
        uploaded_by: Uuid,
        status: FileStatus,
        media: &[u8],
        store: &MediaStore,
    ) -> Uuid {
        let id = Uuid::new_v4();
        sqlx::query(
            "INSERT INTO audio_files (id, project_id, original_name, storage_key, sha256,
                mime, size_bytes, status, uploaded_by)
             VALUES ($1, $2, 'файл.wav', $3, $4, 'audio/wav', $5, $6, $7)",
        )
        .bind(id)
        .bind(project_id)
        .bind(format!("media/{id}.wav"))
        .bind(id.simple().to_string())
        .bind(media.len() as i64)
        .bind(status)
        .bind(uploaded_by)
        .execute(pool)
        .await
        .unwrap();
        store.save_upload(id, "wav", media).unwrap();
        id
    }

    /// The file row, for status assertions.
    async fn row(pool: &PgPool, id: Uuid) -> AudioFile {
        sqlx::query_as("SELECT * FROM audio_files WHERE id = $1")
            .bind(id)
            .fetch_one(pool)
            .await
            .unwrap()
    }

    /// Poll until `id` leaves `processing` (the worker runs asynchronously).
    async fn wait_not_processing(pool: &PgPool, id: Uuid) -> AudioFile {
        for _ in 0..100 {
            let file = row(pool, id).await;
            if file.status != FileStatus::Processing {
                return file;
            }
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
        panic!("file {id} is still processing after 2s");
    }

    /// The startup recovery path: `processing` rows left by a crashed run are
    /// re-fed through a freshly spawned worker and reach `new`/`error` again.
    #[sqlx::test(migrations = "./migrations")]
    async fn reenqueue_stuck_recovers_processing_rows(pool: PgPool) {
        let (user, project) = user_and_project(&pool).await;
        let store = MediaStore::new(tempfile::tempdir().unwrap().keep());

        let stuck_ok = insert_file_with_media(
            &pool,
            project,
            user,
            FileStatus::Processing,
            &sine_wav(440.0, 1.0, 8000),
            &store,
        )
        .await;
        let stuck_bad = insert_file_with_media(
            &pool,
            project,
            user,
            FileStatus::Processing,
            b"definitely not audio",
            &store,
        )
        .await;
        let fresh = insert_file_with_media(
            &pool,
            project,
            user,
            FileStatus::New,
            &sine_wav(880.0, 1.0, 8000),
            &store,
        )
        .await;

        let (jobs, job_rx) = JobQueue::new();
        let _worker = spawn_worker(pool.clone(), store.clone(), job_rx);

        let reenqueued = reenqueue_stuck(&pool, &jobs).await.unwrap();
        assert_eq!(reenqueued, 2, "only the processing rows are re-enqueued");

        let done = wait_not_processing(&pool, stuck_ok).await;
        assert_eq!(done.status, FileStatus::New);
        assert_eq!(done.duration_ms, Some(1000));
        assert_eq!(done.sample_rate, Some(8000));
        assert_eq!(done.error_message, None);
        assert!(store.read_peaks(stuck_ok).is_ok(), "peaks must be written");

        let failed = wait_not_processing(&pool, stuck_bad).await;
        assert_eq!(failed.status, FileStatus::Error);
        assert!(failed.error_message.is_some_and(|m| !m.is_empty()));

        assert_eq!(row(&pool, fresh).await.status, FileStatus::New);

        let (jobs2, job_rx2) = JobQueue::new();
        let _worker2 = spawn_worker(pool.clone(), store, job_rx2);
        assert_eq!(reenqueue_stuck(&pool, &jobs2).await.unwrap(), 0);
    }
}
