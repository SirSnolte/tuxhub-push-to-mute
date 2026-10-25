<p align="center">
  <img src="assets/icon.svg" width="96" height="96" alt="Push-to-Mute icon">
</p>

<h1 align="center">Push-to-Mute</h1>

<p align="center">
  Talk in game without being heard in Discord, TeamSpeak or Mumble.
</p>

Many games have their own voice chat with push-to-talk. While you hold that
key, your friends in Discord usually hear everything you say to strangers in
the game. Discord's own push-to-talk does not solve this, as it works the
other way around.

Push-to-Mute mutes your voice chat applications for as long as you hold the
game's push-to-talk key. The game still receives the key and your voice, so
in-game push-to-talk keeps working. When you release the key, you are back in
your voice chat.

Push-to-Mute is part of the [tuxhub](https://github.com/SirSnolte/tuxhub)
family of gaming tools for Linux. It works on its own and integrates with
tuxhub, whichever is installed first.

## Features

- Any keyboard key or extra mouse button as trigger, set by pressing it.
  Several keys are possible, for example one per game.
- The key is only read, never captured: games, the desktop and other
  applications receive it as usual.
- Mutes only the recording of the selected applications, not the microphone
  itself. Presets for Discord (including the Flatpak), Vesktop, WebCord,
  TeamSpeak and Mumble; others can be added.
- Hold mode (muted while held) or toggle mode (each press switches).
- Release delay, so the end of a sentence does not reach the voice chat.
- Optionally only active in certain applications, for example only while a
  game is running or in the foreground, so the key does not mute the voice
  chat in the browser. The foreground window is detected on KDE Plasma 6;
  elsewhere such entries count while the application is running.
- Runs in the background from login, without a window and without tuxhub.
- Works on Wayland and X11 with PipeWire.

## Installation

### With tuxhub

Install Push-to-Mute from the workshop in tuxhub (version 0.7.0 or newer).
tuxhub takes care of updates, starts the background service when needed and
shows the interface on its own page.

### Without tuxhub

Download the archive from the
[latest release](https://github.com/SirSnolte/tuxhub-push-to-mute/releases/latest),
extract it and run the installer from the extracted folder:

```bash
tar -xf tuxhub-push-to-mute-x86_64-linux.tar.gz
./tuxhub-push-to-mute/tuxhub-push-to-mute install
```

This installs Push-to-Mute for your user into
`~/.local/share/tuxhub-push-to-mute`, adds it to the application menu and
starts it at login. No administrator rights are needed. If you install tuxhub
later, it detects the installation and integrates it, including updates.

To remove it again:

```bash
tuxhub-push-to-mute uninstall           # keeps the settings
tuxhub-push-to-mute uninstall --purge   # also removes the settings
```

### Requirements

- PipeWire with WirePlumber (the default on current distributions)
- Read access to input devices, to detect the push-to-talk key. Members of the
  `input` group have it. Otherwise the interface offers to set it up once
  with a udev rule, which asks for your password.

## Usage

1. Open Push-to-Mute from the application menu or in tuxhub.
2. Choose **Add key** and press the key you use for push-to-talk in your game.
3. Join a voice channel. Your voice chat appears in the list of applications
   and is selected if it is one of the presets.

Hold the key in game: the status turns red and your voice chat is muted.

### Command line

```bash
tuxhub-push-to-mute status          # current state
tuxhub-push-to-mute learn           # press a key to add it
tuxhub-push-to-mute keys BTN_EXTRA  # set the keys by name
tuxhub-push-to-mute streams         # applications recording right now
tuxhub-push-to-mute apps discord teamspeak
tuxhub-push-to-mute mode toggle
tuxhub-push-to-mute only cs2 focused:eldenring.exe  # only in these applications
tuxhub-push-to-mute only            # active everywhere again
tuxhub-push-to-mute windows         # open windows (KDE Plasma)
tuxhub-push-to-mute disable         # pause; enable resumes
```

Key names follow the Linux input subsystem: `BTN_SIDE` and `BTN_EXTRA` are
the side buttons of most mice, `KEY_V` is the V key.

`only` takes terms that are matched against process names (and, for the
foreground, against the window class): `cs2` or `running:cs2` applies while
such a process runs, `focused:cs2` while its window is in the foreground.
For Proton games, use the name of the `.exe`.

## How it works

A small background service watches the push-to-talk keys on all connected
keyboards and mice without grabbing them, and follows the applications that
record audio through PipeWire. While a key is held, it mutes the recording
streams of the selected applications with `wpctl`. When it stops, it always
unmutes them again.

The window, tuxhub and the command line all talk to this service over a
local socket, so every change shows up everywhere at once. Settings are
stored in `~/.config/tuxhub-push-to-mute/config.json`.

Only the selected keys are evaluated. Every other key is discarded
immediately and nothing is stored or sent anywhere.

When muting is limited to certain applications, the service compares the
names of your own processes from `/proc` every two seconds. On KDE Plasma it
also loads a small KWin script that reports the class and process of each
open window over the session bus, never window titles. The script is removed
when the service ends.

## Building

Building requires Rust and the development files of WebKitGTK 4.1
(`webkit2gtk4.1-devel` on Fedora, `libwebkit2gtk-4.1-dev` on Debian and
Ubuntu).

```bash
cargo build --release
cargo test
```

## License

MIT
