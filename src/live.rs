//! tuxhub live interface (protocol 1): the schema of the basic settings and
//! the mapping between its keys and [`Config`]. Keys, applications and
//! conditions stay in the tool's own interface.

use crate::config::{Config, MAX_RELEASE_DELAY_MS};
use serde_json::{Map, Value, json};

pub const PROTOCOL: u64 = 1;
/// Id of the tool in tuxhub, the name of the socket.
pub const TOOL_ID: &str = "push-to-mute";

/// Keys of the schema, in the order of the page.
pub const KEYS: [&str; 4] = ["enabled", "mode", "release_delay_ms", "start_at_login"];

/// A refused request; `field` names the setting it is about.
#[derive(Debug, Clone, PartialEq)]
pub struct Failure {
    pub message: String,
    pub field: Option<String>,
}

impl From<String> for Failure {
    fn from(message: String) -> Self {
        Self {
            message,
            field: None,
        }
    }
}

impl From<&str> for Failure {
    fn from(message: &str) -> Self {
        message.to_string().into()
    }
}

fn on_field(field: &str, message: impl Into<String>) -> Failure {
    Failure {
        message: message.into(),
        field: Some(field.to_string()),
    }
}

/// Answer to `hello`.
pub fn hello(params: &Value) -> Result<Value, Failure> {
    match params.get("protocol").and_then(Value::as_u64) {
        Some(PROTOCOL) => Ok(json!({
            "protocol": PROTOCOL,
            "name": "Push-to-Mute",
            "version": env!("CARGO_PKG_VERSION"),
        })),
        _ => Err(format!("only protocol {PROTOCOL} is supported").into()),
    }
}

/// Schema of the basic settings, in German for `de` and in English otherwise.
pub fn schema(locale: &str) -> Value {
    let de = locale == "de";
    let t = |en: &'static str, de_text: &'static str| if de { de_text } else { en };
    json!({
        "sections": [{
            "id": "general",
            "label": t("General", "Allgemein"),
            "fields": [
                {
                    "key": "enabled",
                    "type": "toggle",
                    "label": t("Enabled", "Aktiv"),
                    "help": t("Pause or resume muting", "Stummschalten pausieren oder fortsetzen"),
                    "default": true,
                },
                {
                    "key": "mode",
                    "type": "select",
                    "label": t("Mode", "Modus"),
                    "options": [
                        { "value": "hold", "label": t("Muted while the key is held", "Stumm, solange die Taste gedrückt ist") },
                        { "value": "toggle", "label": t("Each press switches", "Jeder Druck schaltet um") },
                    ],
                    "default": "hold",
                },
                {
                    "key": "release_delay_ms",
                    "type": "number",
                    "label": t("Release delay", "Verzögerung beim Loslassen"),
                    "help": t(
                        "Time the voice chat stays muted after the key is released",
                        "So lange bleibt der Sprachchat nach dem Loslassen stumm",
                    ),
                    "min": 0,
                    "max": MAX_RELEASE_DELAY_MS,
                    "step": 10,
                    "unit": "ms",
                    "default": 150,
                },
                {
                    "key": "start_at_login",
                    "type": "toggle",
                    "label": t("Start at login", "Bei der Anmeldung starten"),
                    "default": true,
                },
            ],
        }],
        "actions": [
            { "id": "toggle", "label": t("Pause or resume muting", "Stummschalten pausieren oder fortsetzen") },
        ],
    })
}

/// The values of the schema keys.
pub fn values(cfg: &Config) -> Map<String, Value> {
    let all = serde_json::to_value(cfg).unwrap_or_default();
    KEYS.iter()
        .map(|k| (k.to_string(), all.get(*k).cloned().unwrap_or(Value::Null)))
        .collect()
}

