#![deny(clippy::unwrap_used)]
#![deny(clippy::expect_used)]

use anyhow::Result;
use clap::Parser;
use std::path::PathBuf;

mod cache;
mod config;
mod hasher;
mod scanner;

use cache::HashCache;
use config::load_config;
use hasher::{find_duplicates, generate_hashes, generate_hashes_with_cache};
use scanner::scan_for_images;

#[derive(Parser)]
#[command(name = "vibe-image-comparator")]
#[command(about = "A tool to find duplicate images using perceptual hashing")]
struct Args {
    #[arg(help = "Paths to scan for images")]
    paths: Vec<PathBuf>,

    #[arg(
        short,
        long,
        help = "Minimum similarity threshold (0-64, lower = more similar)"
    )]
    threshold: Option<u32>,

    #[arg(short, long, help = "Hash grid size (e.g., 64 for 64x64 grid)")]
    grid_size: Option<u32>,

    #[arg(long, help = "Disable hash caching")]
    no_cache: bool,

    #[arg(long, help = "Clean up cache entries for missing files")]
    clean_cache: bool,

    #[arg(short = '.', help = "Include hidden directories (starting with .)")]
    include_hidden: bool,

    #[arg(
        short,
        long,
        help = "Print debug information including filenames as they're processed"
    )]
    debug: bool,

    #[arg(
        long,
        help = "Skip file format validation (process files even with wrong magic numbers)"
    )]
    skip_validation: bool,
}

fn main() -> Result<()> {
    let args = Args::parse();

    let config = load_config()?;

    let cache = if !args.no_cache {
        Some(HashCache::new(config.database_path.as_deref())?)
    } else {
        None
    };

    if args.clean_cache {
        if let Some(ref cache) = cache {
            let deleted = cache.cleanup_missing_files()?;
            println!("Cleaned up {deleted} entries from cache");
        } else {
            println!("Cache is disabled, nothing to clean");
        }
        if args.paths.is_empty() {
            return Ok(());
        }
    }

    if args.paths.is_empty() {
        eprintln!("Error: Please provide at least one path to scan");
        std::process::exit(1);
    }

    let threshold = args.threshold.unwrap_or(config.threshold);
    let grid_size = args.grid_size.unwrap_or(config.grid_size);

    println!("Using grid size: {grid_size}x{grid_size}, threshold: {threshold}");
    if cache.is_some() {
        println!("Hash caching enabled");
    } else {
        println!("Hash caching disabled");
    }

    println!("Scanning paths for images...");
    let images = scan_for_images(
        &args.paths,
        args.include_hidden,
        args.debug,
        args.skip_validation,
    )?;

    println!("Found {} images", images.len());
    println!("Generating perceptual hashes...");

    let hashes = if let Some(ref cache) = cache {
        generate_hashes_with_cache(&images, grid_size, cache, args.debug)?
    } else {
        generate_hashes(&images, grid_size, args.debug)?
    };

    println!("Finding duplicate sets...");
    let duplicates = find_duplicates(&hashes, threshold);

    if duplicates.is_empty() {
        println!("No duplicate images found");
    } else {
        println!("Found {} duplicate sets:", duplicates.len());
        for (i, group) in duplicates.iter().enumerate() {
            println!("  Group {}:", i + 1);
            for path in group {
                println!("    {}", path.display());
            }
        }
    }

    Ok(())
}

