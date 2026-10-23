# SPEC: Nur aktiv bei bestimmten Anwendungen

Stand: 23.10.2026 · Ziel-Version: 0.3.0

## 1. Ziel

Push-to-Mute schaltet nur noch stumm, wenn eine der gewählten Anwendungen
**läuft** oder **im Vordergrund** ist, je nach Eintrag. Ohne Einträge verhält
es sich wie bisher (immer aktiv). So mutet z. B. Maustaste 5 im Browser nicht
mehr versehentlich Discord, wenn gerade kein Spiel aktiv ist.

## 2. Nicht-Ziele

- Bedingungen pro Taste (gilt global für alle Tasten).
- Vordergrund-Erkennung außerhalb von KDE Plasma 6 (GNOME, Sway, Hyprland,
  X11-Desktops ohne KWin, Plasma 5). Dort gelten „im Vordergrund“-Einträge wie
  „läuft“ (Fallback, siehe 4.4). KDE unter X11 ist abgedeckt, da KWin-Skripte
  dort genauso laufen.
- Fenstertitel lesen, anzeigen oder speichern.
- Spiele in gamescope automatisch erkennen (das fokussierte Fenster ist dann
  gamescope; Nutzer kann `gamescope` eintragen).
- Prozesse anderer Benutzer.
- UND-Verknüpfung oder Ausschlüsse („nicht wenn …“). Einträge sind ODER-verknüpft.
- Automatisch erkennen, welche Anwendungen Spiele sind.

## 3. Dateien & Schnittstellen

| Datei | Änderung |
|---|---|
| `Cargo.toml` | neue Abhängigkeit `zbus = "6.1"` (vom Nutzer freigegeben), nur nötige Features; Version 0.3.0 |
| `src/config.rs` | neues Feld `only_when: Vec<Condition>`, Typen `Condition`, `When`; Validierung/Normalisierung |
| `src/conditions.rs` (neu) | Prozess-Scan aus `/proc`, Fenster-Schnappschuss, Abgleich, `allowed()` |
| `src/kwin.rs` (neu) | D-Bus-Dienst (zbus), KWin-Skript laden/entladen/neu laden, Absenderprüfung |
| `src/kwin.js` (neu) | KWin-Skript (per `include_str!` eingebettet) |
| `src/daemon.rs` | neue `Msg`-Varianten, Bedingungs-Gate in `input()`, Prozess-Scan-Timer, Zustand `waiting`, RPC `windows` + Benachrichtigung `windows` |
| `src/main.rs` | CLI-Befehle `only` und `windows`, Statusausgabe erweitert |
| `src/lib.rs` | neue Module anmelden |
| `ui/index.html`, `ui/app.js`, `ui/style.css` | neue Karte „Nur in bestimmten Anwendungen“, Zustand `waiting` |
| `README.md`, `CHANGELOG.md`, `tuxhub.toml` | Doku, Version 0.3.0 |

### Konfiguration (`config.json`)

```json
"only_when": [
  { "app": "cs2", "when": "running" },
  { "app": "eldenring.exe", "when": "focused" }
]
```

- `app`: Suchbegriff, getrimmt, kleingeschrieben, 1–64 Zeichen.
- `when`: `"running"` oder `"focused"`.
- Höchstens 32 Einträge. Gleicher `app` doppelt → nur der erste bleibt.
- Fehlt das Feld (alte Datei) → leere Liste.

### RPC (Unix-Socket, gleiches Protokoll wie bisher)

- `config.set` akzeptiert `only_when` (strikt validiert wie die übrigen Felder).
- `windows` (neu) → Liste offener Fenster:
  `[{ "term": "cs2", "class": "steam_app_730", "active": true, "matched": true }]`
  (`matched` = von einem Eintrag erfasst). Leer, wenn keine Fenster-Erkennung.
- Benachrichtigung `windows` (neu) bei jeder Änderung der Liste.
- `status` erhält zusätzlich:
  - `state: "waiting"` (neu, siehe 4.3)
  - `allowed: bool` – Bedingungen erfüllt (bzw. keine gesetzt)
  - `met: [String]` – aktuell erfüllte Einträge (`app`)
  - `focus_available: bool` – Vordergrund-Erkennung läuft

