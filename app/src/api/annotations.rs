//! Reading and wholesale replacing the segments of one file.
//!
//! The PUT is a full replacement: the whole segment list arrives in one
//! request, is validated against the project palette, and — only under the
//! caller's live lease — swaps in for the stored segments inside a single
//! transaction. Every touch of the file (GET included) renews the lease
//! owner's expiry, so a failed validation still leaves the caller
//! a fresh 2-hour lease to keep annotating.

use axum::extract::{Path, State};
use axum::routing::get;
use axum::{Json, Router};
use serde::Serialize;
use sqlx::FromRow;
use uuid::Uuid;

use crate::api::AppState;
use crate::auth::CurrentUser;
use crate::error::AppError;
use crate::models::{AudioFile, Label};
use crate::validate::{validate_segments, SegmentInput};

/// Annotation routes; both require a signed-in user.
///
/// POST is an alias of PUT for `navigator.sendBeacon`, which cannot send
/// PUT: the browser's beforeunload flush posts the exact same camelCase
/// body, so it goes through the same [`replace`] handler and lease checks.
pub fn router() -> Router<AppState> {
    Router::new().route(
        "/api/v1/files/{id}/annotations",
        get(list).put(replace).post(replace),
    )
}

/// A stored segment as it travels over the wire: snake_case (unlike the
/// camelCase `SegmentInput` the PUT accepts).
#[derive(Debug, Serialize, FromRow)]
struct AnnotationDto {
    id: Uuid,
    label_id: Uuid,
    start_ms: i32,
    end_ms: i32,
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

/// A touch of a file: a conditional UPDATE that matches only
/// when the caller owns a live lease — `in_progress`, claimed by them, not
/// expired (a missing expiry counts as live) — and renews it for another
/// 2 hours. Returns the file row to the lease owner, `None` to everyone
/// else. The same statement guards the queue's `complete`.
async fn renew_lease(
    pool: &sqlx::PgPool,
    id: Uuid,
    user_id: Uuid,
) -> Result<Option<AudioFile>, AppError> {
    sqlx::query_as(
        "UPDATE audio_files
         SET claim_expires_at = now() + interval '2 hours'
         WHERE id = $1 AND status = 'in_progress' AND claimed_by = $2
           AND (claim_expires_at IS NULL OR claim_expires_at > now())
         RETURNING *",
    )
    .bind(id)
    .bind(user_id)
    .fetch_optional(pool)
    .await
    .map_err(internal)
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

/// `GET /api/v1/files/:id/annotations` — the stored segments in timeline
/// order.
///
/// Any signed-in user may read; only the lease owner's touch renews the
/// lease — everyone else's UPDATE simply matches no rows.
async fn list(
    State(state): State<AppState>,
    user: CurrentUser,
    Path(id): Path<Uuid>,
) -> Result<Json<Vec<AnnotationDto>>, AppError> {
    if !file_exists(&state.pool, id).await? {
        return Err(AppError::NotFound(format!("file {id} not found")));
    }
    renew_lease(&state.pool, id, user.0.id).await?;

    let segments: Vec<AnnotationDto> = sqlx::query_as(
        "SELECT id, label_id, start_ms, end_ms FROM annotations
         WHERE file_id = $1
         ORDER BY start_ms, end_ms, id",
    )
    .bind(id)
    .fetch_all(&state.pool)
    .await
    .map_err(internal)?;
    Ok(Json(segments))
}

/// `PUT /api/v1/files/:id/annotations` — replace every segment of the file
/// with the request body (an empty list wipes them: "no events").
///
/// Only the owner of a live lease may write. Validation happens before
/// anything is deleted, so a rejected body leaves the stored segments
/// untouched — and, the touch having renewed the lease, a fresh 2 hours to
/// fix it. The swap itself is one transaction: DELETE all, INSERT all.
async fn replace(
    State(state): State<AppState>,
    user: CurrentUser,
    Path(id): Path<Uuid>,
    Json(segments): Json<Vec<SegmentInput>>,
) -> Result<Json<Vec<AnnotationDto>>, AppError> {
    let file: AudioFile = match renew_lease(&state.pool, id, user.0.id).await? {
        Some(file) => file,
        None => return Err(conflict_unless_missing(&state.pool, id).await?),
    };

    let labels: Vec<Label> = sqlx::query_as("SELECT * FROM labels WHERE project_id = $1")
        .bind(file.project_id)
        .fetch_all(&state.pool)
        .await
        .map_err(internal)?;
    validate_segments(&segments, &labels, file.duration_ms)
        .map_err(|errors| AppError::Unprocessable { errors })?;

    let mut tx = state.pool.begin().await.map_err(internal)?;
    sqlx::query("DELETE FROM annotations WHERE file_id = $1")
        .bind(id)
        .execute(&mut *tx)
        .await
        .map_err(internal)?;
    let mut stored = Vec::with_capacity(segments.len());
    for seg in &segments {
        let dto: AnnotationDto = sqlx::query_as(
            "INSERT INTO annotations (id, file_id, label_id, start_ms, end_ms, created_by)
             VALUES ($1, $2, $3, $4, $5, $6)
             RETURNING id, label_id, start_ms, end_ms",
        )
        .bind(Uuid::new_v4())
        .bind(id)
        .bind(seg.label_id)
        .bind(seg.start_ms)
        .bind(seg.end_ms)
        .bind(user.0.id)
        .fetch_one(&mut *tx)
        .await
        .map_err(internal)?;
        stored.push(dto);
    }
    tx.commit().await.map_err(internal)?;

    Ok(Json(stored))
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::StatusCode;
    use chrono::{DateTime, Duration, Utc};
    use serde_json::json;
    use sqlx::PgPool;
    use tower::ServiceExt;

    use crate::api::test_support;
    use crate::models::FileStatus;

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
    async fn insert_label(pool: &PgPool, project_id: Uuid) -> Uuid {
        let id = Uuid::new_v4();
        sqlx::query(
            "INSERT INTO labels (id, project_id, name, color, sort_order, archived)
             VALUES ($1, $2, $3, '#e6194b', 0, false)",
        )
        .bind(id)
        .bind(project_id)
        .bind(format!("метка-{}", id.simple()))
        .execute(pool)
        .await
        .unwrap();
        id
    }

    /// Insert an audio_files row directly with an explicit claim state and
    /// duration.
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

    /// A file claimed by `user` under a live 1-hour lease — the state every
    /// annotation writer works in.
    async fn claimed_file(
        pool: &PgPool,
        project_id: Uuid,
        user_id: Uuid,
        duration_ms: Option<i32>,
    ) -> Uuid {
        insert_file(
            pool,
            project_id,
            user_id,
            FileStatus::InProgress,
            Some(user_id),
            Some(Utc::now() + Duration::hours(1)),
            duration_ms,
        )
        .await
    }

    /// Insert an annotation fixture and return its id.
    async fn insert_annotation(
        pool: &PgPool,
        file_id: Uuid,
        label_id: Uuid,
        start_ms: i32,
        end_ms: i32,
        created_by: Uuid,
    ) -> Uuid {
        let id = Uuid::new_v4();
        sqlx::query(
            "INSERT INTO annotations (id, file_id, label_id, start_ms, end_ms, created_by)
             VALUES ($1, $2, $3, $4, $5, $6)",
        )
        .bind(id)
        .bind(file_id)
        .bind(label_id)
        .bind(start_ms)
        .bind(end_ms)
        .bind(created_by)
        .execute(pool)
        .await
        .unwrap();
        id
    }

    /// Segments stored on a file, as raw rows: (label_id, start_ms, end_ms,
    /// created_by) in storage order.
    async fn stored_segments(pool: &PgPool, file_id: Uuid) -> Vec<(Uuid, i32, i32, Uuid)> {
        sqlx::query_as(
            "SELECT label_id, start_ms, end_ms, created_by FROM annotations
             WHERE file_id = $1 ORDER BY start_ms, end_ms, id",
        )
        .bind(file_id)
        .fetch_all(pool)
        .await
        .unwrap()
    }

    /// The current lease expiry of a file; the caller asserts it exists.
    async fn lease_expiry(pool: &PgPool, file_id: Uuid) -> Option<DateTime<Utc>> {
        sqlx::query_scalar("SELECT claim_expires_at FROM audio_files WHERE id = $1")
            .bind(file_id)
            .fetch_one(pool)
            .await
            .unwrap()
    }

    /// Pin the lease expiry of a file to `now + 5 minutes` — close enough to
    /// expire that a renewal to +2h is observable.
    async fn shorten_lease(pool: &PgPool, file_id: Uuid) {
        sqlx::query(
            "UPDATE audio_files SET claim_expires_at = now() + interval '5 minutes'
             WHERE id = $1",
        )
        .bind(file_id)
        .execute(pool)
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn annotations_require_a_session() {
        let pool = PgPool::connect_lazy("postgres://localhost/auau").unwrap();
        let app = crate::api::build_router(test_support::state(pool));
        let uri = format!("/api/v1/files/{}/annotations", Uuid::new_v4());

        let (status, body) = test_support::json(
            app.clone()
                .oneshot(test_support::get(&uri, "auau_session=invalid"))
                .await
                .unwrap(),
        )
        .await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
        assert_eq!(body["error"]["code"], json!("unauthorized"));

        let (status, body) = test_support::json(
            app.oneshot(test_support::put_json(
                &uri,
                "auau_session=invalid",
                json!([]),
            ))
            .await
            .unwrap(),
        )
        .await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
        assert_eq!(body["error"]["code"], json!("unauthorized"));
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn get_returns_segments(pool: PgPool) {
        let app = crate::api::build_router(test_support::state(pool.clone()));
        let (alice, alice_cookie) = test_support::signed_in(&pool).await.unwrap();
        let project = create_project(&app, &alice_cookie).await;
        let label_a = insert_label(&pool, project).await;
        let label_b = insert_label(&pool, project).await;
        let file = claimed_file(&pool, project, alice.id, Some(60_000)).await;

        let id_b = insert_annotation(&pool, file, label_b, 2_000, 3_000, alice.id).await;
        let id_a = insert_annotation(&pool, file, label_a, 0, 1_000, alice.id).await;

        let (status, body) = test_support::json(
            app.clone()
                .oneshot(test_support::get(
                    &format!("/api/v1/files/{file}/annotations"),
                    &alice_cookie,
                ))
                .await
                .unwrap(),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{body}");
        assert_eq!(
            body,
            json!([
                {"id": id_a.to_string(), "label_id": label_a.to_string(),
                 "start_ms": 0, "end_ms": 1_000},
                {"id": id_b.to_string(), "label_id": label_b.to_string(),
                 "start_ms": 2_000, "end_ms": 3_000},
            ])
        );

        let (status, body) = test_support::json(
            app.oneshot(test_support::get(
                &format!("/api/v1/files/{}/annotations", Uuid::new_v4()),
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
    async fn put_replaces_all_segments(pool: PgPool) {
        let app = crate::api::build_router(test_support::state(pool.clone()));
        let (alice, alice_cookie) = test_support::signed_in(&pool).await.unwrap();
        let project = create_project(&app, &alice_cookie).await;
        let label_a = insert_label(&pool, project).await;
        let label_b = insert_label(&pool, project).await;
        let file = claimed_file(&pool, project, alice.id, Some(60_000)).await;

        let v1 = json!([
            {"labelId": label_a.to_string(), "startMs": 0, "endMs": 1_000},
            {"labelId": label_b.to_string(), "startMs": 2_000, "endMs": 3_000},
        ]);
        let (status, body) = test_support::json(
            app.clone()
                .oneshot(test_support::put_json(
                    &format!("/api/v1/files/{file}/annotations"),
                    &alice_cookie,
                    v1,
                ))
                .await
                .unwrap(),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{body}");
        assert_eq!(body.as_array().unwrap().len(), 2, "{body}");
        assert_eq!(body[0]["label_id"], json!(label_a.to_string()));
        assert_eq!(body[1]["label_id"], json!(label_b.to_string()));
        assert!(body[0]["id"].as_str().is_some(), "ids come back: {body}");

        let v2 = json!([
            {"labelId": label_b.to_string(), "startMs": 5_000, "endMs": 6_000},
        ]);
        let (status, body) = test_support::json(
            app.clone()
                .oneshot(test_support::put_json(
                    &format!("/api/v1/files/{file}/annotations"),
                    &alice_cookie,
                    v2,
                ))
                .await
                .unwrap(),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{body}");

        let rows = stored_segments(&pool, file).await;
        assert_eq!(rows, vec![(label_b, 5_000, 6_000, alice.id)]);
    }

    /// POST is the sendBeacon alias of PUT (the browser's beforeunload flush
    /// cannot send PUT): the same camelCase body replaces all segments and
    /// the lease rules apply unchanged.
    #[sqlx::test(migrations = "./migrations")]
    async fn post_alias_replaces_segments_and_requires_lease(pool: PgPool) {
        let app = crate::api::build_router(test_support::state(pool.clone()));
        let (alice, alice_cookie) = test_support::signed_in(&pool).await.unwrap();
        let project = create_project(&app, &alice_cookie).await;
        let label_a = insert_label(&pool, project).await;
        let label_b = insert_label(&pool, project).await;
        let file = claimed_file(&pool, project, alice.id, Some(60_000)).await;

        insert_annotation(&pool, file, label_a, 0, 1_000, alice.id).await;

        let beacon_body = json!([
            {"labelId": label_b.to_string(), "startMs": 2_000, "endMs": 3_000},
        ]);
        let (status, body) = test_support::json(
            app.clone()
                .oneshot(test_support::post_json(
                    &format!("/api/v1/files/{file}/annotations"),
                    &alice_cookie,
                    beacon_body,
                ))
                .await
                .unwrap(),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{body}");
        assert_eq!(body.as_array().unwrap().len(), 1, "{body}");
        assert_eq!(body[0]["label_id"], json!(label_b.to_string()));
        assert_eq!(body[0]["start_ms"], json!(2_000));
        assert_eq!(body[0]["end_ms"], json!(3_000));
        assert_eq!(
            stored_segments(&pool, file).await,
            vec![(label_b, 2_000, 3_000, alice.id)]
        );

        let fresh = insert_file(
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
                .oneshot(test_support::post_json(
                    &format!("/api/v1/files/{fresh}/annotations"),
                    &alice_cookie,
                    json!([{"labelId": label_b.to_string(), "startMs": 0, "endMs": 1_000}]),
                ))
                .await
                .unwrap(),
        )
        .await;
        assert_eq!(status, StatusCode::CONFLICT, "{body}");
        assert_eq!(body["error"]["code"], json!("lease_required"));
        assert_eq!(stored_segments(&pool, fresh).await, vec![]);
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn put_empty_array_clears_segments(pool: PgPool) {
        let app = crate::api::build_router(test_support::state(pool.clone()));
        let (alice, alice_cookie) = test_support::signed_in(&pool).await.unwrap();
        let project = create_project(&app, &alice_cookie).await;
        let label = insert_label(&pool, project).await;
        let file = claimed_file(&pool, project, alice.id, Some(60_000)).await;
        insert_annotation(&pool, file, label, 0, 1_000, alice.id).await;
        insert_annotation(&pool, file, label, 2_000, 3_000, alice.id).await;

        let (status, body) = test_support::json(
            app.clone()
                .oneshot(test_support::put_json(
                    &format!("/api/v1/files/{file}/annotations"),
                    &alice_cookie,
                    json!([]),
                ))
                .await
                .unwrap(),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{body}");
        assert_eq!(body, json!([]));
        assert_eq!(stored_segments(&pool, file).await, vec![]);
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn put_rejects_end_before_start_422(pool: PgPool) {
        let app = crate::api::build_router(test_support::state(pool.clone()));
        let (alice, alice_cookie) = test_support::signed_in(&pool).await.unwrap();
        let project = create_project(&app, &alice_cookie).await;
        let label = insert_label(&pool, project).await;
        let file = claimed_file(&pool, project, alice.id, Some(60_000)).await;
        insert_annotation(&pool, file, label, 0, 1_000, alice.id).await;

        let (status, body) = test_support::json(
            app.clone()
                .oneshot(test_support::put_json(
                    &format!("/api/v1/files/{file}/annotations"),
                    &alice_cookie,
                    json!([{"labelId": label.to_string(), "startMs": 2_000, "endMs": 1_000}]),
                ))
                .await
                .unwrap(),
        )
        .await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
        assert_eq!(body["error"]["code"], json!("unprocessable"));
        assert_eq!(
            body["error"]["errors"][0]["field"],
            json!("segments[0].endMs")
        );
        assert_eq!(
            body["error"]["errors"][0]["message"],
            json!("end must be greater than start")
        );

        assert_eq!(
            stored_segments(&pool, file).await,
            vec![(label, 0, 1_000, alice.id)]
        );
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn put_rejects_label_of_other_project_422(pool: PgPool) {
        let app = crate::api::build_router(test_support::state(pool.clone()));
        let (alice, alice_cookie) = test_support::signed_in(&pool).await.unwrap();
        let project = create_project(&app, &alice_cookie).await;
        let other = create_project(&app, &alice_cookie).await;
        let foreign_label = insert_label(&pool, other).await;
        let file = claimed_file(&pool, project, alice.id, Some(60_000)).await;

        let (status, body) = test_support::json(
            app.clone()
                .oneshot(test_support::put_json(
                    &format!("/api/v1/files/{file}/annotations"),
                    &alice_cookie,
                    json!([{"labelId": foreign_label.to_string(), "startMs": 0, "endMs": 1_000}]),
                ))
                .await
                .unwrap(),
        )
        .await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
        assert_eq!(
            body["error"]["errors"][0]["field"],
            json!("segments[0].labelId")
        );
        assert_eq!(
            body["error"]["errors"][0]["message"],
            json!("label does not belong to this project")
        );
        assert_eq!(stored_segments(&pool, file).await, vec![]);
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn put_rejects_out_of_duration_422(pool: PgPool) {
        let app = crate::api::build_router(test_support::state(pool.clone()));
        let (alice, alice_cookie) = test_support::signed_in(&pool).await.unwrap();
        let project = create_project(&app, &alice_cookie).await;
        let label = insert_label(&pool, project).await;
        let file = claimed_file(&pool, project, alice.id, Some(30_000)).await;

        let (status, body) = test_support::json(
            app.clone()
                .oneshot(test_support::put_json(
                    &format!("/api/v1/files/{file}/annotations"),
                    &alice_cookie,
                    json!([{"labelId": label.to_string(), "startMs": 0, "endMs": 40_000}]),
                ))
                .await
                .unwrap(),
        )
        .await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
        assert_eq!(
            body["error"]["errors"][0]["field"],
            json!("segments[0].endMs")
        );
        assert_eq!(
            body["error"]["errors"][0]["message"],
            json!("end must not exceed the file duration")
        );
        assert_eq!(stored_segments(&pool, file).await, vec![]);
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn put_without_lease_409(pool: PgPool) {
        let app = crate::api::build_router(test_support::state(pool.clone()));
        let (alice, alice_cookie) = test_support::signed_in(&pool).await.unwrap();
        let project = create_project(&app, &alice_cookie).await;
        let label = insert_label(&pool, project).await;

        let fresh = insert_file(
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
                .oneshot(test_support::put_json(
                    &format!("/api/v1/files/{fresh}/annotations"),
                    &alice_cookie,
                    json!([{"labelId": label.to_string(), "startMs": 0, "endMs": 1_000}]),
                ))
                .await
                .unwrap(),
        )
        .await;
        assert_eq!(status, StatusCode::CONFLICT, "{body}");
        assert_eq!(body["error"]["code"], json!("lease_required"));
        assert_eq!(stored_segments(&pool, fresh).await, vec![]);
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn put_after_done_409(pool: PgPool) {
        let app = crate::api::build_router(test_support::state(pool.clone()));
        let (alice, alice_cookie) = test_support::signed_in(&pool).await.unwrap();
        let (_bob, bob_cookie) = test_support::signed_in(&pool).await.unwrap();
        let project = create_project(&app, &alice_cookie).await;
        let label = insert_label(&pool, project).await;
        let payload = json!([{"labelId": label.to_string(), "startMs": 0, "endMs": 1_000}]);

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
        let (status, res) = test_support::json(
            app.clone()
                .oneshot(test_support::put_json(
                    &format!("/api/v1/files/{done}/annotations"),
                    &alice_cookie,
                    payload.clone(),
                ))
                .await
                .unwrap(),
        )
        .await;
        assert_eq!(status, StatusCode::CONFLICT, "{res}");
        assert_eq!(res["error"]["code"], json!("lease_required"));

        let expired = insert_file(
            &pool,
            project,
            alice.id,
            FileStatus::InProgress,
            Some(alice.id),
            Some(Utc::now() - Duration::minutes(1)),
            Some(60_000),
        )
        .await;
        let (status, _) = test_support::json(
            app.clone()
                .oneshot(test_support::put_json(
                    &format!("/api/v1/files/{expired}/annotations"),
                    &alice_cookie,
                    payload.clone(),
                ))
                .await
                .unwrap(),
        )
        .await;
        assert_eq!(status, StatusCode::CONFLICT);

        let live = claimed_file(&pool, project, alice.id, Some(60_000)).await;
        let (status, _) = test_support::json(
            app.clone()
                .oneshot(test_support::put_json(
                    &format!("/api/v1/files/{live}/annotations"),
                    &bob_cookie,
                    payload.clone(),
                ))
                .await
                .unwrap(),
        )
        .await;
        assert_eq!(status, StatusCode::CONFLICT);
        assert_eq!(stored_segments(&pool, live).await, vec![]);

        let (status, res) = test_support::json(
            app.oneshot(test_support::put_json(
                &format!("/api/v1/files/{}/annotations", Uuid::new_v4()),
                &bob_cookie,
                payload.clone(),
            ))
            .await
            .unwrap(),
        )
        .await;
        assert_eq!(status, StatusCode::NOT_FOUND);
        assert_eq!(res["error"]["code"], json!("not_found"));
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn get_put_renew_lease(pool: PgPool) {
        let app = crate::api::build_router(test_support::state(pool.clone()));
        let (alice, alice_cookie) = test_support::signed_in(&pool).await.unwrap();
        let (_bob, bob_cookie) = test_support::signed_in(&pool).await.unwrap();
        let project = create_project(&app, &alice_cookie).await;
        let label = insert_label(&pool, project).await;
        let file = claimed_file(&pool, project, alice.id, Some(60_000)).await;
        let uri = format!("/api/v1/files/{file}/annotations");

        shorten_lease(&pool, file).await;
        assert!(lease_expiry(&pool, file).await.unwrap() < Utc::now() + Duration::minutes(10));

        let (status, _) = test_support::json(
            app.clone()
                .oneshot(test_support::get(&uri, &bob_cookie))
                .await
                .unwrap(),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert!(
            lease_expiry(&pool, file).await.unwrap() < Utc::now() + Duration::minutes(10),
            "only the owner's touch renews the lease"
        );

        let (status, _) = test_support::json(
            app.clone()
                .oneshot(test_support::get(&uri, &alice_cookie))
                .await
                .unwrap(),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert!(
            lease_expiry(&pool, file).await.unwrap() > Utc::now() + Duration::hours(1),
            "GET must renew the owner's lease"
        );

        shorten_lease(&pool, file).await;
        let (status, _) = test_support::json(
            app.clone()
                .oneshot(test_support::put_json(
                    &uri,
                    &alice_cookie,
                    json!([{"labelId": label.to_string(), "startMs": 0, "endMs": 1_000}]),
                ))
                .await
                .unwrap(),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert!(
            lease_expiry(&pool, file).await.unwrap() > Utc::now() + Duration::hours(1),
            "PUT must renew the owner's lease"
        );
    }
}
