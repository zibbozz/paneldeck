[← README](../../README.md) · **English** · [Deutsch](../de/claude-code.md)

# Claude Code

Paneldeck is built for running many [Claude Code](https://docs.anthropic.com/en/docs/claude-code) sessions side by side. The Claude functions form the **Claude mode** (*Settings → Claude*, on by default); without it Paneldeck is a plain terminal manager.

> Paneldeck is an independent project and not affiliated with Anthropic. It starts the `claude` program you installed yourself and reads its local files; it contains no code from Anthropic.

## Starting a Claude session
- Click the **spark button** (or *New Claude session …* in the sidebar or the empty Terminal Hub), pick a folder, done. The dialog also offers the [start options](#start-options).
- Right-click a folder in Explorer → *Start with Claude in Paneldeck* (see [Windows integration](windows-integration.md)), or `paneldeck.exe --open <folder> --claude` (see [Command line](command-line.md)).
- A quick action of a ticket can start a Claude session linked to that ticket (see [Todo list and tickets](todo-and-tickets.md#quick-actions-and-the-quick-menu)).

## Start options
Claude sessions can be started with options for Claude Code. You set them in three places: as **defaults** under *Settings → Claude → Defaults for new Claude sessions*, per session in the **New session** dialog (it preselects your last choice), and later in the **session settings** of an existing session (sidebar context menu → *Settings …*; a change asks *Reload now?*). An empty field means "Claude Code default".

- **Model** (an alias such as `fable`, `opus`, `sonnet`, `haiku`, or a full model name), **Effort** (`low`, `medium`, `high`, `xhigh`, `max`), **Permission-Mode** (`acceptEdits`, `auto`, `plan`, `default`, `dontAsk`) and **Agent** (the name of a Claude Code agent you defined). Names may contain only letters, digits and `. _ : -` (at most 100 characters).
- **bypassPermissions** is only possible as an explicit choice for one session, with a warning – it is never a default, never preselected next time and never possible for agents. A `bypassPermissions` default from an older settings file is dropped with a notice; old `--dangerously-*` arguments of a session are dropped as well.
- **Continue the conversation** (per session, also in the New session dialog, on by default): on *Reload* and when Paneldeck starts, Claude resumes the session's conversation; when it is off, Claude starts fresh.
- **More Claude options** (collapsible, in the dialog, the session settings and the defaults):
  - **Switches** without a value: `--chrome` / `--no-chrome`, `--ide`, `--verbose`, `--brief`, `--restricted`, `--disable-slash-commands` and `--safe-mode`. `--safe-mode` turns off hooks and MCP servers – Paneldeck then cannot detect status and permission prompts – and shows a warning.
  - **Extra folders** (`--add-dir`, one per line), **Allowed tools** and **Disallowed tools**, an **MCP config** (JSON file), **Append system prompt**, a **Fallback model**, **Max budget (USD)** and **Auto-compact** (`auto` or a number of tokens, e.g. `200k`).
  - Options that widen access (extra folders, allowed tools, an MCP config) show a warning. Paths must be local; network shares are never accessed, also not through links. Folders or files that are missing at start are left out with a notice. All values are validated and quoted for the shell.
- **For agent-created sessions** (MCP) and for `paneldeck --open --claude` typed in a Hub terminal, your defaults for extra folders, allowed tools, the MCP config and `--safe-mode` never apply; agents can only add harmless switches (see [Agent interface](agent-interface.md#claude-start-options)).
- The options are stored per session in `state.json` (`claude_opts`, `claude_continue`) and as defaults in `settings.json` (`claude_defaults`); see [Data folder](data-and-backups.md#new-fields-in-230).

## Status, notifications and the inbox
- Paneldeck installs **hooks** for Claude Code (*Settings → Claude*; generated configuration `claude-hooks*.json` in the data folder, optionally installed globally into `~/.claude/settings.json`). With them every session shows its status: **working**, **approval?** or **ready**.
- Sessions that wait for you appear in the **"Waiting for you"** inbox in the sidebar.
- **Notifications** (*Settings → Notifications*): on approval or when Claude is done while the window is in the background – toast, sound, flashing taskbar button.

## Costs
- Token and **cost estimates** per session and in total, based on the local transcripts in `~/.claude` (only an estimate; unknown models are priced like Sonnet). The prices per million tokens can be edited under *Settings → Claude*.
- The **cost overview** (sidebar) shows today, 7 days, total, the daily budget and the most expensive sessions.
- **Daily budget** and **session budget** (USD, 0 = off): a notice when the estimated cost exceeds the amount.

<p align="center"><img src="../images/kosten.png" alt="Cost overview" width="85%"></p>

## History and resume
- **Claude history** (sidebar): search previous conversations and resume them exactly.
- *Restore Claude sessions on start with --resume* (*Settings → Claude*): after a restart of Paneldeck (also after a self-update), Claude sessions continue where they were – unless *Continue the conversation* is off for a session.
- **Reload** (pane header, sidebar context menu, palette) restarts Claude Code in the session with its start options: it continues the conversation or starts a new one, as set per session (see [Terminals – Reload](terminals.md#reload-and-session-settings)).

## Claude files
- **Ctrl+Shift+K** opens an editor for `CLAUDE.md` and the Claude settings files of the session's folder (and the global ones).
- The **prompt library**, the **prompt composer** (**Ctrl+Shift+M**) and the **queue** (**Ctrl+Shift+Q**: prompts that are sent one after another when Claude is ready) help with longer work.

## Claude and the agent interface
Claude Code can also *control* Paneldeck through the [agent interface (MCP)](agent-interface.md): list sessions, read terminals, start child sessions and coordinate several Claude sessions, or work with your to-do list and tickets.

## When status or costs do not show
Check that `claude` is installed and that the hooks are enabled under *Settings → Claude* – see [Troubleshooting](troubleshooting.md).
