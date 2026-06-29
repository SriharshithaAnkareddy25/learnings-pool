//! Phase 6 — the local HTTP API.
//!
//! A small [axum] server, bound to `127.0.0.1` only, that exposes the learnings store over HTTP.
//! The Python MCP server (Phase 7) is a short-lived process that can't host the iroh node, so it
//! talks to this long-running daemon instead. This HTTP boundary is the *only* coupling between
//! the Rust and Python halves. Localhost is the trust boundary — we never bind a public address.
//!
//! Routes:
//!   * `POST /learnings`        — file a learning            → `Learnings::add`
//!   * `GET  /learnings?query=` — search learnings           → `Learnings::list` + filter
//!   * `GET  /learnings/{id}`   — fetch one by id            → `Learnings::get`
//!   * `GET  /status`           — entry count + peer count   → `Learnings::list`/`peer_count`

use std::net::{Ipv4Addr, SocketAddr};

use anyhow::Result;
use axum::{
    Json, Router,
    extract::{Path, Query, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::{get, post},
};
use serde::{Deserialize, Serialize};

use crate::learnings::{Learning, Learnings};

/// Serve the HTTP API on `127.0.0.1:<port>` until the process stops.
pub async fn run(store: Learnings, port: u16) -> Result<()> {
    let app = Router::new()
        .route("/status", get(status))
        .route("/learnings", post(create).get(search))
        .route("/learnings/{id}", get(get_one))
        .with_state(store);

    let addr = SocketAddr::from((Ipv4Addr::LOCALHOST, port));
    let listener = tokio::net::TcpListener::bind(addr).await?;
    println!("HTTP API listening on http://{addr}");
    axum::serve(listener, app).await?;
    Ok(())
}

// --------------------------------------------------------------------------------------------
// POST /learnings
// --------------------------------------------------------------------------------------------

#[derive(Deserialize)]
struct CreateLearning {
    title: String,
    body: String,
    #[serde(default)]
    tags: Vec<String>,
    /// Who's filing it; defaults to "me" if the caller doesn't say.
    #[serde(default = "default_author")]
    author: String,
}

fn default_author() -> String {
    "me".to_string()
}

async fn create(
    State(store): State<Learnings>,
    Json(req): Json<CreateLearning>,
) -> Result<(StatusCode, Json<Learning>), AppError> {
    let tags = req.tags.into_iter().filter(|t| !t.is_empty()).collect();
    let learning = store.add(req.title, req.body, tags, req.author).await?;
    Ok((StatusCode::CREATED, Json(learning)))
}

// --------------------------------------------------------------------------------------------
// GET /learnings?query=
// --------------------------------------------------------------------------------------------

#[derive(Deserialize)]
struct SearchParams {
    #[serde(default)]
    query: String,
}

async fn search(
    State(store): State<Learnings>,
    Query(params): Query<SearchParams>,
) -> Result<Json<Vec<Learning>>, AppError> {
    let mut all = store.list().await?;
    let q = params.query.trim().to_lowercase();
    if !q.is_empty() {
        all.retain(|l| matches_query(l, &q));
    }
    Ok(Json(all))
}

/// Case-insensitive substring match across title, body, and tags.
fn matches_query(l: &Learning, q: &str) -> bool {
    l.title.to_lowercase().contains(q)
        || l.body.to_lowercase().contains(q)
        || l.tags.iter().any(|t| t.to_lowercase().contains(q))
}

// --------------------------------------------------------------------------------------------
// GET /learnings/{id}
// --------------------------------------------------------------------------------------------

async fn get_one(
    State(store): State<Learnings>,
    Path(id): Path<String>,
) -> Result<Json<Learning>, AppError> {
    match store.get(&id).await? {
        Some(l) => Ok(Json(l)),
        None => Err(AppError::not_found("learning not found")),
    }
}

// --------------------------------------------------------------------------------------------
// GET /status
// --------------------------------------------------------------------------------------------

#[derive(Serialize)]
struct StatusBody {
    /// Number of learnings currently in the shared notebook.
    learnings: usize,
    /// Number of peers this notebook is set up to sync with.
    peers: usize,
}

async fn status(State(store): State<Learnings>) -> Result<Json<StatusBody>, AppError> {
    let learnings = store.list().await?.len();
    let peers = store.peer_count().await?;
    Ok(Json(StatusBody { learnings, peers }))
}

// --------------------------------------------------------------------------------------------
// Errors → HTTP responses
// --------------------------------------------------------------------------------------------

/// Turns a handler failure into an HTTP response. Any store error becomes a 500; an explicit
/// not-found becomes a 404. The body is always `{"error": "..."}`.
struct AppError {
    status: StatusCode,
    message: String,
}

impl AppError {
    fn not_found(message: &str) -> Self {
        Self { status: StatusCode::NOT_FOUND, message: message.to_string() }
    }
}

impl From<anyhow::Error> for AppError {
    fn from(e: anyhow::Error) -> Self {
        Self { status: StatusCode::INTERNAL_SERVER_ERROR, message: format!("{e:#}") }
    }
}

impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        (self.status, Json(serde_json::json!({ "error": self.message }))).into_response()
    }
}
