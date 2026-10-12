//! Dataset export: one project's finished annotations as JSON for ML
//! pipelines or a flat CSV for quick checks.
//!
//! Only `done` files make the dataset — what was actually finished. The
//! `labels` dictionary carries every label the exported segments
//! reference, archived ones included: segments store label ids, but the
//! export speaks label names, so an archived label must still resolve.
//! Everything is assembled in memory on the fly (10K files is fine; no
//! snapshot tables).

use std::borrow::Cow;
use std::collections::{HashMap, HashSet};

use axum::extract::{Path, Query, State};
use axum::http::header;
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Json, Router};
use serde::{Deserialize, Serialize};
use sqlx::FromRow;
use uuid::Uuid;

use crate::api::AppState;
use crate::auth::CurrentUser;
use crate::error::AppError;
use crate::models::{FileStatus, Label, Project};

/// Export routes; both formats require a signed-in user.
pub fn router() -> Router<AppState> {
    Router::new().route("/api/v1/projects/{id}/export", get(export))
}

/// Query parameters of the export endpoint; a missing `format` defaults
/// to JSON, the primary ML format.
#[derive(Debug, Deserialize)]
struct ExportQuery {
    format: Option<String>,
}

/// One row of the files+annotations LEFT JOIN: a done file with one of
/// its segments — or, the join having found none, a lone file row whose
/// annotation columns are NULL.
#[derive(FromRow)]
struct ExportRow {
    file_id: Uuid,
    original_name: String,
    duration_ms: Option<i32>,
    annotation_label_id: Option<Uuid>,
    annotation_start_ms: Option<i32>,
    annotation_end_ms: Option<i32>,
}

/// A finished file with its segments, as both renderers want it.
struct FileExport {
    id: Uuid,
    name: String,
    duration_ms: Option<i32>,
    annotations: Vec<SegmentExport>,
}

/// One exported segment; the label name is resolved at render time.
struct SegmentExport {
    label_id: Uuid,
    start_ms: i32,
    end_ms: i32,
}

/// Map a sqlx failure onto the internal-error variant.
fn internal(err: sqlx::Error) -> AppError {
    AppError::Internal(err.into())
}

/// `GET /api/v1/projects/:id/export?format=json|csv`.
async fn export(
    State(state): State<AppState>,
    _user: CurrentUser,
    Path(project_id): Path<Uuid>,
    Query(query): Query<ExportQuery>,
) -> Result<Response, AppError> {
    let csv = match query.format.as_deref() {
        None | Some("json") => false,
        Some("csv") => true,
        Some(other) => {
            return Err(AppError::BadRequest {
                code: "invalid_format".into(),
                message: format!("unknown export format '{other}'; supported: json, csv"),
            });
        }
    };

    let project: Project = sqlx::query_as("SELECT * FROM projects WHERE id = $1")
        .bind(project_id)
        .fetch_optional(&state.pool)
        .await
        .map_err(internal)?
        .ok_or_else(|| AppError::NotFound(format!("project {project_id} not found")))?;

    let labels: Vec<Label> =
        sqlx::query_as("SELECT * FROM labels WHERE project_id = $1 ORDER BY sort_order, name")
            .bind(project_id)
            .fetch_all(&state.pool)
            .await
            .map_err(internal)?;

    let rows: Vec<ExportRow> = sqlx::query_as(
        "SELECT af.id AS file_id, af.original_name, af.duration_ms,
                a.label_id AS annotation_label_id,
                a.start_ms AS annotation_start_ms,
                a.end_ms   AS annotation_end_ms
         FROM audio_files af
         LEFT JOIN annotations a ON a.file_id = af.id
         WHERE af.project_id = $1 AND af.status = $2
         ORDER BY af.created_at, af.id, a.start_ms, a.end_ms, a.id",
    )
    .bind(project_id)
    .bind(FileStatus::Done)
    .fetch_all(&state.pool)
    .await
    .map_err(internal)?;

    let mut referenced: HashSet<Uuid> = HashSet::new();
    let mut files: Vec<FileExport> = Vec::new();
    for row in rows {
        if files.last().is_none_or(|file| file.id != row.file_id) {
            files.push(FileExport {
                id: row.file_id,
                name: row.original_name,
                duration_ms: row.duration_ms,
                annotations: Vec::new(),
            });
        }
        if let (Some(label_id), Some(start_ms), Some(end_ms)) = (
            row.annotation_label_id,
            row.annotation_start_ms,
            row.annotation_end_ms,
        ) {
            referenced.insert(label_id);
            files.last_mut().unwrap().annotations.push(SegmentExport {
                label_id,
                start_ms,
                end_ms,
            });
        }
    }

    let labels: Vec<&Label> = labels
        .iter()
        .filter(|label| referenced.contains(&label.id))
        .collect();
    let names: HashMap<Uuid, &str> = labels
        .iter()
        .map(|label| (label.id, label.name.as_str()))
        .collect();

    if csv {
        Ok(render_csv(&project, &files, &names))
    } else {
        Ok(render_json(&project, &labels, &files, &names))
    }
}

