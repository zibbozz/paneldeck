[← README](../../README.md) · **English** · [Deutsch](../de/chat.md)

# Chats with local models

A **chat** is a session type next to terminals: you talk to a language model that runs **on this computer or in your private network** – with [Ollama](https://ollama.com), LM Studio, the llama.cpp server or vLLM. Chats live in projects and splits like terminals, and a model with tool support can use Paneldeck's [agent tools](agent-interface.md) – list sessions, read terminals, run commands – within limits you set per model.

Nothing goes to a cloud service: Paneldeck only connects to servers you set up, and only to addresses on this computer or in private networks (see [Allowed addresses and security](#allowed-addresses-and-security)).

## Setting up a server

### Ollama (recommended to start)
1. Install Ollama from https://ollama.com (or `winget install Ollama.Ollama`). It runs in the background and listens on `http://localhost:11434`.
2. Download a model, for example a small model with tool support:
   ```
   ollama pull qwen2.5:7b
   ```
   (`llama3.1:8b` works as well; larger models answer better but need more memory.)
3. In Paneldeck: *Settings → Models*. With no server set up yet, Paneldeck suggests **Ollama on this computer** (`http://localhost:11434`) – click **Add**, then **Connect**.

To use Ollama on another computer in your network, start it there with `OLLAMA_HOST=0.0.0.0` and enter its address, e.g. `http://192.168.1.20:11434`.

### LM Studio, llama.cpp server, vLLM (OpenAI-compatible)
These servers speak the OpenAI API (`/v1/models`, `/v1/chat/completions`). Add them with **+ Add server** and the type *OpenAI-compatible (LM Studio, llama.cpp, vLLM)*. The address may be given with or without `/v1`:

| Server | Typical address |
|---|---|
| LM Studio (start the server in its *Developer* tab) | `http://localhost:1234/v1` |
| llama.cpp `llama-server` (tool calling needs `--jinja`) | `http://localhost:8080` |
| vLLM on another machine | `http://192.168.1.30:8000/v1` |

OpenAI-compatible servers do not report whether a model supports tools or how large its context is – set both per model if needed (see below).

## Settings → Models
The tab lists your servers with their models.

- **+ Add server:** type (*Ollama* or *OpenAI-compatible*), name, address, optional **API key** and **Timeout** (waiting time for data, including the first token; 0 = default 300 s). An address outside the [allowed networks](#allowed-addresses-and-security) is shown in red ("Address not allowed") and cannot be saved.
- **Connect** fetches the model list. Each model shows whether it can use tools (*tools ✓*, *no tools*, or *tools ?* when the server does not say) and its context length. If the server is not running you get "Could not connect to the server … (Ollama: port 11434)".
- **Edit** / **Remove …** a server. Chats of a removed server stop answering (they show a notice); add the server again to continue them.

### API key
Some servers (e.g. a vLLM or LM Studio server started with a key) want an API key; it is sent as `Authorization: Bearer <key>`.
- The key is stored **encrypted for your Windows user** with Windows DPAPI – `settings.json` only contains the encrypted blob, never the key itself. *Show*/*Hide* in the dialog, a 🔑 next to the server once a key is stored; leave the field empty when editing to keep the key; *Remove key* deletes it.
- **Moving the data folder:** the encrypted key can only be decrypted by the same Windows user on the same computer. If you copy the data folder to another user or PC, the server entry stays, but the chat shows "The stored API key cannot be decrypted (different user or computer?) – please enter it again." Enter the key again under *Edit*.
- Over plain `http://` the key (like the whole conversation) travels unencrypted through your network – use HTTPS for servers on other computers when that matters.

### Settings per model
Click a model to change its settings. They apply to every chat with that model:

| Setting | Meaning |
|---|---|
| **Level** | **Off** – chat only, no Paneldeck tools · **Read** – read sessions, terminal content, history and the to-do list or tickets · **Control** – also send input, run commands, create and close sessions, change the to-do list or tickets. A new model starts with **Off**; see [The level question](#the-level-question). This level is **independent of the MCP level** under *Settings → Agents*, which only applies to external agents. |
| **Tools** | the tools the level allows; unchecked tools are never offered to the model. |
| **Ask before write actions** | on by default: every write tool waits for your approval in the chat. Write tools are all Control tools except `wait_for_status` and `focus_session` (they only wait or show a session). |
| **Max steps** | at most this many tool calls per answer (1–100, default 10); every call counts, including rejected ones. Then the answer stops with a *Continue* button. |
| **Tool calling** | *Auto* (detected from the server), *On* or *Off*. Set *Off* for a model that fails as soon as tools are sent. |
| **Context length** | *default* (Ollama: chosen automatically for the model's level, see [Limits](#limits); OpenAI-compatible: from the server, otherwise 8192 tokens) or a *custom* number of tokens. A custom value is always kept; if it is too small for the model's tool level, the chat shows a notice once. |

As for external agents, the tools also depend on the to-do mode: with expert mode off a model gets the to-do list tools, with expert mode on the ticket tools.

### The level question
A model you have never set a level for counts as **Off** everywhere – when sending, regenerating, retrying, continuing and for messages from external agents. The first time you send a message in a chat with such a model, the message stays in the input and the chat asks once: "May this model use Paneldeck?" with **Off**, **Read** and **Control**. Your answer is saved for the model (`level_chosen: true` in `settings.json`), and the waiting message is sent. Choosing a level under *Settings → Models* answers the question as well. While the question is open, the chat is marked as *needs you* in the sidebar.

## Starting a chat
- The speech-bubble button in the sidebar (*New chat (local model)*), *New chat* in the command palette, *New chat* in the empty Terminal Hub, or *New chat here* in the context menu of a folder.
- A dialog asks for **server** and **model** (it remembers your last choice; *Connect* refreshes the model list). **Enter** or *Start chat* opens the chat. Without a server the dialog explains what to do and offers *Open settings*.
- A chat is bound to that one model for its whole life. *Duplicate* in the chat's context menu starts an empty chat with the same model.

## Using a chat
- Type your message; **Enter** sends, **Shift+Enter** adds a new line. The answer is streamed and shown as Markdown; code blocks have a **Copy** button.
- **Stop** ends a running answer (the text so far stays). **↻** under the last answer generates it again (it replaces the old one). **✎** next to your last message puts it back into the input ("Editing the last message – sending replaces it."); **Esc** or *Cancel* aborts editing.
- While no answer has arrived yet, the chat shows "…", from 3 seconds on "Waiting for the model … N s" and from 10 seconds on also that the server is probably still loading the model (the first load can take a while). A model that only thinks shows "Thinking …".
- An error (server not reachable, model not found, an answer without text …) appears as a red box with **Retry**.
- Below an answer: the **token usage** ("Tokens: 120 in · 45 out", as far as the server reports it) and, if the model stopped at its token limit, "Answer cut off at the model's token limit."
- A model without tool support shows the notice "This model cannot use tools – chat only."
- **Context:** the model receives the [system prompt](agent-interface.md#system-prompt) (rendered for the model's level), the project name and the folder of the chat – but no direct file system access. When the history no longer fits into the context length, the oldest turns are left out (the system prompt always stays). A marker "Older messages are no longer visible to the model" shows from where on the model still sees the conversation.
- Text from prompt snippets, the prompt composer, a double-click in the file tree or a dropped file goes into the chat's input – it is **never sent automatically**.
- The context menu of a chat has *Save history to file …* (Markdown), *Duplicate* and the usual entries; *Restart* and the queue only exist for terminals.

### Tool calls and approvals
When the model calls a Paneldeck tool, the chat shows **one collapsed line per call** with the tool name and a status: a spinner while it runs, ✓ done, ✗ error, ⊘ denied. Expand it to see the parameters and the result.

- With *Ask before write actions*, a write call waits: "The model wants to use a tool with write access:" with the tool name, the **target session** (id and name) and the **full arguments** (scrollable; a note tells you when more is hidden below), plus **Allow**, **Always for this chat** and **Deny**. Meanwhile the chat is marked as *needs you* in the sidebar, like a Claude approval prompt. **Enter** allows only when the approval line has the focus – Enter in the input never approves anything. *Deny* tells the model that the call was denied.
- **Always for this chat** is only offered when the call has a numeric `session` target, and never for `create_session`. It applies to **this tool on this target session** in this chat only – the same tool on another session asks again. The rules are stored in the chat's history file (`always_allow`, see [Data folder](data-and-backups.md#chat-histories)) and can be revoked at the top of the chat under **Permanent approvals (N)** → *Revoke*.
- Every tool call counts toward the **max steps** of the model, including rejected ones. When the limit is reached, the answer stops: "Stopped after N tool calls." with **Continue**, which allows the same number of calls again. If the model keeps calling tools that cannot run (unknown, not allowed, broken arguments), the chat stops with a separate note: "The model kept calling tools that could not run – stopped."
- **Context full:** tool results are shortened so that the round fits into the model's context. If that is no longer enough, older results of the round that the model has already seen are replaced by a short placeholder (elided); if even that does not help, the chat pauses with "The tool results no longer fit into the model's context – raise the context length in Settings → Models." *Continue* then skips the calls that are still waiting (the model is told they were not run) and asks the model again. No call runs without room for its result, so the system prompt is never cut off.
- **Results:** a tool result is capped at 16 KB (less when the context is tight). For terminal output (`read_terminal`, `run_command`, `read_chat`) the **end** is kept and the start is cut, with a note that the content is untrusted. Screenshots are not passed to chat models – the model only learns that the image was left out.
- **Stop** also cancels tool calls that are still waiting or running.
- The tools run inside Paneldeck with **the chat as the caller**, under the same rules as for external agents: a chat cannot write into its own session, never uses administrator profiles or its own programs, only closes sessions it created itself, does nothing in a session with an open approval prompt, and terminal content is marked as untrusted data. Sessions the chat creates are its child sessions. Every call is listed in the call log (`mcp-log.jsonl`, marked with `how: "chat"` and the `model`).
- Calls that are not allowed (level, unchecked tool) or have broken arguments are not executed; the model gets an error back instead. If Paneldeck quits while a tool runs, the call is marked as interrupted at the next start.

### Chats in projects, splits and windows
- A chat is a row in the sidebar (chat icon, status and model) and can be moved into projects and folders, split next to terminals, detached into its own window and opened in further windows (Ctrl+click on the *Terminal Hub* tab, see [Multiple windows](windows.md)). Answers keep streaming while the window is in the background.
- Chats are restored after a restart, with their history.
- **Closing a chat deletes its history.** A chat with messages asks first (when *Confirm close* is on); closing a window or a folder with chats warns about the histories that will be deleted. Use *Save history to file …* to keep a copy.
- The history is stored in `chats/<id>.json` in the data folder (see [Data folder](data-and-backups.md#chat-histories)).

External agents can see chats too and send them messages – see [Agent interface](agent-interface.md#chats).

## Allowed addresses and security
Paneldeck only connects to **this computer and private networks**:

| Allowed | |
|---|---|
| this computer | `localhost`, `127.0.0.0/8`, `::1` |
| private networks | `10.0.0.0/8`, `172.16.0.0/12`, `192.168.0.0/16`, `fc00::/7` |
| link-local | `169.254.0.0/16`, `fe80::/10` (IPv6 link-local addresses need a zone, which addresses cannot carry here – use IPv4 or an `fc00::/7` address instead) |

`localhost` connects to `127.0.0.1` first and to `::1` only if that fails – like Ollama itself.

Everything else is rejected, for example **public addresses** (`8.8.8.8`, `2001:…`), the carrier-grade NAT range **`100.64.0.0/10`**, `0.0.0.0`, multicast and broadcast, and IPv4 addresses hidden in IPv6 forms (`::ffff:8.8.8.8`, `64:ff9b::…`).

- **Numeric shorthand forms** such as `127.1`, `0x7f.0.0.1`, `0177.0.0.1` or `2130706433` are rejected instead of guessed – write addresses as `192.168.1.10`. User names/passwords in the address, queries (`?`) and fragments (`#`) are not allowed either.
- **Host names** other than `localhost` (e.g. `gpu-box.lan`) are resolved, and **every** resolved address must be allowed – one public address among them is enough to reject the name. The check runs again **before every request**, Paneldeck connects to exactly the address it checked (pinned, so a DNS change in between cannot redirect the connection), and after connecting it checks the actual peer address once more.
- **No proxy** (the connection never goes through a system proxy), **no redirects** (a server that redirects gets the error "The server redirects …" – enter the target address), no cookies, no automatic Windows sign-in.
- **HTTPS:** TLS 1.2/1.3, the certificate is always verified (name, validity, trusted root). A revocation check is not enforced, because internal certificate authorities in a LAN often have no revocation list. A **self-signed** certificate is rejected with a hint: import the certificate or its root into Windows under *Trusted Root Certification Authorities* (current user), or use `http://` in the local network.

## Privacy
- Requests go only to the servers you set up – nothing leaves your computer or private network, and Paneldeck sends no telemetry.
- The model receives what is in the chat: your messages, the system prompt, the project name and folder, and the results of the tools it calls (e.g. terminal content). With a server on another computer, this data travels through your network to that computer.
- Histories stay in the data folder (`chats/`) until you close the chat.

## Limits
- **Timeouts:** 10 s to connect, 30 s to send a request, **300 s** waiting for data by default – also for the first token, which can take minutes on a CPU. The waiting time can be set per server (*Timeout*, 5–3600 s).
- **Sizes:** an answer may have up to 16 MB of text and 128 tool calls (each with up to 1 MB of arguments); larger answers end with "The server's answer is larger than allowed." A history file larger than 8 MB loses its oldest messages when it is saved.
- **Context:** a custom value always applies. Without one, **Ollama** gets a context sized automatically: system prompt + tool descriptions + 4096 tokens of room, divided by 0.75 (the answer reserve), rounded up to a power of two, then at least 8192, at most 32768 and at most the model's own length – today that is 16384 for every level. It is sent as `num_ctx`; more than needed would only cost graphics memory. **OpenAI-compatible** servers use the length they report, otherwise 8192. A quarter of the context is kept free for the answer (for Ollama sent as `num_predict`). If the model is too slow or runs out of graphics memory, set a smaller *custom* context length (and use level *Read*); a custom value that is too small for the level shows a notice once per chat.
- **Tool descriptions:** chat models get compact tool descriptions to save context; external MCP clients get the full ones.
- **Token estimate:** Paneldeck estimates tokens conservatively – rather too many than too few, so that the server never silently cuts off the start of the conversation – and calibrates the estimate with the token counts the server reports.
- **Tool results:** at most 16 KB each; screenshots are not passed to the model.
- **Not shown:** "thinking" output of reasoning models is not displayed separately.

## Troubleshooting
- **"Could not connect to the server … (Ollama: port 11434)":** Ollama (or the other server) is not running, or address/port are wrong. Start Ollama (tray icon) and click *Connect* again.
- **The chat only shows "Waiting for the model …" and `ollama ps` lists nothing:** the model is still loading – Ollama lists a model in `ollama ps` only once it has loaded, and the first load can take minutes (especially on the CPU). Wait, or look at the Ollama log. If an Ollama also runs in WSL, `localhost:11434` may be that one (the port is then held by `wslrelay`): `ollama ps` on Windows and in WSL ask different servers. Run only one Ollama, and keep it up to date – new models need a current version.
- **"Not found – check the address, the server type and the model name (HTTP 404)":** the model was deleted or renamed on the server, or the server type is wrong. *Settings → Models → Connect* shows the current models; for Ollama, `ollama pull <model>` downloads it again.
- **"Address not allowed":** the address is not on this computer or in a private network (see above).
- **"Unexpected answer from the server – is the server type right?":** an OpenAI-compatible server was added as Ollama or the other way round.
- **"Access denied – check the API key (HTTP 401/403)":** the key is missing or wrong. **"The stored API key cannot be decrypted …":** the data folder comes from another Windows user or computer – enter the key again.
- **The model forgets the start of the conversation / "The system prompt is larger than the model's context":** the context length is too small. Remove a too small custom *Context length* (Ollama then sizes it automatically) or raise it, or shorten your own `system-prompt.md`.
- **"The tool results no longer fit into the model's context …" / "The system prompt and tool descriptions are larger than the model's context …":** raise or remove a custom *Context length*, uncheck tools the model does not need, or ask for smaller results. **"This model's context (N tokens) is too small for its tool level …":** the custom context length is smaller than the level needs – raise it in *Settings → Models* or use level *Read*.
- **The model does not use tools:** check the notices "This model cannot use tools – chat only." and "Tools are off for this model (Settings → Models).", the model's *Level* (Off = chat only; a new model starts with Off) and the unchecked tools. Small models often call tools badly – try a larger model with tool support. If the server returns an error as soon as tools are sent (e.g. "does not support tools"), set *Tool calling* to *Off*.
- **Errors about the chat template or roles** (e.g. "roles must alternate") with llama.cpp or LM Studio: some models have strict chat templates. Paneldeck already merges two user messages in a row (after a failed answer) into one; if the server still refuses, start llama.cpp with `--jinja`, pick another model or a newer version of it.
- **Answers time out:** large models on a CPU can take long for the first token – raise the server's *Timeout*.
- **"The server's certificate is not trusted …":** see HTTPS above.
