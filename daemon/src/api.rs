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
//!   * `GET  /retrieve`         — bounded ranked retrieval   → local lexical/vector index
//!   * `GET  /status`           — entry count + peer count   → `Learnings::list`/`peer_count`

use std::net::{Ipv4Addr, SocketAddr};

use anyhow::Result;
use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use serde::{Deserialize, Serialize};

use crate::learnings::{Learning, Learnings};
use crate::retrieval::{RetrievalIndex, RetrievalMode, RetrievalResult};

const MAX_QUERY_LENGTH: usize = 2_000;
const MAX_TOP_K: usize = 50;

#[derive(Clone)]
struct AppState {
    store: Learnings,
    retrieval: RetrievalIndex,
}

/// Serve the HTTP API on `127.0.0.1:<port>` until the process stops.
pub async fn run(store: Learnings, port: u16) -> Result<()> {
    let state = AppState {
        store,
        retrieval: RetrievalIndex::default(),
    };
    let app = Router::new()
        .route("/status", get(status))
        .route("/learnings", post(create).get(search))
        .route("/learnings/{id}", get(get_one))
        .route("/retrieve", get(retrieve))
        .with_state(state);

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
    State(state): State<AppState>,
    Json(req): Json<CreateLearning>,
) -> Result<(StatusCode, Json<Learning>), AppError> {
    if req.title.trim().is_empty() || req.body.trim().is_empty() {
        return Err(AppError::bad_request("title and body must not be empty"));
    }
    let mut tags: Vec<String> = req
        .tags
        .into_iter()
        .map(|t| t.trim().to_string())
        .filter(|t| !t.is_empty())
        .collect();
    tags.sort_by_key(|t| t.to_lowercase());
    tags.dedup_by(|a, b| a.eq_ignore_ascii_case(b));
    let learning = state
        .store
        .add(req.title, req.body, tags, req.author)
        .await?;
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
    State(state): State<AppState>,
    Query(params): Query<SearchParams>,
) -> Result<Json<Vec<Learning>>, AppError> {
    if params.query.len() > MAX_QUERY_LENGTH {
        return Err(AppError::bad_request("query is too long"));
    }
    let mut all = state.store.list().await?;
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
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<Learning>, AppError> {
    match state.store.get(&id).await? {
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

async fn status(State(state): State<AppState>) -> Result<Json<StatusBody>, AppError> {
    let learnings = state.store.list().await?.len();
    let peers = state.store.peer_count().await?;
    Ok(Json(StatusBody { learnings, peers }))
}

// --------------------------------------------------------------------------------------------
// GET /retrieve?query=&mode=hybrid&top_k=5&tags=api,rust&excerpt_chars=500
// --------------------------------------------------------------------------------------------

fn default_mode() -> String {
    "hybrid".to_string()
}
fn default_top_k() -> usize {
    5
}
fn default_excerpt_chars() -> usize {
    500
}
fn default_min_score() -> f32 {
    0.3
}

#[derive(Deserialize)]
struct RetrieveParams {
    query: String,
    #[serde(default = "default_mode")]
    mode: String,
    #[serde(default = "default_top_k")]
    top_k: usize,
    #[serde(default)]
    tags: String,
    #[serde(default = "default_excerpt_chars")]
    excerpt_chars: usize,
    #[serde(default = "default_min_score")]
    min_score: f32,
}

#[derive(Serialize)]
struct RetrieveBody {
    query: String,
    mode: String,
    results: Vec<RetrievalResult>,
}

async fn retrieve(
    State(state): State<AppState>,
    Query(params): Query<RetrieveParams>,
) -> Result<Json<RetrieveBody>, AppError> {
    let query = params.query.trim();
    if query.is_empty() {
        return Err(AppError::bad_request("query must not be empty"));
    }
    if query.len() > MAX_QUERY_LENGTH {
        return Err(AppError::bad_request("query is too long"));
    }
    if params.top_k == 0 || params.top_k > MAX_TOP_K {
        return Err(AppError::bad_request("top_k must be between 1 and 50"));
    }
    if params.excerpt_chars == 0 || params.excerpt_chars > 4_000 {
        return Err(AppError::bad_request(
            "excerpt_chars must be between 1 and 4000",
        ));
    }
    if !(0.0..=1.0).contains(&params.min_score) {
        return Err(AppError::bad_request("min_score must be between 0 and 1"));
    }
    let mode = RetrievalMode::parse(&params.mode).map_err(AppError::from_bad_request)?;
    let tags = params
        .tags
        .split(',')
        .map(str::trim)
        .filter(|t| !t.is_empty())
        .map(str::to_string)
        .collect();
    let results = state
        .retrieval
        .search(
            state.store.list().await?,
            query.to_string(),
            mode,
            params.top_k,
            tags,
            params.excerpt_chars,
            params.min_score,
        )
        .await?;
    Ok(Json(RetrieveBody {
        query: query.to_string(),
        mode: params.mode,
        results,
    }))
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
    fn bad_request(message: &str) -> Self {
        Self {
            status: StatusCode::BAD_REQUEST,
            message: message.to_string(),
        }
    }

    fn from_bad_request(error: anyhow::Error) -> Self {
        Self {
            status: StatusCode::BAD_REQUEST,
            message: error.to_string(),
        }
    }

    fn not_found(message: &str) -> Self {
        Self {
            status: StatusCode::NOT_FOUND,
            message: message.to_string(),
        }
    }
}

impl From<anyhow::Error> for AppError {
    fn from(e: anyhow::Error) -> Self {
        Self {
            status: StatusCode::INTERNAL_SERVER_ERROR,
            message: format!("{e:#}"),
        }
    }
}

impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        (
            self.status,
            Json(serde_json::json!({ "error": self.message })),
        )
            .into_response()
    }
}
