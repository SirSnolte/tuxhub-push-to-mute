//! Installation and desktop integration: application launcher, start at
//! login, and registration with tuxhub.
//!
//! The tool works the same whether it was installed on its own or through
//! tuxhub. An installation on its own registers itself in
//! `~/.local/share/tuxhub/apps/`, where tuxhub finds it when it is installed
//! later.

use crate::config::{APP_DIR, Config};
use crate::{client, paths};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

/// Name shown in window titles and messages.
pub const DISPLAY_NAME: &str = "Push-to-Mute";
pub const APP_ID: &str = "io.github.sirsnolte.TuxhubPushToMute";
pub const TOOL_ID: &str = "push-to-mute";
pub const EXE_NAME: &str = "tuxhub-push-to-mute";
const REPO: &str = "SirSnolte/tuxhub-push-to-mute";
const RELEASE_ASSET: &str = "*x86_64-linux.tar.gz";
/// Data folder of tuxhub, to detect an installation made through it.
const HUB_DATA_DIR: &str = "io.github.sirsnolte.tuxhub";

/// Folder of the running installation: the folder of the executable, if it
/// contains the tool files. Development builds have none.
pub fn tool_root() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    let dir = exe.parent()?;
    dir.join("tuxhub.toml").is_file().then(|| dir.to_path_buf())
}

fn standalone_dir() -> io::Result<PathBuf> {
    Ok(paths::data_home()?.join(APP_DIR))
}

fn registration_file() -> io::Result<PathBuf> {
    Ok(paths::data_home()?
        .join("tuxhub/apps")
        .join(format!("{TOOL_ID}.toml")))
}

fn launcher_file() -> io::Result<PathBuf> {
    Ok(paths::data_home()?
        .join("applications")
        .join(format!("{APP_ID}.desktop")))
}

fn icon_file() -> io::Result<PathBuf> {
    Ok(paths::data_home()?
        .join("icons/hicolor/scalable/apps")
        .join(format!("{APP_ID}.svg")))
}

fn autostart_file() -> io::Result<PathBuf> {
    Ok(paths::config_home()?
        .join("autostart")
        .join(format!("{APP_ID}.desktop")))
}

fn bin_link() -> io::Result<PathBuf> {
    Ok(paths::home()?.join(".local/bin").join(EXE_NAME))
}

/// Quotes a path for the `Exec` key of a desktop entry.
fn desktop_quote(path: &Path) -> String {
    let mut out = String::from("\"");
    for c in path.to_string_lossy().chars() {
        if matches!(c, '"' | '`' | '$' | '\\') {
            out.push('\\');
        }
        out.push(c);
    }
    out.push('"');
    // The Exec key itself treats backslashes as escapes once more.
    out.replace('\\', "\\\\")
}

fn write_if_changed(path: &Path, content: &[u8]) -> io::Result<()> {
    if std::fs::read(path).ok().as_deref() == Some(content) {
        return Ok(());
    }
    std::fs::create_dir_all(path.parent().unwrap())?;
    let tmp = path.with_extension("tmp");
    std::fs::write(&tmp, content)?;
    std::fs::rename(tmp, path)
}

fn remove_if_exists(path: &Path) -> io::Result<bool> {
    match std::fs::remove_file(path) {
        Ok(()) => Ok(true),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(false),
        Err(e) => Err(e),
    }
}

/// Creates the launcher and the login entry for the running installation.
/// Called by the service on every start, so both always point to the
/// current executable, also after tuxhub installed or moved the tool.
pub fn refresh(cfg: &Config) -> io::Result<()> {
    match tool_root() {
        Some(root) => refresh_at(&root, cfg),
        None => Ok(()),
    }
}

fn refresh_at(root: &Path, cfg: &Config) -> io::Result<()> {
    let exe = desktop_quote(&root.join(EXE_NAME));
    let icon = root.join("assets/icon.svg");
    if icon.is_file() {
        write_if_changed(&icon_file()?, &std::fs::read(icon)?)?;
    }
    let launcher = format!(
        "[Desktop Entry]\n\
         Type=Application\n\
         Name=Push-to-Mute\n\
         GenericName=Voice chat muting\n\
         GenericName[de]=Sprachchat stummschalten\n\
         Comment=Mute voice chat while talking in game with push-to-talk\n\
         Comment[de]=Sprachchat stummschalten, während im Spiel per Push-to-Talk gesprochen wird\n\
         Exec={exe}\n\
         Icon={APP_ID}\n\
         Terminal=false\n\
         Categories=AudioVideo;Audio;Game;\n\
         Keywords=discord;teamspeak;mumble;microphone;push to talk;\n\
         StartupWMClass={APP_ID}\n"
    );
    write_if_changed(&launcher_file()?, launcher.as_bytes())?;
    let autostart = autostart_file()?;
    if cfg.start_at_login {
        let entry = format!(
            "[Desktop Entry]\n\
             Type=Application\n\
             Name=Push-to-Mute\n\
             Comment=Background service of Push-to-Mute\n\
             Exec={exe} daemon\n\
             Icon={APP_ID}\n\
             Terminal=false\n\
             NoDisplay=true\n\
             X-GNOME-Autostart-enabled=true\n\
             X-KDE-autostart-phase=2\n"
        );
        write_if_changed(&autostart, entry.as_bytes())?;
    } else {
        remove_if_exists(&autostart)?;
    }
    Ok(())
}

/// Path of the installation made through tuxhub, if there is one.
fn hub_installation() -> Option<PathBuf> {
    let record = paths::data_home()
        .ok()?
        .join(HUB_DATA_DIR)
        .join("tools")
        .join(TOOL_ID)
        .join("tool.json");
    record.is_file().then_some(record)
}

