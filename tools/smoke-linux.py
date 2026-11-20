#!/usr/bin/env python3
"""Ende-zu-Ende-Smoke-Test unter Linux (#149): startet eine Kopie des Hubs (eigener Datenordner, Schein-HOME, Ersatz-claude
im PATH – nie das echte Claude Code, nie das echte ~/.claude) und spricht per JSON-RPC mit `paneldeck mcp`.

Verwendung:  python3 tools/smoke-linux.py <paneldeck-programm>        (braucht ein Display; CI: xvfb-run -a python3 …)

Geprüft: Protokoll, Stufen, Sitzung anlegen, run_command in bash, Zuordnung einer Brücke in der Sitzung (whoami →
process), Einschränkung eines losgelösten Nachfahren ohne Markierung (#149-Review B1: kein externer Aufrufer), Rechte von
Datenordner und ipc.json. Das Windows-Gegenstück mit vielen weiteren Prüfungen ist tools/smoke-mcp.ps1.
"""
import json
import os
import shutil
import stat
import subprocess
import sys
import tempfile
import time

EXE = os.path.abspath(sys.argv[1]) if len(sys.argv) > 1 else sys.exit("Verwendung: smoke-linux.py <paneldeck>")
WORK = tempfile.mkdtemp(prefix="pd-smoke-")
HOME = os.path.join(WORK, "home")
DATA = os.path.join(WORK, "data")
PROJ = os.path.join(WORK, "proj")
FAKEBIN = os.path.join(WORK, "fakebin")
for d in (HOME, DATA, PROJ, FAKEBIN):
    os.makedirs(d)
with open(os.path.join(FAKEBIN, "claude"), "w") as f:
    f.write("#!/bin/sh\necho \"fake-claude $*\" >> \"%s/calls.txt\"\n" % FAKEBIN)
os.chmod(os.path.join(FAKEBIN, "claude"), 0o755)

ENV = {k: v for k, v in os.environ.items() if not k.startswith("PANELDECK_") and not k.startswith("CLAUDE")}
ENV.update(HOME=HOME, XDG_CONFIG_HOME=os.path.join(HOME, ".config"), XDG_DATA_HOME=os.path.join(HOME, ".local", "share"),
           PATH=FAKEBIN + ":/usr/local/bin:/usr/bin:/bin", SHELL="/bin/bash")
ENV.pop("DBUS_SESSION_BUS_ADDRESS", None)  # no keyring/notifications of the real desktop

passed = failed = 0


def check(name, ok, detail=""):
    global passed, failed
    if ok:
        passed += 1
        print("  ok    " + name)
    else:
        failed += 1
        print("  FAIL  %s  %s" % (name, detail))


def write_settings(level):
    with open(os.path.join(DATA, "settings.json"), "w") as f:
        json.dump({"mcp_level": level, "update_auto_check": False}, f)


class Bridge:
    def __init__(self):
        self.p = subprocess.Popen([EXE, "mcp", "--data-dir", DATA], stdin=subprocess.PIPE, stdout=subprocess.PIPE,
                                  stderr=subprocess.DEVNULL, env=ENV, text=True)
        self.n = 0

    def rpc(self, method, params=None):
        self.n += 1
        m = {"jsonrpc": "2.0", "id": self.n, "method": method}
        if params is not None:
            m["params"] = params
        self.p.stdin.write(json.dumps(m) + "\n")
        self.p.stdin.flush()
        line = self.p.stdout.readline()
        return json.loads(line) if line else None

    def tool(self, name, args=None):
        r = self.rpc("tools/call", {"name": name, "arguments": args or {}})
        res = (r or {}).get("result") or {}
        text = next((c.get("text", "") for c in res.get("content", []) if c.get("type") == "text"), "")
        return bool(res.get("isError")) or r is None, text

    def close(self):
        try:
            self.p.stdin.close()
            self.p.wait(3)
        except Exception:
            self.p.kill()


hub = None


def start_hub(level):
    global hub
    stop_hub()
    write_settings(level)
    log = open(os.path.join(WORK, "hub.log"), "a")
    hub = subprocess.Popen([EXE, "--data-dir", DATA, "--no-update-check"], cwd=PROJ, env=ENV, stdout=log, stderr=log)
    ipc = os.path.join(DATA, "ipc.json")
    for _ in range(150):
        if os.path.exists(ipc):
            break
        time.sleep(0.2)
    time.sleep(1.5)
    return os.path.exists(ipc)


def stop_hub():
    global hub
    if hub and hub.poll() is None:
        hub.kill()
        hub.wait(5)
    hub = None
    try:
        os.remove(os.path.join(DATA, "ipc.json"))
    except FileNotFoundError:
        pass


