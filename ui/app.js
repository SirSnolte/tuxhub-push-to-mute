// Interface of Push-to-Mute. It runs inside tuxhub and in the tool's own
// window; both provide window.tuxhub. All state lives in the background
// service, which notifies every open interface about changes.

"use strict";

const STRINGS = {
  en: {
    enabled: "Enabled",
    "enabled.help": "Pause or resume muting",
    "state.active": "Ready",
    "state.active.detail": "Voice chat is muted while you hold {keys}.",
    "state.muted": "Voice chat muted",
    "state.muted.detail": "Muted: {apps}",
    "state.muted.none": "No selected application is recording right now.",
    "state.paused": "Paused",
    "state.paused.detail": "Your microphone is not muted by Push-to-Mute.",
    "state.no-key": "No push-to-talk key set",
    "state.no-key.detail": "Add the key you use for push-to-talk in your games.",
    "state.no-access": "No access to input devices",
    "state.no-access.detail": "The push-to-talk key cannot be detected.",
    "state.no-audio": "PipeWire is not available",
    "state.offline": "Connecting",
    "state.offline.detail": "Waiting for the Push-to-Mute service.",
    "state.waiting": "Waiting for an application",
    "state.waiting.detail": "Mutes only in {apps}.",
    "problem.no-access":
      "Reading the push-to-talk key requires access to input devices. Set it up once (asks for your password), or add your user to the input group and log in again.",
    "problem.setup": "Set up access",
    "problem.devices": "No connected device has the selected keys.",
    "problem.setupFailed": "Setup failed: {error}",
    "keys.title": "Push-to-talk keys",
    "keys.add": "Add key",
    "keys.help":
      "Use the same key as in your game. The game still receives the key, so its push-to-talk keeps working.",
    "keys.none": "No key yet.",
    "keys.capture": "Press the key now. Esc cancels.",
    "keys.cancel": "Cancel",
    "keys.remove": "Remove {key}",
    "only.title": "Only in certain applications",
    "only.help":
      "Always active without entries. With entries, Push-to-Mute only mutes while one of them is running or in the foreground, so the key does not mute voice chat in the browser, for example.",
    "only.none": "No entries: active in every application.",
    "only.placeholder": "Name, for example cs2",
    "only.add": "Add",
    "only.remove": "Remove {name}",
    "only.running": "Running",
    "only.focused": "In foreground",
    "only.met": "Met",
    "windows.title": "Open windows",
    "windows.help": "Switch on a window to mute only while it is in the foreground.",
    "windows.none": "No other window is open.",
    "windows.focused": "In foreground",
    "windows.select": "Mute only while in foreground",
    "windows.unavailable":
      "The window in the foreground is only detected on KDE Plasma. Here, entries for the foreground count while the application is running.",
    "apps.title": "Voice chat applications",
    "apps.help": "Applications that are recording from a microphone right now. Selected ones are muted while you hold the key.",
    "apps.none": "No application is recording right now. Join a voice channel to see it here.",
    "apps.mute": "Mute while held",
    "apps.mutedNow": "Muted",
    "rules.title": "Muted applications",
    "rules.help":
      "Each entry is matched against the application name, the process name and the Flatpak ID. Discord, Vesktop, WebCord, TeamSpeak and Mumble are preset.",
    "rules.placeholder": "Name, for example discord",
    "rules.add": "Add",
    "rules.remove": "Remove {name}",
    "options.title": "Options",
    "mode.label": "Mode",
    "mode.hold": "While held",
    "mode.toggle": "Switch on press",
    "mode.help.hold": "Muted while the key is held.",
    "mode.help.toggle": "Each press switches between muted and unmuted.",
    "delay.label": "Release delay",
    "delay.help": "Keeps voice chat muted briefly after release, so the end of a sentence stays in game.",
    "login.label": "Start at login",
    "login.help": "Runs in the background, also without tuxhub and without this window.",
    "mouse.BTN_SIDE": "Mouse 4 (back)",
    "mouse.BTN_EXTRA": "Mouse 5 (forward)",
    "mouse.BTN_MIDDLE": "Middle mouse button",
    "mouse.BTN_FORWARD": "Mouse forward",
    "mouse.BTN_BACK": "Mouse back",
    "mouse.BTN_TASK": "Mouse task button",
    "mouse.other": "Button {n}",
    "footer": "Push-to-Mute {version}",
  },
  de: {
    enabled: "Aktiv",
    "enabled.help": "Stummschalten pausieren oder fortsetzen",
    "state.active": "Bereit",
    "state.active.detail": "Sprachchat ist stumm, solange du {keys} hältst.",
    "state.muted": "Sprachchat stumm",
    "state.muted.detail": "Stumm: {apps}",
    "state.muted.none": "Gerade nimmt keine ausgewählte Anwendung auf.",
    "state.paused": "Pausiert",
    "state.paused.detail": "Push-to-Mute schaltet dein Mikrofon nicht stumm.",
    "state.no-key": "Keine Push-to-Talk-Taste festgelegt",
    "state.no-key.detail": "Füge die Taste hinzu, die du in deinen Spielen für Push-to-Talk nutzt.",
    "state.no-access": "Kein Zugriff auf Eingabegeräte",
    "state.no-access.detail": "Die Push-to-Talk-Taste kann nicht erkannt werden.",
    "state.no-audio": "PipeWire ist nicht verfügbar",
    "state.offline": "Verbindung wird hergestellt",
    "state.offline.detail": "Warte auf den Push-to-Mute-Dienst.",
    "state.waiting": "Wartet auf Anwendung",
    "state.waiting.detail": "Schaltet nur stumm in: {apps}.",
    "problem.no-access":
      "Zum Erkennen der Push-to-Talk-Taste ist Zugriff auf Eingabegeräte nötig. Richte ihn einmalig ein (fragt nach deinem Passwort) oder füge deinen Benutzer der Gruppe input hinzu und melde dich neu an.",
    "problem.setup": "Zugriff einrichten",
    "problem.devices": "Kein angeschlossenes Gerät hat die gewählten Tasten.",
    "problem.setupFailed": "Einrichtung fehlgeschlagen: {error}",
    "keys.title": "Push-to-Talk-Tasten",
    "keys.add": "Taste hinzufügen",
    "keys.help":
      "Nutze dieselbe Taste wie im Spiel. Das Spiel erhält die Taste weiterhin, sein Push-to-Talk funktioniert also wie gewohnt.",
    "keys.none": "Noch keine Taste.",
    "keys.capture": "Drücke jetzt die Taste. Esc bricht ab.",
    "keys.cancel": "Abbrechen",
    "keys.remove": "{key} entfernen",
    "only.title": "Nur in bestimmten Anwendungen",
    "only.help":
      "Ohne Einträge immer aktiv. Mit Einträgen schaltet Push-to-Mute nur stumm, während eine davon läuft oder im Vordergrund ist – so schaltet die Taste zum Beispiel im Browser nicht den Sprachchat stumm.",
    "only.none": "Keine Einträge: in jeder Anwendung aktiv.",
    "only.placeholder": "Name, zum Beispiel cs2",
    "only.add": "Hinzufügen",
    "only.remove": "{name} entfernen",
    "only.running": "Läuft",
    "only.focused": "Im Vordergrund",
    "only.met": "Erfüllt",
    "windows.title": "Geöffnete Fenster",
    "windows.help": "Schalte ein Fenster ein, damit nur stummgeschaltet wird, während es im Vordergrund ist.",
    "windows.none": "Kein anderes Fenster geöffnet.",
    "windows.focused": "Im Vordergrund",
    "windows.select": "Nur im Vordergrund stummschalten",
    "windows.unavailable":
      "Welches Fenster im Vordergrund ist, wird nur unter KDE Plasma erkannt. Hier gelten Einträge für den Vordergrund, solange die Anwendung läuft.",
    "apps.title": "Sprachchat-Anwendungen",
    "apps.help": "Anwendungen, die gerade vom Mikrofon aufnehmen. Ausgewählte werden stummgeschaltet, solange du die Taste hältst.",
    "apps.none": "Gerade nimmt keine Anwendung auf. Tritt einem Sprachkanal bei, um sie hier zu sehen.",
    "apps.mute": "Beim Halten stumm",
    "apps.mutedNow": "Stumm",
    "rules.title": "Stummgeschaltete Anwendungen",
    "rules.help":
      "Jeder Eintrag wird mit dem Anwendungsnamen, dem Prozessnamen und der Flatpak-ID verglichen. Discord, Vesktop, WebCord, TeamSpeak und Mumble sind voreingestellt.",
    "rules.placeholder": "Name, zum Beispiel discord",
    "rules.add": "Hinzufügen",
    "rules.remove": "{name} entfernen",
    "options.title": "Optionen",
    "mode.label": "Modus",
    "mode.hold": "Beim Halten",
    "mode.toggle": "Umschalten per Tastendruck",
    "mode.help.hold": "Stumm, solange die Taste gehalten wird.",
    "mode.help.toggle": "Jeder Tastendruck wechselt zwischen stumm und laut.",
    "delay.label": "Nachlaufzeit",
    "delay.help": "Hält den Sprachchat nach dem Loslassen kurz stumm, damit das Satzende im Spiel bleibt.",
    "login.label": "Beim Anmelden starten",
    "login.help": "Läuft im Hintergrund, auch ohne tuxhub und ohne dieses Fenster.",
    "mouse.BTN_SIDE": "Maustaste 4 (zurück)",
    "mouse.BTN_EXTRA": "Maustaste 5 (vor)",
    "mouse.BTN_MIDDLE": "Mittlere Maustaste",
    "mouse.BTN_FORWARD": "Maus vor",
    "mouse.BTN_BACK": "Maus zurück",
    "mouse.BTN_TASK": "Maus-Aufgabentaste",
    "mouse.other": "Taste {n}",
    "footer": "Push-to-Mute {version}",
  },
};

