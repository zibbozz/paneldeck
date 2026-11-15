[← README](../../README.md) · **English** · [Deutsch](../de/windows.md)

# Multiple windows and the empty Terminal Hub

## Opening a tab in a new window
**Ctrl+click** on a tab in the title bar (*Terminal Hub* or *Todo list*) does not switch the tab but opens a **new window** showing that tab. The tab tooltips say so ("Ctrl+click: open in a new window").

- A new window opens slightly offset, with the same size, its own title bar and its number in the title ("Paneldeck (2)", "Paneldeck (3)" …; with a focused session "<session> – Paneldeck (2)"). It can be moved, maximized, resized and minimized like the main window (the Windows 11 Snap Layouts menu is only available on the main window).
- All windows run in the **same process**: they use the same data folder and profile and share settings, database, tray, agent interface and backups. A further window is not a new instance (unlike a [profile](command-line.md#profiles-and-several-instances)).
- A **Terminal Hub window has its own sessions**: its own sidebar, layout, active project and tab. A new Terminal Hub window starts **empty**; sessions you create or split there appear only in that window. Moving sessions between windows is not possible in 2.1.0.
- A **To-do list window** shows the same data as the main window: a card or ticket created or changed in one window appears in the others right away.
- Each window keeps its own view: tab, scroll positions, text fields, open dialogs and accent color do not bleed into other windows; an *Undo* toast acts in its own window.

## Working across windows
- The command palette and links find sessions of every window: picking a session of window 2 in the main window brings window 2 to the front with that session. The same goes for a ticket's terminal chip.
- A session detached from window 2 returns to window 2 when its detached window is closed.
- Notices of a session (finished, approval needed) appear in the session's own window, and the taskbar button of that window flashes.
- With *Close → tray*, the main window can go to the tray while the other windows stay open and keep updating.

## Closing and restoring
- Closing a further window **always asks first** ("Close window?") when sessions are still running in it, when one of its sessions is detached or when a session has a Git worktree – even if *Confirm close* is switched off, because a whole window ends many sessions at once:
  - it says how many sessions are still running; they end when you click **Close window**;
  - **detached sessions** stay open in their own windows and move to the main window;
  - **sessions with a Git worktree** are never closed silently: each has a **Close …** button that opens the usual worktree question (keep, remove or merge the worktree). *Close window* stays disabled until they are closed.

  *Cancel* (or `Esc`) keeps the window. A window without sessions (or with only ended sessions) closes without asking.
- **Closing the main window closes all windows** (with *Close → tray* it goes to the tray instead).
- When Paneldeck quits with further windows open, the next start restores them at their position and size, with their sessions. A window whose saved position is no longer on any monitor is moved onto a visible one. Restored windows don't take the focus from the main window, and when Paneldeck starts in the tray (`--tray`, *Start with Windows*) they stay hidden until the main window is shown.
- The windows are stored in `state.json` (field `extra_windows`, and `window` per session). An older Paneldeck (2.0.0) can still read this file and shows all sessions in the main window.

## Agents
`list_sessions` reports the `window` of each session (0 = main window), `focus_session` brings the session's window to the front, and a child session created by an agent opens in the agent's window. A `screenshot` of a session in another window returns `not_visible` with an explanation. See [Agent interface](agent-interface.md).

## The empty Terminal Hub
Paneldeck **always starts without an open terminal** when there is nothing to restore (first start, or all sessions were closed). Instead, the empty Terminal Hub shows a centered hint with **New terminal** (and **New Claude session …** in Claude mode) and the shortcuts for the palette, a new session and splitting – in your language and with your own key bindings.

- Sessions of the previous run are restored as before; no extra terminal is opened.
- Closing the last terminal brings the empty hub back.
- A new Terminal Hub window (Ctrl+click) starts empty as well.
- If the active project of a window has no sessions, the hint says so.
