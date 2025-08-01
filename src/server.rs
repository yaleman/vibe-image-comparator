use anyhow::Result;
use axum::{
    extract::{Path, Query, State},
    http::{header, StatusCode},
    response::{Json, Response},
    routing::{get, post},
    Router,
};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::sync::Arc;
use tokio::net::TcpListener;
use tracing::{error, info, instrument, warn};

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

#[derive(Deserialize, Debug)]
pub struct MatchesQuery {
    threshold: Option<u32>,
    count: Option<usize>,
    offset: Option<usize>,
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

#[derive(Deserialize)]
pub struct DeleteFileRequest {
    path: String,
}

#[derive(Serialize)]
pub struct DeleteFileResponse {
    success: bool,
    message: String,
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
        .route("/styles.css", get(serve_css))
        .route("/api/scan", post(handle_scan))
        .route("/api/matches", get(handle_matches))
        .route("/api/config", get(handle_config))
        .route("/api/image/{*path}", get(serve_image))
        .route("/api/check-files", post(check_files_exist))
        .route("/api/delete-file", post(delete_file))
        .with_state(Arc::new(state));

    let listener = TcpListener::bind("127.0.0.1:8080").await?;
    info!("🌐 Web server running at http://127.0.0.1:8080");
    info!("Press Ctrl+C to stop the server");

    axum::serve(listener, app).await?;
    Ok(())
}

async fn serve_index() -> Result<Response, StatusCode> {
    let html_content = include_str!("../static/index.html");

    let response = Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, "text/html; charset=utf-8")
        .header(header::CACHE_CONTROL, "no-cache, no-store, must-revalidate")
        .header(header::PRAGMA, "no-cache")
        .header(header::EXPIRES, "0")
        .body(html_content.into())
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    Ok(response)
}

async fn handle_scan(
    State(state): State<Arc<AppState>>,
    Json(request): Json<ScanRequest>,
) -> Result<Json<ScanResponse>, StatusCode> {
    let effective_config =
        state
            .config
            .with_overrides(state.grid_size_override, state.threshold_override, None);
    let cache = HashCache::new(effective_config.database_path.as_deref())
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    let threshold = request
        .threshold
        .or(state.threshold_override)
        .unwrap_or(effective_config.threshold);
    let grid_size = request
        .grid_size
        .or(state.grid_size_override)
        .unwrap_or(effective_config.grid_size);

    let paths: Vec<PathBuf> = request.paths.iter().map(PathBuf::from).collect();
    let ignore_paths = effective_config.ignore_paths.clone();

    // Run the expensive scanning and processing in a blocking task
    let scan_result =
        tokio::task::spawn_blocking(move || -> Result<ScanResponse, anyhow::Error> {
            let images = scan_for_images(
                &paths,
                request.include_hidden.unwrap_or(false),
                request.debug.unwrap_or(false),
                request.skip_validation.unwrap_or(false),
                &ignore_paths,
            )?;

            let hashes = generate_hashes_with_cache(&images, grid_size, &cache, false)?;

            let duplicates = find_duplicates(&hashes, threshold);

            // Cache the duplicate groups for future use
            if let Err(e) = cache.store_duplicate_groups(threshold, &duplicates) {
                warn!("Failed to cache duplicate groups: {}", e);
            }

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

            Ok(ScanResponse {
                success: true,
                message: format!(
                    "Scanned {} images, found {} duplicate sets",
                    images.len(),
                    duplicates.len()
                ),
                duplicate_count: duplicates.len(),
                duplicates: duplicate_file_infos,
            })
        })
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    Ok(Json(scan_result))
}

