// KWin script of Push-to-Mute. It reports the open windows to the
// Push-to-Mute service, so that muting can depend on the window in the
// foreground. Only class, app id, desktop file and process id are sent,
// never window titles. The service loads and unloads it, see kwin.rs.

const SERVICE = "io.github.sirsnolte.PushToMute";
const PATH = "/io/github/sirsnolte/PushToMute";
const INTERFACE = "io.github.sirsnolte.PushToMute.Windows";

function listed(w) {
  return w.normalWindow && !w.skipTaskbar;
}

// `removed` is a window that is about to disappear.
function send(removed) {
  const gone = removed ? String(removed.internalId) : "";
  const windows = workspace
    .windowList()
    .filter((w) => listed(w) && String(w.internalId) !== gone)
    .map((w) => ({
      id: String(w.internalId),
      class: String(w.resourceClass || ""),
      name: String(w.resourceName || ""),
      desktop: String(w.desktopFileName || ""),
      pid: w.pid > 0 ? w.pid : 0,
    }));
  const active = workspace.activeWindow;
  const id = active && listed(active) ? String(active.internalId) : "";
  const snapshot = { active: id === gone ? "" : id, windows: windows };
  callDBus(SERVICE, PATH, INTERFACE, "Update", JSON.stringify(snapshot));
}

function watch(w) {
  w.windowClassChanged.connect(() => send());
  w.desktopFileNameChanged.connect(() => send());
}

workspace.windowList().forEach(watch);
workspace.windowAdded.connect((w) => {
  watch(w);
  send();
});
workspace.windowRemoved.connect((w) => send(w));
workspace.windowActivated.connect(() => send());
send();
