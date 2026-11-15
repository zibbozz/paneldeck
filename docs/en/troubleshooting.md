[← README](../../README.md) · **English** · [Deutsch](../de/troubleshooting.md)

# Troubleshooting

## Bug reports
*Settings → About → Copy diagnostics* puts system information on the clipboard (version, data folder and its source, profile, active start options …) – please include it in bug reports, together with `crash.log` from the data folder if present.

## Windows SmartScreen
The releases are not code-signed yet (see the [code signing policy](../../README.md#code-signing-policy)). On first start, SmartScreen may show "Windows protected your PC": click **"More info" → "Run anyway"**.

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
- **Wrong data:** *Settings → About* shows which data folder is active and why; perhaps a profile or `PANELDECK_DATA` is in effect.

## Database
- "The database has schema version …" at start: the data folder was used by a newer Paneldeck. Update Paneldeck (or use another data folder); the database is left untouched.
- A damaged database can be replaced from *Settings → Data & backup → Restore …* or by copying a file from `backups/` over `paneldeck.db` while Paneldeck is closed (see [Data folder and backups](data-and-backups.md)).

## Claude mode
- Status/costs not showing? Check that `claude` is installed and that hooks are enabled under *Settings → Claude*.

## Agent interface
- `mcp_disabled`: switch on *Read only* or *Control* in *Settings → Agents*.
- `hub_not_running`: Paneldeck is not running for the data folder the bridge looked at (the message names it). With profiles, register the instance's own server name (`paneldeck-<profile>`) or pass `--profile` to `mcp` (see [Agent interface](agent-interface.md)).
- After switching expert mode, reload the agent's tool list (`/mcp` in Claude Code).

## Updates
- "Checksum mismatch – the download was discarded": the file did not match `SHA256SUMS.txt`; try again later or download by hand.
- "This version can't be installed automatically": the release has no checksum file `SHA256SUMS.txt` (2.0.0 and older) or no matching file – download it from the release page and update by hand.
- After an update, `paneldeck.exe.old` next to a portable `paneldeck.exe` is removed at the next start.

## Display
- Missing special characters? Windows system fonts are used as a fallback (Segoe UI Symbol, Microsoft YaHei, Yu Gothic, Malgun Gothic …).
