[← README](../../README.md) · **English** · [Deutsch](../de/terminals.md)

# Terminals and sessions

The **Terminal Hub** is Paneldeck's main area: many terminal sessions in one window, organized in projects and folders, with freely splittable panes.

<p align="center"><img src="../images/app.png" alt="Paneldeck with split terminals, an approval prompt and the sidebar" width="100%"></p>

## Shells and emulation
- Terminals use full emulation (ConPTY): **PowerShell 7, Windows PowerShell, CMD, Git Bash and WSL distributions**.
- Colors, alternate screen, mouse, Unicode, color emoji, CJK and seamless box-drawing characters; **bold and italic** text, underline styles (double, curly, dotted, dashed) and underline colors. A program can ask for a **blinking cursor** (*Settings → Appearance → Blinking cursor*, on by default; only in the focused terminal).
- **Terminal font** (*Settings → Appearance → Terminal font*): Cascadia Mono (built-in, the default), Hack (built-in) or **any installed monospace font**, e.g. a Nerd Font. Paneldeck draws Powerline separators, box-drawing and block characters itself, so they line up without gaps. Missing characters fall back to the Windows system fonts.
- **Bundled console host:** the installer and the portable ZIP include a newer ConPTY from Microsoft (`conpty.dll` and `OpenConsole.exe` next to `paneldeck.exe`). It passes color queries and synchronized output through in the right order, so full-screen programs flicker less. *Settings → Appearance → Use the bundled console host (recommended)* is on by default; when it is off, or the files are missing or fail to start, sessions use the console host built into Windows (a notice appears if this happens repeatedly). The setting applies to new sessions.
- Every shell gets `TERM_PROGRAM=Paneldeck` and `TERM_PROGRAM_VERSION`. Paneldeck answers color queries (OSC 4/10/11/12), size queries (CSI 14t/16t) and XTVERSION – also for sessions in the background – and reports focus changes to programs that ask for them (DECSET 1004). `reset` restores colors that a program changed to the theme.
- **Profiles** (*Settings → Profiles*) bundle a shell, a startup command, environment variables and the appearance – e.g. "Production" in red or "Claude in WSL". Projects and folders can preset a profile for new sessions.
- **Environment:** new terminals read the environment variables fresh from Windows, so a variable set with `setx` applies at once. *Settings → General → Inherit Paneldeck's environment* (off by default) gives them exactly the environment Paneldeck was started with instead. Paneldeck's own variables and those of the profile apply in both cases; internal `PANELDECK_*` variables of a Paneldeck that started this one are never passed on.

## Keyboard
- **Kitty keyboard protocol:** programs that switch it on (e.g. Claude Code) get unambiguous keys – Shift+Enter, Ctrl+Backspace, Esc and Alt combinations arrive as `CSI u` sequences. Flags that a crashed program left behind are cleared at the next shell prompt.
- **win32-input-mode** under ConPTY: PowerShell and cmd receive Ctrl+Space, Ctrl+Enter, Ctrl+/, Ctrl+Shift+letter, Esc and the numpad keys as real key events; Ctrl+3 … Ctrl+0 reach the terminal.
- **Ctrl+Backspace** sends `0x08` like Windows Terminal (PowerShell deletes the previous word; in bash it deletes one character). Alt+digit and Alt+punctuation (e.g. Alt+. in bash) work; Alt shortcuts that Paneldeck uses itself don't send characters into the terminal.
- *Settings → Appearance → Shift+Enter = new line in PowerShell* (off by default): PSReadLine inserts a new line instead of running the command. Programs inside PowerShell that don't use the kitty keyboard protocol then receive a plain Enter.
- **Broadcast** encodes keys separately for every target terminal.

## Links, clipboard and paste
- **Links** in the terminal open only on **Ctrl+click**. Programs can also set links themselves (**OSC 8**, the visible text can differ from the target); the tooltip shows the real target and its host. For these links:
  - `http`/`https` links with a plain host name open directly; every other link – other schemes, a user name in the address (`user@host`), host names with non-ASCII characters – first asks *Open link?* with the full target.
  - Never opened, not even after asking: overlong or disguised targets (control, bidi or invisible characters, also percent-encoded), script schemes, misused Windows protocol handlers (including the Office handlers), `file:` links to anything but a local drive path, and alternate data streams.
  - Network and device paths such as `\\.\GLOBALROOT\…` or `\??\UNC\…` never count as local – neither for links nor for the working folder reported by the shell (OSC 7, OSC 9;9).
  - Executable files (also e.g. `.settingcontent-ms`, `.library-ms`, `.chm`, `.msc`, and names with trailing dots or spaces) are never started; Explorer only shows them selected in their folder.
