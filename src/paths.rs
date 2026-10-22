//! XDG base directories.

use std::io;
use std::path::PathBuf;

fn env_dir(var: &str) -> Option<PathBuf> {
    std::env::var_os(var)
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
}

pub fn home() -> io::Result<PathBuf> {
    env_dir("HOME").ok_or_else(|| io::Error::other("HOME is not set"))
}

pub fn config_home() -> io::Result<PathBuf> {
    env_dir("XDG_CONFIG_HOME").map_or_else(|| Ok(home()?.join(".config")), Ok)
}

pub fn data_home() -> io::Result<PathBuf> {
    env_dir("XDG_DATA_HOME").map_or_else(|| Ok(home()?.join(".local/share")), Ok)
}

/// Directory for the control socket. Falls back to the configuration
/// directory on systems without `XDG_RUNTIME_DIR`.
pub fn runtime_dir() -> io::Result<PathBuf> {
    env_dir("XDG_RUNTIME_DIR").map_or_else(|| Ok(config_home()?.join(crate::config::APP_DIR)), Ok)
}