// Labels of named keys. Character keys depend on the keyboard layout, so
// their kernel names are shown instead of guessed characters.
const KEY_NAMES = {
  LEFTCTRL: "Left Ctrl",
  RIGHTCTRL: "Right Ctrl",
  LEFTSHIFT: "Left Shift",
  RIGHTSHIFT: "Right Shift",
  LEFTALT: "Left Alt",
  RIGHTALT: "Right Alt",
  LEFTMETA: "Left Super",
  RIGHTMETA: "Right Super",
  CAPSLOCK: "Caps Lock",
  SPACE: "Space",
  TAB: "Tab",
  ENTER: "Enter",
};

const $ = (id) => document.getElementById(id);

let locale = navigator.language?.toLowerCase().startsWith("de") ? "de" : "en";
let status = { state: "offline" };
let config = null;
let streams = [];
let windows = [];
let setupError = null;

function t(key, vars = {}) {
  let s = STRINGS[locale][key] ?? STRINGS.en[key] ?? key;
  for (const [k, v] of Object.entries(vars)) s = s.replaceAll(`{${k}}`, String(v));
  return s;
}

function keyLabel(name) {
  if (STRINGS.en[`mouse.${name}`]) return t(`mouse.${name}`);
  if (name.startsWith("BTN_")) return t("mouse.other", { n: name.slice(4).toLowerCase() });
  const k = name.replace(/^KEY_/, "");
  if (KEY_NAMES[k]) return KEY_NAMES[k];
  if (/^KP/.test(k)) return `Num ${k.slice(2)}`;
  return k.length === 1 ? k : k.charAt(0) + k.slice(1).toLowerCase();
}