- **Copy from programs (OSC 52):** programs can put text on the Windows clipboard – e.g. Claude Code in a WSL or SSH session; a short notice names the session. *Settings → Appearance → Allow programs to copy to the clipboard (OSC 52)* is on by default. It is write-only (programs can never read the clipboard), only the focused session in the foreground window may copy, at most 1 MB and at most once every 0.3 s, and an empty text never clears the clipboard. Note: an agent with *Control* access can make the focused session copy text, too.
- **Paste:** pasted text is filtered like in Windows Terminal (control characters including ESC and C1 are removed), so hidden Enter sequences cannot slip through. *Settings → Appearance → Warn before pasting multiple lines* (on by default) asks *Paste multiple lines?* when the program does not use bracketed paste – otherwise the shell could run every pasted line as a command right away. *Paste* is enabled after a short delay.

## Notifications, bell and progress
Under *Settings → Notifications*:
- **Notifications from programs** (OSC 9, OSC 777; on by default): a program can send a notification; it reaches you only for sessions you are not looking at, through the usual ways (toast, flashing taskbar), and at most a few per minute.
- **Bell:** a bell (BEL) in a session you are not looking at always marks the session in the sidebar; **Toast on bell** (off by default) also shows a toast or notice.
- **Show progress** (on by default): progress reported by programs (OSC 9;4 – normal, error, paused or indeterminate) appears in the sidebar and on the taskbar button.
- Notification texts and session names in toasts are cleaned (control, bidi and invisible characters removed, length limited); toasts always name Paneldeck and the session. Window titles set by programs are cleaned the same way.

## Mouse wheel
- **Plain shell:** the wheel scrolls Paneldeck's scrollback.
- **Programs with mouse support** (e.g. `htop`, `vim` with `mouse=a`, `tmux`, Midnight Commander): Paneldeck reports wheel and clicks to the program in every mouse encoding (SGR, UTF-8 and the normal encoding). The normal encoding can only carry positions up to column/row 94 – ConPTY would garble larger values – so beyond that Paneldeck sends nothing instead of a wrong position; most programs use SGR anyway.
- **Full-screen programs without mouse support** (e.g. `less`, `man`): the wheel sends arrow keys, so the text scrolls – but only if the program allows it (alternate scroll mode, DECSET 1007, on by default). The setting *Settings → Appearance → Mouse wheel in full-screen programs* chooses **Send arrow keys** (default) or **Off**.
- **Shift+wheel** always scrolls Paneldeck's scrollback, even when the program uses the mouse. Full-screen programs have no scrollback, so Shift+wheel does nothing there.
- **After a crashed full-screen program** that did not switch its modes back, the wheel at the shell prompt behaves like in a plain shell again (no arrow keys into the command line, no garbled mouse codes) until you run the next command. This needs shell integration (the shell reports its prompt); without it, the wheel keeps behaving as in the crashed program.

