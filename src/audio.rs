//! Microphone streams of applications, through PipeWire.
//!
//! `pw-dump --monitor` reports every change of the PipeWire graph, so new
//! streams (for example after joining a voice channel) are known immediately.
//! Streams are muted with `wpctl`, which affects only that application's
//! recording, not the microphone itself.

use serde::Serialize;
use serde_json::Value;
use std::collections::BTreeMap;
use std::io::{self, BufReader};
use std::os::unix::process::CommandExt;
use std::process::{Command, Stdio};
use std::time::Duration;

/// A recording stream of an application (`Stream/Input/Audio`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Stream {
    pub id: u32,
    /// Application name, for example `Discord` or `WEBRTC VoiceEngine`.
    pub name: String,
    /// Process name, for example `Discord` or `ts3client_linux_amd64`.
    pub binary: String,
    /// Flatpak app id, for example `com.discordapp.Discord`.
    pub app_id: String,
}

impl Stream {
    fn from_props(id: u32, props: &Value) -> Self {
        let get = |keys: &[&str]| {
            keys.iter()
                .find_map(|k| {
                    props
                        .get(*k)
                        .and_then(Value::as_str)
                        .filter(|s| !s.is_empty())
                })
                .unwrap_or_default()
                .to_string()
        };
        Self {
            id,
            name: get(&["application.name", "node.name", "media.name"]),
            binary: get(&["application.process.binary"]),
            app_id: get(&["pipewire.access.portal.app_id", "application.id"]),
        }
    }

    /// True if one of the lowercase `terms` occurs in the name, process name
    /// or app id.
    pub fn matches(&self, terms: &[String]) -> bool {
        let fields = [&self.name, &self.binary, &self.app_id].map(|s| s.to_lowercase());
        terms
            .iter()
            .any(|t| !t.is_empty() && fields.iter().any(|f| f.contains(t.as_str())))
    }

    /// The term the interface suggests for selecting this application.
    pub fn suggested_term(&self) -> String {
        let base = if !self.binary.is_empty() {
            &self.binary
        } else {
            &self.name
        };
        base.to_lowercase()
    }
}

pub enum Event {
    Streams(Vec<Stream>),
    /// PipeWire is not available; the text explains why.
    Unavailable(String),
}

/// Watches the recording streams on a background thread and reports the
/// complete list after every change.
pub fn watch(on_event: impl Fn(Event) + Send + 'static) {
    std::thread::Builder::new()
        .name("audio".into())
        .spawn(move || {
            loop {
                if let Err(e) = monitor(&on_event) {
                    on_event(Event::Unavailable(format!(
                        "PipeWire is not available ({e}). Push-to-Mute requires PipeWire with WirePlumber."
                    )));
                }
                std::thread::sleep(Duration::from_secs(3));
            }
        })
        .expect("spawn audio thread");
}

fn monitor(on_event: &impl Fn(Event)) -> io::Result<()> {
    let mut cmd = Command::new("pw-dump");
    cmd.args(["--monitor", "--no-colors"])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    // End pw-dump together with this process.
    unsafe {
        cmd.pre_exec(|| {
            libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGTERM);
            Ok(())
        });
    }
    let mut child = cmd.spawn()?;
    let stdout = child.stdout.take().expect("stdout is piped");
    let mut streams = BTreeMap::new();
    let values = serde_json::Deserializer::from_reader(BufReader::new(stdout)).into_iter::<Value>();
    for value in values {
        let Ok(value) = value else { break };
        if apply(&mut streams, &value) {
            on_event(Event::Streams(streams.values().cloned().collect()));
        }
    }
    let _ = child.kill();
    let status = child.wait()?;
    if !streams.is_empty() {
        on_event(Event::Streams(Vec::new()));
    }
    Err(io::Error::other(format!("pw-dump exited ({status})")))
}

