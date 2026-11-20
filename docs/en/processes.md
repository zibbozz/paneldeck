[← README](../../README.md) · **English** · [Deutsch](../de/processes.md)

# Process list

The **process list** runs long-running programs such as dev servers and watchers – `php -S localhost:8000`, `ng serve`, `npm run dev`, `cargo watch` – apart from your interactive terminal sessions. You don't type into them: you start a process, watch its output in a pane of its own and stop it again. *(Since 2.6.0.)*

## The *Processes* section
The *Processes* section sits in the Terminals sidebar above the templates and can be collapsed. Its title shows how many entries are running (e.g. `PROCESSES 2/5`).

- **+** adds a new entry.
- Each row shows a status dot, the name and on the right a **Start** (▷) or **Stop** (□) button. A running entry with a detected local address gets **↗** (open in browser), and `! n` counts the lines of its output that look like errors.
- **Click** a row to show its output in the main area instead of the sessions; **double-click** a stopped entry to start it. Clicking a session (or **×** in the output's header) brings the sessions back.
- **Right-click** a row: *Start*, *Stop*, *Restart*, *Show output*, *Edit …*, *Duplicate* and – with a log file – *Open the log folder*.
- Entries of another project are hidden unless they are running; entries without a project appear in every project.
- Entries with a **group** are listed under the group's name, with buttons to start or stop the whole group.

## Fields
| Field | Meaning |
|---|---|
| Name | Shown in the list, in notifications and to agents. |
| Command | A command line, run through the chosen shell – so `npm`, `php`, `ng` and other programs are found via `PATH` like in a terminal. |
| Folder | Working folder (empty = the shell's default folder). A folder that doesn't exist makes the start fail. |
| Shell | PowerShell by default (PowerShell 7 if installed, else Windows PowerShell); or any detected shell – cmd, Git Bash, WSL. PowerShell starts without your profile (`-NoProfile`). |
| Project | The project the entry belongs to (or none). |
| Group, Order | Entries with the same group start and stop together: in ascending *Order* when starting (e.g. backend 1, frontend 2), in reverse when stopping. |
| Start automatically | *Never* (default), *With Paneldeck* or *When the project is opened* (also at start when it is the open project; once per run). |
| Log file | Also write the output to `process-logs/<id>-<name>.log` in the data folder (off by default). Each line gets a time stamp; at 5 MB the file is rotated (`.1` … `.3`). |
| Environment variables | Name and value. Values are stored **encrypted** (Windows DPAPI, readable only by the same Windows user on the same computer) and never shown to agents or written to logs. When editing, an empty value keeps the stored one. |

Changes to a running entry apply from its next start. An entry can only be deleted when it is not running.

## Running, stopping, states
A process runs in a console without input (colours and progress bars work, typing is disabled). The output keeps the last **10,000 lines**; *Search* (Ctrl+F), *Copy the whole output* and *Clear the output* are in the pane's header, and selected text is copied with Ctrl+C or a right-click.

**Stop** (button or Ctrl+C in the output without a selection) sends Ctrl+C; if the process hasn't ended **after 5 seconds**, Paneldeck ends the **whole process tree** (Windows: the job object the process runs in; Linux/macOS: its process group). *Restart* stops a running process this way and starts it again.

| State | Meaning |
|---|---|
| running | The process runs. |
| stopping | Ctrl+C was sent; waiting for the end (at most 5 s, then the tree is ended). |
| stopped | Stopped by you – or not started yet. |
| exited | Ended by itself with exit code 0. |
| crashed (code X) | Ended by itself with another exit code. You get a notification (like for Claude), the entry turns red and the output shows *Restart*. |
| failed to start | The command or shell was not found or the folder is missing; the message is in the output, without an exit code. |

**After a restart of Paneldeck** the list is still there and every process is stopped – only entries with *With Paneldeck* (and *When the project is opened* for the open project) start by themselves.

**Quitting Paneldeck** while processes run asks first: *Stop all* or *Cancel* (there is no *keep running* – the processes end with Paneldeck anyway). Hiding Paneldeck in the tray doesn't ask and doesn't stop anything.

## Open in browser
Paneldeck looks for addresses in the output (e.g. `http://localhost:4200/`). Only **local** addresses are offered – `localhost`, `127.0.0.1`, `0.0.0.0`, `[::1]` and this computer's own network address – and they open **only when you click** them (↗ in the list, or the address buttons in the output's header). `0.0.0.0` opens as `localhost`.

## Agents (MCP)
Agents can read the process list from level *Read* (`list_processes`, `read_process_output`; output and commands are marked as untrusted) and start, stop and restart entries or groups from level *Control* (`start_process`, `stop_process`, `restart_process`). Only you can add, change or delete entries. See [Agent interface](agent-interface.md#process-list).

A program in the process list that calls Paneldeck's agent interface itself does not count as any session.

## Storage
The entries are stored in `processes.json` in the data folder – see [Data folder](data-and-backups.md#process-list-processesjson).
