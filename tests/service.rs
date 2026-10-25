//! The background service and its live interface, run as separate processes
//! with their own XDG directories, so nothing touches the real session.

use serde_json::{Value, json};
use std::io::{BufRead, BufReader, Write};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::{Duration, Instant};

const EXE: &str = env!("CARGO_BIN_EXE_tuxhub-push-to-mute");

/// Isolated home, XDG directories and a short runtime directory (socket
/// paths are limited to 108 bytes).
struct Env {
    root: PathBuf,
    runtime: PathBuf,
}

impl Env {
    fn new(name: &str) -> Self {
        static NEXT: AtomicU32 = AtomicU32::new(0);
        let n = NEXT.fetch_add(1, Ordering::Relaxed);
        let tag = format!("{}-{n}", std::process::id());
        let root = std::env::temp_dir().join(format!("tuxhub-ptm-{name}-{tag}"));
        let user_run = PathBuf::from(format!("/run/user/{}", unsafe { libc::getuid() }));
        let runtime = if user_run.is_dir() {
            user_run.join(format!("ptm-{tag}"))
        } else {
            root.join("run")
        };
        for dir in [&root, &runtime] {
            let _ = std::fs::remove_dir_all(dir);
            std::fs::create_dir_all(dir).unwrap();
        }
        Self { root, runtime }
    }

    fn socket(&self) -> PathBuf {
        self.runtime.join("tuxhub/push-to-mute.sock")
    }

    fn command(&self) -> Command {
        let mut cmd = Command::new(EXE);
        cmd.env("HOME", self.root.join("home"))
            .env("XDG_CONFIG_HOME", self.root.join("config"))
            .env("XDG_DATA_HOME", self.root.join("data"))
            .env("XDG_RUNTIME_DIR", &self.runtime)
            // No desktop session: no KWin, no portal.
            .env_remove("DBUS_SESSION_BUS_ADDRESS")
            .env("PIPEWIRE_RUNTIME_DIR", self.root.join("no-pipewire"))
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::piped());
        cmd
    }

    fn spawn_daemon(&self) -> Guard {
        Guard(self.command().arg("daemon").spawn().unwrap())
    }

    /// Starts the service and waits for its socket.
    fn daemon(&self) -> Guard {
        let guard = self.spawn_daemon();
        let deadline = Instant::now() + Duration::from_secs(5);
        while UnixStream::connect(self.socket()).is_err() {
            assert!(Instant::now() < deadline, "the service did not start");
            std::thread::sleep(Duration::from_millis(20));
        }
        guard
    }

    fn connect(&self) -> Conn {
        let stream = UnixStream::connect(self.socket()).unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        Conn {
            reader: BufReader::new(stream.try_clone().unwrap()),
            stream,
            next: 0,
            notes: Vec::new(),
        }
    }

    fn config_file(&self) -> PathBuf {
        self.root.join("config/tuxhub-push-to-mute/config.json")
    }
}

impl Drop for Env {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.runtime);
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

/// Kills a process that a failed test left running.
struct Guard(Child);

