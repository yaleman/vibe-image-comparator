use anyhow::Result;
use std::env;
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use walkdir::WalkDir;
use tracing::{warn, debug};

/// Expand tilde (~) in a path to the user's home directory
fn expand_tilde(path: &str) -> PathBuf {
    if path.starts_with("~/") || path == "~" {
        if let Some(home) = env::var_os("HOME") {
            let home_path = PathBuf::from(home);
            if path == "~" {
                home_path
            } else {
                home_path.join(&path[2..]) // Skip "~/"
            }
        } else {
            PathBuf::from(path)
        }
    } else {
        PathBuf::from(path)
    }
}

/// Check if a path should be ignored based on the ignore list
fn should_ignore_path(path: &Path, ignore_paths: &[String]) -> bool {
    let path_str = path.to_string_lossy();
    
    for ignore_pattern in ignore_paths {
        let expanded_pattern = expand_tilde(ignore_pattern);
        let pattern_str = expanded_pattern.to_string_lossy();
        
        // Check if the path starts with the ignore pattern
        if path_str.starts_with(pattern_str.as_ref()) {
            debug!("Ignoring path {} (matches pattern {})", path_str, pattern_str);
            return true;
        }
    }
    
    false
}

pub fn validate_image_format(path: &Path) -> Result<bool> {
    let mut file = fs::File::open(path)?;
    let mut buffer = [0u8; 16]; // Read first 16 bytes for magic number checking
    let bytes_read = file.read(&mut buffer)?;

    if bytes_read < 4 {
        return Ok(false); // File too small to have valid image header
    }

    let extension = path
        .extension()
        .and_then(|ext| ext.to_str())
        .map(|s| s.to_lowercase())
        .unwrap_or_default();

    match extension.as_str() {
        "png" => {
            // PNG magic number: 89 50 4E 47 0D 0A 1A 0A
            Ok(buffer.starts_with(&[0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A]))
        }
        "jpg" | "jpeg" => {
            // JPEG magic number: FF D8 FF
            Ok(buffer.starts_with(&[0xFF, 0xD8, 0xFF]))
        }
        "gif" => {
            // GIF magic number: GIF87a or GIF89a
            Ok(buffer.starts_with(b"GIF87a") || buffer.starts_with(b"GIF89a"))
        }
        "webp" => {
            // WebP magic number: RIFF ... WEBP
            Ok(buffer.starts_with(b"RIFF") && bytes_read >= 12 && &buffer[8..12] == b"WEBP")
        }
        "bmp" => {
            // BMP magic number: BM
            Ok(buffer.starts_with(b"BM"))
        }
        "tiff" | "tif" => {
            // TIFF magic number: MM00 (big endian) or II*\0 (little endian)
            Ok(buffer.starts_with(&[0x4D, 0x4D, 0x00, 0x2A])
                || buffer.starts_with(&[0x49, 0x49, 0x2A, 0x00]))
        }
        _ => Ok(true), // For unknown extensions, let the image crate handle validation
    }
}

pub fn should_process_image_file(
    path: &Path,
    image_extensions: &[&str],
    skip_validation: bool,
    debug: bool,
) -> bool {
    // Check if file is accessible (handles broken symlinks)
    if !path.exists() || fs::metadata(path).is_err() {
        warn!("Skipping inaccessible file: {}", path.display());
        return false;
    }

    let Some(ext) = path.extension() else {
        return false;
    };

    if !image_extensions.contains(&ext.to_string_lossy().to_lowercase().as_str()) {
        return false;
    }

    if skip_validation {
        if debug {
            debug!("Found image (validation skipped): {}", path.display());
        }
        return true;
    }

    // Validate file format before adding to processing list
    match validate_image_format(path) {
        Ok(true) => {
            if debug {
                debug!("Validated: {}", path.display());
            }
            true
        }
        Ok(false) => {
            if debug {
                warn!(
                    "File {} has wrong format for extension {}",
                    path.display(),
                    ext.to_string_lossy()
                );
            }
            false
        }
        Err(e) => {
            if debug {
                warn!("Could not validate {}: {}", path.display(), e);
            }
            false
        }
    }
}

pub fn process_file(
    path: &Path,
    image_extensions: &[&str],
    skip_validation: bool,
    debug: bool,
) -> Vec<PathBuf> {
    if should_process_image_file(path, image_extensions, skip_validation, debug) {
        vec![path.to_path_buf()]
    } else {
        vec![]
    }
}

pub fn process_dir(
    path: &Path,
    include_hidden: bool,
    image_extensions: &[&str],
    skip_validation: bool,
    debug: bool,
    ignore_paths: &[String],
) -> Result<Vec<PathBuf>> {
    let mut images = Vec::new();
    let walker = WalkDir::new(path)
        .follow_links(true)
        .into_iter()
        .filter_entry(|e| {
            let entry_path = e.path();
            
            // First check if this path should be ignored
            if should_ignore_path(entry_path, ignore_paths) {
                return false;
            }
            
            if include_hidden {
                true
            } else {
                // Allow the root path, skip hidden directories (starting with .)
                if e.depth() == 0 {
                    true
                } else if e.file_type().is_dir() {
                    if let Some(file_name) = e.file_name().to_str() {
                        !file_name.starts_with('.')
                    } else {
                        true
                    }
                } else {
                    true
                }
            }
        });

    for entry in walker {
        match entry {
            Ok(entry) => {
                let path = entry.path();
                if path.is_file() {
                    images.extend(process_file(path, image_extensions, skip_validation, debug));
                }
            }
            Err(e) => {
                warn!("Could not access directory entry: {e}");
            }
        }
    }

    Ok(images)
}

pub fn scan_for_images(
    paths: &[PathBuf],
    include_hidden: bool,
    debug: bool,
    skip_validation: bool,
    ignore_paths: &[String],
) -> Result<Vec<PathBuf>> {
    let mut images = Vec::new();
    let image_extensions = ["jpg", "jpeg", "png", "gif", "bmp", "tiff", "tif", "webp"];

    for path in paths {
        // Check if the path itself should be ignored
        if should_ignore_path(path, ignore_paths) {
            debug!("Skipping ignored path: {}", path.display());
            continue;
        }
        
        if path.is_file() {
            images.extend(process_file(
                path,
                &image_extensions,
                skip_validation,
                debug,
            ));
        } else if path.is_dir() {
            images.extend(process_dir(
                path,
                include_hidden,
                &image_extensions,
                skip_validation,
                debug,
                ignore_paths,
            )?);
        }
    }

    Ok(images)
}
