<#
.SYNOPSIS
  Ende-zu-Ende-Test der Agenten-Schnittstelle (MCP) gegen eine echte, laufende Paneldeck-Instanz.

.DESCRIPTION
  Kopiert die .exe in einen frischen Temp-Ordner (eigener Datenordner, deine Einstellungen bleiben unberührt),
  startet sie minimiert und spricht per JSON-RPC mit `paneldeck.exe mcp` – so wie es ein Agent tut.
  Geprüft werden Protokoll, die drei Stufen (Aus/Lesen/Steuern), Lese- und Steuer-Werkzeuge, run_command in
  allen vorhandenen Shells, simulierte Claude-Hooks (queue_prompt/wait_for_status) und die Sicherheitsregeln.
  Dazu die Aufgaben-Werkzeuge je Modus: einfache Todo-Liste (Expertenmodus aus) und Projekte/Tickets (Expertenmodus an).
  Für den Ticket-Teil wird die Datenbank des Test-Datenordners per Python (sqlite3) mit einem kleinen Projekt "PAN"
  (Typ Task mit Workflow, Pflichtfeld "Area", Übergangsregel "Result", Dropdown "Priority") und dem Expertenmodus präpariert.
  Mit -Visible zusätzlich: Screenshot-Werkzeug (braucht ein sichtbares Fenster).
  Es werden weder Maus noch Tastatur benutzt.

.EXAMPLE
  powershell -ExecutionPolicy Bypass -File tools\smoke-mcp.ps1
  powershell -ExecutionPolicy Bypass -File tools\smoke-mcp.ps1 -Exe dist\paneldeck-0.20.0.exe -Visible
#>
param(
  [string]$Exe = "$PSScriptRoot\..\target\release\paneldeck.exe",
  [switch]$Visible,
  [switch]$KeepFiles
)
$ErrorActionPreference = 'Stop'
$Exe = (Resolve-Path $Exe).Path
$work = Join-Path $env:TEMP ("hubsmoke-" + [guid]::NewGuid().ToString('N').Substring(0, 8))
$dir = New-Item -ItemType Directory -Force "$work\hub"; $proj = New-Item -ItemType Directory -Force "$work\proj"
Copy-Item $Exe "$dir\paneldeck.exe"
$script:prestart = $null
$script:pass = 0; $script:fail = 0; $script:hub = $null; $script:bridge = $null; $script:rpc = 0

function Check($name, [bool]$ok, $detail = '') {
  if ($ok) { $script:pass++; Write-Host "  ok    $name" -ForegroundColor Green }
  else { $script:fail++; Write-Host "  FAIL  $name  $detail" -ForegroundColor Red }
  $script:lastOk = $ok
}
function Set-Level($level) {
  Stop-Hub
  $f = "$dir\paneldeck-data\settings.json"
  New-Item -ItemType Directory -Force "$dir\paneldeck-data" | Out-Null
  $j = if (Test-Path $f) { [IO.File]::ReadAllText($f).TrimStart([char]0xFEFF) | ConvertFrom-Json } else { [pscustomobject]@{} }
  if ($j.PSObject.Properties.Name -contains 'mcp_level') { $j.mcp_level = $level } else { $j | Add-Member mcp_level $level }
  [IO.File]::WriteAllText($f, ($j | ConvertTo-Json -Depth 10), (New-Object Text.UTF8Encoding $false))
  if ($script:prestart) { & $script:prestart }
  $style = if ($Visible) { 'Normal' } else { 'Minimized' }
  $script:hub = Start-Process "$dir\paneldeck.exe" -PassThru -WindowStyle $style
  $t0 = Get-Date
  while (-not (Test-Path "$dir\paneldeck-data\ipc.json") -and ((Get-Date) - $t0).TotalSeconds -lt 20) { Start-Sleep -Milliseconds 200 }
  Start-Sleep 2
  Start-Bridge
}
function Stop-Hub {
  if ($script:bridge -and -not $script:bridge.HasExited) { try { $script:bridge.StandardInput.Close(); [void]$script:bridge.WaitForExit(2000) } catch {} ; if (-not $script:bridge.HasExited) { $script:bridge.Kill() } }
  if ($script:hub -and -not $script:hub.HasExited) { $script:hub.Kill(); Start-Sleep -Milliseconds 800 }
  Remove-Item -LiteralPath "$dir\paneldeck-data\ipc.json" -ErrorAction SilentlyContinue
}
function Start-Bridge {
  $psi = New-Object System.Diagnostics.ProcessStartInfo
  $psi.FileName = "$dir\paneldeck.exe"; $psi.Arguments = 'mcp'
  $psi.RedirectStandardInput = $true; $psi.RedirectStandardOutput = $true; $psi.RedirectStandardError = $true
  $psi.UseShellExecute = $false; $psi.CreateNoWindow = $true; $psi.StandardOutputEncoding = [Text.Encoding]::UTF8
  $script:bridge = [System.Diagnostics.Process]::Start($psi); $script:rpc = 0
}
function Send-Raw([string]$line) {
  $b = (New-Object Text.UTF8Encoding $false).GetBytes($line + "`n")
  $script:bridge.StandardInput.BaseStream.Write($b, 0, $b.Length); $script:bridge.StandardInput.BaseStream.Flush()
}
function Read-Line($ms = 20000) { $t = $script:bridge.StandardOutput.ReadLineAsync(); if ($t.Wait($ms)) { $t.Result | ConvertFrom-Json } else { $null } }
function Rpc($method, $params = $null, $ms = 20000) {
  $script:rpc++; $m = @{ jsonrpc = '2.0'; id = $script:rpc; method = $method }; if ($null -ne $params) { $m.params = $params }
  Send-Raw ($m | ConvertTo-Json -Depth 20 -Compress); Read-Line $ms
}
function Tool($name, $arguments = @{}, $ms = 30000) {
  $r = Rpc 'tools/call' @{ name = $name; arguments = $arguments } $ms
  if ($null -ne $r -and $r.PSObject.Properties.Name -contains 'result') {
    $c = $r.result.content | Where-Object type -eq 'text' | Select-Object -First 1
    [pscustomobject]@{ err = [bool]$r.result.isError; text = $c.text; data = $r.result.structuredContent; raw = $r.result }
  } else { [pscustomobject]@{ err = $true; text = 'KEINE ANTWORT'; data = $null; raw = $r } }
}
function Code($r) { if ($r.err) { ($r.text -split ':')[0] } else { '' } }

