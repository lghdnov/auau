//! Upload, listing, and deletion of audio files.
//!
//! `POST /api/v1/projects/{id}/files` accepts a multipart body of file parts;
//! every part is validated (extension allowlist, per-file size cap), deduped
//! by sha256 within the project *before* touching the disk, stored, and
//! enqueued onto the processing pipeline that produces waveform peaks.

use axum::extract::{Multipart, Path, Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Json, Router};
use chrono::Utc;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use uuid::Uuid;

use crate::api::AppState;
use crate::auth::CurrentUser;
use crate::error::AppError;
use crate::models::{AudioFile, FileStatus};

/// Page size of the file listing.
const PAGE_SIZE: i64 = 50;

/// File routes; all of them require a signed-in user.
pub fn router() -> Router<AppState> {
    Router::new()
        .route("/api/v1/projects/{id}/files", get(list).post(upload))
        .route("/api/v1/files/{id}", get(show).delete(remove))
}

/// Per-file outcome of a multipart upload request.
#[derive(Debug, Serialize)]
pub struct UploadResult {
    pub original_name: String,
    pub status: UploadStatus,
    pub file_id: Option<Uuid>,
    pub message: Option<String>,
}

/// Verdict for one uploaded file part.
#[derive(Debug, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum UploadStatus {
    Accepted,
    Duplicate,
    Rejected,
}

/// Query parameters of the file list.
#[derive(Debug, Deserialize)]
struct ListQuery {
    status: Option<FileStatus>,
    page: Option<u32>,
}

/// Map a sqlx failure onto the internal-error variant.
fn internal(err: sqlx::Error) -> AppError {
    AppError::Internal(err.into())
}

/// Map a malformed multipart stream onto a 400.
fn invalid_multipart(err: axum::extract::multipart::MultipartError) -> AppError {
    AppError::BadRequest {
        code: "invalid_multipart".into(),
        message: format!("malformed multipart body: {err}"),
    }
}

/// The upload-extension allowlist. The extension is taken from the original
/// filename, lowercased; per the `media_path` contract only these values may
/// ever reach a storage path. (`m4a` is deliberately included: per spec the
/// container is accepted and decode is attempted — an AAC failure lands in
/// status `error` with a recorded cause.)
fn allowed_ext(filename: &str) -> Option<&'static str> {
    let ext = filename.rsplit_once('.')?.1.to_ascii_lowercase();
    match ext.as_str() {
        "wav" => Some("wav"),
        "flac" => Some("flac"),
        "mp3" => Some("mp3"),
        "ogg" => Some("ogg"),
        "m4a" => Some("m4a"),
        _ => None,
    }
}

/// Canonical MIME type per allowed extension.
fn mime_for(ext: &str) -> &'static str {
    match ext {
        "flac" => "audio/flac",
        "mp3" => "audio/mpeg",
        "ogg" => "audio/ogg",
        "m4a" => "audio/mp4",
        _ => "audio/wav",
    }
}

/// 404 unless the project exists.
async fn ensure_project(pool: &sqlx::PgPool, id: Uuid) -> Result<(), AppError> {
    let exists: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM projects WHERE id = $1)")
        .bind(id)
        .fetch_one(pool)
        .await
        .map_err(internal)?;
    if exists {
        Ok(())
    } else {
        Err(AppError::NotFound(format!("project {id} not found")))
    }
}

/// Id of the file with this digest already stored in the project, if any.
async fn find_by_sha256(
    pool: &sqlx::PgPool,
    project_id: Uuid,
    sha256: &str,
) -> Result<Option<Uuid>, AppError> {
    sqlx::query_scalar("SELECT id FROM audio_files WHERE project_id = $1 AND sha256 = $2")
        .bind(project_id)
        .bind(sha256)
        .fetch_optional(pool)
        .await
        .map_err(internal)
}

fn rejected(original_name: String, message: &str) -> UploadResult {
    UploadResult {
        original_name,
        status: UploadStatus::Rejected,
        file_id: None,
        message: Some(message.into()),
    }
}

fn duplicate(original_name: String, existing: Uuid) -> UploadResult {
    UploadResult {
        original_name,
        status: UploadStatus::Duplicate,
        file_id: Some(existing),
        message: Some("дубликат: файл уже загружен в этот проект".into()),
    }
}

