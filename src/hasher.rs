use anyhow::Result;
use imghash::{perceptual::PerceptualHasher, ImageHash, ImageHasher};
use rayon::prelude::*;
use sha2::{Digest, Sha256};
use std::fs;
use std::path::{Path, PathBuf};

use crate::cache::{FileMetadata, HashCache};

#[derive(Debug, Clone)]
pub struct ImageMetadata {
    pub path: PathBuf,
    pub size: u64,
    pub sha256: String,
}

pub fn calculate_file_sha256(path: &Path) -> Result<String> {
    let data = fs::read(path)?;
    let mut hasher = Sha256::new();
    hasher.update(&data);
    let result = hasher.finalize();
    Ok(format!("{result:x}"))
}

pub fn get_file_metadata(path: &Path) -> Result<(u64, String)> {
    let metadata = fs::metadata(path)?;
    let size = metadata.len();
    let sha256 = calculate_file_sha256(path)?;
    Ok((size, sha256))
}

pub fn generate_rotation_invariant_hash_safe(
    hasher: &PerceptualHasher,
    img: &image::DynamicImage,
) -> Result<ImageHash> {
    let original_hash = hasher.hash_from_img(img);
    let rotated_90 = img.rotate90();
    let rotated_90_hash = hasher.hash_from_img(&rotated_90);
    let rotated_180 = img.rotate180();
    let rotated_180_hash = hasher.hash_from_img(&rotated_180);
    let rotated_270 = img.rotate270();
    let rotated_270_hash = hasher.hash_from_img(&rotated_270);

    let mut candidates = vec![
        (original_hash.encode(), original_hash),
        (rotated_90_hash.encode(), rotated_90_hash),
        (rotated_180_hash.encode(), rotated_180_hash),
        (rotated_270_hash.encode(), rotated_270_hash),
    ];

    candidates.sort_by_key(|(encoded, _)| encoded.clone());
    candidates
        .into_iter()
        .next()
        .map(|(_, hash)| hash)
        .ok_or_else(|| anyhow::anyhow!("No rotation candidate hashes generated"))
}

pub fn generate_hashes_with_cache(
    images: &[PathBuf],
    _grid_size: u32,
    cache: &HashCache,
    debug: bool,
) -> Result<Vec<(PathBuf, ImageHash)>> {
    // First, collect metadata for all images in parallel
    let metadata_results: Vec<_> = images
        .par_iter()
        .map(|image_path| match get_file_metadata(image_path) {
            Ok((size, sha256)) => Some(ImageMetadata {
                path: image_path.clone(),
                size,
                sha256,
            }),
            Err(e) => {
                eprintln!(
                    "Warning: Could not get metadata for {} (possibly broken symlink): {}",
                    image_path.display(),
                    e
                );
                None
            }
        })
        .collect();

    // Separate cache hits from cache misses (sequential due to SQLite constraints)
    let mut hashes = Vec::new();
    let mut cache_hits = 0;
    let mut cache_misses = 0;
    let mut files_to_process: Vec<ImageMetadata> = Vec::new();

    // First pass: check cache and collect cache hits
    for metadata in metadata_results.into_iter().flatten() {
        if let Ok(Some(hash_string)) = cache.get_cached_hash(&metadata.path, metadata.size, &metadata.sha256) {
            // Decode the string back to ImageHash
            match ImageHash::decode(&hash_string, 8, 8) {
                Ok(hash) => {
                    if debug {
                        println!("Cache hit: {}", metadata.path.display());
                    }
                    hashes.push((metadata.path, hash));
                    cache_hits += 1;
                }
                Err(e) => {
                    eprintln!(
                        "Warning: Invalid cached hash format for {}: {}",
                        metadata.path.display(),
                        e
                    );
                    // Need to reprocess this file
                    files_to_process.push(metadata);
                }
            }
        } else {
            // Cache miss - need to process this file
            files_to_process.push(metadata);
        }
    }

    // Only create hasher if we have files to process
    if !files_to_process.is_empty() {
        let hasher = PerceptualHasher::default();

        // Second pass: process files in parallel, then store results sequentially
        let processing_results: Vec<_> = files_to_process
            .par_iter()
            .map(|metadata| {
                if debug {
                    println!("Processing: {}", metadata.path.display());
                }

                match image::open(&metadata.path) {
                    Ok(img) => match generate_rotation_invariant_hash_safe(&hasher, &img) {
                        Ok(hash) => {
                            let file_metadata = FileMetadata {
                                path: metadata.path.clone(),
                                size: metadata.size,
                                sha256: metadata.sha256.clone(),
                                perceptual_hash: hash.encode(),
                            };
                            Ok((metadata.path.clone(), hash, Some(file_metadata)))
                        }
                        Err(e) => {
                            eprintln!(
                                "Warning: Could not generate hash for {}: {}",
                                metadata.path.display(),
                                e
                            );
                            Err(metadata.path.clone())
                        }
                    },
                    Err(e) => {
                        // Provide more specific error messages for common image format issues
                        let error_msg = if e.to_string().contains("invalid PNG signature") {
                            format!("Invalid PNG file (corrupted or wrong format): {e}")
                        } else if e.to_string().contains("invalid JPEG") {
                            format!("Invalid JPEG file (corrupted or wrong format): {e}")
                        } else if e.to_string().contains("unsupported") {
                            format!("Unsupported image format: {e}")
                        } else {
                            format!("Image decoding error: {e}")
                        };

                        if debug {
                            eprintln!(
                                "Warning: Could not open {}: {}",
                                metadata.path.display(),
                                error_msg
                            );
                        } else {
                            eprintln!("Warning: Skipping {}: {}", metadata.path.display(), error_msg);
                        }

                        Err(metadata.path.clone())
                    }
                }
            })
            .collect();

        // Now handle cache operations and result collection sequentially
        for result in processing_results {
            match result {
                Ok((image_path, hash, metadata_opt)) => {
                    if let Some(metadata) = metadata_opt {
                        if let Err(e) = cache.store_hash(&metadata) {
                            eprintln!(
                                "Warning: Could not cache hash for {}: {}",
                                image_path.display(),
                                e
                            );
                        }
                    }
                    hashes.push((image_path, hash));
                    cache_misses += 1;
                }
                Err(image_path) => {
                    // Remove broken file from cache if it exists
                    if let Err(cache_err) = cache.remove_file_entry(&image_path) {
                        eprintln!("Warning: Could not remove broken file from cache: {cache_err}");
                    }
                }
            }
        }
    }

    if cache_hits > 0 || cache_misses > 0 {
        println!("Cache stats: {cache_hits} hits, {cache_misses} misses");
    }

    Ok(hashes)
}

