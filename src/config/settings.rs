use anyhow::{anyhow, Result};
use directories::ProjectDirs;
use serde::Serialize;
use serde_json::Value;
use std::fs;
use std::path::{Path, PathBuf};

use super::AppConfig;

const APP_NAME: &str = "ContextFileConcat";
const CONFIG_FILE: &str = "config.json";

/// Production implementation for getting the platform-specific config directory.
#[cfg(not(test))]
fn get_platform_config_dir() -> Option<PathBuf> {
    ProjectDirs::from("com", "contextfileconcat", APP_NAME)
        .map(|dirs| dirs.config_dir().to_path_buf())
}

/// Test implementation that can be mocked.
#[cfg(test)]
fn get_platform_config_dir() -> Option<PathBuf> {
    if tests::MOCK_PROJECT_DIRS_FAILS.load(std::sync::atomic::Ordering::SeqCst) {
        None
    } else {
        ProjectDirs::from("com", "contextfileconcat", APP_NAME)
            .map(|dirs| dirs.config_dir().to_path_buf())
    }
}

// This private helper function centralizes path resolution logic.
fn get_path(path_override: Option<&Path>) -> Result<PathBuf> {
    match path_override {
        Some(path) => Ok(path.to_path_buf()),
        None => get_platform_config_dir()
            .map(|dir| dir.join(CONFIG_FILE))
            .ok_or_else(|| anyhow!("Could not determine config directory")),
    }
}

/// Loads the application configuration.
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
    if config_content.trim().is_empty() {
        tracing::warn!(
            "Config file at {:?} is empty. Falling back to default config.",
            config_path
        );
        return Ok(AppConfig::default());
    }

    match serde_json::from_str::<AppConfig>(&config_content) {
        Ok(config) => {
            tracing::info!("Loaded config from {:?}", config_path);
            Ok(config)
        }
        Err(e) => {
            tracing::warn!(
                "Failed to parse config file at {:?}: {}. Attempting migration or fallback to default.",
                config_path,
                e
            );
            migrate_legacy_config(&config_content).or_else(|migration_err| {
                tracing::error!(
                    "Migration also failed: {}. Using default config.",
                    migration_err
                );
                Ok(AppConfig::default())
            })
        }
    }
}

/// Saves the provided configuration.
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
pub fn export_config(config: &AppConfig, export_path: &Path) -> Result<()> {
    save_config(config, Some(export_path))
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
            tracing::info!(
                "Attempting to import legacy config format from {:?}",
                import_path
            );
            migrate_legacy_config(&config_content)
        }
    }
}

/// Helper to ensure a field in the config JSON exists, adding it from a default if not.
/// This function isolates the fallible serialization step, making it testable.
fn ensure_field_from_default<T: Serialize>(
    obj: &mut serde_json::Map<String, Value>,
    key: &str,
    default_val: T,
) -> Result<()> {
    if !obj.contains_key(key) || obj.get(key) == Some(&Value::Null) {
        let val = serde_json::to_value(default_val)?;
        obj.insert(key.to_string(), val);
    }
    Ok(())
}

/// Attempts to migrate a configuration from an older format to the current `AppConfig` struct.
fn migrate_legacy_config(config_content: &str) -> Result<AppConfig> {
    let mut value: Value = serde_json::from_str(config_content)?;
    let obj = value
        .as_object_mut()
        .ok_or_else(|| anyhow!("Config is not a JSON object"))?;

    let defaults = AppConfig::default();

    ensure_field_from_default(obj, "ignore_patterns", &defaults.ignore_patterns)?;
    ensure_field_from_default(obj, "tree_ignore_patterns", &defaults.tree_ignore_patterns)?;
    ensure_field_from_default(obj, "last_directory", &defaults.last_directory)?;
    ensure_field_from_default(obj, "output_directory", &defaults.output_directory)?;
    ensure_field_from_default(obj, "output_filename", &defaults.output_filename)?;
    ensure_field_from_default(obj, "case_sensitive_search", defaults.case_sensitive_search)?;
    ensure_field_from_default(
        obj,
        "include_tree_by_default",
        defaults.include_tree_by_default,
    )?;
    ensure_field_from_default(obj, "use_relative_paths", defaults.use_relative_paths)?;
    ensure_field_from_default(
        obj,
        "remove_empty_directories",
        defaults.remove_empty_directories,
    )?;
    ensure_field_from_default(obj, "window_size", defaults.window_size)?;
    ensure_field_from_default(obj, "window_position", defaults.window_position)?;
    ensure_field_from_default(
        obj,
        "auto_load_last_directory",
        defaults.auto_load_last_directory,
    )?;
    ensure_field_from_default(obj, "max_file_size_mb", defaults.max_file_size_mb)?;
    ensure_field_from_default(obj, "scan_chunk_size", defaults.scan_chunk_size)?;

    let migrated_config: AppConfig = serde_json::from_value(Value::Object(obj.clone()))?;
    tracing::info!("Successfully migrated legacy config");
    Ok(migrated_config)
}

#[cfg(test)]
pub mod tests {
    use super::*;
    use serde::ser::{Error, Serializer};
    use serde::Serialize;
    use serde_json::json;
    use std::sync::atomic::{AtomicBool, Ordering};
    use tempfile::TempDir;

    // A flag to control the mock's behavior for get_platform_config_dir
    pub static MOCK_PROJECT_DIRS_FAILS: AtomicBool = AtomicBool::new(false);