fn copy_dir(from: &Path, to: &Path) -> io::Result<()> {
    std::fs::create_dir_all(to)?;
    for entry in std::fs::read_dir(from)? {
        let entry = entry?;
        let target = to.join(entry.file_name());
        let ty = entry.file_type()?;
        if ty.is_dir() {
            copy_dir(&entry.path(), &target)?;
        } else if ty.is_file() {
            std::fs::copy(entry.path(), &target)?;
        }
    }
    Ok(())
}

/// Installs the tool for the current user from an extracted release folder.
pub fn install(force: bool) -> io::Result<PathBuf> {
    let src = tool_root().ok_or_else(|| {
        io::Error::other(
            "run `install` from the extracted release folder, which contains tuxhub.toml",
        )
    })?;
    if !force && let Some(record) = hub_installation() {
        return Err(io::Error::other(format!(
            "Push-to-Mute is already installed through tuxhub ({}). Update it there, or pass --force to \
             install a second copy.",
            record.parent().unwrap().display()
        )));
    }
    let dest = standalone_dir()?;
    client::stop_service();
    if src != dest {
        // Copy to a staging folder first, so an interrupted installation
        // never leaves a broken copy behind.
        let staging = dest.with_extension("new");
        let old = dest.with_extension("old");
        let _ = std::fs::remove_dir_all(&staging);
        copy_dir(&src, &staging)?;
        if dest.exists() {
            let _ = std::fs::remove_dir_all(&old);
            std::fs::rename(&dest, &old)?;
        }
        std::fs::rename(&staging, &dest)?;
        let _ = std::fs::remove_dir_all(&old);
    }

    let registration = format!(
        "# Written by {EXE_NAME}. tuxhub uses this file to find the installation.\n\
         id = \"{TOOL_ID}\"\n\
         path = {}\n\
         repo = \"{REPO}\"\n\
         asset = \"{RELEASE_ASSET}\"\n\
         category = \"input\"\n",
        serde_json::to_string(&dest.to_string_lossy()).unwrap()
    );
    write_if_changed(&registration_file()?, registration.as_bytes())?;

    let link = bin_link()?;
    if link
        .symlink_metadata()
        .is_ok_and(|m| m.file_type().is_symlink())
    {
        std::fs::remove_file(&link)?;
    }
    if !link.exists() {
        std::fs::create_dir_all(link.parent().unwrap())?;
        std::os::unix::fs::symlink(dest.join(EXE_NAME), &link)?;
    }

    refresh_at(&dest, &Config::load())?;
    crate::client::start_service_at(&dest.join(EXE_NAME))?;
    Ok(dest)
}

/// Removes the desktop integration of this installation. An installation
/// made with `install` is removed completely; an installation made through
/// tuxhub keeps its files, which tuxhub removes afterwards.
pub fn uninstall(purge: bool) -> io::Result<Vec<PathBuf>> {
    let root = tool_root();
    let standalone = standalone_dir()?;
    client::stop_service();

    let mut removed = Vec::new();
    for path in [autostart_file()?, launcher_file()?, icon_file()?] {
        if remove_if_exists(&path)? {
            removed.push(path);
        }
    }
    // Only remove the registration and the command link if they belong to
    // this installation.
    let registration = registration_file()?;
    let registered = std::fs::read_to_string(&registration)
        .ok()
        .is_some_and(|s| {
            let quoted = |p: &Path| serde_json::to_string(&p.to_string_lossy()).unwrap();
            s.contains(&quoted(&standalone))
                || root.as_deref().is_some_and(|r| s.contains(&quoted(r)))
        });
    if registered && remove_if_exists(&registration)? {
        removed.push(registration);
    }
    let link = bin_link()?;
    if let Ok(target) = std::fs::read_link(&link)
        && (target.starts_with(&standalone)
            || root.as_deref().is_some_and(|r| target.starts_with(r)))
    {
        std::fs::remove_file(&link)?;
        removed.push(link);
    }
    if root.as_deref() == Some(standalone.as_path()) {
        std::fs::remove_dir_all(&standalone)?;
        removed.push(standalone);
    }
    if purge {
        let dir = paths::config_home()?.join(APP_DIR);
        if dir.exists() {
            std::fs::remove_dir_all(&dir)?;
            removed.push(dir);
        }
    }
    Ok(removed)
}

/// udev rule that gives the user at the local seat read access to input
/// devices, as members of the `input` group have.
pub const UDEV_RULE_PATH: &str = "/etc/udev/rules.d/60-tuxhub-input.rules";
pub const UDEV_RULE: &str = "SUBSYSTEM==\"input\", KERNEL==\"event*\", TAG+=\"uaccess\"\n";

/// Installs the udev rule with `pkexec`, which asks for the password in a
/// graphical dialog. The rule is passed on standard input, not interpolated
/// into the shell command.
pub fn setup_input_access() -> io::Result<()> {
    let script = format!(
        "cat > {UDEV_RULE_PATH} && udevadm control --reload-rules && \
         udevadm trigger --subsystem-match=input --action=change"
    );
    let mut child = Command::new("pkexec")
        .args(["sh", "-c", &script])
        .stdin(Stdio::piped())
        .spawn()?;
    child
        .stdin
        .take()
        .expect("stdin is piped")
        .write_all(UDEV_RULE.as_bytes())?;
    if child.wait()?.success() {
        Ok(())
    } else {
        Err(io::Error::other("the setup was cancelled or failed"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quotes_exec_paths() {
        assert_eq!(desktop_quote(Path::new("/opt/a b/x")), "\"/opt/a b/x\"");
        assert_eq!(desktop_quote(Path::new("/a$b")), "\"/a\\\\$b\"");
    }
}
