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

    let effective_grid_size = grid_size_override.unwrap_or(config.grid_size);
    let effective_threshold = threshold_override.unwrap_or(config.threshold);

    println!("Grid size: {effective_grid_size}x{effective_grid_size}");
    if let Some(override_val) = grid_size_override {
        if override_val != config.grid_size {
            println!(
                "  (overridden from config default: {}x{})",
                config.grid_size, config.grid_size
            );
        }
    }

    println!("Threshold: {effective_threshold}");
    if let Some(override_val) = threshold_override {
        if override_val != config.threshold {
            println!("  (overridden from config default: {})", config.threshold);
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
