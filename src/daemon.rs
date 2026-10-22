//! The background service. It owns the configuration, watches the trigger
//! keys and mutes the selected applications.
//!
//! Exactly one service runs per user. The window, tuxhub and the command line
//! talk to it over a Unix socket with the same line-based JSON-RPC protocol
//! that tuxhub uses, so all of them always show the same state.

use crate::audio::{self, Stream};
use crate::config::{self, Config, Mode};
use crate::input::{self, Listener};
use crate::{integration, paths};
use serde_json::{Value, json};
use std::collections::BTreeSet;
use std::io::{self, BufRead, BufReader, Write};
use std::os::fd::AsRawFd;
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{Receiver, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

const SOCKET_NAME: &str = "tuxhub-push-to-mute.sock";
const LOCK_NAME: &str = "tuxhub-push-to-mute.lock";
const LEARN_TIMEOUT: Duration = Duration::from_secs(15);
const EXE_CHECK_INTERVAL: Duration = Duration::from_secs(2);

pub fn socket_path() -> io::Result<PathBuf> {
    Ok(paths::runtime_dir()?.join(SOCKET_NAME))
}

enum Msg {
    Input(input::Event),
    Audio(audio::Event),
    Request {
        method: String,
        params: Value,
        reply: Sender<Result<Value, String>>,
    },
    Terminate,
}

/// Runs the service until it is terminated. Returns `Ok(false)` if another
/// instance is already running.
pub fn run() -> io::Result<bool> {
    let dir = paths::runtime_dir()?;
    std::fs::create_dir_all(&dir)?;
    let Some(lock) = try_lock(&dir.join(LOCK_NAME))? else {
        return Ok(false);
    };
    let socket = socket_path()?;
    let _ = std::fs::remove_file(&socket);
    let server = UnixListener::bind(&socket)?;
    std::fs::set_permissions(&socket, std::fs::Permissions::from_mode(0o600))?;

    let (tx, rx) = std::sync::mpsc::channel();
    {
        let tx = tx.clone();
        ctrlc::set_handler(move || {
            let _ = tx.send(Msg::Terminate);
        })
        .map_err(io::Error::other)?;
    }

    let cfg = Config::load();
    if let Err(e) = integration::refresh(&cfg) {
        eprintln!("desktop integration: {e}");
    }
    let listener = {
        let tx = tx.clone();
        Listener::spawn(cfg.key_codes(), move |e| {
            let _ = tx.send(Msg::Input(e));
        })
    };
    {
        let tx = tx.clone();
        audio::watch(move |e| {
            let _ = tx.send(Msg::Audio(e));
        });
    }
    let clients = Clients::default();
    {
        let clients = clients.clone();
        std::thread::Builder::new()
            .name("accept".into())
            .spawn(move || accept(server, clients, tx))?;
    }

    let mut service = Service::new(cfg, listener, clients);
    let restart = service.run(rx);
    service.set_muted(false);
    let _ = std::fs::remove_file(&socket);
    drop(lock);
    if restart {
        exec_self();
    }
    Ok(true)
}

/// Takes an exclusive lock that is released when the process exits.
fn try_lock(path: &PathBuf) -> io::Result<Option<std::fs::File>> {
    let file = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(path)?;
    let ok = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } == 0;
    Ok(ok.then_some(file))
}

/// Replaces the process with the current executable, after an update.
fn exec_self() {
    use std::os::unix::process::CommandExt;
    if let Ok(exe) = std::env::current_exe() {
        let err = std::process::Command::new(exe).arg("daemon").exec();
        eprintln!("could not restart after an update: {err}");
    }
}

// ----------------------------------------------------------------- Clients

type Writer = Arc<Mutex<UnixStream>>;

#[derive(Clone, Default)]
struct Clients(Arc<Mutex<Vec<(u64, Writer)>>>);

impl Clients {
    fn add(&self, id: u64, w: Writer) {
        self.0.lock().unwrap().push((id, w));
    }

    fn remove(&self, id: u64) {
        self.0.lock().unwrap().retain(|(i, _)| *i != id);
    }

    fn notify(&self, method: &str, params: Value) {
        let line = format!("{}\n", json!({ "method": method, "params": params }));
        self.0
            .lock()
            .unwrap()
            .retain(|(_, w)| w.lock().unwrap().write_all(line.as_bytes()).is_ok());
    }
}

