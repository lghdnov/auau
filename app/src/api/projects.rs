use std::collections::HashMap;

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::routing::get;
use axum::{Json, Router};
use serde::Deserialize;
use sqlx::FromRow;
use uuid::Uuid;

use crate::api::dto::{FileCounts, ProjectDetail, ProjectSummary};
use crate::api::AppState;
use crate::auth::CurrentUser;
use crate::error::AppError;
use crate::models::{FileStatus, Label, Project};

/// Project routes; all of them require a signed-in user.
pub fn router() -> Router<AppState> {
    Router::new()
        .route("/api/v1/projects", get(list).post(create))
        .route("/api/v1/projects/{id}", get(detail))
}

/// Body of `POST /api/v1/projects`.
#[derive(Debug, Deserialize)]
struct CreateProject {
    name: String,
    description: Option<String>,
}

/// One row of the per-project `GROUP BY status` count query.
#[derive(FromRow)]
struct ProjectStatusCount {
    project_id: Uuid,
    status: FileStatus,
    count: i64,
}

/// One row of the single-project `GROUP BY status` count query.
#[derive(FromRow)]
struct StatusCount {
    status: FileStatus,
    count: i64,
}

/// Map a raw sqlx failure onto the internal-error variant.
fn internal(err: sqlx::Error) -> AppError {
    AppError::Internal(err.into())
}

/// `GET /api/v1/projects` — every project with its per-status file counters.
async fn list(
    State(state): State<AppState>,
    _user: CurrentUser,
) -> Result<Json<Vec<ProjectSummary>>, AppError> {
    let projects: Vec<Project> = sqlx::query_as("SELECT * FROM projects ORDER BY created_at, id")
        .fetch_all(&state.pool)
        .await
        .map_err(internal)?;

    let rows: Vec<ProjectStatusCount> = sqlx::query_as(
        "SELECT af.project_id, af.status, count(*) AS count
         FROM audio_files af
         JOIN projects p ON p.id = af.project_id
         GROUP BY af.project_id, af.status",
    )
    .fetch_all(&state.pool)
    .await
    .map_err(internal)?;
    let mut counts: HashMap<Uuid, FileCounts> = HashMap::new();
    for row in rows {
        counts
            .entry(row.project_id)
            .or_default()
            .add(row.status, row.count);
    }

    let summaries = projects
        .into_iter()
        .map(|project| {
            let counts = counts.remove(&project.id).unwrap_or_default();
            ProjectSummary { project, counts }
        })
        .collect();
    Ok(Json(summaries))
}

/// `POST /api/v1/projects` — create a project for the current user.
async fn create(
    State(state): State<AppState>,
    user: CurrentUser,
    Json(payload): Json<CreateProject>,
) -> Result<(StatusCode, Json<Project>), AppError> {
    let name = payload.name.trim();
    if name.is_empty() {
        return Err(AppError::BadRequest {
            code: "invalid_name".into(),
            message: "project name must not be empty or whitespace".into(),
        });
    }

    let inserted = sqlx::query_as::<_, Project>(
        "INSERT INTO projects (id, name, description, created_by)
         VALUES ($1, $2, $3, $4)
         RETURNING *",
    )
    .bind(Uuid::new_v4())
    .bind(name)
    .bind(&payload.description)
    .bind(user.0.id)
    .fetch_one(&state.pool)
    .await;

    match inserted {
        Ok(project) => Ok((StatusCode::CREATED, Json(project))),
        Err(sqlx::Error::Database(db)) if db.is_unique_violation() => Err(AppError::Conflict {
            code: "duplicate".into(),
            message: format!("a project named '{name}' already exists"),
        }),
        Err(err) => Err(internal(err)),
    }
}

