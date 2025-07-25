use anyhow::Result;
use axum::{
    extract::{Query, State},
    http::StatusCode,
    response::{Html, Json},
    routing::{get, post},
    Router,
};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::sync::Arc;
use tokio::net::TcpListener;

use crate::cache::{Config, HashCache};
use crate::hasher::{find_duplicates, generate_hashes_with_cache, get_duplicates_from_cache};
use crate::scanner::scan_for_images;

#[derive(Clone)]
pub struct AppState {
    config: Config,
    threshold_override: Option<u32>,
    grid_size_override: Option<u32>,
}

#[derive(Deserialize)]
pub struct ScanRequest {
    paths: Vec<String>,
    threshold: Option<u32>,
    grid_size: Option<u32>,
    include_hidden: Option<bool>,
    debug: Option<bool>,
    skip_validation: Option<bool>,
}

#[derive(Serialize)]
pub struct ScanResponse {
    success: bool,
    message: String,
    duplicate_count: usize,
    duplicates: Vec<Vec<String>>,
}

#[derive(Deserialize)]
pub struct MatchesQuery {
    threshold: Option<u32>,
}

#[derive(Serialize)]
pub struct MatchesResponse {
    success: bool,
    duplicates: Vec<Vec<String>>,
    threshold: u32,
}

#[derive(Serialize)]
pub struct ConfigResponse {
    grid_size: u32,
    threshold: u32,
    database_path: Option<String>,
}

pub async fn start_server(
    config: Config,
    threshold_override: Option<u32>,
    grid_size_override: Option<u32>,
) -> Result<()> {
    let state = AppState {
        config,
        threshold_override,
        grid_size_override,
    };

    let app = Router::new()
        .route("/", get(serve_index))
        .route("/api/scan", post(handle_scan))
        .route("/api/matches", get(handle_matches))
        .route("/api/config", get(handle_config))
        .with_state(Arc::new(state));

    let listener = TcpListener::bind("127.0.0.1:8080").await?;
    println!("🌐 Web server running at http://127.0.0.1:8080");
    println!("Press Ctrl+C to stop the server");

    axum::serve(listener, app).await?;
    Ok(())
}

async fn serve_index() -> Html<&'static str> {
    Html(include_str!("../static/index.html"))
}

async fn handle_scan(
    State(state): State<Arc<AppState>>,
    Json(request): Json<ScanRequest>,
) -> Result<Json<ScanResponse>, StatusCode> {
    let cache = HashCache::new(state.config.database_path.as_deref())
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    let threshold = request
        .threshold
        .or(state.threshold_override)
        .unwrap_or(state.config.threshold);
    let grid_size = request
        .grid_size
        .or(state.grid_size_override)
        .unwrap_or(state.config.grid_size);

    let paths: Vec<PathBuf> = request.paths.iter().map(PathBuf::from).collect();

    let images = scan_for_images(
        &paths,
        request.include_hidden.unwrap_or(false),
        request.debug.unwrap_or(false),
        request.skip_validation.unwrap_or(false),
    )
    .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    let hashes = generate_hashes_with_cache(&images, grid_size, &cache, false)
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    let duplicates = find_duplicates(&hashes, threshold);

    let duplicate_strings: Vec<Vec<String>> = duplicates
        .iter()
        .map(|group| group.iter().map(|p| p.display().to_string()).collect())
        .collect();

    let response = ScanResponse {
        success: true,
        message: format!(
            "Scanned {} images, found {} duplicate sets",
            images.len(),
            duplicates.len()
        ),
        duplicate_count: duplicates.len(),
        duplicates: duplicate_strings,
    };

    Ok(Json(response))
}

async fn handle_matches(
    State(state): State<Arc<AppState>>,
    Query(query): Query<MatchesQuery>,
) -> Result<Json<MatchesResponse>, StatusCode> {
    let cache = HashCache::new(state.config.database_path.as_deref())
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    let threshold = query
        .threshold
        .or(state.threshold_override)
        .unwrap_or(state.config.threshold);

    let duplicates = get_duplicates_from_cache(&cache, threshold)
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    let duplicate_strings: Vec<Vec<String>> = duplicates
        .iter()
        .map(|group| group.iter().map(|p| p.display().to_string()).collect())
        .collect();

    let response = MatchesResponse {
        success: true,
        duplicates: duplicate_strings,
        threshold,
    };

    Ok(Json(response))
}

async fn handle_config(State(state): State<Arc<AppState>>) -> Json<ConfigResponse> {
    let response = ConfigResponse {
        grid_size: state.grid_size_override.unwrap_or(state.config.grid_size),
        threshold: state.threshold_override.unwrap_or(state.config.threshold),
        database_path: state.config.database_path.clone(),
    };

    Json(response)
}