/// JSON export: field names and order are pinned by the format,
/// `schema_version` is fixed so the schema can evolve without breaking
/// consumers.
#[derive(Serialize)]
struct ExportJson<'a> {
    schema_version: u32,
    project: ExportProjectJson<'a>,
    labels: Vec<ExportLabelJson<'a>>,
    files: Vec<ExportFileJson<'a>>,
}

#[derive(Serialize)]
struct ExportProjectJson<'a> {
    id: Uuid,
    name: &'a str,
}

#[derive(Serialize)]
struct ExportLabelJson<'a> {
    id: Uuid,
    name: &'a str,
}

#[derive(Serialize)]
struct ExportFileJson<'a> {
    id: Uuid,
    name: &'a str,
    duration_ms: Option<i32>,
    annotations: Vec<ExportAnnotationJson<'a>>,
}

#[derive(Serialize)]
struct ExportAnnotationJson<'a> {
    label: &'a str,
    start_ms: i32,
    end_ms: i32,
}

/// Render the JSON dataset (`Content-Type: application/json`).
fn render_json<'a>(
    project: &Project,
    labels: &[&'a Label],
    files: &[FileExport],
    names: &HashMap<Uuid, &'a str>,
) -> Response {
    Json(ExportJson {
        schema_version: 1,
        project: ExportProjectJson {
            id: project.id,
            name: &project.name,
        },
        labels: labels
            .iter()
            .map(|label| ExportLabelJson {
                id: label.id,
                name: label.name.as_str(),
            })
            .collect(),
        files: files
            .iter()
            .map(|file| ExportFileJson {
                id: file.id,
                name: &file.name,
                duration_ms: file.duration_ms,
                annotations: file
                    .annotations
                    .iter()
                    .map(|segment| ExportAnnotationJson {
                        label: names[&segment.label_id],
                        start_ms: segment.start_ms,
                        end_ms: segment.end_ms,
                    })
                    .collect(),
            })
            .collect(),
    })
    .into_response()
}

/// Render the flat CSV: one row per segment, files by created_at and
/// segments by start_ms — the same order as the JSON — behind the fixed
/// column line and a UTF-8 BOM (so Excel opens it as UTF-8). A finished
/// file without segments simply contributes no rows.
fn render_csv(project: &Project, files: &[FileExport], names: &HashMap<Uuid, &str>) -> Response {
    let mut csv = String::from("\u{FEFF}file,label,start_ms,end_ms,duration_ms\n");
    for file in files {
        let duration_ms = file
            .duration_ms
            .map(|duration| duration.to_string())
            .unwrap_or_default();
        for segment in &file.annotations {
            csv.push_str(&csv_field(&file.name));
            csv.push(',');
            csv.push_str(&csv_field(names[&segment.label_id]));
            csv.push(',');
            csv.push_str(&segment.start_ms.to_string());
            csv.push(',');
            csv.push_str(&segment.end_ms.to_string());
            csv.push(',');
            csv.push_str(&duration_ms);
            csv.push('\n');
        }
    }

    let disposition = content_disposition(&project.name);
    (
        [
            (header::CONTENT_TYPE, "text/csv"),
            (header::CONTENT_DISPOSITION, disposition.as_str()),
        ],
        csv,
    )
        .into_response()
}

