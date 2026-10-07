//! The free annotation queue: claim the next file, hand it back, mark it
//! done, reopen a finished one.
//!
//! Claims are lease-based (2 hours, renewed on every touch of the file) so
//! several annotators can work one project without stealing rows from each
//! other; expired leases are returned to the queue by
//! [`crate::jobs::sweep_expired_leases`], keeping the partial annotations.

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::post;
use axum::{Json, Router};
use serde::Deserialize;
use uuid::Uuid;

use crate::api::AppState;
use crate::auth::CurrentUser;
use crate::error::AppError;
use crate::models::{Annotation, AudioFile, Label};
use crate::validate::{validate_segments, SegmentInput};

/// Queue routes; all of them require a signed-in user.
pub fn router() -> Router<AppState> {
    Router::new()
        .route("/api/v1/projects/{id}/queue/claim", post(claim))
        .route("/api/v1/files/{id}/release", post(release))
        .route("/api/v1/files/{id}/complete", post(complete))
        .route("/api/v1/files/{id}/reopen", post(reopen))
}

/// Query parameters of the claim endpoint.
#[derive(Debug, Deserialize)]
struct ClaimQuery {
    order: Option<String>,
}

/// Map a sqlx failure onto the internal-error variant.
fn internal(err: sqlx::Error) -> AppError {
    AppError::Internal(err.into())
}

/// Whether an audio file row with this id exists.
async fn file_exists(pool: &sqlx::PgPool, id: Uuid) -> Result<bool, AppError> {
    sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM audio_files WHERE id = $1)")
        .bind(id)
        .fetch_one(pool)
        .await
        .map_err(internal)
}

/// `POST /api/v1/projects/:id/queue/claim?order=created` — take the oldest
/// `new` file of the project under a fresh 2-hour lease.
///
/// The row is picked and locked in one statement (`FOR UPDATE SKIP LOCKED`),
/// so concurrent claims never hand the same file to two annotators: every
/// loser moves on to the next row — or gets 204 when the queue ran dry.
async fn claim(
    State(state): State<AppState>,
    user: CurrentUser,
    Path(project_id): Path<Uuid>,
    Query(query): Query<ClaimQuery>,
) -> Result<Response, AppError> {
    match query.order.as_deref() {
        None | Some("created") => {}
        Some(other) => {
            return Err(AppError::BadRequest {
                code: "invalid_order".into(),
                message: format!("unknown queue order '{other}'; supported: created"),
            });
        }
    }

    let file: Option<AudioFile> = sqlx::query_as(
        "UPDATE audio_files
         SET status = 'in_progress', claimed_by = $2, claim_expires_at = now() + interval '2 hours'
         WHERE id = (
             SELECT id FROM audio_files
             WHERE project_id = $1 AND status = 'new'
             ORDER BY created_at
             LIMIT 1 FOR UPDATE SKIP LOCKED
         )
         RETURNING *",
    )
    .bind(project_id)
    .bind(user.0.id)
    .fetch_optional(&state.pool)
    .await
    .map_err(internal)?;

    match file {
        Some(file) => Ok(Json(file).into_response()),
        None => {
            let exists: bool =
                sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM projects WHERE id = $1)")
                    .bind(project_id)
                    .fetch_one(&state.pool)
                    .await
                    .map_err(internal)?;
            if exists {
                Ok(StatusCode::NO_CONTENT.into_response())
            } else {
                Err(AppError::NotFound(format!(
                    "project {project_id} not found"
                )))
            }
        }
    }
}

/// A 409 `lease_required` when the file exists, a 404 when it does not —
/// the follow-up to a conditional UPDATE that matched no rows.
async fn conflict_unless_missing(pool: &sqlx::PgPool, id: Uuid) -> Result<AppError, AppError> {
    if file_exists(pool, id).await? {
        Ok(AppError::Conflict {
            code: "lease_required".into(),
            message: "a live lease owned by the caller is required".into(),
        })
    } else {
        Ok(AppError::NotFound(format!("file {id} not found")))
    }
}

