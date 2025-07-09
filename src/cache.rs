use anyhow::Result;
use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Debug, Serialize, Deserialize)]
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
    pub perceptual_hash: Vec<u8>,
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

        Ok(HashCache { conn })
    }

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
        let mut stmt = conn
            .prepare("SELECT name FROM sqlite_master WHERE type='table' AND name='file_hashes'")?;
        let old_table_exists = stmt.exists([])?;

        if old_table_exists {
            println!("Migrating existing cache data to normalized schema...");

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
            println!("Migration completed successfully");
        }

        Ok(())
    }

    pub fn get_cached_hash(&self, path: &Path, size: u64, sha256: &str) -> Result<Option<Vec<u8>>> {
        let mut stmt = self.conn.prepare(
            "SELECT ph.perceptual_hash 
             FROM files f 
             JOIN perceptual_hashes ph ON f.perceptual_hash_id = ph.id 
             WHERE f.path = ?1 AND f.size = ?2 AND ph.sha256 = ?3",
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
            println!("Cleaned up {orphaned} orphaned perceptual hashes");
        }

        Ok(deleted)
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
            eprintln!(
                "Cleaned up {orphaned} orphaned perceptual hashes after removing broken file"
            );
        }

        Ok(())
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
            println!(
                "Deduplication ratio: {:.2} (lower = more deduplication)",
                ratio
            );
        }

        println!("=== End Debug Info ===\n");
        Ok(())
    }
}