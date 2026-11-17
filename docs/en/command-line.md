[← README](../../README.md) · **English** · [Deutsch](../de/command-line.md)

# Command line

```
paneldeck.exe [options]
paneldeck.exe mcp [--data-dir <folder>] [--profile <name>] [--lang en|de]
```

`paneldeck.exe --help` prints the same information, with an example for every option. The help, the version and error messages go to the console of the calling terminal (cmd, PowerShell); without a console (e.g. Win+R or a shortcut) a message box shows the text.

Options with a value can also be written as `--option=value` (`--profile=work`). **Unknown options and extra arguments are an error** (exit code 2) – before 2.1.0 they were silently ignored.

## Overview
| Option | Purpose |
|---|---|
| `--open <folder>` | Open a new session in `<folder>` (a file opens its folder) |
| `--claude` | With `--open`: start Claude Code in the new session |
| `--tray` | Start minimized in the tray |
| `--no-tray` | No tray icon for this start |
| `--data-dir <folder>` | Use `<folder>` as the data folder |
| `--profile <name>` | Use the profile `<name>` (its own data folder) |
| `--backup-dir <folder>` | Write database backups to `<folder>` for this start |
| `--lang en\|de` | Interface language for this start |
| `--reset-settings` | Reset the settings (a backup copy is kept) |
| `--no-update-check` | No automatic update check for this start |
| `--version` | Print the version and exit |
| `--help`, `-h`, `-?` | Print the help and exit |
| `<folder>` | A single folder without an option works like `--open <folder>` |
| `mcp` | Agent interface (MCP server over stdin/stdout) |

`--hook` and `--uninstall-hooks` are used internally by the Claude Code hooks and the uninstaller.

## Options in detail

### --open \<folder\>
Opens a new terminal session in `<folder>` in the running Paneldeck (or starts it). If `<folder>` is a file, its folder is used. The Explorer context menu uses this; drive roots such as `"C:\"` work too.

    paneldeck.exe --open "C:\Dev\my-app"

### --claude
Together with `--open`: the new session starts Claude Code (needs Claude mode).

    paneldeck.exe --open "C:\Dev\my-app" --claude