/// `GET /api/v1/projects/:id` — project, live labels, per-status counters.
async fn detail(
    State(state): State<AppState>,
    _user: CurrentUser,
    Path(id): Path<Uuid>,
) -> Result<Json<ProjectDetail>, AppError> {
    let project: Project = sqlx::query_as("SELECT * FROM projects WHERE id = $1")
        .bind(id)
        .fetch_optional(&state.pool)
        .await
        .map_err(internal)?
        .ok_or_else(|| AppError::NotFound(format!("project {id} not found")))?;

    let labels: Vec<Label> = sqlx::query_as(
        "SELECT * FROM labels WHERE project_id = $1 AND NOT archived ORDER BY sort_order",
    )
    .bind(id)
    .fetch_all(&state.pool)
    .await
    .map_err(internal)?;

    let rows: Vec<StatusCount> = sqlx::query_as(
        "SELECT status, count(*) AS count
         FROM audio_files
         WHERE project_id = $1
         GROUP BY status",
    )
    .bind(id)
    .fetch_all(&state.pool)
    .await
    .map_err(internal)?;
    let mut counts = FileCounts::default();
    for row in rows {
        counts.add(row.status, row.count);
    }

    Ok(Json(ProjectDetail {
        project,
        labels,
        counts,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use serde_json::{json, Value};
    use sqlx::PgPool;
    use tower::ServiceExt;

    use crate::api::test_support;

    fn zero_counts() -> Value {
        json!({
            "new": 0,
            "in_progress": 0,
            "done": 0,
            "error": 0,
        })
    }

    #[tokio::test]
    async fn projects_require_a_session() {
        let pool = PgPool::connect_lazy("postgres://localhost/auau").unwrap();
        let app = crate::api::router(test_support::state(pool));

        for uri in [
            "/api/v1/projects".to_string(),
            format!("/api/v1/projects/{}", Uuid::new_v4()),
        ] {
            let (status, body) = test_support::json(
                app.clone()
                    .oneshot(Request::get(uri).body(Body::empty()).unwrap())
                    .await
                    .unwrap(),
            )
            .await;
            assert_eq!(status, StatusCode::UNAUTHORIZED);
            assert_eq!(body["error"]["code"], json!("unauthorized"));
        }

        let (status, body) = test_support::json(
            app.oneshot(
                Request::post("/api/v1/projects")
                    .header("content-type", "application/json")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap(),
        )
        .await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
        assert_eq!(body["error"]["code"], json!("unauthorized"));
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn projects_crud_flow(pool: PgPool) {
        let app = crate::api::router(test_support::state(pool.clone()));
        let (user, cookie) = test_support::signed_in(&pool).await.unwrap();

        let (status, body) = test_support::json(
            app.clone()
                .oneshot(test_support::post_json(
                    "/api/v1/projects",
                    &cookie,
                    json!({"name": "   "}),
                ))
                .await
                .unwrap(),
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert_eq!(body["error"]["code"], json!("invalid_name"));

        let (status, first) = test_support::json(
            app.clone()
                .oneshot(test_support::post_json(
                    "/api/v1/projects",
                    &cookie,
                    json!({"name": "podcast", "description": "выпуск №1"}),
                ))
                .await
                .unwrap(),
        )
        .await;
        assert_eq!(status, StatusCode::CREATED);
        assert_eq!(first["name"], json!("podcast"));
        assert_eq!(first["description"], json!("выпуск №1"));
        assert_eq!(first["created_by"], json!(user.id.to_string()));
        let first_id: Uuid = first["id"].as_str().unwrap().parse().unwrap();

        let (status, second) = test_support::json(
            app.clone()
                .oneshot(test_support::post_json(
                    "/api/v1/projects",
                    &cookie,
                    json!({"name": "интервью"}),
                ))
                .await
                .unwrap(),
        )
        .await;
        assert_eq!(status, StatusCode::CREATED);
        assert_eq!(second["description"], Value::Null);

        let (status, list) = test_support::json(
            app.clone()
                .oneshot(test_support::get("/api/v1/projects", &cookie))
                .await
                .unwrap(),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        let items = list.as_array().unwrap();
        assert_eq!(items.len(), 2);
        let mut names: Vec<&str> = items
            .iter()
            .map(|item| item["project"]["name"].as_str().unwrap())
            .collect();
        names.sort_unstable();
        assert_eq!(names, vec!["podcast", "интервью"]);
        for item in items {
            assert_eq!(item["counts"], zero_counts());
        }

        insert_label(&pool, first_id, "смех", 1, false).await;
        insert_label(&pool, first_id, "тишина", 0, false).await;
        insert_label(&pool, first_id, "старая", 2, true).await;

        let (status, detail) = test_support::json(
            app.clone()
                .oneshot(test_support::get(
                    &format!("/api/v1/projects/{first_id}"),
                    &cookie,
                ))
                .await
                .unwrap(),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(detail["project"]["id"], json!(first_id.to_string()));
        assert_eq!(detail["counts"], zero_counts());
        let label_names: Vec<&str> = detail["labels"]
            .as_array()
            .unwrap()
            .iter()
            .map(|label| label["name"].as_str().unwrap())
            .collect();
        assert_eq!(label_names, vec!["тишина", "смех"]);

        let (status, body) = test_support::json(
            app.oneshot(test_support::get(
                &format!("/api/v1/projects/{}", Uuid::new_v4()),
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
    async fn create_duplicate_name_conflicts(pool: PgPool) {
        let app = crate::api::router(test_support::state(pool.clone()));
        let (_user, cookie) = test_support::signed_in(&pool).await.unwrap();

        let (status, _) = test_support::json(
            app.clone()
                .oneshot(test_support::post_json(
                    "/api/v1/projects",
                    &cookie,
                    json!({"name": "alpha"}),
                ))
                .await
                .unwrap(),
        )
        .await;
        assert_eq!(status, StatusCode::CREATED);

        let (status, body) = test_support::json(
            app.clone()
                .oneshot(test_support::post_json(
                    "/api/v1/projects",
                    &cookie,
                    json!({"name": "alpha"}),
                ))
                .await
                .unwrap(),
        )
        .await;
        assert_eq!(status, StatusCode::CONFLICT);
        assert_eq!(body["error"]["code"], json!("duplicate"));

        let count: i64 = sqlx::query_scalar("SELECT count(*) FROM projects WHERE name = 'alpha'")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(count, 1, "duplicate must not create a second row");
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn detail_counts_reflect_statuses(pool: PgPool) {
        let app = crate::api::router(test_support::state(pool.clone()));
        let (user, cookie) = test_support::signed_in(&pool).await.unwrap();

        let (status, project) = test_support::json(
            app.clone()
                .oneshot(test_support::post_json(
                    "/api/v1/projects",
                    &cookie,
                    json!({"name": "разметка"}),
                ))
                .await
                .unwrap(),
        )
        .await;
        assert_eq!(status, StatusCode::CREATED);
        let project_id: Uuid = project["id"].as_str().unwrap().parse().unwrap();

        for (i, status) in [
            "new",
            "new",
            "in_progress",
            "done",
            "done",
            "done",
            "error",
            "processing",
        ]
        .into_iter()
        .enumerate()
        {
            insert_file(&pool, project_id, user.id, status, i).await;
        }

        let (status, detail) = test_support::json(
            app.clone()
                .oneshot(test_support::get(
                    &format!("/api/v1/projects/{project_id}"),
                    &cookie,
                ))
                .await
                .unwrap(),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(
            detail["counts"],
            json!({"new": 2, "in_progress": 1, "done": 3, "error": 1})
        );

        let (status, list) = test_support::json(
            app.oneshot(test_support::get("/api/v1/projects", &cookie))
                .await
                .unwrap(),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        let summary = list
            .as_array()
            .unwrap()
            .iter()
            .find(|item| item["project"]["id"] == json!(project_id.to_string()))
            .unwrap();
        assert_eq!(
            summary["counts"],
            json!({"new": 2, "in_progress": 1, "done": 3, "error": 1})
        );
    }

    async fn insert_label(
        pool: &PgPool,
        project_id: Uuid,
        name: &str,
        sort_order: i32,
        archived: bool,
    ) {
        sqlx::query(
            "INSERT INTO labels (id, project_id, name, color, sort_order, archived)
             VALUES ($1, $2, $3, '#f59e0b', $4, $5)",
        )
        .bind(Uuid::new_v4())
        .bind(project_id)
        .bind(name)
        .bind(sort_order)
        .bind(archived)
        .execute(pool)
        .await
        .unwrap();
    }

    async fn insert_file(
        pool: &PgPool,
        project_id: Uuid,
        uploaded_by: Uuid,
        status: &str,
        salt: usize,
    ) {
        sqlx::query(
            "INSERT INTO audio_files (id, project_id, original_name, storage_key, sha256,
                mime, size_bytes, status, uploaded_by)
             VALUES ($1, $2, $3, $4, $5, 'audio/wav', 10, $6, $7)",
        )
        .bind(Uuid::new_v4())
        .bind(project_id)
        .bind(format!("файл_{salt}.wav"))
        .bind(format!("media/{salt}.wav"))
        .bind(format!("{salt:064x}"))
        .bind(status)
        .bind(uploaded_by)
        .execute(pool)
        .await
        .unwrap();
    }
}
