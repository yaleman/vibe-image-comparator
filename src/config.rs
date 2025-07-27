use anyhow::Result;

use crate::cache::Config;

pub fn load_config() -> Result<Config> {
    let config_dir =
        dirs::config_dir().ok_or_else(|| anyhow::anyhow!("Could not find config directory"))?;

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

/// Takes overrides because the CLI may want to show the config with different values
pub fn show_config_with_overrides(
    threshold_override: Option<u32>,
    grid_size_override: Option<u32>,
) -> Result<()> {
    let config_dir =
        dirs::config_dir().ok_or_else(|| anyhow::anyhow!("Could not find config directory"))?;

    let config = load_config()?;

    println!("=== Configuration ===");

    let effective_config = config.with_overrides(grid_size_override, threshold_override, None);
    let effective_grid_size = effective_config.grid_size;
    let effective_threshold = effective_config.threshold;

    println!("Grid size: {effective_grid_size}x{effective_grid_size}");
    if let Some(override_val) = grid_size_override {
        if let Some(config_grid_size) = config.grid_size {
            if override_val != config_grid_size {
                println!(
                    "  (overridden from config default: {config_grid_size}x{config_grid_size})"
                );
            }
        } else {
            println!("  (overridden from default: 128x128)");
        }
    }

    println!("Threshold: {effective_threshold}");
    if let Some(override_val) = threshold_override {
        if let Some(config_threshold) = config.threshold {
            if override_val != config_threshold {
                println!("  (overridden from config default: {config_threshold})");
            }
        } else {
            println!("  (overridden from default: 15)");
        }
    }

    if let Some(ref db_path) = config.database_path {
        println!("Database path: {db_path}");
    } else {
        let cache_dir = dirs::cache_dir()
            .unwrap_or_else(|| std::path::PathBuf::from("."))
            .join("vibe-image-comparator");
        let default_db_path = cache_dir.join("hashes.db");
        println!("Database path: {} (default)", default_db_path.display());
    }

    // Show ignore paths
    let ignore_paths = effective_config.ignore_paths;
    if ignore_paths.is_empty() {
        println!("Ignore paths: (none)");
    } else {
        println!("Ignore paths:");
        for path in &ignore_paths {
            println!("  - {path}");
        }
    }

    let default_config_path = config_dir.join("vibe-image-comparator.json");
    if default_config_path.exists() {
        println!("Config file: {}", default_config_path.display());
    } else {
        println!(
            "Config file: {} (not found, using defaults)",
            default_config_path.display()
        );
    }

    println!("=== End Configuration ===");
    Ok(())
}
