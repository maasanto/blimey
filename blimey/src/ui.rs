//! The web UI: `ui/dist`, embedded at build time by build.rs.

use axum::http::{StatusCode, Uri, header};
use axum::response::{IntoResponse, Response};

// Defines `UI_FILES: &[(&str, &[u8])]`, path relative to ui/dist → content.
include!(concat!(env!("OUT_DIR"), "/ui_files.rs"));

pub async fn serve(uri: Uri) -> Response {
    let path = match uri.path().trim_start_matches('/') {
        "" => "index.html",
        path => path,
    };
    match UI_FILES.iter().find(|(name, _)| *name == path) {
        Some((_, content)) => {
            ([(header::CONTENT_TYPE, content_type(path))], *content).into_response()
        }
        None => StatusCode::NOT_FOUND.into_response(),
    }
}

fn content_type(path: &str) -> &'static str {
    match path.rsplit_once('.').map(|(_, extension)| extension) {
        Some("html") => "text/html; charset=utf-8",
        Some("js") => "text/javascript",
        Some("css") => "text/css",
        Some("svg") => "image/svg+xml",
        _ => "application/octet-stream",
    }
}
