mod archive;
mod error;
mod live;
mod markdown;
mod models;
mod store;
mod web;

use std::{env, sync::Arc};

use anyhow::Result;
use tracing_subscriber::{EnvFilter, layer::SubscriberExt, util::SubscriberInitExt};

use crate::{live::LiveHub, web::AppState};

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::registry()
        .with(
            EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| EnvFilter::new("slides=debug,tower_http=info")),
        )
        .with(tracing_subscriber::fmt::layer())
        .init();

    let database_url =
        env::var("SLIDES_DATABASE_URL").unwrap_or_else(|_| "sqlite://slides.db".into());
    let bind_address = env::var("SLIDES_BIND").unwrap_or_else(|_| "127.0.0.1:3000".into());
    let admin_password = env::var("SLIDES_ADMIN_PASSWORD")
        .map_err(|_| anyhow::anyhow!("SLIDES_ADMIN_PASSWORD is required to start the server"))?;
    if admin_password.is_empty() {
        anyhow::bail!("SLIDES_ADMIN_PASSWORD cannot be empty");
    }
    let secure_cookies = env::var("SLIDES_SECURE_COOKIES")
        .is_ok_and(|value| matches!(value.as_str(), "1" | "true" | "yes"));

    let pool = store::connect(&database_url).await?;
    let session_secret = web::random_token();
    let state = AppState {
        pool,
        hub: Arc::new(LiveHub::default()),
        admin_password_hash: web::hash(&admin_password),
        admin_cookie: web::hash(&session_secret),
        secure_cookies,
    };
    let app = web::router(state);
    let listener = tokio::net::TcpListener::bind(&bind_address).await?;
    tracing::info!(address = %bind_address, "Slides is ready");

    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal())
        .await?;
    Ok(())
}

async fn shutdown_signal() {
    if let Err(error) = tokio::signal::ctrl_c().await {
        tracing::error!(?error, "failed to listen for shutdown signal");
    }
}
