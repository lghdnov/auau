//! The frontend bundle, embedded into the binary.
//!
//! `web/dist` is produced by the frontend build; the committed `index.html`
//! placeholder keeps the folder non-empty so release builds — which embed
//! the files — always compile. In debug builds rust-embed reads from disk
//! instead, so frontend changes show up without recompiling the server.

use axum::http::{header, Uri};
use axum::response::{IntoResponse, Response};
use rust_embed::{EmbeddedFile, RustEmbed};

use crate::error::AppError;

/// The built SPA, served for every path outside `/api/v1` and `/media`.
#[derive(RustEmbed)]
#[folder = "$CARGO_MANIFEST_DIR/../web/dist"]
struct Assets;

/// SPA fallback for paths no API or media route matched.
///
/// Serves an exact asset hit (bundled JS/CSS) when one exists, and the
/// `index.html` shell otherwise, so client-side routes survive hard
/// reloads. Paths under `/api` and `/media` keep the JSON 404 envelope
/// instead — API clients must not receive HTML.
pub async fn spa_fallback(uri: Uri) -> Response {
    let path = uri.path();
    if path == "/api"
        || path.starts_with("/api/")
        || path == "/media"
        || path.starts_with("/media/")
    {
        return AppError::NotFound("route not found".into()).into_response();
    }

    let rel = path.trim_start_matches('/');
    let rel = if rel.is_empty() { "index.html" } else { rel };
    match Assets::get(rel).or_else(|| Assets::get("index.html")) {
        Some(file) => embedded(file).into_response(),
        None => AppError::Internal(anyhow::anyhow!("embedded SPA is missing index.html"))
            .into_response(),
    }
}

/// Render one embedded file with its guessed MIME type.
fn embedded(file: EmbeddedFile) -> Response {
    (
        [(header::CONTENT_TYPE, file.metadata.mimetype())],
        file.data.into_owned(),
    )
        .into_response()
}
