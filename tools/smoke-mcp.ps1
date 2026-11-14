<#
.SYNOPSIS
  Ende-zu-Ende-Test der Agenten-Schnittstelle (MCP) gegen eine echte, laufende Paneldeck-Instanz.

.DESCRIPTION
  Kopiert die .exe in einen frischen Temp-Ordner (eigener Datenordner, deine Einstellungen bleiben unberührt),
  startet sie minimiert und spricht per JSON-RPC mit `paneldeck.exe mcp` – so wie es ein Agent tut.
  Geprüft werden Protokoll, die drei Stufen (Aus/Lesen/Steuern), Lese- und Steuer-Werkzeuge, run_command in
  allen vorhandenen Shells, simulierte Claude-Hooks (queue_prompt/wait_for_status) und die Sicherheitsregeln.
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
} finally {
  Stop-Hub
  if (-not $KeepFiles) { Remove-Item -LiteralPath $work -Recurse -Force -ErrorAction SilentlyContinue }
}
Write-Host ""
Write-Host ("Ergebnis: {0} bestanden, {1} fehlgeschlagen" -f $script:pass, $script:fail) -ForegroundColor $(if ($script:fail -eq 0) { 'Green' } else { 'Red' })
exit $(if ($script:fail -eq 0) { 0 } else { 1 })
