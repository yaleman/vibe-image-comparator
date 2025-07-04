use anyhow::Result;
use clap::Parser;
use img_hash::{HasherConfig, HashAlg};
use rusqlite::{Connection, params};
use serde::{Deserialize, Serialize};
use sha2::{Sha256, Digest};
use std::fs;
use std::path::{Path, PathBuf};
use walkdir::WalkDir;

#[derive(Debug, Serialize, Deserialize)]
struct Config {
    grid_size: u32,
    threshold: u32,
    database_path: Option<String>,
}

#[derive(Debug)]
struct FileMetadata {
    path: PathBuf,
    size: u64,
    sha256: String,
    perceptual_hash: Vec<u8>,
}

struct HashCache {
    conn: Connection,
}

impl HashCache {
    fn new(database_path: Option<&str>) -> Result<Self> {
        let conn = if let Some(path) = database_path {
            Connection::open(path)?
        } else {
            let cache_dir = dirs::cache_dir()
                .unwrap_or_else(|| PathBuf::from("."))
                .join("vibe-image-comparator");
            
            fs::create_dir_all(&cache_dir)?;
            let db_path = cache_dir.join("hashes.db");
            Connection::open(db_path)?
        };
        
        Self::create_tables(&conn)?;
        Self::migrate_old_schema(&conn)?;
        
        Ok(HashCache { conn })
    }
    
    #[cfg(test)]
    fn new_in_memory() -> Result<Self> {
        let conn = Connection::open(":memory:")?;
        Self::create_tables(&conn)?;
        Ok(HashCache { conn })
    }
    
    fn create_tables(conn: &Connection) -> Result<()> {
        // Create perceptual_hashes table first (referenced table)
        conn.execute(
            "CREATE TABLE IF NOT EXISTS perceptual_hashes (
                id INTEGER PRIMARY KEY,
                sha256 TEXT UNIQUE NOT NULL,
                perceptual_hash BLOB NOT NULL,
                created_at DATETIME DEFAULT CURRENT_TIMESTAMP
            )",
            [],
        )?;
        
