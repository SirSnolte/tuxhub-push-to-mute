//! Connection to the background service, which is started on demand.

use crate::daemon;
use serde_json::{Value, json};
use std::io::{self, BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::os::unix::process::CommandExt;
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

/// Connects to the running service.
pub fn connect() -> io::Result<UnixStream> {
    UnixStream::connect(daemon::socket_path()?)
}

/// Connects to the service and starts it first if it is not running.
pub fn connect_or_start() -> io::Result<UnixStream> {
    if let Ok(s) = connect() {
        return Ok(s);
    }
    start_service()?;
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        match connect() {
            Ok(s) => return Ok(s),
            Err(e) if Instant::now() >= deadline => {
                return Err(io::Error::new(
                    e.kind(),
                    format!("the service did not start ({e})"),
                ));
            }
            Err(_) => std::thread::sleep(Duration::from_millis(50)),
        }
    }
}

/// Starts the service as an independent process, so it outlives whichever of
/// the window and tuxhub is closed first.
pub fn start_service() -> io::Result<()> {
    start_service_at(&std::env::current_exe()?)
}

/// Like `start_service`, for the binary at `exe`.
pub fn start_service_at(exe: &Path) -> io::Result<()> {
    let mut cmd = Command::new(exe);
    cmd.arg("daemon")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    unsafe {
        cmd.pre_exec(|| {
            libc::setsid();
            Ok(())
        });
    }
    let mut child = cmd.spawn()?;
    // The service lives on in its own session; collect the exit status of a
    // second instance that ends right away.
    std::thread::spawn(move || child.wait());
    Ok(())
}

/// Sends a single request and waits for the reply, skipping notifications.
pub fn request(method: &str, params: Value) -> io::Result<Value> {
    let stream = connect_or_start()?;
    request_on(&stream, method, params)
}

pub fn request_on(stream: &UnixStream, method: &str, params: Value) -> io::Result<Value> {
    stream.set_read_timeout(Some(Duration::from_secs(5)))?;
    let mut w = stream;
    writeln!(
        w,
        "{}",
        json!({ "id": 1, "method": method, "params": params })
    )?;
    for line in BufReader::new(stream).lines() {
        let msg: Value = serde_json::from_str(&line?).map_err(io::Error::other)?;
        if msg.get("id") != Some(&json!(1)) {
            continue;
        }
        if let Some(e) = msg.get("error") {
            let text = e
                .get("message")
                .and_then(Value::as_str)
                .unwrap_or("unknown error");
            return Err(io::Error::other(text.to_string()));
        }
        return Ok(msg.get("result").cloned().unwrap_or(Value::Null));
    }
    Err(io::Error::other("the service closed the connection"))
}

/// Asks a running service to stop and exit. Does nothing if none runs.
pub fn stop_service() {
    if let Ok(s) = connect() {
        let _ = request_on(&s, "quit", Value::Null);
        // Wait until the service has released its socket.
        let deadline = Instant::now() + Duration::from_secs(3);
        while connect().is_ok() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(50));
        }
    }
}