pub fn find_duplicates(hashes: &[(PathBuf, ImageHash)], threshold: u32) -> Vec<Vec<PathBuf>> {
    let mut groups: Vec<Vec<PathBuf>> = Vec::new();
    let mut processed = vec![false; hashes.len()];

    for (i, (path1, hash1)) in hashes.iter().enumerate() {
        if processed[i] {
            continue;
        }

        let mut group = vec![path1.clone()];
        processed[i] = true;

        // Parallelize the distance computation for remaining hashes
        let remaining_hashes: Vec<_> = hashes
            .iter()
            .enumerate()
            .skip(i + 1)
            .filter(|(j, _)| !processed[*j])
            .collect();

        let matches: Vec<_> = remaining_hashes
            .par_iter()
            .filter_map(|(j, (path2, hash2))| match hash1.distance(hash2) {
                Ok(distance) => {
                    if distance <= threshold as usize {
                        Some((*j, path2.clone()))
                    } else {
                        None
                    }
                }
                Err(_) => None,
            })
            .collect();

        for (j, path2) in matches {
            group.push(path2);
            processed[j] = true;
        }

        if group.len() > 1 {
            groups.push(group);
        }
    }

    groups
}

pub fn get_duplicates_from_cache(cache: &HashCache, threshold: u32) -> Result<Vec<Vec<PathBuf>>> {
    println!("Retrieving hashes from cache...");
    let cached_data = cache.get_all_cached_hashes()?;
    
    if cached_data.is_empty() {
        println!("No cached hashes found");
        return Ok(Vec::new());
    }

    println!("Found {} cached entries", cached_data.len());
    
    // Convert cached data to (PathBuf, ImageHash) tuples
    let mut hashes = Vec::new();
    let mut failed_conversions = 0;
    
    for (path, hash_string) in cached_data {
        // Decode the string to ImageHash
        match ImageHash::decode(&hash_string, 8, 8) {
            Ok(hash) => {
                hashes.push((path, hash));
            }
            Err(e) => {
                eprintln!("Warning: Could not decode hash for {}: {}", path.display(), e);
                failed_conversions += 1;
            }
        }
    }
    
    if failed_conversions > 0 {
        println!("Warning: Failed to convert {failed_conversions} cached entries");
    }
    
    println!("Processing {} valid cached hashes for duplicates...", hashes.len());
    
    // Find duplicates using the existing function
    let duplicates = find_duplicates(&hashes, threshold);
    
    Ok(duplicates)
}
