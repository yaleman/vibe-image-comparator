use anyhow::Result;
use axum::{
    extract::{Path, Query, State},
    http::{header, StatusCode},
    response::{Html, Json, Response},
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
pub struct FileInfo {
    path: String,
    exists: bool,
}

#[derive(Serialize)]
pub struct ScanResponse {
    success: bool,
    message: String,
    duplicate_count: usize,
    duplicates: Vec<Vec<FileInfo>>,
}

#[derive(Deserialize)]
pub struct MatchesQuery {
    threshold: Option<u32>,
}

#[derive(Serialize)]
pub struct MatchesResponse {
    success: bool,
    duplicates: Vec<Vec<FileInfo>>,
    threshold: u32,
}

#[derive(Serialize)]
pub struct ConfigResponse {
    grid_size: u32,
    threshold: u32,
    database_path: Option<String>,
}

#[derive(Deserialize)]
pub struct CheckFilesRequest {
    paths: Vec<String>,
}

#[derive(Serialize)]
pub struct CheckFilesResponse {
    files: Vec<FileInfo>,
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
        .route("/api/image/{*path}", get(serve_image))
        .route("/api/check-files", post(check_files_exist))
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

    let duplicate_file_infos: Vec<Vec<FileInfo>> = duplicates
        .iter()
        .map(|group| {
            group
                .iter()
                .map(|p| FileInfo {
                    path: p.display().to_string(),
                    exists: p.exists(),
                })
                .collect()
        })
        .collect();

    let response = ScanResponse {
        success: true,
        message: format!(
            "Scanned {} images, found {} duplicate sets",
            images.len(),
            duplicates.len()
        ),
        duplicate_count: duplicates.len(),
        duplicates: duplicate_file_infos,
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

    let duplicate_file_infos: Vec<Vec<FileInfo>> = duplicates
        .iter()
        .map(|group| {
            group
                .iter()
                .map(|p| FileInfo {
                    path: p.display().to_string(),
                    exists: true, // Don't check existence here - will check lazily when needed
                })
                .collect()
        })
        .collect();

    let response = MatchesResponse {
        success: true,
        duplicates: duplicate_file_infos,
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

async fn serve_image(Path(image_path): Path<String>) -> Result<Response, StatusCode> {
    // Remove leading slash if present
    let clean_path = image_path.strip_prefix('/').unwrap_or(&image_path);
    let file_path = std::path::Path::new(clean_path);
    
    // Security check: ensure the path is absolute and exists
    if !file_path.is_absolute() {
        return Err(StatusCode::BAD_REQUEST);
    }
    
    if !file_path.exists() {
        return Err(StatusCode::NOT_FOUND);
    }
    
    // Check if it's actually a file (not a directory)
    if !file_path.is_file() {
        return Err(StatusCode::BAD_REQUEST);
    }
    
    // Read the image file
    let image_data = match tokio::fs::read(file_path).await {
        Ok(data) => data,
        Err(_) => return Err(StatusCode::INTERNAL_SERVER_ERROR),
    };
    
    // Determine content type based on file extension
    let content_type = match file_path.extension().and_then(|ext| ext.to_str()) {
        Some("jpg") | Some("jpeg") => "image/jpeg",
        Some("png") => "image/png",
        Some("gif") => "image/gif",
        Some("webp") => "image/webp",
        Some("bmp") => "image/bmp",
        Some("tiff") | Some("tif") => "image/tiff",
        _ => "application/octet-stream",
    };
    
    let response = Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, content_type)
        .header(header::CACHE_CONTROL, "public, max-age=3600") // Cache for 1 hour
        .body(image_data.into())
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    
    Ok(response)
}

async fn check_files_exist(
    Json(request): Json<CheckFilesRequest>,
) -> Json<CheckFilesResponse> {
    let files: Vec<FileInfo> = request
        .paths
        .iter()
        .map(|path_str| {
            let path = std::path::Path::new(path_str);
            FileInfo {
                path: path_str.clone(),
                exists: path.exists(),
            }
        })
        .collect();

    Json(CheckFilesResponse { files })
}