### D-Bus (neu, Sitzungsbus)

- Name `io.github.sirsnolte.PushToMute`, Objekt `/io/github/sirsnolte/PushToMute`,
  Schnittstelle `io.github.sirsnolte.PushToMute.Windows`, Methode
  `Update(s json)`. Wird ausschließlich vom eigenen KWin-Skript aufgerufen.
- KWin-Skript (Plasma-6-API): sendet bei `windowActivated`, `windowAdded`,
  `windowRemoved` und Klassenänderung einen Schnappschuss
  `{ "active": "<id>", "windows": [{ "id", "class", "name", "desktop", "pid" }] }`
  aller `normalWindow`-Fenster ohne `skipTaskbar`. Keine Titel.

### CLI

- `tuxhub-push-to-mute only cs2 focused:eldenring.exe` – ersetzt die Liste;
  `running:` oder ohne Präfix = läuft, `focused:` = im Vordergrund.
- `tuxhub-push-to-mute only` ohne Argumente – Liste leeren (immer aktiv).
- `tuxhub-push-to-mute windows` – offene Fenster mit Begriff und Klasse.
- `status` zeigt Bedingungen und ob sie erfüllt sind.

## 4. Verhaltensvertrag

### 4.1 Abgleich

- Begriffe werden wie bei den Sprachchat-Anwendungen als **Teilstring ohne
  Groß-/Kleinschreibung** verglichen.
