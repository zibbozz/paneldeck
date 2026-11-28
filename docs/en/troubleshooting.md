[← README](../../README.md) · **English** · [Deutsch](../de/troubleshooting.md)

# Troubleshooting

## Bug reports
*Settings → Data & backup → Diagnostics → Copy diagnostics* puts system information on the clipboard (version, data folder and its source, profile, active start options …) – please include it in bug reports, together with `crash.log` from the data folder if present.

## Windows SmartScreen
The releases are not code-signed yet (see the [note in the README](../../README.md#note-the-program-is-not-yet-code-signed)). On first start, SmartScreen may show "Windows protected your PC": click **"More info" → "Run anyway"**.

## Antivirus false positives
Unsigned builds of Paneldeck are now and then flagged by Microsoft Defender or other antivirus programs as malware (so far for example `Trojan:Win32/Bearfoos.A!ml` – a detection by machine learning that changes from build to build). Paneldeck contains no malware: it is open source, built from this repository, not packed, and only connects to the network for the [update check](updates.md) when you ask for it.

What you can do:
- Download only from the [GitHub Releases](https://github.com/zibbozz/paneldeck/releases) page and compare the checksum with `SHA256SUMS.txt` (see [Updates](updates.md#integrity)).
- Report the false positive to Microsoft: https://www.microsoft.com/en-us/wdsi/filesubmission ("Incorrectly detected as malware/malicious").
- If you trust the file, restore it from quarantine and add an exclusion for the Paneldeck folder in Windows Security – only for that folder.
- Or build Paneldeck yourself (see [Building](building.md)).

Each release is checked with Microsoft Defender before it is published (`tools/defender-scan.ps1`).

## Start and command line
- **"Unknown option: …"** at start: since 2.1.0, unknown options and extra arguments are an error (exit code 2). Check old shortcuts and scripts; `paneldeck.exe --help` lists all options ([Command line](command-line.md)).
- **"The data folder cannot be created or written: …"**: the folder given with `--data-dir` or `PANELDECK_DATA` (or the profile folder) cannot be created or written. Paneldeck does not fall back to another folder in this case (exit code 1).
- **An option has no effect:** `--lang`, `--backup-dir`, `--no-tray` and `--no-update-check` only apply when the start opens the instance. If Paneldeck already runs with that data folder, quit it first (see [What is handed over](command-line.md#what-is-handed-over-to-a-running-instance)). `--reset-settings` fails while it runs.
- **Wrong data:** *Settings → About → Version* shows which data folder is active and why; perhaps a profile or `PANELDECK_DATA` is in effect.

## Database
- "The database has schema version …" at start: the data folder was used by a newer Paneldeck. Update Paneldeck (or use another data folder); the database is left untouched. This also happens when you go back from 2.3 to 2.2 (schema version 9) – see [Going back to 2.2](data-and-backups.md#new-fields-in-230).
- A damaged database can be replaced from *Settings → Data & backup → Restore …* or by copying a file from `backups/` over `paneldeck.db` while Paneldeck is closed (see [Data folder and backups](data-and-backups.md)).

## Claude mode
- Status/costs not showing? Check that `claude` is installed and that hooks are enabled under *Settings → Claude → Hooks*. `--safe-mode` (*More Claude options*) turns off hooks, so Paneldeck cannot see the status of such a session.
- A start option is missing in a session started by an agent: extra folders, allowed tools, an MCP config and `--safe-mode` from your defaults never apply to agent-created sessions (see [Claude Code](claude-code.md#start-options)).

## Agent interface
- `mcp_disabled`: switch on *Read only* or *Control* in *Settings → Agents → Access*.
- `hub_not_running`: Paneldeck is not running for the data folder the bridge looked at (the message names it). With profiles, register the instance's own server name (`paneldeck-<profile>`) or pass `--profile` to `mcp` (see [Agent interface](agent-interface.md)).
- After switching expert mode, reload the agent's tool list (`/mcp` in Claude Code).

## Chats with local models
- Server not reachable, model not found, context too small, a model that does not use tools: see [Chats – Troubleshooting](chat.md#troubleshooting).

## Updates
- "Checksum mismatch – the download was discarded": the file did not match `SHA256SUMS.txt`; try again later or download by hand.
- "This version can't be installed automatically": the release has no checksum file `SHA256SUMS.txt` (2.0.0 and older) or no matching file – download it from the release page and update by hand.
- After an update, `paneldeck.exe.old` next to a portable `paneldeck.exe` is removed as soon as the new version has started (its first frame is drawn). If the new version does not start at all, quit Paneldeck, delete `paneldeck.exe` and rename `paneldeck.exe.old` back to `paneldeck.exe`.
- "The certificate revocation check could not be done …": a firewall, proxy or virus scanner blocks the server for certificate revocation lists. For safety nothing is downloaded; try again later or check the firewall/proxy – or update by hand.

## Display
- Missing special characters? Windows system fonts are used as a fallback (Segoe UI Symbol, Microsoft YaHei, Yu Gothic, Malgun Gothic …). For Powerline or Nerd Font symbols, choose such a font under *Settings → Appearance → Fonts & scaling → Terminal font*.

## Terminals
- **A session doesn't start, or a notice says the bundled console host failed:** turn off *Settings → Terminal → Behaviour → Use the bundled console host (recommended)*; new sessions then use the console host built into Windows. Paneldeck also falls back to it by itself when `conpty.dll` or `OpenConsole.exe` are missing or fail.
- **Shift+Enter runs the command in PowerShell:** switch on *Settings → Terminal → Behaviour → Shift+Enter = new line in PowerShell*.
- **Ctrl+Backspace deletes only one character in bash:** it sends `0x08` like Windows Terminal; in PowerShell it deletes the previous word.
- **"Paste multiple lines?"** appears when the program does not use bracketed paste; the shell could otherwise run every line right away. Switch it off under *Settings → Terminal → Clipboard & paste → Warn before pasting multiple lines*.
- **A link does not open:** links open with **Ctrl+click**; some links set by programs ask first, and some are never opened for safety (see [Links, clipboard and paste](terminals.md#links-clipboard-and-paste)).
- **Closing asks about programs that keep running:** these are background programs started from the session (e.g. `code .`, a browser). *Keep running* leaves them open; *End all* ends them immediately without saving (see [Closing sessions](terminals.md#closing-sessions-and-background-programs)).
