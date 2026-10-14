<div align="center">

<img src="data/icons/hicolor/scalable/apps/TvMaxe.svg" width="96" alt="TV-Maxe icon">

# TV-Maxe

**Internet TV and radio player for Linux**

A from-scratch Rust rebuild of the classic TV-Maxe, with GTK 4 and libmpv.

[![CI](https://github.com/condor2/tv-maxe-rs/actions/workflows/ci.yml/badge.svg)](https://github.com/condor2/tv-maxe-rs/actions/workflows/ci.yml)
[![License: GPL v3+](https://img.shields.io/badge/license-GPL--3.0--or--later-blue.svg)](LICENSE)
![Rust 1.92+](https://img.shields.io/badge/rust-1.92%2B-orange.svg)
![GTK 4](https://img.shields.io/badge/GTK-4.14%2B-4a86cf.svg)

<img src="docs/screenshot.png" width="800" alt="TV-Maxe main window">

</div>

TV-Maxe keeps the window layout of the original application by Ovidiu Nițan and opens its channel
lists, but it is a complete rewrite and contains no code from the original project.

> [!NOTE]
> TV-Maxe ships without channel lists. Only add lists you have the right to use.

## Contents

- [Features](#features)
- [Installation](#installation)
- [Usage](#usage)
- [Supported list formats](#supported-list-formats)
- [Language](#language)
- [Data and settings](#data-and-settings)
- [KDE Plasma](#kde-plasma)
- [Development](#development)
- [Troubleshooting](#troubleshooting)
- [Roadmap](#roadmap)
- [License](#license)

## Features

- **TV and radio tabs** with channel logos and a *Channel list* picker for each subscription
- **Subscriptions** from the internet or from disk (TV-Maxe `.db`, M3U), which can be enabled,
  disabled and updated
- **Several sources per channel**: when one URL fails, the next is tried automatically; right-click a
  channel to pick a source yourself, and the info bar shows which one is playing (*source 2/3*)
- **My channels**: your own channels, added by hand, editable from the right-click menu and exportable
  as a TV-Maxe `.db`
- **TV guide (EPG)** from XMLTV (`.xml` / `.xml.gz`): what's on now under every channel, the current
  programme with progress and the next one under the video, and a day-by-day **TV guide** window
- **Large lists**: filter by group, diacritics-insensitive search, logos downloaded only for the
  visible rows (a 12,000-channel list loads in about two seconds)
- **Per-channel HTTP headers** (`http-referrer`, `http-user-agent`, `#EXTVLCOPT`) for servers that
  require them
- **Smooth playback on slow machines**: switches to mpv's fast scaling on software OpenGL or when too
  many frames are dropped
- **Imports** the old TV-Maxe local list (`~/.tvmaxe/userlist.db`) on first start
- **English and Romanian** interface, following the system language
- Works on **Wayland and X11**

## Installation

### 1. Dependencies

You need Rust 1.92 or newer, GTK 4.14 or newer and libmpv.

| Distribution | Command |
|---|---|
| Arch Linux | `sudo pacman -S --needed base-devel rust gtk4 mpv` |
| Debian / Ubuntu 24.04+ | `sudo apt install build-essential pkg-config libgtk-4-dev libmpv-dev`, plus Rust from [rustup.rs](https://rustup.rs) |
| Fedora | `sudo dnf install gcc pkgconf-pkg-config gtk4-devel mpv-libs-devel rust cargo` |

### 2. Install

Run the script as your normal user, not as root. It asks for `sudo` only to copy the files.

```sh
git clone https://github.com/condor2/tv-maxe-rs.git
cd tv-maxe-rs
./install.sh
```

The script checks the dependencies, builds the release binary and installs:

| File | Path |
|---|---|
| Program | `/usr/local/bin/tvmaxe` |
| Menu entry | `/usr/local/share/applications/TvMaxe.desktop` |
| Icon | `/usr/local/share/icons/hicolor/scalable/apps/TvMaxe.svg` |

If a step fails, the previously installed version is restored. To install somewhere else, set the
prefix: `PREFIX=/usr ./install.sh`.

### Uninstall

```sh
./uninstall.sh            # removes the program; your channels and settings are kept
./uninstall.sh --purge    # also deletes ~/.local/share/tvmaxe
```

If you installed with a custom prefix, pass the same one: `PREFIX=/usr ./uninstall.sh`.

## Usage

Start **TV-Maxe** from the application menu (*Multimedia*), or from a terminal:

```sh
tvmaxe                         # normal start
tvmaxe ~/lists/channels.m3u    # add a subscription from a file
tvmaxe https://…/list.m3u      # add a subscription from a URL
```

Add your channel lists in **TV-Maxe → Subscriptions…**, then double-click a channel to play it.

Right-click a channel for more:

| Menu item | What it does |
|---|---|
| Play | Plays the channel from its first source |
| Source → 1, 2, 3… | Plays a specific URL (shown for channels with several sources). If it fails, the next ones are tried, then the earlier ones; ▶ marks the source that is playing |
| Edit channel… | Changes the name, type and URLs of a channel in *My channels* (one URL per line, in order of preference) |
| Delete channel | Removes a channel from *My channels* |

Channels that come from a subscription cannot be edited, because the next list update would
overwrite them; add your own version to *My channels* instead.

### Keyboard shortcuts

| Key | Action |
|---|---|
| Double-click, Enter | Play the selected channel |
| Ctrl+B | Subscriptions |
| Ctrl+R, F5 | Update the lists |
| Ctrl+N | Add a channel |
| Ctrl+, | Preferences |
| Ctrl+G | TV guide |
| Ctrl+F | Search |
| F11, double-click on the video | Fullscreen (Esc leaves it) |
| F9 | Hide or show the channel list |
| Ctrl+Q | Quit |

### Preferences

**TV-Maxe → Preferences…** has hardware decoding, network buffer, connection timeout, video
quality (*Automatic*, *Best quality*, *Fast*), the User-Agent for lists and streams, extra EPG
sources, updating the lists at startup, logo downloads and playing the last channel on startup.
Changes apply immediately.

## Supported list formats

The format is detected from the content, not from the file extension.

| Format | Details |
|---|---|
| TV-Maxe `.db` | SQLite with `info`, `tv_channels`, `radio_channels`; icons as base64 PNG (0.x) or blob (NG); `streamurls` as JSON |
| TV-Maxe text (old) | `\|tvchannels\|\|newchannel\|Name\|-\|url\|-\|logo…` |
| M3U / M3U8 | `tvg-id`, `tvg-logo`, `group-title`, `#EXTGRP`, `url-tvg` / `x-tvg-url`, `radio="true"`, `http-referrer`, `http-user-agent`, `#EXTVLCOPT` |

In M3U files, consecutive URL lines after the same `#EXTINF` become fallback URLs of one channel.

**TV guide:** sources come from the lists (`url-tvg` in M3U, `epgurl` in TV-Maxe) or from
Preferences. Channels are matched by `tvg-id`, then by name (ignoring diacritics and HD/FHD
suffixes). Only channels present in your lists and the days from yesterday to a week ahead are
kept; the guide refreshes every 12 hours.

## Language

The interface is in **English**, or in **Romanian** on a Romanian system. The language is picked
like for any Linux program, from `LANGUAGE`, then `LC_ALL`, `LC_MESSAGES` and `LANG`. To choose it
for one run:

```sh
LANGUAGE=ro tvmaxe    # Romanian
LANGUAGE=en tvmaxe    # English
```

Translations are compiled into the binary, so there are no extra files to install. Adding a
language means adding a catalog next to
[`catalog_ro.rs`](crates/tvmaxe-core/src/i18n/catalog_ro.rs).

## Data and settings

Everything is stored in one SQLite database, `~/.local/share/tvmaxe/tvmaxe.db`:

| Table | Contents |
|---|---|
| `subscriptions` | Subscriptions: name, URL, enabled, last update, last error |
| `channels` | Local copy of the channels, so TV-Maxe starts without internet too |
| `logos` | Downloaded logos; failures are remembered and not retried on every start |
| `epg_sources`, `epg_channels`, `epg_programmes` | The downloaded TV guide |
| `settings` | Preferences (`pref.*`), volume, window size, tab and last channel |

## KDE Plasma

TV-Maxe is a GTK 4 application, not Qt/Kirigami. It runs normally on Plasma (Wayland and X11), but
it looks like a GTK application. For a better fit:

- Install the Breeze theme for GTK (`breeze-gtk` on Arch, `breeze-gtk-theme` on Debian/Ubuntu) and
  select it in *System Settings → Application Style → Configure GNOME/GTK Application Style*.
- For the KDE file dialog, install `xdg-desktop-portal-kde` and set `GDK_DEBUG=portals`. Without it,
  GTK 4 uses its own file dialog outside Flatpak.

## Development

### Build and run

```sh
cargo build --release
./target/release/tvmaxe
cargo run -p tvmaxe-core --example inspect -- list.db    # print what a list contains
```

All application code is Rust. GTK 4 (interface) and libmpv/FFmpeg (video) are system libraries.
SQLite is compiled into the binary (`rusqlite` with `bundled`).

### Tests

```sh
cargo test --workspace                   # unit tests + translation coverage, no display needed
scripts/integration-tests.sh x11         # integration tests on Xvfb
scripts/integration-tests.sh wayland     # integration tests on headless Weston
```

The integration tests run the real application with libmpv and GTK on a virtual display with
software OpenGL. They need no internet: the video is generated by mpv (`lavfi`) and the HTTP server
is local.

| Test | What it checks |
|---|---|
| [`player.rs`](crates/tvmaxe/tests/player.rs) | mpv under a non-C locale; playback **and rendering** in GtkGLArea (a capture of the video area must show the colour bars); fallback URLs; failure when every URL fails; Referer / User-Agent headers; pause and stop |
| [`ui.rs`](crates/tvmaxe/tests/ui.rs) | Main window and menu; a subscription from a local M3U; playback with a fallback URL; choosing a source; editing a channel in *My channels*; every dialog opened and closed three times; F9; closing the window during playback |
| [`translations.rs`](crates/tvmaxe/tests/translations.rs) | Every `tr("…")` has a Romanian translation, and the catalog has no unused entries |

Any GTK warning (for example *"A window is shown after it has been destroyed"*) fails the test.
Without a display, `cargo test` skips the integration tests; `TVMAXE_REQUIRE_DISPLAY=1` makes them
mandatory, and `TVMAXE_TEST_CAPTURE=/tmp/capture.png` saves the video capture.

**CI** ([`ci.yml`](.github/workflows/ci.yml), Ubuntu 24.04) runs `cargo fmt --check`,
`cargo clippy -D warnings`, `shellcheck`, `cargo test`, and the integration tests on both X11 and
Wayland. The video capture and the mpv log are kept as artifacts.

### Project layout

```text
crates/tvmaxe-core/          no UI, fully unit-tested
  src/formats/               list detection and parsers (TV-Maxe .db, TV-Maxe text, M3U), .db export
  src/epg.rs                 XMLTV parser (gzip, filtering, name matching)
  src/db.rs                  SQLite storage: subscriptions, channels, logos, guide, settings
  src/net.rs                 downloads of lists, logos and guides (worker threads)
  src/i18n.rs, src/i18n/     tr / trn / fill and the Romanian catalog
crates/tvmaxe/               GTK 4 application
  src/main.rs                the binary: calls tvmaxe::run()
  src/lib.rs                 application, menu and style
  src/window.rs, window/     main window, playback, subscriptions, logos, guide
  src/channel_list.rs        TV / Radio tab
  src/dialogs/               Subscriptions, Add channel, Preferences, TV guide, About
  src/settings.rs            preferences stored in SQLite
  src/player/mod.rs          Player facade and Backend trait
  src/player/mpv/            libmpv backend rendering into a GtkGLArea
  tests/                     integration and translation tests
data/                        .desktop file and icon
install.sh, uninstall.sh     system-wide install / uninstall
scripts/integration-tests.sh integration tests on Xvfb or headless Weston
```

### Video backend

The interface talks to `Player` ([`player/mod.rs`](crates/tvmaxe/src/player/mod.rs)), not to mpv
directly. `Player` holds the channel logic: the URL list, switching to the next URL when one fails,
state and pause. A backend implements only the simple operations of the `Backend` trait:

| Method | Purpose |
|---|---|
| `widget()` | The widget that shows the picture |
| `load(url, http)` / `stop()` | Start / stop a URL |
| `set_paused`, `is_paused`, `set_volume`, `position` | Playback controls |
| `apply_settings` | Playback preferences (hwdec, buffer, timeout, User-Agent, quality) |
| events | `Connecting`, `Playing`, `Buffering(%)`, `Ended`, `Failed(message)` |

The only backend today is `mpv` (default Cargo feature). A new one, such as libVLC 4 once it is
stable, needs a `player/<name>/` module, a Cargo feature and a branch in `create_backend()`; the
rest of the application stays the same.

## Troubleshooting

| Problem | What to do |
|---|---|
| A channel does not play | Run `TVMAXE_MPV_LOG=/tmp/mpv.log tvmaxe` and check the full mpv log. Some servers need a specific User-Agent (*Preferences*). |
| Choppy video on an old computer | Set *Preferences → Video quality* to *Fast*. On software OpenGL this happens automatically. |
| TV-Maxe is missing from the menu | Log out and back in. If `/usr/local/share` is not in `XDG_DATA_DIRS`, reinstall with `PREFIX=/usr ./install.sh`. |
| Radio plays no sound | Check that an audio output (PipeWire or PulseAudio) is running. |

## Roadmap

- [ ] Recording and scheduler
- [ ] Sleep timer
- [ ] Favourites, keyboard zapping, audio track selection
- [ ] AUR and Flatpak packages
- [ ] More interface languages

## License

TV-Maxe is free software, released under the
[GNU General Public License, version 3 or later](LICENSE) (`GPL-3.0-or-later`).

The original TV-Maxe was created by **Ovidiu Nițan**. This rewrite keeps the credit to the original
author but contains no code from the original project.
