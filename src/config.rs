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
const MAX_CONDITIONS: usize = 32;
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

/// When an entry of [`Config::only_when`] is met.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum When {
    /// A matching process of the user is running.
    Running,
    /// A matching window is in the foreground.
    Focused,
}

/// An application that has to run, or be in the foreground, for muting.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Condition {
    /// Lowercase term, matched like the terms of [`Config::apps`].
    pub app: String,
    pub when: When,
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
    /// Muting only happens while one of these entries is met. Empty means
    /// always.
    pub only_when: Vec<Condition>,
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
            only_when: Vec::new(),
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
            .and_then(|s| Self::parse(&s))
            .unwrap_or_default()
    }

    /// Reads the contents of a configuration file. Conditions are read one by
    /// one, so an invalid entry does not discard the rest of the file.
    fn parse(text: &str) -> Option<Self> {
        let mut value: serde_json::Value = serde_json::from_str(text).ok()?;
        let only_when = value.as_object_mut()?.remove("only_when");
        let mut cfg: Config = serde_json::from_value(value).ok()?;
        cfg.only_when = only_when
            .as_ref()
            .and_then(serde_json::Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(|v| serde_json::from_value(v.clone()).ok())
            .collect();
        Some(cfg.normalized())
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
        self.only_when = clean_conditions(&self.only_when);
        self.only_when.retain(|c| valid_term(&c.app));
        self.only_when.truncate(MAX_CONDITIONS);
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
        if self.only_when.len() > MAX_CONDITIONS {
            return Err(format!("at most {MAX_CONDITIONS} conditions are supported"));
        }
        if !self.only_when.iter().all(|c| valid_term(&c.app)) {
            return Err("application names must have 1 to 64 characters".into());
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
        cfg.only_when = clean_conditions(&cfg.only_when);
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

/// Trims and lowercases the terms and keeps the first entry of each term.
/// Empty or long terms are kept, so that validation can reject them.
fn clean_conditions(list: &[Condition]) -> Vec<Condition> {
    let mut out: Vec<Condition> = Vec::new();
    for c in list {
        let app = c.app.trim().to_lowercase();
        if !out.iter().any(|o| o.app == app) {
            out.push(Condition { app, when: c.when });
        }
    }
    out
}

fn valid_term(term: &str) -> bool {
    (1..=MAX_APP_LEN).contains(&term.chars().count())
}

/// Parses the command line form of conditions: `cs2` or `running:cs2` for a
/// running application, `focused:cs2` for one in the foreground.
pub fn parse_conditions(args: &[String]) -> Result<Vec<Condition>, String> {
    args.iter()
        .map(|arg| {
            let (when, app) = match arg.split_once(':') {
                None => (When::Running, arg.as_str()),
                Some(("running", app)) => (When::Running, app),
                Some(("focused", app)) => (When::Focused, app),
                Some((prefix, _)) => {
                    return Err(format!(
                        "unknown prefix {prefix:?} in {arg:?}; use running: or focused:"
                    ));
                }
            };
            let app = app.trim().to_lowercase();
            if !valid_term(&app) {
                return Err(format!(
                    "application names must have 1 to {MAX_APP_LEN} characters ({arg:?})"
                ));
            }
            Ok(Condition { app, when })
        })
        .collect()
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
    use serde_json::{Value, json};

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

    fn cond(app: &str, when: When) -> Condition {
        Condition {
            app: app.into(),
            when,
        }
    }

    #[test]
    fn validates_conditions_strictly() {
        let cfg = Config::default();
        let entry = |app: &str| json!({ "app": app, "when": "running" });
        let set = |list: Value| cfg.merged(json!({ "only_when": list }));
        let many = |n: usize| {
            Value::from(
                (0..n)
                    .map(|i| entry(&format!("game{i}")))
                    .collect::<Vec<_>>(),
            )
        };
        assert!(set(many(33)).is_err());
        assert_eq!(set(many(32)).unwrap().only_when.len(), 32);
        assert!(set(json!([entry("  ")])).is_err());
        assert!(set(json!([entry(&"x".repeat(65))])).is_err());
        assert!(set(json!([{ "app": "cs2", "when": "foo" }])).is_err());
        assert!(set(json!([{ "app": "cs2" }])).is_err());
        assert!(set(json!([{ "app": "cs2", "when": "running", "extra": 1 }])).is_err());
        assert!(set(json!("cs2")).is_err());
        assert!(set(json!([42])).is_err());

        let ok = set(json!([
            { "app": " CS2 ", "when": "focused" },
            { "app": "cs2", "when": "running" },
            entry(&"x".repeat(64)),
        ]))
        .unwrap();
        assert_eq!(
            ok.only_when,
            [
                cond("cs2", When::Focused),
                cond(&"x".repeat(64), When::Running)
            ]
        );
        // A failed change leaves the configuration as it was.
        assert_eq!(ok.merged(json!({ "only_when": [entry("")] })).ok(), None);
        assert_eq!(ok.only_when.len(), 2);
        assert!(set(json!([])).unwrap().only_when.is_empty());
    }

    #[test]
    fn reads_conditions_from_old_and_edited_files() {
        let old = Config::parse(r#"{ "keys": ["BTN_EXTRA"], "apps": ["mumble"] }"#).unwrap();
        assert!(old.only_when.is_empty());
        assert_eq!(old.keys, ["BTN_EXTRA"]);
        assert_eq!(old.apps, ["mumble"]);

        let edited = Config::parse(
            &json!({
                "keys": ["BTN_EXTRA"],
                "only_when": [
                    { "app": "cs2", "when": "running" },
                    { "app": "doom", "when": "sometimes" },
                    { "app": "", "when": "running" },
                    { "app": "x".repeat(65), "when": "running" },
                    { "app": "quake", "when": "focused", "note": 1 },
                    "nonsense",
                    { "app": " EldenRing.exe ", "when": "focused" },
                    { "app": "CS2", "when": "focused" },
                ]
            })
            .to_string(),
        )
        .unwrap();
        assert_eq!(
            edited.only_when,
            [
                cond("cs2", When::Running),
                cond("eldenring.exe", When::Focused)
            ]
        );
        assert_eq!(edited.keys, ["BTN_EXTRA"]);

        let many: Vec<Value> = (0..40)
            .map(|i| json!({ "app": format!("game{i}"), "when": "running" }))
            .collect();
        let capped = Config::parse(&json!({ "only_when": many }).to_string()).unwrap();
        assert_eq!(capped.only_when.len(), 32);

        let not_a_list = Config::parse(r#"{ "only_when": "cs2", "mode": "toggle" }"#).unwrap();
        assert!(not_a_list.only_when.is_empty());
        assert_eq!(not_a_list.mode, Mode::Toggle);
    }

    #[test]
    fn parses_conditions_from_the_command_line() {
        let parse = |args: &[&str]| {
            parse_conditions(&args.iter().map(|s| s.to_string()).collect::<Vec<_>>())
        };
        assert_eq!(
            parse(&[
                "cs2",
                "running:Doom",
                "focused:eldenring.exe",
                "running:a:b"
            ])
            .unwrap(),
            [
                cond("cs2", When::Running),
                cond("doom", When::Running),
                cond("eldenring.exe", When::Focused),
                cond("a:b", When::Running),
            ]
        );
        assert!(parse(&["foo:cs2"]).is_err());
        assert!(parse(&["focused:"]).is_err());
        assert!(parse(&["  "]).is_err());
        assert_eq!(parse(&[]).unwrap(), []);
    }
}
