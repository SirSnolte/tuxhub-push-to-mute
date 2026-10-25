//! Building blocks of the background service that every tuxhub tool shares:
//! the single-instance lock, the private live socket, the list of connected
//! clients and the line reader. The file is the same in every tool; change it
//! in one place first, then copy it.

use serde_json::{Value, json};
use std::io::{self, BufRead, Read, Write};
use std::os::fd::AsRawFd;
use std::os::unix::fs::{FileTypeExt, PermissionsExt};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::Path;
use std::sync::{Arc, Mutex};

/// Longest line a client may send, without the line break.
pub(crate) const MAX_LINE: usize = 1 << 20;

/// Listens on `path`: creates its folder with mode 0700, replaces a socket
/// that nobody listens on, and never removes anything else.
pub(crate) fn bind(path: &Path) -> io::Result<UnixListener> {
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

/// Takes an exclusive lock that is released when the process exits.
pub(crate) fn try_lock(path: &Path) -> io::Result<Option<std::fs::File>> {
    let file = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(path)?;
    let ok = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } == 0;
    Ok(ok.then_some(file))
}

pub(crate) type Writer = Arc<Mutex<UnixStream>>;

#[derive(Clone, Default)]
pub(crate) struct Clients(Arc<Mutex<Vec<(u64, Writer)>>>);

// Not every tool needs every method of this shared file.
#[allow(dead_code)]
impl Clients {
    pub(crate) fn add(&self, id: u64, w: Writer) {
        self.0.lock().unwrap().push((id, w));
    }

    pub(crate) fn remove(&self, id: u64) {
        self.0.lock().unwrap().retain(|(i, _)| *i != id);
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.0.lock().unwrap().is_empty()
    }

    pub(crate) fn notify(&self, method: &str, params: Value) {
        self.notify_except(None, method, params);
    }

    /// Notifies every connection but `except`.
    pub(crate) fn notify_except(&self, except: Option<u64>, method: &str, params: Value) {
        let line = format!("{}\n", json!({ "method": method, "params": params }));
        self.0.lock().unwrap().retain(|(id, w)| {
            Some(*id) == except || w.lock().unwrap().write_all(line.as_bytes()).is_ok()
        });
    }

    /// Notifies connection `client` only.
    pub(crate) fn notify_one(&self, client: u64, method: &str, params: Value) {
        let line = format!("{}\n", json!({ "method": method, "params": params }));
        if let Some((_, w)) = self.0.lock().unwrap().iter().find(|(id, _)| *id == client) {
            let _ = w.lock().unwrap().write_all(line.as_bytes());
        }
    }
}

/// Reads one line of at most [`MAX_LINE`] bytes; `None` at the end, on an
/// error or for a longer line.
pub(crate) fn read_line(reader: &mut impl BufRead, buf: &mut Vec<u8>) -> Option<()> {
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
