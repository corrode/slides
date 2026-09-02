use std::path::Path as FilePath;

use axum::{
    extract::{Path, State},
    http::{HeaderMap, HeaderValue, header},
    response::{Html, IntoResponse, Redirect, Response},
};

use crate::{
    archive,
    error::{AppError, AppResult},
    store::{self, SessionArtifact},
    web::AppState,
};

pub async fn redirect_to_archive(
    State(state): State<AppState>,
    Path(token): Path<String>,
) -> AppResult<Response> {
    required_artifact(&state, &token).await?;
    Ok(Redirect::permanent(&format!("/shared/{token}/")).into_response())
}

pub async fn archive_page(
    State(state): State<AppState>,
    Path(token): Path<String>,
) -> AppResult<Response> {
    let artifact = required_artifact(&state, &token).await?;
    let page = archive::read_entry(&artifact.archive, "index.html")?
        .ok_or_else(|| AppError::not_found("The archived presentation is incomplete."))?;
    let page = String::from_utf8(page)
        .map_err(|_| AppError::not_found("The archived presentation is invalid."))?;
    Ok((immutable_headers(), Html(page)).into_response())
}

pub async fn archive_file(
    State(state): State<AppState>,
    Path((token, path)): Path<(String, String)>,
) -> AppResult<Response> {
    if path != "audience-input.json" && !path.starts_with("assets/") {
        return Err(AppError::not_found("Archived file not found."));
    }
    let artifact = required_artifact(&state, &token).await?;
    let contents = archive::read_entry(&artifact.archive, &path)?
        .ok_or_else(|| AppError::not_found("Archived file not found."))?;
    let mut headers = immutable_headers();
    headers.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static(content_type(&path)),
    );
    Ok((headers, contents).into_response())
}

pub async fn download(
    State(state): State<AppState>,
    Path(token): Path<String>,
) -> AppResult<Response> {
    let artifact = required_artifact(&state, &token).await?;
    let filename = archive_filename(&artifact);
    let mut headers = immutable_headers();
    headers.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("application/zip"),
    );
    headers.insert(
        header::CONTENT_DISPOSITION,
        HeaderValue::from_str(&format!("attachment; filename=\"{filename}\""))
            .expect("archive filename contains only safe ASCII characters"),
    );
    Ok((headers, artifact.archive).into_response())
}

async fn required_artifact(state: &AppState, token: &str) -> AppResult<SessionArtifact> {
    if token.len() != 64 || !token.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(AppError::not_found("Session archive not found."));
    }
    store::artifact_by_token(&state.pool, token)
        .await?
        .ok_or_else(|| AppError::not_found("Session archive not found."))
}

fn immutable_headers() -> HeaderMap {
    let mut headers = HeaderMap::new();
    headers.insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static("public, max-age=31536000, immutable"),
    );
    headers
}

fn content_type(path: &str) -> &'static str {
    match FilePath::new(path)
        .extension()
        .and_then(|value| value.to_str())
    {
        Some("css") => "text/css; charset=utf-8",
        Some("json") => "application/json; charset=utf-8",
        Some("jpg" | "jpeg") => "image/jpeg",
        Some("png") => "image/png",
        Some("gif") => "image/gif",
        Some("webp") => "image/webp",
        Some("svg") => "image/svg+xml",
        _ => "application/octet-stream",
    }
}

fn archive_filename(artifact: &SessionArtifact) -> String {
    let mut slug = String::new();
    let mut previous_dash = false;
    for character in artifact.title.chars().flat_map(char::to_lowercase) {
        if character.is_ascii_alphanumeric() {
            slug.push(character);
            previous_dash = false;
        } else if !slug.is_empty() && !previous_dash {
            slug.push('-');
            previous_dash = true;
        }
        if slug.len() >= 60 {
            break;
        }
    }
    let slug = slug.trim_end_matches('-');
    let slug = if slug.is_empty() {
        "presentation"
    } else {
        slug
    };
    format!("{slug}-{}.zip", artifact.code)
}

#[cfg(test)]
mod tests {
    use super::{archive_filename, content_type};
    use crate::store::SessionArtifact;

    #[test]
    fn creates_safe_archive_filenames() {
        let artifact = SessionArtifact {
            share_token: "a".repeat(64),
            archive: Vec::new(),
            code: "123456".into(),
            title: "Rust: Errors & APIs!".into(),
        };

        assert_eq!(archive_filename(&artifact), "rust-errors-apis-123456.zip");
        assert_eq!(content_type("assets/cat.webp"), "image/webp");
    }
}