The session uses your default [Claude start options](claude-code.md#start-options). Typed in a Paneldeck terminal, `--open … --claude` never applies your defaults for extra folders, allowed tools, an MCP config or `--safe-mode`, and the new session is marked with an [agent chain](agent-interface.md#security) only if that terminal has one.

### --tray
Starts minimized to the tray icon (used by *Start with Windows*). If Paneldeck already runs, it is shown instead.

    paneldeck.exe --tray

### --no-tray
Starts without a tray icon. *Close → tray* and *Minimize → tray* then behave like a normal window (close quits, minimize uses the taskbar); *Settings → Window* shows a hint.

    paneldeck.exe --no-tray

### --data-dir \<folder\>
Uses `<folder>` for settings, sessions, database and everything else. The folder is created if needed; if it cannot be created or written, Paneldeck stops with an error naming the folder (exit code 1, no silent fallback). A relative path is relative to the current directory.

    paneldeck.exe --data-dir "D:\PaneldeckData"

### --profile \<name\>
Uses `<data folder>\profiles\<name>` and runs as its own instance next to the default one. Names: 1–40 characters from `A–Z`, `a–z`, `0–9`, `_` and `-`; Windows device names (`CON`, `NUL`, `COM1` …) are not allowed. Can be combined with `--data-dir` or `PANELDECK_DATA`.

    paneldeck.exe --profile work
    paneldeck.exe --data-dir "D:\PaneldeckData" --profile customer-a

### --backup-dir \<folder\>
Writes database backups to `<folder>` for this start. *Settings → Data & backup* shows the folder with "(set by --backup-dir)" and locks the folder choice; the saved backup folder is not changed.

    paneldeck.exe --backup-dir "E:\Backups\Paneldeck"

### --lang en|de
Interface and message language for this start (also for `--help` and error messages). The saved language stays; texts that Paneldeck stores (e.g. default names) keep the saved language. If you choose a language in *Settings → General* during such a start, that choice is saved.

    paneldeck.exe --lang de

### --reset-settings
Renames `settings.json` to `settings.json.bak-<YYYYMMDD-HHMMSS>` and starts with default settings; a toast names the backup. Sessions, prompts, themes and the database stay untouched. Not possible while Paneldeck runs with the same data folder (error, exit code 1).

    paneldeck.exe --reset-settings
    paneldeck.exe --profile work --reset-settings

### --no-update-check
Suppresses the automatic update check for this start, even if it is switched on in *Settings → About*. A manual check is still possible. See [Updates](updates.md).

    paneldeck.exe --no-update-check

### --version
Prints the version number.

    paneldeck.exe --version

### --help, -h, -?
Prints the help (in the language of `--lang` or of the saved settings).

    paneldeck.exe --help > paneldeck-help.txt

### A single folder without an option
Exactly one argument without an option that is an existing folder works like `--open <folder>`; an existing file opens its folder. This is what happens when you drop a folder or a file onto `paneldeck.exe`. It can be combined with other options (e.g. `--profile`), but not with `--open` or `mcp`; a second such argument or a path that does not exist is an error (exit code 2).

    paneldeck.exe "C:\Dev\my-app"

### mcp
Runs the MCP server for AI agents over stdin/stdout and talks to the running Paneldeck. It finds the hub in this order: `--data-dir`/`--profile`, the Paneldeck terminal it was started from, then the data folder lookup below. Only `--data-dir`, `--profile`, `--lang` and `--help` are allowed after `mcp`. The agent interface must be enabled in *Settings → Agents* – see [Agent interface](agent-interface.md).

    claude mcp add paneldeck --scope user -- "C:\Program Files\Paneldeck\paneldeck.exe" mcp
    paneldeck.exe mcp --profile work

## Data folder lookup
The data folder is chosen in this order (the first match wins):
1. `--data-dir <folder>`
2. the environment variable **`PANELDECK_DATA`**
3. the portable folder `paneldeck-data` next to `paneldeck.exe` (if it can be written – this is also where the installer's per-user installation keeps its data)
4. `%APPDATA%\Paneldeck`
5. the current directory (fallback)

`--profile <name>` then uses the subfolder `profiles\<name>` of that folder. If `--data-dir` or `PANELDECK_DATA` points to a folder that cannot be used, Paneldeck stops with an error instead of falling back.
*Settings → About* and *Copy diagnostics* show the data folder, where it comes from (parameter, environment variable, portable, `%APPDATA%`), the profile and the active start options. See [Data folder and backups](data-and-backups.md).

## Profiles and several instances
- Only **one Paneldeck runs per data folder** (instance lock). A second start with the same folder hands over to the running one (and brings its window to the front).
- Profiles and other data folders run **side by side**: each has its own instance, settings, sessions and database.
- Integrations follow the instance: autostart, the Explorer context menu and the MCP registration of a profile instance carry its `--profile`/`--data-dir` (see [Windows integration](windows-integration.md) and [Agent interface](agent-interface.md#instances-with-their-own-data-folder-or-profile)).
- A further *window* (Ctrl+click on a tab) is not a new instance: it belongs to the same process and data folder (see [Multiple windows](windows.md)).

## What is handed over to a running instance
If Paneldeck already runs with the same data folder, a new start only hands over **`--open`/`--claude`** (or shows the window for `--tray` and a plain start). The options that only affect the start of an instance – **`--lang`, `--backup-dir`, `--no-tray`, `--no-update-check`** – have no effect then, and **`--reset-settings`** fails with an error. Quit Paneldeck first to use them.

## Exit codes
| Code | Meaning |
|---|---|
| `0` | ok (also for `--help` and `--version`) |
| `1` | error, e.g. unusable data folder, `--reset-settings` while running |
| `2` | invalid command line (unknown option, missing value, invalid profile name or language, extra argument) |

## Environment variables
| Variable | Meaning |
|---|---|
| `PANELDECK_DATA` | fixed data folder (step 2 of the lookup) |

Paneldeck sets further variables in its terminals (e.g. for the hooks and the MCP bridge to find their hub); they are internal and may change.
