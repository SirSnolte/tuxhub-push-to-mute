//! Conditions for muting: applications that run, or are in the foreground.
//!
//! Processes are read from `/proc`, windows are reported by KWin (see the
//! `kwin` module). Both are controlled by other programs of the user, so
//! their contents are only compared, reads are limited in size, and window
//! titles are never part of them.

use crate::config::{Condition, When};
use crate::integration::{APP_ID, EXE_NAME};
use serde::Serialize;
use serde_json::Value;
use std::io::Read;
use std::path::Path;

pub const PROC: &str = "/proc";
/// Read limit for the files of a process.
const MAX_READ: u64 = 4096;
/// Limits for a snapshot of the windows.
pub const MAX_SNAPSHOT: usize = 256 * 1024;
const MAX_WINDOWS: usize = 256;
const MAX_FIELD: usize = 256;
/// The kernel cuts `comm` to this length.
const COMM_LEN: usize = 15;

pub fn own_uid() -> u32 {
    unsafe { libc::getuid() }
}

fn read_limited(path: &Path) -> Option<String> {
    let mut buf = Vec::new();
    std::fs::File::open(path)
        .ok()?
        .take(MAX_READ)
        .read_to_end(&mut buf)
        .ok()?;
    Some(String::from_utf8_lossy(&buf).into_owned())
}

/// Lowercase names of a process: `comm` and the file name of `argv[0]`.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ProcNames {
    pub comm: String,
    pub exe: String,
}

impl ProcNames {
    /// Reads the names of process `pid` if it belongs to `uid`.
    pub fn read(root: &Path, pid: u32, uid: u32) -> Option<Self> {
        let dir = root.join(pid.to_string());
        let status = read_limited(&dir.join("status"))?;
        let owner: u32 = status
            .lines()
            .find_map(|l| l.strip_prefix("Uid:"))?
            .split_whitespace()
            .next()?
            .parse()
            .ok()?;
        if owner != uid {
            return None;
        }
        let comm = read_limited(&dir.join("comm"))?;
        let cmdline = read_limited(&dir.join("cmdline")).unwrap_or_default();
        // Wine and Proton show Windows paths such as `Z:\Games\Game.exe`.
        let argv0 = cmdline.split('\0').next().unwrap_or_default();
        let exe = argv0.rsplit(['/', '\\']).next().unwrap_or_default();
        Some(Self {
            comm: comm.trim_end_matches('\n').to_lowercase(),
            exe: exe.to_lowercase(),
        })
    }

    pub fn matches(&self, term: &str) -> bool {
        self.comm.contains(term) || self.exe.contains(term)
    }

    /// The term the interface suggests for this process: `comm`, or the
    /// complete file name of `argv[0]` if `comm` was cut from it.
    pub fn term(&self) -> &str {
        if self.comm.len() >= COMM_LEN && self.exe.starts_with(&self.comm) {
            &self.exe
        } else {
            &self.comm
        }
    }
}

/// Names of all processes of `uid`. Unreadable processes are skipped.
pub fn scan(root: &Path, uid: u32) -> Vec<ProcNames> {
    let Ok(dir) = std::fs::read_dir(root) else {
        return Vec::new();
    };
    dir.filter_map(|e| e.ok()?.file_name().to_str()?.parse::<u32>().ok())
        .filter_map(|pid| ProcNames::read(root, pid, uid))
        .collect()
}

/// A window as reported by the KWin script.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Window {
    pub id: String,
    pub class: String,
    pub name: String,
    pub desktop: String,
    /// 0 if unknown.
    pub pid: u32,
    /// Names of the process `pid`, see [`Snapshot::resolve`].
    pub process: Option<ProcNames>,
}

impl Window {
    /// Reads a window of a snapshot. `None` if a field is invalid or too long.
    fn from_value(v: &Value) -> Option<Self> {
        let text = |key: &str| match v.get(key) {
            None | Some(Value::Null) => Some(String::new()),
            Some(Value::String(s)) if s.chars().count() <= MAX_FIELD => Some(s.clone()),
            Some(_) => None,
        };
        let id = text("id").filter(|id| !id.is_empty())?;
        Some(Self {
            id,
            class: text("class")?,
            name: text("name")?,
            desktop: text("desktop")?,
            pid: v
                .get("pid")
                .and_then(Value::as_u64)
                .and_then(|p| u32::try_from(p).ok())
                .unwrap_or(0),
            process: None,
        })
    }

