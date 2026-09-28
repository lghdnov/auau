use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde::Serialize;
use serde_json::json;

/// A single field-level validation error, serialized in camelCase.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FieldError {
    pub field: String,
    pub message: String,
}

/// Application error rendered as `{"error":{"code":...,"message":...}}` JSON.
#[derive(Debug)]
pub enum AppError {
    Unauthorized,
    NotFound(String),
    BadRequest { code: String, message: String },
    Conflict { code: String, message: String },
    Unprocessable { errors: Vec<FieldError> },
    Internal(anyhow::Error),
}

impl From<anyhow::Error> for AppError {
    fn from(err: anyhow::Error) -> Self {
        AppError::Internal(err)
    }
}

impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        match self {
            AppError::Unauthorized => error_json(
                StatusCode::UNAUTHORIZED,
                "unauthorized",
                "authentication required",
            ),
            AppError::NotFound(message) => error_json(StatusCode::NOT_FOUND, "not_found", &message),
            AppError::BadRequest { code, message } => {
                error_json(StatusCode::BAD_REQUEST, &code, &message)
            }
            AppError::Conflict { code, message } => {
                error_json(StatusCode::CONFLICT, &code, &message)
            }
            AppError::Unprocessable { errors } => (
                StatusCode::UNPROCESSABLE_ENTITY,
                Json(json!({
                    "error": {
                        "code": "unprocessable",
                        "message": "validation failed",
                        "errors": errors,
                    }
                })),
            )
                .into_response(),
            AppError::Internal(err) => {
                tracing::error!(error = ?err, "internal server error");
                error_json(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "internal",
                    "internal server error",
                )
            }
        }
    }
}

/// Render the shared error envelope with the given status.
fn error_json(status: StatusCode, code: &str, message: &str) -> Response {
    (
        status,
        Json(json!({
            "error": {
                "code": code,
                "message": message,
            }
        })),
    )
        .into_response()
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::StatusCode;
    use serde_json::json;

    async fn error_body(error: AppError) -> (StatusCode, serde_json::Value) {
        let response = error.into_response();
        let status = response.status();
        let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let body = serde_json::from_slice(&bytes).unwrap();
        (status, body)
    }

    #[tokio::test]
    async fn conflict_renders_json_409() {
        let error = AppError::Conflict {
            code: "duplicate".into(),
            message: "file already exists".into(),
        };

        let (status, body) = error_body(error).await;

        assert_eq!(status, StatusCode::CONFLICT);
        assert_eq!(
            body,
            json!({"error": {"code": "duplicate", "message": "file already exists"}})
        );
    }

    #[tokio::test]
    async fn unprocessable_renders_422_with_camel_case_field_errors() {
        let error = AppError::Unprocessable {
            errors: vec![FieldError {
                field: "fileName".into(),
                message: "unsupported format".into(),
            }],
        };

        let (status, body) = error_body(error).await;

        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
        assert_eq!(body["error"]["code"], json!("unprocessable"));
        assert_eq!(
            body["error"]["errors"],
            json!([{"field": "fileName", "message": "unsupported format"}])
        );
    }

    #[tokio::test]
    async fn internal_renders_json_500_with_generic_code() {
        let error = AppError::Internal(anyhow::anyhow!("db exploded"));

        let (status, body) = error_body(error).await;

        assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR);
        assert_eq!(body["error"]["code"], json!("internal"));
    }
}