- **Prozessnamen** eines Prozesses: `/proc/<pid>/comm` und der Dateiname aus
  `argv[0]` (getrennt an `/` und `\`, damit Proton-Pfade wie
  `Z:\…\Cyberpunk2077.exe` greifen; `comm` ist auf 15 Zeichen gekürzt).
  Nur Prozesse mit eigener UID. Unlesbare Prozesse werden übersprungen.
- **„läuft“** ist erfüllt, wenn irgendein eigener Prozess passt.
- **„im Vordergrund“** ist erfüllt, wenn das aktive Fenster passt: `class`,
  `name`, `desktop` oder die Prozessnamen seiner `pid`.
- `allowed = only_when ist leer ODER mindestens ein Eintrag erfüllt`.

### 4.2 Erkennung

- Prozess-Scan alle 2 s, **nur wenn `only_when` nicht leer ist**. Zusätzlich
  sofort beim Tastendruck, wenn `allowed` gerade falsch ist (Spiel erst
  gestartet).
- Fenster: Der Dienst prüft beim Start, ob `org.kde.KWin` auf dem Sitzungsbus
  existiert. Wenn ja: D-Bus-Namen belegen, Skript nach
  `$XDG_RUNTIME_DIR/tuxhub-push-to-mute-kwin.js` schreiben (0600, fester Inhalt),
  gleichnamiges altes Skript entladen, laden, starten. Bei Neustart von KWin
  (`NameOwnerChanged` für `org.kde.KWin`) neu laden. Beim Beenden des Dienstes
  entladen.
- `focus_available` ist wahr, sobald ein gültiger Schnappschuss angekommen ist,
  und wird falsch, wenn KWin verschwindet.

### 4.3 Stummschalten mit Bedingung (Annahme „sofort laut“, vom Nutzer offen gelassen)

- **Tastendruck**: `held = true`. Ist `allowed` falsch → nichts stummschalten.
  Sonst wie bisher (Halten bzw. Umschalten).
- **Loslassen**: wie bisher.
- **`allowed` wechselt von wahr auf falsch** (Alt-Tab, Spiel beendet): sofort
  entstummen, ohne Nachlaufzeit; eine geplante Entstummung entfällt;
  Umschalt-Modus wird damit zurückgesetzt.
- **`allowed` wechselt von falsch auf wahr, während die Taste gehalten wird**:
  nicht stummschalten; erst der nächste Tastendruck zählt.
- **Statusreihenfolge**: `paused` > `no-key` > `no-access` > `no-audio` >
  `muted` > `waiting` (Einträge gesetzt, `allowed` falsch) > `active`.
- `running` im Status bleibt nur bei `active`/`muted` wahr.

### 4.4 Fallback ohne Vordergrund-Erkennung

- `focus_available` falsch → „im Vordergrund“-Einträge werden wie „läuft“
  ausgewertet. Die Oberfläche zeigt einen Hinweis, die Fensterliste entfällt.

### 4.5 Ungültige Eingaben

- `config.set` mit ungültigem `only_when` (kein Array, >32 Einträge, leerer oder
  >64 Zeichen langer Begriff, unbekanntes `when`, fremde Felder) → Fehler, die
  gespeicherte Konfiguration bleibt unverändert.
- Handbearbeitete Datei mit ungültigen Einträgen → ungültige Einträge werden
  verworfen, der Rest gilt (wie bei `keys`/`apps`).
- D-Bus `Update` von einem anderen Absender als dem aktuellen Besitzer von
  `org.kde.KWin` → abgelehnt (D-Bus-Fehler), Zustand unverändert.
- `Update` mit ungültigem JSON, mehr als 256 KiB, mehr als 256 Fenstern oder
  Strings über 256 Zeichen → verworfen bzw. betroffene Fenster übersprungen;
  der letzte gültige Schnappschuss bleibt.
- CLI `only` mit unbekanntem Präfix (z. B. `foo:cs2`) → Fehlermeldung, Exit 1.

### 4.6 Oberfläche

- Neue Karte nach „Push-to-Talk-Tasten“: Titel „Nur in bestimmten Anwendungen“,
  Hilfetext „Ohne Einträge immer aktiv …“.
- Einträge als Zeilen: Begriff, Auswahl „Läuft“/„Im Vordergrund“, Badge, wenn
  gerade erfüllt, Entfernen-Knopf (mit Tastatur erreichbar, benannt).
- Eingabe + „Hinzufügen“ für Freitext. Neue Einträge: „Im Vordergrund“, wenn
  verfügbar, sonst „Läuft“.
- Unterabschnitt „Geöffnete Fenster“: Live-Liste mit Schalter pro Fenster
  (Name = Begriff, Hinweis = Klasse, Badge „Im Vordergrund“ beim aktiven).
  Schalter an → Eintrag hinzufügen; aus → passende Einträge entfernen.
- Ohne Vordergrund-Erkennung: Hinweis statt Fensterliste.
- Statuskarte `waiting`: Titel „Wartet auf Anwendung“, Detail nennt die Einträge.
- Nur Web-Awesome-Bauteile und `--wa-*`-Tokens; Texte de/en. Vor dem Bau Skill
  `app-design-defaults` laden.

## 5. Randfälle (je ein Test)

1. `only_when` leer → Tastendruck mutet wie bisher.
2. Einträge gesetzt, keiner erfüllt → Tastendruck mutet nicht, `held` wahr, Status `waiting`.
3. „läuft“ + passender eigener Prozess (Teilstring, Groß-/Kleinschreibung egal) → mutet.
4. Proton: `comm` = `Cyberpunk2077.e`, `argv[0]` = `Z:\Games\Cyberpunk2077.exe` → Begriff `cyberpunk2077.exe` passt.
5. Prozess einer fremden UID mit passendem Namen → zählt nicht.
6. „im Vordergrund“ + aktives Fenster passt über `class` / über Prozessname der `pid` → erfüllt; anderes Fenster aktiv → nicht erfüllt.
7. „im Vordergrund“ ohne Erkennung → wird wie „läuft“ ausgewertet.
8. Halten: Bedingung fällt während gehaltener Taste weg → sofort laut; kehrt zurück, Taste noch gehalten → bleibt laut bis zum nächsten Druck.
9. Umschalten: stumm, Bedingung fällt weg → laut; nächster Druck (Bedingung erfüllt) → stumm.
10. Nachlaufzeit läuft, Bedingung fällt weg → sofort laut, Timer gelöscht.
11. Tastendruck bei `allowed` falsch, Prozess erst seit <2 s da → Sofort-Scan → mutet.
12. Pausiert (`enabled` falsch) → Status `paused`, unabhängig von Bedingungen.
13. Validierung: 33 Einträge, leerer Begriff, 65 Zeichen, `when: "foo"`, fremdes Feld → Fehler; Duplikate und Leerzeichen/Großschreibung normalisiert.
14. Alte `config.json` ohne `only_when` → leere Liste; Datei mit ungültigen Einträgen → nur diese verworfen.
15. Schnappschuss: ungültiges JSON, >256 KiB, >256 Fenster, zu lange Strings → wie in 4.5.
16. `Update` von fremdem D-Bus-Absender → abgelehnt.
17. Fensterliste: nur normale Fenster, eigenes Tool-Fenster ausgeblendet, gleiche Begriffe zusammengefasst; Begriff aus `argv[0]`, wenn `comm` gekürzt ist; ohne `pid` die Klasse.
18. CLI-Parser: `cs2`, `running:cs2`, `focused:x` korrekt, `foo:x` Fehler, keine Argumente → leere Liste.
19. Prozess-Scan läuft nicht, solange `only_when` leer ist.

KWin-Lebenszyklus (laden, Neustart, entladen) wird im End-to-End-Check geprüft,
nicht per Unit-Test.

## 6. Sicherheit

- **Externe Eingaben**: `/proc`-Inhalte (von anderen Prozessen steuerbar;
  begrenzte Lesegröße, nur eigene UID, nur Vergleich), D-Bus-`Update`
  (Absenderprüfung gegen Besitzer von `org.kde.KWin`, Größen- und
  Feldgrenzen), `config.set` über den Socket (strikte Validierung), CLI-Argumente.
- Gefälschte Prozess- oder Fensternamen können höchstens bewirken, dass
  gemutet wird bzw. nicht; dasselbe kann jeder Prozess des Nutzers ohnehin
  per `wpctl`.
- KWin-Skript: fester, einkompilierter Inhalt, keine Nutzereingaben darin;
  Datei 0600 im Laufzeitordner.
- Keine Shell-Befehle aus Eingaben. Fenstertitel werden nie gelesen. Nichts
  wird gespeichert außer `only_when` in der eigenen Konfiguration.
- **Neue Abhängigkeit**: `zbus` 6.1.0, MIT, github.com/z-galaxy/zbus, zuletzt
  aktualisiert 23.10.2026, ~100 Mio. Downloads, MSRV 1.89 (CI nutzt stable).
  Vom Nutzer freigegeben.

## 7. End-to-End-Check

1. `cargo fmt --all --check && cargo clippy --all-targets -- -D warnings && cargo test` → alles grün (Ausgabe beilegen).
2. Isolierter Dienst (eigenes `XDG_CONFIG_HOME`, kurzes `XDG_RUNTIME_DIR` unter
   `/run/user/1000`, eigene D-Bus-Sitzung per `dbus-run-session`):
   - `only fakegame` → `status` zeigt `waiting`.
   - Kopie von `sleep` als `fakegame` starten → binnen 2 s `active`; beenden → wieder `waiting`.
3. KWin isoliert: `dbus-run-session -- kwin_wayland --virtual --no-lockscreen`,
   darin zwei Test-Fenster; Dienst starten → `windows` listet beide, `status`
   zeigt `focus_available: true`; Fenster aktivieren → `met` wechselt.
   Dienst beenden → `isScriptLoaded` falsch. KWin neu starten → Skript wieder geladen.
   Falls `--virtual` nicht reicht: Prüfung auf echtem Desktop nur lesend
   (`windows`, `status`), Skript danach entladen; im Bericht kennzeichnen.
4. Tastendruck-Pfad: über Tests auf Dienst-Ebene mit eingespeisten
   `input::Event`s (keine echten Eingaben ungegrabbt auf den Desktop).
5. Oberfläche: Screenshots im eigenen Fenster und eingebettet in tuxhub, hell
   und dunkel, bei Mindestbreite; Zustände: keine Einträge, `waiting`, erfüllt,
   ohne Vordergrund-Erkennung.
6. Abschließend durch den Nutzer: echtes Spiel, Alt-Tab, Maustaste 5 im Browser.
