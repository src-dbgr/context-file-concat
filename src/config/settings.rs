use anyhow::Result;
use directories::ProjectDirs;
use serde_json::Value;
use std::collections::HashSet;
use std::fs;
use std::path::PathBuf;

use super::AppConfig;

const APP_NAME: &str = "ContextFileConcat";
const CONFIG_FILE: &str = "config.json";

pub fn get_config_directory() -> Option<PathBuf> {
    ProjectDirs::from("com", "contextfileconcat", APP_NAME)
        .map(|proj_dirs| proj_dirs.config_dir().to_path_buf())
}

pub fn get_config_file_path() -> Option<PathBuf> {
    get_config_directory().map(|dir| dir.join(CONFIG_FILE))
}

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

    // Try to load as current format first
    match serde_json::from_str::<AppConfig>(&config_content) {
        Ok(config) => {
            tracing::info!("Loaded config from {:?}", config_path);
            Ok(config)
        }
        Err(_) => {
            // Fallback: Load as JSON Value and migrate
            tracing::info!(
                "Config format mismatch, attempting migration from {:?}",
                config_path
            );
            migrate_legacy_config(&config_content)
        }
    }
}

fn migrate_legacy_config(config_content: &str) -> Result<AppConfig> {
    let mut value: Value = serde_json::from_str(config_content)?;
    let obj = value
        .as_object_mut()
        .ok_or_else(|| anyhow::anyhow!("Config is not a JSON object"))?;

    // Ensure all required fields exist with defaults
    if !obj.contains_key("tree_ignore_patterns") {
        obj.insert("tree_ignore_patterns".to_string(), Value::Array(vec![]));
    }

    if !obj.contains_key("output_filename") {
        let default_filename = format!(
            "cfc_output_{}.txt",
            chrono::Local::now().format("%Y%m%d_%H%M%S")
        );
        obj.insert(
            "output_filename".to_string(),
            Value::String(default_filename),
        );
    }

    if !obj.contains_key("use_relative_paths") {
        obj.insert("use_relative_paths".to_string(), Value::Bool(true));
    }

    // Convert ignore_patterns from array to the expected format if needed
    if let Some(ignore_patterns) = obj.get("ignore_patterns") {
        if ignore_patterns.is_array() {
            // It's already an array, which will be deserialized as HashSet
        }
    }

    // Try to deserialize the migrated config
    let migrated_config: AppConfig = serde_json::from_value(Value::Object(obj.clone()))?;

    tracing::info!("Successfully migrated legacy config");
    Ok(migrated_config)
}

pub fn save_config(config: &AppConfig) -> Result<()> {
    let config_dir = get_config_directory()
        .ok_or_else(|| anyhow::anyhow!("Could not determine config directory"))?;

    // Create config directory if it doesn't exist
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

pub fn export_config(config: &AppConfig, export_path: &PathBuf) -> Result<()> {
    let config_json = serde_json::to_string_pretty(config)?;
    fs::write(export_path, config_json)?;
    tracing::info!("Exported config to {:?}", export_path);
    Ok(())
}

pub fn import_config(import_path: &PathBuf) -> Result<AppConfig> {
    let config_content = fs::read_to_string(import_path)?;

    // Try current format first, then legacy migration
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

// Platform-specific paths for reference:
// macOS:    ~/Library/Application Support/ContextFileConcat/
// Linux:    ~/.config/ContextFileConcat/
// Windows:  %APPDATA%/ContextFileConcat/
