mod auth;
mod config;
mod crypto;
mod error;
mod queue;
mod routes;
mod state;

use std::time::Duration;

use axum::routing::{get, post};
use axum::Router;
use tower_http::trace::TraceLayer;
use tracing_subscriber::EnvFilter;

use crate::config::Config;
use crate::state::AppState;

#[tokio::main]
async fn main() {
    // Initialize structured logging
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .json()
        .init();

    let config = Config::from_env();

    tracing::info!(
        listen_addr = %config.listen_addr,
        redis_url = %config.redis_url,
        "starting herald-server"
    );

    // Connect to Redis
    let redis_client =
        redis::Client::open(config.redis_url.as_str()).expect("invalid Redis URL");
    let redis_conn = redis_client
        .get_multiplexed_async_connection()
        .await
        .expect("failed to connect to Redis");

    let state = AppState {
        redis: redis_conn,
        config: config.clone(),
    };

    // Start the reaper background task
    let reaper_state = state.clone();
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(Duration::from_secs(10));
        loop {
            interval.tick().await;
            let mut conn = reaper_state.redis.clone();
            if let Err(e) = queue::reap_expired(&mut conn, 3).await {
                tracing::error!(error = %e, "reaper error");
            }
        }
    });

    // Build the router
    let app = Router::new()
        // Inbound webhook ingestion (no auth — providers POST freely)
        .route(
            "/{customer_id}/{endpoint_name}",
            post(routes::ingest::ingest_webhook),
        )
        // Agent polling and management (auth required)
        .route("/queue/{endpoint_name}", get(routes::agent::poll_messages))
        .route(
            "/ack/{endpoint_name}/{message_id}",
            post(routes::agent::ack_message),
        )
        .route(
            "/ack/{endpoint_name}",
            post(routes::agent::batch_ack_messages),
        )
        .route(
            "/nack/{endpoint_name}/{message_id}",
            post(routes::agent::nack_message),
        )
        .route(
            "/heartbeat/{endpoint_name}/{message_id}",
            post(routes::agent::heartbeat),
        )
        // WebSocket streaming
        .route(
            "/stream/{endpoint_name}",
            get(routes::websocket::websocket_handler),
        )
        // Health check
        .route("/health", get(health_check))
        .layer(TraceLayer::new_for_http())
        .with_state(state);

    let listener = tokio::net::TcpListener::bind(&config.listen_addr)
        .await
        .expect("failed to bind");

    tracing::info!(addr = %config.listen_addr, "herald-server listening");

    axum::serve(listener, app).await.expect("server error");
}

async fn health_check() -> &'static str {
    "ok"
}
