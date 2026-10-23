//! Focus detection on KDE Plasma. A small KWin script reports the open
//! windows over the session bus, see `kwin.js`.
//!
//! The script is fixed and compiled in, and only the current owner of
//! `org.kde.KWin` may report windows. When KWin restarts, the script is
//! loaded again; when the service ends, it is unloaded.

use crate::conditions::Snapshot;
use crate::paths;
use futures_core::Stream;
use std::io::{self, Write};
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use zbus::Connection;
use zbus::fdo::{self, DBusProxy, RequestNameFlags, RequestNameReply};
use zbus::message::Header;
use zbus::names::BusName;

const KWIN: &str = "org.kde.KWin";
const BUS_NAME: &str = "io.github.sirsnolte.PushToMute";
const OBJECT_PATH: &str = "/io/github/sirsnolte/PushToMute";
/// Name of the script inside KWin.
const PLUGIN: &str = "tuxhub-push-to-mute";
const SCRIPT: &str = include_str!("kwin.js");
const SCRIPT_FILE: &str = "tuxhub-push-to-mute-kwin.js";
const TIMEOUT: Duration = Duration::from_secs(3);
/// A restarted KWin may take a moment until it accepts scripts.
const LOAD_ATTEMPTS: u32 = 10;

/// Set once the script was loaded, so that it is only unloaded then.
static LOADED: AtomicBool = AtomicBool::new(false);

type OnWindows = Arc<dyn Fn(Option<Snapshot>) + Send + Sync>;
/// Unique bus name of the running KWin.
type Owner = Arc<Mutex<Option<String>>>;

/// Starts focus detection on a background thread. Outside KDE Plasma it
/// ends right away and no windows are ever reported.
pub fn watch(on_windows: impl Fn(Option<Snapshot>) + Send + Sync + 'static) {
    let on_windows: OnWindows = Arc::new(on_windows);
    std::thread::Builder::new()
        .name("kwin".into())
        .spawn(move || {
            if let Err(e) = zbus::block_on(run(on_windows)) {
                eprintln!("focus detection: {e}");
            }
        })
        .expect("spawn kwin thread");
}

/// Unloads the script from KWin, when the service ends.
pub fn unload() {
    if !LOADED.swap(false, Ordering::SeqCst) {
        return;
    }
    let result = zbus::block_on(async {
        let conn = connect().await?;
        scripting::<_, bool>(&conn, "unloadScript", &(PLUGIN,)).await
    });
    if let Err(e) = result {
        eprintln!("focus detection: could not unload the KWin script: {e}");
    }
    if let Ok(path) = script_path() {
        let _ = std::fs::remove_file(path);
    }
}

async fn connect() -> zbus::Result<Connection> {
    zbus::connection::Builder::session()
        .method_timeout(TIMEOUT)
        .build()
        .await
}

async fn run(on_windows: OnWindows) -> zbus::Result<()> {
    let conn = connect().await?;
    let dbus = DBusProxy::new(&conn).await?;
    let kwin = BusName::try_from(KWIN)?;
    if !dbus.name_has_owner(kwin.clone()).await? {
        // Not KDE Plasma: entries for the foreground count as running.
        return Ok(());
    }
    // Subscribe before asking for the owner, so that no restart is missed.
    let mut changes = dbus
        .receive_name_owner_changed_with_args(&[(0, KWIN)])
        .await?;
    // Without the owner every report would be rejected.
    let owner: Owner = Arc::new(Mutex::new(Some(
        dbus.get_name_owner(kwin).await?.to_string(),
    )));
    let windows = Windows {
        owner: owner.clone(),
        on_windows: on_windows.clone(),
    };
    conn.object_server().at(OBJECT_PATH, windows).await?;
    let reply = conn
        .request_name_with_flags(BUS_NAME, RequestNameFlags::DO_NOT_QUEUE)
        .await?;
    if reply != RequestNameReply::PrimaryOwner {
        return Err(zbus::Error::Failure(format!(
            "{BUS_NAME} is already in use"
        )));
    }
    let file = write_script()?;
    load(&conn, &file).await;
    while let Some(change) = next(&mut changes).await {
        let Ok(args) = change.args() else { continue };
        let new = args.new_owner().as_ref().map(|o| o.to_string());
        *owner.lock().unwrap() = new.clone();
        // The windows of the previous KWin are gone.
        on_windows(None);
        if new.is_some() {
            load(&conn, &file).await;
        }
    }
    Ok(())
}

