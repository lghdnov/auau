//! Top-level media streaming routes (outside `/api/v1`, per spec).
//!
//! `GET /media/{id}` serves the stored original through `ServeFile`, which
//! answers `Range` requests with `206 Partial Content` out of the box; the
//! path always comes from the row's `storage_key`, never from the request.
//! `GET /media/{id}/peaks` returns the precomputed peaks binary — 404 while
//! the file is still `processing` or no peaks exist yet.

use std::io;

use axum::body::Body;
use axum::extract::{Path, Request, State};
use axum::http::header;
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::Router;
use tower::ServiceExt;
use tower_http::services::ServeFile;
use uuid::Uuid;

use crate::api::AppState;
use crate::auth::CurrentUser;
use crate::error::AppError;
use crate::models::{AudioFile, FileStatus};

/// Media routes (top-level, outside `/api/v1`, per spec); both require a
/// signed-in user.
pub fn router() -> Router<AppState> {
    Router::new()
        .route("/media/{id}", get(stream))
        .route("/media/{id}/peaks", get(peaks))
}

/// Map a sqlx failure onto the internal-error variant.
fn internal(err: sqlx::Error) -> AppError {
    AppError::Internal(err.into())
}

/// Load one audio file row or answer with the shared 404 envelope.
async fn load_file(pool: &sqlx::PgPool, id: Uuid) -> Result<AudioFile, AppError> {
    sqlx::query_as("SELECT * FROM audio_files WHERE id = $1")
        .bind(id)
        .fetch_optional(pool)
        .await
        .map_err(internal)?
        .ok_or_else(|| AppError::NotFound(format!("file {id} not found")))
}

/// `GET /media/{id}` — the stored original, served read-only.
///
/// The path always comes from the row's server-generated `storage_key`
/// (`media/<uuid>.<allowlisted ext>`), never from the request. `ServeFile`
/// answers `Range` requests with `206 Partial Content` out of the box.
async fn stream(
    State(state): State<AppState>,
    _user: CurrentUser,
    Path(id): Path<Uuid>,
    request: Request,
) -> Result<Response, AppError> {
    let file = load_file(&state.pool, id).await?;
    let ext = file.storage_key.rsplit('.').next().unwrap_or("bin");
    let path = state.store.media_path(id, ext);
    if !path.is_file() {
        return Err(AppError::NotFound(format!("media for file {id} not found")));
    }
    let response = ServeFile::new(path)
        .oneshot(request)
        .await
        .unwrap_or_else(|never| match never {})
        .map(Body::new);
    Ok(response)
}

/// `GET /media/{id}/peaks` — the precomputed waveform peaks binary.
///
/// 404 while the file is still `processing` (peaks are not trustworthy until
/// the pipeline finished) and when no peaks file exists on disk — e.g. after
/// a decode error, where the worker never wrote one.
async fn peaks(
    State(state): State<AppState>,
    _user: CurrentUser,
    Path(id): Path<Uuid>,
) -> Result<Response, AppError> {
    let file = load_file(&state.pool, id).await?;
    if file.status == FileStatus::Processing {
        return Err(AppError::NotFound(format!("peaks for file {id} not ready")));
    }
    let bytes = state.store.read_peaks(id).map_err(|err| match err.kind() {
        io::ErrorKind::NotFound => AppError::NotFound(format!("peaks for file {id} not found")),
        _ => AppError::Internal(anyhow::anyhow!("failed to read peaks of file {id}: {err}")),
    })?;
    Ok(([(header::CONTENT_TYPE, "application/octet-stream")], bytes).into_response())
}

#[cfg(test)]
mod tests {
    use axum::body::{to_bytes, Body};
    use axum::http::{header, Request, StatusCode};
    use sqlx::PgPool;
    use tower::ServiceExt;

    use crate::api::test_support;
    use crate::media::peaks::{self, MinMax, PeaksData};
    use crate::models::FileStatus;