function el(tag, props = {}, ...children) {
  const e = document.createElement(tag);
  for (const [k, v] of Object.entries(props)) {
    if (k === "class") e.className = v;
    else if (k.startsWith("on")) e.addEventListener(k.slice(2), v);
    else if (v !== undefined && v !== null) e.setAttribute(k, v);
  }
  for (const c of children) if (c !== null && c !== undefined) e.append(c);
  return e;
}

function showError(e) {
  $("error").textContent = e ? String(e.message ?? e) : "";
}

async function update(patch) {
  showError(null);
  try {
    config = await tuxhub.call("config.set", patch);
    render();
  } catch (e) {
    showError(e);
    render();
  }
}

// ------------------------------------------------------------------ Render

function applyTexts() {
  document.documentElement.lang = locale;
  for (const n of document.querySelectorAll("[data-i18n]")) n.textContent = t(n.dataset.i18n);
  for (const n of document.querySelectorAll("[data-i18n-title]")) n.title = t(n.dataset.i18nTitle);
  for (const n of document.querySelectorAll("[data-i18n-placeholder]")) n.placeholder = t(n.dataset.i18nPlaceholder);
  for (const n of document.querySelectorAll("[data-i18n-aria-label]")) n.setAttribute("aria-label", t(n.dataset.i18nAriaLabel));
}