fn accept(server: UnixListener, clients: Clients, tx: Sender<Msg>) {
    static NEXT: AtomicU64 = AtomicU64::new(1);
    for stream in server.incoming() {
        let Ok(stream) = stream else { continue };
        let (Ok(read), true) = (
            stream.try_clone(),
            stream
                .set_write_timeout(Some(Duration::from_secs(1)))
                .is_ok(),
        ) else {
            continue;
        };
        let id = NEXT.fetch_add(1, Ordering::Relaxed);
        let writer: Writer = Arc::new(Mutex::new(stream));
        clients.add(id, writer.clone());
        let (clients, tx) = (clients.clone(), tx.clone());
        let _ = std::thread::Builder::new()
            .name(format!("client-{id}"))
            .spawn(move || {
                serve_client(read, &writer, &tx);
                clients.remove(id);
            });
    }
}

fn serve_client(read: UnixStream, writer: &Writer, tx: &Sender<Msg>) {
    for line in BufReader::new(read).lines() {
        let Ok(line) = line else { break };
        let Ok(msg) = serde_json::from_str::<Value>(&line) else {
            continue;
        };
        let id = msg.get("id").cloned();
        let method = msg
            .get("method")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        let params = msg.get("params").cloned().unwrap_or(Value::Null);
        let (rtx, rrx) = std::sync::mpsc::channel();
        if tx
            .send(Msg::Request {
                method,
                params,
                reply: rtx,
            })
            .is_err()
        {
            break;
        }
        let result = rrx
            .recv_timeout(Duration::from_secs(4))
            .unwrap_or_else(|_| Err("the service did not respond".into()));
        if let Some(id) = id {
            let reply = match result {
                Ok(r) => json!({ "id": id, "result": r }),
                Err(e) => json!({ "id": id, "error": { "message": e } }),
            };
            if writeln!(writer.lock().unwrap(), "{reply}").is_err() {
                break;
            }
        }
    }
}

// ----------------------------------------------------------------- Service

struct Service {
    cfg: Config,
    listener: Listener,
    clients: Clients,
    streams: Vec<Stream>,
    audio_error: Option<String>,
    devices: usize,
    input_readable: bool,
    held: bool,
    muted: bool,
    /// Streams muted by this service, so only those are unmuted again.
    muted_ids: BTreeSet<u32>,
    mute_error: Option<String>,
    unmute_at: Option<Instant>,
    learn_until: Option<Instant>,
    last_status: Value,
    exe: Option<(PathBuf, (u64, u64))>,
    exe_checked: Instant,
}

/// Device and inode of a file, to detect when an update replaced it.
fn file_id(path: &PathBuf) -> Option<(u64, u64)> {
    use std::os::unix::fs::MetadataExt;
    std::fs::metadata(path).ok().map(|m| (m.dev(), m.ino()))
}

impl Service {
    fn new(cfg: Config, listener: Listener, clients: Clients) -> Self {
        let exe = std::env::current_exe()
            .ok()
            .and_then(|p| Some((p.clone(), file_id(&p)?)));
        Self {
            cfg,
            listener,
            clients,
            streams: Vec::new(),
            audio_error: None,
            devices: 0,
            input_readable: true,
            held: false,
            muted: false,
            muted_ids: BTreeSet::new(),
            mute_error: None,
            unmute_at: None,
            learn_until: None,
            last_status: Value::Null,
            exe,
            exe_checked: Instant::now(),
        }
    }

    /// Processes messages until the service ends. Returns true if the
    /// executable was replaced and the service should restart itself.
    fn run(&mut self, rx: Receiver<Msg>) -> bool {
        loop {
            let now = Instant::now();
            let next = [
                self.unmute_at,
                self.learn_until,
                Some(self.exe_checked + EXE_CHECK_INTERVAL),
            ]
            .into_iter()
            .flatten()
            .min()
            .unwrap();
            match rx.recv_timeout(next.saturating_duration_since(now)) {
                Ok(Msg::Terminate) | Err(RecvTimeoutError::Disconnected) => return false,
                Ok(Msg::Request {
                    method,
                    params,
                    reply,
                }) => {
                    if method == "quit" {
                        let _ = reply.send(Ok(json!(true)));
                        return false;
                    }
                    let _ = reply.send(self.request(&method, params));
                }
                Ok(Msg::Input(e)) => self.input(e),
                Ok(Msg::Audio(e)) => self.audio(e),
                Err(RecvTimeoutError::Timeout) => {}
            }
            let now = Instant::now();
            if self.unmute_at.is_some_and(|t| now >= t) {
                self.unmute_at = None;
                self.set_muted(false);
            }
            if self.learn_until.is_some_and(|t| now >= t) {
                self.stop_learning(None);
            }
            if now >= self.exe_checked + EXE_CHECK_INTERVAL {
                self.exe_checked = now;
                if self.exe_replaced() {
                    return true;
                }
            }
            self.publish_status();
        }
    }

