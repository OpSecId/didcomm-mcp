//! The web UI's static files, built from `ui/` (`npm run build` → `ui/dist`) and
//! embedded in release builds. Unknown paths get `index.html`, for the SPA's routes.

use axum::{
    http::{header, StatusCode, Uri},
    response::{IntoResponse, Response},
};

#[derive(rust_embed::RustEmbed)]
#[folder = "ui/dist"]
struct Assets;

pub async fn serve(uri: Uri) -> Response {
    let path = uri.path().trim_start_matches('/');
    if let Some(file) = Assets::get(path).filter(|_| !path.is_empty()) {
        // Hashed asset names (assets/*) never change; everything else is revalidated.
        let cache = if path.starts_with("assets/") { "public, max-age=31536000, immutable" } else { "no-cache" };
        return (
            [(header::CONTENT_TYPE, file.metadata.mimetype().to_string()), (header::CACHE_CONTROL, cache.to_string())],
            file.data.into_owned(),
        )
            .into_response();
    }
    if path.starts_with("api/") || path.contains('.') {
        return StatusCode::NOT_FOUND.into_response();
    }
    match Assets::get("index.html") {
        Some(index) => (
            [
                (header::CONTENT_TYPE, "text/html; charset=utf-8".to_string()),
                (header::CACHE_CONTROL, "no-cache".to_string()),
                // The UI loads nothing from elsewhere but profile pictures.
                (
                    header::CONTENT_SECURITY_POLICY,
                    "default-src 'self'; img-src 'self' https: http: data:; style-src 'self' 'unsafe-inline'; frame-ancestors 'none'".to_string(),
                ),
            ],
            index.data.into_owned(),
        )
            .into_response(),
        None => (StatusCode::NOT_FOUND, "the web UI isn't built (cd ui && npm run build)").into_response(),
    }
}