#[instrument(level = "info", skip(state))]
async fn handle_matches(
    State(state): State<Arc<AppState>>,
    Query(query): Query<MatchesQuery>,
) -> Result<Json<MatchesResponse>, StatusCode> {
    let effective_config =
        state
            .config
            .with_overrides(state.grid_size_override, state.threshold_override, None);
    let cache = HashCache::new(effective_config.database_path.as_deref())
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    let threshold = query
        .threshold
        .or(state.threshold_override)
        .unwrap_or(effective_config.threshold);

    // Run the expensive computation in a blocking task to avoid blocking the async runtime
    let duplicates = tokio::task::spawn_blocking(move || {
        get_duplicates_from_cache(&cache, threshold, query.count, query.offset)
    })
    .await
    .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
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
        grid_size: state
            .grid_size_override
            .unwrap_or(state.config.grid_size.unwrap_or(128)),
        threshold: state
            .threshold_override
            .unwrap_or(state.config.threshold.unwrap_or(15)),
        database_path: state.config.database_path.clone(),
    };

    Json(response)
}

#[instrument(level = "info")]
async fn serve_image(Path(image_path): Path<String>) -> Result<Response, StatusCode> {
    // URL decode the path first
    let decoded_path = match urlencoding::decode(&image_path) {
        Ok(path) => path.to_string(),
        Err(e) => {
            error!("Failed to decode URL path '{}': {}", image_path, e);
            return Err(StatusCode::BAD_REQUEST);
        }
    };

    let file_path = std::path::Path::new(&decoded_path);

    // Security check: ensure the path is absolute and exists
    if !file_path.is_absolute() {
        error!("Requested path is not absolute: {}", file_path.display());
        return Err(StatusCode::BAD_REQUEST);
    }

    if !file_path.exists() {
        error!("Requested file does not exist: {}", file_path.display());
        return Err(StatusCode::NOT_FOUND);
    }

    // Check if it's actually a file (not a directory)
    if !file_path.is_file() {
        error!("Requested path is not a file: {}", file_path.display());
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

async fn check_files_exist(Json(request): Json<CheckFilesRequest>) -> Json<CheckFilesResponse> {
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

async fn serve_css() -> Result<Response, StatusCode> {
    let css_content = include_str!("../static/styles.css");

    let response = Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, "text/css")
        .header(header::CACHE_CONTROL, "no-cache, no-store, must-revalidate")
        .header(header::PRAGMA, "no-cache")
        .header(header::EXPIRES, "0")
        .body(css_content.into())
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    Ok(response)
}

async fn delete_file(
    State(state): State<Arc<AppState>>,
    Json(request): Json<DeleteFileRequest>,
) -> Json<DeleteFileResponse> {
    let file_path = std::path::Path::new(&request.path);

    // Security check: ensure the path is absolute
    if !file_path.is_absolute() {
        return Json(DeleteFileResponse {
            success: false,
            message: "Path must be absolute".to_string(),
        });
    }

    // Check if file exists
    if !file_path.exists() {
        return Json(DeleteFileResponse {
            success: false,
            message: "File does not exist".to_string(),
        });
    }

    // Check if it's actually a file (not a directory)
    if !file_path.is_file() {
        return Json(DeleteFileResponse {
            success: false,
            message: "Path is not a file".to_string(),
        });
    }

    // Get the effective config for database path
    let effective_config =
        state
            .config
            .with_overrides(state.grid_size_override, state.threshold_override, None);

    // Attempt to delete the file
    match std::fs::remove_file(file_path) {
        Ok(()) => {
            info!("Deleted file: {}", file_path.display());

            // Remove file from database
            if let Ok(cache) = HashCache::new(effective_config.database_path.as_deref()) {
                if let Err(e) = cache.remove_file_entry(file_path) {
                    warn!("Failed to remove file from database: {}", e);
                    // Don't fail the entire operation if database cleanup fails
                }
            } else {
                warn!("Failed to connect to database for cleanup");
            }

            Json(DeleteFileResponse {
                success: true,
                message: "File deleted successfully".to_string(),
            })
        }
        Err(e) => {
            error!("Failed to delete file {}: {}", file_path.display(), e);
            Json(DeleteFileResponse {
                success: false,
                message: format!("Failed to delete file: {e}"),
            })
        }
    }
}