def code(text):
    return text.split(":")[0].strip()


try:
    print("== Start")
    check("Hub startet (ipc.json)", start_hub("Off"), open(os.path.join(WORK, "hub.log")).read()[-2000:])
    b = Bridge()
    i = b.rpc("initialize", {"protocolVersion": "2025-06-18", "capabilities": {}, "clientInfo": {"name": "smoke", "version": "1"}})
    check("initialize", bool(i) and i["result"]["serverInfo"]["name"] == "paneldeck", i)
    b.p.stdin.write('{"jsonrpc":"2.0","method":"notifications/initialized"}\n')
    b.p.stdin.flush()
    tools = b.rpc("tools/list")
    check("tools/list", len(tools["result"]["tools"]) >= 14)
    err, text = b.tool("list_sessions")
    check("Stufe Aus: verweigert", err and code(text) == "mcp_disabled", text)
    b.close()

    print("== Rechte im Datenordner")
    mode = stat.S_IMODE(os.stat(DATA).st_mode)
    check("Datenordner 0700", mode == 0o700, oct(mode))
    mode = stat.S_IMODE(os.stat(os.path.join(DATA, "ipc.json")).st_mode)
    check("ipc.json 0600", mode == 0o600, oct(mode))

    print("== Stufe Steuern")
    check("Hub neu (Steuern)", start_hub("Control"))
    b = Bridge()
    b.rpc("initialize", {"protocolVersion": "2025-06-18", "capabilities": {}, "clientInfo": {"name": "smoke", "version": "1"}})
    err, text = b.tool("whoami")
    who = json.loads(text) if not err else {}
    check("whoami von außen: none", who.get("identified_by") == "none", text)
    err, text = b.tool("create_session", {"shell": "bash", "cwd": PROJ, "name": "smoke-bash"})
    check("create_session bash", not err, text)
    sid = json.loads(text).get("id") if not err else None
    err, text = b.tool("run_command", {"session": sid, "command": "echo smoke-$((40+2))", "timeout_secs": 20})
    out = json.loads(text).get("output", "") if not err else text
    check("run_command in bash", "smoke-42" in out, text[:400])

    # Brücke in der Sitzung: der Hub ordnet sie über den Prozessbaum zu
    req = os.path.join(WORK, "who.jsonl")
    with open(req, "w") as f:
        f.write('{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"in","version":"1"}}}\n')
        f.write('{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"whoami","arguments":{}}}\n')
    inner = os.path.join(WORK, "inner.txt")
    err, text = b.tool("run_command", {"session": sid, "command": "'%s' mcp < '%s' > '%s' 2>&1" % (EXE, req, inner), "timeout_secs": 20})
    got = open(inner).read() if os.path.exists(inner) else ""
    check("Brücke in der Sitzung: whoami = process", '\\"identified_by\\":\\"process\\"' in got or '"identified_by":"process"' in got, got[-600:])

    # #149-Review B1: losgelöster Nachfahre ohne Markierung (env -i, doppelter Fork) darf kein externer Aufrufer sein:
    # ein Chat-Start (Agent) ist ihm verboten, von außen nicht (dort fehlt nur ein Modell).
    with open(req, "w") as f:
        f.write('{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"det","version":"1"}}}\n')
        f.write('{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"create_session","arguments":{"kind":"chat"}}}\n')
    det = os.path.join(WORK, "detached.txt")
    cmd = "(env -i '%s' mcp --data-dir '%s' < '%s' > '%s' 2>&1 &) ; sleep 3" % (EXE, DATA, req, det)
    b.tool("run_command", {"session": sid, "command": cmd, "timeout_secs": 20})
    for _ in range(50):
        if os.path.exists(det) and "\"id\":2" in open(det).read():
            break
        time.sleep(0.2)
    got = open(det).read() if os.path.exists(det) else ""
    check("losgelöster Nachfahre: Chat-Start verboten (orphan)", "forbidden" in got, got[-600:])
    err, text = b.tool("create_session", {"kind": "chat"})
    check("von außen: Chat-Start nicht als 'forbidden' abgewiesen", code(text) != "forbidden", text)
    b.close()

    print("== Isolation")
    check("echtes Claude nie gestartet (nur Ersatz)", shutil.which("claude", path=ENV["PATH"]) == os.path.join(FAKEBIN, "claude"))
finally:
    stop_hub()
    shutil.rmtree(WORK, ignore_errors=True)

print("\nErgebnis: %d bestanden, %d fehlgeschlagen" % (passed, failed))
sys.exit(1 if failed else 0)