    /// Test harness for creating isolated temporary directories and config files.
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
        fn temp_path(&self) -> &Path {
            self._temp_dir.path()
        }
    }

    // =========================================================================
    // SECTION: get_path Tests
    // =========================================================================

    #[test]
    fn test_get_path_fails_when_no_dir_is_found() {
        MOCK_PROJECT_DIRS_FAILS.store(true, Ordering::SeqCst);
        let result = get_path(None);
        MOCK_PROJECT_DIRS_FAILS.store(false, Ordering::SeqCst);

        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("Could not determine config directory"));
    }

    // =========================================================================
    // SECTION: load_config Tests
    // =========================================================================

    #[test]
    fn test_load_config_creates_default_when_nonexistent() {
        let harness = TestHarness::new();
        assert!(!harness.config_path.exists());
        let loaded_config = load_config(Some(&harness.config_path)).unwrap();
        assert_eq!(loaded_config, AppConfig::default());
        assert!(harness.config_path.exists());
    }

    #[test]
    fn test_load_config_with_corrupt_json_returns_default() {
        let harness = TestHarness::new();
        harness.write_to_config_file("{ \"key\": \"value\", }");
        let loaded_config = load_config(Some(&harness.config_path)).unwrap();
        assert_eq!(loaded_config, AppConfig::default());
    }

    #[test]
    fn test_load_config_with_empty_file_returns_default() {
        let harness = TestHarness::new();
        harness.write_to_config_file("");
        let loaded_config = load_config(Some(&harness.config_path)).unwrap();
        assert_eq!(loaded_config, AppConfig::default());
    }

    #[test]
    fn test_load_config_with_unmigratable_file_returns_default() {
        let harness = TestHarness::new();
        harness.write_to_config_file("[1, 2, 3]");
        let config = load_config(Some(&harness.config_path)).unwrap();
        assert_eq!(config, AppConfig::default());
    }

    // =========================================================================
    // SECTION: save_config & export_config Tests
    // =========================================================================

    #[test]
    fn test_config_roundtrip() {
        let harness = TestHarness::new();
        let mut original_config = AppConfig::default();
        original_config.case_sensitive_search = true;
        original_config
            .ignore_patterns
            .insert("test-pattern".to_string());
        save_config(&original_config, Some(&harness.config_path)).unwrap();
        let loaded_config = load_config(Some(&harness.config_path)).unwrap();
        assert_eq!(original_config, loaded_config);
    }

    #[test]
    fn test_save_config_creates_parent_directories() {
        let harness = TestHarness::new();
        let nested_path = harness.temp_path().join("new/nested/config.json");
        assert!(!nested_path.parent().unwrap().exists());
        save_config(&AppConfig::default(), Some(&nested_path)).unwrap();
        assert!(nested_path.exists());
    }

    // =========================================================================
    // SECTION: import_config Tests
    // =========================================================================

    #[test]
    fn test_import_config_with_corrupt_file_returns_err() {
        let harness = TestHarness::new();
        harness.write_to_config_file("{ \"key\": \"value\", }");
        let result = import_config(&harness.config_path);
        assert!(result.is_err());
    }

    #[test]
    fn test_import_config_with_legacy_file_succeeds_and_migrates() {
        let harness = TestHarness::new();
        let legacy_json = json!({ "case_sensitive_search": true });
        harness.write_to_config_file(&legacy_json.to_string());
        let result = import_config(&harness.config_path);
        assert!(result.is_ok());
    }

    // =========================================================================
    // SECTION: Migration Logic & Helpers Tests
    // =========================================================================

    struct Unserializable;
    impl Serialize for Unserializable {
        fn serialize<S>(&self, _serializer: S) -> Result<S::Ok, S::Error>
        where
            S: Serializer,
        {
            Err(S::Error::custom("serialization is designed to fail"))
        }
    }

    #[test]
    fn test_ensure_field_from_default_handles_serialization_error() {
        let mut obj_map = serde_json::Map::new();
        let result = ensure_field_from_default(&mut obj_map, "bad_key", Unserializable);
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("serialization is designed to fail"));
    }

    #[test]
    fn test_ensure_field_from_default_inserts_missing_key() {
        let mut obj_map = serde_json::Map::new();
        let default_value = "hello".to_string();
        ensure_field_from_default(&mut obj_map, "my_key", default_value.clone()).unwrap();
        assert_eq!(obj_map.get("my_key").unwrap().as_str().unwrap(), "hello");
    }

    #[test]
    fn test_migrate_legacy_config_fails_for_non_object() {
        let non_object_content = "[1, 2, 3]".to_string();
        let result = migrate_legacy_config(&non_object_content);
        assert!(result.is_err());
    }

    #[test]
    fn test_migrate_legacy_config_populates_all_fields() {
        let legacy_content = "{}".to_string(); // Start with empty object
        let migrated_config = migrate_legacy_config(&legacy_content).unwrap();
        assert_eq!(migrated_config, AppConfig::default());
    }

    // =========================================================================
    // SECTION: I/O Error Tests
    // =========================================================================

    #[test]
    fn test_io_failures_on_readonly_directory() {
        let temp_dir = tempfile::tempdir().unwrap();
        let readonly_dir = temp_dir.path().join("readonly");
        fs::create_dir(&readonly_dir).unwrap();
        let mut perms = fs::metadata(&readonly_dir).unwrap().permissions();
        perms.set_readonly(true);
        fs::set_permissions(&readonly_dir, perms.clone()).unwrap();

        let config_path = readonly_dir.join("config.json");
        let config = AppConfig::default();

        let save_result = save_config(&config, Some(&config_path));
        let export_result = export_config(&config, &config_path);
        assert!(!config_path.exists());
        let load_result = load_config(Some(&config_path));

        if cfg!(unix) {
            assert!(save_result.is_err());
            assert!(export_result.is_err());
            assert!(load_result.is_err());
        }

        perms.set_readonly(false);
        fs::set_permissions(&readonly_dir, perms).unwrap();
    }
}
