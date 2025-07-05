# Vibe Image Comparator

A fast Rust CLI tool for finding duplicate images using rotation-invariant perceptual hashing with intelligent SQLite caching.

## Features

- **Rotation-Invariant Detection**: Finds duplicates even when images are rotated (90°, 180°, 270°)
- **Format Agnostic**: Works across different image formats (JPEG, PNG, WebP, GIF, BMP, TIFF)
- **Intelligent Caching**: SQLite-based cache with normalized schema for optimal performance
- **Configurable Similarity**: Adjustable thresholds and hash grid sizes for different use cases
- **Fast Performance**: Parallel processing and efficient caching for large image collections
- **Data Deduplication**: Multiple files with identical content share cache entries

## Quick Start

```bash
# Find duplicates in a directory
cargo run -- /path/to/images

# Find duplicates with custom sensitivity
cargo run -- /path/to/images --threshold 10

# Include hidden directories
cargo run -- /path/to/images -.

# Clean up stale cache entries
cargo run -- --clean-cache
```

## Installation

### Prerequisites

- Rust 1.70+ (install via [rustup](https://rustup.rs/))
- Git

### From Source

```bash
git clone https://github.com/yaleman/vibe-image-comparator.git
cd vibe-image-comparator
cargo build --release
```

The binary will be available at `target/release/vibe-image-comparator`.

### Using Cargo

```bash
cargo install --git https://github.com/yaleman/vibe-image-comparator.git
```

## Usage

### Basic Commands

```bash
# Scan a single directory
vibe-image-comparator /path/to/photos

# Scan multiple directories
vibe-image-comparator /path/to/photos /another/path

# Scan with custom threshold (lower = more strict)
vibe-image-comparator /path/to/photos --threshold 5

# Use higher precision hashing
vibe-image-comparator /path/to/photos --grid-size 64

# Disable caching for one-time scans
vibe-image-comparator /path/to/photos --no-cache

# Include hidden directories (starting with .)
vibe-image-comparator /path/to/photos -.
```

### Cache Management

```bash
# Clean up cache entries for missing files
vibe-image-comparator --clean-cache

# Combine with scanning
vibe-image-comparator --clean-cache /path/to/photos
```

## Configuration

Create a configuration file at `~/.config/vibe-image-comparator.json`:

```json
{
  "grid_size": 16,
  "threshold": 5,
  "database_path": "/custom/path/to/cache.db"
}
```

### Configuration Options

| Option | Default | Description |
|--------|---------|-------------|
| `grid_size` | 16 | Hash grid size (16x16). Higher values = more precision |
| `threshold` | 5 | Similarity threshold (0-64). Lower = more strict matching |
| `database_path` | Auto | Custom cache database location (optional) |

## How It Works

### Perceptual Hashing

The tool uses a rotation-invariant Mean-based perceptual hash algorithm:

1. **Image Loading**: Supports common formats via the `image` crate
2. **Rotation Generation**: Creates hashes for 0°, 90°, 180°, 270° rotations
3. **Canonical Selection**: Chooses lexicographically smallest hash for consistency
4. **Similarity Comparison**: Uses Hamming distance for duplicate detection

### Caching System

Intelligent SQLite-based caching with normalized schema:

- **Files Table**: Stores file paths, sizes, and references to perceptual hashes
- **Perceptual Hashes Table**: Stores unique SHA256 → perceptual hash mappings
- **Deduplication**: Multiple files with identical content share hash entries
- **Validation**: Uses SHA256 + file size to ensure cache validity
- **Performance**: Dramatically faster repeat scans (cache hits vs misses shown)

### Supported Formats

- JPEG (.jpg, .jpeg)
- PNG (.png)
- WebP (.webp)
- GIF (.gif)
- BMP (.bmp)
- TIFF (.tiff, .tif)

## Performance

### Benchmarks

Typical performance on modern hardware:

- **First scan**: ~50-100 images/second (depends on image size)
- **Cached scan**: ~500-1000 images/second (cache hits)
- **Memory usage**: ~10-50MB for typical collections

### Optimization Tips

1. **Use caching**: Leave caching enabled for repeated scans
2. **Adjust grid size**: Higher values (32, 64) for more precision, lower (8, 16) for speed
3. **Tune threshold**: Start with default (5), increase for more matches
4. **Clean cache**: Run `--clean-cache` periodically to remove stale entries

## Examples

### Finding Exact Duplicates

```bash
# Very strict matching
vibe-image-comparator /photos --threshold 1
```

### Finding Similar Images

```bash
# More lenient matching for edited images
vibe-image-comparator /photos --threshold 15
```

### High Precision Scanning

```bash
# Use larger hash grid for better discrimination
vibe-image-comparator /photos --grid-size 32 --threshold 8
```

### Scanning Large Collections

```bash
# Optimize for large collections with caching
vibe-image-comparator /large-photo-collection --threshold 10
```

## Troubleshooting

### Common Issues

**"No duplicate images found" when duplicates exist**
- Try increasing the threshold: `--threshold 15`
- Check if images are significantly different in quality
- Verify image formats are supported

**Slow performance on first scan**
- This is normal - subsequent scans will be much faster due to caching
- Consider using a smaller grid size for faster processing: `--grid-size 8`

**Cache taking up too much space**
- Run `--clean-cache` to remove entries for deleted files
- Consider using `--no-cache` for one-time scans

**Images not detected after rotation**
- The tool should handle 90° rotations automatically
- For other transformations, try increasing the threshold

### Debug Information

The tool provides helpful output:
- Cache hit/miss statistics
- Number of images found and processed
- Duplicate group information

## Development

### Building

```bash
# Debug build
cargo build

# Release build
cargo build --release

# Run tests
cargo test

# Run with justfile
just check  # lint + test
just run /path/to/images
```

### Project Structure

```
├── src/
│   └── main.rs          # Main application code
├── test_images/         # Test image samples
│   ├── all_same/        # Identical images in different formats
│   └── rotated/         # Rotated image pairs
├── CLAUDE.md           # Development documentation
├── Cargo.toml          # Rust dependencies
├── justfile           # Build automation
└── README.md          # This file
```

## Contributing

1. Fork the repository
2. Create a feature branch
3. Make your changes
4. Run `cargo test` and `cargo clippy`
5. Submit a pull request

## License

This project is licensed under the MIT License - see the LICENSE file for details.

## Acknowledgments

- Built with [Rust](https://www.rust-lang.org/)
- Uses [img_hash](https://github.com/abonander/img_hash) for perceptual hashing
- Image processing via [image](https://github.com/image-rs/image)
- SQLite caching with [rusqlite](https://github.com/rusqlite/rusqlite)