        // Create files table with foreign key reference
        conn.execute(
            "CREATE TABLE IF NOT EXISTS files (
                id INTEGER PRIMARY KEY,
                path TEXT UNIQUE NOT NULL,
                size INTEGER NOT NULL,
                perceptual_hash_id INTEGER NOT NULL,
                created_at DATETIME DEFAULT CURRENT_TIMESTAMP,
                FOREIGN KEY (perceptual_hash_id) REFERENCES perceptual_hashes(id)
            )",
            [],
        )?;
        
        // Enable foreign key constraints
        conn.execute("PRAGMA foreign_keys = ON", [])?;
        
        Ok(())
    }
    
    fn migrate_old_schema(conn: &Connection) -> Result<()> {
        // Check if old table exists
        let mut stmt = conn.prepare(
            "SELECT name FROM sqlite_master WHERE type='table' AND name='file_hashes'"
        )?;
        let old_table_exists = stmt.exists([])?;
        
        if old_table_exists {
            println!("Migrating existing cache data to normalized schema...");
            
            // Read all data from old table
            let mut stmt = conn.prepare(
                "SELECT path, size, sha256, perceptual_hash FROM file_hashes"
            )?;
            let old_data: Vec<(String, i64, String, Vec<u8>)> = stmt.query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, i64>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, Vec<u8>>(3)?,
                ))
            })?.collect::<Result<Vec<_>, _>>()?;
            
            // Insert into new normalized tables
            for (path, size, sha256, perceptual_hash) in old_data {
                // Insert or get perceptual hash ID
                conn.execute(
                    "INSERT OR IGNORE INTO perceptual_hashes (sha256, perceptual_hash) VALUES (?1, ?2)",
                    params![sha256, perceptual_hash],
                )?;
                
                let perceptual_hash_id: i64 = conn.query_row(
                    "SELECT id FROM perceptual_hashes WHERE sha256 = ?1",
                    params![sha256],
                    |row| row.get(0),
                )?;
                
                // Insert file record
                conn.execute(
                    "INSERT OR IGNORE INTO files (path, size, perceptual_hash_id) VALUES (?1, ?2, ?3)",
                    params![path, size, perceptual_hash_id],
                )?;
            }
            
            // Drop old table
            conn.execute("DROP TABLE file_hashes", [])?;
            println!("Migration completed successfully");
        }
        
        Ok(())
    }
    
    fn get_cached_hash(&self, path: &Path, size: u64, sha256: &str) -> Result<Option<Vec<u8>>> {
        let mut stmt = self.conn.prepare(
            "SELECT ph.perceptual_hash 
             FROM files f 
             JOIN perceptual_hashes ph ON f.perceptual_hash_id = ph.id 
             WHERE f.path = ?1 AND f.size = ?2 AND ph.sha256 = ?3"
        )?;
        
        let mut rows = stmt.query_map(params![path.to_string_lossy(), size, sha256], |row| {
            row.get::<_, Vec<u8>>(0)
        })?;
        
        if let Some(row) = rows.next() {
            Ok(Some(row?))
        } else {
            Ok(None)
        }
    }
    
    fn store_hash(&self, metadata: &FileMetadata) -> Result<()> {
        // Insert or get perceptual hash ID
        self.conn.execute(
            "INSERT OR IGNORE INTO perceptual_hashes (sha256, perceptual_hash) VALUES (?1, ?2)",
            params![metadata.sha256, metadata.perceptual_hash],
        )?;
        
        let perceptual_hash_id: i64 = self.conn.query_row(
            "SELECT id FROM perceptual_hashes WHERE sha256 = ?1",
            params![metadata.sha256],
            |row| row.get(0),
        )?;
        
        // Insert or replace file record
        self.conn.execute(
            "INSERT OR REPLACE INTO files (path, size, perceptual_hash_id) VALUES (?1, ?2, ?3)",
            params![
                metadata.path.to_string_lossy(),
                metadata.size,
                perceptual_hash_id
            ],
        )?;
        
        Ok(())
    }
    
    fn cleanup_missing_files(&self) -> Result<usize> {
        let mut stmt = self.conn.prepare("SELECT path FROM files")?;
        let paths: Vec<String> = stmt.query_map([], |row| {
            row.get::<_, String>(0)
        })?.collect::<Result<Vec<_>, _>>()?;
        
        let mut deleted = 0;
        for path_str in paths {
            let path = PathBuf::from(&path_str);
            if !path.exists() {
                self.conn.execute("DELETE FROM files WHERE path = ?1", params![path_str])?;
                deleted += 1;
            }
        }
        
        // Clean up orphaned perceptual hashes (not referenced by any files)
        let orphaned = self.conn.execute(
            "DELETE FROM perceptual_hashes 
             WHERE id NOT IN (SELECT DISTINCT perceptual_hash_id FROM files)",
            [],
        )?;
        
        if orphaned > 0 {
            println!("Cleaned up {orphaned} orphaned perceptual hashes");
        }
        
        Ok(deleted)
    }
    
    #[allow(dead_code)]
    fn debug_tables(&self) -> Result<()> {
        println!("\n=== Database Debug Info ===");
        
        // Count files
        let file_count: i64 = self.conn.query_row(
            "SELECT COUNT(*) FROM files",
            [],
            |row| row.get(0),
        )?;
        println!("Files table: {file_count} entries");
        
        // Count perceptual hashes
        let hash_count: i64 = self.conn.query_row(
            "SELECT COUNT(*) FROM perceptual_hashes",
            [],
            |row| row.get(0),
        )?;
        println!("Perceptual hashes table: {hash_count} entries");
        
        // Show deduplication ratio
        if file_count > 0 && hash_count > 0 {
            let ratio = hash_count as f64 / file_count as f64;
            println!("Deduplication ratio: {:.2} (lower = more deduplication)", ratio);
        }
        
        println!("=== End Debug Info ===\n");
        Ok(())
    }
}

fn calculate_file_sha256(path: &Path) -> Result<String> {
    let data = fs::read(path)?;
    let mut hasher = Sha256::new();
    hasher.update(&data);
    let result = hasher.finalize();
    Ok(format!("{result:x}"))
}

fn get_file_metadata(path: &Path) -> Result<(u64, String)> {
    let metadata = fs::metadata(path)?;
    let size = metadata.len();
    let sha256 = calculate_file_sha256(path)?;
    Ok((size, sha256))
}

impl Default for Config {
    fn default() -> Self {
        Self {
            grid_size: 16,
            threshold: 5,
            database_path: None,
        }
    }
}

#[derive(Parser)]
#[command(name = "vibe-image-comparator")]
#[command(about = "A tool to find duplicate images using perceptual hashing")]
struct Args {
    #[arg(help = "Paths to scan for images")]
    paths: Vec<PathBuf>,
    
    #[arg(short, long, help = "Minimum similarity threshold (0-64, lower = more similar)")]
    threshold: Option<u32>,
    
    #[arg(short, long, help = "Hash grid size (e.g., 64 for 64x64 grid)")]
    grid_size: Option<u32>,
    
    #[arg(long, help = "Disable hash caching")]
    no_cache: bool,
    
    #[arg(long, help = "Clean up cache entries for missing files")]
    clean_cache: bool,
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
    let images = scan_for_images(&args.paths)?;
    
    println!("Found {} images", images.len());
    println!("Generating perceptual hashes...");
    