    fn exe_replaced(&self) -> bool {
        let Some((path, id)) = &self.exe else {
            return false;
        };
        // A missing file means the tool is being removed, not updated.
        file_id(path).is_some_and(|now| now != *id)
    }

    fn input(&mut self, e: input::Event) {
        match e {
            input::Event::Devices { watched, readable } => {
                self.devices = watched;
                self.input_readable = readable;
            }
            input::Event::Learned(code) => {
                let key = (code != evdev::KeyCode::KEY_ESC).then(|| config::key_name(code));
                self.stop_learning(key);
            }
            input::Event::Pressed => {
                self.held = true;
                if !self.cfg.enabled {
                    return;
                }
                match self.cfg.mode {
                    Mode::Hold => {
                        self.unmute_at = None;
                        self.set_muted(true);
                    }
                    Mode::Toggle => {
                        let m = !self.muted;
                        self.set_muted(m);
                    }
                }
            }
            input::Event::Released => {
                self.held = false;
                if self.cfg.enabled && self.cfg.mode == Mode::Hold && self.muted {
                    let delay = Duration::from_millis(self.cfg.release_delay_ms);
                    if delay.is_zero() {
                        self.set_muted(false);
                    } else {
                        self.unmute_at = Some(Instant::now() + delay);
                    }
                }
            }
        }
    }

    fn audio(&mut self, e: audio::Event) {
        match e {
            audio::Event::Streams(s) => {
                self.audio_error = None;
                self.streams = s;
                let gone: Vec<u32> = self
                    .muted_ids
                    .iter()
                    .copied()
                    .filter(|id| !self.streams.iter().any(|s| s.id == *id))
                    .collect();
                for id in gone {
                    self.muted_ids.remove(&id);
                }
                // Mute streams that appear while the key is held.
                self.apply_mute();
                self.clients.notify("streams", self.stream_list());
            }
            audio::Event::Unavailable(text) => {
                self.audio_error = Some(text);
                self.streams.clear();
                self.muted_ids.clear();
            }
        }
    }

    fn set_muted(&mut self, muted: bool) {
        self.muted = muted;
        if !muted {
            self.unmute_at = None;
        }
        self.apply_mute();
    }

    /// Brings the streams in line with the desired state.
    fn apply_mute(&mut self) {
        let target: BTreeSet<u32> = if self.muted {
            self.streams
                .iter()
                .filter(|s| s.matches(&self.cfg.apps))
                .map(|s| s.id)
                .collect()
        } else {
            BTreeSet::new()
        };
        let mute: Vec<u32> = target.difference(&self.muted_ids).copied().collect();
        let unmute: Vec<u32> = self.muted_ids.difference(&target).copied().collect();
        let mut errors = Vec::new();
        for (id, res) in audio::set_mute(&mute, true) {
            match res {
                Ok(()) => {
                    self.muted_ids.insert(id);
                }
                Err(e) => errors.push(e.to_string()),
            }
        }
        for (id, res) in audio::set_mute(&unmute, false) {
            // Forget the stream either way; it may have disappeared meanwhile.
            self.muted_ids.remove(&id);
            if let Err(e) = res {
                errors.push(e.to_string());
            }
        }
        if !mute.is_empty() || !unmute.is_empty() {
            self.mute_error = errors.into_iter().next();
        }
    }

    fn stop_learning(&mut self, key: Option<String>) {
        if self.learn_until.take().is_some() {
            self.listener.learn(false);
            self.clients.notify("learned", json!({ "key": key }));
        }
    }

    fn stream_list(&self) -> Value {
        json!(
            self.streams
                .iter()
                .map(|s| json!({
                    "id": s.id,
                    "name": s.name,
                    "binary": s.binary,
                    "app_id": s.app_id,
                    "term": s.suggested_term(),
                    "matched": s.matches(&self.cfg.apps),
                    "muted": self.muted_ids.contains(&s.id),
                }))
                .collect::<Vec<_>>()
        )
    }

