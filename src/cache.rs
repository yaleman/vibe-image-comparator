use anyhow::Result;
use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::fs;
use std::path::{Path, PathBuf};
use tracing::{info, debug};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    pub grid_size: u32,
    pub threshold: u32,
    pub database_path: Option<String>,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            grid_size: 64,
            threshold: 15,
            database_path: None,
        }
    }
}

#[derive(Debug)]
pub struct FileMetadata {
    pub path: PathBuf,
    pub size: u64,
    pub sha256: String,
    pub perceptual_hash: String,
}

pub struct HashCache {
    conn: Connection,
}

impl HashCache {
    pub fn new(database_path: Option<&str>) -> Result<Self> {
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
        Self::migrate_blob_to_text(&conn)?;

        Ok(HashCache { conn })
    }

    #[cfg(test)]
    #[allow(dead_code)]
    pub fn new_in_memory() -> Result<Self> {
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
                perceptual_hash TEXT NOT NULL,
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

        // Create duplicate groups table for caching computed duplicates
        conn.execute(
            "CREATE TABLE IF NOT EXISTS duplicate_groups (
                id INTEGER PRIMARY KEY,
                threshold INTEGER NOT NULL,
                group_hash TEXT NOT NULL,
                created_at DATETIME DEFAULT CURRENT_TIMESTAMP
            )",
            [],
        )?;