// A tag with a remove button. The built-in remove button of wa-tag cannot be
// reached with the keyboard and is not named after the tag, so it is not used.
function chip(text, removeLabel, onRemove, props = {}) {
  return el(
    "wa-tag",
    props,
    text,
    el(
      "wa-button",
      { class: "remove", appearance: "plain", size: "s", title: removeLabel, onclick: onRemove },
      el("wa-icon", { name: "xmark", library: "system", variant: "solid", label: removeLabel }),
    ),
  );
}

function renderStatus() {
  const state = status.state ?? "offline";
  $("hero").dataset.state = state;
  $("state-title").textContent = t(`state.${state}`);
  let detail = "";
  if (state === "active") {
    detail = t("state.active.detail", { keys: (config?.keys ?? []).map(keyLabel).join(" / ") });
  } else if (state === "muted") {
    const apps = status.matched ?? [];
    detail = apps.length ? t("state.muted.detail", { apps: apps.join(", ") }) : t("state.muted.none");
  } else if (state === "waiting") {
    detail = t("state.waiting.detail", { apps: (config?.only_when ?? []).map((c) => c.app).join(", ") });
  } else if (state === "no-audio") {
    detail = status.text ?? "";
  } else {
    detail = STRINGS.en[`state.${state}.detail`] ? t(`state.${state}.detail`) : "";
  }
  $("state-detail").textContent = detail;
  $("enabled").checked = status.enabled ?? config?.enabled ?? false;
  $("enabled").disabled = state === "offline";

  let problem = null;
  if (setupError) problem = t("problem.setupFailed", { error: setupError });
  else if (state === "no-access") problem = t("problem.no-access");
  else if (state === "active" && status.devices === 0) problem = t("problem.devices");
  $("problem").classList.toggle("hidden", !problem);
  $("problem-text").textContent = problem ?? "";
  $("setup").classList.toggle("hidden", state !== "no-access" && !setupError);

  const learning = !!status.learning;
  $("capture").classList.toggle("hidden", !learning);
  $("learn").disabled = learning || state === "offline";

  const held = !!status.held && status.enabled;
  for (const tag of document.querySelectorAll("wa-tag.key")) tag.variant = held ? "danger" : "brand";
  renderMet();
  if (!!status.focus_available !== focusShown) renderWindows();
  else renderWindowHints();
  $("footer").textContent = status.version ? t("footer", { version: status.version }) : "";
}

function renderKeys() {
  const box = $("keys");
  box.replaceChildren();
  const keys = config?.keys ?? [];
  if (!keys.length) box.append(el("span", { class: "placeholder" }, t("keys.none")));
  for (const k of keys) {
    const label = keyLabel(k);
    box.append(
      chip(label, t("keys.remove", { key: label }), () => update({ keys: keys.filter((x) => x !== k) }), {
        class: "key",
        variant: "brand",
        title: k,
      }),
    );
  }
}

function matches(stream, terms) {
  const fields = [stream.name, stream.binary, stream.app_id].map((s) => (s ?? "").toLowerCase());
  return terms.some((term) => term && fields.some((f) => f.includes(term)));
}