    pub fn matches(&self, term: &str) -> bool {
        [&self.class, &self.name, &self.desktop]
            .iter()
            .any(|f| f.to_lowercase().contains(term))
            || self.process.as_ref().is_some_and(|p| p.matches(term))
    }

    /// The term the interface suggests for this window: the name of its
    /// process, or its class if the process is unknown.
    pub fn term(&self) -> String {
        let process = self.process.as_ref().map(ProcNames::term).unwrap_or("");
        [process, &self.class, &self.desktop, &self.name]
            .into_iter()
            .find(|s| !s.is_empty())
            .unwrap_or_default()
            .to_lowercase()
    }

    /// The window of this tool. On Wayland, GTK names it after the program
    /// rather than the app id.
    fn is_own(&self) -> bool {
        let exe = self.process.as_ref().map(|p| p.exe.as_str()).unwrap_or("");
        [self.class.as_str(), &self.desktop, exe]
            .iter()
            .any(|f| f.eq_ignore_ascii_case(APP_ID) || f.eq_ignore_ascii_case(EXE_NAME))
    }
}

/// The open windows and the id of the active one.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Snapshot {
    pub active: String,
    pub windows: Vec<Window>,
}

impl Snapshot {
    /// Parses a snapshot of the KWin script. Fails for invalid or oversized
    /// snapshots; windows with invalid or oversized fields are skipped.
    pub fn parse(text: &str) -> Result<Self, String> {
        if text.len() > MAX_SNAPSHOT {
            return Err("the window list is too large".into());
        }
        let v: Value =
            serde_json::from_str(text).map_err(|e| format!("invalid window list: {e}"))?;
        let active = match v.get("active") {
            None | Some(Value::Null) => String::new(),
            Some(Value::String(s)) if s.chars().count() <= MAX_FIELD => s.clone(),
            Some(_) => return Err("invalid active window".into()),
        };
        let list = v
            .get("windows")
            .and_then(Value::as_array)
            .ok_or("the window list is missing")?;
        if list.len() > MAX_WINDOWS {
            return Err(format!("more than {MAX_WINDOWS} windows"));
        }
        Ok(Self {
            active,
            windows: list.iter().filter_map(Window::from_value).collect(),
        })
    }

    /// Looks up the process names of each window.
    pub fn resolve(&mut self, root: &Path, uid: u32) {
        for w in &mut self.windows {
            w.process = (w.pid != 0)
                .then(|| ProcNames::read(root, w.pid, uid))
                .flatten();
        }
    }

    pub fn active(&self) -> Option<&Window> {
        if self.active.is_empty() {
            return None;
        }
        self.windows.iter().find(|w| w.id == self.active)
    }
}

/// Terms of the entries that are met. Without focus detection (`windows` is
/// `None`), entries for the foreground count as soon as the application runs.
pub fn met(
    conditions: &[Condition],
    procs: &[ProcNames],
    windows: Option<&Snapshot>,
) -> Vec<String> {
    let active = windows.and_then(Snapshot::active);
    conditions
        .iter()
        .filter(|c| match (c.when, windows) {
            (When::Focused, Some(_)) => active.is_some_and(|w| w.matches(&c.app)),
            _ => procs.iter().any(|p| p.matches(&c.app)),
        })
        .map(|c| c.app.clone())
        .collect()
}

/// An entry of the window list in the interface.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct WindowEntry {
    pub term: String,
    pub class: String,
    pub active: bool,
    pub matched: bool,
}

