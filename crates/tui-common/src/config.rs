//! Optional on-disk settings.
//!
//! The file is read at startup and never written unless the user explicitly
//! asks. A missing file is fine and yields defaults; a malformed one is
//! reported and stops startup, so a typo shows up as an error rather than as
//! a setting that silently does nothing.

use std::path::PathBuf;

use color_eyre::eyre::{Context, Result};
use serde::{de::DeserializeOwned, Serialize};

/// `$XDG_CONFIG_HOME/<app>/config.toml`, or the platform equivalent.
pub fn path(app: &str) -> Option<PathBuf> {
    Some(dirs::config_dir()?.join(app).join("config.toml"))
}

/// Reads the config file. Returns the default when it does not exist, and an
/// error only when it exists but cannot be understood.
pub fn load<T: DeserializeOwned + Default>(app: &str) -> Result<T> {
    let Some(path) = path(app) else {
        return Ok(T::default());
    };
    let text = match std::fs::read_to_string(&path) {
        Ok(text) => text,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(T::default()),
        Err(e) => return Err(e).with_context(|| format!("reading {}", path.display())),
    };
    toml::from_str(&text).with_context(|| format!("parsing {}", path.display()))
}

pub fn save<T: Serialize>(app: &str, config: &T) -> Result<PathBuf> {
    let path = path(app).ok_or_else(|| color_eyre::eyre::eyre!("no config directory"))?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("creating {}", parent.display()))?;
    }
    let text = toml::to_string_pretty(config).context("serializing config")?;
    std::fs::write(&path, text).with_context(|| format!("writing {}", path.display()))?;
    Ok(path)
}
