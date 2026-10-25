//! The background service. It owns the configuration, watches the trigger
//! keys and mutes the selected applications.
//!
//! Exactly one service runs per user. The window, tuxhub and the command line
//! talk to it over the Unix socket of the tuxhub live interface
//! (`$XDG_RUNTIME_DIR/tuxhub/push-to-mute.sock`, one JSON object per line), so
//! all of them always show the same state.

use crate::audio::{self, Stream};
use crate::conditions::{self, ProcNames, Snapshot};
use crate::config::{self, Config, Mode};
use crate::input::{self, Listener};
use crate::live::{self, Failure};
use crate::{integration, kwin, paths};
use serde_json::{Value, json};
use std::collections::BTreeSet;
use std::io::{self, BufRead, BufReader, Read, Write};
use std::os::fd::AsRawFd;
use std::os::unix::fs::{FileTypeExt, PermissionsExt};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{Receiver, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

const LOCK_NAME: &str = "tuxhub-push-to-mute.lock";
/// Longest line a client may send, without the line break.
const MAX_LINE: usize = 1 << 20;
const LEARN_TIMEOUT: Duration = Duration::from_secs(15);
const EXE_CHECK_INTERVAL: Duration = Duration::from_secs(2);
const SCAN_INTERVAL: Duration = Duration::from_secs(2);

pub fn socket_path() -> io::Result<PathBuf> {
    Ok(paths::runtime_dir()?
        .join("tuxhub")
        .join(format!("{}.sock", live::TOOL_ID)))
}

/// Listens on `path`: creates its folder with mode 0700, replaces a socket
/// that nobody listens on, and never removes anything else.
fn bind(path: &std::path::Path) -> io::Result<UnixListener> {
    let dir = path
        .parent()
        .ok_or_else(|| io::Error::other("invalid socket path"))?;
    match std::fs::symlink_metadata(dir) {
        Ok(m) if m.is_dir() => {}
        Ok(_) => {
            return Err(io::Error::other(format!(
                "{} is not a folder",
                dir.display()
            )));
        }
        Err(_) => {
            std::fs::create_dir_all(dir)?;
            std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))?;
        }
    }
    match std::fs::symlink_metadata(path) {
        Ok(m) if m.file_type().is_socket() => {
            if UnixStream::connect(path).is_ok() {
                return Err(io::Error::new(
                    io::ErrorKind::AddrInUse,
                    format!("{} is in use", path.display()),
                ));
            }
            std::fs::remove_file(path)?;
        }
        Ok(_) => {
            return Err(io::Error::other(format!(
                "{} exists and is not a socket; remove it to start the service",
                path.display()
            )));
        }
        Err(_) => {}
    }
    let server = UnixListener::bind(path)?;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
    Ok(server)
}

