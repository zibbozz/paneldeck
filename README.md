<h1 align="center"><img src="docs/images/banner.png" alt="Paneldeck" width="100%"></h1>

<p align="center"><strong>English</strong> · <a href="README.de.md">Deutsch</a></p>

A hub for many terminals – built for Claude Code, usable as a general-purpose terminal manager.
For Windows 10 (1809+) / 11 (x64): either as an installer (per user, no administrator rights) or as a portable ZIP with a single `paneldeck.exe`.
Paneldeck works **offline**: it only contacts GitHub when you check for updates yourself or switch on the automatic update check, and chats only talk to the local model servers you set up (see [Privacy](#privacy)).

> Independent open-source project – not affiliated with, endorsed by or reviewed by Anthropic. "Claude" and "Claude Code" are trademarks of Anthropic, PBC. See [Legal](#legal).

<p align="center"><img src="docs/images/app.png" alt="Paneldeck with split terminals, an approval prompt and the sidebar" width="100%"><br><sub>Several sessions in projects and folders, split panes, a "Waiting for you" inbox – all in one window.</sub></p>

## Features
- **Terminals** with full emulation (ConPTY): PowerShell 7, Windows PowerShell, CMD, Git Bash, WSL distributions – organized in projects, with freely splittable panes, several windows and a command palette. Modern keyboard protocols (kitty, win32-input-mode), links from programs (OSC 8, Ctrl+click), progress and notifications from programs, any installed monospace font (e.g. Nerd Fonts), WSL sessions that start in Linux folders and a bundled, newer console host from Microsoft.
- **Background programs:** every terminal's shell runs in its own Windows Job Object, so programs started from a session stay part of it, even when started detached. This includes long-lived apps such as `code .`, a browser or `wt`. When you close or reload a session, or quit Paneldeck, and such programs are still running, Paneldeck lists them and asks whether to keep them running or end them all. "End all" terminates them immediately, without saving. Programs inside a session cannot break away from its job (`CREATE_BREAKAWAY_FROM_JOB` fails); this is intended.
- **Process list:** dev servers and watchers (`npm run dev`, `ng serve` …) apart from your terminals – start, stop and restart them (also as a group), with their output in a pane of their own and a link to the local address ([Process list](docs/en/processes.md)).
- **Claude mode** (can be turned off): status via hooks (working / approval? / ready), notifications, token and cost estimates, history with exact resume, start options (model, effort, permission mode, agent and more), reload and settings per session.
- **Todo list and tickets:** a simple to-do list that turns into a small Jira in *expert mode* – projects, ticket types, workflows, boards, a query language, sprints and dashboards, all stored locally. Every ticket has a description, a due date with configurable markers and labels; icons from the Lucide set and an editable color palette. A [maintenance page](docs/en/todo-and-tickets.md#maintenance) finds stale, broken and duplicate tickets and unused configuration, archives tickets and takes care of the database.
- **Themes:** a [theme editor](docs/en/themes.md) with live preview, design check and named interface colours; a new theme can be generated from a background and an accent colour.
- **Clean up:** see what Paneldeck stores and delete what you no longer need – backups, histories, logs, content, settings – up to a complete reset ([Clean up](docs/en/data-and-backups.md#clean-up)).
- **Chats with local models:** talk to Ollama, LM Studio, llama.cpp or vLLM on this computer or in your network, right next to your terminals – the model can use Paneldeck's tools within limits you set per model.
- **Agent interface (MCP):** AI agents can see sessions, read terminals, run commands and work with your to-do list and tickets – off by default, with a configurable system prompt.
- **Command line and profiles:** separate data folders and profiles that run side by side, each with its own autostart and MCP entry.
- **Self-update** from GitHub Releases on request, checked against `SHA256SUMS.txt`.
- **English and German** (default: English).

<p align="center"><img src="docs/images/todo-dashboard.png" alt="A dashboard with burndown chart, sprint progress and charts" width="100%"><br><sub>Expert mode: a dashboard with burndown chart, sprint progress, tickets by status and priority and booked time.</sub></p>

<p align="center"><img src="docs/images/kosten.png" alt="Cost overview" width="85%"><br><sub>Cost overview: today, 7 days, total, daily budget and most expensive sessions.</sub></p>

## Download
Paneldeck is distributed only through **GitHub Releases**: https://github.com/zibbozz/paneldeck/releases – each release has an installer (`Paneldeck-Setup-<version>.exe`), a portable ZIP (`paneldeck-<version>-windows-x64.zip`) and, from 2.1.0 on, `SHA256SUMS.txt` with the checksums of both, all built from this repository.

**Linux** (from 2.5.0, x86_64; Ubuntu 22.04 / Debian 12 / Fedora 39 or newer): an AppImage (updates itself), a `.deb`, an `.rpm` and a tarball, built by GitHub Actions; their checksums are in the same `SHA256SUMS.txt`. Installation, data folder and desktop integration: [Linux](docs/en/linux.md).

**macOS** (from 2.5.0, **experimental**, macOS 12 or newer, Apple Silicon and Intel): a universal `Paneldeck.app` as `.dmg` and `.zip` (the app updates itself), built by GitHub Actions; checksums in the same `SHA256SUMS.txt`. The app is **not signed** – the first start needs right-click → *Open* (macOS 15: *System Settings → Privacy & Security → Open Anyway*) or `xattr -d com.apple.quarantine /Applications/Paneldeck.app`. Details: [macOS](docs/en/macos.md).

### Note: the program is not (yet) code-signed
The installer and `paneldeck.exe` are **not code-signed**. A certificate costs money, and Paneldeck is meant to stay a free hobby project. For now this means:
- On first start, **Windows SmartScreen** may show "Windows protected your PC". Click **"More info" → "Run anyway"**.
- Some antivirus programs (including Microsoft Defender) occasionally raise false alarms for new, unsigned programs with a tray icon and autostart. The source code is open, and you can build Paneldeck yourself (see [Building](docs/en/building.md)); see also [Troubleshooting](docs/en/troubleshooting.md#antivirus-false-positives).

## Getting started
1. Run the **installer** `Paneldeck-Setup-<version>.exe` – or unpack the **portable ZIP** and start `paneldeck.exe`.
2. Paneldeck starts with an empty Terminal Hub: **Ctrl+Shift+T** opens a new session, **Ctrl+Shift+P** the command palette, **Ctrl+Shift+D/E** splits a pane.
3. For Claude: click the spark button, pick a folder, done.
4. **Ctrl+2** (or the *Todo list* tab in the title bar) opens the to-do list, **Ctrl+1** goes back to the Terminal Hub. **Ctrl+click** on a tab opens it in a new window.

All shortcuts can be changed under *Settings → Shortcuts*.

## Documentation
The guides live in [`docs/en/`](docs/en/) (German: [`docs/de/`](docs/de/)) and are versioned with the code:

- [Terminals and sessions](docs/en/terminals.md) – sessions, projects, splits, palette, keyboard shortcuts
- [Multiple windows](docs/en/windows.md) – Ctrl+click on a tab, the empty Terminal Hub
- [Claude Code](docs/en/claude-code.md) – Claude mode, status, costs, history
- [Todo list and tickets](docs/en/todo-and-tickets.md) – simple and expert mode, boards, sprints, dashboards, quick actions, maintenance and archive
- [Query language](docs/en/query-language.md) – searching tickets, examples, reference
- [Process list](docs/en/processes.md) – dev servers and watchers with their output in a pane of their own
- [Chats with local models](docs/en/chat.md) – Ollama and OpenAI-compatible servers, models, tools and approvals, security
- [Themes and the theme editor](docs/en/themes.md) – editing and generating themes, design check, file format and colour names
- [Agent interface (MCP)](docs/en/agent-interface.md) – levels, tools, security, system prompt
- [Command line](docs/en/command-line.md) – every option, data folder lookup, profiles, exit codes
- [Data folder and backups](docs/en/data-and-backups.md) – files, backups, clean up, portable vs. installed
- [Windows integration](docs/en/windows-integration.md) – tray, autostart, Explorer context menu
- [Linux](docs/en/linux.md) – packages, data folder, tray, autostart, file manager, API keys, self-update of the AppImage
- [macOS](docs/en/macos.md) (experimental) – installing the unsigned app, data folder, menu bar, autostart, Keychain, self-update
- [Updates](docs/en/updates.md) – self-update, checksums, privacy
- [Troubleshooting](docs/en/troubleshooting.md) – common problems, antivirus false positives
- [Building](docs/en/building.md) – building from source, release files

The changelog is available in English ([`Changelog.md`](Changelog.md)) and German ([`Changelog.de.md`](Changelog.de.md)); the developer notes (`CLAUDE.md`, `TESTPLAN.md`) are written in German.

## Privacy
This program will not transfer any information to other networked systems unless specifically requested by the user or the person installing or operating it.

- Paneldeck sends no telemetry. It reads Claude transcripts (`~/.claude`) locally for history and cost estimates; everything stays on your computer, including your to-do list and tickets (local database `paneldeck.db`).
- **Update check:** Paneldeck only asks GitHub for the latest release when you click *Check for updates* (*Settings → About*) or after you switch on the automatic check there (off by default; at start, at most once a day). The request goes to the public GitHub API (`api.github.com`); downloads come from `github.com` and GitHub's download servers – all over HTTPS, without a login. GitHub sees your IP address and the Paneldeck version (in the `User-Agent`, e.g. `Paneldeck/2.1.0`); no other data is sent. See [Updates](docs/en/updates.md).
- The agent interface (MCP) is off by default and only reachable from your own computer (`127.0.0.1`).
- **Chats with local models** only connect to the model servers you set up yourself, and only on this computer or in private networks (see [Chats](docs/en/chat.md#privacy)).
- Links in terminals and tickets are only opened in your browser when you click them.

## Issues and contributions
Paneldeck is still a young project. Issues and contributions will be opened to everyone once the project has reached a state I am happy with and feel comfortable letting the public work on it. Until then, please be patient – thank you for your understanding.

## Legal
- **License:** MIT (`LICENSE`). The software is provided without any warranty; use is at your own risk – this applies in particular to the agent interface, which lets AI agents write into your terminals.
- **No affiliation with Anthropic:** Paneldeck is an independent project. "Claude" and "Claude Code" are trademarks of Anthropic, PBC and are mentioned here only to describe which program Paneldeck works with. Paneldeck contains no code and no logos from Anthropic.
- **Privacy:** see [Privacy](#privacy).
- **Third-party software:** licenses and notices are in `THIRD_PARTY.md` and `THIRD_PARTY_LICENSES.txt` (included with the installer and the ZIP). The embedded fonts Cascadia Mono and Noto Color Emoji are licensed under the SIL Open Font License 1.1 (`assets/fonts/LICENSE-*.txt`); the embedded Lucide icons are licensed under the ISC License (`assets/fonts/LICENSE-Lucide.txt`). The installer and the ZIP include the console host ConPTY from Microsoft (`conpty.dll`, `OpenConsole.exe`, unchanged, MIT License; `licenses/LICENSE-ConPTY.txt`).
