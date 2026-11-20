[← README](../../README.md) · **English** · [Deutsch](../de/linux.md)

# Linux

From 2.5.0 on Paneldeck runs on Linux (x86_64). Minimum: **Ubuntu 22.04, Debian 12, Fedora 39** or a distribution of the same age (glibc 2.35). X11 and Wayland both work.

## Installing
Every release on [GitHub Releases](https://github.com/zibbozz/paneldeck/releases) has four Linux files, built by GitHub Actions from the tagged source, plus `SHA256SUMS.txt`:

| File | For | Updates |
|---|---|---|
| `Paneldeck-<version>-x86_64.AppImage` | any distribution, no installation | **self-update** (see below) |
| `paneldeck_<version>_amd64.deb` | Debian, Ubuntu, Mint … | through the package (Paneldeck only announces new versions) |
| `paneldeck-<version>-1.x86_64.rpm` | Fedora, openSUSE … | through the package (announced only) |
| `paneldeck-<version>-linux-x86_64.tar.gz` | unpack anywhere | by hand (announced only) |

- **AppImage:** `chmod +x Paneldeck-*.AppImage`, then start it. Without FUSE 2 (`libfuse2`) start it with `--appimage-extract-and-run` or install the package `libfuse2`/`fuse-libs`.
- **.deb:** `sudo apt install ./paneldeck_<version>_amd64.deb` · **.rpm:** `sudo dnf install ./paneldeck-<version>-1.x86_64.rpm`. Both install `/usr/bin/paneldeck`, a menu entry and the icon.
- **Tarball:** unpack, start `paneldeck-<version>/paneldeck`. Copy `paneldeck.desktop` to `~/.local/share/applications/` (adjust `Exec=`) for a menu entry.
- Check a download yourself: `sha256sum -c --ignore-missing SHA256SUMS.txt`.

## Data folder
`~/.local/share/Paneldeck` (or `$XDG_DATA_HOME/Paneldeck`). **Portable mode:** create a folder `paneldeck-data` next to the program (tarball) or next to the `.AppImage` file – if it exists, Paneldeck uses it. Unlike on Windows it is never created automatically, so an unpacked archive does not silently become a data folder. `--data-dir`, `PANELDECK_DATA` and `--profile` work as everywhere (see [Command line](command-line.md)).

## Shells
The terminals use the shells from `/etc/shells`; the default is your login shell (`$SHELL`).

## Desktop integration
All switches are under *Settings → Window*, section *Desktop integration*. Everything is written into your home folder only (no root) and works the same for every package format.
- **Tray:** a StatusNotifierItem (KDE, Xfce, Cinnamon, MATE, Budgie …). **GNOME** needs the extension *AppIndicator and KStatusNotifierItem Support*; without a tray Paneldeck works normally, only *Close → tray* and *Minimize → tray* have no effect.
- **Start at login** (off by default): an XDG autostart entry `~/.config/autostart/paneldeck.desktop` (instances with `--profile`/`--data-dir` get their own file), started minimized in the tray.
- **File manager: "Open in Paneldeck"** (off by default): turning it on writes "Open in Paneldeck" and "Start with Claude in Paneldeck" for **Dolphin** (service menu, right-click a folder), **Nemo** (actions, right-click a folder or the background) and **Nautilus** (right-click → *Scripts*). Turning it off removes exactly these files again (files without Paneldeck's marker are never touched).
- **Notifications** (on by default) go through the desktop's notification service (D-Bus).

## API keys of chat servers
API keys are never stored as plain text:
- With a **system keyring** (Secret Service: GNOME Keyring, KWallet, KeePassXC …) Paneldeck keeps one random key in it ("Paneldeck – API key encryption") and stores the API keys encrypted with it in `settings.json`. The keyring may ask to be unlocked.
- **Without a keyring** Paneldeck asks for a **master password** – only on the first access to a key (saving a key, a chat with a server that has a key), never at start, so autostart and tray stay silent. The password is kept in memory until Paneldeck quits and is not stored anywhere. Without it stored keys cannot be read any more (enter the key again).

## Self-update (AppImage)
The AppImage updates itself like the portable Windows version: the new `.AppImage` is downloaded from GitHub, **checked against `SHA256SUMS.txt`**, put in place of the old one (which stays as `.old` until the new version has started) and Paneldeck restarts. Honestly: the checksum protects against **transfer errors and incomplete downloads, not against a compromised release** – there is no separate signature. `.deb`, `.rpm` and the tarball only announce a new version with a link; update them through your package manager or by hand.

## Agent interface (MCP)
Works as on Windows. The hub identifies the calling process itself (owner of the TCP connection from `/proc/net/tcp` and `/proc/<pid>/fd`, its process tree, and a per-session marker in the environment instead of Windows Job Objects). A caller that cannot be identified is **refused**. As on Windows this is no boundary against hostile programs of the same user, see [Agent interface](agent-interface.md).

## Limits (2.5.0)
- File dialogs (export, import, choose a folder) are not native yet: exports go to the data folder, folders are typed or picked in Paneldeck's own folder picker.
- Pasting an image from the clipboard (Ctrl+V into a terminal) uses X11 (on Wayland through XWayland).
- macOS follows separately (experimental, #150).
