use std::path::PathBuf;
use std::fs;
use anyhow::Result;
use directories::ProjectDirs;

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
        tracing::info!("Config file not found, creating default config at {:?}", config_path);
        let default_config = AppConfig::default();
        save_config(&default_config)?;
        return Ok(default_config);
    }
    
    let config_content = fs::read_to_string(&config_path)?;
    let config: AppConfig = serde_json::from_str(&config_content)?;
    
    tracing::info!("Loaded config from {:?}", config_path);
    Ok(config)
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
    let config: AppConfig = serde_json::from_str(&config_content)?;
    tracing::info!("Imported config from {:?}", import_path);
    Ok(config)
}

// Platform-specific paths for reference:
// macOS:    ~/Library/Application Support/ContextFileConcat/
// Linux:    ~/.config/ContextFileConcat/
// Windows:  %APPDATA%/ContextFileConcat/