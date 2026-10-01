use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::routing::{patch, post};
use axum::{Json, Router};
use serde::Deserialize;
use uuid::Uuid;

use crate::api::AppState;
use crate::auth::CurrentUser;
use crate::error::AppError;
use crate::models::Label;

/// The fixed palette new labels cycle through, indexed by `sort_order % 8`.
const PALETTE: [&str; 8] = [
    "#e6194b", "#3cb44b", "#ffe119", "#4363d8", "#f58231", "#911eb4", "#46f0f7", "#f032e6",
];

/// Label routes; all of them require a signed-in user.
pub fn router() -> Router<AppState> {
    Router::new()
        .route("/api/v1/projects/{id}/labels", post(create))
        .route("/api/v1/labels/{id}", patch(update).delete(archive))
}

/// Body of `POST /api/v1/projects/:id/labels`.
#[derive(Debug, Deserialize)]
struct CreateLabel {
    name: String,
    hotkey: Option<String>,
}

/// Body of `PATCH /api/v1/labels/:id`; absent fields keep their values.
#[derive(Debug, Deserialize)]
struct UpdateLabel {
    name: Option<String>,
    color: Option<String>,
    hotkey: Option<String>,
    sort_order: Option<i32>,
}

/// Map a raw sqlx failure onto the internal-error variant.
fn internal(err: sqlx::Error) -> AppError {
    AppError::Internal(err.into())
}

/// A hotkey must be exactly one character; anything else is a 400.
fn validate_hotkey(hotkey: Option<&str>) -> Result<Option<&str>, AppError> {
    match hotkey {
        None => Ok(None),
        Some(raw) => {
            let trimmed = raw.trim();
            if trimmed.chars().count() == 1 {
                Ok(Some(trimmed))
            } else {
                Err(AppError::BadRequest {
                    code: "invalid_hotkey".into(),
                    message: "hotkey must be exactly one character".into(),
                })
            }
        }
    }
}

/// A trimmed non-empty name, or a 400.
fn validate_name(raw: &str) -> Result<&str, AppError> {
    let name = raw.trim();
    if name.is_empty() {
        return Err(AppError::BadRequest {
            code: "invalid_name".into(),
            message: "label name must not be empty or whitespace".into(),
        });
    }
    Ok(name)
}

/// A `duplicate` conflict naming the constraint that fired, if known.
fn duplicate(db: &dyn sqlx::error::DatabaseError) -> AppError {
    let message = match db.constraint() {
        Some("labels_project_hotkey_key") => {
            "this hotkey is already used by a label in the project".into()
        }
        _ => "a label with this name already exists in the project".into(),
    };
    AppError::Conflict {
        code: "duplicate".into(),
        message,
    }
}

/// `POST /api/v1/projects/:id/labels` — append a label to the project palette.
///
/// `sort_order` is max+1 within the project and the color is picked from the
/// fixed palette at that index, so clients never send either.
async fn create(
    State(state): State<AppState>,
    _user: CurrentUser,
    Path(project_id): Path<Uuid>,
    Json(payload): Json<CreateLabel>,
) -> Result<(StatusCode, Json<Label>), AppError> {
    let name = validate_name(&payload.name)?;
    let hotkey = validate_hotkey(payload.hotkey.as_deref())?;

    let next: i32 = sqlx::query_scalar(
        "SELECT COALESCE(MAX(sort_order), -1) + 1 FROM labels WHERE project_id = $1",
    )
    .bind(project_id)
    .fetch_one(&state.pool)
    .await
    .map_err(internal)?;
    let color = PALETTE[usize::try_from(next).unwrap_or(0) % PALETTE.len()];

    let inserted = sqlx::query_as::<_, Label>(
        "INSERT INTO labels (id, project_id, name, color, hotkey, sort_order)
         VALUES ($1, $2, $3, $4, $5, $6)
         RETURNING *",
    )
    .bind(Uuid::new_v4())
    .bind(project_id)
    .bind(name)
    .bind(color)
    .bind(hotkey)
    .bind(next)
    .fetch_one(&state.pool)
    .await;

    match inserted {
        Ok(label) => Ok((StatusCode::CREATED, Json(label))),
        Err(sqlx::Error::Database(db)) if db.is_unique_violation() => Err(duplicate(db.as_ref())),
        Err(sqlx::Error::Database(db)) if db.is_foreign_key_violation() => Err(AppError::NotFound(
            format!("project {project_id} not found"),
        )),
        Err(err) => Err(internal(err)),
    }
}

