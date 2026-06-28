#![deny(clippy::unwrap_used)]
#![deny(clippy::expect_used)]

use anyhow::Result;
use clap::Parser;
use console::style;
use dialoguer::Confirm;
use indicatif::{ProgressBar, ProgressStyle};
use std::path::PathBuf;
use std::time::Instant;
use tracing::{debug, error, warn};
use vibe_image_comparator::cache::HashCache;
use vibe_image_comparator::config::{load_config, show_config_with_overrides};
use vibe_image_comparator::hasher::{
    find_duplicates, generate_hashes_with_cache, get_duplicates_from_cache,
};
use vibe_image_comparator::scanner::scan_for_images;
use vibe_image_comparator::server;

#[derive(Parser)]
#[command(name = "vibe-image-comparator")]
#[command(about = "Find duplicate images using perceptual hashing")]
#[command(long_about = "A fast CLI tool to find duplicate or similar images using perceptual hashing.\n\
\n\
The tool computes a fingerprint for each image that is resistant to minor edits,\n\
format changes, and rotations. Images with similar fingerprints are grouped together.\n\
\n\
EXAMPLES:\n\
  # Scan a directory with default settings\n\
  vibe-image-comparator /path/to/images\n\
\n\
  # Scan with custom threshold (lower = more similar, 0-64)\n\
  vibe-image-comparator /path/to/images --threshold 10\n\
\n\
  # Scan with larger grid for more precision\n\
  vibe-image-comparator /path/to/images --grid-size 128\n\
\n\
  # Include hidden directories and show debug output\n\
  vibe-image-comparator /path/to/images -a --debug\n\
\n\
  # Scan without using the hash cache\n\
  vibe-image-comparator /path/to/images --no-cache\n\
\n\
  # Show cached duplicate matches without rescanning\n\
  vibe-image-comparator --show-matches --threshold 15\n\
\n\
  # Start web interface for browser-based duplicate management\n\
  vibe-image-comparator --server")]
struct Args {
    #[arg(help = "Paths to scan for images (files or directories)")]
    paths: Vec<PathBuf>,

    #[arg(
        short,
        long,
        help = "Similarity threshold (0-64, lower = more similar)",
        long_help = "Similarity threshold for comparing image hashes (0-64).\n\
Lower values require more similarity. 0 = identical, 64 = very permissive.\n\
Default: 15 (good balance for most use cases)"
    )]
    threshold: Option<u32>,

    #[arg(short, long, help = "Hash grid size (e.g., 64 for 64x64 grid)")]
    grid_size: Option<u32>,

    #[arg(
        long,
        help = "Remove missing files and orphaned hashes from database"
    )]
    clean_missing: bool,

    #[arg(
        long,
        help = "Completely clear all cache data (files, hashes, duplicate groups)"
    )]
    clear_cache: bool,

    #[arg(
        short = 'a',
        long,
        help = "Include hidden directories (starting with .)"
    )]
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

    #[arg(
        long,
        help = "Show duplicate matches from cache database only (no scanning)"
    )]
    show_matches: bool,

    #[arg(long, help = "Show current configuration settings")]
    show_config: bool,

    #[arg(long, help = "Start web server for browser-based interface")]
    server: bool,

    #[arg(
        long,
        help = "Port for web server (default: 8080)"
    )]
    port: Option<u16>,

    #[arg(
        long,
        help = "Disable hash caching for this scan"
    )]
    no_cache: bool,
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    let args = Args::parse();
    let config = load_config()?;

    if args.show_config {
        show_config_with_overrides(args.threshold, args.grid_size)?;
        return Ok(());
    }

    if args.server {
        let config = config.clone();
        let port = args.port.unwrap_or(8080);
        return server::start_server(config, args.threshold, args.grid_size, port).await;
    }

    let effective_config = config.with_overrides(args.grid_size, args.threshold, None);

    if args.clean_missing {
        if !Confirm::new()
            .with_prompt("Remove missing files and orphaned hashes from database?")
            .default(false)
            .interact()?
        {
            println!("Aborted.");
            return Ok(());
        }
        let cache = HashCache::new(effective_config.database_path.as_deref())?;
        let (files_removed, hashes_removed) = cache.cleanup_missing_files_and_hashes()?;
        println!(
            "✓ Cleaned up {} missing files and {} orphaned hashes",
            style(files_removed).green(),
            style(hashes_removed).green()
        );
        if args.paths.is_empty() {
            return Ok(());
        }
    }

    if args.clear_cache {
        if !Confirm::new()
            .with_prompt("Clear ALL cache data? This cannot be undone.")
            .default(false)
            .interact()?
        {
            println!("Aborted.");
            return Ok(());
        }
        let cache = HashCache::new(effective_config.database_path.as_deref())?;
        cache.clear_all_cache()?;
        println!("✓ All cache data cleared");
        if args.paths.is_empty() {
            return Ok(());
        }
    }

    if args.show_matches {
        let threshold = args.threshold.unwrap_or(effective_config.threshold);
        println!("Using threshold: {}", style(threshold).cyan());

        let cache = HashCache::new(effective_config.database_path.as_deref())?;
        let duplicates = get_duplicates_from_cache(&cache, threshold, None, None)?;

        if duplicates.is_empty() {
            println!("No duplicate images found in cache");
        } else {
            println!(
                "\nFound {} duplicate sets in cache:\n",
                style(duplicates.len()).green().bold()
            );
            for (i, group) in duplicates.iter().enumerate() {
                println!("  {} Group {}:", style("▸").cyan(), style(i + 1).bold());
                for path in group {
                    println!("    {}", style(path.display()).dim());
                }
                println!();
            }
        }
        return Ok(());
    }

    if args.paths.is_empty() {
        error!(
            "No paths provided. Use {} for help.",
            style("--help").cyan()
        );
        std::process::exit(1);
    }

    let threshold = args.threshold.unwrap_or(effective_config.threshold);
    let grid_size = args.grid_size.unwrap_or(effective_config.grid_size);

    println!(
        "\n{} {}x{}, threshold: {}",
        style("Configuration:").bold(),
        style(grid_size).cyan(),
        style(grid_size).cyan(),
        style(threshold).cyan()
    );

    let cache = if args.no_cache {
        None
    } else {
        Some(HashCache::new(effective_config.database_path.as_deref())?)
    };

    println!("\n{}", style("Scanning for images...").bold());
    let scan_start = Instant::now();
    let images = scan_for_images(
        &args.paths,
        args.include_hidden,
        args.debug,
        args.skip_validation,
        &effective_config.ignore_paths,
    )?;
    let scan_time = scan_start.elapsed();

    println!(
        "✓ Found {} images in {:.2}s",
        style(images.len()).green().bold(),
        scan_time.as_secs_f64()
    );

    if images.is_empty() {
        println!("No images to process.");
        return Ok(());
    }

    println!("\n{}", style("Generating perceptual hashes...").bold());
    let hash_start = Instant::now();

    let pb = ProgressBar::new(images.len() as u64);
    if let Ok(style) = ProgressStyle::default_bar()
        .template("{spinner:.green} [{elapsed_precise}] {bar:40.cyan/blue} {pos}/{len} ({eta}) {msg}")
    {
        pb.set_style(style.progress_chars("#>-"));
    }
    pb.set_message("hashing images");

    let hashes = if let Some(ref cache) = cache {
        generate_hashes_with_cache(&images, grid_size, cache, args.debug)?
    } else {
        generate_hashes_with_cache_no_cache(&images, grid_size, args.debug, &pb)?
    };

    pb.finish_with_message("complete");
    let hash_time = hash_start.elapsed();

    println!(
        "✓ Hashed {} images in {:.2}s ({:.1} img/s)",
        style(hashes.len()).green().bold(),
        hash_time.as_secs_f64(),
        hashes.len() as f64 / hash_time.as_secs_f64().max(0.001)
    );

    println!("\n{}", style("Finding duplicates...").bold());
    let dup_start = Instant::now();
    let duplicates = find_duplicates(&hashes, threshold);
    let dup_time = dup_start.elapsed();

    if let Some(ref cache) = cache {
        if let Err(e) = cache.store_duplicate_groups(threshold, &duplicates) {
            warn!("Failed to cache duplicate groups: {}", e);
        }
    }

    println!(
        "✓ Duplicate detection complete in {:.2}s",
        dup_time.as_secs_f64()
    );

    println!("\n{}", style("═".repeat(50)).dim());
    if duplicates.is_empty() {
        println!("{}", style("No duplicate images found").yellow().bold());
    } else {
        println!(
            "Found {} duplicate sets:\n",
            style(duplicates.len()).green().bold()
        );
        for (i, group) in duplicates.iter().enumerate() {
            println!(
                "  {} Group {} ({} files):",
                style("▸").cyan(),
                style(i + 1).bold(),
                style(group.len()).green()
            );
            for path in group {
                println!("    {}", style(path.display()).dim());
            }
            println!();
        }
    }
    println!("{}", style("═".repeat(50)).dim());

    Ok(())
}

