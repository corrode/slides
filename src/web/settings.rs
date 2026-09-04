use askama::Template;
use axum::{
    extract::State,
    http::{HeaderValue, header},
    response::{IntoResponse, Redirect, Response},
};
use axum_extra::extract::cookie::CookieJar;

use crate::{
    error::AppResult,
    markdown::highlight_code,
    models::ApiTokenSummary,
    store,
    web::{AppState, hash, is_admin, random_token, require_admin, template},
};

const AUTHORIZATION_EXAMPLE: &str = "Authorization: Bearer <token>";
const CREATE_JSON_EXAMPLE: &str = r##"{
  "title": "Reliable Rust services",
  "slug": "reliable-rust-services",
  "source": "# Reliable Rust services\n\nOne idea per slide.\n\n---\n\n# Start with failure modes",
  "theme": {
    "headline_font": "bebas-neue",
    "text_font": "inter",
    "code_font": "jetbrains-mono",
    "background": "#1e1e2e",
    "text": "#cdd6f4",
    "accent": "#f9e2af"
  }
}"##;
const CREATE_CURL_EXAMPLE: &str = r#"curl --fail-with-body \
  --request POST \
  --url "$SLIDES_URL/api/v1/presentations" \
  --header "Authorization: Bearer $SLIDES_API_TOKEN" \
  --header "Content-Type: application/json" \
  --data @presentation.json"#;
const UPDATE_CURL_EXAMPLE: &str = r##"curl --fail-with-body \
  --request PATCH \
  --url "$SLIDES_URL/api/v1/presentations/reliable-rust-services" \
  --header "Authorization: Bearer $SLIDES_API_TOKEN" \
  --header "Content-Type: application/json" \
  --data '{"source":"# Revised deck\n\nUpdated content."}'"##;

#[derive(Template)]
#[template(path = "settings.html")]
struct SettingsTemplate {
    token: Option<ApiTokenSummary>,
    revealed_token: Option<String>,
    authorization_example: String,
    create_json_example: String,
    create_curl_example: String,
    update_curl_example: String,
}

pub async fn page(State(state): State<AppState>, jar: CookieJar) -> AppResult<Response> {
    if !is_admin(&jar, &state) {
        return Ok(Redirect::to("/admin/login").into_response());
    }
    render_settings(&state, None).await
}

pub async fn rotate_token(State(state): State<AppState>, jar: CookieJar) -> AppResult<Response> {
    require_admin(&jar, &state)?;
    let token = format!("slides_{}", random_token());
    let prefix = format!("{}…", token.chars().take(15).collect::<String>());
    store::replace_api_token(&state.pool, &hash(&token), &prefix).await?;
    render_settings(&state, Some(token)).await
}

pub async fn revoke_token(State(state): State<AppState>, jar: CookieJar) -> AppResult<Response> {
    require_admin(&jar, &state)?;
    store::revoke_api_token(&state.pool).await?;
    Ok(Redirect::to("/admin/settings").into_response())
}

async fn render_settings(state: &AppState, revealed_token: Option<String>) -> AppResult<Response> {
    let mut response = template(SettingsTemplate {
        token: store::api_token(&state.pool).await?,
        revealed_token,
        authorization_example: highlight_code("http", AUTHORIZATION_EXAMPLE),
        create_json_example: highlight_code("json", CREATE_JSON_EXAMPLE),
        create_curl_example: highlight_code("sh", CREATE_CURL_EXAMPLE),
        update_curl_example: highlight_code("sh", UPDATE_CURL_EXAMPLE),
    })?;
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    Ok(response)
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use axum::{body::to_bytes, http::StatusCode};
    use axum_extra::extract::cookie::{Cookie, CookieJar};

    use crate::live::LiveHub;

    use super::*;

    async fn test_state() -> (tempfile::TempDir, AppState) {
        let directory = tempfile::tempdir().unwrap();
        let database_url = format!("sqlite://{}", directory.path().join("slides.db").display());
        let pool = store::connect(&database_url).await.unwrap();
        let state = AppState {
            pool,
            hub: Arc::new(LiveHub::default()),
            admin_password_hash: hash("password"),
            admin_cookie: hash("cookie"),
            secure_cookies: false,
        };
        (directory, state)
    }

    fn admin_jar(state: &AppState) -> CookieJar {
        CookieJar::new().add(Cookie::new("slides_admin", state.admin_cookie.clone()))
    }

    async fn response_body(response: Response) -> String {
        let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        String::from_utf8(bytes.to_vec()).unwrap()
    }

    #[tokio::test]
    async fn settings_page_redirects_without_admin_cookie() {
        let (_directory, state) = test_state().await;
        let response = page(State(state), CookieJar::new()).await.unwrap();
        assert_eq!(response.status(), StatusCode::SEE_OTHER);
        assert_eq!(
            response.headers().get(header::LOCATION).unwrap(),
            "/admin/login"
        );
    }

    #[tokio::test]
    async fn token_is_revealed_once_stored_as_a_hash_and_revoked() {
        let (_directory, state) = test_state().await;
        let response = rotate_token(State(state.clone()), admin_jar(&state))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            response.headers().get(header::CACHE_CONTROL).unwrap(),
            "no-store"
        );

        let body = response_body(response).await;
        assert!(body.contains("#a6e3a1"));
        let token = body
            .split("id=\"api-token-value\" value=\"")
            .nth(1)
            .and_then(|value| value.split('"').next())
            .unwrap();
        assert!(token.starts_with("slides_"));
        assert!(
            store::api_token_matches(&state.pool, &hash(token))
                .await
                .unwrap()
        );

        let stored_hash: String =
            sqlx::query_scalar("SELECT token_hash FROM api_token WHERE id = 1")
                .fetch_one(&state.pool)
                .await
                .unwrap();
        assert_eq!(stored_hash, hash(token));
        assert_ne!(stored_hash, token);

        let response = page(State(state.clone()), admin_jar(&state)).await.unwrap();
        assert!(!response_body(response).await.contains(token));

        let response = revoke_token(State(state.clone()), admin_jar(&state))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::SEE_OTHER);
        assert!(store::api_token(&state.pool).await.unwrap().is_none());
        assert!(
            !store::api_token_matches(&state.pool, &hash(token))
                .await
                .unwrap()
        );
    }
}
