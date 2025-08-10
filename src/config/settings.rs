use anyhow::Result;
use directories::ProjectDirs;
use serde_json::Value;
use std::fs;
use std::path::{Path, PathBuf};

use super::AppConfig;

const APP_NAME: &str = "ContextFileConcat";
const CONFIG_FILE: &str = "config.json";

// This private helper function centralizes path resolution logic.
fn get_path(path_override: Option<&Path>) -> Result<PathBuf> {
    match path_override {
        Some(path) => Ok(path.to_path_buf()),
        None => ProjectDirs::from("com", "contextfileconcat", APP_NAME)
            .map(|dirs| dirs.config_dir().join(CONFIG_FILE))
            .ok_or_else(|| anyhow::anyhow!("Could not determine config directory")),
    }
}

/// Loads the application configuration.
/// For tests, an override path can be provided.
pub fn load_config(path_override: Option<&Path>) -> Result<AppConfig> {
    let config_path = get_path(path_override)?;

    if !config_path.exists() {
        tracing::info!(
            "Config file not found, creating default config at {:?}",
            config_path
        );
        let default_config = AppConfig::default();
        save_config(&default_config, Some(&config_path))?;
        return Ok(default_config);
    }

    let config_content = fs::read_to_string(&config_path)?;

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
            migrate_legacy_config(&config_content).or_else(|_| Ok(AppConfig::default()))
        }
    }
}

/// Saves the provided configuration.
/// For tests, an override path can be provided.
pub fn save_config(config: &AppConfig, path_override: Option<&Path>) -> Result<()> {
    let config_path = get_path(path_override)?;
    if let Some(parent) = config_path.parent() {
        if !parent.exists() {
            fs::create_dir_all(parent)?;
        }
    }
    let config_json = serde_json::to_string_pretty(config)?;
    fs::write(&config_path, config_json)?;
    tracing::info!("Saved config to {:?}", config_path);
    Ok(())
}

/// Exports the current configuration to a user-specified JSON file.
pub fn export_config(config: &AppConfig, export_path: &PathBuf) -> Result<()> {
    save_config(config, Some(export_path))
}

/// Imports an application configuration from a user-specified JSON file.
/// This is stricter than `load_config` and will fail on corrupt files.
pub fn import_config(import_path: &PathBuf) -> Result<AppConfig> {
    let config_content = fs::read_to_string(import_path)?;
    match serde_json::from_str::<AppConfig>(&config_content) {
        Ok(config) => {
            tracing::info!("Imported config from {:?}", import_path);
            Ok(config)
        }
        Err(_) => {
            tracing::info!(
                "Attempting to import legacy config format from {:?}",
                import_path
            );
            // If migration fails, the error will now propagate up, as it should.
            migrate_legacy_config(&config_content)
        }
    }
}

