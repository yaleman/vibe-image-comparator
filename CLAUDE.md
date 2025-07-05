# Vibe Image Comparator

## Project Overview
A Rust command-line tool that finds duplicate images using perceptual hashing. The tool is designed to detect images that are similar even after minor crops, edits, or rotations.

## Architecture
- **CLI**: Uses `clap` for command-line argument parsing
- **Image Processing**: Uses `image` crate for loading various image formats
- **Perceptual Hashing**: Uses `img_hash` crate with gradient-based hashing algorithm
- **File System**: Uses `walkdir` for recursive directory traversal

## Core Components

### Image Scanning (`scan_for_images`)
- Accepts both individual files and directories
- Recursively scans directories for image files
- Supports common image formats: jpg, jpeg, png, gif, bmp, tiff, tif, webp
- Follows symbolic links during traversal
- **Hidden directory filtering**: Skips directories starting with `.` by default (use `-. ` flag to include them)

### Hash Generation (`generate_hashes`)
- Uses configurable grid size for Mean-based perceptual hashing
- Default 64x64 grid, customizable via config file or CLI
- Rotation-invariant: generates hashes for all 4 rotations and selects the canonical one
- Resistant to minor edits, format changes, and rotations
- Gracefully handles unreadable images with warnings
- **Parallel processing**: File metadata calculation and image loading parallelized with rayon

### Duplicate Detection (`find_duplicates`)
- Compares hash distances using Hamming distance
- Configurable similarity threshold (default: 15)
- Groups similar images into duplicate sets
- Uses efficient processing to avoid redundant comparisons
- **Parallel processing**: Hash distance computation parallelized for better performance

## Configuration
The tool loads configuration from `~/.config/vibe-image-comparator.json` (XDG config directory).

Example config file:
```json
{
  "grid_size": 64,
  "threshold": 15,
  "database_path": "/custom/path/to/cache.db"
}
```

Configuration options:
- `grid_size`: Hash grid size (e.g., 64 for 64x64 grid) - higher values = more precision
- `threshold`: Similarity threshold (0-max, lower = more similar)
- `database_path`: Custom path for the cache database (optional, defaults to XDG cache directory)

## Usage
```bash
# Scan a single directory (uses config file settings)
cargo run -- /path/to/images

# Scan with custom threshold and grid size
cargo run -- /path/to/images --threshold 3 --grid-size 32

# Include hidden directories (starting with .)
cargo run -- /path/to/images -.

# Disable caching for one-time scans
cargo run -- /path/to/images --no-cache

# Clean up cache entries for missing files
cargo run -- --clean-cache

# Using justfile
just run /path/to/images --threshold 10 --grid-size 64
```

## Development Commands
- `just test` - Run tests
- `just lint` - Run clippy linting
- `just check` - Run both lint and test (required before completion)
- `just build` - Build release version

## Development Practices
- Use cargo commands instead of editing Cargo.toml directly
- Commit changes when a task is done

## Caching System
The tool includes a SQLite-based caching system to speed up repeated scans:
- **Normalized schema**: Separate tables for files and perceptual hashes to reduce data duplication
- **Deduplication**: Multiple files with identical content share the same perceptual hash entry
- **File integrity**: Uses SHA256 + file size to validate cached entries
- **Test isolation**: Tests use in-memory databases to avoid side effects
- **Configurable location**: Default `~/.cache/vibe-image-comparator/hashes.db` or custom path via config
- **Performance**: Significantly faster on repeat scans (cache hits vs. misses shown)
- **Maintenance**: Use `--no-cache` to disable or `--clean-cache` to remove stale entries

## Dependencies
- `clap` - CLI argument parsing
- `image` - Image loading and processing
- `img_hash` - Perceptual hashing algorithms
- `rayon` - Parallel processing for improved performance
- `rusqlite` - SQLite database for hash caching
- `sha2` - SHA256 hashing for file integrity
- `walkdir` - Directory traversal
- `anyhow` - Error handling

## Hash Algorithm Details
The tool uses a rotation-invariant Mean-based perceptual hash that:
- Resizes images to configurable grid size (default 64x64)
- Generates hashes for original and all 3 rotations (90°, 180°, 270°)
- Selects the lexicographically smallest hash for rotation invariance
- Computes mean pixel values to capture overall image characteristics
- Generates hash resistant to format changes, minor modifications, and rotations
- Higher grid sizes = more precision but larger hashes and longer processing
- Lower threshold values = more strict matching
- Higher threshold values = more lenient matching