impl Guard {
    fn exits_within(&mut self, timeout: Duration) -> Option<std::process::ExitStatus> {
        let deadline = Instant::now() + timeout;
        while Instant::now() < deadline {
            if let Some(status) = self.0.try_wait().unwrap() {
                return Some(status);
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        None
    }
}

impl Drop for Guard {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

/// A connection to the service that keeps the notifications it skips.
struct Conn {
    stream: UnixStream,
    reader: BufReader<UnixStream>,
    next: u64,
    notes: Vec<Value>,
}

impl Conn {
    fn read(&mut self) -> Option<Value> {
        let mut line = String::new();
        match self.reader.read_line(&mut line) {
            Ok(0) | Err(_) => None,
            Ok(_) => Some(serde_json::from_str(&line).unwrap()),
        }
    }

    /// The answer as `Ok(result)` or `Err(error object)`.
    fn call(&mut self, method: &str, params: Value) -> Result<Value, Value> {
        self.next += 1;
        let id = self.next;
        writeln!(
            self.stream,
            "{}",
            json!({ "id": id, "method": method, "params": params })
        )
        .unwrap();
        loop {
            let msg = self.read().expect("connection closed");
            if msg.get("id") == Some(&json!(id)) {
                return match msg.get("error") {
                    Some(e) => Err(e.clone()),
                    None => Ok(msg["result"].clone()),
                };
            }
            self.notes.push(msg);
        }
    }

    fn hello(&mut self) -> Value {
        self.call(
            "hello",
            json!({ "protocol": 1, "client": "test", "locale": "en" }),
        )
        .unwrap()
    }

    /// Notifications named `method` received within `wait`.
    fn notes(&mut self, method: &str, wait: Duration) -> Vec<Value> {
        self.stream.set_read_timeout(Some(wait)).unwrap();
        while let Some(msg) = self.read() {
            self.notes.push(msg);
        }
        self.stream
            .set_read_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        self.notes
            .iter()
            .filter(|n| n["method"] == method)
            .map(|n| n["params"].clone())
            .collect()
    }
}

#[test]
fn answers_the_live_protocol() {
    let env = Env::new("protocol");
    let _service = env.daemon();
    let mut c = env.connect();
    let hello = c.hello();
    assert_eq!(hello["protocol"], json!(1));
    assert_eq!(hello["name"], json!("Push-to-Mute"));
    assert_eq!(hello["version"], json!(env!("CARGO_PKG_VERSION")));
    assert!(c.call("hello", json!({ "protocol": 2 })).is_err());

    let de = c.call("schema", json!({ "locale": "de" })).unwrap();
    assert_eq!(de["sections"][0]["label"], json!("Allgemein"));
    let fr = c.call("schema", json!({ "locale": "fr" })).unwrap();
    assert_eq!(fr["sections"][0]["label"], json!("General"));

    let values = c.call("get", json!({})).unwrap();
    assert_eq!(
        values,
        json!({ "values": { "enabled": true, "mode": "hold", "release_delay_ms": 150, "start_at_login": true } })
    );
    // The tool's own methods are unchanged; `configure` is gone.
    assert_eq!(
        c.call("config.get", json!({})).unwrap()["mode"],
        json!("hold")
    );
    assert!(c.call("configure", json!({})).is_err());
}

#[test]
fn socket_and_folder_are_private() {
    let env = Env::new("modes");
    let _service = env.daemon();
    let dir = std::fs::metadata(env.socket().parent().unwrap()).unwrap();
    let sock = std::fs::metadata(env.socket()).unwrap();
    assert_eq!(dir.permissions().mode() & 0o777, 0o700);
    assert_eq!(sock.permissions().mode() & 0o777, 0o600);
}

/// Edge case 1: a second service ends at once and leaves the socket alone.
#[test]
fn a_second_service_ends_and_keeps_the_socket() {
    let env = Env::new("second");
    let _service = env.daemon();
    let mut second = env.spawn_daemon();
    assert!(second.exits_within(Duration::from_secs(5)).is_some());
    let mut c = env.connect();
    assert_eq!(c.hello()["protocol"], json!(1));
}

/// Edge case 2: a socket file that nobody listens on is replaced.
#[test]
fn a_dead_socket_is_replaced() {
    let env = Env::new("dead");
    std::fs::create_dir_all(env.socket().parent().unwrap()).unwrap();
    drop(UnixListener::bind(env.socket()).unwrap());
    assert!(env.socket().exists());
    let _service = env.daemon();
    assert_eq!(env.connect().hello()["protocol"], json!(1));
}

/// Edge case 3: a regular file or a symbolic link in place of the socket is
/// never removed; the service refuses to start.
#[test]
fn foreign_files_at_the_socket_path_are_kept() {
    for kind in ["file", "symlink"] {
        let env = Env::new(kind);
        std::fs::create_dir_all(env.socket().parent().unwrap()).unwrap();
        let target = env.root.join("target");
        std::fs::write(&target, "keep").unwrap();
        if kind == "file" {
            std::fs::write(env.socket(), "keep").unwrap();
        } else {
            std::os::unix::fs::symlink(&target, env.socket()).unwrap();
        }
        let mut service = env.spawn_daemon();
        let status = service
            .exits_within(Duration::from_secs(5))
            .expect("still running");
        assert!(!status.success(), "{kind}");
        let meta = std::fs::symlink_metadata(env.socket()).unwrap();
        assert_eq!(meta.file_type().is_symlink(), kind == "symlink");
        assert_eq!(std::fs::read_to_string(&target).unwrap(), "keep");
    }
}

/// Edge case 4: refused values name the field and change nothing.
#[test]
fn set_refuses_invalid_values() {
    let env = Env::new("refuse");
    let _service = env.daemon();
    let mut c = env.connect();
    c.hello();
    let before = c.call("config.get", json!({})).unwrap();
    for (key, value) in [
        ("keys", json!([])),
        ("enabled", json!("yes")),
        ("release_delay_ms", json!(99999)),
    ] {
        let e = c
            .call("set", json!({ "key": key, "value": value }))
            .unwrap_err();
        assert_eq!(e["field"], json!(key), "{e}");
        assert!(e["message"].as_str().is_some_and(|m| !m.is_empty()));
    }
    assert_eq!(c.call("config.get", json!({})).unwrap(), before);
    assert!(!env.config_file().exists());
}

/// Edge case 5: the connection that sets a value gets no `changed`, the
/// others do; the value is saved.
#[test]
fn set_notifies_the_other_connections() {
    let env = Env::new("set");
    let _service = env.daemon();
    let (mut a, mut b) = (env.connect(), env.connect());
    a.hello();
    b.hello();
    let applied = a
        .call("set", json!({ "key": "release_delay_ms", "value": 400 }))
        .unwrap();
    assert_eq!(applied, json!({ "value": 400 }));
    let changed = b.notes("changed", Duration::from_millis(500));
    assert_eq!(changed.len(), 1);
    assert_eq!(changed[0]["values"]["release_delay_ms"], json!(400));
    assert_eq!(b.notes("config", Duration::from_millis(10)).len(), 1);
    assert!(a.notes("changed", Duration::from_millis(300)).is_empty());
    let saved: Value =
        serde_json::from_str(&std::fs::read_to_string(env.config_file()).unwrap()).unwrap();
    assert_eq!(saved["release_delay_ms"], json!(400));
}

/// Edge case 6: a change from the window reaches every live connection.
#[test]
fn config_set_notifies_every_connection() {
    let env = Env::new("window");
    let _service = env.daemon();
    let (mut window, mut hub) = (env.connect(), env.connect());
    hub.hello();
    window
        .call("config.set", json!({ "enabled": false }))
        .unwrap();
    let changed = hub.notes("changed", Duration::from_millis(500));
    assert_eq!(changed.len(), 1);
    assert_eq!(changed[0]["values"]["enabled"], json!(false));
    assert_eq!(window.notes("changed", Duration::from_millis(300)).len(), 1);
    // The action changes `enabled` as well.
    window.call("action", json!({ "id": "toggle" })).unwrap();
    let changed = hub.notes("changed", Duration::from_millis(500));
    assert_eq!(changed.last().unwrap()["values"]["enabled"], json!(true));
    // A change outside the schema sends no `changed`.
    let count = hub.notes("changed", Duration::from_millis(10)).len();
    window
        .call("config.set", json!({ "apps": ["discord"] }))
        .unwrap();
    assert_eq!(
        hub.notes("changed", Duration::from_millis(300)).len(),
        count
    );
}

/// Edge case 8: `quit` answers, ends the service and removes the socket.
#[test]
fn quit_removes_the_socket() {
    let env = Env::new("quit");
    let mut service = env.daemon();
    let mut c = env.connect();
    c.hello();
    assert_eq!(c.call("quit", json!({})).unwrap(), json!(true));
    assert!(service.exits_within(Duration::from_secs(5)).is_some());
    assert!(!env.socket().exists());
}

/// Edge case 9: a line over 1 MiB, or one that is not a JSON object, closes
/// only that connection.
#[test]
fn invalid_lines_close_only_their_connection() {
    let env = Env::new("invalid");
    let _service = env.daemon();
    let mut other = env.connect();
    other.hello();
    for line in [
        format!(
            "{{\"id\":1,\"method\":\"status\",\"params\":\"{}\"}}",
            "x".repeat(1 << 20)
        ),
        "[1, 2]".to_string(),
        "not json".to_string(),
    ] {
        let mut bad = env.connect();
        let _ = bad.stream.write_all(format!("{line}\n").as_bytes());
        assert!(
            bad.read().is_none(),
            "{} kept open",
            &line[..line.len().min(20)]
        );
    }
    assert_eq!(
        other.call("get", json!({})).unwrap()["values"]["mode"],
        json!("hold")
    );
}