/// Attempts to migrate a configuration from an older format to the current `AppConfig` struct.
fn migrate_legacy_config(config_content: &str) -> Result<AppConfig> {
    let mut value: Value = serde_json::from_str(config_content)?;
    let obj = value
        .as_object_mut()
        .ok_or_else(|| anyhow::anyhow!("Config is not a JSON object"))?;

    let defaults = AppConfig::default();

    let ensure_field = |obj: &mut serde_json::Map<String, Value>, key: &str, default_val: Value| {
        if !obj.contains_key(key) || obj.get(key) == Some(&Value::Null) {
            obj.insert(key.to_string(), default_val);
        }
    };

    // This is the complete and correct list of all fields to ensure.
    ensure_field(
        obj,
        "ignore_patterns",
        serde_json::to_value(&defaults.ignore_patterns)?,
    );
    ensure_field(
        obj,
        "tree_ignore_patterns",
        serde_json::to_value(&defaults.tree_ignore_patterns)?,
    );
    ensure_field(
        obj,
        "last_directory",
        serde_json::to_value(&defaults.last_directory)?,
    );
    ensure_field(
        obj,
        "output_directory",
        serde_json::to_value(&defaults.output_directory)?,
    );
    ensure_field(
        obj,
        "output_filename",
        serde_json::to_value(&defaults.output_filename)?,
    );
    ensure_field(
        obj,
        "case_sensitive_search",
        Value::Bool(defaults.case_sensitive_search),
    );
    ensure_field(
        obj,
        "include_tree_by_default",
        Value::Bool(defaults.include_tree_by_default),
    );
    ensure_field(
        obj,
        "use_relative_paths",
        Value::Bool(defaults.use_relative_paths),
    );
    ensure_field(
        obj,
        "remove_empty_directories",
        Value::Bool(defaults.remove_empty_directories),
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

    let migrated_config: AppConfig = serde_json::from_value(Value::Object(obj.clone()))?;
    tracing::info!("Successfully migrated legacy config");
    Ok(migrated_config)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use tempfile::TempDir;
    struct TestHarness {
        _temp_dir: TempDir,
        config_path: PathBuf,
    }

    impl TestHarness {
        fn new() -> Self {
            let temp_dir = tempfile::tempdir().expect("Failed to create temp dir");
            let config_path = temp_dir.path().join("test-config.json");
            Self {
                _temp_dir: temp_dir,
                config_path,
            }
        }
        fn write_to_config_file(&self, content: &str) {
            fs::write(&self.config_path, content).unwrap();
        }
    }

    #[test]
    fn test_load_config_creates_default_when_nonexistent() {
        let harness = TestHarness::new();
        assert!(!harness.config_path.exists());
        let loaded_config = load_config(Some(&harness.config_path)).unwrap();
        assert_eq!(loaded_config, AppConfig::default());
        assert!(harness.config_path.exists());
    }

    #[test]
    fn test_config_roundtrip() {
        let harness = TestHarness::new();
        let mut original_config = AppConfig::default();
        original_config.case_sensitive_search = true;
        original_config
            .ignore_patterns
            .insert("test-pattern".to_string());
        let save_result = save_config(&original_config, Some(&harness.config_path));
        assert!(save_result.is_ok());
        let loaded_config = load_config(Some(&harness.config_path)).unwrap();
        assert_eq!(original_config, loaded_config);
    }

    #[test]
    fn test_load_legacy_config_migrates_correctly() {
        let harness = TestHarness::new();
        let legacy_json = json!({ "ignore_patterns": ["node_modules/"], "last_directory": null, "case_sensitive_search": true });
        harness.write_to_config_file(&legacy_json.to_string());
        let migrated_config = load_config(Some(&harness.config_path)).unwrap();
        assert!(migrated_config.case_sensitive_search);
        assert!(migrated_config.ignore_patterns.contains("node_modules/"));
        let default_config = AppConfig::default();
        assert_eq!(
            migrated_config.remove_empty_directories,
            default_config.remove_empty_directories
        );
        assert_eq!(
            migrated_config.include_tree_by_default,
            default_config.include_tree_by_default
        );
    }

    #[test]
    fn test_load_corrupt_config_returns_default() {
        let harness = TestHarness::new();
        harness.write_to_config_file("{ \"key\": \"value\", }");
        let loaded_config = load_config(Some(&harness.config_path)).unwrap();
        assert_eq!(loaded_config, AppConfig::default());
    }

    #[test]
    fn test_legacy_migration_handles_null_values() {
        let legacy_content =
            json!({ "case_sensitive_search": true, "window_size": null, "max_file_size_mb": 5 })
                .to_string();
        let migrated_config = migrate_legacy_config(&legacy_content).unwrap();
        let default_config = AppConfig::default();
        assert_eq!(migrated_config.window_size, default_config.window_size);
        assert_eq!(migrated_config.max_file_size_mb, 5);
        assert!(migrated_config.case_sensitive_search);
    }

    #[test]
    fn test_migrate_legacy_config_fails_for_non_object() {
        let non_object_content = "[1, 2, 3]".to_string();
        let result = migrate_legacy_config(&non_object_content);
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("Config is not a JSON object"));
    }

    #[test]
    fn test_save_to_readonly_path_fails() {
        let temp_dir = tempfile::tempdir().unwrap();
        let readonly_dir = temp_dir.path().join("readonly");
        fs::create_dir(&readonly_dir).unwrap();
        let mut perms = fs::metadata(&readonly_dir).unwrap().permissions();
        perms.set_readonly(true);
        fs::set_permissions(&readonly_dir, perms).unwrap();
        let config_path = readonly_dir.join("config.json");
        let config = AppConfig::default();
        let result = save_config(&config, Some(&config_path));
        if cfg!(unix) {
            assert!(
                result.is_err(),
                "Saving to a read-only directory should fail on Unix"
            );
        } else {
            if result.is_ok() {
                eprintln!("Warning: `save_to_readonly` test passed on a non-Unix system. This is acceptable.");
            }
        }
        let mut perms = fs::metadata(&readonly_dir).unwrap().permissions();
        perms.set_readonly(false);
        fs::set_permissions(&readonly_dir, perms).unwrap();
    }
}
