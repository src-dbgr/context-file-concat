use anyhow::{anyhow, Result};
use directories::ProjectDirs;
use serde::Serialize;
use serde_json::Value;
use std::ffi::OsString;
use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use super::AppConfig;

const APP_NAME: &str = "ContextFileConcat";
const CONFIG_FILE: &str = "config.json";

/// Environment variable that overrides the directory holding `config.json`.
///
/// Meant for integration tests (which must never touch the user's real configuration)
/// and for users who want to keep the configuration somewhere else.
pub const CONFIG_DIR_ENV: &str = "CFC_CONFIG_DIR";

/// The platform's standard per-user config directory for this application.
fn platform_config_dir() -> Option<PathBuf> {
    ProjectDirs::from("com", "contextfileconcat", APP_NAME)
        .map(|dirs| dirs.config_dir().to_path_buf())
}

/// Picks the config directory: a non-empty override wins over the platform default.
fn resolve_config_dir(
    override_dir: Option<OsString>,
    platform_dir: impl FnOnce() -> Option<PathBuf>,
) -> Option<PathBuf> {
    match override_dir {
        Some(dir) if !dir.is_empty() => Some(PathBuf::from(dir)),
        _ => platform_dir(),
    }
}

/// The file the configuration is kept in: `config.json` in the directory named by
/// [`CONFIG_DIR_ENV`] if that is set, in the platform's standard location otherwise.
/// `None` if no such directory can be determined.
///
/// This is the only place deciding that, and the only function that looks at the
/// environment. Everything else takes the path as a parameter, so the caller (the
/// composition root in `main.rs`) decides which file is used.
pub fn default_config_path() -> Option<PathBuf> {
    resolve_config_dir(std::env::var_os(CONFIG_DIR_ENV), platform_config_dir)
        .map(|dir| dir.join(CONFIG_FILE))
}

/// Loads the configuration from `config_path`.
///
/// A missing file is created with the default configuration. A file that is empty or
/// cannot be read as a configuration (not even after migrating an older format) yields
/// the default configuration and is left as it is.
pub fn load_config(config_path: &Path) -> Result<AppConfig> {
    if !config_path.exists() {
        tracing::info!(
            "Config file not found, creating default config at {:?}",
            config_path
        );
        let default_config = AppConfig::default();
        save_config(&default_config, config_path)?;
        return Ok(default_config);
    }

    let config_content = fs::read_to_string(config_path)?;
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

/// Writes the configuration to `config_path`, creating its directory if necessary.
///
/// The file is replaced atomically (see [`write_atomically`]): whatever goes wrong
/// while writing, the file holds either the previous or the new configuration, never
/// a part of one.
pub fn save_config(config: &AppConfig, config_path: &Path) -> Result<()> {
    if let Some(parent) = config_path.parent() {
        if !parent.exists() {
            fs::create_dir_all(parent)?;
        }
    }
    let config_json = serde_json::to_string_pretty(config)?;
    write_atomically(config_path, |file| file.write_all(config_json.as_bytes()))?;
    tracing::info!("Saved config to {:?}", config_path);
    Ok(())
}

/// Replaces the content of `path` with what `write_content` writes, atomically.
///
/// The content goes to a temporary file in the same directory, which then takes the
/// place of `path` by a rename. Nobody ever sees a half-written file, and if writing
/// fails (disk full, crash, error in `write_content`) the previous file is untouched
/// and the temporary file is removed.
///
/// A symbolic link at `path` is followed: the file it points to is replaced, the link
/// stays. The permissions of an existing file are kept.
///
/// If the directory does not allow creating the temporary file while the file itself
/// can be written, the content is written in place instead (not atomically), as there
/// is no other way to save.
fn write_atomically(
    path: &Path,
    write_content: impl Fn(&mut fs::File) -> io::Result<()>,
) -> io::Result<()> {
    // Renaming over a link would replace the link, not the file behind it.
    let target = fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    let temp_path = temp_path_beside(&target);

    let mut temp_file = match fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&temp_path)
    {
        Ok(file) => file,
        Err(error) if error.kind() == io::ErrorKind::PermissionDenied && target.is_file() => {
            tracing::warn!(
                "Cannot create a temporary file beside {:?} ({}); writing it in place.",
                target,
                error
            );
            return write_content(&mut fs::File::create(&target)?);
        }
        Err(error) => return Err(error),
    };

    let written = (|| {
        write_content(&mut temp_file)?;
        // The content has to be on disk before the rename makes it the file.
        temp_file.sync_all()?;
        if let Ok(metadata) = fs::metadata(&target) {
            fs::set_permissions(&temp_path, metadata.permissions())?;
        }
        drop(temp_file);
        fs::rename(&temp_path, &target)
    })();

    if written.is_err() {
        // Best effort; the error worth reporting is the one of the write.
        let _ = fs::remove_file(&temp_path);
    }
    written
}

