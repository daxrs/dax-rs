pub mod concurrency_limit;
pub mod config;
pub mod dashboard;
pub mod dax_provider;
pub mod models;
pub mod provider;
pub mod storage;
pub mod xmla;

pub use provider::ServerProvider;

use std::{sync::Arc, time::Duration};

use axum::{routing::get, Json, Router};
use serde::Serialize;
use tokio::net::TcpListener;
use tower_http::trace::{DefaultMakeSpan, DefaultOnRequest, DefaultOnResponse, TraceLayer};
use tracing::Level;

use crate::server::concurrency_limit::ConcurrencyLimitLayer;

#[derive(Serialize)]
struct HealthResponse {
    status: &'static str,
}

async fn health() -> Json<HealthResponse> {
    Json(HealthResponse { status: "ok" })
}

pub async fn run(provider: Arc<dyn ServerProvider>, server_config: config::ServerConfig) {
    let trace_layer = TraceLayer::new_for_http()
        .make_span_with(DefaultMakeSpan::new().level(Level::INFO))
        .on_request(DefaultOnRequest::new().level(Level::INFO))
        .on_response(DefaultOnResponse::new().level(Level::INFO));

    let concurrency = server_config.concurrency.clone();
    let bind_addr = server_config.bind_addr();
    let xmla_url = server_config.xmla_url();

    let mut app = Router::new()
        .route("/", axum::routing::get(dashboard::dashboard))
        .route("/health", get(health))
        .merge(xmla::routes(Arc::clone(&provider), Arc::new(server_config)))
        .merge(models::routes(provider))
        .layer(trace_layer);

    if concurrency.max_concurrency > 0 {
        app = app.layer(ConcurrencyLimitLayer::new(
            concurrency.max_concurrency,
            Duration::from_secs(concurrency.max_wait_secs),
        ));
    }

    let listener = TcpListener::bind(&bind_addr).await.expect("bind failed");

    tracing::info!("XMLA server running on {xmla_url}");

    axum::serve(listener, app).await.expect("server failed");
}