/// `PATCH /api/v1/labels/:id` — rename, recolor, re-hotkey or reorder.
/// Absent fields keep their current values.
async fn update(
    State(state): State<AppState>,
    _user: CurrentUser,
    Path(id): Path<Uuid>,
    Json(payload): Json<UpdateLabel>,
) -> Result<Json<Label>, AppError> {
    let name = match payload.name.as_deref() {
        Some(raw) => Some(validate_name(raw)?),
        None => None,
    };
    let hotkey = validate_hotkey(payload.hotkey.as_deref())?;

    let updated = sqlx::query_as::<_, Label>(
        "UPDATE labels SET
            name = COALESCE($2, name),
            color = COALESCE($3, color),
            hotkey = COALESCE($4, hotkey),
            sort_order = COALESCE($5, sort_order)
         WHERE id = $1
         RETURNING *",
    )
    .bind(id)
    .bind(name)
    .bind(&payload.color)
    .bind(hotkey)
    .bind(payload.sort_order)
    .fetch_optional(&state.pool)
    .await;

    match updated {
        Ok(Some(label)) => Ok(Json(label)),
        Ok(None) => Err(AppError::NotFound(format!("label {id} not found"))),
        Err(sqlx::Error::Database(db)) if db.is_unique_violation() => Err(duplicate(db.as_ref())),
        Err(err) => Err(internal(err)),
    }
}