/// Applies one `pw-dump` update to `streams`. Returns true if the list changed.
fn apply(streams: &mut BTreeMap<u32, Stream>, update: &Value) -> bool {
    let mut changed = false;
    for obj in update.as_array().into_iter().flatten() {
        let Some(id) = obj
            .get("id")
            .and_then(Value::as_u64)
            .and_then(|i| u32::try_from(i).ok())
        else {
            continue;
        };
        let info = obj.get("info").unwrap_or(&Value::Null);
        if info.is_null() {
            // The object was removed.
            changed |= streams.remove(&id).is_some();
            continue;
        }
        if obj.get("type").and_then(Value::as_str) != Some("PipeWire:Interface:Node") {
            continue;
        }
        // Updates contain only the changed parts; without props the stream
        // properties are unchanged.
        let Some(props) = info.get("props").filter(|p| p.is_object()) else {
            continue;
        };
        if props.get("media.class").and_then(Value::as_str) == Some("Stream/Input/Audio") {
            let s = Stream::from_props(id, props);
            if streams.get(&id) != Some(&s) {
                streams.insert(id, s);
                changed = true;
            }
        } else {
            changed |= streams.remove(&id).is_some();
        }
    }
    changed
}

/// Mutes or unmutes the given streams. All commands run in parallel.
pub fn set_mute(ids: &[u32], mute: bool) -> Vec<(u32, io::Result<()>)> {
    let value = if mute { "1" } else { "0" };
    let children: Vec<_> = ids
        .iter()
        .map(|&id| {
            let child = Command::new("wpctl")
                .args(["set-mute", &id.to_string(), value])
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn();
            (id, child)
        })
        .collect();
    children
        .into_iter()
        .map(|(id, child)| {
            let res = child.and_then(|mut c| c.wait()).and_then(|status| {
                if status.success() {
                    Ok(())
                } else {
                    Err(io::Error::other(format!(
                        "wpctl failed for stream {id} ({status})"
                    )))
                }
            });
            (id, res)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn node(id: u32, class: &str, props: Value) -> Value {
        let mut p = props;
        p["media.class"] = json!(class);
        json!({ "id": id, "type": "PipeWire:Interface:Node", "info": { "props": p } })
    }

    #[test]
    fn tracks_recording_streams() {
        let mut s = BTreeMap::new();
        let first = json!([
            node(60, "Stream/Input/Audio", json!({
                "application.name": "WEBRTC VoiceEngine",
                "application.process.binary": "Discord",
                "pipewire.access.portal.app_id": "com.discordapp.Discord"
            })),
            node(160, "Stream/Input/Audio", json!({ "application.name": "cs2" })),
            node(70, "Audio/Source", json!({ "node.name": "alsa_input.usb" })),
            { "id": 5, "type": "PipeWire:Interface:Client", "info": { "props": {} } }
        ]);
        assert!(apply(&mut s, &first));
        assert_eq!(s.keys().copied().collect::<Vec<_>>(), [60, 160]);
        assert_eq!(s[&60].binary, "Discord");
        assert_eq!(s[&60].app_id, "com.discordapp.Discord");

        // Partial update without props: nothing changes.
        assert!(!apply(
            &mut s,
            &json!([{ "id": 60, "type": "PipeWire:Interface:Node", "info": { "params": {} } }])
        ));
        // Removal.
        assert!(apply(&mut s, &json!([{ "id": 160, "info": null }])));
        assert_eq!(s.len(), 1);
        assert!(!apply(&mut s, &json!([{ "id": 999, "info": null }])));
    }

    #[test]
    fn matches_terms_case_insensitively() {
        let discord = Stream {
            id: 1,
            name: "WEBRTC VoiceEngine".into(),
            binary: "Discord".into(),
            app_id: "com.discordapp.Discord".into(),
        };
        let game = Stream {
            id: 2,
            name: "cs2".into(),
            binary: String::new(),
            app_id: String::new(),
        };
        let terms = vec!["discord".to_string(), "mumble".to_string()];
        assert!(discord.matches(&terms));
        assert!(!game.matches(&terms));
        assert!(!discord.matches(&[String::new()]));
        assert_eq!(discord.suggested_term(), "discord");
        assert_eq!(game.suggested_term(), "cs2");
    }
}