    /// Insert a project + `audio_files` row directly, returning the file id.
    ///
    /// `storage_key` is `media/<uuid>.wav`, mirroring what the upload handler
    /// generates.
    async fn insert_file(pool: &PgPool, uploaded_by: uuid::Uuid, status: FileStatus) -> uuid::Uuid {
        let project_id = uuid::Uuid::new_v4();
        sqlx::query("INSERT INTO projects (id, name, created_by) VALUES ($1, $2, $3)")
            .bind(project_id)
            .bind(format!("project-{}", uuid::Uuid::new_v4().simple()))
            .bind(uploaded_by)
            .execute(pool)
            .await
            .unwrap();

        let id = uuid::Uuid::new_v4();
        sqlx::query(
            "INSERT INTO audio_files (id, project_id, original_name, storage_key, sha256,
                mime, size_bytes, status, uploaded_by)
             VALUES ($1, $2, 'файл.wav', $3, $4, 'audio/wav', 10, $5, $6)",
        )
        .bind(id)
        .bind(project_id)
        .bind(format!("media/{id}.wav"))
        .bind(uuid::Uuid::new_v4().simple().to_string())
        .bind(status)
        .bind(uploaded_by)
        .execute(pool)
        .await
        .unwrap();
        id
    }

    /// 1000 distinguishable payload bytes: `i % 251` so any slice is unique.
    fn kilobyte() -> Vec<u8> {
        (0..1000u32).map(|i| (i % 251) as u8).collect()
    }

    /// Drain a response into (status, headers, body bytes).
    async fn drain(
        response: axum::response::Response,
    ) -> (StatusCode, axum::http::HeaderMap, Vec<u8>) {
        let status = response.status();
        let headers = response.headers().clone();
        let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        (status, headers, body.to_vec())
    }

    #[tokio::test]
    async fn media_requires_auth() {
        let pool = PgPool::connect_lazy("postgres://localhost/auau").unwrap();
        let app = crate::api::router(test_support::state(pool));
        let id = uuid::Uuid::new_v4();

        for uri in [format!("/media/{id}"), format!("/media/{id}/peaks")] {
            let response = app
                .clone()
                .oneshot(Request::get(uri).body(Body::empty()).unwrap())
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
        }
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn range_request_returns_exact_bytes(pool: PgPool) {
        let state = test_support::state(pool.clone());
        let app = crate::api::router(state.clone());
        let (user, cookie) = test_support::signed_in(&pool).await.unwrap();
        let file_id = insert_file(&pool, user.id, FileStatus::New).await;
        let bytes = kilobyte();
        state.store.save_upload(file_id, "wav", &bytes).unwrap();

        let response = app
            .oneshot(
                Request::get(format!("/media/{file_id}"))
                    .header(header::COOKIE, cookie)
                    .header(header::RANGE, "bytes=100-199")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        let (status, headers, body) = drain(response).await;
        assert_eq!(status, StatusCode::PARTIAL_CONTENT);
        assert_eq!(
            headers
                .get(header::CONTENT_LENGTH)
                .and_then(|v| v.to_str().ok()),
            Some("100")
        );
        assert_eq!(
            headers
                .get(header::CONTENT_RANGE)
                .and_then(|v| v.to_str().ok()),
            Some("bytes 100-199/1000")
        );
        assert_eq!(body, bytes[100..200].to_vec());
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn full_file_returns_200(pool: PgPool) {
        let state = test_support::state(pool.clone());
        let app = crate::api::router(state.clone());
        let (user, cookie) = test_support::signed_in(&pool).await.unwrap();
        let file_id = insert_file(&pool, user.id, FileStatus::New).await;
        let bytes = kilobyte();
        state.store.save_upload(file_id, "wav", &bytes).unwrap();

        let response = app
            .oneshot(
                Request::get(format!("/media/{file_id}"))
                    .header(header::COOKIE, cookie)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        let (status, headers, body) = drain(response).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(
            headers
                .get(header::CONTENT_LENGTH)
                .and_then(|v| v.to_str().ok()),
            Some("1000")
        );
        assert_eq!(body, bytes);
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn peaks_ready_returns_binary(pool: PgPool) {
        let state = test_support::state(pool.clone());
        let app = crate::api::router(state.clone());
        let (user, cookie) = test_support::signed_in(&pool).await.unwrap();
        let file_id = insert_file(&pool, user.id, FileStatus::New).await;

        let data = PeaksData {
            overview: vec![MinMax {
                min: -1000,
                max: 1000,
            }],
            detail: vec![MinMax { min: -5, max: 5 }; 7],
        };
        state
            .store
            .write_peaks(file_id, &peaks::encode(&data))
            .unwrap();

        let response = app
            .oneshot(
                Request::get(format!("/media/{file_id}/peaks"))
                    .header(header::COOKIE, cookie)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        let (status, headers, body) = drain(response).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(
            headers
                .get(header::CONTENT_TYPE)
                .and_then(|v| v.to_str().ok()),
            Some("application/octet-stream")
        );
        assert_eq!(peaks::decode(&body).unwrap(), data);
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn peaks_processing_returns_404(pool: PgPool) {
        let state = test_support::state(pool.clone());
        let app = crate::api::router(state.clone());
        let (user, cookie) = test_support::signed_in(&pool).await.unwrap();
        let file_id = insert_file(&pool, user.id, FileStatus::Processing).await;

        let data = PeaksData {
            overview: Vec::new(),
            detail: Vec::new(),
        };
        state
            .store
            .write_peaks(file_id, &peaks::encode(&data))
            .unwrap();

        let response = app
            .oneshot(
                Request::get(format!("/media/{file_id}/peaks"))
                    .header(header::COOKIE, cookie)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn media_missing_file_404(pool: PgPool) {
        let app = crate::api::router(test_support::state(pool.clone()));
        let (user, cookie) = test_support::signed_in(&pool).await.unwrap();

        let file_id = insert_file(&pool, user.id, FileStatus::New).await;
        let response = app
            .clone()
            .oneshot(
                Request::get(format!("/media/{file_id}"))
                    .header(header::COOKIE, &cookie)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::NOT_FOUND);

        let response = app
            .clone()
            .oneshot(
                Request::get(format!("/media/{}", uuid::Uuid::new_v4()))
                    .header(header::COOKIE, &cookie)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::NOT_FOUND);

        let response = app
            .oneshot(
                Request::get(format!("/media/{file_id}/peaks"))
                    .header(header::COOKIE, &cookie)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }
}