    let hashes = if let Some(ref cache) = cache {
        generate_hashes_with_cache(&images, grid_size, cache)?
    } else {
        generate_hashes(&images, grid_size)?
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

fn load_config() -> Result<Config> {
    let config_dir = dirs::config_dir()
        .ok_or_else(|| anyhow::anyhow!("Could not find config directory"))?;
    
    let config_path = config_dir.join("vibe-image-comparator.json");
    
    if config_path.exists() {
        let config_str = std::fs::read_to_string(&config_path)?;
        let config: Config = serde_json::from_str(&config_str)?;
        println!("Loaded config from: {}", config_path.display());
        Ok(config)
    } else {
        Ok(Config::default())
    }
}

fn scan_for_images(paths: &[PathBuf]) -> Result<Vec<PathBuf>> {
    let mut images = Vec::new();
    let image_extensions = ["jpg", "jpeg", "png", "gif", "bmp", "tiff", "tif", "webp"];
    
    for path in paths {
        if path.is_file() {
            if let Some(ext) = path.extension() {
                if image_extensions.contains(&ext.to_string_lossy().to_lowercase().as_str()) {
                    images.push(path.clone());
                }
            }
        } else if path.is_dir() {
            for entry in WalkDir::new(path).follow_links(true) {
                let entry = entry?;
                let path = entry.path();
                
                if path.is_file() {
                    if let Some(ext) = path.extension() {
                        if image_extensions.contains(&ext.to_string_lossy().to_lowercase().as_str()) {
                            images.push(path.to_path_buf());
                        }
                    }
                }
            }
        }
    }
    
    Ok(images)
}

fn generate_hashes_with_cache(images: &[PathBuf], grid_size: u32, cache: &HashCache) -> Result<Vec<(PathBuf, img_hash::ImageHash)>> {
    let hasher = HasherConfig::new()
        .hash_size(grid_size, grid_size)
        .hash_alg(HashAlg::Mean)
        .to_hasher();
    
    let mut hashes = Vec::new();
    let mut cache_hits = 0;
    let mut cache_misses = 0;
    
    for image_path in images {
        match get_file_metadata(image_path) {
            Ok((size, sha256)) => {
                if let Ok(Some(cached_hash_bytes)) = cache.get_cached_hash(image_path, size, &sha256) {
                    match img_hash::ImageHash::from_bytes(&cached_hash_bytes) {
                        Ok(hash) => {
                            hashes.push((image_path.clone(), hash));
                            cache_hits += 1;
                        }
                        Err(e) => {
                            eprintln!("Warning: Invalid cached hash for {}: {:?}", image_path.display(), e);
                            match image::open(image_path) {
                                Ok(img) => {
                                    let hash = generate_rotation_invariant_hash(&hasher, &img);
                                    let metadata = FileMetadata {
                                        path: image_path.clone(),
                                        size,
                                        sha256,
                                        perceptual_hash: hash.as_bytes().to_vec(),
                                    };
                                    
                                    if let Err(e) = cache.store_hash(&metadata) {
                                        eprintln!("Warning: Could not cache hash for {}: {}", image_path.display(), e);
                                    }
                                    
                                    hashes.push((image_path.clone(), hash));
                                    cache_misses += 1;
                                }
                                Err(e) => {
                                    eprintln!("Warning: Could not process {}: {}", image_path.display(), e);
                                }
                            }
                        }
                    }
                } else {
                    match image::open(image_path) {
                        Ok(img) => {
                            let hash = generate_rotation_invariant_hash(&hasher, &img);
                            let metadata = FileMetadata {
                                path: image_path.clone(),
                                size,
                                sha256,
                                perceptual_hash: hash.as_bytes().to_vec(),
                            };
                            
                            if let Err(e) = cache.store_hash(&metadata) {
                                eprintln!("Warning: Could not cache hash for {}: {}", image_path.display(), e);
                            }
                            
                            hashes.push((image_path.clone(), hash));
                            cache_misses += 1;
                        }
                        Err(e) => {
                            eprintln!("Warning: Could not process {}: {}", image_path.display(), e);
                        }
                    }
                }
            }
            Err(e) => {
                eprintln!("Warning: Could not get metadata for {}: {}", image_path.display(), e);
            }
        }
    }
    
    if cache_hits > 0 || cache_misses > 0 {
        println!("Cache stats: {cache_hits} hits, {cache_misses} misses");
    }
    
    Ok(hashes)
}

fn generate_hashes(images: &[PathBuf], grid_size: u32) -> Result<Vec<(PathBuf, img_hash::ImageHash)>> {
    let hasher = HasherConfig::new()
        .hash_size(grid_size, grid_size)
        .hash_alg(HashAlg::Mean)
        .to_hasher();
    
    let mut hashes = Vec::new();
    
    for image_path in images {
        match image::open(image_path) {
            Ok(img) => {
                let hash = generate_rotation_invariant_hash(&hasher, &img);
                hashes.push((image_path.clone(), hash));
            }
            Err(e) => {
                eprintln!("Warning: Could not process {}: {}", image_path.display(), e);
            }
        }
    }
    
    Ok(hashes)
}

fn generate_rotation_invariant_hash(hasher: &img_hash::Hasher<Box<[u8]>>, img: &image::DynamicImage) -> img_hash::ImageHash<Box<[u8]>> {
    let original_hash = hasher.hash_image(img);
    let rotated_90 = img.rotate90();
    let rotated_90_hash = hasher.hash_image(&rotated_90);
    let rotated_180 = img.rotate180();
    let rotated_180_hash = hasher.hash_image(&rotated_180);
    let rotated_270 = img.rotate270();
    let rotated_270_hash = hasher.hash_image(&rotated_270);
    
    let mut candidates = vec![
        (original_hash.as_bytes().to_vec(), original_hash),
        (rotated_90_hash.as_bytes().to_vec(), rotated_90_hash),
        (rotated_180_hash.as_bytes().to_vec(), rotated_180_hash),
        (rotated_270_hash.as_bytes().to_vec(), rotated_270_hash),
    ];
    
    candidates.sort_by_key(|(bytes, _)| bytes.clone());
    candidates.into_iter().next().unwrap().1
}

fn find_duplicates(hashes: &[(PathBuf, img_hash::ImageHash)], threshold: u32) -> Vec<Vec<PathBuf>> {
    let mut groups: Vec<Vec<PathBuf>> = Vec::new();
    let mut processed = vec![false; hashes.len()];
    
    for (i, (path1, hash1)) in hashes.iter().enumerate() {
        if processed[i] {
            continue;
        }
        
        let mut group = vec![path1.clone()];
        processed[i] = true;
        
        for (j, (path2, hash2)) in hashes.iter().enumerate().skip(i + 1) {
            if processed[j] {
                continue;
            }
            
            let distance = hash1.dist(hash2);
            if distance <= threshold {
                group.push(path2.clone());
                processed[j] = true;
            }
        }
        
        if group.len() > 1 {
            groups.push(group);
        }
    }
    
    groups
}


#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    #[test]
    fn test_all_same_directory_finds_three_duplicates() {
        let test_dir = Path::new("test_images/all_same");
        if !test_dir.exists() {
            panic!("Test directory test_images/all_same does not exist");
        }

        let paths = vec![test_dir.to_path_buf()];
        let images = scan_for_images(&paths).expect("Failed to scan for images");
        
        assert_eq!(images.len(), 3, "Should find exactly 3 images in test_images/all_same");
        
        // Test with in-memory cache
        let cache = HashCache::new_in_memory().expect("Failed to create in-memory cache");
        let grid_size = 16;
        let hashes = generate_hashes_with_cache(&images, grid_size, &cache).expect("Failed to generate hashes");
        
        assert_eq!(hashes.len(), 3, "Should generate 3 hashes");
        
        let threshold = 15;
        let duplicates = find_duplicates(&hashes, threshold);
        
        assert!(!duplicates.is_empty(), "Should find at least 1 duplicate group");
        let total_images_in_groups: usize = duplicates.iter().map(|g| g.len()).sum();
        assert_eq!(total_images_in_groups, 3, "All 3 images should be in duplicate groups");
        
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
        let hashes2 = generate_hashes_with_cache(&images, grid_size, &cache).expect("Failed to generate hashes second time");
        assert_eq!(hashes2.len(), 3, "Should generate 3 hashes on cache hit");
    }

    #[test]
    fn test_scan_for_images_finds_expected_extensions() {
        let test_dir = Path::new("test_images/all_same");
        if !test_dir.exists() {
            return;
        }

        let paths = vec![test_dir.to_path_buf()];
        let images = scan_for_images(&paths).expect("Failed to scan for images");
        
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
        let images = scan_for_images(&paths).expect("Failed to scan for images");
        
        assert_eq!(images.len(), 2, "Should find exactly 2 images in test_images/rotated");
        
        // Test with in-memory cache
        let cache = HashCache::new_in_memory().expect("Failed to create in-memory cache");
        let grid_size = 16;
        let hashes = generate_hashes_with_cache(&images, grid_size, &cache).expect("Failed to generate hashes");
        
        assert_eq!(hashes.len(), 2, "Should generate 2 hashes");
        
        let threshold = 20;
        let duplicates = find_duplicates(&hashes, threshold);
        
        assert!(!duplicates.is_empty(), "Should find at least 1 duplicate group for rotated images");
        let total_images_in_groups: usize = duplicates.iter().map(|g| g.len()).sum();
        assert_eq!(total_images_in_groups, 2, "Both rotated images should be in duplicate groups");
    }
}