/// `POST /api/v1/files/:id/release` — return a claimed file to the queue.
/// Only the owner of a live lease may do so; segments are kept.
async fn release(
    State(state): State<AppState>,
    user: CurrentUser,
    Path(id): Path<Uuid>,
) -> Result<Json<AudioFile>, AppError> {
    let released: Option<AudioFile> = sqlx::query_as(
        "UPDATE audio_files
         SET status = 'new', claimed_by = NULL, claim_expires_at = NULL
         WHERE id = $1 AND status = 'in_progress' AND claimed_by = $2
           AND (claim_expires_at IS NULL OR claim_expires_at > now())
         RETURNING *",
    )
    .bind(id)
    .bind(user.0.id)
    .fetch_optional(&state.pool)
    .await
    .map_err(internal)?;

    match released {
        Some(file) => Ok(Json(file)),
        None => Err(conflict_unless_missing(&state.pool, id).await?),
    }
}

/// `POST /api/v1/files/:id/complete` — mark a claimed file done.
///
/// The stored segments are validated against the project palette; zero
/// segments is a valid outcome ("no events"). Every touch of the file renews
/// the lease, so a failed validation still leaves the caller a fresh 2-hour
/// lease to fix the segments. On success `claimed_by` is kept (attribution
/// of who finished) while `claim_expires_at` is cleared.
async fn complete(
    State(state): State<AppState>,
    user: CurrentUser,
    Path(id): Path<Uuid>,
) -> Result<Json<AudioFile>, AppError> {
    let file: AudioFile = match sqlx::query_as::<_, AudioFile>(
        "UPDATE audio_files
         SET claim_expires_at = now() + interval '2 hours'
         WHERE id = $1 AND status = 'in_progress' AND claimed_by = $2
           AND (claim_expires_at IS NULL OR claim_expires_at > now())
         RETURNING *",
    )
    .bind(id)
    .bind(user.0.id)
    .fetch_optional(&state.pool)
    .await
    .map_err(internal)?
    {
        Some(file) => file,
        None => return Err(conflict_unless_missing(&state.pool, id).await?),
    };

    let labels: Vec<Label> = sqlx::query_as("SELECT * FROM labels WHERE project_id = $1")
        .bind(file.project_id)
        .fetch_all(&state.pool)
        .await
        .map_err(internal)?;
    let annotations: Vec<Annotation> =
        sqlx::query_as("SELECT * FROM annotations WHERE file_id = $1 ORDER BY created_at, id")
            .bind(id)
            .fetch_all(&state.pool)
            .await
            .map_err(internal)?;
    let segments: Vec<SegmentInput> = annotations
        .iter()
        .map(|a| SegmentInput {
            label_id: a.label_id,
            start_ms: a.start_ms,
            end_ms: a.end_ms,
        })
        .collect();
    validate_segments(&segments, &labels, file.duration_ms)
        .map_err(|errors| AppError::Unprocessable { errors })?;

    let done: Option<AudioFile> = sqlx::query_as(
        "UPDATE audio_files SET status = 'done', claim_expires_at = NULL
         WHERE id = $1 AND status = 'in_progress' AND claimed_by = $2
         RETURNING *",
    )
    .bind(id)
    .bind(user.0.id)
    .fetch_optional(&state.pool)
    .await
    .map_err(internal)?;
    done.map(Json).ok_or_else(|| {
        AppError::Internal(anyhow::anyhow!(
            "lease on file {id} vanished during complete"
        ))
    })
}