/// Record an infrastructure failure on a fresh row so it is not stuck in
/// `processing`.
async fn mark_processing_error(
    pool: &sqlx::PgPool,
    id: Uuid,
    message: &str,
) -> Result<(), AppError> {
    sqlx::query(
        "UPDATE audio_files SET status = 'error', error_message = $2
         WHERE id = $1 AND status = 'processing'",
    )
    .bind(id)
    .bind(message)
    .execute(pool)
    .await
    .map_err(internal)?;
    Ok(())
}

/// `POST /api/v1/projects/:id/files` — upload one or more audio files.
///
/// Every file part is validated (extension allowlist, per-file size cap),
/// deduped by sha256 within the project *before* touching the disk, stored
/// under its own UUID, and enqueued onto the processing pipeline.
async fn upload(
    State(state): State<AppState>,
    user: CurrentUser,
    Path(project_id): Path<Uuid>,
    mut multipart: Multipart,
) -> Result<(StatusCode, Json<Vec<UploadResult>>), AppError> {
    ensure_project(&state.pool, project_id).await?;

    let mut results = Vec::new();
    while let Some(mut field) = multipart.next_field().await.map_err(invalid_multipart)? {
        let Some(original_name) = field.file_name().map(ToOwned::to_owned) else {
            continue;
        };

        let mut hasher = Sha256::new();
        let mut bytes = Vec::new();
        let mut too_large = false;
        while let Some(chunk) = field.chunk().await.map_err(invalid_multipart)? {
            hasher.update(&chunk);
            if !too_large && bytes.len() + chunk.len() > state.max_upload_bytes {
                too_large = true;
            }
            if !too_large {
                bytes.extend_from_slice(&chunk);
            }
        }

        if too_large {
            results.push(rejected(original_name, "файл больше лимита"));
            continue;
        }
        let Some(ext) = allowed_ext(&original_name) else {
            results.push(rejected(
                original_name,
                "неподдерживаемый тип файла (wav/flac/mp3/ogg/m4a)",
            ));
            continue;
        };

        let sha256 = format!("{:x}", hasher.finalize());

        if let Some(existing) = find_by_sha256(&state.pool, project_id, &sha256).await? {
            results.push(duplicate(original_name, existing));
            continue;
        }

        let id = Uuid::new_v4();
        let inserted = sqlx::query(
            "INSERT INTO audio_files (id, project_id, original_name, storage_key, sha256,
                mime, size_bytes, status, uploaded_by)
             VALUES ($1, $2, $3, $4, $5, $6, $7, 'processing', $8)",
        )
        .bind(id)
        .bind(project_id)
        .bind(&original_name)
        .bind(format!("media/{id}.{ext}"))
        .bind(&sha256)
        .bind(mime_for(ext))
        .bind(bytes.len() as i64)
        .bind(user.0.id)
        .execute(&state.pool)
        .await;

        match inserted {
            Ok(_) => {}
            Err(sqlx::Error::Database(db)) if db.is_unique_violation() => {
                let existing = find_by_sha256(&state.pool, project_id, &sha256)
                    .await?
                    .ok_or_else(|| {
                        AppError::Internal(anyhow::anyhow!(
                            "unique violation on (project_id, sha256) but no existing row found"
                        ))
                    })?;
                results.push(duplicate(original_name, existing));
                continue;
            }
            Err(err) => return Err(internal(err)),
        }

        if let Err(err) = state.store.save_upload(id, ext, &bytes) {
            mark_processing_error(&state.pool, id, &err.to_string()).await?;
            return Err(AppError::Internal(anyhow::anyhow!(
                "failed to store upload {id}: {err}"
            )));
        }
        if let Err(err) = state.jobs.enqueue(id).await {
            mark_processing_error(&state.pool, id, &err.to_string()).await?;
            return Err(AppError::Internal(anyhow::anyhow!(
                "failed to enqueue upload {id}: {err}"
            )));
        }

        results.push(UploadResult {
            original_name,
            status: UploadStatus::Accepted,
            file_id: Some(id),
            message: None,
        });
    }

    Ok((StatusCode::CREATED, Json(results)))
}

