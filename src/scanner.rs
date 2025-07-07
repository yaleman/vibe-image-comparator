use anyhow::Result;
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use walkdir::WalkDir;

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
        eprintln!("Warning: Skipping inaccessible file: {}", path.display());
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
            println!("Found image (validation skipped): {}", path.display());
        }
        return true;
    }

    // Validate file format before adding to processing list
    match validate_image_format(path) {
        Ok(true) => {
            if debug {
                println!("Found valid image: {}", path.display());
            }
            true
        }
        Ok(false) => {
            if debug {
                eprintln!(
                    "Warning: File {} has wrong format for extension {}",
                    path.display(),
                    ext.to_string_lossy()
                );
            }
            false
        }
        Err(e) => {
            if debug {
                eprintln!("Warning: Could not validate {}: {}", path.display(), e);
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
) -> Result<Vec<PathBuf>> {
    let mut images = Vec::new();
    let walker = WalkDir::new(path)
        .follow_links(true)
        .into_iter()
        .filter_entry(|e| {
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
                eprintln!("Warning: Could not access directory entry: {}", e);
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
) -> Result<Vec<PathBuf>> {
    let mut images = Vec::new();
    let image_extensions = ["jpg", "jpeg", "png", "gif", "bmp", "tiff", "tif", "webp"];

    for path in paths {
        if path.is_file() {
            images.extend(process_file(path, &image_extensions, skip_validation, debug));
        } else if path.is_dir() {
            images.extend(process_dir(path, include_hidden, &image_extensions, skip_validation, debug)?);
        }
    }

    Ok(images)
}