function renderStreams() {
  const list = $("streams");
  list.replaceChildren();
  const terms = config?.apps ?? [];
  for (const s of streams) {
    const detail = [s.binary, s.app_id].filter((x) => x && x !== s.name).join(" · ");
    const box = el(
      "wa-switch",
      { class: "setting", title: t("apps.mute") },
      el("span", { class: "name" }, s.name || s.binary || `#${s.id}`),
      s.muted ? el("span", { class: "badge" }, t("apps.mutedNow")) : null,
      detail ? el("span", { slot: "hint" }, detail) : null,
    );
    box.checked = matches(s, terms);
    box.addEventListener("change", () => {
      if (box.checked) update({ apps: [...terms, s.term] });
      else update({ apps: terms.filter((term) => !matches(s, [term])) });
    });
    list.append(box);
  }
  $("streams-empty").classList.toggle("hidden", streams.length > 0);
}

function renderRules() {
  const box = $("rules");
  box.replaceChildren();
  const terms = config?.apps ?? [];
  for (const term of terms) {
    box.append(chip(term, t("rules.remove", { name: term }), () => update({ apps: terms.filter((x) => x !== term) })));
  }
}

function setConditions(list) {
  update({ only_when: list });
}

function renderConditions() {
  const box = $("conditions");
  box.replaceChildren();
  const list = config?.only_when ?? [];
  for (const c of list) {
    const label = t("only.remove", { name: c.app });
    const select = el(
      "wa-select",
      { class: "setting" },
      el(
        "span",
        { slot: "label" },
        el("span", { class: "name" }, c.app),
        el("span", { class: "badge success hidden", "data-met": c.app }, t("only.met")),
      ),
      el("wa-option", { value: "running" }, t("only.running")),
      el("wa-option", { value: "focused" }, t("only.focused")),
    );
    select.value = c.when;
    select.addEventListener("change", () =>
      setConditions(list.map((x) => (x.app === c.app ? { ...x, when: select.value } : x))),
    );
    const remove = el(
      "wa-button",
      { appearance: "plain", title: label, onclick: () => setConditions(list.filter((x) => x.app !== c.app)) },
      el("wa-icon", { name: "xmark", library: "system", variant: "solid", label }),
    );
    box.append(el("div", { class: "condition" }, select, remove));
  }
  $("conditions-empty").classList.toggle("hidden", list.length > 0);
  renderMet();
}

// Marks the conditions that are met, without rebuilding the rows.
function renderMet() {
  const met = status.met ?? [];
  for (const b of document.querySelectorAll("[data-met]")) b.classList.toggle("hidden", !met.includes(b.dataset.met));
}

let focusShown = false;

function renderWindowHints() {
  const offline = (status.state ?? "offline") === "offline";
  $("windows-unavailable").classList.toggle("hidden", focusShown || offline);
  $("windows-help").classList.toggle("hidden", !focusShown);
  $("windows-empty").classList.toggle("hidden", !focusShown || windows.length > 0);
}

function renderWindows() {
  focusShown = !!status.focus_available;
  renderWindowHints();
  const list = $("windows");
  // The list changes with every window switch; keep the keyboard focus.
  const focused = document.activeElement?.closest?.("#windows wa-switch")?.dataset.term;
  list.replaceChildren();
  if (!focusShown) return;
  const conditions = config?.only_when ?? [];
  for (const w of windows) {
    const box = el(
      "wa-switch",
      { class: "setting", title: t("windows.select"), "data-term": w.term },
      el("span", { class: "name" }, w.term),
      w.active ? el("span", { class: "badge success" }, t("windows.focused")) : null,
      w.class && w.class.toLowerCase() !== w.term ? el("span", { slot: "hint" }, w.class) : null,
    );
    box.checked = w.matched;
    box.addEventListener("change", () => {
      if (!box.checked) return setConditions(conditions.filter((c) => !(w.matches ?? []).includes(c.app)));
      if (conditions.some((c) => c.app === w.term)) {
        setConditions(conditions.map((c) => (c.app === w.term ? { ...c, when: "focused" } : c)));
      } else {
        setConditions([...conditions, { app: w.term, when: "focused" }]);
      }
    });
    list.append(box);
    if (w.term === focused) box.focus();
  }
}