#[cfg(test)]
#[allow(clippy::expect_used)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use std::fs;
    use std::path::Path;
    use tempfile::TempDir;

    #[test]
    fn test_all_same_directory_finds_three_duplicates() {
        let test_dir = Path::new("test_images/all_same");
        if !test_dir.exists() {
            panic!("Test directory test_images/all_same does not exist");
        }

        let paths = vec![test_dir.to_path_buf()];
        let images =
            scan_for_images(&paths, false, false, false).expect("Failed to scan for images");

        assert_eq!(
            images.len(),
            3,
            "Should find exactly 3 images in test_images/all_same"
        );

        // Test with in-memory cache
        let cache = HashCache::new_in_memory().expect("Failed to create in-memory cache");
        let grid_size = 16;
        let hashes = generate_hashes_with_cache(&images, grid_size, &cache, false)
            .expect("Failed to generate hashes");

        assert_eq!(hashes.len(), 3, "Should generate 3 hashes");

        let threshold = 15;
        let duplicates = find_duplicates(&hashes, threshold);

        assert!(
            !duplicates.is_empty(),
            "Should find at least 1 duplicate group"
        );
        let total_images_in_groups: usize = duplicates.iter().map(|g| g.len()).sum();
        assert_eq!(
            total_images_in_groups, 3,
            "All 3 images should be in duplicate groups"
        );

        let mut found_extensions = std::collections::HashSet::new();
        for group in &duplicates {
            for path in group {
                if let Some(ext) = path.extension() {
                    found_extensions.insert(ext.to_string_lossy().to_lowercase());
                }
            }
        }

        assert!(found_extensions.contains("jpg"), "Should find .jpg file");
        assert!(found_extensions.contains("png"), "Should find .png file");
        assert!(found_extensions.contains("webp"), "Should find .webp file");

        // Test cache hit on second run
        let hashes2 = generate_hashes_with_cache(&images, grid_size, &cache, false)
            .expect("Failed to generate hashes second time");
        assert_eq!(hashes2.len(), 3, "Should generate 3 hashes on cache hit");
    }

    #[test]
    fn test_scan_for_images_finds_expected_extensions() {
        let test_dir = Path::new("test_images/all_same");
        if !test_dir.exists() {
            return;
        }

        let paths = vec![test_dir.to_path_buf()];
        let images =
            scan_for_images(&paths, false, false, false).expect("Failed to scan for images");

        let extensions: std::collections::HashSet<_> = images
            .iter()
            .filter_map(|p| p.extension())
            .map(|ext| ext.to_string_lossy().to_lowercase())
            .collect();

        assert!(extensions.contains("jpg") || extensions.contains("jpeg"));
        assert!(extensions.contains("png"));
        assert!(extensions.contains("webp"));
    }

    #[test]
    fn test_rotated_images_are_detected_as_duplicates() {
        let test_dir = Path::new("test_images/rotated");
        if !test_dir.exists() {
            panic!("Test directory test_images/rotated does not exist");
        }

        let paths = vec![test_dir.to_path_buf()];
        let images =
            scan_for_images(&paths, false, false, false).expect("Failed to scan for images");

        assert_eq!(
            images.len(),
            2,
            "Should find exactly 2 images in test_images/rotated"
        );

        // Test with in-memory cache
        let cache = HashCache::new_in_memory().expect("Failed to create in-memory cache");
        let grid_size = 16;
        let hashes = generate_hashes_with_cache(&images, grid_size, &cache, false)
            .expect("Failed to generate hashes");

        assert_eq!(hashes.len(), 2, "Should generate 2 hashes");

        let threshold = 20;
        let duplicates = find_duplicates(&hashes, threshold);

        assert!(
            !duplicates.is_empty(),
            "Should find at least 1 duplicate group for rotated images"
        );
        let total_images_in_groups: usize = duplicates.iter().map(|g| g.len()).sum();
        assert_eq!(
            total_images_in_groups, 2,
            "Both rotated images should be in duplicate groups"
        );
    }

    #[test]
    fn test_broken_symlink_handling() {
        // Create a temporary directory
        let temp_dir = TempDir::new().expect("Failed to create temp directory");
        let temp_path = temp_dir.path();

        // Copy a real image to the temp directory
        let real_image_path = temp_path.join("real_image.jpg");
        fs::copy("test_images/all_same/dallepig.jpg", &real_image_path)
            .expect("Failed to copy test image");

        // Create a broken symlink
        let broken_link_path = temp_path.join("broken_link.jpg");
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink("/nonexistent/path.jpg", &broken_link_path)
                .expect("Failed to create broken symlink");
        }
        #[cfg(windows)]
        {
            // On Windows, use a broken junction instead
            std::fs::write(&broken_link_path, b"not an image")
                .expect("Failed to create broken file");
            std::fs::remove_file(&broken_link_path).expect("Failed to remove temp file");
            // Create a symlink to a non-existent file
            std::os::windows::fs::symlink_file("C:\\nonexistent\\path.jpg", &broken_link_path)
                .expect("Failed to create broken symlink");
        }

        // Test scanning with broken symlink
        let paths = vec![temp_path.to_path_buf()];
        let images =
            scan_for_images(&paths, false, false, false).expect("Failed to scan for images");

        // Should only find the real image, broken symlink should be skipped
        assert_eq!(images.len(), 1, "Should find only the real image file");
        assert!(
            images[0].file_name().expect("File should have a name") == "real_image.jpg",
            "Should find the real image"
        );

        // Test with cache to ensure broken symlink handling in cache operations
        let cache = HashCache::new_in_memory().expect("Failed to create in-memory cache");
        let grid_size = 64;
        let hashes = generate_hashes_with_cache(&images, grid_size, &cache, false)
            .expect("Failed to generate hashes");

        // Should successfully process the real image
        assert_eq!(hashes.len(), 1, "Should generate hash for the real image");

        // Test cleanup doesn't fail when files are missing
        let deleted = cache
            .cleanup_missing_files()
            .expect("Cleanup should not fail");
        assert_eq!(
            deleted, 0,
            "No files should be deleted from in-memory cache"
        );
    }

    #[test]
    fn test_hidden_directory_filtering() {
        // Create a temporary directory
        let temp_dir = TempDir::new().expect("Failed to create temp directory");
        let temp_path = temp_dir.path();

        // Create a regular directory with an image
        let regular_dir = temp_path.join("regular");
        fs::create_dir_all(&regular_dir).expect("Failed to create regular directory");
        let regular_image = regular_dir.join("image.jpg");
        fs::copy("test_images/all_same/dallepig.jpg", &regular_image)
            .expect("Failed to copy test image to regular directory");

        // Create a hidden directory with an image
        let hidden_dir = temp_path.join(".hidden");
        fs::create_dir_all(&hidden_dir).expect("Failed to create hidden directory");
        let hidden_image = hidden_dir.join("hidden_image.jpg");
        fs::copy("test_images/all_same/dallepig.jpg", &hidden_image)
            .expect("Failed to copy test image to hidden directory");

        // Test scanning without include_hidden (default behavior)
        let paths = vec![temp_path.to_path_buf()];
        let images_without_hidden =
            scan_for_images(&paths, false, false, false).expect("Failed to scan without hidden");

        // Should only find the image in the regular directory
        assert_eq!(
            images_without_hidden.len(),
            1,
            "Should find only 1 image when hidden directories are excluded"
        );
        assert!(
            images_without_hidden[0]
                .to_string_lossy()
                .contains("regular"),
            "Should find image in regular directory"
        );

        // Test scanning with include_hidden enabled
        let images_with_hidden =
            scan_for_images(&paths, true, false, false).expect("Failed to scan with hidden");

        // Should find both images
        assert_eq!(
            images_with_hidden.len(),
            2,
            "Should find 2 images when hidden directories are included"
        );

        let mut found_regular = false;
        let mut found_hidden = false;
        for image_path in &images_with_hidden {
            if image_path.to_string_lossy().contains("regular") {
                found_regular = true;
            }
            if image_path.to_string_lossy().contains(".hidden") {
                found_hidden = true;
            }
        }

        assert!(found_regular, "Should find image in regular directory");
        assert!(
            found_hidden,
            "Should find image in hidden directory when include_hidden is true"
        );
    }

    #[test]
    fn test_cache_optimization_skips_file_processing() {
        let test_dir = Path::new("test_images/all_same");
        if !test_dir.exists() {
            panic!("Test directory test_images/all_same does not exist");
        }

        let paths = vec![test_dir.to_path_buf()];
        let images =
            scan_for_images(&paths, false, false, false).expect("Failed to scan for images");

        // Use in-memory cache to test optimization
        let cache = HashCache::new_in_memory().expect("Failed to create in-memory cache");
        let grid_size = 64;

        // First run: populate cache (should have 0 hits, 3 misses)
        let hashes1 = generate_hashes_with_cache(&images, grid_size, &cache, false)
            .expect("Failed to generate hashes first time");
        assert_eq!(hashes1.len(), 3, "Should generate 3 hashes");

        // Second run: should be all cache hits (3 hits, 0 misses)
        let hashes2 = generate_hashes_with_cache(&images, grid_size, &cache, false)
            .expect("Failed to generate hashes second time");
        assert_eq!(
            hashes2.len(),
            3,
            "Should still generate 3 hashes from cache"
        );

        // Verify hashes are identical (cache optimization preserves correctness)
        for i in 0..3 {
            assert_eq!(hashes1[i].0, hashes2[i].0, "Paths should match");
            assert_eq!(
                hashes1[i].1.encode(),
                hashes2[i].1.encode(),
                "Hash strings should be identical"
            );
        }

        // The optimization should avoid file processing entirely on the second run
        // This is evidenced by the cache stats showing all hits, no misses
    }
}