/// The open windows for the interface: one entry per term, without the
/// window of this tool.
pub fn window_list(windows: Option<&Snapshot>, conditions: &[Condition]) -> Vec<WindowEntry> {
    let Some(snapshot) = windows else {
        return Vec::new();
    };
    let mut out: Vec<WindowEntry> = Vec::new();
    for w in snapshot.windows.iter().filter(|w| !w.is_own()) {
        let term = w.term();
        if term.is_empty() {
            continue;
        }
        let active = snapshot.active == w.id;
        let matched = conditions.iter().any(|c| w.matches(&c.app));
        match out.iter_mut().find(|e| e.term == term) {
            Some(e) => {
                e.active |= active;
                e.matched |= matched;
            }
            None => out.push(WindowEntry {
                term,
                class: w.class.clone(),
                active,
                matched,
            }),
        }
    }
    out
}

/// A fake `/proc` in a temporary folder.
#[cfg(test)]
pub(crate) struct FakeProc(pub std::path::PathBuf);

#[cfg(test)]
impl FakeProc {
    pub fn new(name: &str) -> Self {
        let dir = std::env::temp_dir().join(format!("ptm-proc-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        Self(dir)
    }

    pub fn add(&self, pid: u32, uid: u32, comm: &str, argv0: &str) {
        let dir = self.0.join(pid.to_string());
        std::fs::create_dir_all(&dir).unwrap();
        let status = format!(
            "Name:\t{comm}\nUmask:\t0022\nState:\tS (sleeping)\nUid:\t{uid}\t{uid}\t{uid}\t{uid}\n"
        );
        std::fs::write(dir.join("status"), status).unwrap();
        std::fs::write(dir.join("comm"), format!("{comm}\n")).unwrap();
        std::fs::write(dir.join("cmdline"), format!("{argv0}\0--flag\0")).unwrap();
    }
}

#[cfg(test)]
impl Drop for FakeProc {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn cond(app: &str, when: When) -> Condition {
        Condition {
            app: app.into(),
            when,
        }
    }

    fn names(comm: &str, exe: &str) -> ProcNames {
        ProcNames {
            comm: comm.into(),
            exe: exe.into(),
        }
    }

    fn window(id: &str, class: &str, pid: u32, process: Option<ProcNames>) -> Window {
        Window {
            id: id.into(),
            class: class.into(),
            name: class.to_lowercase(),
            desktop: String::new(),
            pid,
            process,
        }
    }

    #[test]
    fn scans_own_processes_only() {
        let uid = own_uid();
        let proc = FakeProc::new("scan");
        proc.add(10, uid, "FakeGame-bin", "/opt/games/FakeGame-bin");
        proc.add(11, uid + 1, "othergame", "/usr/bin/othergame");
        proc.add(12, uid, "Cyberpunk2077.e", "Z:\\Games\\Cyberpunk2077.exe");
        // Unreadable: no status file.
        std::fs::create_dir_all(proc.0.join("13")).unwrap();
        std::fs::write(proc.0.join("13/comm"), "ghost\n").unwrap();
        // Not a process.
        std::fs::create_dir_all(proc.0.join("self-test")).unwrap();

        let mut found = scan(&proc.0, uid);
        found.sort_by(|a, b| a.comm.cmp(&b.comm));
        assert_eq!(
            found,
            [
                names("cyberpunk2077.e", "cyberpunk2077.exe"),
                names("fakegame-bin", "fakegame-bin"),
            ]
        );
        assert_eq!(ProcNames::read(&proc.0, 11, uid), None);
        assert_eq!(ProcNames::read(&proc.0, 99, uid), None);
        assert_eq!(
            ProcNames::read(&proc.0, 11, uid + 1),
            Some(names("othergame", "othergame"))
        );
    }

    #[test]
    fn scans_the_real_proc() {
        // The test binary itself, whose name is longer than `comm`.
        let procs = scan(Path::new(PROC), own_uid());
        let me = procs
            .iter()
            .find(|p| p.exe.starts_with("tuxhub_push_to_mute-"))
            .expect("the test process is listed");
        assert_eq!(me.comm, "tuxhub_push_to_");
        assert!(me.term().starts_with("tuxhub_push_to_mute-"));
    }

    #[test]
    fn running_matches_processes_by_substring() {
        let procs = [names("fakegame-bin", "fakegame-bin"), names("bash", "bash")];
        let met_for = |app: &str| met(&[cond(app, When::Running)], &procs, None);
        assert_eq!(met_for("fakegame"), ["fakegame"]);
        assert_eq!(met_for("game-b"), ["game-b"]);
        assert!(met_for("cs2").is_empty());
        assert!(met(&[], &procs, None).is_empty());
    }

    #[test]
    fn matches_proton_games_by_their_full_name() {
        let proc = FakeProc::new("proton");
        proc.add(
            20,
            own_uid(),
            "Cyberpunk2077.e",
            "Z:\\Games\\Cyberpunk2077.exe",
        );
        let procs = scan(&proc.0, own_uid());
        let list = [cond("cyberpunk2077.exe", When::Running)];
        assert_eq!(met(&list, &procs, None), ["cyberpunk2077.exe"]);
        assert_eq!(procs[0].term(), "cyberpunk2077.exe");
        // comm is not cut: it is the term, even if argv[0] differs.
        assert_eq!(names("discord", "electron").term(), "discord");
    }

    #[test]
    fn foreign_processes_do_not_count() {
        let proc = FakeProc::new("foreign");
        proc.add(30, own_uid() + 1, "fakegame", "/usr/bin/fakegame");
        let procs = scan(&proc.0, own_uid());
        assert!(met(&[cond("fakegame", When::Running)], &procs, None).is_empty());
    }

    #[test]
    fn focused_matches_the_active_window() {
        let snapshot = Snapshot {
            active: "{a}".into(),
            windows: vec![
                window("{a}", "steam_app_730", 40, Some(names("cs2", "cs2"))),
                window("{b}", "firefox", 41, Some(names("firefox", "firefox"))),
            ],
        };
        let procs = [names("cs2", "cs2"), names("firefox", "firefox")];
        let check = |app: &str, s: &Snapshot| met(&[cond(app, When::Focused)], &procs, Some(s));
        // By class and by process name of the window.
        assert_eq!(check("steam_app", &snapshot), ["steam_app"]);
        assert_eq!(check("cs2", &snapshot), ["cs2"]);
        // A window that is open but not active.
        assert!(check("firefox", &snapshot).is_empty());
        let other = Snapshot {
            active: "{b}".into(),
            ..snapshot.clone()
        };
        assert!(check("cs2", &other).is_empty());
        assert_eq!(check("firefox", &other), ["firefox"]);
        // No active window at all.
        let none = Snapshot {
            active: String::new(),
            ..snapshot
        };
        assert!(check("cs2", &none).is_empty());
    }

    #[test]
    fn focused_counts_as_running_without_detection() {
        let procs = [names("cs2", "cs2")];
        let list = [cond("cs2", When::Focused), cond("doom", When::Focused)];
        assert_eq!(met(&list, &procs, None), ["cs2"]);
        // With detection, the running process alone is not enough.
        assert!(met(&list, &procs, Some(&Snapshot::default())).is_empty());
    }

    #[test]
    fn parses_snapshots_within_limits() {
        let w = |id: &str, class: &str| json!({ "id": id, "class": class, "name": "n", "desktop": "d", "pid": 7 });
        let ok = Snapshot::parse(
            &json!({ "active": "{a}", "windows": [w("{a}", "Firefox"), w("{b}", "kate")] })
                .to_string(),
        )
        .unwrap();
        assert_eq!(ok.active, "{a}");
        assert_eq!(ok.windows.len(), 2);
        assert_eq!(ok.windows[0].class, "Firefox");
        assert_eq!(ok.windows[0].pid, 7);
        assert_eq!(ok.active().map(|w| w.class.as_str()), Some("Firefox"));

        // No active window, missing or invalid pid.
        let lax = Snapshot::parse(
            r#"{ "active": null, "windows": [{ "id": "{a}", "class": "x", "name": "", "desktop": "" , "pid": -1 }, { "id": "{b}", "class": "y" }] }"#,
        )
        .unwrap();
        assert_eq!(lax.active, "");
        assert_eq!(lax.active(), None);
        assert_eq!(
            lax.windows.iter().map(|w| w.pid).collect::<Vec<_>>(),
            [0, 0]
        );

        assert!(Snapshot::parse("{ not json").is_err());
        assert!(Snapshot::parse(r#"{ "active": "{a}" }"#).is_err());
        assert!(Snapshot::parse(r#"[]"#).is_err());
        let huge =
            json!({ "active": "", "windows": [w("{a}", &"x".repeat(MAX_SNAPSHOT))] }).to_string();
        assert!(Snapshot::parse(&huge).is_err());
        let many: Vec<Value> = (0..257).map(|i| w(&format!("{{{i}}}"), "x")).collect();
        assert!(Snapshot::parse(&json!({ "active": "", "windows": many }).to_string()).is_err());
        let max: Vec<Value> = (0..256).map(|i| w(&format!("{{{i}}}"), "x")).collect();
        assert_eq!(
            Snapshot::parse(&json!({ "active": "", "windows": max }).to_string())
                .unwrap()
                .windows
                .len(),
            256
        );

        // Windows with long or invalid fields are skipped, the rest is kept.
        let skipped = Snapshot::parse(
            &json!({ "active": "{a}", "windows": [
                w("{a}", &"x".repeat(257)),
                w("{b}", &"x".repeat(256)),
                { "id": 5, "class": "y" },
                { "class": "no id" },
                "nonsense",
            ] })
            .to_string(),
        )
        .unwrap();
        assert_eq!(
            skipped
                .windows
                .iter()
                .map(|w| w.id.as_str())
                .collect::<Vec<_>>(),
            ["{b}"]
        );
        assert!(
            Snapshot::parse(&json!({ "active": "x".repeat(257), "windows": [] }).to_string())
                .is_err()
        );
    }

    #[test]
    fn resolves_the_processes_of_windows() {
        let proc = FakeProc::new("resolve");
        proc.add(50, own_uid(), "cs2", "/games/cs2");
        let mut s = Snapshot {
            active: "{a}".into(),
            windows: vec![
                window("{a}", "steam_app_730", 50, None),
                window("{b}", "kate", 0, None),
            ],
        };
        s.resolve(&proc.0, own_uid());
        assert_eq!(s.windows[0].process, Some(names("cs2", "cs2")));
        assert_eq!(s.windows[1].process, None);
    }

    #[test]
    fn lists_windows_by_term() {
        let snapshot = Snapshot {
            active: "{c}".into(),
            windows: vec![
                window(
                    "{a}",
                    "steam_app_1091500",
                    60,
                    Some(names("cyberpunk2077.e", "cyberpunk2077.exe")),
                ),
                window("{b}", "org.kde.kate", 61, Some(names("kate", "kate"))),
                window("{c}", "org.kde.kate", 62, Some(names("kate", "kate"))),
                window(
                    "{d}",
                    "io.github.sirsnolte.TuxhubPushToMute",
                    63,
                    Some(names("tuxhub-push-to-", "tuxhub-push-to-mute")),
                ),
                Window {
                    desktop: "io.github.sirsnolte.TuxhubPushToMute".into(),
                    ..window("{e}", "", 64, None)
                },
                window("{f}", "Gimp", 0, None),
                // The window as GTK names it after the program on Wayland.
                window(
                    "{g}",
                    "tuxhub-push-to-mute",
                    65,
                    Some(names("tuxhub-push-to-", "tuxhub-push-to-mute")),
                ),
            ],
        };
        let list = window_list(Some(&snapshot), &[cond("kate", When::Focused)]);
        let entry = |term: &str, class: &str, active: bool, matched: bool| WindowEntry {
            term: term.into(),
            class: class.into(),
            active,
            matched,
        };
        assert_eq!(
            list,
            [
                entry("cyberpunk2077.exe", "steam_app_1091500", false, false),
                entry("kate", "org.kde.kate", true, true),
                entry("gimp", "Gimp", false, false),
            ]
        );
        assert!(window_list(None, &[]).is_empty());
    }
}