/// `DELETE /api/v1/labels/:id` — archive. The row is kept: existing
/// annotations reference it, and the name stays reserved for the project.
async fn archive(
    State(state): State<AppState>,
    _user: CurrentUser,
    Path(id): Path<Uuid>,
) -> Result<Json<Label>, AppError> {
    let archived =
        sqlx::query_as::<_, Label>("UPDATE labels SET archived = true WHERE id = $1 RETURNING *")
            .bind(id)
            .fetch_optional(&state.pool)
            .await
            .map_err(internal)?
            .ok_or_else(|| AppError::NotFound(format!("label {id} not found")))?;

    Ok(Json(archived))
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::header;
    use axum::http::{Request, StatusCode};
    use serde_json::{json, Value};
    use sqlx::PgPool;
    use tower::ServiceExt;

    use crate::api::test_support;

    /// A PATCH request with a JSON body, authorized by the cookie.
    fn patch_json(uri: &str, cookie: &str, body: impl serde::Serialize) -> Request<Body> {
        Request::patch(uri)
            .header(header::COOKIE, cookie)
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(serde_json::to_string(&body).unwrap()))
            .unwrap()
    }

    /// A DELETE request, authorized by the cookie.
    fn delete(uri: &str, cookie: &str) -> Request<Body> {
        Request::delete(uri)
            .header(header::COOKIE, cookie)
            .body(Body::empty())
            .unwrap()
    }

    /// Create a project fixture and return its id.
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
            "project fixture failed: {body}"
        );
        body["id"].as_str().unwrap().parse().unwrap()
    }

    /// POST a label fixture; returns the raw (status, body) pair.
    async fn post_label(
        app: &axum::Router,
        cookie: &str,
        project_id: Uuid,
        body: Value,
    ) -> (StatusCode, Value) {
        test_support::json(
            app.clone()
                .oneshot(test_support::post_json(
                    &format!("/api/v1/projects/{project_id}/labels"),
                    cookie,
                    body,
                ))
                .await
                .unwrap(),
        )
        .await
    }

    #[tokio::test]
    async fn labels_require_a_session() {
        let pool = PgPool::connect_lazy("postgres://localhost/auau").unwrap();
        let app = crate::api::router(test_support::state(pool.clone()));

        for (method, uri, body) in [
            (
                "POST",
                format!("/api/v1/projects/{}/labels", Uuid::new_v4()),
                Body::from("{}"),
            ),
            (
                "PATCH",
                format!("/api/v1/labels/{}", Uuid::new_v4()),
                Body::from("{}"),
            ),
            (
                "DELETE",
                format!("/api/v1/labels/{}", Uuid::new_v4()),
                Body::empty(),
            ),
        ] {
            let request = Request::builder()
                .method(method)
                .uri(uri.as_str())
                .header("content-type", "application/json")
                .body(body)
                .unwrap();
            let (status, body) =
                test_support::json(app.clone().oneshot(request).await.unwrap()).await;
            assert_eq!(status, StatusCode::UNAUTHORIZED, "{method} {uri}");
            assert_eq!(body["error"]["code"], json!("unauthorized"));
        }
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn create_label_autocolors_and_orders(pool: PgPool) {
        let app = crate::api::router(test_support::state(pool.clone()));
        let (_user, cookie) = test_support::signed_in(&pool).await.unwrap();
        let project_id = create_project(&app, &cookie, "подкаст").await;

        let (status, first) = post_label(
            &app,
            &cookie,
            project_id,
            json!({"name": "смех", "hotkey": "1"}),
        )
        .await;
        assert_eq!(status, StatusCode::CREATED, "{first}");
        assert_eq!(first["name"], json!("смех"));
        assert_eq!(first["project_id"], json!(project_id.to_string()));
        assert_eq!(first["hotkey"], json!("1"));
        assert_eq!(first["sort_order"], json!(0));
        assert_eq!(first["color"], json!(PALETTE[0]));
        assert_eq!(first["archived"], json!(false));

        let (status, second) =
            post_label(&app, &cookie, project_id, json!({"name": "тишина"})).await;
        assert_eq!(status, StatusCode::CREATED, "{second}");
        assert_eq!(second["sort_order"], json!(1));
        assert_eq!(second["color"], json!(PALETTE[1]));
        assert_eq!(second["hotkey"], Value::Null);
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn hotkey_unique_per_project_409(pool: PgPool) {
        let app = crate::api::router(test_support::state(pool.clone()));
        let (_user, cookie) = test_support::signed_in(&pool).await.unwrap();
        let alpha = create_project(&app, &cookie, "alpha").await;
        let beta = create_project(&app, &cookie, "beta").await;

        let (status, _) =
            post_label(&app, &cookie, alpha, json!({"name": "смех", "hotkey": "1"})).await;
        assert_eq!(status, StatusCode::CREATED);

        let (status, body) = post_label(
            &app,
            &cookie,
            alpha,
            json!({"name": "тишина", "hotkey": "1"}),
        )
        .await;
        assert_eq!(status, StatusCode::CONFLICT, "{body}");
        assert_eq!(body["error"]["code"], json!("duplicate"));

        let (status, other) =
            post_label(&app, &cookie, beta, json!({"name": "шум", "hotkey": "1"})).await;
        assert_eq!(status, StatusCode::CREATED, "{other}");

        let count: i64 = sqlx::query_scalar("SELECT count(*) FROM labels WHERE project_id = $1")
            .bind(alpha)
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(count, 1, "conflicting create must not leave a row");
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn create_label_duplicate_name_409(pool: PgPool) {
        let app = crate::api::router(test_support::state(pool.clone()));
        let (_user, cookie) = test_support::signed_in(&pool).await.unwrap();
        let project_id = create_project(&app, &cookie, "разметка").await;

        let (status, _) = post_label(&app, &cookie, project_id, json!({"name": "смех"})).await;
        assert_eq!(status, StatusCode::CREATED);

        let (status, body) = post_label(&app, &cookie, project_id, json!({"name": "смех"})).await;
        assert_eq!(status, StatusCode::CONFLICT, "{body}");
        assert_eq!(body["error"]["code"], json!("duplicate"));
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn create_label_on_missing_project_404(pool: PgPool) {
        let app = crate::api::router(test_support::state(pool.clone()));
        let (_user, cookie) = test_support::signed_in(&pool).await.unwrap();

        let (status, body) =
            post_label(&app, &cookie, Uuid::new_v4(), json!({"name": "в никуда"})).await;
        assert_eq!(status, StatusCode::NOT_FOUND, "{body}");
        assert_eq!(body["error"]["code"], json!("not_found"));
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn archive_label_hides_from_detail(pool: PgPool) {
        let app = crate::api::router(test_support::state(pool.clone()));
        let (_user, cookie) = test_support::signed_in(&pool).await.unwrap();
        let project_id = create_project(&app, &cookie, "подкаст").await;

        let (status, label) = post_label(&app, &cookie, project_id, json!({"name": "смех"})).await;
        assert_eq!(status, StatusCode::CREATED);
        let label_id: Uuid = label["id"].as_str().unwrap().parse().unwrap();

        let (status, body) = test_support::json(
            app.clone()
                .oneshot(delete(&format!("/api/v1/labels/{label_id}"), &cookie))
                .await
                .unwrap(),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{body}");
        assert_eq!(body["archived"], json!(true));

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
        assert_eq!(detail["labels"].as_array().unwrap().len(), 0);

        let archived: Option<bool> =
            sqlx::query_scalar("SELECT archived FROM labels WHERE id = $1")
                .bind(label_id)
                .fetch_optional(&pool)
                .await
                .unwrap();
        assert_eq!(archived, Some(true), "archive must keep the row");

        let (status, _) = test_support::json(
            app.clone()
                .oneshot(delete(&format!("/api/v1/labels/{label_id}"), &cookie))
                .await
                .unwrap(),
        )
        .await;
        assert_eq!(status, StatusCode::OK);

        let (status, body) = test_support::json(
            app.oneshot(delete(
                &format!("/api/v1/labels/{}", Uuid::new_v4()),
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
    async fn patch_label_renames_and_recolors(pool: PgPool) {
        let app = crate::api::router(test_support::state(pool.clone()));
        let (_user, cookie) = test_support::signed_in(&pool).await.unwrap();
        let project_id = create_project(&app, &cookie, "подкаст").await;

        let (status, label) = post_label(&app, &cookie, project_id, json!({"name": "смех"})).await;
        assert_eq!(status, StatusCode::CREATED);
        let label_id: Uuid = label["id"].as_str().unwrap().parse().unwrap();

        let (status, patched) = test_support::json(
            app.clone()
                .oneshot(patch_json(
                    &format!("/api/v1/labels/{label_id}"),
                    &cookie,
                    json!({"name": "хохот", "color": "#123456", "hotkey": "7", "sort_order": 5}),
                ))
                .await
                .unwrap(),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{patched}");
        assert_eq!(patched["name"], json!("хохот"));
        assert_eq!(patched["color"], json!("#123456"));
        assert_eq!(patched["hotkey"], json!("7"));
        assert_eq!(patched["sort_order"], json!(5));

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
        assert_eq!(detail["labels"][0]["name"], json!("хохот"));

        let (status, patched) = test_support::json(
            app.clone()
                .oneshot(patch_json(
                    &format!("/api/v1/labels/{label_id}"),
                    &cookie,
                    json!({"name": "ржач"}),
                ))
                .await
                .unwrap(),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{patched}");
        assert_eq!(patched["name"], json!("ржач"));
        assert_eq!(patched["color"], json!("#123456"));
        assert_eq!(patched["hotkey"], json!("7"));

        let (status, body) = test_support::json(
            app.oneshot(patch_json(
                &format!("/api/v1/labels/{}", Uuid::new_v4()),
                &cookie,
                json!({"name": "нет"}),
            ))
            .await
            .unwrap(),
        )
        .await;
        assert_eq!(status, StatusCode::NOT_FOUND);
        assert_eq!(body["error"]["code"], json!("not_found"));
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn create_label_rejects_empty_name_400(pool: PgPool) {
        let app = crate::api::router(test_support::state(pool.clone()));
        let (_user, cookie) = test_support::signed_in(&pool).await.unwrap();
        let project_id = create_project(&app, &cookie, "подкаст").await;

        for name in ["", "   "] {
            let (status, body) = post_label(&app, &cookie, project_id, json!({"name": name})).await;
            assert_eq!(status, StatusCode::BAD_REQUEST, "name {name:?}: {body}");
            assert_eq!(body["error"]["code"], json!("invalid_name"));
        }
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn hotkey_multichar_rejected_400(pool: PgPool) {
        let app = crate::api::router(test_support::state(pool.clone()));
        let (_user, cookie) = test_support::signed_in(&pool).await.unwrap();
        let project_id = create_project(&app, &cookie, "подкаст").await;

        for hotkey in ["ab", "", "  "] {
            let (status, body) = post_label(
                &app,
                &cookie,
                project_id,
                json!({"name": format!("метка-{hotkey:?}"), "hotkey": hotkey}),
            )
            .await;
            assert_eq!(status, StatusCode::BAD_REQUEST, "hotkey {hotkey:?}: {body}");
            assert_eq!(body["error"]["code"], json!("invalid_hotkey"));
        }

        let (status, label) = post_label(&app, &cookie, project_id, json!({"name": "смех"})).await;
        assert_eq!(status, StatusCode::CREATED);
        let label_id: Uuid = label["id"].as_str().unwrap().parse().unwrap();

        let (status, body) = test_support::json(
            app.oneshot(patch_json(
                &format!("/api/v1/labels/{label_id}"),
                &cookie,
                json!({"hotkey": "ab"}),
            ))
            .await
            .unwrap(),
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
        assert_eq!(body["error"]["code"], json!("invalid_hotkey"));
    }
}
