//! This app's on-disk settings. The reading and writing is generic and lives
//! in `tui_common::config`; what is here is the shape of the file.

use std::path::PathBuf;

use color_eyre::eyre::Result;
use serde::{Deserialize, Serialize};

/// Directory under the platform config dir.
const APP: &str = "macro-tui";

#[derive(Debug, Default, Clone, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Config {
    /// Tab to open on, 1 for the board and 2 for news.
    pub tab: Option<u8>,
}

impl Config {
    pub fn load() -> Result<Self> {
        tui_common::config::load(APP)
    }

    pub fn save(&self) -> Result<PathBuf> {
        tui_common::config::save(APP, self)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_empty_config_is_valid() {
        assert_eq!(toml::from_str::<Config>("").unwrap(), Config::default());
    }

    #[test]
    fn round_trips() {
        let config = Config { tab: Some(2) };
        let text = toml::to_string_pretty(&config).unwrap();
        assert_eq!(toml::from_str::<Config>(&text).unwrap(), config);
    }

    /// A typo should be reported, not silently ignored, or the user will
    /// wonder why their setting does nothing.
    #[test]
    fn unknown_keys_are_rejected() {
        assert!(toml::from_str::<Config>("tabb = 2").is_err());
    }
}
