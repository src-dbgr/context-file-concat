use anyhow::Result;
use directories::ProjectDirs;
use serde_json::Value;
use std::fs;
use std::path::PathBuf;

use super::AppConfig;

const APP_NAME: &str = "ContextFileConcat";
const CONFIG_FILE: &str = "config.json";

/// Returns the platform-specific configuration directory for the application.
pub fn get_config_directory() -> Option<PathBuf> {
    ProjectDirs::from("com", "contextfileconcat", APP_NAME)
        .map(|proj_dirs| proj_dirs.config_dir().to_path_buf())
}

/// Returns the full path to the configuration file.
pub fn get_config_file_path() -> Option<PathBuf> {
    get_config_directory().map(|dir| dir.join(CONFIG_FILE))
}

/// Loads the application configuration from the config file.
/// If the file doesn't exist, it creates a default one.
/// If the file is corrupted or cannot be parsed, it logs a warning
/// and falls back to the default configuration to prevent a crash.
pub fn load_config() -> Result<AppConfig> {
    let config_path = get_config_file_path()
        .ok_or_else(|| anyhow::anyhow!("Could not determine config directory"))?;

    if !config_path.exists() {
        tracing::info!(
            "Config file not found, creating default config at {:?}",
            config_path
        );
        let default_config = AppConfig::default();
        save_config(&default_config)?;
        return Ok(default_config);
    }

    let config_content = fs::read_to_string(&config_path)?;

    // Attempt to parse the config. If it fails, log a warning and fall back
    // to defaults. This makes the application more resilient.
    match serde_json::from_str::<AppConfig>(&config_content) {
        Ok(config) => {
            tracing::info!("Loaded config from {:?}", config_path);
            Ok(config)
        }
        Err(e) => {
            tracing::warn!(
                "Failed to parse config file at {:?}: {}. Falling back to default config.",
                config_path,
                e
            );
            // Attempt to migrate legacy config as a fallback before using default.
            migrate_legacy_config(&config_content).or_else(|_| Ok(AppConfig::default()))
        }
    }
}

/// Attempts to migrate a configuration from an older format to the current `AppConfig` struct.
/// This function is now more robust and handles missing or null fields gracefully.
fn migrate_legacy_config(config_content: &str) -> Result<AppConfig> {
    let mut value: Value = serde_json::from_str(config_content)?;
    let obj = value
        .as_object_mut()
        .ok_or_else(|| anyhow::anyhow!("Config is not a JSON object"))?;

    let defaults = AppConfig::default();

    // Helper to insert a default value if the key is missing or its value is null.
    let ensure_field = |obj: &mut serde_json::Map<String, Value>, key: &str, default_val: Value| {
        if !obj.contains_key(key) || obj.get(key) == Some(&Value::Null) {
            obj.insert(key.to_string(), default_val);
        }
    };

    // Ensure all potentially missing or null fields exist by inserting default values.
    ensure_field(
        obj,
        "tree_ignore_patterns",
        serde_json::to_value(&defaults.tree_ignore_patterns)?,
    );
    ensure_field(
        obj,
        "window_size",
        serde_json::to_value(defaults.window_size)?,
    );
    ensure_field(
        obj,
        "window_position",
        serde_json::to_value(defaults.window_position)?,
    );
    ensure_field(
        obj,
        "output_filename",
        serde_json::to_value(&defaults.output_filename)?,
    );
    ensure_field(
        obj,
        "use_relative_paths",
        Value::Bool(defaults.use_relative_paths),
    );
    ensure_field(
        obj,
        "auto_load_last_directory",
        Value::Bool(defaults.auto_load_last_directory),
    );
    ensure_field(
        obj,
        "max_file_size_mb",
        serde_json::to_value(defaults.max_file_size_mb)?,
    );
    ensure_field(
        obj,
        "scan_chunk_size",
        serde_json::to_value(defaults.scan_chunk_size)?,
    );
    ensure_field(
        obj,
        "remove_empty_directories",
        Value::Bool(defaults.remove_empty_directories),
    );

    let migrated_config: AppConfig = serde_json::from_value(Value::Object(obj.clone()))?;
    tracing::info!("Successfully migrated legacy config");
    Ok(migrated_config)
}

/// Saves the provided configuration to the config file.
pub fn save_config(config: &AppConfig) -> Result<()> {
    let config_dir = get_config_directory()
        .ok_or_else(|| anyhow::anyhow!("Could not determine config directory"))?;

    // Create config directory if it doesn't exist.
    if !config_dir.exists() {
        fs::create_dir_all(&config_dir)?;
        tracing::info!("Created config directory: {:?}", config_dir);
    }

    let config_path = config_dir.join(CONFIG_FILE);
    let config_json = serde_json::to_string_pretty(config)?;

    fs::write(&config_path, config_json)?;
    tracing::info!("Saved config to {:?}", config_path);

    Ok(())
}

/// Exports the current configuration to a user-specified JSON file.
pub fn export_config(config: &AppConfig, export_path: &PathBuf) -> Result<()> {
    let config_json = serde_json::to_string_pretty(config)?;
    fs::write(export_path, config_json)?;
    tracing::info!("Exported config to {:?}", export_path);
    Ok(())
}

/// Imports an application configuration from a user-specified JSON file.
pub fn import_config(import_path: &PathBuf) -> Result<AppConfig> {
    let config_content = fs::read_to_string(import_path)?;
    match serde_json::from_str::<AppConfig>(&config_content) {
        Ok(config) => {
            tracing::info!("Imported config from {:?}", import_path);
            Ok(config)
        }
        Err(_) => {
            tracing::info!("Importing legacy config format from {:?}", import_path);
            migrate_legacy_config(&config_content)
        }
    }
}

// Platform-specific configuration paths for reference:
// macOS:   ~/Library/Application Support/com.contextfileconcat.ContextFileConcat/
// Linux:   ~/.config/com.contextfileconcat.ContextFileConcat/
// Windows: %APPDATA%/com.contextfileconcat.ContextFileConcat/config/