/// `GET /api/v1/projects/:id/files?status=&page=` — paged file listing with
/// the total row count in the `x-total-count` header.
async fn list(
    State(state): State<AppState>,
    _user: CurrentUser,
    Path(project_id): Path<Uuid>,
    Query(query): Query<ListQuery>,
) -> Result<Response, AppError> {
    ensure_project(&state.pool, project_id).await?;

    let page = query.page.unwrap_or(1).max(1);
    let offset = (i64::from(page) - 1) * PAGE_SIZE;

    let (files, total): (Vec<AudioFile>, i64) = match query.status {
        Some(status) => {
            let files = sqlx::query_as(
                "SELECT * FROM audio_files WHERE project_id = $1 AND status = $2
                 ORDER BY created_at, id LIMIT $3 OFFSET $4",
            )
            .bind(project_id)
            .bind(status)
            .bind(PAGE_SIZE)
            .bind(offset)
            .fetch_all(&state.pool)
            .await
            .map_err(internal)?;
            let total: i64 = sqlx::query_scalar(
                "SELECT count(*) FROM audio_files WHERE project_id = $1 AND status = $2",
            )
            .bind(project_id)
            .bind(status)
            .fetch_one(&state.pool)
            .await
            .map_err(internal)?;
            (files, total)
        }
        None => {
            let files = sqlx::query_as(
                "SELECT * FROM audio_files WHERE project_id = $1
                 ORDER BY created_at, id LIMIT $2 OFFSET $3",
            )
            .bind(project_id)
            .bind(PAGE_SIZE)
            .bind(offset)
            .fetch_all(&state.pool)
            .await
            .map_err(internal)?;
            let total: i64 =
                sqlx::query_scalar("SELECT count(*) FROM audio_files WHERE project_id = $1")
                    .bind(project_id)
                    .fetch_one(&state.pool)
                    .await
                    .map_err(internal)?;
            (files, total)
        }
    };

    Ok(([("x-total-count", total.to_string())], Json(files)).into_response())
}

/// `GET /api/v1/files/:id` — one audio file.
async fn show(
    State(state): State<AppState>,
    _user: CurrentUser,
    Path(id): Path<Uuid>,
) -> Result<Json<AudioFile>, AppError> {
    let file: AudioFile = sqlx::query_as("SELECT * FROM audio_files WHERE id = $1")
        .bind(id)
        .fetch_optional(&state.pool)
        .await
        .map_err(internal)?
        .ok_or_else(|| AppError::NotFound(format!("file {id} not found")))?;
    Ok(Json(file))
}