/// A name for a temporary file in the directory of `target` that no other write uses,
/// neither of this process nor of another one.
fn temp_path_beside(target: &Path) -> PathBuf {
    static COUNTER: AtomicU64 = AtomicU64::new(0);

    let mut name = OsString::from(".");
    name.push(target.file_name().unwrap_or_default());
    name.push(format!(
        ".{}.{}.tmp",
        std::process::id(),
        COUNTER.fetch_add(1, Ordering::Relaxed)
    ));
    target.with_file_name(name)
}

/// Exports the current configuration to a user-specified JSON file.
pub fn export_config(config: &AppConfig, export_path: &Path) -> Result<()> {
    save_config(config, export_path)
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
    // The defaults are logical units. A file that brings a geometry of its own without
    // naming its unit keeps the unit of such files (physical pixels, see
    // `GeometryUnits`), which is what a missing `geometry_units` deserializes to.
    if !obj.contains_key("window_size") && !obj.contains_key("window_position") {
        ensure_field_from_default(obj, "geometry_units", defaults.geometry_units)?;
    }
    ensure_field_from_default(obj, "window_size", defaults.window_size)?;
    ensure_field_from_default(obj, "window_position", defaults.window_position)?;
    ensure_field_from_default(obj, "max_file_size_mb", defaults.max_file_size_mb)?;

    let migrated_config: AppConfig = serde_json::from_value(Value::Object(obj.clone()))?;
    tracing::info!("Successfully migrated legacy config");
    Ok(migrated_config)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::utils::test_helpers::running_as_root;
    use serde::ser::{Error, Serializer};
    use serde::Serialize;
    use serde_json::json;
    use tempfile::TempDir;

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
    // SECTION: Location of the config file
    // =========================================================================

    #[test]
    fn test_resolve_config_dir_prefers_a_non_empty_override() {
        let platform = || Some(PathBuf::from("/platform"));
        assert_eq!(
            resolve_config_dir(Some("/custom/dir".into()), platform),
            Some(PathBuf::from("/custom/dir"))
        );
        assert_eq!(
            resolve_config_dir(Some(OsString::new()), platform),
            Some(PathBuf::from("/platform"))
        );
        assert_eq!(
            resolve_config_dir(None, platform),
            Some(PathBuf::from("/platform"))
        );
        assert_eq!(resolve_config_dir(None, || None), None);
        // The platform lookup is not consulted when the override is used.
        assert_eq!(
            resolve_config_dir(Some("/custom".into()), || unreachable!()),
            Some(PathBuf::from("/custom"))
        );
    }

    // =========================================================================
    // SECTION: load_config Tests
    // =========================================================================

    #[test]
    fn test_load_config_creates_default_when_nonexistent() {
        let harness = TestHarness::new();
        assert!(!harness.config_path.exists());
        let loaded_config = load_config(&harness.config_path).unwrap();
        assert_eq!(loaded_config, AppConfig::default());
        assert!(harness.config_path.exists());
    }

    #[test]
    fn test_load_config_with_corrupt_json_returns_default() {
        let harness = TestHarness::new();
        harness.write_to_config_file("{ \"key\": \"value\", }");
        let loaded_config = load_config(&harness.config_path).unwrap();
        assert_eq!(loaded_config, AppConfig::default());
    }

    #[test]
    fn test_load_config_with_empty_file_returns_default() {
        let harness = TestHarness::new();
        harness.write_to_config_file("");
        let loaded_config = load_config(&harness.config_path).unwrap();
        assert_eq!(loaded_config, AppConfig::default());
    }

    #[test]
    fn test_load_config_with_unmigratable_file_returns_default() {
        let harness = TestHarness::new();
        harness.write_to_config_file("[1, 2, 3]");
        let config = load_config(&harness.config_path).unwrap();
        assert_eq!(config, AppConfig::default());
    }

    // =========================================================================
    // SECTION: save_config & export_config Tests
    // =========================================================================

    #[test]
    fn test_config_roundtrip() {
        let harness = TestHarness::new();
        let mut original_config = AppConfig {
            case_sensitive_search: true,
            ..Default::default()
        };
        original_config
            .ignore_patterns
            .insert("test-pattern".to_string());
        save_config(&original_config, &harness.config_path).unwrap();
        let loaded_config = load_config(&harness.config_path).unwrap();
        assert_eq!(original_config, loaded_config);
    }

    /// The names of the files in `dir`, sorted.
    fn file_names_in(dir: &Path) -> Vec<String> {
        let mut names: Vec<String> = fs::read_dir(dir)
            .unwrap()
            .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        names
    }

    #[test]
    fn test_failed_write_leaves_the_previous_file_intact() {
        let harness = TestHarness::new();
        save_config(&AppConfig::default(), &harness.config_path).unwrap();
        let previous = fs::read_to_string(&harness.config_path).unwrap();

        // The write breaks off after a part of the content, as with a full disk.
        let result = write_atomically(&harness.config_path, |file| {
            file.write_all(b"{ \"ignore_patterns\": [")?;
            Err(io::Error::other("disk full"))
        });

        assert_eq!(result.unwrap_err().to_string(), "disk full");
        assert_eq!(
            fs::read_to_string(&harness.config_path).unwrap(),
            previous,
            "the previous configuration must survive a failed write"
        );
        assert_eq!(
            file_names_in(harness.temp_path()),
            ["test-config.json"],
            "no temporary file may be left behind"
        );
        assert_eq!(
            load_config(&harness.config_path).unwrap(),
            AppConfig::default()
        );
    }

    #[test]
    fn test_failed_first_write_creates_no_file() {
        let harness = TestHarness::new();

        let result = write_atomically(&harness.config_path, |file| {
            file.write_all(b"partial")?;
            Err(io::Error::other("disk full"))
        });

        assert!(result.is_err());
        assert!(file_names_in(harness.temp_path()).is_empty());
    }

    #[test]
    fn test_save_config_replaces_the_file_and_leaves_no_temporary_file() {
        let harness = TestHarness::new();
        let first = AppConfig {
            output_filename: "first.txt".to_string(),
            ..AppConfig::default()
        };
        let second = AppConfig {
            output_filename: "second.txt".to_string(),
            ..AppConfig::default()
        };

        save_config(&first, &harness.config_path).unwrap();
        save_config(&second, &harness.config_path).unwrap();

        assert_eq!(load_config(&harness.config_path).unwrap(), second);
        assert_eq!(file_names_in(harness.temp_path()), ["test-config.json"]);
    }

    #[cfg(unix)]
    #[test]
    fn test_save_config_keeps_permissions_and_follows_a_symbolic_link() {
        use std::os::unix::fs::PermissionsExt;

        let harness = TestHarness::new();
        let real_dir = harness.temp_path().join("dotfiles");
        fs::create_dir(&real_dir).unwrap();
        let real_file = real_dir.join("cfc.json");
        fs::write(&real_file, "{}").unwrap();
        fs::set_permissions(&real_file, fs::Permissions::from_mode(0o600)).unwrap();
        std::os::unix::fs::symlink(&real_file, &harness.config_path).unwrap();

        save_config(&AppConfig::default(), &harness.config_path).unwrap();

        assert!(
            fs::symlink_metadata(&harness.config_path)
                .unwrap()
                .file_type()
                .is_symlink(),
            "the link must stay a link"
        );
        assert_eq!(load_config(&real_file).unwrap(), AppConfig::default());
        assert_eq!(
            fs::metadata(&real_file).unwrap().permissions().mode() & 0o777,
            0o600
        );
        assert_eq!(file_names_in(&real_dir), ["cfc.json"]);
    }

    /// A file that can be written in a directory that cannot: saving still works, as
    /// it did before writes went through a temporary file.
    #[cfg(unix)]
    #[test]
    fn test_save_config_writes_in_place_when_the_directory_is_read_only() {
        if running_as_root() {
            return;
        }
        let harness = TestHarness::new();
        let dir = harness.temp_path().join("locked");
        fs::create_dir(&dir).unwrap();
        let config_path = dir.join("config.json");
        fs::write(&config_path, "{}").unwrap();
        let writable = fs::metadata(&dir).unwrap().permissions();
        let mut read_only = writable.clone();
        read_only.set_readonly(true);
        fs::set_permissions(&dir, read_only).unwrap();

        let result = save_config(&AppConfig::default(), &config_path);

        fs::set_permissions(&dir, writable).unwrap();
        result.unwrap();
        assert_eq!(load_config(&config_path).unwrap(), AppConfig::default());
    }

    #[test]
    fn test_load_and_import_raise_a_file_size_limit_of_zero() {
        let harness = TestHarness::new();
        let mut json = serde_json::to_value(AppConfig::default()).unwrap();
        json["max_file_size_mb"] = json!(0);
        harness.write_to_config_file(&json.to_string());

        let loaded = load_config(&harness.config_path).unwrap();
        let imported = import_config(&harness.config_path).unwrap();

        assert_eq!(loaded.max_file_size_mb, crate::config::MIN_MAX_FILE_SIZE_MB);
        assert_eq!(
            imported.max_file_size_mb,
            crate::config::MIN_MAX_FILE_SIZE_MB
        );
        // Nothing else is touched by that.
        assert_eq!(
            AppConfig {
                max_file_size_mb: AppConfig::default().max_file_size_mb,
                ..loaded
            },
            AppConfig::default()
        );
    }

    #[test]
    fn test_save_config_creates_parent_directories() {
        let harness = TestHarness::new();
        let nested_path = harness.temp_path().join("new/nested/config.json");
        assert!(!nested_path.parent().unwrap().exists());
        save_config(&AppConfig::default(), &nested_path).unwrap();
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
    // SECTION: Retired settings
    // =========================================================================

    /// A complete configuration as an older version wrote it: with the retired
    /// `scan_chunk_size` and `auto_load_last_directory` and with settings that differ
    /// from the defaults.
    fn config_json_with_retired_field() -> (Value, AppConfig) {
        let expected = AppConfig {
            case_sensitive_search: true,
            max_file_size_mb: 7,
            output_filename: "kept.txt".to_string(),
            ..Default::default()
        };
        let mut value = serde_json::to_value(&expected).unwrap();
        value
            .as_object_mut()
            .unwrap()
            .insert("scan_chunk_size".to_string(), json!(100));
        value
            .as_object_mut()
            .unwrap()
            .insert("auto_load_last_directory".to_string(), json!(true));
        (value, expected)
    }

    #[test]
    fn test_config_with_retired_field_deserializes_without_migration() {
        let (value, expected) = config_json_with_retired_field();

        let config: AppConfig = serde_json::from_value(value).unwrap();

        assert_eq!(config, expected);
    }

    #[test]
    fn test_load_config_accepts_file_with_retired_field() {
        let harness = TestHarness::new();
        let (value, expected) = config_json_with_retired_field();
        harness.write_to_config_file(&value.to_string());

        let config = load_config(&harness.config_path).unwrap();

        assert_eq!(config, expected);
    }

    #[test]
    fn test_import_config_accepts_file_with_retired_field() {
        let harness = TestHarness::new();
        let (value, expected) = config_json_with_retired_field();
        harness.write_to_config_file(&value.to_string());

        let config = import_config(&harness.config_path).unwrap();

        assert_eq!(config, expected);
    }

    /// The migration of an incomplete file tolerates the retired field as well.
    #[test]
    fn test_migrate_legacy_config_ignores_retired_field() {
        let legacy = json!({ "case_sensitive_search": true, "scan_chunk_size": 100 });

        let config = migrate_legacy_config(&legacy.to_string()).unwrap();

        assert_eq!(
            config,
            AppConfig {
                case_sensitive_search: true,
                ..Default::default()
            }
        );
    }

    #[test]
    fn test_saved_and_exported_config_do_not_contain_retired_field() {
        let harness = TestHarness::new();
        let export_path = harness.temp_path().join("export.json");

        save_config(&AppConfig::default(), &harness.config_path).unwrap();
        export_config(&AppConfig::default(), &export_path).unwrap();

        for path in [&harness.config_path, &export_path] {
            let written = fs::read_to_string(path).unwrap();
            assert!(!written.contains("scan_chunk_size"), "{path:?}");
        }
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
        #[cfg(unix)]
        if running_as_root() {
            eprintln!("Skipping permission-based test because process runs as root (Docker/act).");
            return;
        }
        let temp_dir = tempfile::tempdir().unwrap();
        let readonly_dir = temp_dir.path().join("readonly");
        fs::create_dir(&readonly_dir).unwrap();
        let original_perms = fs::metadata(&readonly_dir).unwrap().permissions();
        let mut readonly_perms = original_perms.clone();
        readonly_perms.set_readonly(true);
        fs::set_permissions(&readonly_dir, readonly_perms).unwrap();

        let config_path = readonly_dir.join("config.json");
        let config = AppConfig::default();

        let save_result = save_config(&config, &config_path);
        let export_result = export_config(&config, &config_path);
        assert!(!config_path.exists());
        let load_result = load_config(&config_path);

        if cfg!(unix) {
            assert!(save_result.is_err());
            assert!(export_result.is_err());
            assert!(load_result.is_err());
        }

        // Restore the original permissions so the temp dir can be cleaned up.
        fs::set_permissions(&readonly_dir, original_perms).unwrap();
    }
}