    fn status(&self) -> Value {
        let matched: Vec<&Stream> = self
            .streams
            .iter()
            .filter(|s| s.matches(&self.cfg.apps))
            .collect();
        let names: Vec<String> = matched
            .iter()
            .map(|s| {
                if s.binary.is_empty() {
                    s.name.clone()
                } else {
                    s.binary.clone()
                }
            })
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect();
        // `state` is for the interface; `text` is an English fallback.
        let (state, text) = if !self.cfg.enabled {
            ("paused", "Paused".to_string())
        } else if self.cfg.keys.is_empty() {
            ("no-key", "No push-to-talk key is set".to_string())
        } else if !self.input_readable {
            ("no-access", "No access to input devices".to_string())
        } else if let Some(e) = &self.audio_error {
            ("no-audio", e.clone())
        } else if self.muted {
            ("muted", format!("Muted: {}", names.join(", ")))
        } else {
            ("active", "Active".to_string())
        };
        json!({
            "running": state == "active" || state == "muted",
            "state": state,
            "text": text,
            "enabled": self.cfg.enabled,
            "muted": self.muted,
            "held": self.held,
            "devices": self.devices,
            "matched": names,
            "error": self.mute_error,
            "learning": self.learn_until.is_some(),
            "version": env!("CARGO_PKG_VERSION"),
        })
    }

    fn publish_status(&mut self) {
        let s = self.status();
        if s != self.last_status {
            self.clients.notify("status", s.clone());
            self.last_status = s;
        }
    }

    fn set_config(&mut self, cfg: Config) -> Result<(), String> {
        if cfg == self.cfg {
            return Ok(());
        }
        cfg.save()
            .map_err(|e| format!("could not save the settings: {e}"))?;
        let old = std::mem::replace(&mut self.cfg, cfg);
        if old.keys != self.cfg.keys {
            self.listener.set_keys(self.cfg.key_codes());
        }
        if old.start_at_login != self.cfg.start_at_login
            && let Err(e) = integration::refresh(&self.cfg)
        {
            eprintln!("desktop integration: {e}");
        }
        if !self.cfg.enabled || old.mode != self.cfg.mode {
            self.set_muted(false);
        } else if old.apps != self.cfg.apps {
            self.apply_mute();
        }
        self.clients.notify("config", json!(self.cfg));
        self.clients.notify("streams", self.stream_list());
        Ok(())
    }

    fn request(&mut self, method: &str, params: Value) -> Result<Value, String> {
        match method {
            // Settings stored by tuxhub; this tool keeps its own configuration.
            "configure" => Ok(json!(true)),
            "status" => Ok(self.status()),
            "config.get" => Ok(json!(self.cfg)),
            "config.set" => {
                let cfg = self.cfg.merged(params)?;
                self.set_config(cfg)?;
                Ok(json!(self.cfg))
            }
            "streams" => Ok(self.stream_list()),
            "learn.start" => {
                self.learn_until = Some(Instant::now() + LEARN_TIMEOUT);
                self.listener.learn(true);
                Ok(json!(true))
            }
            "learn.cancel" => {
                self.stop_learning(None);
                Ok(json!(true))
            }
            "action" => {
                let id = params.get("id").and_then(Value::as_str).unwrap_or_default();
                if id == "setup" {
                    // The password dialog can take a while; do not block the service.
                    let (listener, clients, keys) = (
                        self.listener.clone(),
                        self.clients.clone(),
                        self.cfg.key_codes(),
                    );
                    std::thread::spawn(move || {
                        let result = integration::setup_input_access();
                        listener.set_keys(keys);
                        clients.notify(
                            "setup",
                            json!({ "error": result.err().map(|e| e.to_string()) }),
                        );
                    });
                    return Ok(json!(true));
                }
                let enabled = match id {
                    "toggle" => !self.cfg.enabled,
                    "enable" => true,
                    "disable" => false,
                    other => return Err(format!("unknown action {other:?}")),
                };
                let cfg = self.cfg.merged(json!({ "enabled": enabled }))?;
                self.set_config(cfg)?;
                Ok(json!(self.cfg.enabled))
            }
            other => Err(format!("unknown method {other:?}")),
        }
    }
}