/// The configuration after `set {key, value}`, checked like every change.
pub fn set(cfg: &Config, params: &Value) -> Result<(Config, Value), Failure> {
    let key = params
        .get("key")
        .and_then(Value::as_str)
        .ok_or("`key` is missing")?;
    if !KEYS.contains(&key) {
        return Err(on_field(key, format!("unknown setting {key:?}")));
    }
    let value = params.get("value").cloned().unwrap_or(Value::Null);
    let fits = match key {
        "enabled" | "start_at_login" => value.is_boolean(),
        "mode" => matches!(value.as_str(), Some("hold" | "toggle")),
        "release_delay_ms" => value.as_u64().is_some(),
        _ => false,
    };
    if !fits {
        return Err(on_field(key, format!("invalid value for {key:?}")));
    }
    let next = cfg
        .merged(json!({ key: value }))
        .map_err(|e| on_field(key, e))?;
    let applied = values(&next).remove(key).unwrap_or(Value::Null);
    Ok((next, applied))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fields(schema: &Value) -> Vec<&Value> {
        schema["sections"]
            .as_array()
            .unwrap()
            .iter()
            .flat_map(|s| s["fields"].as_array().unwrap())
            .collect()
    }

    fn texts(v: &Value, out: &mut Vec<String>) {
        match v {
            Value::Object(o) => {
                for (k, v) in o {
                    if matches!(k.as_str(), "label" | "help") {
                        out.push(v.as_str().unwrap().to_string());
                    } else {
                        texts(v, out);
                    }
                }
            }
            Value::Array(a) => a.iter().for_each(|v| texts(v, out)),
            _ => {}
        }
    }

    /// Edge case 7: complete in both languages, English for unknown ones.
    #[test]
    fn schema_is_complete_in_german_and_english() {
        let (en, de) = (schema("en"), schema("de"));
        let keys: Vec<&str> = fields(&en)
            .iter()
            .map(|f| f["key"].as_str().unwrap())
            .collect();
        assert_eq!(keys, KEYS);
        let (mut en_texts, mut de_texts) = (Vec::new(), Vec::new());
        texts(&en, &mut en_texts);
        texts(&de, &mut de_texts);
        assert_eq!(en_texts.len(), de_texts.len());
        for (e, d) in en_texts.iter().zip(&de_texts) {
            assert!(!e.is_empty() && !d.is_empty());
            assert_ne!(e, d, "{e:?} is not translated");
        }
        assert_eq!(schema("fr"), en);
        assert_eq!(schema(""), en);
        // The defaults are those of a new configuration.
        let defaults = values(&Config::default());
        for f in fields(&en) {
            assert_eq!(
                f["default"],
                defaults[f["key"].as_str().unwrap()],
                "{}",
                f["key"]
            );
        }
    }

    #[test]
    fn values_have_exactly_the_schema_keys() {
        let v = values(&Config::default());
        assert_eq!(
            v.keys().map(String::as_str).collect::<Vec<_>>().len(),
            KEYS.len()
        );
        assert_eq!(
            Value::Object(v),
            json!({ "enabled": true, "mode": "hold", "release_delay_ms": 150, "start_at_login": true })
        );
    }

    /// Edge case 4: every refused value names its field and changes nothing.
    #[test]
    fn set_refuses_unknown_keys_wrong_types_and_values_out_of_range() {
        let cfg = Config::default();
        for (params, field) in [
            (json!({ "key": "keys", "value": [] }), "keys"),
            (json!({ "key": "nope", "value": 1 }), "nope"),
            (json!({ "key": "enabled", "value": "yes" }), "enabled"),
            (json!({ "key": "mode", "value": "always" }), "mode"),
            (
                json!({ "key": "release_delay_ms", "value": "100" }),
                "release_delay_ms",
            ),
            (
                json!({ "key": "release_delay_ms", "value": -1 }),
                "release_delay_ms",
            ),
            (
                json!({ "key": "release_delay_ms", "value": 1.5 }),
                "release_delay_ms",
            ),
            (
                json!({ "key": "release_delay_ms", "value": MAX_RELEASE_DELAY_MS + 1 }),
                "release_delay_ms",
            ),
        ] {
            let e = set(&cfg, &params).unwrap_err();
            assert_eq!(e.field.as_deref(), Some(field), "{params}");
        }
        assert!(
            set(&cfg, &json!({ "value": 1 }))
                .unwrap_err()
                .field
                .is_none()
        );
    }

    #[test]
    fn set_returns_the_new_configuration_and_value() {
        let (cfg, v) = set(
            &Config::default(),
            &json!({ "key": "release_delay_ms", "value": 300 }),
        )
        .unwrap();
        assert_eq!((cfg.release_delay_ms, v), (300, json!(300)));
        let (cfg, v) = set(&cfg, &json!({ "key": "mode", "value": "toggle" })).unwrap();
        assert_eq!(v, json!("toggle"));
        assert_eq!(values(&cfg)["release_delay_ms"], json!(300));
    }

    #[test]
    fn hello_needs_protocol_1() {
        assert_eq!(
            hello(&json!({ "protocol": 1 })).unwrap()["protocol"],
            json!(1)
        );
        assert!(hello(&json!({ "protocol": 2 })).is_err());
        assert!(hello(&json!({})).is_err());
    }
}