enum Msg {
    Input(input::Event),
    Audio(audio::Event),
    /// Windows reported by KWin, `None` once KWin is gone.
    Windows(Option<Snapshot>),
    Request {
        /// Connection the request came from.
        client: u64,
        method: String,
        params: Value,
        reply: Sender<Result<Value, Failure>>,
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
    let server = bind(&socket)?;

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
    {
        let tx = tx.clone();
        kwin::watch(move |w| {
            let _ = tx.send(Msg::Windows(w));
        });
    }
    let clients = Clients::default();
    {
        let clients = clients.clone();
        std::thread::Builder::new()
            .name("accept".into())
            .spawn(move || accept(server, clients, tx))?;
    }

    let mut service = Service::new(cfg, listener, clients, PathBuf::from(conditions::PROC));
    let restart = service.run(rx);
    service.set_muted(false);
    kwin::unload();
    let _ = std::fs::remove_file(&socket);
    drop(lock);
    if restart {
        exec_self(service.exe.as_ref().map(|(p, _)| p));
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
/// `path` is the executable as it was at start. `current_exe()` cannot be
/// used here: after an update it names the deleted old file.
fn exec_self(path: Option<&PathBuf>) {
    use std::os::unix::process::CommandExt;
    if let Some(exe) = path {
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
        self.notify_except(None, method, params);
    }

    /// Notifies every connection but `except`.
    fn notify_except(&self, except: Option<u64>, method: &str, params: Value) {
        let line = format!("{}\n", json!({ "method": method, "params": params }));
        self.0.lock().unwrap().retain(|(id, w)| {
            Some(*id) == except || w.lock().unwrap().write_all(line.as_bytes()).is_ok()
        });
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
                serve_client(id, read, &writer, &tx);
                clients.remove(id);
                // Wakes up a client that is still blocked on its socket.
                let _ = writer.lock().unwrap().shutdown(std::net::Shutdown::Both);
            });
    }
}

/// Reads one line of at most [`MAX_LINE`] bytes; `None` at the end, on an
/// error or for a longer line.
fn read_line(reader: &mut impl BufRead, buf: &mut Vec<u8>) -> Option<()> {
    buf.clear();
    let n = reader
        .take(MAX_LINE as u64 + 1)
        .read_until(b'\n', buf)
        .ok()?;
    if n == 0 || buf.last() != Some(&b'\n') {
        return None;
    }
    buf.pop();
    Some(())
}

/// Answers the requests of one connection. A line that is too long or not a
/// JSON object closes it.
fn serve_client(client: u64, read: UnixStream, writer: &Writer, tx: &Sender<Msg>) {
    let mut reader = BufReader::new(read);
    let mut buf = Vec::new();
    while read_line(&mut reader, &mut buf).is_some() {
        let Ok(msg @ Value::Object(_)) = serde_json::from_slice::<Value>(&buf) else {
            break;
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
                client,
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
                Err(Failure {
                    message,
                    field: Some(field),
                }) => json!({ "id": id, "error": { "message": message, "field": field } }),
                Err(e) => json!({ "id": id, "error": { "message": e.message } }),
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
    last_windows: Value,
    exe: Option<(PathBuf, (u64, u64))>,
    exe_checked: Instant,
    /// Root of the process file system, a fake one in tests.
    proc_root: PathBuf,
    uid: u32,
    /// Processes of the last scan; only scanned while there are conditions.
    procs: Vec<ProcNames>,
    scan_at: Option<Instant>,
    /// `None` without focus detection.
    windows: Option<Snapshot>,
    /// Conditions that are met.
    met: Vec<String>,
}

/// Device and inode of a file, to detect when an update replaced it.
fn file_id(path: &PathBuf) -> Option<(u64, u64)> {
    use std::os::unix::fs::MetadataExt;
    std::fs::metadata(path).ok().map(|m| (m.dev(), m.ino()))
}

impl Service {
    fn new(cfg: Config, listener: Listener, clients: Clients, proc_root: PathBuf) -> Self {
        let exe = std::env::current_exe()
            .ok()
            .and_then(|p| Some((p.clone(), file_id(&p)?)));
        let mut service = Self {
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
            last_windows: Value::Null,
            exe,
            exe_checked: Instant::now(),
            proc_root,
            uid: conditions::own_uid(),
            procs: Vec::new(),
            scan_at: None,
            windows: None,
            met: Vec::new(),
        };
        service.scan();
        service
    }

    /// Processes messages until the service ends. Returns true if the
    /// executable was replaced and the service should restart itself.
    fn run(&mut self, rx: Receiver<Msg>) -> bool {
        loop {
            let now = Instant::now();
            let next = [
                self.unmute_at,
                self.learn_until,
                self.scan_at,
                Some(self.exe_checked + EXE_CHECK_INTERVAL),
            ]
            .into_iter()
            .flatten()
            .min()
            .unwrap();
            match rx.recv_timeout(next.saturating_duration_since(now)) {
                Ok(Msg::Terminate) | Err(RecvTimeoutError::Disconnected) => return false,
                Ok(Msg::Request {
                    client,
                    method,
                    params,
                    reply,
                }) => {
                    if method == "quit" {
                        let _ = reply.send(Ok(json!(true)));
                        return false;
                    }
                    let _ = reply.send(self.request(client, &method, params));
                }
                Ok(Msg::Input(e)) => self.input(e),
                Ok(Msg::Audio(e)) => self.audio(e),
                Ok(Msg::Windows(w)) => self.windows(w),
                Err(RecvTimeoutError::Timeout) => {}
            }
            if self.timers(Instant::now()) {
                return true;
            }
            self.publish_status();
            self.publish_windows();
        }
    }

    /// Runs the timers that are due. Returns true if the executable was
    /// replaced.
    fn timers(&mut self, now: Instant) -> bool {
        if self.unmute_at.is_some_and(|t| now >= t) {
            self.unmute_at = None;
            self.set_muted(false);
        }
        if self.learn_until.is_some_and(|t| now >= t) {
            self.stop_learning(None);
        }
        if self.scan_at.is_some_and(|t| now >= t) {
            self.scan();
        }
        if now >= self.exe_checked + EXE_CHECK_INTERVAL {
            self.exe_checked = now;
            return self.exe_replaced();
        }
        false
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
                if !self.allowed() {
                    // The game may have started since the last scan.
                    self.scan();
                    if !self.allowed() {
                        return;
                    }
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

    fn windows(&mut self, snapshot: Option<Snapshot>) {
        self.windows = snapshot.map(|mut s| {
            s.resolve(&self.proc_root, self.uid);
            s
        });
        self.evaluate();
    }

    /// Reads the processes again, while there are conditions.
    fn scan(&mut self) {
        if self.cfg.only_when.is_empty() {
            self.procs.clear();
            self.scan_at = None;
        } else {
            self.procs = conditions::scan(&self.proc_root, self.uid);
            self.scan_at = Some(Instant::now() + SCAN_INTERVAL);
        }
        self.evaluate();
    }

    /// Updates which conditions are met, and unmutes at once when none is,
    /// for example after Alt-Tab or when the game ends.
    fn evaluate(&mut self) {
        self.met = conditions::met(&self.cfg.only_when, &self.procs, self.windows.as_ref());
        if !self.allowed() && self.muted {
            self.set_muted(false);
        }
    }

    /// True if there are no conditions or one of them is met.
    fn allowed(&self) -> bool {
        self.cfg.only_when.is_empty() || !self.met.is_empty()
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
        } else if !self.allowed() {
            let apps: Vec<&str> = self.cfg.only_when.iter().map(|c| c.app.as_str()).collect();
            ("waiting", format!("Waiting for {}", apps.join(", ")))
        } else {
            ("active", "Active".to_string())
        };
        json!({
            "running": state == "active" || state == "muted",
            "allowed": self.allowed(),
            "met": self.met,
            "focus_available": self.windows.is_some(),
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

    fn window_list(&self) -> Value {
        json!(conditions::window_list(
            self.windows.as_ref(),
            &self.cfg.only_when
        ))
    }

    fn publish_windows(&mut self) {
        let w = self.window_list();
        if w != self.last_windows {
            self.clients.notify("windows", w.clone());
            self.last_windows = w;
        }
    }

    fn set_config(&mut self, cfg: Config) -> Result<(), String> {
        self.set_config_from(cfg, None)
    }

    /// Saves and applies `cfg`. `origin` is the connection that set a value
    /// over the live interface; it gets no `changed` for it.
    fn set_config_from(&mut self, cfg: Config, origin: Option<u64>) -> Result<(), String> {
        if cfg == self.cfg {
            return Ok(());
        }
        cfg.save()
            .map_err(|e| format!("could not save the settings: {e}"))?;
        self.apply_config(cfg, origin);
        Ok(())
    }

    fn apply_config(&mut self, cfg: Config, origin: Option<u64>) {
        let old = std::mem::replace(&mut self.cfg, cfg);
        let values = live::values(&self.cfg);
        if live::values(&old) != values {
            self.clients
                .notify_except(origin, "changed", json!({ "values": values }));
        }
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
        if old.only_when != self.cfg.only_when {
            self.scan();
        }
        self.clients.notify("config", json!(self.cfg));
        self.clients.notify("streams", self.stream_list());
    }

    fn request(&mut self, client: u64, method: &str, params: Value) -> Result<Value, Failure> {
        match method {
            "hello" => live::hello(&params),
            "schema" => Ok(live::schema(
                params.get("locale").and_then(Value::as_str).unwrap_or("en"),
            )),
            "get" => Ok(json!({ "values": live::values(&self.cfg) })),
            "set" => {
                let (cfg, value) = live::set(&self.cfg, &params)?;
                self.set_config_from(cfg, Some(client))?;
                Ok(json!({ "value": value }))
            }
            "status" => Ok(self.status()),
            "config.get" => Ok(json!(self.cfg)),
            "config.set" => {
                let cfg = self.cfg.merged(params)?;
                self.set_config(cfg)?;
                Ok(json!(self.cfg))
            }
            "streams" => Ok(self.stream_list()),
            "windows" => Ok(self.window_list()),
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
                    other => return Err(format!("unknown action {other:?}").into()),
                };
                let cfg = self.cfg.merged(json!({ "enabled": enabled }))?;
                self.set_config(cfg)?;
                Ok(json!(self.cfg.enabled))
            }
            other => Err(format!("unknown method {other:?}").into()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::conditions::{FakeProc, Window, own_uid};
    use crate::config::{Condition, When};

    const GAME_PID: u32 = 4242;

    fn service(proc: &FakeProc, only_when: &[(&str, When)], mode: Mode) -> Service {
        let cfg = Config {
            keys: vec!["BTN_EXTRA".into()],
            mode,
            only_when: conditions(only_when),
            ..Config::default()
        };
        let mut s = Service::new(
            cfg,
            Listener::detached(),
            Clients::default(),
            proc.0.clone(),
        );
        // Input devices and PipeWire as on a working system.
        s.input(input::Event::Devices {
            watched: 1,
            readable: true,
        });
        s
    }

    fn conditions(list: &[(&str, When)]) -> Vec<Condition> {
        list.iter()
            .map(|(app, when)| Condition {
                app: app.to_string(),
                when: *when,
            })
            .collect()
    }

    fn start_game(proc: &FakeProc) {
        proc.add(GAME_PID, own_uid(), "fakegame", "/opt/fakegame/fakegame");
    }

    fn stop_game(proc: &FakeProc) {
        std::fs::remove_dir_all(proc.0.join(GAME_PID.to_string())).unwrap();
    }

    fn state(s: &Service) -> String {
        s.status()["state"].as_str().unwrap().to_string()
    }

    /// KWin reports the game window, or another one, as active.
    fn focus(s: &mut Service, game: bool) {
        let window = |id: &str, class: &str| Window {
            id: id.into(),
            class: class.into(),
            ..Window::default()
        };
        s.windows(Some(Snapshot {
            active: if game { "{game}" } else { "{browser}" }.into(),
            windows: vec![window("{game}", "fakegame"), window("{browser}", "firefox")],
        }));
    }

    #[test]
    fn mutes_as_before_without_conditions() {
        let proc = FakeProc::new("svc-plain");
        let mut s = service(&proc, &[], Mode::Hold);
        assert!(s.allowed());
        assert_eq!(state(&s), "active");
        s.input(input::Event::Pressed);
        assert!(s.muted);
        assert_eq!(state(&s), "muted");
        s.input(input::Event::Released);
        assert!(s.unmute_at.is_some());
    }

    #[test]
    fn waits_while_no_condition_is_met() {
        let proc = FakeProc::new("svc-wait");
        let mut s = service(&proc, &[("fakegame", When::Running)], Mode::Hold);
        s.input(input::Event::Pressed);
        assert!(s.held);
        assert!(!s.muted);
        let status = s.status();
        assert_eq!(status["state"], "waiting");
        assert_eq!(status["allowed"], false);
        assert_eq!(status["running"], false);
        assert_eq!(status["met"], json!([]));
        assert_eq!(status["focus_available"], false);
        assert!(status["text"].as_str().unwrap().contains("fakegame"));
    }

    #[test]
    fn mutes_while_a_matching_process_runs() {
        let proc = FakeProc::new("svc-run");
        proc.add(GAME_PID, own_uid(), "FakeGame-Bin", "/opt/FakeGame-Bin");
        let mut s = service(&proc, &[("fakegame", When::Running)], Mode::Hold);
        assert_eq!(s.status()["met"], json!(["fakegame"]));
        assert_eq!(state(&s), "active");
        s.input(input::Event::Pressed);
        assert!(s.muted);
        assert_eq!(s.status()["running"], true);
    }

    #[test]
    fn hold_unmutes_when_the_window_loses_focus() {
        let proc = FakeProc::new("svc-hold");
        let mut s = service(&proc, &[("fakegame", When::Focused)], Mode::Hold);
        focus(&mut s, true);
        assert_eq!(s.status()["focus_available"], true);
        s.input(input::Event::Pressed);
        assert!(s.muted);
        // Alt-Tab while the key is held: unmuted at once.
        focus(&mut s, false);
        assert!(!s.muted);
        assert_eq!(state(&s), "waiting");
        // Back in the game, key still held: stays unmuted until the next press.
        focus(&mut s, true);
        assert!(!s.muted);
        assert_eq!(state(&s), "active");
        s.input(input::Event::Released);
        assert!(!s.muted);
        assert_eq!(s.unmute_at, None);
        s.input(input::Event::Pressed);
        assert!(s.muted);
    }

    #[test]
    fn toggle_resets_when_the_condition_ends() {
        let proc = FakeProc::new("svc-toggle");
        start_game(&proc);
        let mut s = service(&proc, &[("fakegame", When::Running)], Mode::Toggle);
        s.input(input::Event::Pressed);
        s.input(input::Event::Released);
        assert!(s.muted);
        stop_game(&proc);
        s.scan();
        assert!(!s.muted);
        start_game(&proc);
        s.scan();
        assert!(!s.muted);
        s.input(input::Event::Pressed);
        assert!(s.muted);
    }

    #[test]
    fn release_delay_is_cancelled_when_the_condition_ends() {
        let proc = FakeProc::new("svc-delay");
        start_game(&proc);
        let mut s = service(&proc, &[("fakegame", When::Running)], Mode::Hold);
        s.input(input::Event::Pressed);
        s.input(input::Event::Released);
        assert!(s.muted);
        assert!(s.unmute_at.is_some());
        stop_game(&proc);
        s.scan();
        assert!(!s.muted);
        assert_eq!(s.unmute_at, None);
    }

    #[test]
    fn scans_at_once_when_a_key_is_pressed() {
        let proc = FakeProc::new("svc-instant");
        let mut s = service(&proc, &[("fakegame", When::Running)], Mode::Hold);
        assert!(!s.allowed());
        // The game started less than a scan interval ago.
        start_game(&proc);
        s.input(input::Event::Pressed);
        assert!(s.muted);
    }

    #[test]
    fn paused_wins_over_conditions() {
        let proc = FakeProc::new("svc-paused");
        let mut s = service(&proc, &[("fakegame", When::Running)], Mode::Hold);
        let cfg = Config {
            enabled: false,
            ..s.cfg.clone()
        };
        s.apply_config(cfg, None);
        assert_eq!(state(&s), "paused");
        start_game(&proc);
        s.input(input::Event::Pressed);
        assert!(!s.muted);
        assert_eq!(state(&s), "paused");
    }

    #[test]
    fn scans_only_while_there_are_conditions() {
        let proc = FakeProc::new("svc-scan");
        start_game(&proc);
        let mut s = service(&proc, &[], Mode::Hold);
        assert!(s.procs.is_empty());
        assert_eq!(s.scan_at, None);
        s.timers(Instant::now() + Duration::from_secs(10));
        assert!(s.procs.is_empty());

        let mut cfg = s.cfg.clone();
        cfg.only_when = conditions(&[("fakegame", When::Running)]);
        s.apply_config(cfg, None);
        assert!(!s.procs.is_empty());
        assert!(s.scan_at.is_some());
        assert_eq!(s.met, ["fakegame"]);

        // A scan is due after the interval.
        stop_game(&proc);
        s.timers(Instant::now() + SCAN_INTERVAL);
        assert!(s.met.is_empty());

        let mut cfg = s.cfg.clone();
        cfg.only_when.clear();
        s.apply_config(cfg, None);
        assert!(s.procs.is_empty());
        assert_eq!(s.scan_at, None);
        assert!(s.allowed());
    }

    #[test]
    fn new_conditions_unmute_at_once() {
        let proc = FakeProc::new("svc-config");
        let mut s = service(&proc, &[], Mode::Hold);
        s.input(input::Event::Pressed);
        assert!(s.muted);
        let mut cfg = s.cfg.clone();
        cfg.only_when = conditions(&[("fakegame", When::Running)]);
        s.apply_config(cfg, None);
        assert!(!s.muted);
        assert_eq!(state(&s), "waiting");
    }

    #[test]
    fn lists_windows_and_forgets_them_without_kwin() {
        let proc = FakeProc::new("svc-windows");
        let mut s = service(&proc, &[("fakegame", When::Focused)], Mode::Hold);
        assert_eq!(s.window_list(), json!([]));
        focus(&mut s, true);
        assert_eq!(
            s.window_list(),
            json!([
                { "term": "fakegame", "class": "fakegame", "active": true, "matched": true, "matches": ["fakegame"] },
                { "term": "firefox", "class": "firefox", "active": false, "matched": false, "matches": [] },
            ])
        );
        assert_eq!(s.met, ["fakegame"]);
        s.windows(None);
        assert_eq!(s.window_list(), json!([]));
        assert_eq!(s.status()["focus_available"], false);
        // Without focus detection the entry counts while the game runs.
        assert!(s.met.is_empty());
        start_game(&proc);
        s.scan();
        assert_eq!(s.met, ["fakegame"]);
    }

    #[test]
    fn switching_to_focused_applies_at_once() {
        let proc = FakeProc::new("svc-switch");
        start_game(&proc);
        let mut s = service(&proc, &[("fakegame", When::Running)], Mode::Hold);
        // The browser is in front, the game runs in the background.
        focus(&mut s, false);
        assert_eq!(s.met, ["fakegame"]);
        let mut cfg = s.cfg.clone();
        cfg.only_when = conditions(&[("fakegame", When::Focused)]);
        s.apply_config(cfg, None);
        assert!(s.met.is_empty());
        s.input(input::Event::Pressed);
        assert!(!s.muted);
        s.input(input::Event::Released);
        focus(&mut s, true);
        s.input(input::Event::Pressed);
        assert!(s.muted);
    }
}