/// `POST /api/v1/files/:id/reopen` — reopen a done file, claiming it.
///
/// Anyone may reopen (free queue, no roles): the file goes back to
/// `in_progress` under a fresh 2-hour lease held by the caller; the stored
/// segments are kept as the starting point.
async fn reopen(
    State(state): State<AppState>,
    user: CurrentUser,
    Path(id): Path<Uuid>,
) -> Result<Json<AudioFile>, AppError> {
    let reopened: Option<AudioFile> = sqlx::query_as(
        "UPDATE audio_files
         SET status = 'in_progress', claimed_by = $2, claim_expires_at = now() + interval '2 hours'
         WHERE id = $1 AND status = 'done'
         RETURNING *",
    )
    .bind(id)
    .bind(user.0.id)
    .fetch_optional(&state.pool)
    .await
    .map_err(internal)?;

    match reopened {
        Some(file) => Ok(Json(file)),
        None => {
            if file_exists(&state.pool, id).await? {
                Err(AppError::Conflict {
                    code: "not_done".into(),
                    message: "only done files can be reopened".into(),
                })
            } else {
                Err(AppError::NotFound(format!("file {id} not found")))
            }
        }
    }
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
    use crate::models::FileStatus;

    /// A body-less POST request authorized by the given cookie header value.
    fn post(uri: &str, cookie: &str) -> Request<Body> {
        Request::post(uri)
            .header(header::COOKIE, cookie)
            .body(Body::empty())
            .unwrap()
    }

    /// Drain a response into (status, raw bytes); 204 bodies are empty and
    /// must not be forced through a JSON parse.
    async fn drain(response: Response) -> (StatusCode, Vec<u8>) {
        let status = response.status();
        let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap()
            .to_vec();
        (status, bytes)
    }

    /// Create a project through the API and return its id.
    async fn create_project(app: &axum::Router, cookie: &str) -> Uuid {
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

    /// Insert a label fixture directly and return its id.
    async fn insert_label(pool: &PgPool, project_id: Uuid, archived: bool) -> Uuid {
        let id = Uuid::new_v4();
        sqlx::query(
            "INSERT INTO labels (id, project_id, name, color, sort_order, archived)
             VALUES ($1, $2, $3, '#e6194b', 0, $4)",
        )
        .bind(id)
        .bind(project_id)
        .bind(format!("метка-{}", id.simple()))
        .bind(archived)
        .execute(pool)
        .await
        .unwrap();
        id
    }

    /// Insert an audio_files row directly with an explicit claim state and
    /// duration.
    #[allow(clippy::too_many_arguments)]
    async fn insert_file(
        pool: &PgPool,
        project_id: Uuid,
        uploaded_by: Uuid,
        status: FileStatus,
        claimed_by: Option<Uuid>,
        claim_expires_at: Option<DateTime<Utc>>,
        duration_ms: Option<i32>,
    ) -> Uuid {
        let id = Uuid::new_v4();
        sqlx::query(
            "INSERT INTO audio_files (id, project_id, original_name, storage_key, sha256,
                mime, size_bytes, duration_ms, status, claimed_by, claim_expires_at, uploaded_by)
             VALUES ($1, $2, 'файл.wav', $3, $4, 'audio/wav', 10, $5, $6, $7, $8, $9)",
        )
        .bind(id)
        .bind(project_id)
        .bind(format!("media/{id}.wav"))
        .bind(Uuid::new_v4().simple().to_string())
        .bind(duration_ms)
        .bind(status)
        .bind(claimed_by)
        .bind(claim_expires_at)
        .bind(uploaded_by)
        .execute(pool)
        .await
        .unwrap();
        id
    }

    /// Insert an annotation fixture; `created_at` is explicit so fetches can
    /// be ordered deterministically.
    async fn insert_annotation(
        pool: &PgPool,
        file_id: Uuid,
        label_id: Uuid,
        start_ms: i32,
        end_ms: i32,
        created_by: Uuid,
        created_at: DateTime<Utc>,
    ) {
        sqlx::query(
            "INSERT INTO annotations (id, file_id, label_id, start_ms, end_ms, created_by,
                created_at, updated_at)
             VALUES ($1, $2, $3, $4, $5, $6, $7, $7)",
        )
        .bind(Uuid::new_v4())
        .bind(file_id)
        .bind(label_id)
        .bind(start_ms)
        .bind(end_ms)
        .bind(created_by)
        .bind(created_at)
        .execute(pool)
        .await
        .unwrap();
    }

    /// Load a file row; the caller asserts it exists.
    async fn file_row(pool: &PgPool, id: Uuid) -> AudioFile {
        sqlx::query_as("SELECT * FROM audio_files WHERE id = $1")
            .bind(id)
            .fetch_one(pool)
            .await
            .unwrap()
    }

    /// Segments stored on a file.
    async fn segment_count(pool: &PgPool, file_id: Uuid) -> i64 {
        sqlx::query_scalar("SELECT count(*) FROM annotations WHERE file_id = $1")
            .bind(file_id)
            .fetch_one(pool)
            .await
            .unwrap()
    }

    #[tokio::test]
    async fn queue_requires_a_session() {
        let pool = PgPool::connect_lazy("postgres://localhost/auau").unwrap();
        let app = crate::api::router(test_support::state(pool));

        for uri in [
            format!("/api/v1/projects/{}/queue/claim", Uuid::new_v4()),
            format!("/api/v1/files/{}/release", Uuid::new_v4()),
            format!("/api/v1/files/{}/complete", Uuid::new_v4()),
            format!("/api/v1/files/{}/reopen", Uuid::new_v4()),
        ] {
            let (status, body) = test_support::json(
                app.clone()
                    .oneshot(post(&uri, "auau_session=invalid"))
                    .await
                    .unwrap(),
            )
            .await;
            assert_eq!(status, StatusCode::UNAUTHORIZED, "{uri}");
            assert_eq!(body["error"]["code"], json!("unauthorized"));
        }
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn claim_race_single_file_one_wins(pool: PgPool) {
        let app = crate::api::router(test_support::state(pool.clone()));
        let (alice, alice_cookie) = test_support::signed_in(&pool).await.unwrap();
        let (bob, bob_cookie) = test_support::signed_in(&pool).await.unwrap();
        let project = create_project(&app, &alice_cookie).await;
        let file = insert_file(&pool, project, alice.id, FileStatus::New, None, None, None).await;

        let uri = format!("/api/v1/projects/{project}/queue/claim?order=created");
        let (alice_res, bob_res) = tokio::join!(
            app.clone().oneshot(post(&uri, &alice_cookie)),
            app.clone().oneshot(post(&uri, &bob_cookie)),
        );
        let (alice_status, alice_bytes) = drain(alice_res.unwrap()).await;
        let (bob_status, bob_bytes) = drain(bob_res.unwrap()).await;

        let (winner_body, winner_id) = if alice_status == StatusCode::OK {
            assert_eq!(bob_status, StatusCode::NO_CONTENT);
            assert!(bob_bytes.is_empty(), "204 must carry an empty body");
            (
                serde_json::from_slice::<Value>(&alice_bytes).unwrap(),
                alice.id,
            )
        } else {
            assert_eq!(alice_status, StatusCode::NO_CONTENT);
            assert!(alice_bytes.is_empty(), "204 must carry an empty body");
            assert_eq!(bob_status, StatusCode::OK);
            (serde_json::from_slice::<Value>(&bob_bytes).unwrap(), bob.id)
        };
        assert_eq!(winner_body["id"], json!(file.to_string()));
        assert_eq!(winner_body["status"], json!("in_progress"));
        assert_eq!(winner_body["claimed_by"], json!(winner_id.to_string()));
        assert!(
            winner_body["claim_expires_at"].is_string(),
            "the lease must carry an expiry: {winner_body}"
        );

        let row = file_row(&pool, file).await;
        assert_eq!(row.status, FileStatus::InProgress);
        assert_eq!(row.claimed_by, Some(winner_id));
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn claim_empty_queue_returns_204(pool: PgPool) {
        let app = crate::api::router(test_support::state(pool.clone()));
        let (user, cookie) = test_support::signed_in(&pool).await.unwrap();
        let project = create_project(&app, &cookie).await;

        let (status, bytes) = drain(
            app.clone()
                .oneshot(post(
                    &format!("/api/v1/projects/{project}/queue/claim"),
                    &cookie,
                ))
                .await
                .unwrap(),
        )
        .await;
        assert_eq!(status, StatusCode::NO_CONTENT);
        assert!(bytes.is_empty(), "204 must carry an empty body");

        insert_file(&pool, project, user.id, FileStatus::Error, None, None, None).await;
        let (status, bytes) = drain(
            app.clone()
                .oneshot(post(
                    &format!("/api/v1/projects/{project}/queue/claim"),
                    &cookie,
                ))
                .await
                .unwrap(),
        )
        .await;
        assert_eq!(status, StatusCode::NO_CONTENT);
        assert!(bytes.is_empty());

        let (status, body) = test_support::json(
            app.oneshot(post(
                &format!("/api/v1/projects/{}/queue/claim", Uuid::new_v4()),
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
    async fn claim_rejects_unknown_order_400(pool: PgPool) {
        let app = crate::api::router(test_support::state(pool.clone()));
        let (user, cookie) = test_support::signed_in(&pool).await.unwrap();
        let project = create_project(&app, &cookie).await;
        insert_file(&pool, project, user.id, FileStatus::New, None, None, None).await;
        insert_file(&pool, project, user.id, FileStatus::New, None, None, None).await;

        let (status, body) = test_support::json(
            app.clone()
                .oneshot(post(
                    &format!("/api/v1/projects/{project}/queue/claim?order=randomness"),
                    &cookie,
                ))
                .await
                .unwrap(),
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
        assert_eq!(body["error"]["code"], json!("invalid_order"));

        let fresh: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM audio_files WHERE project_id = $1 AND status = 'new'",
        )
        .bind(project)
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(fresh, 2);

        let (status, _) = test_support::json(
            app.clone()
                .oneshot(post(
                    &format!("/api/v1/projects/{project}/queue/claim"),
                    &cookie,
                ))
                .await
                .unwrap(),
        )
        .await;
        assert_eq!(status, StatusCode::OK);

        let (status, _) = test_support::json(
            app.clone()
                .oneshot(post(
                    &format!("/api/v1/projects/{project}/queue/claim?order=created"),
                    &cookie,
                ))
                .await
                .unwrap(),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn expired_lease_returns_file_keeps_annotations(pool: PgPool) {
        let app = crate::api::router(test_support::state(pool.clone()));
        let (alice, alice_cookie) = test_support::signed_in(&pool).await.unwrap();
        let (bob, bob_cookie) = test_support::signed_in(&pool).await.unwrap();
        let project = create_project(&app, &alice_cookie).await;
        let label = insert_label(&pool, project, false).await;

        let stale = insert_file(
            &pool,
            project,
            alice.id,
            FileStatus::InProgress,
            Some(alice.id),
            Some(Utc::now() - Duration::hours(24)),
            Some(60_000),
        )
        .await;
        let live = insert_file(
            &pool,
            project,
            alice.id,
            FileStatus::InProgress,
            Some(alice.id),
            Some(Utc::now() + Duration::hours(1)),
            None,
        )
        .await;
        insert_annotation(&pool, stale, label, 1_000, 2_000, alice.id, Utc::now()).await;

        let swept = crate::jobs::sweep_expired_leases(&pool).await.unwrap();
        assert_eq!(swept, 1, "only the expired lease may be swept");

        let row = file_row(&pool, stale).await;
        assert_eq!(row.status, FileStatus::New);
        assert_eq!(row.claimed_by, None);
        assert_eq!(row.claim_expires_at, None);

        assert_eq!(segment_count(&pool, stale).await, 1);

        let row = file_row(&pool, live).await;
        assert_eq!(row.status, FileStatus::InProgress);
        assert_eq!(row.claimed_by, Some(alice.id));

        assert_eq!(crate::jobs::sweep_expired_leases(&pool).await.unwrap(), 0);

        let (status, body) = test_support::json(
            app.clone()
                .oneshot(post(
                    &format!("/api/v1/projects/{project}/queue/claim"),
                    &bob_cookie,
                ))
                .await
                .unwrap(),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{body}");
        assert_eq!(body["id"], json!(stale.to_string()));
        assert_eq!(body["claimed_by"], json!(bob.id.to_string()));
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn complete_with_zero_segments_succeeds(pool: PgPool) {
        let app = crate::api::router(test_support::state(pool.clone()));
        let (alice, alice_cookie) = test_support::signed_in(&pool).await.unwrap();
        let (_bob, bob_cookie) = test_support::signed_in(&pool).await.unwrap();
        let project = create_project(&app, &alice_cookie).await;
        let file = insert_file(
            &pool,
            project,
            alice.id,
            FileStatus::New,
            None,
            None,
            Some(60_000),
        )
        .await;

        let (status, body) = test_support::json(
            app.clone()
                .oneshot(post(
                    &format!("/api/v1/projects/{project}/queue/claim"),
                    &alice_cookie,
                ))
                .await
                .unwrap(),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{body}");
        assert_eq!(body["id"], json!(file.to_string()));

        let (status, body) = test_support::json(
            app.clone()
                .oneshot(post(&format!("/api/v1/files/{file}/complete"), &bob_cookie))
                .await
                .unwrap(),
        )
        .await;
        assert_eq!(status, StatusCode::CONFLICT, "{body}");
        assert_eq!(body["error"]["code"], json!("lease_required"));

        let (status, body) = test_support::json(
            app.clone()
                .oneshot(post(
                    &format!("/api/v1/files/{file}/complete"),
                    &alice_cookie,
                ))
                .await
                .unwrap(),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{body}");
        assert_eq!(body["status"], json!("done"));
        assert_eq!(body["claimed_by"], json!(alice.id.to_string()));
        assert_eq!(body["claim_expires_at"], Value::Null);

        let row = file_row(&pool, file).await;
        assert_eq!(row.status, FileStatus::Done);
        assert_eq!(row.claimed_by, Some(alice.id), "attribution is kept");
        assert_eq!(row.claim_expires_at, None);
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn complete_invalid_segments_422(pool: PgPool) {
        let app = crate::api::router(test_support::state(pool.clone()));
        let (alice, alice_cookie) = test_support::signed_in(&pool).await.unwrap();
        let project = create_project(&app, &alice_cookie).await;
        let other = create_project(&app, &alice_cookie).await;
        let home_label = insert_label(&pool, project, false).await;
        let foreign_label = insert_label(&pool, other, false).await;

        sqlx::query("ALTER TABLE annotations DROP CONSTRAINT annotations_check")
            .execute(&pool)
            .await
            .unwrap();

        let file = insert_file(
            &pool,
            project,
            alice.id,
            FileStatus::New,
            None,
            None,
            Some(30_000),
        )
        .await;
        let (status, body) = test_support::json(
            app.clone()
                .oneshot(post(
                    &format!("/api/v1/projects/{project}/queue/claim"),
                    &alice_cookie,
                ))
                .await
                .unwrap(),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{body}");

        insert_annotation(
            &pool,
            file,
            home_label,
            1_000,
            500,
            alice.id,
            Utc::now() - Duration::seconds(10),
        )
        .await;
        insert_annotation(
            &pool,
            file,
            foreign_label,
            0,
            2_000,
            alice.id,
            Utc::now() - Duration::seconds(5),
        )
        .await;

        sqlx::query(
            "UPDATE audio_files SET claim_expires_at = now() + interval '5 minutes' WHERE id = $1",
        )
        .bind(file)
        .execute(&pool)
        .await
        .unwrap();

        let (status, body) = test_support::json(
            app.clone()
                .oneshot(post(
                    &format!("/api/v1/files/{file}/complete"),
                    &alice_cookie,
                ))
                .await
                .unwrap(),
        )
        .await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
        assert_eq!(body["error"]["code"], json!("unprocessable"));
        let errors = body["error"]["errors"].as_array().unwrap();
        assert_eq!(errors.len(), 2, "{body}");
        assert_eq!(errors[0]["field"], json!("segments[0].endMs"));
        assert_eq!(
            errors[0]["message"],
            json!("end must be greater than start")
        );
        assert_eq!(errors[1]["field"], json!("segments[1].labelId"));
        assert_eq!(
            errors[1]["message"],
            json!("label does not belong to this project")
        );

        let row = file_row(&pool, file).await;
        assert_eq!(row.status, FileStatus::InProgress);
        assert_eq!(row.claimed_by, Some(alice.id));
        assert!(
            row.claim_expires_at.unwrap() > Utc::now() + Duration::hours(1),
            "complete must renew the lease: {:?}",
            row.claim_expires_at
        );
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn release_by_non_owner_409(pool: PgPool) {
        let app = crate::api::router(test_support::state(pool.clone()));
        let (alice, alice_cookie) = test_support::signed_in(&pool).await.unwrap();
        let (_bob, bob_cookie) = test_support::signed_in(&pool).await.unwrap();
        let project = create_project(&app, &alice_cookie).await;
        let label = insert_label(&pool, project, false).await;

        let claimed = insert_file(
            &pool,
            project,
            alice.id,
            FileStatus::InProgress,
            Some(alice.id),
            Some(Utc::now() + Duration::hours(1)),
            Some(60_000),
        )
        .await;
        insert_annotation(&pool, claimed, label, 0, 1_000, alice.id, Utc::now()).await;

        let (status, body) = test_support::json(
            app.clone()
                .oneshot(post(
                    &format!("/api/v1/files/{claimed}/release"),
                    &bob_cookie,
                ))
                .await
                .unwrap(),
        )
        .await;
        assert_eq!(status, StatusCode::CONFLICT, "{body}");
        assert_eq!(body["error"]["code"], json!("lease_required"));
        let row = file_row(&pool, claimed).await;
        assert_eq!(row.status, FileStatus::InProgress);
        assert_eq!(row.claimed_by, Some(alice.id));

        let expired = insert_file(
            &pool,
            project,
            alice.id,
            FileStatus::InProgress,
            Some(alice.id),
            Some(Utc::now() - Duration::minutes(1)),
            None,
        )
        .await;
        let (status, body) = test_support::json(
            app.clone()
                .oneshot(post(
                    &format!("/api/v1/files/{expired}/release"),
                    &alice_cookie,
                ))
                .await
                .unwrap(),
        )
        .await;
        assert_eq!(status, StatusCode::CONFLICT, "{body}");
        assert_eq!(body["error"]["code"], json!("lease_required"));

        let done = insert_file(
            &pool,
            project,
            alice.id,
            FileStatus::Done,
            Some(alice.id),
            None,
            None,
        )
        .await;
        let (status, _) = test_support::json(
            app.clone()
                .oneshot(post(
                    &format!("/api/v1/files/{done}/release"),
                    &alice_cookie,
                ))
                .await
                .unwrap(),
        )
        .await;
        assert_eq!(status, StatusCode::CONFLICT);

        let (status, body) = test_support::json(
            app.clone()
                .oneshot(post(
                    &format!("/api/v1/files/{claimed}/release"),
                    &alice_cookie,
                ))
                .await
                .unwrap(),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{body}");
        assert_eq!(body["status"], json!("new"));
        assert_eq!(body["claimed_by"], Value::Null);
        assert_eq!(body["claim_expires_at"], Value::Null);
        assert_eq!(segment_count(&pool, claimed).await, 1);

        let (status, body) = test_support::json(
            app.oneshot(post(
                &format!("/api/v1/files/{}/release", Uuid::new_v4()),
                &alice_cookie,
            ))
            .await
            .unwrap(),
        )
        .await;
        assert_eq!(status, StatusCode::NOT_FOUND);
        assert_eq!(body["error"]["code"], json!("not_found"));
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn reopen_resets_to_in_progress(pool: PgPool) {
        let app = crate::api::router(test_support::state(pool.clone()));
        let (alice, alice_cookie) = test_support::signed_in(&pool).await.unwrap();
        let (bob, bob_cookie) = test_support::signed_in(&pool).await.unwrap();
        let project = create_project(&app, &alice_cookie).await;
        let label = insert_label(&pool, project, false).await;

        let done = insert_file(
            &pool,
            project,
            alice.id,
            FileStatus::Done,
            Some(alice.id),
            None,
            Some(60_000),
        )
        .await;
        insert_annotation(&pool, done, label, 1_000, 2_000, alice.id, Utc::now()).await;

        let (status, body) = test_support::json(
            app.clone()
                .oneshot(post(&format!("/api/v1/files/{done}/reopen"), &bob_cookie))
                .await
                .unwrap(),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{body}");
        assert_eq!(body["status"], json!("in_progress"));
        assert_eq!(body["claimed_by"], json!(bob.id.to_string()));
        let expires: DateTime<Utc> = body["claim_expires_at"].as_str().unwrap().parse().unwrap();
        assert!(
            expires > Utc::now() + Duration::hours(1),
            "reopen must grant a fresh 2h lease: {expires}"
        );

        assert_eq!(segment_count(&pool, done).await, 1);

        let fresh = insert_file(&pool, project, alice.id, FileStatus::New, None, None, None).await;
        let (status, body) = test_support::json(
            app.clone()
                .oneshot(post(
                    &format!("/api/v1/files/{fresh}/reopen"),
                    &alice_cookie,
                ))
                .await
                .unwrap(),
        )
        .await;
        assert_eq!(status, StatusCode::CONFLICT, "{body}");
        assert_eq!(body["error"]["code"], json!("not_done"));

        let (status, body) = test_support::json(
            app.oneshot(post(
                &format!("/api/v1/files/{}/reopen", Uuid::new_v4()),
                &alice_cookie,
            ))
            .await
            .unwrap(),
        )
        .await;
        assert_eq!(status, StatusCode::NOT_FOUND);
        assert_eq!(body["error"]["code"], json!("not_found"));
    }
}