fn generate_hashes_with_cache_no_cache(
    images: &[PathBuf],
    _grid_size: u32,
    debug: bool,
    pb: &ProgressBar,
) -> Result<Vec<(PathBuf, imghash::ImageHash)>> {
    use imghash::{perceptual::PerceptualHasher, ImageHasher};
    use rayon::prelude::*;

    let hasher = PerceptualHasher::default();

    let results: Vec<_> = images
        .par_iter()
        .map(|image_path| {
            if debug {
                debug!("Processing: {}", image_path.display());
            }

            match image::open(image_path) {
                Ok(img) => {
                    let original_hash = hasher.hash_from_img(&img).ok();
                    let rotated_90 = img.rotate90();
                    let rotated_90_hash = hasher.hash_from_img(&rotated_90).ok();
                    let rotated_180 = img.rotate180();
                    let rotated_180_hash = hasher.hash_from_img(&rotated_180).ok();
                    let rotated_270 = img.rotate270();
                    let rotated_270_hash = hasher.hash_from_img(&rotated_270).ok();

                    let mut candidates: Vec<(String, imghash::ImageHash)> = Vec::new();
                    if let Some(h) = original_hash {
                        if let Ok(enc) = h.encode() {
                            candidates.push((enc, h));
                        }
                    }
                    if let Some(h) = rotated_90_hash {
                        if let Ok(enc) = h.encode() {
                            candidates.push((enc, h));
                        }
                    }
                    if let Some(h) = rotated_180_hash {
                        if let Ok(enc) = h.encode() {
                            candidates.push((enc, h));
                        }
                    }
                    if let Some(h) = rotated_270_hash {
                        if let Ok(enc) = h.encode() {
                            candidates.push((enc, h));
                        }
                    }

                    candidates.sort_by_key(|(encoded, _)| encoded.clone());
                    let hash = candidates.into_iter().next().map(|(_, h)| h);

                    pb.inc(1);

                    match hash {
                        Some(h) => Ok((image_path.clone(), h)),
                        None => {
                            warn!("Could not generate hash for {}", image_path.display());
                            pb.inc(0);
                            Err(image_path.clone())
                        }
                    }
                }
                Err(e) => {
                    warn!("Could not open {}: {}", image_path.display(), e);
                    pb.inc(1);
                    Err(image_path.clone())
                }
            }
        })
        .collect();

    let hashes: Vec<_> = results.into_iter().flatten().collect();
    Ok(hashes)
}