        // Create junction table linking files to duplicate groups
        conn.execute(
            "CREATE TABLE IF NOT EXISTS duplicate_group_files (
                group_id INTEGER NOT NULL,
                file_path TEXT NOT NULL,
                PRIMARY KEY (group_id, file_path),
                FOREIGN KEY (group_id) REFERENCES duplicate_groups(id) ON DELETE CASCADE
            )",
            [],
        )?;

        // Enable foreign key constraints
        conn.execute("PRAGMA foreign_keys = ON", [])?;

        Ok(())
    }

    fn migrate_old_schema(conn: &Connection) -> Result<()> {
        // Check if old table exists
        let mut stmt = conn
            .prepare("SELECT name FROM sqlite_master WHERE type='table' AND name='file_hashes'")?;
        let old_table_exists = stmt.exists([])?;

        if old_table_exists {
            info!("Migrating existing cache data to normalized schema...");

            // Read all data from old table
            let mut stmt =
                conn.prepare("SELECT path, size, sha256, perceptual_hash FROM file_hashes")?;
            let old_data: Vec<(String, i64, String, Vec<u8>)> = stmt
                .query_map([], |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, i64>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, Vec<u8>>(3)?,
                    ))
                })?
                .collect::<Result<Vec<_>, _>>()?;

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
            info!("Migration completed successfully");
        }

        Ok(())
    }

    pub fn get_cached_hash(&self, path: &Path, size: u64, sha256: &str) -> Result<Option<String>> {
        let mut stmt = self.conn.prepare(
            "SELECT ph.perceptual_hash 
             FROM files f 
             JOIN perceptual_hashes ph ON f.perceptual_hash_id = ph.id 
             WHERE f.path = ?1 AND f.size = ?2 AND ph.sha256 = ?3",
        )?;

        let mut rows = stmt.query_map(params![path.to_string_lossy(), size, sha256], |row| {
            row.get::<_, String>(0)
        })?;

        if let Some(row) = rows.next() {
            Ok(Some(row?))
        } else {
            Ok(None)
        }
    }

    pub fn store_hash(&self, metadata: &FileMetadata) -> Result<()> {
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

    pub fn cleanup_missing_files(&self) -> Result<usize> {
        let mut stmt = self.conn.prepare("SELECT path FROM files")?;
        let paths: Vec<String> = stmt
            .query_map([], |row| row.get::<_, String>(0))?
            .collect::<Result<Vec<_>, _>>()?;

        let mut deleted = 0;
        for path_str in paths {
            let path = PathBuf::from(&path_str);
            if !path.exists() {
                self.conn
                    .execute("DELETE FROM files WHERE path = ?1", params![path_str])?;
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
            info!("Cleaned up {orphaned} orphaned perceptual hashes");
        }

        Ok(deleted)
    }

    pub fn cleanup_missing_files_and_hashes(&self) -> Result<(usize, usize)> {
        info!("Scanning database for missing files...");
        
        // Get all file paths from database
        let mut stmt = self.conn.prepare("SELECT path FROM files")?;
        let paths: Vec<String> = stmt
            .query_map([], |row| row.get::<_, String>(0))?
            .collect::<Result<Vec<_>, _>>()?;
        
        let total_files = paths.len();
        let mut files_removed = 0;
        
        // Check each file and collect missing ones
        let mut missing_paths = Vec::new();
        for (i, path_str) in paths.iter().enumerate() {
            if i % 100 == 0 {
                debug!("Checked {i}/{total_files} files...");
            }
            
            let path = PathBuf::from(path_str);
            if !path.exists() {
                missing_paths.push(path_str);
            }
        }
        
        info!("Found {} missing files out of {} total", missing_paths.len(), total_files);
        
        if missing_paths.is_empty() {
            return Ok((0, 0));
        }
        
        // Remove missing files from database
        info!("Removing missing files from database...");
        let tx = self.conn.unchecked_transaction()?;
        
        for path_str in missing_paths {
            tx.execute(
                "DELETE FROM files WHERE path = ?1",
                params![path_str],
            )?;
            files_removed += 1;
        }
        
        // Clean up orphaned perceptual hashes
        info!("Cleaning up orphaned hashes...");
        let hashes_removed = tx.execute(
            "DELETE FROM perceptual_hashes 
             WHERE id NOT IN (SELECT DISTINCT perceptual_hash_id FROM files)",
            [],
        )?;
        
        tx.commit()?;
        
        // Clear cached duplicate groups since file cache has changed
        self.clear_duplicate_groups_cache()?;
        
        info!("Database cleanup completed successfully");
        Ok((files_removed, hashes_removed))
    }

    pub fn remove_file_entry(&self, path: &Path) -> Result<()> {
        self.conn.execute(
            "DELETE FROM files WHERE path = ?1",
            params![path.to_string_lossy()],
        )?;

        // Clean up orphaned perceptual hashes after removing the file
        let orphaned = self.conn.execute(
            "DELETE FROM perceptual_hashes 
             WHERE id NOT IN (SELECT DISTINCT perceptual_hash_id FROM files)",
            [],
        )?;

        if orphaned > 0 {
            info!(
                "Cleaned up {orphaned} orphaned perceptual hashes after removing broken file"
            );
        }

        // Clear cached duplicate groups since file cache has changed
        self.clear_duplicate_groups_cache()?;

        Ok(())
    }

    fn migrate_blob_to_text(conn: &Connection) -> Result<()> {
        // Check if perceptual_hashes table has BLOB column
        let mut stmt = conn.prepare("PRAGMA table_info(perceptual_hashes)")?;
        let column_info: Vec<(String, String)> = stmt
            .query_map([], |row| {
                Ok((
                    row.get::<_, String>(1)?, // column name
                    row.get::<_, String>(2)?, // column type
                ))
            })?
            .collect::<Result<Vec<_>, _>>()?;

        // Check if perceptual_hash column is BLOB
        let needs_migration = column_info
            .iter()
            .any(|(name, col_type)| name == "perceptual_hash" && col_type.to_uppercase() == "BLOB");

        if needs_migration {
            info!("Migrating cache schema from BLOB to TEXT...");
            
            // Drop the old tables - this will lose cached data but ensures compatibility
            conn.execute("DROP TABLE IF EXISTS files", [])?;
            conn.execute("DROP TABLE IF EXISTS perceptual_hashes", [])?;
            
            // Recreate tables with correct schema
            Self::create_tables(conn)?;
            
            info!("Cache schema migration completed");
        }

        Ok(())
    }

    pub fn get_all_cached_hashes(&self) -> Result<Vec<(PathBuf, String)>> {
        let mut stmt = self.conn.prepare(
            "SELECT f.path, ph.perceptual_hash 
             FROM files f 
             JOIN perceptual_hashes ph ON f.perceptual_hash_id = ph.id
             WHERE EXISTS (SELECT 1 FROM files WHERE path = f.path)"
        )?;

        let rows = stmt.query_map([], |row| {
            Ok((
                PathBuf::from(row.get::<_, String>(0)?),
                row.get::<_, String>(1)?,
            ))
        })?;

        let mut results = Vec::new();
        for row in rows {
            results.push(row?);
        }

        Ok(results)
    }

    #[allow(dead_code)]
    pub fn debug_tables(&self) -> Result<()> {
        println!("\n=== Database Debug Info ===");

        // Count files
        let file_count: i64 = self
            .conn
            .query_row("SELECT COUNT(*) FROM files", [], |row| row.get(0))?;
        println!("Files table: {file_count} entries");

        // Count perceptual hashes
        let hash_count: i64 =
            self.conn
                .query_row("SELECT COUNT(*) FROM perceptual_hashes", [], |row| {
                    row.get(0)
                })?;
        println!("Perceptual hashes table: {hash_count} entries");

        // Show deduplication ratio
        if file_count > 0 && hash_count > 0 {
            let ratio = hash_count as f64 / file_count as f64;
            println!("Deduplication ratio: {ratio:.2} (lower = more deduplication)");
        }

        println!("=== End Debug Info ===\n");
        Ok(())
    }

    /// Generate a hash for the current set of cached files to detect changes
    fn generate_cache_state_hash(&self) -> Result<String> {
        let mut stmt = self.conn.prepare(
            "SELECT f.path, ph.perceptual_hash 
             FROM files f 
             JOIN perceptual_hashes ph ON f.perceptual_hash_id = ph.id 
             ORDER BY f.path"
        )?;

        let rows = stmt.query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
            ))
        })?;

        let mut hasher = Sha256::new();
        for row in rows {
            let (path, hash) = row?;
            hasher.update(path.as_bytes());
            hasher.update(hash.as_bytes());
        }

        Ok(format!("{:x}", hasher.finalize()))
    }

    /// Store duplicate groups for a given threshold
    pub fn store_duplicate_groups(&self, threshold: u32, duplicates: &[Vec<PathBuf>]) -> Result<()> {
        if duplicates.is_empty() {
            return Ok(());
        }

        let cache_hash = self.generate_cache_state_hash()?;
        
        // Clear any existing duplicate groups for this threshold
        self.conn.execute(
            "DELETE FROM duplicate_groups WHERE threshold = ?1",
            params![threshold],
        )?;

        let tx = self.conn.unchecked_transaction()?;

        for group in duplicates {
            if group.len() < 2 {
                continue; // Skip non-duplicate groups
            }

            // Insert the group
            tx.execute(
                "INSERT INTO duplicate_groups (threshold, group_hash) VALUES (?1, ?2)",
                params![threshold, cache_hash],
            )?;

            let group_id: i64 = tx.last_insert_rowid();

            // Insert the file paths for this group
            for path in group {
                tx.execute(
                    "INSERT INTO duplicate_group_files (group_id, file_path) VALUES (?1, ?2)",
                    params![group_id, path.to_string_lossy()],
                )?;
            }
        }

        tx.commit()?;
        info!("Cached {} duplicate groups for threshold {}", duplicates.len(), threshold);
        Ok(())
    }

    /// Get cached duplicate groups for a given threshold
    pub fn get_cached_duplicate_groups(&self, threshold: u32) -> Result<Option<Vec<Vec<PathBuf>>>> {
        let current_cache_hash = self.generate_cache_state_hash()?;

        // Check if we have cached groups for this threshold with matching cache state
        let mut stmt = self.conn.prepare(
            "SELECT COUNT(*) FROM duplicate_groups WHERE threshold = ?1 AND group_hash = ?2"
        )?;
        
        let count: i64 = stmt.query_row(params![threshold, current_cache_hash], |row| row.get(0))?;
        
        if count == 0 {
            info!("No valid cached duplicate groups found for threshold {}", threshold);
            return Ok(None);
        }

        // Retrieve the cached groups
        let mut groups_stmt = self.conn.prepare(
            "SELECT dg.id FROM duplicate_groups dg WHERE dg.threshold = ?1 AND dg.group_hash = ?2"
        )?;

        let group_ids: Vec<i64> = groups_stmt.query_map(params![threshold, current_cache_hash], |row| {
            row.get(0)
        })?.collect::<Result<Vec<_>, _>>()?;

        let mut duplicates = Vec::new();
        
        for group_id in group_ids {
            let mut files_stmt = self.conn.prepare(
                "SELECT file_path FROM duplicate_group_files WHERE group_id = ?1"
            )?;
            
            let file_paths: Vec<PathBuf> = files_stmt.query_map(params![group_id], |row| {
                Ok(PathBuf::from(row.get::<_, String>(0)?))
            })?.collect::<Result<Vec<_>, _>>()?;
            
            if file_paths.len() >= 2 {
                duplicates.push(file_paths);
            }
        }

        info!("Retrieved {} cached duplicate groups for threshold {}", duplicates.len(), threshold);
        Ok(Some(duplicates))
    }

    /// Clear all cached duplicate groups (e.g., when file cache changes)
    pub fn clear_duplicate_groups_cache(&self) -> Result<()> {
        let deleted = self.conn.execute("DELETE FROM duplicate_groups", [])?;
        if deleted > 0 {
            info!("Cleared {} cached duplicate groups", deleted);
        }
        Ok(())
    }
}