function renderOptions() {
  if (!config) return;
  $("mode").value = config.mode;
  $("mode-help").textContent = t(`mode.help.${config.mode}`);
  if (document.activeElement !== $("delay")) $("delay").value = config.release_delay_ms;
  $("login").checked = config.start_at_login;
}

function render() {
  applyTexts();
  renderKeys();
  renderConditions();
  renderWindows();
  renderStreams();
  renderRules();
  renderOptions();
  renderStatus();
}

// ------------------------------------------------------------------ Events

$("enabled").addEventListener("change", (e) => update({ enabled: e.target.checked }));
$("mode").addEventListener("change", (e) => update({ mode: e.target.value }));
$("login").addEventListener("change", (e) => update({ start_at_login: e.target.checked }));
$("delay").addEventListener("change", (e) => {
  const v = Math.round(Number(e.target.value));
  if (Number.isFinite(v)) update({ release_delay_ms: Math.min(2000, Math.max(0, v)) });
});

$("rule-form").addEventListener("submit", (e) => {
  e.preventDefault();
  const term = $("rule-input").value.trim().toLowerCase();
  if (!term) return;
  $("rule-input").value = "";
  const terms = config?.apps ?? [];
  if (!terms.includes(term)) update({ apps: [...terms, term] });
});

$("condition-form").addEventListener("submit", (e) => {
  e.preventDefault();
  const app = $("condition-input").value.trim().toLowerCase();
  if (!app) return;
  $("condition-input").value = "";
  const list = config?.only_when ?? [];
  // Without focus detection, entries for the foreground would only count as running.
  const when = status.focus_available ? "focused" : "running";
  if (!list.some((c) => c.app === app)) setConditions([...list, { app, when }]);
});

$("learn").addEventListener("click", async () => {
  showError(null);
  // Release focus, so the key press is not taken by the button.
  $("learn").blur();
  try {
    await tuxhub.call("learn.start");
    status = { ...status, learning: true };
    renderStatus();
  } catch (e) {
    showError(e);
  }
});
$("learn-cancel").addEventListener("click", () => tuxhub.call("learn.cancel").catch(showError));

$("setup").addEventListener("click", async () => {
  setupError = null;
  try {
    await tuxhub.action("setup");
  } catch (e) {
    showError(e);
  }
});

tuxhub.on("status", (s) => {
  status = s ?? { state: "offline" };
  renderStatus();
  if (status.state === "offline") return;
  // After a reconnect the configuration may have changed.
  if (!config) load();
});
tuxhub.on("config", (c) => {
  config = c;
  render();
});
tuxhub.on("streams", (list) => {
  streams = list ?? [];
  renderStreams();
});
tuxhub.on("windows", (list) => {
  windows = list ?? [];
  renderWindows();
});
tuxhub.on("learned", ({ key }) => {
  status = { ...status, learning: false };
  if (key && config && !config.keys.includes(key)) update({ keys: [...config.keys, key] });
  else renderStatus();
});
tuxhub.on("setup", ({ error }) => {
  setupError = error;
  renderStatus();
});
function applyTheme(dark) {
  document.documentElement.classList.toggle("wa-dark", dark);
  document.documentElement.classList.toggle("wa-light", !dark);
}

// The live connection of tuxhub ended (service quit or restarted after an
// update): show the state and connect again, which starts the service.
tuxhub.on("closed", () => {
  status = { state: "offline" };
  config = null;
  renderStatus();
  setTimeout(load, 1000);
});
tuxhub.on("theme", ({ dark }) => applyTheme(dark));
tuxhub.on("locale", ({ locale: l }) => {
  locale = l === "de" ? "de" : "en";
  render();
});

async function load() {
  try {
    const [c, s, list, open] = await Promise.all([
      tuxhub.call("config.get"),
      tuxhub.call("status"),
      tuxhub.call("streams"),
      tuxhub.call("windows"),
    ]);
    config = c;
    status = s;
    streams = list;
    windows = open ?? [];
    showError(null);
  } catch (e) {
    showError(e);
  }
  render();
}

if (tuxhub.standalone) document.documentElement.dataset.standalone = "";
applyTheme(matchMedia("(prefers-color-scheme: dark)").matches);
render();
load();