## Starting sessions
- **Ctrl+Shift+T** or *New session …* in the sidebar opens a session; *New Claude session …* (spark button) starts one with Claude Code (see [Claude Code](claude-code.md)). The speech-bubble button starts a **chat with a local model** instead of a terminal (see [Chats with local models](chat.md)).
- On a fresh start Paneldeck opens **no terminal**: the empty Terminal Hub shows a short hint with *New terminal* (and *New Claude session …* in Claude mode) and the shortcuts. Sessions from the previous run are restored as before. See [Multiple windows](windows.md#the-empty-terminal-hub).
- From outside: the Explorer context menu (*Open in Paneldeck* / *Start with Claude in Paneldeck*), `paneldeck.exe --open <folder>`, or a folder dropped onto `paneldeck.exe` (see [Command line](command-line.md) and [Windows integration](windows-integration.md)).

### WSL start folders
A WSL session can start in a folder **inside WSL**:
- The folder field of *New session* has a **Windows path / WSL path** switch. With *WSL path* you pick one of the installed distributions (the default first; templates and Docker Desktop are hidden) and type a Linux path – absolute or `~` – with completion and a check whether the folder exists. *Choose folder* opens in the distribution's home folder, and the choice becomes the Linux path. *Recently used WSL folders* are kept per distribution.
- Default folders of tree folders and projects whose shell is WSL can be Linux paths as well.
- *Open in Paneldeck* in Explorer on a `\\wsl.localhost\<distro>\…` folder opens a WSL session of that distribution in the matching Linux path.
- A WSL session without a start folder starts in the Linux home (`~`). A restored WSL session whose saved folder cannot be used starts in `~` with a note. Folders containing `"` or `\` are rejected.

## Organization
- **Projects** with their own layout, plus **folders**, groups, colors, tags, filters (`Filter: name or #tag`) and notes. The sidebar shows the sessions as a list or a folder tree; drag & drop moves sessions and folders.
- Folders can have a working folder and **defaults for new sessions** (profile); *Save as template …* stores a group of sessions as a **session template**.
- **Panes:** split right (**Ctrl+Shift+D**) or below (**Ctrl+Shift+E**), rearrange by drag & drop, minimize a pane (**Ctrl+Shift+W**), or detach a session into its own window (see [Panes and minimizing](#panes-and-minimizing)).
- **Several windows:** **Ctrl+click** on the *Terminal Hub* tab opens a further window with its own sessions (see [Multiple windows](windows.md)).
- **Colors** of sessions, folders, projects and profiles come from the editable color palette (*Settings → Colors*, see [Language and appearance](#language-and-appearance)).
- On restart Paneldeck restores sessions, projects and layout; the scrollback of each terminal is saved in the data folder (`scrollback/`).
- **Remember sessions** (*Settings → General*, on by default): when it is off, every start is fresh – no terminals, chats, projects or folders from the last run, and chat histories are deleted on exit. Settings, the to-do list and tickets stay. Turning it off asks first and backs up the current state as `state.json.bak-<date>` (only the newest is kept). An update restart still restores the sessions once.

## Panes and minimizing
- **Pane header buttons** (left to right): *Link ticket …* (only when ticket links are available), *Split right*, *Split below*, *Open in own window*, **Reload** (see [Reload and session settings](#reload-and-session-settings)), **Minimize pane** and *Close session* (×). A detached window has *Dock back* (×, **Ctrl+Shift+W**) and *Minimize*.
- **Minimize** takes the pane out of the split layout; its neighbour takes the space, and the terminal or chat **keeps running**. This also works for the last pane – the Terminal Hub then shows "All panes are minimized". Minimize is in the pane header, the context menus of the pane and of the session in the sidebar, the command palette (*Minimize pane*) and on **Ctrl+Shift+W**. In a detached window, *Minimize* closes the window without docking the session back.
- **Sidebar:** a **dimmed** name means the session is not in the layout right now; a small **underline marker** means it is minimized. The tooltip says what a click does.
- **Bringing a session back** – click it in the sidebar or use *Switch to:* in the palette:
  - already in the layout → it just gets the focus;
  - empty Terminal Hub → it becomes the only pane;
  - **minimized → it opens alone:** it replaces the layout of the window or project, and the panes that were visible become minimized sessions that keep running. A short notice says how many panes were minimized. The old place in the split is not remembered;
  - only switched away (not minimized) → it replaces the focused pane.
- **Ctrl+Tab** / **Ctrl+Shift+Tab** also reach minimized sessions: a minimized session is shown in the focused pane, and the pane it replaces is minimized.
- Agents choose where a session appears with `placement` (see [Agent interface](agent-interface.md#placement-and-visibility)).
- Minimized sessions stay minimized after a restart. Panes keep a **minimum size** of about 120 × 70 px, also in deep splits and while dragging a divider.

## Reload and session settings
- **Reload** (button in the pane header – also in detached windows –, *Reload …* in the sidebar context menu, *Reload session …* in the palette, optional shortcut *Reload session* without a default) restarts the session's shell, or Claude Code according to the session's options. While a program is running it asks first (*Reload session?*); if the session has background programs, see [Closing sessions](#closing-sessions-and-background-programs).
- **Session settings** (*Settings …* in the sidebar context menu, *Session settings …* in the palette) change an existing session: profile, shell, start folder, name, color, tree folder, tags, notes, Claude on/off and the Claude start options (see [Claude Code](claude-code.md#start-options)). Changes that only take effect on the next start offer *Reload now?*.

## Closing sessions and background programs
- **Close** a session with the × in the pane header, the context menus or a **middle-click** on the session in the sidebar (with the same confirmation as the close button, if *Settings → General → Confirm close* is on).
- **Close all sessions at once:** a trash button on folders in the sidebar (appears on hover; includes subfolders, minimized sessions and chats), one right of the filter field (all sessions of all projects and windows), *Close all sessions of the project …* in the project context menu, the command palette and an optional shortcut *Close all sessions* (no default). The confirmation lists what will end and enables *Close all* after 3 seconds. Git worktrees and branches are kept.
- **Background programs (Job Objects):** every terminal's shell runs in its own Windows Job Object, so programs started from a session stay part of it, even when started detached (`Start-Process`, `cmd /c start`, a double fork). This includes long-lived apps such as `code .`, a browser or `wt`. When you close or reload a session, close a window or quit Paneldeck and such programs are still running, the confirmation lists them – name, PID and window title, programs with an open window marked – and offers **Keep running** or **End all** (when reloading: *Reload, keep running* / *Reload and end all*; for a window: *Close, keep running* / *Close and end all*). **"End all" terminates them immediately, without saving** – better save and close programs with an open window yourself first. It is enabled after 3 seconds, also when quitting. Programs inside a session cannot break away from its job (`CREATE_BREAKAWAY_FROM_JOB` fails); this is intended.
- **Agent chain:** sessions that agents created are marked; their context menu has *Reset agent chain* and the tooltip names the chain (see [Agent interface](agent-interface.md#security)).

## Convenience
- **Command palette** (**Ctrl+Shift+P**): all commands, sessions and – in expert mode – tickets.
- **Search** in the scrollback (**Ctrl+Shift+F**) and across all sessions (**Ctrl+Shift+G**); jump between commands (**Ctrl+Shift+Up/Down**, needs shell integration).
- **Prompt library**, **prompt composer** (**Ctrl+Shift+M**) and a **queue** per session (**Ctrl+Shift+Q**).
- **Broadcast** (**Ctrl+Shift+B**): type into all visible panes at once.
- **File tree** (**Ctrl+Shift+O**), **changes** with diff, stage and commit (**Ctrl+Shift+H**), git info in the header.
- Drag & drop of files, images from the clipboard (stored in the data folder for 14 days), clickable links (Ctrl+click, see [Links, clipboard and paste](#links-clipboard-and-paste)).
- Tray icon, transparency, themes (JSON, see [Data folder](data-and-backups.md)), smooth animations.

## Keyboard shortcuts
All shortcuts can be changed under *Settings → Shortcuts*.

| Shortcut | Action |
|---|---|
| `Ctrl+Shift+T` | New session |
| `Ctrl+Shift+D` / `Ctrl+Shift+E` | Split pane right / below |
| `Ctrl+Shift+W` | Minimize pane (in a detached window: dock back) |
| `Ctrl+Shift+R` | Rename session |
| `Ctrl+Shift+P` | Commands (palette) |
| `Ctrl+Shift+F` | Search scrollback |
| `Ctrl+Shift+G` | Search all sessions |
| `Ctrl+Shift+B` | Toggle broadcast |
| `Ctrl+Shift+N` | Notes |
| `Ctrl+Shift+O` | File tree |
| `Ctrl+Shift+K` | CLAUDE.md / Claude files |
| `Ctrl+Shift+H` | Changes (diff) |
| `Ctrl+Shift+Q` | Session queue |
| `Ctrl+Shift+M` | Prompt composer |
| `Ctrl+Shift+Up` / `Ctrl+Shift+Down` | Jump to previous / next command |
| `Ctrl+Tab` / `Ctrl+Shift+Tab` | Next / previous session (also minimized ones) |
| `Ctrl+1` / `Ctrl+2` (also `Alt+Shift+1` / `Alt+Shift+2`) | *Terminal Hub* / *Todo list* tab |
| `Ctrl`+click on a title bar tab | Open the tab in a new window |
| (none) | *Reload session*, *Close all sessions* – assign a key under *Settings → Shortcuts* |

The shortcuts of the to-do list are listed in [Todo list and tickets](todo-and-tickets.md#keyboard-shortcuts).

## Language and appearance
- English or German: *Settings → General → Language* (default: English). `--lang en|de` switches the language for one start only (see [Command line](command-line.md#--lang-ende)).
- Themes, terminal font, font size and transparency: *Settings → Appearance*; custom themes are JSON files in `themes/` in the data folder (an example is included).
- **Color palette** (*Settings → Colors*): by default one shared palette for the Terminal Hub (session, folder, project and profile colors), the tags of the to-do list and expert mode (projects, types, statuses, dropdown values, charts) – or, with *Separate palette per area*, one per area (both are kept when you switch). Add colors (color wheel or hex), give them an optional name (shown in the tooltip, e.g. "Cool blue (#2288FF)"), drag to reorder and delete them; deleting offers *Undo* for a few seconds. *Restore defaults* re-adds deleted default colors. An area's own palette starts with that area's previous colors; *Copy from shared palette* replaces it with a copy of the shared one (can be undone). Every color picker also offers *Custom color …* for a one-off color that is not added to the palette. Existing items keep their color.