/// `DELETE /api/v1/files/:id` — delete a file row (annotations cascade) and
/// its media from disk.
///
/// Deletion rights: `new`/`error` (and mid-`processing`) files are free to
/// delete; an `in_progress` file belongs to its claimant until the lease
/// expires; a `done` file must be reopened first (409).
async fn remove(
    State(state): State<AppState>,
    user: CurrentUser,
    Path(id): Path<Uuid>,
) -> Result<StatusCode, AppError> {
    let file: AudioFile = sqlx::query_as("SELECT * FROM audio_files WHERE id = $1")
        .bind(id)
        .fetch_optional(&state.pool)
        .await
        .map_err(internal)?
        .ok_or_else(|| AppError::NotFound(format!("file {id} not found")))?;

    match file.status {
        FileStatus::Done => {
            return Err(AppError::Conflict {
                code: "file_done".into(),
                message: "file is marked done; reopen it before deleting".into(),
            });
        }
        FileStatus::InProgress => {
            let someone_elses_live_claim = file.claimed_by != Some(user.0.id)
                && file
                    .claim_expires_at
                    .is_none_or(|expires| expires > Utc::now());
            if someone_elses_live_claim {
                return Err(AppError::Conflict {
                    code: "file_claimed".into(),
                    message: "file is checked out by another user".into(),
                });
            }
        }
        FileStatus::Processing | FileStatus::New | FileStatus::Error => {}
    }

    let deleted = sqlx::query("DELETE FROM audio_files WHERE id = $1")
        .bind(id)
        .execute(&state.pool)
        .await
        .map_err(internal)?;
    if deleted.rows_affected() == 0 {
        return Err(AppError::NotFound(format!("file {id} not found")));
    }

    let ext = file.storage_key.rsplit('.').next().unwrap_or("bin");
    let _ = state.store.delete_media(id, ext);
    let _ = std::fs::remove_file(state.store.peaks_path(id));

    Ok(StatusCode::NO_CONTENT)
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::{header, Request, StatusCode};
    use chrono::{DateTime, Duration, Utc};
    use serde_json::{json, Value};
    use sqlx::PgPool;
    use tower::ServiceExt;

    use crate::api::test_support;
    use crate::media::testutil::sine_wav;

    /// Multipart boundary used by the request builder below.
    const BOUNDARY: &str = "auau-test-boundary";

    /// Build a one-file multipart upload request for `uri`.
    fn upload_file(uri: &str, cookie: &str, filename: &str, bytes: &[u8]) -> Request<Body> {
        let mut body = Vec::new();
        body.extend_from_slice(format!("--{BOUNDARY}\r\n").as_bytes());
        body.extend_from_slice(
            format!("Content-Disposition: form-data; name=\"file\"; filename=\"{filename}\"\r\n")
                .as_bytes(),
        );
        body.extend_from_slice(b"Content-Type: application/octet-stream\r\n\r\n");
        body.extend_from_slice(bytes);
        body.extend_from_slice(format!("\r\n--{BOUNDARY}--\r\n").as_bytes());

        Request::post(uri)
            .header(header::COOKIE, cookie)
            .header(
                header::CONTENT_TYPE,
                format!("multipart/form-data; boundary={BOUNDARY}"),
            )
            .body(Body::from(body))
            .unwrap()
    }

    /// An authorized DELETE request.
    fn delete(uri: &str, cookie: &str) -> Request<Body> {
        Request::delete(uri)
            .header(header::COOKIE, cookie)
            .body(Body::empty())
            .unwrap()
    }

    /// Create a project through the API and return its id.
    async fn create_project(app: &Router, cookie: &str) -> Uuid {
        let (status, body) = test_support::json(
            app.clone()
                .oneshot(test_support::post_json(
                    "/api/v1/projects",
                    cookie,
                    json!({"name": format!("project-{}", Uuid::new_v4().simple())}),
                ))
                .await
                .unwrap(),
        )
        .await;
        assert_eq!(
            status,
            StatusCode::CREATED,
            "project creation failed: {body}"
        );
        body["id"].as_str().unwrap().parse().unwrap()
    }

    /// Upload one file part and drain the response into (status, json).
    async fn upload(
        app: &Router,
        cookie: &str,
        project_id: Uuid,
        filename: &str,
        bytes: &[u8],
    ) -> (StatusCode, Value) {
        test_support::json(
            app.clone()
                .oneshot(upload_file(
                    &format!("/api/v1/projects/{project_id}/files"),
                    cookie,
                    filename,
                    bytes,
                ))
                .await
                .unwrap(),
        )
        .await
    }

    /// Poll `GET /api/v1/files/:id` until its status reaches `want` (≤2 s).
    async fn poll_status(app: &Router, cookie: &str, file_id: Uuid, want: &str) -> Value {
        let mut last = Value::Null;
        for _ in 0..40 {
            let (status, body) = test_support::json(
                app.clone()
                    .oneshot(test_support::get(
                        &format!("/api/v1/files/{file_id}"),
                        cookie,
                    ))
                    .await
                    .unwrap(),
            )
            .await;
            assert_eq!(status, StatusCode::OK);
            if body["status"] == json!(want) {
                return body;
            }
            last = body;
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        }
        panic!("file {file_id} did not reach status '{want}' within 2s, last: {last}");
    }

    /// Insert an audio_files row directly with an explicit claim state.
    async fn insert_file(
        pool: &PgPool,
        project_id: Uuid,
        uploaded_by: Uuid,
        status: FileStatus,
        claimed_by: Option<Uuid>,
        claim_expires_at: Option<DateTime<Utc>>,
    ) -> Uuid {
        let id = Uuid::new_v4();
        sqlx::query(
            "INSERT INTO audio_files (id, project_id, original_name, storage_key, sha256,
                mime, size_bytes, status, claimed_by, claim_expires_at, uploaded_by)
             VALUES ($1, $2, 'файл.wav', $3, $4, 'audio/wav', 10, $5, $6, $7, $8)",
        )
        .bind(id)
        .bind(project_id)
        .bind(format!("media/{id}.wav"))
        .bind(Uuid::new_v4().simple().to_string())
        .bind(status)
        .bind(claimed_by)
        .bind(claim_expires_at)
        .bind(uploaded_by)
        .execute(pool)
        .await
        .unwrap();
        id
    }

    #[tokio::test]
    async fn upload_requires_a_session() {
        let pool = PgPool::connect_lazy("postgres://localhost/auau").unwrap();
        let app = crate::api::build_router(test_support::state(pool));

        let (status, body) = test_support::json(
            app.oneshot(upload_file(
                &format!("/api/v1/projects/{}/files", Uuid::new_v4()),
                "auau_session=invalid",
                "a.wav",
                b"ab",
            ))
            .await
            .unwrap(),
        )
        .await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
        assert_eq!(body["error"]["code"], json!("unauthorized"));
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn upload_processes_to_new(pool: PgPool) {
        let state = test_support::state(pool.clone());
        let app = crate::api::build_router(state.clone());
        let (_user, cookie) = test_support::signed_in(&pool).await.unwrap();
        let project = create_project(&app, &cookie).await;

        let (status, body) = upload(
            &app,
            &cookie,
            project,
            "sine.wav",
            &sine_wav(440.0, 1.0, 8000),
        )
        .await;
        assert_eq!(status, StatusCode::CREATED);
        assert_eq!(body.as_array().unwrap().len(), 1);
        assert_eq!(body[0]["status"], json!("accepted"));
        assert_eq!(body[0]["original_name"], json!("sine.wav"));
        let file_id: Uuid = body[0]["file_id"].as_str().unwrap().parse().unwrap();

        let file = poll_status(&app, &cookie, file_id, "new").await;
        assert_eq!(file["duration_ms"], json!(1000));
        assert_eq!(file["sample_rate"], json!(8000));
        assert_eq!(file["channels"], json!(1));
        assert_eq!(file["error_message"], Value::Null);

        let peaks = crate::media::peaks::decode(&state.store.read_peaks(file_id).unwrap())
            .expect("peaks must decode");
        assert_eq!(peaks.detail.len(), 100);
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn upload_invalid_audio_marks_error(pool: PgPool) {
        let app = crate::api::build_router(test_support::state(pool.clone()));
        let (_user, cookie) = test_support::signed_in(&pool).await.unwrap();
        let project = create_project(&app, &cookie).await;

        let (status, body) = upload(
            &app,
            &cookie,
            project,
            "broken.wav",
            b"definitely not audio",
        )
        .await;
        assert_eq!(status, StatusCode::CREATED);
        assert_eq!(body[0]["status"], json!("accepted"));
        let broken_id: Uuid = body[0]["file_id"].as_str().unwrap().parse().unwrap();

        let file = poll_status(&app, &cookie, broken_id, "error").await;
        assert!(
            file["error_message"]
                .as_str()
                .is_some_and(|m| !m.is_empty()),
            "error_message must be non-empty: {file}"
        );

        let (status, body) = upload(
            &app,
            &cookie,
            project,
            "good.wav",
            &sine_wav(440.0, 1.0, 8000),
        )
        .await;
        assert_eq!(status, StatusCode::CREATED);
        let good_id: Uuid = body[0]["file_id"].as_str().unwrap().parse().unwrap();
        let file = poll_status(&app, &cookie, good_id, "new").await;
        assert_eq!(file["duration_ms"], json!(1000));
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn upload_duplicate_sha256_conflicts(pool: PgPool) {
        let app = crate::api::build_router(test_support::state(pool.clone()));
        let (_user, cookie) = test_support::signed_in(&pool).await.unwrap();
        let project = create_project(&app, &cookie).await;

        let wav = sine_wav(440.0, 1.0, 8000);
        let (_, first) = upload(&app, &cookie, project, "sine.wav", &wav).await;
        assert_eq!(first[0]["status"], json!("accepted"));
        let first_id: Uuid = first[0]["file_id"].as_str().unwrap().parse().unwrap();

        let (status, second) = upload(&app, &cookie, project, "copy.wav", &wav).await;
        assert_eq!(status, StatusCode::CREATED);
        assert_eq!(second[0]["status"], json!("duplicate"));
        assert_eq!(
            second[0]["message"],
            json!("дубликат: файл уже загружен в этот проект")
        );
        let second_id: Uuid = second[0]["file_id"].as_str().unwrap().parse().unwrap();
        assert_eq!(second_id, first_id);

        let count: i64 = sqlx::query_scalar("SELECT count(*) FROM audio_files")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(count, 1);
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn upload_rejects_oversize(pool: PgPool) {
        let dir = tempfile::tempdir().unwrap().keep();
        let app = crate::api::build_router(test_support::state_with_store(pool.clone(), dir, 64));
        let (_user, cookie) = test_support::signed_in(&pool).await.unwrap();
        let project = create_project(&app, &cookie).await;

        let (status, body) = upload(
            &app,
            &cookie,
            project,
            "big.wav",
            &sine_wav(440.0, 1.0, 8000),
        )
        .await;
        assert_eq!(status, StatusCode::CREATED);
        assert_eq!(body[0]["status"], json!("rejected"));
        assert_eq!(body[0]["message"], json!("файл больше лимита"));
        assert_eq!(body[0]["file_id"], Value::Null);

        let count: i64 = sqlx::query_scalar("SELECT count(*) FROM audio_files")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(count, 0, "oversized file must not be stored");
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn upload_rejects_unknown_extension(pool: PgPool) {
        let app = crate::api::build_router(test_support::state(pool.clone()));
        let (_user, cookie) = test_support::signed_in(&pool).await.unwrap();
        let project = create_project(&app, &cookie).await;

        for filename in ["notes.txt", "program.exe"] {
            let (status, body) = upload(&app, &cookie, project, filename, b"whatever").await;
            assert_eq!(status, StatusCode::CREATED);
            assert_eq!(body[0]["status"], json!("rejected"));
            assert_eq!(
                body[0]["message"],
                json!("неподдерживаемый тип файла (wav/flac/mp3/ogg/m4a)")
            );
            assert_eq!(body[0]["file_id"], Value::Null);
        }

        let (status, body) = upload(
            &app,
            &cookie,
            project,
            "SINE.WAV",
            &sine_wav(440.0, 1.0, 8000),
        )
        .await;
        assert_eq!(status, StatusCode::CREATED);
        assert_eq!(body[0]["status"], json!("accepted"));

        let count: i64 = sqlx::query_scalar("SELECT count(*) FROM audio_files")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(count, 1, "only the allowlisted upload is stored");
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn upload_accepts_m4a_for_decode_attempt(pool: PgPool) {
        let app = crate::api::build_router(test_support::state(pool.clone()));
        let (_user, cookie) = test_support::signed_in(&pool).await.unwrap();
        let project = create_project(&app, &cookie).await;

        let (status, body) = upload(&app, &cookie, project, "song.m4a", b"fake mp4 bytes").await;
        assert_eq!(status, StatusCode::CREATED);
        assert_eq!(body[0]["status"], json!("accepted"));
        let file_id: Uuid = body[0]["file_id"].as_str().unwrap().parse().unwrap();

        let stored: Option<String> =
            sqlx::query_scalar("SELECT original_name FROM audio_files WHERE id = $1")
                .bind(file_id)
                .fetch_optional(&pool)
                .await
                .unwrap();
        assert_eq!(stored.as_deref(), Some("song.m4a"));
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn delete_done_file_conflicts(pool: PgPool) {
        let app = crate::api::build_router(test_support::state(pool.clone()));
        let (user, cookie) = test_support::signed_in(&pool).await.unwrap();
        let project = create_project(&app, &cookie).await;

        let done = insert_file(&pool, project, user.id, FileStatus::Done, None, None).await;

        let (status, body) = test_support::json(
            app.clone()
                .oneshot(delete(&format!("/api/v1/files/{done}"), &cookie))
                .await
                .unwrap(),
        )
        .await;
        assert_eq!(status, StatusCode::CONFLICT);
        assert_eq!(body["error"]["code"], json!("file_done"));

        let count: i64 = sqlx::query_scalar("SELECT count(*) FROM audio_files WHERE id = $1")
            .bind(done)
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(count, 1, "done file must survive");

        let (status, body) = test_support::json(
            app.oneshot(delete(
                &format!("/api/v1/files/{}", Uuid::new_v4()),
                &cookie,
            ))
            .await
            .unwrap(),
        )
        .await;
        assert_eq!(status, StatusCode::NOT_FOUND);
        assert_eq!(body["error"]["code"], json!("not_found"));
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn delete_new_file_removes_disk(pool: PgPool) {
        let dir = tempfile::tempdir().unwrap().keep();
        let state =
            test_support::state_with_store(pool.clone(), dir, test_support::TEST_MAX_UPLOAD_BYTES);
        let app = crate::api::build_router(state.clone());
        let (_user, cookie) = test_support::signed_in(&pool).await.unwrap();
        let project = create_project(&app, &cookie).await;

        let (_, body) = upload(
            &app,
            &cookie,
            project,
            "sine.wav",
            &sine_wav(440.0, 1.0, 8000),
        )
        .await;
        let file_id: Uuid = body[0]["file_id"].as_str().unwrap().parse().unwrap();
        poll_status(&app, &cookie, file_id, "new").await;

        let media = state.store.media_path(file_id, "wav");
        let peaks = state.store.peaks_path(file_id);
        assert!(media.exists(), "original must be on disk before delete");
        assert!(peaks.exists(), "peaks must be on disk before delete");

        let response = app
            .clone()
            .oneshot(delete(&format!("/api/v1/files/{file_id}"), &cookie))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::NO_CONTENT);

        let count: i64 = sqlx::query_scalar("SELECT count(*) FROM audio_files WHERE id = $1")
            .bind(file_id)
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(count, 0, "row must be gone");
        assert!(!media.exists(), "original must be removed from disk");
        assert!(!peaks.exists(), "peaks must be removed from disk");
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn delete_in_progress_follows_lease(pool: PgPool) {
        let app = crate::api::build_router(test_support::state(pool.clone()));
        let (alice, alice_cookie) = test_support::signed_in(&pool).await.unwrap();
        let (bob, bob_cookie) = test_support::signed_in(&pool).await.unwrap();
        let project = create_project(&app, &alice_cookie).await;

        let claimed = insert_file(
            &pool,
            project,
            alice.id,
            FileStatus::InProgress,
            Some(bob.id),
            Some(Utc::now() + Duration::minutes(10)),
        )
        .await;
        let (status, body) = test_support::json(
            app.clone()
                .oneshot(delete(&format!("/api/v1/files/{claimed}"), &alice_cookie))
                .await
                .unwrap(),
        )
        .await;
        assert_eq!(status, StatusCode::CONFLICT);
        assert_eq!(body["error"]["code"], json!("file_claimed"));

        let response = app
            .clone()
            .oneshot(delete(&format!("/api/v1/files/{claimed}"), &bob_cookie))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::NO_CONTENT);

        let expired = insert_file(
            &pool,
            project,
            alice.id,
            FileStatus::InProgress,
            Some(bob.id),
            Some(Utc::now() - Duration::minutes(1)),
        )
        .await;
        let response = app
            .oneshot(delete(&format!("/api/v1/files/{expired}"), &alice_cookie))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::NO_CONTENT);
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn list_files_filters_paginates_and_counts(pool: PgPool) {
        let app = crate::api::build_router(test_support::state(pool.clone()));
        let (user, cookie) = test_support::signed_in(&pool).await.unwrap();
        let project = create_project(&app, &cookie).await;

        for _ in 0..51 {
            insert_file(&pool, project, user.id, FileStatus::New, None, None).await;
        }
        for _ in 0..2 {
            insert_file(&pool, project, user.id, FileStatus::Error, None, None).await;
        }

        async fn page(app: &Router, cookie: &str, project: Uuid, query: &str) -> (String, Value) {
            let response = app
                .clone()
                .oneshot(test_support::get(
                    &format!("/api/v1/projects/{project}/files{query}"),
                    cookie,
                ))
                .await
                .unwrap();
            let total = response
                .headers()
                .get("x-total-count")
                .expect("x-total-count header")
                .to_str()
                .unwrap()
                .to_string();
            let (_, body) = test_support::json(response).await;
            (total, body)
        }

        let (total, body) = page(&app, &cookie, project, "?status=new&page=1").await;
        assert_eq!(total, "51");
        assert_eq!(body.as_array().unwrap().len(), 50);
        assert!(body
            .as_array()
            .unwrap()
            .iter()
            .all(|file| file["status"] == json!("new")));

        let (total, body) = page(&app, &cookie, project, "?status=new&page=2").await;
        assert_eq!(total, "51");
        assert_eq!(body.as_array().unwrap().len(), 1);

        let (total, body) = page(&app, &cookie, project, "?status=error").await;
        assert_eq!(total, "2");
        assert_eq!(body.as_array().unwrap().len(), 2);

        let (total, body) = page(&app, &cookie, project, "").await;
        assert_eq!(total, "53");
        assert_eq!(body.as_array().unwrap().len(), 50);
    }
}
