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
use hasher::{find_duplicates, generate_hashes_with_cache, get_duplicates_from_cache};
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

    #[arg(
        long,
        help = "Show duplicate matches from cache database only (no scanning)"
    )]
    show_matches: bool,
}

fn main() -> Result<()> {
    let args = Args::parse();

    let config = load_config()?;

    let cache = HashCache::new(config.database_path.as_deref())?;

    if args.clean_cache {
        let deleted = cache.cleanup_missing_files()?;
        println!("Cleaned up {deleted} entries from cache");
        if args.paths.is_empty() {
            return Ok(());
        }
    }

    // Handle show_matches flag - only show cached duplicates
    if args.show_matches {
        let threshold = args.threshold.unwrap_or(config.threshold);
        println!("Using threshold: {threshold}");
        println!("Hash caching enabled");
        
        let duplicates = get_duplicates_from_cache(&cache, threshold)?;
        
        if duplicates.is_empty() {
            println!("No duplicate images found in cache");
        } else {
            println!("Found {} duplicate sets in cache:", duplicates.len());
            for (i, group) in duplicates.iter().enumerate() {
                println!("  Group {}:", i + 1);
                for path in group {
                    println!("    {}", path.display());
                }
            }
        }
        
        return Ok(());
    }

    if args.paths.is_empty() {
        eprintln!("Error: Please provide at least one path to scan");
        std::process::exit(1);
    }

    let threshold = args.threshold.unwrap_or(config.threshold);
    let grid_size = args.grid_size.unwrap_or(config.grid_size);

    println!("Using grid size: {grid_size}x{grid_size}, threshold: {threshold}");
    println!("Hash caching enabled");

    println!("Scanning paths for images...");
    let images = scan_for_images(
        &args.paths,
        args.include_hidden,
        args.debug,
        args.skip_validation,
    )?;

    println!("Found {} images", images.len());
    println!("Generating perceptual hashes...");

    let hashes = generate_hashes_with_cache(&images, grid_size, &cache, args.debug)?;

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
