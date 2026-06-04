//! Atomic local persistence of user-approved state under
//! `%LOCALAPPDATA%\LangCheck` (`blueprint.md` Sections 8.12, 14).
//!
//! Files are written via a temporary sibling and an atomic rename, so a crash mid
//! write never corrupts the live file. Only settings and user-approved
//! dictionary/rule entries are ever stored here — never a history of typed words.
//! `delete_all_state` backs the "delete all LangCheck state" action.
//!
//! Implemented in delivery Step 08 (Native Tray, Settings, and Persistence).

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use crate::config::Config;

/// Per-user application data directory name.
pub const APP_DIR_NAME: &str = "LangCheck";

/// `%LOCALAPPDATA%\LangCheck`, or `None` if the environment is missing.
pub fn data_dir() -> Option<PathBuf> {
    std::env::var_os("LOCALAPPDATA").map(|base| Path::new(&base).join(APP_DIR_NAME))
}

/// Path to `config.toml`.
pub fn config_path() -> Option<PathBuf> {
    data_dir().map(|dir| dir.join("config.toml"))
}

/// Write `contents` to `path` atomically: write a temporary sibling, then rename
/// over the destination (replacing it on Windows). Creates the parent directory.
pub fn atomic_write(path: &Path, contents: &[u8]) -> io::Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let temp = temp_sibling(path);
    fs::write(&temp, contents)?;
    match fs::rename(&temp, path) {
        Ok(()) => Ok(()),
        Err(e) => {
            let _ = fs::remove_file(&temp);
            Err(e)
        }
    }
}

fn temp_sibling(path: &Path) -> PathBuf {
    let mut name = path
        .file_name()
        .map(std::ffi::OsStr::to_os_string)
        .unwrap_or_default();
    name.push(format!(".{}.tmp", std::process::id()));
    path.with_file_name(name)
}

/// Load config from an explicit path; a missing or unreadable file yields defaults.
pub fn load_config_from(path: &Path) -> Config {
    match fs::read_to_string(path) {
        Ok(text) => Config::load_str(&text),
        Err(_) => Config::default(),
    }
}

/// Atomically save config to an explicit path.
pub fn save_config_to(path: &Path, config: &Config) -> io::Result<()> {
    atomic_write(path, config.to_toml().as_bytes())
}

/// Load config from the standard location (defaults if unavailable).
pub fn load_config() -> Config {
    config_path().map_or_else(Config::default, |path| load_config_from(&path))
}

/// Atomically save config to the standard location.
pub fn save_config(config: &Config) -> io::Result<()> {
    match config_path() {
        Some(path) => save_config_to(&path, config),
        None => Err(io::Error::new(
            io::ErrorKind::NotFound,
            "LOCALAPPDATA is not set",
        )),
    }
}

/// Recursively delete a state directory (no error if it is absent).
pub fn delete_all_state_in(dir: &Path) -> io::Result<()> {
    if dir.exists() {
        fs::remove_dir_all(dir)
    } else {
        Ok(())
    }
}

/// Delete all LangCheck state under the standard data directory.
pub fn delete_all_state() -> io::Result<()> {
    match data_dir() {
        Some(dir) => delete_all_state_in(&dir),
        None => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn unique_temp_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("langcheck-test-{}-{tag}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).expect("create temp dir");
        dir
    }

    #[test]
    fn atomic_write_round_trips_and_overwrites() {
        let dir = unique_temp_dir("atomic");
        let path = dir.join("nested").join("file.txt");
        atomic_write(&path, b"first").unwrap();
        assert_eq!(fs::read_to_string(&path).unwrap(), "first");
        atomic_write(&path, b"second").unwrap();
        assert_eq!(fs::read_to_string(&path).unwrap(), "second");
        // No stray temp files remain in the directory.
        let leftovers = fs::read_dir(path.parent().unwrap())
            .unwrap()
            .filter_map(Result::ok)
            .filter(|e| e.file_name().to_string_lossy().contains(".tmp"))
            .count();
        assert_eq!(leftovers, 0);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn config_save_then_load_is_stable() {
        let dir = unique_temp_dir("config");
        let path = dir.join("config.toml");
        let config = Config {
            start_at_login: true,
            enabled: false,
            ..Config::default()
        };
        save_config_to(&path, &config).unwrap();
        assert_eq!(load_config_from(&path), config);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn missing_config_loads_defaults() {
        let dir = unique_temp_dir("missing");
        let path = dir.join("does-not-exist.toml");
        assert_eq!(load_config_from(&path), Config::default());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn delete_all_state_removes_directory() {
        let dir = unique_temp_dir("delete");
        atomic_write(&dir.join("state").join("x.txt"), b"data").unwrap();
        assert!(dir.exists());
        delete_all_state_in(&dir).unwrap();
        assert!(!dir.exists());
        // Deleting an absent directory is not an error.
        delete_all_state_in(&dir).unwrap();
    }
}
