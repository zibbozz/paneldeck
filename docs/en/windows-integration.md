[← README](../../README.md) · **English** · [Deutsch](../de/windows-integration.md)

# Windows integration

Everything Paneldeck sets up in Windows is per user (`HKCU`, no administrator rights) and can be switched off again. The settings are under *Settings → Window*, section *Windows integration (current user only, no admin rights)*.

## Tray
- **Close → tray:** the window goes to the tray instead of quitting (quit via the tray menu).
- **Minimize → tray:** no taskbar button while minimized; clicking the tray icon brings the window back.
- `--no-tray` starts without a tray icon; both options then behave like a normal window for that start (see [Command line](command-line.md#--no-tray)).
- Further windows (Ctrl+click on a tab) stay open and keep updating while the main window is in the tray.

## Start with Windows (autostart)
Starts Paneldeck minimized in the tray after login (`"<exe>" --tray`, Run value `Paneldeck` under `HKCU\Software\Microsoft\Windows\CurrentVersion\Run`).

**Per instance:** an instance started with `--profile` or `--data-dir` gets its own Run value with its own parameters, so every instance can start with Windows independently:

| Instance | Run value | Command |
|---|---|---|
| default | `Paneldeck` | `"<exe>" --tray` |
| `--profile work` | `Paneldeck (work)` | `"<exe>" --profile work --tray` |
| `--data-dir D:\pd` | `Paneldeck (<hash>)` | `"<exe>" --data-dir "D:\pd" --tray` |
| both | `Paneldeck (work-<hash>)` | `"<exe>" --data-dir "D:\pd" --profile work --tray` |

`<hash>` is a short, stable 8-character hash of the data folder. Switching autostart off in an instance removes only its own value.

## Explorer context menu
Right-click a folder or the empty background of a folder window (also of a drive root such as `C:\`): **Open in Paneldeck** / **Start with Claude in Paneldeck**. The entries are registered for folders (`Directory\shell`) and folder backgrounds (`Directory\Background\shell`), not for the drive icons themselves (e.g. in *This PC*). The commands are `"<exe>" [instance options] --open "%1"` and `"<exe>" [instance options] --claude --open "%1"` (`%V` for the background).

*Open in Paneldeck* on a WSL folder (`\\wsl.localhost\<distro>\…`) opens a WSL session of that distribution in the matching Linux path (see [WSL start folders](terminals.md#wsl-start-folders)).

There is **only one context menu**: the instance that switches it on last is the one it starts. Another instance then shows "→ another Paneldeck" next to the switch (the command is in the tooltip); switching it on there takes it over.

## Open in Paneldeck editor
*(Since 3.0.0.)* With *Settings → Window → Explorer: "Open in Paneldeck editor"* (**off by default**, separate from the folder entries) a right-click on **any file** offers **Open in Paneldeck editor**: the file opens in a new tab of the [Editor tab](editor.md) (an open file is brought to the front). The entry is the key `HKCU\Software\Classes\*\shell\PaneldeckEdit` (label, `Icon`, `command` = `"<exe>" [instance options] --edit "%1"`, see [`--edit`](command-line.md#--edit-file)). Like the folder entries it exists only once (the instance that switches it on last), shows "→ another Paneldeck" when it points elsewhere, and is written in the saved language. Switching it off deletes the key. The uninstaller and *Settings → Data & backup → Clean up* (entry *Explorer context menu*) remove it as well, if it points to this `paneldeck.exe`.

<!-- screenshot-manual: Explorer context menu of a .txt file with "Open in Paneldeck editor" -->

## Status in the settings
For autostart and the context menu, *Settings → Window* shows whether the entry is **off**, **on** (exactly this instance's command) or points to **another Paneldeck** (another `.exe` or another instance).

If the path of `paneldeck.exe` contains a `%` character, Paneldeck cannot write these commands safely and refuses them with a message.

## Installer and uninstaller
- The installer offers a desktop shortcut, autostart and the context menu for the **default instance** only. A silent update (also the [self-update](updates.md)) keeps your choice: a deleted desktop shortcut or a switched-off context menu does not come back.
- The uninstaller removes the default entries, the autostart values `Paneldeck (…)` of profile instances that point to the installed program, the Claude Code hooks and the MCP registrations of this Paneldeck (see [Agent interface](agent-interface.md#uninstalling)). Foreign entries stay untouched. It also removes the bundled console host (`conpty.dll`, `OpenConsole.exe`) and asks whether the data folder should be deleted.

## Language
The labels of the context menu are written in the saved language of the instance that switched it on (English or German; `--lang` does not change them).