async fn next<S: Stream + Unpin>(stream: &mut S) -> Option<S::Item> {
    std::future::poll_fn(|cx| Pin::new(&mut *stream).poll_next(cx)).await
}

fn script_path() -> io::Result<PathBuf> {
    Ok(paths::runtime_dir()?.join(SCRIPT_FILE))
}

/// Writes the script to a new file that only this user can read.
fn write_script() -> io::Result<PathBuf> {
    let path = script_path()?;
    // A new file never follows a link or keeps other permissions.
    let _ = std::fs::remove_file(&path);
    std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&path)?
        .write_all(SCRIPT.as_bytes())?;
    Ok(path)
}

/// Loads and starts the script, replacing a copy that a previous service
/// left behind.
async fn load(conn: &Connection, file: &Path) {
    let file = file.to_string_lossy();
    let mut error = None;
    for attempt in 0..LOAD_ATTEMPTS {
        if attempt > 0 {
            // Short and rare; no D-Bus traffic is waiting meanwhile.
            std::thread::sleep(Duration::from_millis(500));
        }
        let result = async {
            scripting::<_, bool>(conn, "unloadScript", &(PLUGIN,)).await?;
            let id: i32 = scripting(conn, "loadScript", &(file.as_ref(), PLUGIN)).await?;
            if id < 0 {
                return Err(zbus::Error::Failure("KWin did not load the script".into()));
            }
            scripting::<_, ()>(conn, "start", &()).await
        }
        .await;
        match result {
            Ok(()) => {
                LOADED.store(true, Ordering::SeqCst);
                return;
            }
            Err(e) => error = Some(e),
        }
    }
    if let Some(e) = error {
        eprintln!("focus detection: could not load the KWin script: {e}");
    }
}

async fn scripting<B, R>(conn: &Connection, method: &str, body: &B) -> zbus::Result<R>
where
    B: serde::Serialize + zbus::DynamicType,
    R: for<'d> zbus::wire::DynamicDeserialize<'d>,
{
    conn.call_method(
        Some(KWIN),
        "/Scripting",
        Some("org.kde.kwin.Scripting"),
        method,
        body,
    )
    .await?
    .body()
    .deserialize()
}

/// Only the running KWin may report windows.
fn authorized(sender: Option<&str>, owner: Option<&str>) -> bool {
    matches!((sender, owner), (Some(s), Some(o)) if s == o)
}

struct Windows {
    owner: Owner,
    on_windows: OnWindows,
}

#[zbus::interface(name = "io.github.sirsnolte.PushToMute.Windows")]
impl Windows {
    /// Receives a snapshot of the windows from the KWin script.
    fn update(&self, json: &str, #[zbus(header)] header: Header<'_>) -> fdo::Result<()> {
        let sender = header.sender().map(|s| s.as_str());
        if !authorized(sender, self.owner.lock().unwrap().as_deref()) {
            return Err(fdo::Error::AccessDenied(
                "only KWin may report windows".into(),
            ));
        }
        let snapshot = Snapshot::parse(json).map_err(fdo::Error::InvalidArgs)?;
        (self.on_windows)(Some(snapshot));
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_windows_only_from_kwin() {
        assert!(authorized(Some(":1.42"), Some(":1.42")));
        assert!(!authorized(Some(":1.43"), Some(":1.42")));
        assert!(!authorized(None, Some(":1.42")));
        assert!(!authorized(Some(":1.42"), None));
        assert!(!authorized(None, None));
    }

    #[test]
    fn script_reads_no_window_titles() {
        assert!(!SCRIPT.contains("caption"));
        assert!(SCRIPT.contains(BUS_NAME) && SCRIPT.contains(OBJECT_PATH));
        assert!(SCRIPT.contains("\"io.github.sirsnolte.PushToMute.Windows\""));
    }
}
