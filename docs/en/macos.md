[← README](../../README.md) · **English** · [Deutsch](../de/macos.md)

# macOS (experimental)

From 2.5.0 on Paneldeck runs on macOS 12 (Monterey) or newer, on Apple Silicon and Intel Macs (one universal app). **The macOS version is experimental:** it is built and tested automatically by GitHub Actions on macOS, but not yet used every day on a real Mac. Please report anything that does not work as an [issue](https://github.com/zibbozz/paneldeck/issues). The *About* page in the settings says so too.

## Installing
Every release on [GitHub Releases](https://github.com/zibbozz/paneldeck/releases) has two macOS files, built by GitHub Actions from the tagged source, plus `SHA256SUMS.txt`:

| File | Content |
|---|---|
| `Paneldeck-<version>-macos-universal.dmg` | disk image: drag `Paneldeck.app` onto *Applications* |
| `paneldeck-<version>-macos-universal.zip` | `Paneldeck.app` as a ZIP (unpack, move to *Applications*) – also what the self-update downloads |

Check a download yourself before opening it: `shasum -a 256 -c --ignore-missing SHA256SUMS.txt`.

### The app is not signed – opening it the first time
Paneldeck has no Apple Developer ID (it costs money; the project stays free). The app is only *ad-hoc* signed, so Gatekeeper blocks the first start of a downloaded copy ("cannot be opened because Apple cannot check it for malicious software"). Only do the following for a file from the release page whose checksum matches:
- **macOS 12–14:** in *Applications* right-click (or Ctrl+click) `Paneldeck.app` → **Open** → **Open**. macOS remembers the decision.
- **macOS 15 (Sequoia) and newer:** double-click the app, close the message, then *System Settings → Privacy & Security*, scroll down to "Paneldeck was blocked …" → **Open Anyway** and confirm.
- **Terminal (all versions):** `xattr -d com.apple.quarantine /Applications/Paneldeck.app` removes the download mark; the app then opens normally.

Start it from *Applications*, not from the disk image: an app started from the disk image (or from a quarantined folder, "App Translocation") runs from a read-only location and cannot update itself.

## Data folder
`~/Library/Application Support/Paneldeck`, readable only by you (folder `0700`, files `0600`). **Portable mode:** a folder `paneldeck-data` next to the program inside the app is not practical on macOS; use `--data-dir` or `PANELDECK_DATA` instead. `--data-dir`, `PANELDECK_DATA` and `--profile` work as everywhere (see [Command line](command-line.md)); the program itself is `/Applications/Paneldeck.app/Contents/MacOS/paneldeck`.

## Shells
The terminals use the shells from `/etc/shells`; the default is your login shell (`$SHELL`, normally **zsh**). They start as login shells (`-l`), so `PATH` and your profile are set up like in Terminal.app and `claude` is found. zsh and fish get the shell integration (command marks, exit codes, folder tracking, exact `run_command`) without changing your `.zshrc` – see [Terminals](terminals.md).

## Desktop integration
All switches are under *Settings → General → Desktop integration*.
- **Menu bar:** instead of a tray icon Paneldeck shows an icon in the menu bar (click = show the window, menu: *Show Paneldeck* / *Quit*). *Close → tray* and *Minimize → tray* use it.
- **Start at login** (off by default): a LaunchAgent `~/Library/LaunchAgents/io.github.zibbozz.paneldeck.plist` (instances with `--profile`/`--data-dir` get their own file) that starts Paneldeck minimized at the next login. macOS shows "Background item added" and lists it under *System Settings → General → Login Items*. Turning the switch off removes the file (a file without Paneldeck's marker is never touched). It cannot be turned on while Paneldeck runs from the disk image or a translocated location – move the app to *Applications* first.
- **Notifications** (on by default) are shown through `osascript`. Because the app is not signed they appear under the name **"Script Editor"**; allow (or turn off) them in *System Settings → Notifications → Script Editor*.
- **File dialogs** (export, import, choose a folder) are the native ones of macOS.
- **Finder: "Open in Paneldeck"** is not set up by Paneldeck in 2.5.0. You can add it yourself as a Quick Action in two minutes:
  1. Open *Automator* → *New Document* → **Quick Action**.
  2. At the top: *Workflow receives current* **folders** in **Finder**.
  3. Add the action **Run Shell Script**, *Pass input:* **as arguments**, and enter:
     ```sh
     for f in "$@"; do /Applications/Paneldeck.app/Contents/MacOS/paneldeck --open "$f"; done
     ```
     (with `--claude --open` instead for "Start with Claude").
  4. Save it as "Open in Paneldeck". It appears on right-click → *Quick Actions* (or *Services*) on a folder. A running Paneldeck opens the folder as a new session.

## API keys of chat servers
API keys are never stored as plain text. Paneldeck keeps one random key in your **login Keychain** (item "Paneldeck", account "api-key-encryption") and stores the API keys encrypted with it in `settings.json`. Because the app is not signed, macOS may ask once whether Paneldeck may use this item – choose *Always Allow*. After an update the question can come again (the signature of the new version differs). If there is no usable Keychain, Paneldeck asks for a **master password** on the first access to a key (never at start), as on Linux.

## Self-update
The app updates itself: the new `paneldeck-<version>-macos-universal.zip` is downloaded from GitHub, **checked against `SHA256SUMS.txt`**, unpacked next to the app and put in place of the old `Paneldeck.app` (which stays hidden as `.Paneldeck.app.old` until the new version has started), then Paneldeck restarts. This needs write access to the folder of the app (normal for `/Applications` for an admin user); otherwise Paneldeck only announces the new version with a link. Before the swap the unpacked app is checked with `codesign --verify --deep --strict`; a damaged bundle is discarded.

Honestly: the checksum protects against **transfer errors and incomplete downloads, not against a compromised release** – there is no separate signature. The app written by the self-update has **no quarantine attribute**, so Gatekeeper does not check updates at all. The only trust is the match with `SHA256SUMS.txt` of the GitHub release.

## Agent interface (MCP)
Works as on Windows and Linux. The hub identifies the calling process itself (owner of the TCP connection through `libproc`, and its process tree). A caller that cannot be identified is **refused**. A caller belongs to a session only if it is the session's shell or one of its descendants. macOS has no "subreaper": a program that detaches from a session after the hub started (its parent chain ends at `launchd`) counts as "belongs to no session it can prove" and is restricted like on Windows. As on Windows and Linux this is no boundary against hostile programs of the same user, see [Agent interface](agent-interface.md).

Known detours (like WMI or the task scheduler on Windows): a session can have a program that is **already running** start something for it. That program is then no descendant of the session, and the hub counts it as an outside client. Examples:
- `open -a Terminal x.command` or `open x.command`: Terminal.app runs the script in a new window;
- `osascript -e 'tell application "Terminal" to do script "…"'` (likewise for iTerm2): a command typed into an already open terminal window;
- a `tmux`/`screen` server started before or outside the session: `tmux new-window …`, `tmux send-keys …`;
- `launchctl` jobs or login items.

The chain rules protect against steered agents (prompt injection), not against hostile local programs. An agent with level *Control* that can run shell commands can use these detours. Leave the level at *Off* or *Read* if that matters.

## Limits (2.5.0)
- **Experimental**, not signed, not notarized.
- Window: Paneldeck draws its own title bar with the window buttons on the right (as on Windows), not the macOS "traffic lights".
- Keyboard: app shortcuts still use **Ctrl** (as on Windows/Linux), not Cmd.
- Finder integration only by hand (see above).
- Clicking a path in the terminal never starts a program: apps (`.app`), `.command`/`.tool`/`.terminal`, AppleScripts, installers (`.pkg`), `.webloc`/`.inetloc`/`.fileloc`, `.prefPane`, shell scripts and every file with the execute bit are only shown in Finder.
- Notifications appear as "Script Editor".
- Installed monospace fonts are read from `/System/Library/Fonts`, `/Library/Fonts` and `~/Library/Fonts`.