try {
  Write-Host "== Protokoll" -ForegroundColor Cyan
  Set-Level 'Off'
  $i = Rpc 'initialize' @{ protocolVersion = '2025-06-18'; capabilities = @{}; clientInfo = @{ name = 'smoke'; version = '1' } }
  Check 'initialize: Version und Name' ($i.result.protocolVersion -eq '2025-06-18' -and $i.result.serverInfo.name -eq 'paneldeck')
  Send-Raw '{"jsonrpc":"2.0","method":"notifications/initialized"}'
  Check 'ping' ((Rpc 'ping').result -ne $null)
  Check 'unbekannte Methode -> -32601' ((Rpc 'gibt/es/nicht').error.code -eq -32601)
  Send-Raw '{kaputt'; Check 'Parse-Fehler -> -32700' ((Read-Line).error.code -eq -32700)
  $l = Rpc 'tools/list'; Check 'tools/list (Aus): volle Liste' ($l.result.tools.Count -ge 14)
  Check 'Stufe Aus: Aufruf verweigert' ((Code (Tool 'list_sessions')) -eq 'mcp_disabled')

  Write-Host "== Stufe: Nur lesen" -ForegroundColor Cyan
  Set-Level 'Read'
  [void](Rpc 'initialize' @{ protocolVersion = '2025-06-18'; capabilities = @{}; clientInfo = @{ name = 'smoke'; version = '1' } })
  $l = Rpc 'tools/list'; $names = @($l.result.tools | ForEach-Object name)
  Check 'tools/list nur Lese-Werkzeuge' ($names -contains 'list_sessions' -and $names -contains 'read_terminal' -and $names -notcontains 'send_text')
  $ls = Tool 'list_sessions'; Check 'list_sessions' (-not $ls.err -and $ls.data.sessions.Count -ge 1)
  $sid = $ls.data.sessions[0].id
  $rt = Tool 'read_terminal' @{ session = $sid; lines = 5 }; Check 'read_terminal' (-not $rt.err -and $null -ne $rt.data.text)
  Check 'get_usage' (-not (Tool 'get_usage').err)
  Check 'search_history' (-not (Tool 'search_history' @{ query = 'smoke-test-gibt-es-nicht-xyz' }).err)
  Check 'send_text verboten' ((Code (Tool 'send_text' @{ session = $sid; text = 'x' })) -eq 'forbidden')
  Check 'run_command verboten' ((Code (Tool 'run_command' @{ session = $sid; command = 'dir' })) -eq 'forbidden')
  Check 'create_session verboten' ((Code (Tool 'create_session' @{})) -eq 'forbidden')

  Write-Host "== Stufe: Steuern" -ForegroundColor Cyan
  Set-Level 'Control'
  [void](Rpc 'initialize' @{ protocolVersion = '2025-06-18'; capabilities = @{}; clientInfo = @{ name = 'smoke'; version = '1' } })
  Check 'tools/list vollstaendig' ((Rpc 'tools/list').result.tools.Count -ge 14)
  $shells = @('powershell', 'cmd')
  if (Get-Command bash -ErrorAction SilentlyContinue) { if (Test-Path "$env:ProgramFiles\Git\bin\bash.exe") { $shells += 'gitbash' } }
  foreach ($sh in $shells) {
    $c = Tool 'create_session' @{ shell = $sh; cwd = "$proj"; name = "smoke-$sh" }
    Check "create_session $sh" (-not $c.err) $c.text
    if ($c.err) { continue }
    $id = $c.data.id; Start-Sleep 4
    if ($sh -eq 'cmd') {
      $r = Tool 'run_command' @{ session = $id; command = 'echo hallo-cmd'; timeout_secs = 25 }
      Check "run_command $sh (Ausgabe)" ($r.data.finished -and $r.data.output.Trim() -eq 'hallo-cmd') $r.text
    } elseif ($sh -eq 'gitbash') {
      $r = Tool 'run_command' @{ session = $id; command = 'echo hallo-bash'; timeout_secs = 25 }
      Check "run_command $sh (Ausgabe)" ($r.data.finished -and $r.data.output.Trim() -eq 'hallo-bash') $r.text
      $r = Tool 'run_command' @{ session = $id; command = 'false'; timeout_secs = 25 }
      Check "run_command $sh (Exit 1, leere Ausgabe)" ($r.data.exit_code -eq 1 -and $r.data.output -eq '') $r.text
      $r = Tool 'run_command' @{ session = $id; command = 'printf "x\ny\n"'; timeout_secs = 25 }
      Check "run_command $sh (zweiter Befehl sauber)" ($r.data.output -eq "x`ny") $r.text
    } else {
      $r = Tool 'run_command' @{ session = $id; command = 'echo hallo-ps'; timeout_secs = 25 }
      Check "run_command $sh (Ausgabe)" ($r.data.finished -and $r.data.output.Trim() -eq 'hallo-ps') $r.text
      $r = Tool 'run_command' @{ session = $id; command = 'cmd /c exit 3'; timeout_secs = 25 }
      Check "run_command $sh (Exit-Code 3)" ($r.data.exit_code -eq 3) $r.text
      # Exit-Code-Genauigkeit: Cmdlet-Fehler nach einem nativen Programm darf nicht dessen alten Wert melden
      $r = Tool 'run_command' @{ session = $id; command = 'Get-Item gibt-es-nicht-xyz'; timeout_secs = 25 }
      Check "run_command $sh (Cmdlet-Fehler = 1, nicht der alte Exit-Code 3)" ($r.data.exit_code -eq 1) "exit=$($r.data.exit_code)"
      $r = Tool 'run_command' @{ session = $id; command = 'cmd /c exit 3'; timeout_secs = 25 }
      Check "run_command $sh (gleicher nativer Exit-Code erneut = 3)" ($r.data.exit_code -eq 3) "exit=$($r.data.exit_code)"
      # langer, umbrochener Befehl: Ausgabe exakt, Qualitaet exact
      $long = 'Write-Output hallo-lang # ' + ('z' * 220)
      $r = Tool 'run_command' @{ session = $id; command = $long; timeout_secs = 25 }
      Check "run_command $sh (langer Befehl: Ausgabe exakt)" ($r.data.output -eq 'hallo-lang' -and $r.data.quality -eq 'exact') "out='$($r.data.output)' q=$($r.data.quality)"
      $r = Tool 'run_command' @{ session = $id; command = 'Start-Sleep 4'; timeout_secs = 1 }
      Check "run_command $sh (Zeitlimit)" ($r.data.timed_out -eq $true -and -not $r.data.finished) $r.text
      Start-Sleep 5
      Check 'send_text' (-not (Tool 'send_text' @{ session = $id; text = 'echo per-send_text'; submit = $true }).err); Start-Sleep 1
      Check 'send_keys' (-not (Tool 'send_keys' @{ session = $id; keys = @('ctrl+l') }).err)
      Check 'send_keys ungueltige Taste' ((Code (Tool 'send_keys' @{ session = $id; keys = @('ctrl+alt+entf') })) -eq 'bad_args')
      Check 'rename_session' ((Tool 'rename_session' @{ session = $id; name = 'Agent' }).data.name -eq 'Agent')
      Check 'wait_for_status: keine Claude-Sitzung' ((Code (Tool 'wait_for_status' @{ session = $id; status = @('ready'); timeout_secs = 1 })) -eq 'not_claude')
      Check 'queue_prompt: keine Claude-Sitzung' ((Code (Tool 'queue_prompt' @{ session = $id; prompt = 'x' })) -eq 'not_claude')
    }
    Check "close_session $sh (selbst angelegt)" (-not (Tool 'close_session' @{ session = $id }).err)
  }
  Check 'unbekannte Sitzung' ((Code (Tool 'send_text' @{ session = 99999; text = 'x' })) -eq 'not_found')
  Check 'Fehlertexte sind standardmaessig englisch (keine Umlaute)' ((Tool 'send_text' @{ session = 99999; text = 'x' }).text -notmatch '[äöüÄÖÜß]')
  Check 'fremde Sitzung nicht schliessbar' ((Code (Tool 'close_session' @{ session = $sid })) -eq 'forbidden')
  Check 'unbekannte Shell' ((Code (Tool 'create_session' @{ shell = 'gibtsnicht' })) -eq 'bad_args')
  Check 'ungueltiger Ordner' ((Code (Tool 'create_session' @{ cwd = "$work\gibtsnicht" })) -eq 'bad_args')

  Write-Host "== Selbst-Identifikation und Kinder-Sitzungen" -ForegroundColor Cyan
  $w0 = Tool 'whoami'
  Check 'whoami von aussen: nicht in einem Hub-Terminal' ($null -eq $w0.data.session -and $w0.data.identified_by -eq 'none')
  $ac = Tool 'create_session' @{ shell = 'powershell'; cwd = "$proj"; name = 'Eltern-Agent' }; $A = $ac.data.id; Start-Sleep 4
  # Ein Agent in der Sitzung $A: startet die Bruecke als Kindprozess der Shell (Prozessbaum), ohne Umgebungsvariable.
  function InA($tool, $targs) {
    $req = @{ jsonrpc = '2.0'; id = 1; method = 'tools/call'; params = @{ name = $tool; arguments = $targs } } | ConvertTo-Json -Depth 8 -Compress
    $cmd = "`$env:PANELDECK_SESSION=`$null; '" + $req.Replace("'", "''") + "' | & '$dir\paneldeck.exe' mcp | Out-String"
    $r = Tool 'run_command' @{ session = $A; command = $cmd; timeout_secs = 40 }
    try { ($r.data.output | ConvertFrom-Json).result } catch { $null }
  }
  $me = InA 'whoami' @{}
  Check 'whoami in Hub-Sitzung (Prozessbaum, ohne Umgebungsvariable)' ($me.structuredContent.session -eq $A -and $me.structuredContent.identified_by -eq 'process') ($me | ConvertTo-Json -Compress -Depth 5)
  $kid = InA 'create_session' @{ shell = 'powershell'; cwd = "$proj"; name = 'Kind' }; $K = $kid.structuredContent.id
  Check 'Kind-Sitzung unter dem Ordner des Eltern-Agenten angelegt' ($K -gt 0 -and $kid.structuredContent.group -like '*Eltern-Agent*' -and $kid.structuredContent.parent -eq $A) ($kid | ConvertTo-Json -Compress -Depth 5)
  Start-Sleep 4
  $kl = ((Tool 'list_sessions').data.sessions | Where-Object { $_.id -eq $K })
  Check 'list_sessions: parent und group des Kindes' ($kl.parent -eq $A -and $kl.group -like '*Eltern-Agent*')
  Check 'whoami listet die Kinder' ((InA 'whoami' @{}).structuredContent.children -contains $K)
  Check 'fremder Agent darf das Kind nicht schliessen' ((Code (Tool 'close_session' @{ session = $K })) -eq 'forbidden')
  $rc = InA 'run_command' @{ session = $K; command = 'echo kind-ok'; timeout_secs = 25 }
  Check 'Eltern-Agent steuert sein Kind (run_command)' ($rc.structuredContent.output.Trim() -eq 'kind-ok') ($rc | ConvertTo-Json -Compress -Depth 5)
  $self = InA 'send_text' @{ session = $A; text = 'x' }
  Check 'Eltern-Agent darf nicht in die eigene Sitzung schreiben' ($self.isError -eq $true -and $self.content[0].text -like 'forbidden*')
  $cl = InA 'close_session' @{ session = $K }
  Check 'Eltern-Agent schliesst sein Kind' ($cl.isError -ne $true)
  [void](Tool 'close_session' @{ session = $A })

  Write-Host "== Claude-Sitzung (simulierte Hooks)" -ForegroundColor Cyan
  $tok = Tool 'create_session' @{ shell = 'powershell'; cwd = "$proj"; name = 'smoke-token' }; $psid = $tok.data.id; Start-Sleep 4
  $o = (Tool 'run_command' @{ session = $psid; command = '"$env:PANELDECK_PORT|$env:PANELDECK_TOKEN"'; timeout_secs = 25 }).data.output.Trim()
  $port, $hookTok = $o.Split('|')
  Check 'Hook-Zugang aus der Sitzung gelesen' ($port -match '^\d+$' -and $hookTok.Length -gt 20)
  $cs = Tool 'create_session' @{ claude = $true; cwd = "$proj"; name = 'smoke-claude' }
  Check 'create_session claude' (-not $cs.err) $cs.text
  if ($script:lastOk) {
    $cid = $cs.data.id; Start-Sleep 2
    function Hook($evt, $extra = @{}) {
      $body = @{ token = $hookTok; session = [uint64]$cid; payload = (@{ hook_event_name = $evt; session_id = 'smoke1' } + $extra) } | ConvertTo-Json -Compress -Depth 5
      $cl = New-Object Net.Sockets.TcpClient('127.0.0.1', [int]$port); $st = $cl.GetStream(); $by = [Text.Encoding]::UTF8.GetBytes($body)
      $st.Write($by, 0, $by.Length); $cl.Client.Shutdown('Send'); Start-Sleep -Milliseconds 300; $cl.Close()
    }
    $w = Tool 'wait_for_status' @{ session = $cid; status = @('ready'); timeout_secs = 2 }
    Check 'wait_for_status ohne Hooks: Zeitlimit' ($w.data.timed_out -eq $true)
    Hook 'SessionStart'
    Check 'wait_for_status nach SessionStart' ((Tool 'wait_for_status' @{ session = $cid; status = @('ready'); timeout_secs = 5 }).data.matched -eq $true)
    $q = Tool 'queue_prompt' @{ session = $cid; prompt = 'hallo' }
    Check 'queue_prompt sendet sofort' ($q.data.sent_immediately -eq $true)
    Send-Raw (@{ jsonrpc = '2.0'; id = 900; method = 'tools/call'; params = @{ name = 'wait_for_status'; arguments = @{ session = $cid; status = @('ready'); timeout_secs = 20 } } } | ConvertTo-Json -Depth 8 -Compress)
    Start-Sleep 2; Hook 'Stop'
    $wr = (Read-Line 15000).result.structuredContent
    Check 'asynchrones wait_for_status endet mit Stop-Hook' ($wr.matched -eq $true -and $wr.waited_ms -ge 1500)
    Hook 'Notification' @{ notification_type = 'permission_prompt'; message = 'darf ich?' }; Start-Sleep 0.5
    Check 'Freigabe-Frage: send_text verweigert' ((Code (Tool 'send_text' @{ session = $cid; text = 'ja' })) -eq 'permission_pending')
    [void](Tool 'close_session' @{ session = $cid })
  }
  [void](Tool 'close_session' @{ session = $psid })

  if ($Visible) {
    Write-Host "== Screenshot (Fenster sichtbar)" -ForegroundColor Cyan
    Send-Raw (@{ jsonrpc = '2.0'; id = 950; method = 'tools/call'; params = @{ name = 'screenshot'; arguments = @{} } } | ConvertTo-Json -Depth 5 -Compress)
    $s1 = Read-Line 20000; $img = $s1.result.content | Where-Object type -eq 'image' | Select-Object -First 1
    Check 'screenshot (Fenster)' ($null -ne $img -and $img.mimeType -eq 'image/png' -and $s1.result.structuredContent.width -gt 100)
    $any = (Tool 'list_sessions').data.sessions | Where-Object { $_.status -ne 'exited' } | Select-Object -First 1
    [void](Tool 'focus_session' @{ session = $any.id }); Start-Sleep 1
    Send-Raw (@{ jsonrpc = '2.0'; id = 951; method = 'tools/call'; params = @{ name = 'screenshot'; arguments = @{ session = $any.id } } } | ConvertTo-Json -Depth 5 -Compress)
    $s2 = Read-Line 20000; $img2 = $s2.result.content | Where-Object type -eq 'image' | Select-Object -First 1
    Check 'screenshot (Sitzungs-Ausschnitt kleiner als Fenster)' ($null -ne $img2 -and $s2.result.structuredContent.height -lt $s1.result.structuredContent.height + 1)
  }

  Write-Host "== Aufgaben-Werkzeuge: einfache Todo-Liste (Expertenmodus aus)" -ForegroundColor Cyan
  Set-Level 'Control'
  [void](Rpc 'initialize' @{ protocolVersion = '2025-06-18'; capabilities = @{}; clientInfo = @{ name = 'smoke'; version = '1' } })
  $names = @((Rpc 'tools/list').result.tools | ForEach-Object name)
  Check 'einfacher Modus: tools/list hat die Todo-Werkzeuge' ($names -contains 'list_todos' -and $names -contains 'add_todo' -and $names -contains 'set_todo_done')
  Check 'einfacher Modus: tools/list hat keine Ticket-Werkzeuge' ($names -notcontains 'create_ticket' -and $names -notcontains 'get_ticket' -and $names -notcontains 'link_terminal')
  Check 'kein Werkzeug zum Loeschen von Tickets/Karten' (-not ($names | Where-Object { $_ -match 'delete|remove' }))
  $t1 = Tool 'add_todo' @{ title = 'Milch kaufen'; text = '2 Liter'; tags = @('Haushalt', 'haushalt', 'Einkauf') }
  Check 'add_todo legt eine Karte an (Tags automatisch, ohne Doppelte)' (-not $t1.err -and $t1.data.todo.key -eq 'TODO-1' -and $t1.data.todo.tags.Count -eq 2) $t1.text
  Check 'add_todo ohne Titel und Text: bad_args' ((Code (Tool 'add_todo' @{})) -eq 'bad_args')
  Check 'add_todo: unbekannter Parameter: bad_args' ((Code (Tool 'add_todo' @{ title = 'x'; bogus = 1 })) -eq 'bad_args')
  Check 'add_todo: zu langer Titel: too_large' ((Code (Tool 'add_todo' @{ title = ('x' * 300) })) -eq 'too_large')
  $lt = Tool 'list_todos'
  Check 'list_todos: eine offene Karte, nicht nur lesbar' ($lt.data.total -eq 1 -and $lt.data.read_only -eq $false -and $lt.data.todos[0].title -eq 'Milch kaufen')
  Check 'list_todos: Filter nach Tag' ((Tool 'list_todos' @{ tags = @('einkauf') }).data.total -eq 1 -and (Tool 'list_todos' @{ tags = @('gibts-nicht') }).data.total -eq 0)
  $sd = Tool 'set_todo_done' @{ id = 'TODO-1' }
  Check 'set_todo_done hakt ab' (-not $sd.err -and $sd.data.todo.done -eq $true -and $sd.data.changed -eq $true)
  Check 'set_todo_done erneut: keine Aenderung' ((Tool 'set_todo_done' @{ id = 1 }).data.changed -eq $false)
  Check 'list_todos: erledigte standardmaessig ausgeblendet' ((Tool 'list_todos').data.total -eq 0 -and (Tool 'list_todos' @{ state = 'all' }).data.total -eq 1)
  Check 'einfacher Modus: create_ticket -> wrong_mode' ((Code (Tool 'create_ticket' @{ project = 'PAN'; type = 'Task'; title = 'x' })) -eq 'wrong_mode')
  Check 'einfacher Modus: get_ticket -> wrong_mode (englische Meldung)' ((Tool 'get_ticket' @{ ticket = 'PAN-1' }).text -match 'Expert mode is off')
  Check 'Todo-Aufruf im Protokoll ohne Langtext' ((Get-Content "$dir\paneldeck-data\mcp-log.jsonl" -Raw) -notmatch '2 Liter')

  Write-Host "== Aufgaben-Werkzeuge: Stufe Nur lesen" -ForegroundColor Cyan
  Set-Level 'Read'
  $names = @((Rpc 'tools/list').result.tools | ForEach-Object name)
  Check 'Nur lesen: list_todos ja, add_todo nein' ($names -contains 'list_todos' -and $names -notcontains 'add_todo' -and $names -notcontains 'set_todo_done')
  Check 'Nur lesen: add_todo verboten' ((Code (Tool 'add_todo' @{ title = 'x' })) -eq 'forbidden')
  Check 'Nur lesen: list_todos liest die Karte' ((Tool 'list_todos' @{ state = 'all' }).data.total -eq 1)

  Write-Host "== Aufgaben-Werkzeuge: Projekte und Tickets (Expertenmodus an)" -ForegroundColor Cyan
  $py = Get-Command python -ErrorAction SilentlyContinue
  Check 'python fuer die Praeparierung der Datenbank vorhanden' ($null -ne $py)
  if ($py) {
    $seed = @'
import sqlite3, sys, json
c = sqlite3.connect(sys.argv[1])
c.execute('PRAGMA foreign_keys=ON')
now = '2026-01-01T00:00:00'
c.execute("INSERT INTO projects (key,name,description,color,work_dir,use_sprints,use_versions,next_number,position,created_at) VALUES ('PAN','Smoke','','#112233','',1,1,1,1,?)", (now,))
pid = c.execute("SELECT id FROM projects WHERE key='PAN'").fetchone()[0]
c.execute("INSERT INTO sprints (project_id,name,goal,start_date,end_date,state,default_days,created_at,started_at) VALUES (?,?,?,?,?,'active',10,?,?)", (pid, 'Sprint 1', 'Smoke goal', '2026-01-01', '2026-01-14', now, now))
c.execute("INSERT INTO versions (project_id,name,position) VALUES (?,?,0)", (pid, '1.0'))
st = {}
for i, (n, cat) in enumerate([('To Do', 'todo'), ('In Progress', 'progress'), ('Done', 'done')]):
    c.execute("INSERT INTO statuses (project_id,name,color,category,position) VALUES (?,?,?,?,?)", (pid, n, '#888888', cat, i))
    st[cat] = c.execute('SELECT last_insert_rowid()').fetchone()[0]
c.execute("INSERT INTO ticket_types (project_id,name,color,workflow,system,position) VALUES (?,?,?,?,0,0)", (pid, 'Task', '#3366cc', 1))
tt = c.execute('SELECT last_insert_rowid()').fetchone()[0]
c.execute("INSERT INTO ticket_types (project_id,name,color,workflow,system,position) VALUES (?,?,?,?,0,1)", (pid, 'Note', '#999999', 0))
for i, cat in enumerate(['todo', 'progress', 'done']):
    c.execute("INSERT INTO type_statuses (type_id,status_id,position) VALUES (?,?,?)", (tt, st[cat], i))
def field(name, kind, required, options='{}', pos=0):
    c.execute("INSERT INTO fields (type_id,name,kind,options,required,position) VALUES (?,?,?,?,?,?)", (tt, name, kind, options, required, pos))
    return c.execute('SELECT last_insert_rowid()').fetchone()[0]
field('Area', 'text', 1, pos=0)
res = field('Result', 'text', 0, pos=1)
field('Priority', 'dropdown', 0, json.dumps({'values': [{'id': 1, 'name': 'Low'}, {'id': 2, 'name': 'High'}]}), pos=2)
def tr(a, b, req=()):
    c.execute("INSERT INTO transitions (type_id,from_status_id,to_status_id,required_fields) VALUES (?,?,?,?)", (tt, st[a], st[b], json.dumps(list(req))))
tr('todo', 'progress'); tr('progress', 'done', [res]); tr('progress', 'todo'); tr('done', 'todo')
for i, n in enumerate(['Development', 'Testing']):
    c.execute("INSERT INTO work_kinds (project_id,name,position) VALUES (?,?,?)", (pid, n, i))
c.execute("INSERT INTO meta (key,value) VALUES ('expert_mode','1') ON CONFLICT(key) DO UPDATE SET value=excluded.value")
c.commit()
c.close()
'@
    $seedFile = "$work\seed.py"; [IO.File]::WriteAllText($seedFile, $seed, (New-Object Text.UTF8Encoding $false))
    $script:prestart = { $env:PYTHONUTF8 = '1'; & python -I $seedFile "$dir\paneldeck-data\paneldeck.db"; if ($LASTEXITCODE -ne 0) { throw 'seed.py fehlgeschlagen' } }
    Set-Level 'Control'
    $script:prestart = $null
    [void](Rpc 'initialize' @{ protocolVersion = '2025-06-18'; capabilities = @{}; clientInfo = @{ name = 'smoke'; version = '1' } })
    $names = @((Rpc 'tools/list').result.tools | ForEach-Object name)
    $expectTools = @('list_projects', 'get_project', 'search_tickets', 'get_ticket', 'get_time_entries', 'list_sprints', 'get_sprint', 'list_versions', 'create_ticket', 'update_ticket', 'transition_ticket', 'add_comment', 'log_time', 'link_ticket', 'link_terminal')
    Check 'Expertenmodus: tools/list hat die Ticket-Werkzeuge' (@($expectTools | Where-Object { $names -notcontains $_ }).Count -eq 0)
    Check 'Expertenmodus: keine schreibenden Todo-Werkzeuge, list_todos bleibt' ($names -notcontains 'add_todo' -and $names -notcontains 'set_todo_done' -and $names -contains 'list_todos')
    Check 'Expertenmodus: add_todo -> wrong_mode' ((Code (Tool 'add_todo' @{ title = 'x' })) -eq 'wrong_mode')
    Check 'Expertenmodus: set_todo_done -> wrong_mode' ((Code (Tool 'set_todo_done' @{ id = 1 })) -eq 'wrong_mode')
    $lt = Tool 'list_todos' @{ state = 'all' }
    Check 'Expertenmodus: list_todos nur lesbar (read_only)' ($lt.data.read_only -eq $true -and $lt.data.total -eq 1)
    $lp = Tool 'list_projects'
    Check 'list_projects: TODO (schreibgeschuetzt) und PAN' (@($lp.data.projects | Where-Object { $_.key -eq 'TODO' -and $_.read_only }).Count -eq 1 -and @($lp.data.projects | Where-Object key -eq 'PAN').Count -eq 1)
    $gp = Tool 'get_project' @{ project = 'pan' }
    $task = $gp.data.types | Where-Object name -eq 'Task'
    Check 'get_project: Typ, Felder, Workflow, Uebergaenge' (-not $gp.err -and @($task.fields).Count -eq 3 -and @($task.workflow.transitions).Count -eq 4 -and (($task.workflow.transitions | Where-Object { $_.to -eq 'Done' }).required_fields -contains 'Result'))
    Check 'get_project: unbekanntes Projekt nennt die verfuegbaren' ((Tool 'get_project' @{ project = 'ZZZ' }).data.error.details.available -contains 'PAN')
    $c1 = Tool 'create_ticket' @{ project = 'PAN'; type = 'Task'; title = 'Login reparieren' }
    Check 'create_ticket ohne Pflichtfeld: required_missing mit Liste' ((Code $c1) -eq 'required_missing' -and $c1.data.error.details.missing_fields[0].name -eq 'Area') $c1.text
    $c1 = Tool 'create_ticket' @{ project = 'PAN'; type = 'Task'; title = 'Login reparieren'; fields = @{ Area = 'auth'; Priority = 'High' } }
    Check 'create_ticket: Schluessel, Startstatus, Dropdown per Name' (-not $c1.err -and $c1.data.ticket.key -eq 'PAN-1' -and $c1.data.ticket.status -eq 'To Do' -and (($c1.data.ticket.fields | Where-Object name -eq 'Priority').value -eq 'High')) $c1.text
    Check 'create_ticket: unbekannter Typ nennt die Typen' ((Tool 'create_ticket' @{ project = 'PAN'; type = 'Epic'; title = 'x' }).data.error.details.available -contains 'Task')
    Check 'create_ticket: ungueltiger Dropdown-Wert: invalid_value' ((Code (Tool 'create_ticket' @{ project = 'PAN'; type = 'Task'; title = 'x'; fields = @{ Area = 'a'; Priority = 'Gigantisch' } })) -eq 'invalid_value')
    Check 'create_ticket: Feld, das der Typ nicht hat: bad_args' ((Code (Tool 'create_ticket' @{ project = 'PAN'; type = 'Task'; title = 'x'; fields = @{ Area = 'a'; Nope = 1 } })) -eq 'bad_args')
    Check 'create_ticket im System-Projekt TODO: forbidden' ((Code (Tool 'create_ticket' @{ project = 'TODO'; type = 'todolist'; title = 'x' })) -eq 'forbidden')
    $n2 = Tool 'create_ticket' @{ project = 'PAN'; type = 'Note'; title = 'Notiz ohne Workflow'; fields = @{} }
    Check 'Typ ohne Workflow: kein Status, transition_ticket -> no_workflow' ($n2.data.ticket.key -eq 'PAN-2' -and $null -eq $n2.data.ticket.status -and (Code (Tool 'transition_ticket' @{ ticket = 'PAN-2'; status = 'Done' })) -eq 'no_workflow')
    $u = Tool 'update_ticket' @{ ticket = 'PAN-1'; title = 'Login endgueltig reparieren'; fields = @{ Result = 'noch offen' } }
    Check 'update_ticket: Titel und Feld' (-not $u.err -and $u.data.changed -eq $true -and $u.data.ticket.title -eq 'Login endgueltig reparieren') $u.text
    Check 'update_ticket ohne Aenderung: changed=false' ((Tool 'update_ticket' @{ ticket = 'PAN-1'; title = 'Login endgueltig reparieren' }).data.changed -eq $false)
    Check 'update_ticket: Status nur ueber transition_ticket (Parameter status unbekannt)' ((Code (Tool 'update_ticket' @{ ticket = 'PAN-1'; status = 'Done' })) -eq 'bad_args')
    Check 'transition_ticket: nicht erlaubter Uebergang' ((Code (Tool 'transition_ticket' @{ ticket = 'PAN-1'; status = 'Done' })) -eq 'transition_not_allowed')
    Check 'transition_ticket: erlaubter Uebergang' ((Tool 'transition_ticket' @{ ticket = 'PAN-1'; status = 'In Progress' }).data.ticket.status -eq 'In Progress')
    $e3 = Tool 'update_ticket' @{ ticket = 'PAN-1'; fields = @{ Result = $null } }
    $e4 = Tool 'transition_ticket' @{ ticket = 'PAN-1'; status = 'Done' }
    Check 'transition_ticket: fehlende Pflichtfelder des Uebergangs mit Liste' ((Code $e4) -eq 'transition_needs_fields' -and $e4.data.error.details.missing_fields[0].name -eq 'Result') $e4.text
    $e5 = Tool 'transition_ticket' @{ ticket = 'PAN-1'; status = 'Done'; fields = @{ Result = 'behoben' } }
    Check 'transition_ticket: Pflichtfeld in derselben Operation mitgeben' (-not $e5.err -and $e5.data.ticket.status -eq 'Done') $e5.text
    Check 'add_comment' (-not (Tool 'add_comment' @{ ticket = 'PAN-1'; body = 'GEHEIM-KOMMENTAR-XYZ' }).err)
    Check 'add_comment: leer -> bad_args' ((Code (Tool 'add_comment' @{ ticket = 'PAN-1'; body = '  ' })) -eq 'bad_args')
    $lg = Tool 'log_time' @{ ticket = 'PAN-1'; duration = '1h 30m'; comment = 'Review' }
    Check 'log_time: 1h 30m = 90 Minuten, Standard-Art Development' ($lg.data.entry.minutes -eq 90 -and $lg.data.entry.work_kind -eq 'Development') $lg.text
    $lg2 = Tool 'log_time' @{ ticket = 'PAN-1'; duration = 2; start = '2026-11-13'; work_kind = 'Testing' }
    Check 'log_time: bloße Zahl = Stunden, Datum = 08:00' ($lg2.data.entry.minutes -eq 120 -and $lg2.data.entry.start -eq '2026-11-13T08:00:00' -and $lg2.data.ticket_total_minutes -eq 210) $lg2.text
    Check 'log_time: ungueltige Dauer -> bad_args' ((Code (Tool 'log_time' @{ ticket = 'PAN-1'; duration = 'abc' })) -eq 'bad_args')
    Check 'get_time_entries' ((Tool 'get_time_entries' @{ ticket = 'PAN-1' }).data.entries_total -eq 2)
    $lk = Tool 'link_ticket' @{ ticket = 'PAN-1'; to_ticket = 'PAN-2'; type = 'blocks' }
    Check 'link_ticket: Ticket mit Ticket' (-not $lk.err -and $lk.data.type -eq 'blocks') $lk.text
    Check 'link_ticket: doppelt -> already_exists' ((Code (Tool 'link_ticket' @{ ticket = 'PAN-1'; to_ticket = 'PAN-2'; type = 'blocks' })) -eq 'already_exists')
    Check 'link_ticket: mit sich selbst -> link_not_allowed' ((Code (Tool 'link_ticket' @{ ticket = 'PAN-1'; to_ticket = 'PAN-1' })) -eq 'link_not_allowed')
    $s1 = Tool 'search_tickets' @{ project = 'PAN'; status = 'Done' }
    Check 'search_tickets: nach Status' ($s1.data.total -eq 1 -and $s1.data.tickets[0].key -eq 'PAN-1')
    Check 'search_tickets: Text, Seiten' ((Tool 'search_tickets' @{ text = 'notiz' }).data.total -eq 1 -and (Tool 'search_tickets' @{ limit = 1; offset = 1 }).data.tickets.Count -eq 1)
    Check 'search_tickets: type ohne project -> bad_args' ((Code (Tool 'search_tickets' @{ type = 'Task' })) -eq 'bad_args')
    $q1 = Tool 'search_tickets' @{ project = 'PAN'; query = 'status = Done ORDER BY key' }
    Check 'search_tickets: query (Abfragesprache)' (-not $q1.err -and $q1.data.total -eq 1 -and $q1.data.tickets[0].key -eq 'PAN-1') $q1.text
    Check 'search_tickets: query mit Sprint-Funktion (leerer Sprint)' ((Tool 'search_tickets' @{ project = 'PAN'; query = 'sprint = activeSprint()' }).data.total -eq 0)
    $q2 = Tool 'search_tickets' @{ project = 'PAN'; query = 'type = Task AND nonsense = 1' }
    Check 'search_tickets: ungueltige query -> invalid_query mit Position' ((Code $q2) -eq 'invalid_query' -and $q2.text -match 'Position 17')
    Check 'search_tickets: query mit text -> bad_args' ((Code (Tool 'search_tickets' @{ query = ''; text = 'x' })) -eq 'bad_args')
    $ls = Tool 'list_sprints' @{ project = 'PAN' }
    Check 'list_sprints: Name, Ziel, Status' (-not $ls.err -and $ls.data.total -eq 1 -and $ls.data.sprints[0].name -eq 'Sprint 1' -and $ls.data.sprints[0].goal -eq 'Smoke goal' -and $ls.data.sprints[0].state -eq 'active') $ls.text
    $gs = Tool 'get_sprint' @{ project = 'PAN'; sprint = 'sprint 1' }
    Check 'get_sprint: Name mit project, Bericht' (-not $gs.err -and $gs.data.sprint.name -eq 'Sprint 1' -and $null -ne $gs.data.report.planned) $gs.text
    Check 'get_sprint: unbekannter Sprint -> not_found' ((Code (Tool 'get_sprint' @{ project = 'PAN'; sprint = 'Nope' })) -eq 'not_found')
    Check 'list_versions: Version 1.0' ((Tool 'list_versions' @{ project = 'PAN' }).data.versions[0].name -eq '1.0')
    $gt = Tool 'get_ticket' @{ ticket = 'PAN-1' }
    Check 'get_ticket: Kommentar, Zeit, Verknuepfung, Verlauf' (-not $gt.err -and $gt.data.comments_total -eq 1 -and $gt.data.time.entries_total -eq 2 -and @($gt.data.links).Count -eq 1 -and @($gt.data.history | Where-Object source -eq 'agent').Count -ge 5)
    Check 'get_ticket: Hinweis auf nicht vertrauenswuerdige Texte' ($gt.data.note -match 'untrusted')
    Check 'Ticket-Aufruf im Protokoll ohne Kommentartext' ((Get-Content "$dir\paneldeck-data\mcp-log.jsonl" -Raw) -notmatch 'GEHEIM-KOMMENTAR')

    Write-Host "== Aufgaben-Werkzeuge: Sitzung und Ticket verknuepfen, Quelle im Verlauf" -ForegroundColor Cyan
    $foreign = ((Tool 'list_sessions').data.sessions | Select-Object -First 1).id
    Check 'link_terminal: Sitzung des Nutzers (nicht selbst angelegt) -> forbidden' ((Code (Tool 'link_terminal' @{ ticket = 'PAN-1'; session = $foreign })) -eq 'forbidden')
    Check 'link_terminal von aussen ohne session -> bad_args' ((Code (Tool 'link_terminal' @{ ticket = 'PAN-1' })) -eq 'bad_args')
    $pc = Tool 'create_session' @{ shell = 'powershell'; cwd = "$proj"; name = 'Plan-Agent' }; $A = $pc.data.id; Start-Sleep 4
    $ow = Tool 'create_session' @{ shell = 'powershell'; cwd = "$proj"; name = 'Von-aussen' }; $O = $ow.data.id
    $lt1 = Tool 'link_terminal' @{ ticket = 'PAN-1'; session = $O }
    Check 'link_terminal: selbst angelegte Sitzung (von aussen) -> session:<id>' (-not $lt1.err -and $lt1.data.terminal -eq "session:$O") $lt1.text
    Check 'link_terminal: doppelt -> already_exists' ((Code (Tool 'link_terminal' @{ ticket = 'PAN-1'; session = $O })) -eq 'already_exists')
    $own = InA 'link_terminal' @{ ticket = 'PAN-2' }
    Check 'link_terminal: eigene Sitzung (Prozessbaum) ohne session-Angabe' ($own.isError -ne $true -and $own.structuredContent.terminal -eq "session:$A") ($own | ConvertTo-Json -Compress -Depth 5)
    $oth = InA 'link_terminal' @{ ticket = 'PAN-2'; session = $O }
    Check 'link_terminal: Sitzung eines anderen Agenten -> forbidden' ($oth.isError -eq $true -and $oth.content[0].text -like 'forbidden*')
    Check 'get_ticket: linked_terminals' ((Tool 'get_ticket' @{ ticket = 'PAN-2' }).data.linked_terminals -contains "session:$A")
    $cm = InA 'add_comment' @{ ticket = 'PAN-2'; body = 'vom Plan-Agent' }
    Check 'Agent in Sitzung schreibt einen Kommentar' ($cm.isError -ne $true) ($cm | ConvertTo-Json -Compress -Depth 5)
    $h = (Tool 'get_ticket' @{ ticket = 'PAN-2' }).data.history
    Check 'Verlauf: Quelle agent mit Sitzungsname (Plan-Agent)' (@($h | Where-Object { $_.source -eq 'agent' -and $_.source_detail -eq 'Plan-Agent' }).Count -ge 1)
    Check 'Verlauf: Aufruf von aussen ohne Sitzung hat Quelle agent ohne Namen' (@((Tool 'get_ticket' @{ ticket = 'PAN-1' }).data.history | Where-Object { $_.source -eq 'agent' -and $null -eq $_.source_detail }).Count -ge 1)
    [void](Tool 'close_session' @{ session = $A }); [void](Tool 'close_session' @{ session = $O })

    Write-Host "== Aufgaben-Werkzeuge: Expertenmodus, Stufe Nur lesen" -ForegroundColor Cyan
    Set-Level 'Read'
    $names = @((Rpc 'tools/list').result.tools | ForEach-Object name)
    Check 'Expert + Nur lesen: Ticket-Lesewerkzeuge ja, schreibende nein' ($names -contains 'get_ticket' -and $names -contains 'search_tickets' -and $names -notcontains 'create_ticket' -and $names -notcontains 'link_terminal')
    Check 'Expert + Nur lesen: create_ticket verboten' ((Code (Tool 'create_ticket' @{ project = 'PAN'; type = 'Task'; title = 'x' })) -eq 'forbidden')
    Check 'Expert + Nur lesen: get_ticket liest' ((Tool 'get_ticket' @{ ticket = 'PAN-1' }).data.key -eq 'PAN-1')
    Check 'Expert + Nur lesen: list_sprints, list_versions und query lesen' ($names -contains 'list_sprints' -and $names -contains 'get_sprint' -and $names -contains 'list_versions' -and (Tool 'list_sprints' @{ project = 'PAN' }).data.total -eq 1 -and (Tool 'search_tickets' @{ query = 'type = Task' }).data.total -ge 1)
  }
} finally {
  Stop-Hub
  if (-not $KeepFiles) { Remove-Item -LiteralPath $work -Recurse -Force -ErrorAction SilentlyContinue }
}
Write-Host ""
Write-Host ("Ergebnis: {0} bestanden, {1} fehlgeschlagen" -f $script:pass, $script:fail) -ForegroundColor $(if ($script:fail -eq 0) { 'Green' } else { 'Red' })
exit $(if ($script:fail -eq 0) { 0 } else { 1 })
