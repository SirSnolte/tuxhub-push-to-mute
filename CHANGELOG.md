# Changelog

All notable changes are documented in this file. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and versions follow
[Semantic Versioning](https://semver.org/).

## [0.3.0] - 2026-10-23

### Added

- Muting can be limited to certain applications: while one of them is
  running, or while its window is in the foreground. Without entries,
  Push-to-Mute stays active everywhere. The status shows when it is waiting
  for an application.
- List of open windows with a switch each, on KDE Plasma 6. Other desktops
  treat entries for the foreground like running applications.
- Commands `only` and `windows`; `status` shows the conditions.

## [0.2.0] - 2026-10-23

### Changed

- Interface rebuilt with Web Awesome and the tuxhub theme, so the window and
  the page inside tuxhub match tuxhub.
- Keys and applications can be removed with the keyboard; each remove button
  is named after its entry.
- Each recording application is a row with a switch; the whole row toggles it.

## [0.1.0] - 2026-10-22

First release.

### Added

- Mutes selected voice chat applications while a push-to-talk key is held,
  or switches on each press. The key still reaches the game.
- Any keyboard key or extra mouse button as trigger, set by pressing it;
  several keys at once.
- Live list of the applications recording from a microphone; presets for
  Discord, Vesktop, WebCord, TeamSpeak and Mumble.
- Adjustable release delay, pause and resume, start at login.
- Background service shared by the window, tuxhub and the command line.
- Installation without tuxhub, with launcher and login entry. tuxhub detects
  such an installation automatically.

[0.3.0]: https://github.com/SirSnolte/tuxhub-push-to-mute/releases/tag/v0.3.0
[0.2.0]: https://github.com/SirSnolte/tuxhub-push-to-mute/releases/tag/v0.2.0
[0.1.0]: https://github.com/SirSnolte/tuxhub-push-to-mute/releases/tag/v0.1.0