/// Quote a CSV field iff it contains a separator, a quote, or a line
/// break; embedded quotes double up (RFC 4180).
fn csv_field(value: &str) -> Cow<'_, str> {
    if value
        .bytes()
        .any(|byte| matches!(byte, b',' | b'"' | b'\n' | b'\r'))
    {
        Cow::Owned(format!("\"{}\"", value.replace('"', "\"\"")))
    } else {
        Cow::Borrowed(value)
    }
}

/// `attachment; filename="<project>-export.csv"` — but header values
/// must stay ASCII, and project names need not be: a name outside plain
/// ASCII (or carrying a quote/backslash) moves into an RFC 5987
/// `filename*` with a neutral ASCII fallback.
fn content_disposition(project_name: &str) -> String {
    let plain = project_name
        .bytes()
        .all(|byte| matches!(byte, b' '..=b'~') && byte != b'"' && byte != b'\\');
    if plain {
        format!("attachment; filename=\"{project_name}-export.csv\"")
    } else {
        format!(
            "attachment; filename=\"export.csv\"; filename*=UTF-8''{}-export.csv",
            percent_encode(project_name)
        )
    }
}

/// Percent-encode everything outside RFC 3986's unreserved set — the
/// ASCII-only encoding RFC 5987 wants in `filename*`.
fn percent_encode(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for byte in value.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => {
                out.push(byte as char);
            }
            _ => out.push_str(&format!("%{byte:02X}")),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::{header, HeaderMap, StatusCode};
    use chrono::{DateTime, Duration, Utc};
    use serde_json::{json, Value};
    use sqlx::PgPool;
    use tower::ServiceExt;

    use crate::api::test_support;
    use crate::models::FileStatus;

    /// Create a project through the API and return its id.
    async fn create_project(app: &axum::Router, cookie: &str, name: &str) -> Uuid {
        let (status, body) = test_support::json(
            app.clone()
                .oneshot(test_support::post_json(
                    "/api/v1/projects",
                    cookie,
                    json!({"name": name}),
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
    async fn insert_label(
        pool: &PgPool,
        project_id: Uuid,
        name: &str,
        sort_order: i32,
        archived: bool,
    ) -> Uuid {
        let id = Uuid::new_v4();
        sqlx::query(
            "INSERT INTO labels (id, project_id, name, color, sort_order, archived)
             VALUES ($1, $2, $3, '#e6194b', $4, $5)",
        )
        .bind(id)
        .bind(project_id)
        .bind(name)
        .bind(sort_order)
        .bind(archived)
        .execute(pool)
        .await
        .unwrap();
        id
    }

    /// Insert an audio_files row directly with an explicit status, name,
    /// duration and creation time (the export order is `created_at`).
    async fn insert_file(
        pool: &PgPool,
        project_id: Uuid,
        uploaded_by: Uuid,
        status: FileStatus,
        original_name: &str,
        duration_ms: Option<i32>,
        created_at: DateTime<Utc>,
    ) -> Uuid {
        let id = Uuid::new_v4();
        sqlx::query(
            "INSERT INTO audio_files (id, project_id, original_name, storage_key, sha256,
                mime, size_bytes, duration_ms, status, uploaded_by, created_at)
             VALUES ($1, $2, $3, $4, $5, 'audio/wav', 10, $6, $7, $8, $9)",
        )
        .bind(id)
        .bind(project_id)
        .bind(original_name)
        .bind(format!("media/{id}.wav"))
        .bind(Uuid::new_v4().simple().to_string())
        .bind(duration_ms)
        .bind(status)
        .bind(uploaded_by)
        .bind(created_at)
        .execute(pool)
        .await
        .unwrap();
        id
    }

    /// Insert an annotation fixture.
    async fn insert_annotation(
        pool: &PgPool,
        file_id: Uuid,
        label_id: Uuid,
        start_ms: i32,
        end_ms: i32,
        created_by: Uuid,
    ) {
        sqlx::query(
            "INSERT INTO annotations (id, file_id, label_id, start_ms, end_ms, created_by)
             VALUES ($1, $2, $3, $4, $5, $6)",
        )
        .bind(Uuid::new_v4())
        .bind(file_id)
        .bind(label_id)
        .bind(start_ms)
        .bind(end_ms)
        .bind(created_by)
        .execute(pool)
        .await
        .unwrap();
    }

    /// The export URI of a project in a given format.
    fn export_uri(project_id: Uuid, format: &str) -> String {
        format!("/api/v1/projects/{project_id}/export?format={format}")
    }

    /// Drain a response into status, headers and raw bytes — the CSV
    /// path is not JSON, so `test_support::json` does not apply.
    async fn raw(response: axum::response::Response) -> (StatusCode, HeaderMap, Vec<u8>) {
        let status = response.status();
        let headers = response.headers().clone();
        let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap()
            .to_vec();
        (status, headers, bytes)
    }

    #[test]
    fn csv_field_quotes_only_when_needed() {
        assert_eq!(csv_field("смех"), "смех");
        assert_eq!(csv_field("шум, фон"), "\"шум, фон\"");
        assert_eq!(csv_field("он сказал \"а\""), "\"он сказал \"\"а\"\"\"");
        assert_eq!(csv_field("две\nстроки"), "\"две\nстроки\"");
    }

    #[test]
    fn content_disposition_stays_ascii() {
        assert_eq!(
            content_disposition("podcast"),
            "attachment; filename=\"podcast-export.csv\""
        );
        assert_eq!(
            content_disposition("подкаст"),
            concat!(
                "attachment; filename=\"export.csv\"; ",
                "filename*=UTF-8''%D0%BF%D0%BE%D0%B4%D0%BA%D0%B0%D1%81%D1%82-export.csv",
            )
        );
        assert_eq!(
            content_disposition("он сказал \"а\""),
            concat!(
                "attachment; filename=\"export.csv\"; ",
                "filename*=UTF-8''%D0%BE%D0%BD%20%D1%81%D0%BA%D0%B0%D0%B7%D0%B0%D0%BB%20%22%D0%B0%22-export.csv",
            )
        );
    }

    #[tokio::test]
    async fn export_requires_a_session() {
        let pool = PgPool::connect_lazy("postgres://localhost/auau").unwrap();
        let app = crate::api::build_router(test_support::state(pool));
        let uri = export_uri(Uuid::new_v4(), "json");

        let (status, body) = test_support::json(
            app.oneshot(test_support::get(&uri, "auau_session=invalid"))
                .await
                .unwrap(),
        )
        .await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
        assert_eq!(body["error"]["code"], json!("unauthorized"));
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn export_json_includes_only_done_files(pool: PgPool) {
        let app = crate::api::build_router(test_support::state(pool.clone()));
        let (alice, cookie) = test_support::signed_in(&pool).await.unwrap();
        let project = create_project(&app, &cookie, "подкаст").await;
        let t0 = Utc::now();

        let laugh = insert_label(&pool, project, "смех", 1, false).await;
        let noise = insert_label(&pool, project, "шум", 2, false).await;
        insert_label(&pool, project, "прочее", 3, false).await;

        let first = insert_file(
            &pool,
            project,
            alice.id,
            FileStatus::Done,
            "файл_042.wav",
            Some(132_000),
            t0 + Duration::seconds(1),
        )
        .await;
        let second = insert_file(
            &pool,
            project,
            alice.id,
            FileStatus::Done,
            "интервью.mp3",
            Some(90_000),
            t0 + Duration::seconds(2),
        )
        .await;
        let swept = insert_file(
            &pool,
            project,
            alice.id,
            FileStatus::New,
            "вернувшийся.wav",
            Some(10_000),
            t0 + Duration::seconds(3),
        )
        .await;
        insert_file(
            &pool,
            project,
            alice.id,
            FileStatus::Error,
            "битый.wav",
            None,
            t0 + Duration::seconds(4),
        )
        .await;

        insert_annotation(&pool, first, noise, 5_000, 6_000, alice.id).await;
        insert_annotation(&pool, first, laugh, 3_100, 5_400, alice.id).await;
        insert_annotation(&pool, second, laugh, 0, 1_000, alice.id).await;
        insert_annotation(&pool, swept, laugh, 0, 500, alice.id).await;

        let (status, headers, bytes) = raw(app
            .clone()
            .oneshot(test_support::get(&export_uri(project, "json"), &cookie))
            .await
            .unwrap())
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(
            headers
                .get(header::CONTENT_TYPE)
                .and_then(|value| value.to_str().ok()),
            Some("application/json")
        );
        let body: Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(
            body,
            json!({
                "schema_version": 1,
                "project": {"id": project.to_string(), "name": "подкаст"},
                "labels": [
                    {"id": laugh.to_string(), "name": "смех"},
                    {"id": noise.to_string(), "name": "шум"},
                ],
                "files": [
                    {"id": first.to_string(), "name": "файл_042.wav",
                     "duration_ms": 132_000,
                     "annotations": [
                         {"label": "смех", "start_ms": 3_100, "end_ms": 5_400},
                         {"label": "шум", "start_ms": 5_000, "end_ms": 6_000},
                     ]},
                    {"id": second.to_string(), "name": "интервью.mp3",
                     "duration_ms": 90_000,
                     "annotations": [
                         {"label": "смех", "start_ms": 0, "end_ms": 1_000},
                     ]},
                ]
            })
        );

        let (status, _) = test_support::json(
            app.oneshot(test_support::get(
                &format!("/api/v1/projects/{project}/export"),
                &cookie,
            ))
            .await
            .unwrap(),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn export_archived_label_still_resolves(pool: PgPool) {
        let app = crate::api::build_router(test_support::state(pool.clone()));
        let (alice, cookie) = test_support::signed_in(&pool).await.unwrap();
        let project = create_project(&app, &cookie, "архивный набор").await;
        let t0 = Utc::now();

        let old = insert_label(&pool, project, "старая", 1, true).await;
        let live = insert_label(&pool, project, "смех", 2, false).await;
        let file = insert_file(
            &pool,
            project,
            alice.id,
            FileStatus::Done,
            "запись.wav",
            Some(60_000),
            t0 + Duration::seconds(1),
        )
        .await;
        insert_annotation(&pool, file, old, 0, 1_000, alice.id).await;
        insert_annotation(&pool, file, live, 2_000, 3_000, alice.id).await;

        let (status, body) = test_support::json(
            app.oneshot(test_support::get(&export_uri(project, "json"), &cookie))
                .await
                .unwrap(),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{body}");
        assert_eq!(
            body["labels"],
            json!([
                {"id": old.to_string(), "name": "старая"},
                {"id": live.to_string(), "name": "смех"},
            ])
        );
        assert_eq!(
            body["files"][0]["annotations"],
            json!([
                {"label": "старая", "start_ms": 0, "end_ms": 1_000},
                {"label": "смех", "start_ms": 2_000, "end_ms": 3_000},
            ])
        );
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn export_csv_columns_and_rows(pool: PgPool) {
        let app = crate::api::build_router(test_support::state(pool.clone()));
        let (alice, cookie) = test_support::signed_in(&pool).await.unwrap();
        let project = create_project(&app, &cookie, "podcast").await;
        let t0 = Utc::now();

        let laugh = insert_label(&pool, project, "смех", 1, false).await;
        let noisy = insert_label(&pool, project, "шум, \"тихий\"", 2, false).await;

        let first = insert_file(
            &pool,
            project,
            alice.id,
            FileStatus::Done,
            "файл_042.wav",
            Some(132_000),
            t0 + Duration::seconds(1),
        )
        .await;
        let second = insert_file(
            &pool,
            project,
            alice.id,
            FileStatus::Done,
            "интервью.mp3",
            Some(90_000),
            t0 + Duration::seconds(2),
        )
        .await;
        insert_file(
            &pool,
            project,
            alice.id,
            FileStatus::Done,
            "пустой.wav",
            Some(5_000),
            t0 + Duration::seconds(3),
        )
        .await;
        insert_file(
            &pool,
            project,
            alice.id,
            FileStatus::New,
            "новый.wav",
            Some(10_000),
            t0 + Duration::seconds(4),
        )
        .await;

        insert_annotation(&pool, first, noisy, 5_000, 6_000, alice.id).await;
        insert_annotation(&pool, first, laugh, 3_100, 5_400, alice.id).await;
        insert_annotation(&pool, second, laugh, 0, 1_000, alice.id).await;

        let (status, headers, body) = raw(app
            .oneshot(test_support::get(&export_uri(project, "csv"), &cookie))
            .await
            .unwrap())
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(
            headers
                .get(header::CONTENT_TYPE)
                .and_then(|value| value.to_str().ok()),
            Some("text/csv")
        );
        assert_eq!(
            headers
                .get(header::CONTENT_DISPOSITION)
                .and_then(|value| value.to_str().ok()),
            Some("attachment; filename=\"podcast-export.csv\"")
        );
        assert_eq!(&body[..3], &[0xEF, 0xBB, 0xBF][..]);
        assert_eq!(
            String::from_utf8_lossy(&body),
            concat!(
                "\u{FEFF}file,label,start_ms,end_ms,duration_ms\n",
                "файл_042.wav,смех,3100,5400,132000\n",
                "файл_042.wav,\"шум, \"\"тихий\"\"\",5000,6000,132000\n",
                "интервью.mp3,смех,0,1000,90000\n",
            )
        );
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn export_invalid_format_400(pool: PgPool) {
        let app = crate::api::build_router(test_support::state(pool.clone()));
        let (_alice, cookie) = test_support::signed_in(&pool).await.unwrap();
        let project = create_project(&app, &cookie, "форматы").await;

        for format in ["xml", ""] {
            let (status, body) = test_support::json(
                app.clone()
                    .oneshot(test_support::get(&export_uri(project, format), &cookie))
                    .await
                    .unwrap(),
            )
            .await;
            assert_eq!(status, StatusCode::BAD_REQUEST, "format '{format}'");
            assert_eq!(body["error"]["code"], json!("invalid_format"));
        }
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn export_missing_project_404(pool: PgPool) {
        let app = crate::api::build_router(test_support::state(pool.clone()));
        let (_alice, cookie) = test_support::signed_in(&pool).await.unwrap();

        let (status, body) = test_support::json(
            app.oneshot(test_support::get(
                &export_uri(Uuid::new_v4(), "csv"),
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
    async fn export_empty_project(pool: PgPool) {
        let app = crate::api::build_router(test_support::state(pool.clone()));
        let (alice, cookie) = test_support::signed_in(&pool).await.unwrap();
        let project = create_project(&app, &cookie, "пусто").await;
        insert_label(&pool, project, "смех", 1, false).await;
        insert_file(
            &pool,
            project,
            alice.id,
            FileStatus::New,
            "новый.wav",
            Some(10_000),
            Utc::now(),
        )
        .await;

        let (status, body) = test_support::json(
            app.oneshot(test_support::get(&export_uri(project, "json"), &cookie))
                .await
                .unwrap(),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{body}");
        assert_eq!(body["schema_version"], json!(1));
        assert_eq!(body["project"]["id"], json!(project.to_string()));
        assert_eq!(body["labels"], json!([]));
        assert_eq!(body["files"], json!([]));
    }
}
