//! Persistent configuration, shared by the daemon, the window, tuxhub and the
//! command line. Only the daemon writes it.

use evdev::KeyCode;
use serde::{Deserialize, Serialize};
use std::io;
use std::path::PathBuf;
use std::str::FromStr;

pub const APP_DIR: &str = "tuxhub-push-to-mute";

const MAX_KEYS: usize = 8;
const MAX_APPS: usize = 32;
const MAX_APP_LEN: usize = 64;
pub const MAX_RELEASE_DELAY_MS: u64 = 2000;

/// Applications muted by default. Matched case-insensitively against the
/// application name, the process name and the Flatpak app id of each stream.
pub const DEFAULT_APPS: &[&str] = &[
    "discord",
    "vesktop",
    "webcord",
    "teamspeak",
    "ts3client",
    "mumble",
];

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, clap::ValueEnum)]
#[serde(rename_all = "lowercase")]
pub enum Mode {
    /// Muted while a key is held.
    Hold,
    /// Each press switches between muted and unmuted.
    Toggle,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    pub enabled: bool,
    /// Key names as defined by the kernel, for example `BTN_EXTRA` or `KEY_V`.
    pub keys: Vec<String>,
    /// Lowercase terms that select the applications to mute.
    pub apps: Vec<String>,
    pub mode: Mode,
    /// Time the microphone stays muted after the key is released.
    pub release_delay_ms: u64,
    pub start_at_login: bool,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            enabled: true,
            keys: Vec::new(),
            apps: DEFAULT_APPS.iter().map(|s| s.to_string()).collect(),
            mode: Mode::Hold,
            release_delay_ms: 150,
            start_at_login: true,
        }
    }
}

impl Config {
    pub fn path() -> io::Result<PathBuf> {
        Ok(crate::paths::config_home()?
            .join(APP_DIR)
            .join("config.json"))
    }

    /// Loads the configuration. A missing or unreadable file yields the defaults.
    pub fn load() -> Self {
        Self::path()
            .ok()
            .and_then(|p| std::fs::read_to_string(p).ok())
            .and_then(|s| serde_json::from_str::<Config>(&s).ok())
            .map(Config::normalized)
            .unwrap_or_default()
    }

    pub fn save(&self) -> io::Result<()> {
        let path = Self::path()?;
        std::fs::create_dir_all(path.parent().unwrap())?;
        let tmp = path.with_extension("json.tmp");
        std::fs::write(&tmp, serde_json::to_string_pretty(self).unwrap() + "\n")?;
        std::fs::rename(tmp, path)
    }

    /// Drops invalid entries instead of failing, for files edited by hand.
    fn normalized(mut self) -> Self {
        self.keys.retain(|k| parse_key(k).is_some());
        self.keys.dedup();
        self.keys.truncate(MAX_KEYS);
        self.apps = clean_apps(&self.apps);
        self.release_delay_ms = self.release_delay_ms.min(MAX_RELEASE_DELAY_MS);
        self
    }

    /// Strict validation for changes requested through the interface.
    pub fn validate(&self) -> Result<(), String> {
        if self.keys.len() > MAX_KEYS {
            return Err(format!("at most {MAX_KEYS} keys are supported"));
        }
        for k in &self.keys {
            if parse_key(k).is_none() {
                return Err(format!("unknown key {k:?}"));
            }
        }
        if self.apps.len() > MAX_APPS {
            return Err(format!("at most {MAX_APPS} applications are supported"));
        }
        if self
            .apps
            .iter()
            .any(|a| a.trim().is_empty() || a.len() > MAX_APP_LEN)
        {
            return Err("application names must have 1 to 64 characters".into());
        }
        if self.release_delay_ms > MAX_RELEASE_DELAY_MS {
            return Err(format!(
                "the release delay is limited to {MAX_RELEASE_DELAY_MS} ms"
            ));
        }
        Ok(())
    }

    /// Applies the fields present in `patch` and validates the result.
    pub fn merged(&self, patch: serde_json::Value) -> Result<Self, String> {
        let mut value = serde_json::to_value(self).unwrap();
        let (Some(obj), Some(patch)) = (value.as_object_mut(), patch.as_object()) else {
            return Err("expected an object".into());
        };
        for (k, v) in patch {
            if !obj.contains_key(k) {
                return Err(format!("unknown setting {k:?}"));
            }
            obj.insert(k.clone(), v.clone());
        }
        let mut cfg: Config =
            serde_json::from_value(value).map_err(|e| format!("invalid settings: {e}"))?;
        cfg.apps = clean_apps(&cfg.apps);
        cfg.keys.dedup();
        cfg.validate()?;
        Ok(cfg)
    }

    pub fn key_codes(&self) -> Vec<KeyCode> {
        self.keys.iter().filter_map(|k| parse_key(k)).collect()
    }
}

fn clean_apps(apps: &[String]) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for a in apps {
        let a = a.trim().to_lowercase();
        if !a.is_empty() && a.len() <= MAX_APP_LEN && !out.contains(&a) {
            out.push(a);
        }
    }
    out.truncate(MAX_APPS);
    out
}

/// Parses a kernel key name such as `BTN_EXTRA` or `KEY_V`.
pub fn parse_key(name: &str) -> Option<KeyCode> {
    let code = KeyCode::from_str(name).ok()?;
    usable_key(code).then_some(code)
}

/// Keys that may serve as a trigger. The primary mouse buttons are excluded,
/// as they would mute the microphone on every click.
pub fn usable_key(code: KeyCode) -> bool {
    let c = code.code();
    let named = format!("{code:?}");
    (named.starts_with("KEY_") || named.starts_with("BTN_"))
        && code != KeyCode::BTN_LEFT
        && code != KeyCode::BTN_RIGHT
        && c != 0
}

pub fn key_name(code: KeyCode) -> String {
    format!("{code:?}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parses_key_names() {
        assert_eq!(parse_key("BTN_EXTRA"), Some(KeyCode::BTN_EXTRA));
        assert_eq!(parse_key("KEY_V"), Some(KeyCode::KEY_V));
        assert_eq!(parse_key("BTN_LEFT"), None);
        assert_eq!(parse_key("NOT_A_KEY"), None);
        assert_eq!(key_name(KeyCode::BTN_SIDE), "BTN_SIDE");
    }

    #[test]
    fn merges_partial_changes() {
        let cfg = Config::default();
        let cfg = cfg
            .merged(json!({ "keys": ["BTN_EXTRA"], "apps": [" Discord ", "discord", "mumble"] }))
            .unwrap();
        assert_eq!(cfg.keys, ["BTN_EXTRA"]);
        assert_eq!(cfg.apps, ["discord", "mumble"]);
        assert!(cfg.merged(json!({ "keys": ["BTN_LEFT"] })).is_err());
        assert!(cfg.merged(json!({ "release_delay_ms": 5000 })).is_err());
        assert!(cfg.merged(json!({ "unknown": 1 })).is_err());
        assert!(cfg.merged(json!({ "mode": "toggle" })).unwrap().mode == Mode::Toggle);
    }

    #[test]
    fn normalizes_files_edited_by_hand() {
        let cfg: Config = serde_json::from_value(json!({
            "keys": ["BTN_EXTRA", "BOGUS"], "apps": ["", "TeamSpeak"], "release_delay_ms": 99999
        }))
        .unwrap();
        let cfg = cfg.normalized();
        assert_eq!(cfg.keys, ["BTN_EXTRA"]);
        assert_eq!(cfg.apps, ["teamspeak"]);
        assert_eq!(cfg.release_delay_ms, MAX_RELEASE_DELAY_MS);
        assert!(cfg.enabled);
    }
}
