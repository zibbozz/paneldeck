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
  Isolation (tools\test-env.ps1): Hubs, Bruecken und Terminals laufen mit Schein-Profil (USERPROFILE, HOME, APPDATA,
  LOCALAPPDATA, CLAUDE_CONFIG_DIR unter dem Arbeitsordner), ohne CLAUDECODE/CLAUDE_*/ANTHROPIC_* und mit einem PATH, in
  dem nur ein Ersatz-claude (fakebin\claude.cmd) liegt; das echte Claude Code wird nie gestartet. Am Ende prueft der Test,
  dass das echte ~/.claude unberuehrt blieb.

.EXAMPLE
  powershell -ExecutionPolicy Bypass -File tools\smoke-mcp.ps1
  powershell -ExecutionPolicy Bypass -File tools\smoke-mcp.ps1 -Exe dist\paneldeck-0.20.0.exe -Visible
#>
param(
  [string]$Exe = "$PSScriptRoot\..\target\release\paneldeck.exe",
  [switch]$Visible,
  [switch]$KeepFiles,
  # Ordner fuer die Kopie der .exe und die Test-Datenordner (z. B. ein vom Virenscanner ausgenommener Ordner)
  [string]$WorkRoot = $env:TEMP,
  # Schutzpruefung streng: jede Aenderung von ~/.claude.json bzw. ~/.claude/settings.json ist ein Fehler (sonst nur,
  # wenn sie den Testordner nennt - ein gleichzeitig laufendes, echtes Claude Code schreibt ~/.claude.json laufend)
  [switch]$StrictClaudeConfig,
  # Konsolen-Host (#183): conpty.dll + OpenConsole.exe neben -Exe werden mitkopiert (tools\fetch-conpty.ps1 -Dest <ordner der exe>).
  #   auto    = Standardeinstellung; mitgelieferte ConPTY, wenn die Dateien da sind
  #   bundled = wie auto, aber die Dateien muessen da sein und genutzt werden
  #   system  = Einstellung "Mitgelieferten Konsolen-Host verwenden" aus (Dateien liegen trotzdem daneben)
  [ValidateSet('auto', 'bundled', 'system')][string]$ConPty = 'auto',
  # Ordner mit conpty.dll + OpenConsole.exe (Standard: der Ordner von -Exe); release.sh gibt dist\conpty an
  [string]$ConPtyDir = '',
  # OSC-52-Pruefung (#181) auslassen: sie setzt kurz die echte Windows-Zwischenablage (Text wird gesichert und
  # wiederhergestellt, eine Zwischenablage mit anderen als reinen Textformaten bleibt ohnehin unberuehrt)
  [switch]$NoClipboard
)
$ErrorActionPreference = 'Stop'
$Exe = (Resolve-Path $Exe).Path
New-Item -ItemType Directory -Force $WorkRoot | Out-Null
$work = Join-Path $WorkRoot ("hubsmoke-" + [guid]::NewGuid().ToString('N').Substring(0, 8))
$dir = New-Item -ItemType Directory -Force "$work\hub"; $proj = New-Item -ItemType Directory -Force "$work\proj"
Copy-Item $Exe "$dir\paneldeck.exe"
$exeDir = if ($ConPtyDir) { $ConPtyDir } else { Split-Path $Exe }
$hasConPty = (Test-Path "$exeDir\conpty.dll") -and (Test-Path "$exeDir\OpenConsole.exe")
if ($hasConPty) { Copy-Item "$exeDir\conpty.dll", "$exeDir\OpenConsole.exe" $dir }
if ($ConPty -eq 'bundled' -and -not $hasConPty) { Write-Host "-ConPty bundled: conpty.dll/OpenConsole.exe fehlen in $exeDir (tools\fetch-conpty.ps1 -Dest $exeDir)" -ForegroundColor Red; exit 1 }
$useBundled = $hasConPty -and $ConPty -ne 'system'
Write-Host ("Konsolen-Host: " + $(if ($useBundled) { 'mitgeliefert (OpenConsole)' } elseif ($ConPty -eq 'system') { 'Windows (Einstellung aus)' } else { 'Windows (keine mitgelieferten Dateien)' })) -ForegroundColor DarkGray
# Arbeitsordner des naechsten Hub-Starts (Set-Level), sonst der des Skripts.
$script:hubCwd = $null
# Datenordner der Test-Instanz (portabel neben der Kopie). Ein gesetztes PANELDECK_DATA bzw. PANELDECK_HUB_DATA
# eines umgebenden Paneldeck-Terminals wuerde Hub und Bruecke sonst auf andere Ordner lenken.
$data = "$dir\paneldeck-data"
# Alle PANELDECK_*-Variablen eines umgebenden Paneldeck-Terminals entfernen: PANELDECK_SESSION/PORT/TOKEN (vom Hub in
# jedes Terminal gesetzt) liessen die Bruecke sonst als "Sitzung 1" der Test-Instanz gelten, und die Pruefungen
# "von aussen" schluegen fehl, wenn das Skript selbst in einem Paneldeck-Terminal laeuft.
Get-ChildItem Env: | Where-Object { $_.Name -like 'PANELDECK_*' } | ForEach-Object { Remove-Item -LiteralPath "Env:$($_.Name)" -ErrorAction SilentlyContinue }
# Isolation (fail closed): nie das echte Claude Code und nie das echte ~/.claude. Ab hier gelten fuer dieses Skript und damit
# fuer jeden Hub, jede Bruecke und jedes Terminal ein Schein-Profil unter $work\profile, ein PATH ohne echtes claude und ein
# Ersatz-claude in $work\fakebin (Aufrufe -> fakebin\calls.txt, Eingaben -> fakebin\input.txt). Siehe tools\test-env.ps1.
. "$PSScriptRoot\test-env.ps1"
$realHome = $env:USERPROFILE
$realSnap = Get-RealClaudeSnapshot $realHome
$tag = Split-Path -Leaf $work
$fakebin = New-FakeClaudeBin "$work\fakebin"
$calls = "$fakebin\calls.txt"; $inputs = "$fakebin\input.txt"
$script:iso = Enter-IsolatedTestEnv $work $fakebin
# OSC 52 (#181): Nur die fokussierte Sitzung im Fenster im Vordergrund darf kopieren. Der Test-Hub laeuft minimiert und ohne
# Fokus, deshalb hebt diese Variable (nur in der Umgebung der Test-Hubs) die Fokus-Regel auf; siehe src\app\osc52.rs.
$env:PANELDECK_TEST_OSC52_ANY_FOCUS = '1'
Write-Host "Isolation: USERPROFILE=$env:USERPROFILE, claude -> $((Get-Command claude -CommandType Application).Source)" -ForegroundColor DarkGray
# Die Terminals des Hubs bekommen ihre Umgebung von portable-pty frisch aus der Registry (echter PATH mit ~/.local/bin).
# Deshalb laeuft jede Sitzung ueber das Profil 1 "Smoke-Isolation" (Standardprofil jedes Projekts, auch jeder
# wiederhergestellten Sitzung), dessen Variablen PATH/Profil wieder auf die Schein-Umgebung setzen. Vor jedem Hub-Start.
$ptyEnv = Get-PtyIsolationEnv $script:iso
function Set-IsolationProfile {
  $u8 = New-Object Text.UTF8Encoding $false
  $f = "$data\settings.json"
  $j = [IO.File]::ReadAllText($f).TrimStart([char]0xFEFF) | ConvertFrom-Json
  $p = [pscustomobject]@{ id = 1; name = 'Smoke-Isolation'; env = $ptyEnv }
  if ($j.PSObject.Properties.Name -contains 'profiles') { $j.profiles = @($p) } else { $j | Add-Member profiles @($p) }
  [IO.File]::WriteAllText($f, ($j | ConvertTo-Json -Depth 10), $u8)
  $sf = "$data\state.json"
  if (Test-Path $sf) {
    $s = [IO.File]::ReadAllText($sf).TrimStart([char]0xFEFF) | ConvertFrom-Json
    if (-not ($s.PSObject.Properties.Name -contains 'projects') -or -not @($s.projects).Count) { $s | Add-Member -Force projects @([pscustomobject]@{ id = 1; name = 'Standard' }) }
    foreach ($o in @($s.projects) + @($s.sessions | Where-Object { $_ })) { $o | Add-Member -Force profile 1 }
    [IO.File]::WriteAllText($sf, ($s | ConvertTo-Json -Depth 30), $u8)
  } else {
    [IO.File]::WriteAllText($sf, '{"projects":[{"id":1,"name":"Standard","profile":1}],"active_project":1,"next_id":1}', $u8)
  }
}
$script:extra = @()
$script:prestart = $null
$script:pass = 0; $script:fail = 0; $script:hub = $null; $script:bridge = $null; $script:rpc = 0

function Check($name, [bool]$ok, $detail = '') {
  if ($ok) { $script:pass++; Write-Host "  ok    $name" -ForegroundColor Green }
  else { $script:fail++; Write-Host "  FAIL  $name  $detail" -ForegroundColor Red }
  $script:lastOk = $ok
}
# Pruefung, die in diesem Build nicht moeglich ist (z. B. Funktion eines anderen Teils fehlt): weder bestanden noch fehlgeschlagen.
$script:skip = 0
function Skip($name, $why) { $script:skip++; Write-Host "  skip  $name  ($why)" -ForegroundColor Yellow }
# OSC 52 (#181): ein Programm im Terminal kopiert in die Zwischenablage, der Hub legt den Text in die Windows-Zwischenablage.
# Beruehrt die echte Zwischenablage: vorhandener Text wird vorher gesichert und danach wiederhergestellt (nur, wenn
# inzwischen niemand anderes etwas kopiert hat). Enthaelt sie andere als reine Textformate (Bild, Dateien, HTML, RTF ...), wird die
# Pruefung uebersprungen und die Zwischenablage nicht angefasst. -NoClipboard ueberspringt sie immer.
function Test-Osc52([int]$sid, [string]$sh) {
  $name = "OSC 52: Programm in $sh kopiert in die Windows-Zwischenablage"
  if ($NoClipboard) { Skip $name '-NoClipboard'; return }
  Add-Type -AssemblyName System.Windows.Forms
  $C = [System.Windows.Forms.Clipboard]
  try {
    # Nur reine Textformate (leere Zwischenablage = keine Formate) - alles andere ginge beim Wiederherstellen verloren.
    $textOnly = @('Text', 'UnicodeText', 'OEMText', 'Locale', 'System.String')
    $obj = $C::GetDataObject()
    $formats = if ($obj) { @($obj.GetFormats()) } else { @() }
    $other = @($formats | Where-Object { $textOnly -notcontains $_ })
    if ($other.Count -gt 0) {
      Skip $name "Zwischenablage enthaelt andere Formate ($($other -join ', ')) und bleibt unberuehrt"; return
    }
    $had = $C::ContainsText(); $saved = if ($had) { $C::GetText() } else { '' }
  } catch { Skip $name "Zwischenablage nicht lesbar: $($_.Exception.Message)"; return }
  $marker = 'hello-osc52-' + [guid]::NewGuid().ToString('N').Substring(0, 8)
  $cmd = if ($sh -eq 'gitbash') { "printf '\033]52;c;%s\a' `"`$(printf $marker | base64)`"" }
         else { '$b = [Convert]::ToBase64String([Text.Encoding]::UTF8.GetBytes(''' + $marker + ''')); [Console]::Write("$([char]27)]52;c;$b$([char]7)")' }
  $r = Tool 'run_command' @{ session = $sid; command = $cmd; timeout_secs = 25 }
  $got = ''
  for ($i = 0; $i -lt 50 -and $got -ne $marker; $i++) { Start-Sleep -Milliseconds 100; try { $got = $C::GetText() } catch { } }
  Check $name ($got -eq $marker) "Zwischenablage: '$got', run_command: $($r.text)"
  try {
    if ($C::GetText() -eq $marker) { if ($had) { $C::SetText($saved) } else { $C::Clear() } }
  } catch { Write-Host "  Hinweis: Zwischenablage nicht wiederhergestellt: $($_.Exception.Message)" -ForegroundColor Yellow }
}
function Set-Level($level, [switch]$Empty, [switch]$Tray) {
  Stop-Hub
  $f = "$data\settings.json"
  New-Item -ItemType Directory -Force "$data" | Out-Null
  $j = if (Test-Path $f) { [IO.File]::ReadAllText($f).TrimStart([char]0xFEFF) | ConvertFrom-Json } else { [pscustomobject]@{} }
  if ($j.PSObject.Properties.Name -contains 'mcp_level') { $j.mcp_level = $level } else { $j | Add-Member mcp_level $level }
  if ($ConPty -eq 'system') { $j | Add-Member -Force bundled_conpty $false }
  [IO.File]::WriteAllText($f, ($j | ConvertTo-Json -Depth 10), (New-Object Text.UTF8Encoding $false))
  if ($script:prestart) { & $script:prestart }
  # -Empty: ohne gespeicherte Sitzungen (Set-IsolationProfile legt dann einen state.json nur mit dem Projekt an)
  if ($Empty) { Remove-Item -LiteralPath "$data\state.json" -ErrorAction SilentlyContinue }
  Set-IsolationProfile
  $style = if ($Visible) { 'Normal' } else { 'Minimized' }
  $cwdArg = if ($script:hubCwd) { @{ WorkingDirectory = $script:hubCwd } } else { @{} }
  # Paneldeck startet ohne Terminal (#111); sonst eine Sitzung des Nutzers per --open anlegen (wie ein Start aus dem Explorer).
  if ($Empty) {
    $script:hub = Start-Process "$dir\paneldeck.exe" -PassThru -WindowStyle $style @cwdArg
  } elseif ($Tray) {
    # Vorbereiteten state.json behalten; mit --tray bleiben auch wiederhergestellte weitere Fenster verborgen (#110).
    $script:hub = Start-Process "$dir\paneldeck.exe" -PassThru -WindowStyle $style -ArgumentList '--tray' @cwdArg
  } else {
    $script:hub = Start-Process "$dir\paneldeck.exe" -PassThru -WindowStyle $style -ArgumentList '--open', "`"$proj`"" @cwdArg
  }
  $t0 = Get-Date
  while (-not (Test-Path "$data\ipc.json") -and ((Get-Date) - $t0).TotalSeconds -lt 20) { Start-Sleep -Milliseconds 200 }
  Start-Sleep 2
  Start-Bridge
}
function Stop-Hub {
  if ($script:bridge -and -not $script:bridge.HasExited) { try { $script:bridge.StandardInput.Close(); [void]$script:bridge.WaitForExit(2000) } catch {} ; if (-not $script:bridge.HasExited) { $script:bridge.Kill() } }
  if ($script:hub -and -not $script:hub.HasExited) { $script:hub.Kill(); Start-Sleep -Milliseconds 800 }
  Remove-Item -LiteralPath "$data\ipc.json" -ErrorAction SilentlyContinue
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
# Inhalt einer Datei des Ersatz-claude (leer, wenn es sie nicht gibt); wartet bis $ms auf $pattern.
function Wait-FakeFile([string]$f, [string]$pattern, [int]$ms = 15000) {
  $t0 = Get-Date
  do {
    $txt = if (Test-Path -LiteralPath $f) { try { [IO.File]::ReadAllText($f) } catch { '' } } else { '' }
    if ($txt -match $pattern) { return $txt }
    Start-Sleep -Milliseconds 250
  } while (((Get-Date) - $t0).TotalMilliseconds -lt $ms)
  $txt
}
# Jeder gefundene claude-Pfad (eine Zeile je Treffer) muss im Ersatz-Ordner liegen; mindestens einer.
function Only-Fake([string]$out) {
  $l = @(($out -split "`r?`n") | ForEach-Object { $_.Trim() } | Where-Object { $_ })
  $l.Count -ge 1 -and -not ($l | Where-Object { $_ -notmatch 'fakebin[\\/]claude(\.cmd)?$' })
}
$script:psClaudeFake = $false

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
  Set-Level 'Read' -Empty
  $ls0 = Tool 'list_sessions'; Check 'Start ohne Terminal (#111): keine Sitzung' (-not $ls0.err -and @($ls0.data.sessions).Count -eq 0) $ls0.text
  Set-Level 'Read'
  [void](Rpc 'initialize' @{ protocolVersion = '2025-06-18'; capabilities = @{}; clientInfo = @{ name = 'smoke'; version = '1' } })
  $l = Rpc 'tools/list'; $names = @($l.result.tools | ForEach-Object name)
  Check 'tools/list nur Lese-Werkzeuge' ($names -contains 'list_sessions' -and $names -contains 'read_terminal' -and $names -notcontains 'send_text')
  $ls = Tool 'list_sessions'; Check 'list_sessions' (-not $ls.err -and $ls.data.sessions.Count -ge 1)
  $sid = $ls.data.sessions[0].id
  Check 'list_sessions: Feld window (0 = Hauptfenster)' ($ls.data.sessions[0].PSObject.Properties.Name -contains 'window' -and $ls.data.sessions[0].window -eq 0)
  Check 'list_sessions: Terminal hat kind = terminal' ($ls.data.sessions[0].kind -eq 'terminal') ($ls.data.sessions[0] | ConvertTo-Json -Compress)
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
    # Isolation im Terminal des Hubs: `claude` darf nur der Ersatz sein.
    $wc = switch ($sh) { 'cmd' { 'where claude' } 'gitbash' { 'type -ap claude' } default { 'Get-Command claude -All | ForEach-Object { if ($_.CommandType -in ''Application'', ''ExternalScript'') { $_.Source } else { "$($_.CommandType):$($_.Name)" } }' } }
    $r = Tool 'run_command' @{ session = $id; command = $wc; timeout_secs = 25 }
    Check "Isolation: claude im $sh-Terminal des Hubs ist nur der Ersatz (fakebin)" (Only-Fake $r.data.output) $r.data.output
    if ($sh -eq 'powershell' -and $script:lastOk) {
      $r = Tool 'run_command' @{ session = $id; command = '"$env:USERPROFILE|$env:CLAUDE_CONFIG_DIR|" + ((Get-ChildItem Env: | Where-Object { $_.Name -match ''^(CLAUDECODE|CLAUDE_CODE_|ANTHROPIC_)'' -and $_.Value }).Name -join '','')'; timeout_secs = 25 }
      $want = "$($script:iso.Profile)|$($script:iso.Profile)\.claude|"
      Check 'Isolation: Terminal-Umgebung mit Schein-Profil, ohne CLAUDECODE/CLAUDE_CODE_*/ANTHROPIC_*' ($r.data.output.Trim() -eq $want) "'$($r.data.output.Trim())' statt '$want'"
      if ($script:lastOk) { $script:psClaudeFake = $true }
    }
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
      Test-Osc52 $id $sh
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
      Test-Osc52 $id $sh
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

  Write-Host "== WSL: Linux-Startordner (#172)" -ForegroundColor Cyan
  # Erkannte WSL-Shells aus der Fehlermeldung zu einer unbekannten Shell; Vorlagen/System-Distributionen auslassen.
  $avail = (Tool 'create_session' @{ shell = 'gibtsnicht' }).text
  $wslSh = [regex]::Matches($avail, 'wsl:[^,\s]+') | ForEach-Object { $_.Value.TrimEnd('.') } | Where-Object { $_ -notmatch '(?i)-template$|^wsl:docker-desktop' } | Select-Object -First 1
  if (-not $wslSh) {
    Skip 'WSL: Linux-Startordner' 'keine WSL-Distribution'
  } else {
    # Einschleusung ueber `wsl.exe --cd` (ein `"` beendet dort den Wert): immer bad_args, ohne zu starten
    $inj = Tool 'create_session' @{ shell = $wslSh; cwd = '/tmp/x" echo INJECTED-$((6*7)); #'; name = 'smoke-wsl-inj' }
    Check "WSL: cwd mit Anfuehrungszeichen abgelehnt ($wslSh)" ((Code $inj) -eq 'bad_args') $inj.text
    Check 'WSL: cwd mit Backslash abgelehnt' ((Code (Tool 'create_session' @{ shell = $wslSh; cwd = '/tmp/a\b' })) -eq 'bad_args')
    Check 'WSL: ~/x abgelehnt (nur ~ allein)' ((Code (Tool 'create_session' @{ shell = $wslSh; cwd = '~/x' })) -eq 'bad_args')
    Check 'Linux-cwd fuer Windows-Shell abgelehnt' ((Code (Tool 'create_session' @{ shell = 'powershell'; cwd = '/tmp' })) -eq 'bad_args')
    # Fehlender Ordner: Pruefung in WSL im Hintergrund -> bad_args (antwortet WSL nicht, wird angelegt mit cwd_checked=false)
    $miss = Tool 'create_session' @{ shell = $wslSh; cwd = '/paneldeck-smoke-gibt-es-nicht'; name = 'smoke-wsl-miss' } 60000
    if ($miss.err) { Check 'WSL: fehlender Linux-Ordner = bad_args' ((Code $miss) -eq 'bad_args') $miss.text }
    else { Check 'WSL: fehlender Linux-Ordner (WSL antwortete nicht: cwd_checked=false)' ($miss.data.cwd_checked -eq $false) $miss.text; [void](Tool 'close_session' @{ session = $miss.data.id }) }
    # Vorhandener Ordner: Sitzung startet dort (wsl.exe --cd /tmp)
    $w = Tool 'create_session' @{ shell = $wslSh; cwd = '/tmp/'; name = 'smoke-wsl' } 60000
    Check "WSL: create_session mit Linux-cwd ($wslSh)" (-not $w.err -and $w.data.cwd -eq '/tmp' -and $w.data.shell -eq $wslSh) $w.text
    if (-not $w.err) {
      Check 'WSL: cwd_checked gesetzt' ($null -ne $w.data.cwd_checked) $w.text
      Start-Sleep 6
      $r = Tool 'run_command' @{ session = $w.data.id; command = 'pwd'; timeout_secs = 30 } 45000
      if ($r.data.finished) { Check 'WSL: Sitzung laeuft in /tmp (pwd)' ($r.data.output.Trim() -eq '/tmp') $r.text }
      else { $rt = Tool 'read_terminal' @{ session = $w.data.id; lines = 30 }; Skip 'WSL: pwd' "run_command nicht beendet (Shell-Integration in WSL?): $($rt.data.text)" }
      Check 'WSL: close_session' (-not (Tool 'close_session' @{ session = $w.data.id }).err)
    }
  }

  Write-Host "== Selbst-Identifikation und Kinder-Sitzungen" -ForegroundColor Cyan
  $w0 = Tool 'whoami'
  Check 'whoami von aussen: nicht in einem Hub-Terminal' ($null -eq $w0.data.session -and $w0.data.identified_by -eq 'none')
  $ac = Tool 'create_session' @{ shell = 'powershell'; cwd = "$proj"; name = 'Eltern-Agent' }; $A = $ac.data.id; Start-Sleep 4
  # Ein Agent in der Sitzung $A: startet die Bruecke als Kindprozess der Shell (Prozessbaum), ohne Umgebungsvariable.
  function InS($s, $tool, $targs) {
    $req = @{ jsonrpc = '2.0'; id = 1; method = 'tools/call'; params = @{ name = $tool; arguments = $targs } } | ConvertTo-Json -Depth 8 -Compress
    $cmd = "`$env:PANELDECK_SESSION=`$null; '" + $req.Replace("'", "''") + "' | & '$dir\paneldeck.exe' mcp | Out-String"
    $r = Tool 'run_command' @{ session = $s; command = $cmd; timeout_secs = 40 }
    try { ($r.data.output | ConvertFrom-Json).result } catch { $null }
  }
  function InA($tool, $targs) { InS $A $tool $targs }
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
  # Agenten-Kette (#152): das Kind ist markiert; ein Agent darin darf nicht in den Eltern-Agenten schreiben, nur lesen.
  $kw = InS $K 'whoami' @{}
  Check 'Kette: whoami im Kind nennt agent_chain = Eltern-Agent' ($kw.structuredContent.session -eq $K -and @($kw.structuredContent.agent_chain) -contains $A) ($kw | ConvertTo-Json -Compress -Depth 5)
  Check 'Kette: list_sessions zeigt agent_chain des Kindes' (@(((Tool 'list_sessions').data.sessions | Where-Object { $_.id -eq $K }).agent_chain) -contains $A)
  $kx = InS $K 'send_text' @{ session = $A; text = 'echo kind-leak' }
  Check 'Kette: Agent im Kind -> send_text in den Eltern-Agenten forbidden' ($kx.isError -eq $true -and $kx.content[0].text -like 'forbidden*' -and $kx.content[0].text -match 'agent chain') ($kx | ConvertTo-Json -Compress -Depth 5)
  Check 'Kette: Agent im Kind -> rename_session des Eltern-Agenten forbidden' ((InS $K 'rename_session' @{ session = $A; name = 'x' }).content[0].text -like 'forbidden*')
  Check 'Kette: Agent im Kind darf den Eltern-Agenten lesen' ((InS $K 'read_terminal' @{ session = $A; lines = 5 }).isError -ne $true)
  foreach ($t in @(@('run_command', @{ session = $A; command = 'echo kind-leak' }), @('send_keys', @{ session = $A; keys = @('enter') }), @('queue_prompt', @{ session = $A; prompt = 'x' }), @('focus_session', @{ session = $A }))) {
    $kr = InS $K $t[0] $t[1]
    Check "Kette: Agent im Kind -> $($t[0]) in den Eltern-Agenten forbidden" ($kr.isError -eq $true -and $kr.content[0].text -like 'forbidden*') ($kr | ConvertTo-Json -Compress -Depth 5)
  }
  # Gefaelschter/fehlender Aufrufer: eine rohe TCP-Anfrage mit dem Token aus ipc.json, ohne caller/caller_pid, aus dem
  # Prozessbaum des Kindes -> der Hub ermittelt den Absender selbst (TCP-Tabelle + Prozessbaum) und weist ab.
  $rawCmd = "`$j = Get-Content -Raw '$data\ipc.json' | ConvertFrom-Json; `$c = New-Object Net.Sockets.TcpClient('127.0.0.1', [int]`$j.port); `$s = `$c.GetStream(); `$b = [Text.Encoding]::UTF8.GetBytes('{""token"":""' + `$j.token + '"",""cmd"":""mcp"",""tool"":""send_text"",""caller"":null,""caller_pid"":4,""args"":{""session"":$A,""text"":""echo raw-leak""}}'); `$s.Write(`$b, 0, `$b.Length); `$c.Client.Shutdown(1); (New-Object IO.StreamReader(`$s)).ReadToEnd()"
  $raw = Tool 'run_command' @{ session = $K; command = $rawCmd; timeout_secs = 30 }
  Check 'Kette: rohe TCP-Anfrage ohne Aufrufer aus dem Kind -> zugeordnet und forbidden' ($raw.data.output -match '"code":"forbidden"' -and $raw.data.output -match 'agent chain') $raw.text
  # Losgeloester Absender: eine kurzlebige Shell im Kind startet per Start-Process einen Prozess und endet sofort; der
  # Prozess (Elternteil beendet, nach dem Hub gestartet = verwaist) schickt dann eine rohe Anfrage in den Eltern-Agenten.
  # Er ist keiner Sitzung zuzuordnen und darf nicht in einen moeglichen Vorfahren schreiben.
  $detOut = "$work\detached-out.txt"; $detPs = "$work\detached.ps1"
  [IO.File]::WriteAllText($detPs, @"
Start-Sleep -Seconds 3
`$j = Get-Content -Raw '$data\ipc.json' | ConvertFrom-Json
`$c = New-Object Net.Sockets.TcpClient('127.0.0.1', [int]`$j.port)
`$s = `$c.GetStream()
`$b = [Text.Encoding]::UTF8.GetBytes('{"token":"' + `$j.token + '","cmd":"mcp","tool":"send_text","args":{"session":$A,"text":"echo detached-leak"}}')
`$s.Write(`$b, 0, `$b.Length); `$c.Client.Shutdown(1)
[IO.File]::WriteAllText('$detOut', (New-Object IO.StreamReader(`$s)).ReadToEnd())
"@, (New-Object Text.UTF8Encoding $false))
  $detCmd = "powershell -NoProfile -Command ""Start-Process powershell -WindowStyle Hidden -ArgumentList '-NoProfile','-ExecutionPolicy','Bypass','-File','$detPs'"""
  [void](Tool 'run_command' @{ session = $K; command = $detCmd; timeout_secs = 30 })
  $deadline = (Get-Date).AddSeconds(25)
  while (-not (Test-Path $detOut) -and (Get-Date) -lt $deadline) { Start-Sleep -Milliseconds 300 }
  $det = if (Test-Path $detOut) { [IO.File]::ReadAllText($detOut) } else { '' }
  Check 'Kette: losgeloester Absender (Start-Process aus kurzlebiger Shell) -> forbidden in den Eltern-Agenten' ($det -match '"code":"forbidden"' -and $det -match 'cannot be attributed') $det
  Start-Sleep 1
  Check 'Kette: im Eltern-Agenten wurde nichts getippt' ((Tool 'read_terminal' @{ session = $A; lines = 200 }).data.text -notmatch 'kind-leak|raw-leak|detached-leak')
  Check 'Kette: von aussen (ohne Kette) bleibt Schreiben erlaubt' (-not (Tool 'send_text' @{ session = $A; text = ' '; submit = $false }).err)
  $cl = InA 'close_session' @{ session = $K }
  Check 'Eltern-Agent schliesst sein Kind' ($cl.isError -ne $true)
  [void](Tool 'close_session' @{ session = $A })

  Write-Host "== Claude-Sitzung (simulierte Hooks)" -ForegroundColor Cyan
  $tok = Tool 'create_session' @{ shell = 'powershell'; cwd = "$proj"; name = 'smoke-token' }; $psid = $tok.data.id; Start-Sleep 4
  $o = (Tool 'run_command' @{ session = $psid; command = '"$env:PANELDECK_PORT|$env:PANELDECK_TOKEN"'; timeout_secs = 25 }).data.output.Trim()
  $port, $hookTok = $o.Split('|')
  Check 'Hook-Zugang aus der Sitzung gelesen' ($port -match '^\d+$' -and $hookTok.Length -gt 20)
  # Fail closed: ohne bestaetigten Ersatz im PowerShell-Terminal des Hubs wird keine Claude-Sitzung angelegt.
  if (-not $script:psClaudeFake) { throw 'SICHERHEIT: claude im PowerShell-Terminal des Hubs nicht als Ersatz bestaetigt - keine Claude-Sitzung' }
  # Shell fest auf PowerShell (nie WSL: dort gaelte der Ersatz-PATH nicht).
  $cs = Tool 'create_session' @{ claude = $true; shell = 'powershell'; cwd = "$proj"; name = 'smoke-claude' }
  Check 'create_session claude (PowerShell, nicht WSL)' (-not $cs.err -and $cs.data.shell -notlike 'wsl*') $cs.text
  if ($script:lastOk) {
    $cid = $cs.data.id
    $ct = Wait-FakeFile $calls '--session-id'
    Check 'Claude-Start erreicht den Ersatz (fakebin\calls.txt: --session-id, Hooks per --settings aus dem Test-Datenordner)' ($ct -match 'claude\.cmd' -and $ct -match '--session-id [0-9a-f-]{36}' -and $ct -match ('--settings "?[^ ]*' + [regex]::Escape($tag) + '/hub/paneldeck-data/claude-hooks\.json')) "calls.txt='$ct'"
    Check 'Schein-Profil: keine globalen Hooks in ~/.claude/settings.json angelegt' (-not (Test-Path "$($script:iso.Profile)\.claude\settings.json"))
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
    Check 'queue_prompt kommt beim Ersatz-claude an (fakebin\input.txt: hallo)' ((Wait-FakeFile $inputs 'hallo' 10000) -match 'hallo') "input.txt='$(Wait-FakeFile $inputs 'hallo' 1)'"
    Send-Raw (@{ jsonrpc = '2.0'; id = 900; method = 'tools/call'; params = @{ name = 'wait_for_status'; arguments = @{ session = $cid; status = @('ready'); timeout_secs = 20 } } } | ConvertTo-Json -Depth 8 -Compress)
    Start-Sleep 2; Hook 'Stop'
    $wr = (Read-Line 15000).result.structuredContent
    Check 'asynchrones wait_for_status endet mit Stop-Hook' ($wr.matched -eq $true -and $wr.waited_ms -ge 1500)
    Hook 'Notification' @{ notification_type = 'permission_prompt'; message = 'darf ich?' }; Start-Sleep 0.5
    Check 'Freigabe-Frage: send_text verweigert' ((Code (Tool 'send_text' @{ session = $cid; text = 'ja' })) -eq 'permission_pending')
    [void](Tool 'close_session' @{ session = $cid })
  }
  [void](Tool 'close_session' @{ session = $psid })

  Write-Host "== Claude-Startoptionen (#161)" -ForegroundColor Cyan
  Check 'create_session: model ohne claude -> bad_args' ((Code (Tool 'create_session' @{ model = 'opus' })) -eq 'bad_args')
  Check 'create_session: effort ohne claude -> bad_args' ((Code (Tool 'create_session' @{ shell = 'cmd'; effort = 'high' })) -eq 'bad_args')
  $bp = Tool 'create_session' @{ claude = $true; cwd = "$proj"; permission_mode = 'bypassPermissions' }
  Check 'create_session: bypassPermissions -> bad_args' ((Code $bp) -eq 'bad_args' -and $bp.text -match 'never allowed via MCP') $bp.text
  Check 'create_session: ungueltiges Modell -> bad_args' ((Code (Tool 'create_session' @{ claude = $true; cwd = "$proj"; model = 'opus; calc' })) -eq 'bad_args')
  Check 'create_session: unbekannter effort -> bad_args' ((Code (Tool 'create_session' @{ claude = $true; cwd = "$proj"; effort = 'ultra' })) -eq 'bad_args')
  # Gestartet wird nur der Ersatz (fakebin, bestaetigt im PowerShell-Terminal); geprueft werden die Argumente, die er
  # bekommt (fakebin\calls.txt), nicht die angezeigte Zeile.
  if (-not $script:psClaudeFake) { throw 'SICHERHEIT: claude im PowerShell-Terminal des Hubs nicht als Ersatz bestaetigt - keine Claude-Sitzung' }
  $co = Tool 'create_session' @{ claude = $true; shell = 'powershell'; cwd = "$proj"; name = 'smoke-copts'; model = 'haiku'; effort = 'high'; permission_mode = 'plan'; agent = 'smoke-agent' }
  Check 'create_session mit model/effort/permission_mode/agent' (-not $co.err -and $co.data.model -eq 'haiku' -and $co.data.effort -eq 'high' -and $co.data.permission_mode -eq 'plan' -and $co.data.agent -eq 'smoke-agent') $co.text
  if ($script:lastOk) {
    $coid = $co.data.id
    $cl = (Tool 'list_sessions').data.sessions | Where-Object { $_.id -eq $coid }
    Check 'list_sessions meldet die Startoptionen' ($cl.claude -eq $true -and $cl.model -eq 'haiku' -and $cl.effort -eq 'high' -and $cl.permission_mode -eq 'plan' -and $cl.agent -eq 'smoke-agent') ($cl | ConvertTo-Json -Compress)
    $re = 'args=.*--model haiku --effort high --permission-mode plan --agent smoke-agent --session-id [0-9a-f-]{36}'
    $ct = Wait-FakeFile $calls $re
    Check 'Ersatz-claude bekommt die geprueften Optionen (fakebin\calls.txt)' ($ct -match $re) "calls.txt='$ct'"
    [void](Tool 'close_session' @{ session = $coid })
  }
  # Ohne permission_mode: ausdruecklich "default" (sonst gaelte permissions.defaultMode aus ~/.claude/settings.json).
  $cd = Tool 'create_session' @{ claude = $true; shell = 'powershell'; cwd = "$proj"; name = 'smoke-copts2'; model = 'sonnet' }
  Check 'create_session ohne permission_mode meldet "default"' (-not $cd.err -and $cd.data.permission_mode -eq 'default') $cd.text
  if ($script:lastOk) {
    $re2 = 'args=.*--model sonnet --permission-mode default --session-id'
    $ct2 = Wait-FakeFile $calls $re2
    Check 'Ersatz-claude bekommt --permission-mode default' ($ct2 -match $re2) "calls.txt='$ct2'"
    [void](Tool 'close_session' @{ session = $cd.data.id })
  }
  Check 'kein bypassPermissions in fakebin\calls.txt' ((Wait-FakeFile $calls 'bypassPermissions' 1) -notmatch 'bypassPermissions|dangerously')
  $pl = Tool 'create_session' @{ shell = 'cmd'; cwd = "$proj"; name = 'smoke-plain' }
  $plr = (Tool 'list_sessions').data.sessions | Where-Object { $_.id -eq $pl.data.id }
  Check 'list_sessions: ohne Claude sind die Optionen null' ($plr.PSObject.Properties.Name -contains 'effort' -and $null -eq $plr.model -and $null -eq $plr.effort -and $null -eq $plr.permission_mode -and $null -eq $plr.agent)
  [void](Tool 'close_session' @{ session = $pl.data.id })

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
  $t1 = Tool 'add_todo' @{ title = 'Milch kaufen'; text = '2 Liter'; tags = @('Haushalt', 'haushalt', 'Einkauf'); due = '20.11.2026 18:00' }
  Check 'add_todo legt eine Karte an (Tags automatisch, ohne Doppelte)' (-not $t1.err -and $t1.data.todo.key -eq 'TODO-1' -and $t1.data.todo.tags.Count -eq 2) $t1.text
  Check 'add_todo: Faelligkeit (deutsches Datum mit Uhrzeit) wird gespeichert und ausgewertet' ($t1.data.todo.due -eq '2026-11-20T18:00' -and $null -ne $t1.data.todo.due_state.minutes_left) $t1.text
  Check 'add_todo: unlesbare Faelligkeit -> bad_args' ((Code (Tool 'add_todo' @{ title = 'x'; due = 'irgendwann' })) -eq 'bad_args')
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
  Check 'Todo-Aufruf im Protokoll ohne Langtext' ((Get-Content "$data\mcp-log.jsonl" -Raw) -notmatch '2 Liter')

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
    $script:prestart = { $env:PYTHONUTF8 = '1'; & python -I $seedFile "$data\paneldeck.db"; if ($LASTEXITCODE -ne 0) { throw 'seed.py fehlgeschlagen' } }
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
    Check 'get_project: Typ, Felder, Workflow, Uebergaenge' (-not $gp.err -and @($task.fields).Count -eq 6 -and @($task.workflow.transitions).Count -eq 4 -and (($task.workflow.transitions | Where-Object { $_.to -eq 'Done' }).required_fields -contains 'Result'))
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
    $gsid = Tool 'get_sprint' @{ sprint = $ls.data.sprints[0].id }
    Check 'get_sprint: ueber die Kennung ohne project' (-not $gsid.err -and $gsid.data.sprint.name -eq 'Sprint 1') $gsid.text
    Check 'list_sprints: state-Filter und Seite hinter dem Ende' ((Tool 'list_sprints' @{ project = 'PAN'; state = 'planned' }).data.total -eq 0 -and (Tool 'list_sprints' @{ project = 'PAN'; limit = 1; offset = 5 }).data.sprints.Count -eq 0)
    Check 'list_sprints: Projekt ohne Sprints -> feature_disabled' ((Code (Tool 'list_sprints' @{ project = 'TODO' })) -eq 'feature_disabled')
    Check 'list_sprints: falscher state -> bad_args' ((Code (Tool 'list_sprints' @{ project = 'PAN'; state = 'x' })) -eq 'bad_args')
    $lv = Tool 'list_versions' @{ project = 'PAN' }
    Check 'list_versions: Summen und Meilenstein-Liste vorhanden' (-not $lv.err -and $lv.data.versions_total -ge 1 -and $null -ne $lv.data.milestones_total) $lv.text
    Check 'search_tickets: query mit Seite, total bleibt die Gesamtzahl' ((Tool 'search_tickets' @{ query = 'ORDER BY key'; limit = 1; offset = 1 }).data.tickets.Count -eq 1 -and (Tool 'search_tickets' @{ query = ''; limit = 1 }).data.total -ge 2)
    Check 'search_tickets: zu lange query -> too_large' ((Code (Tool 'search_tickets' @{ query = ('x' * 4001) })) -eq 'too_large')
    $uml = [string][char]0xD6
    $q3 = Tool 'search_tickets' @{ project = 'PAN'; query = ('summary ~ "' + $uml + $uml + '" AND nonsense = 1') }
    Check 'search_tickets: Fehlerposition zaehlt Zeichen, nicht Bytes (Umlaute)' ((Code $q3) -eq 'invalid_query' -and $q3.text -match 'Position 20' -and $q3.data.error.details.problems[0].position -eq 20) $q3.text
    $gt = Tool 'get_ticket' @{ ticket = 'PAN-1' }
    Check 'get_ticket: Kommentar, Zeit, Verknuepfung, Verlauf' (-not $gt.err -and $gt.data.comments_total -eq 1 -and $gt.data.time.entries_total -eq 2 -and @($gt.data.links).Count -eq 1 -and @($gt.data.history | Where-Object source -eq 'agent').Count -ge 5)
    Check 'get_ticket: Hinweis auf nicht vertrauenswuerdige Texte' ($gt.data.note -match 'untrusted')
    Check 'Ticket-Aufruf im Protokoll ohne Kommentartext' ((Get-Content "$data\mcp-log.jsonl" -Raw) -notmatch 'GEHEIM-KOMMENTAR')
    Check 'get_project: eingebaute Felder Beschreibung, Faelligkeit und Labels an jedem Typ' (@($task.fields | Where-Object { $_.key -eq 'description' -or $_.key -eq 'due' -or $_.key -eq 'labels' }).Count -eq 3)
    $dd = Tool 'create_ticket' @{ project = 'PAN'; type = 'Note'; title = 'Mit Termin'; description = 'Erst **lesen**'; due = '2026-11-20 14:30' }
    Check 'create_ticket: Beschreibung und Faelligkeit (Datum mit Uhrzeit)' (-not $dd.err -and $dd.data.ticket.due -eq '2026-11-20T14:30' -and $dd.data.ticket.description -eq 'Erst **lesen**' -and $null -ne $dd.data.ticket.due_state) $dd.text
    $ddk = $dd.data.ticket.key
    $gd = Tool 'get_ticket' @{ ticket = $ddk }
    Check 'get_ticket: Faelligkeit lesen (auch in den Feldern)' ($gd.data.due -eq '2026-11-20T14:30' -and @($gd.data.fields | Where-Object { $_.key -eq 'due' -and $_.value -eq '2026-11-20T14:30' }).Count -eq 1) $gd.text
    $ud = Tool 'update_ticket' @{ ticket = $ddk; due = '2026-11-21' }
    Check 'update_ticket: nur Datum (Ende des Tages)' (-not $ud.err -and $ud.data.ticket.due -eq '2026-11-21') $ud.text
    Check 'update_ticket: unlesbare Faelligkeit -> invalid_value' ((Code (Tool 'update_ticket' @{ ticket = $ddk; due = 'morgen' })) -eq 'invalid_value')
    $sq = Tool 'search_tickets' @{ project = 'PAN'; query = 'due = 2026-11-21 AND description ~ lesen' }
    Check 'search_tickets: due und description in der Abfrage' (-not $sq.err -and $sq.data.total -eq 1 -and $sq.data.tickets[0].key -eq $ddk) $sq.text
    $ul = Tool 'update_ticket' @{ ticket = $ddk; labels = @('smoke', 'Smoke', 'mcp') }
    Check 'update_ticket: eingebaute Labels (ohne Doppelte)' (-not $ul.err -and @($ul.data.ticket.labels).Count -eq 2) $ul.text
    Check 'search_tickets: labels in der Abfrage' ((Tool 'search_tickets' @{ project = 'PAN'; query = 'labels = mcp' }).data.total -eq 1)
    $cl = Tool 'update_ticket' @{ ticket = $ddk; due = $null }
    Check 'update_ticket: due null leert die Faelligkeit' (-not $cl.err -and $null -eq $cl.data.ticket.due) $cl.text

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
    Check 'Expert + Nur lesen: get_sprint und list_versions lesen' ((Tool 'get_sprint' @{ project = 'PAN'; sprint = 'Sprint 1' }).data.sprint.state -eq 'active' -and (Tool 'list_versions' @{ project = 'PAN' }).data.versions_total -ge 1)
  }

  Write-Host "== Systemprompt (initialize -> instructions)" -ForegroundColor Cyan
  Set-Level 'Read'
  $i = Rpc 'initialize' @{ protocolVersion = '2025-06-18'; capabilities = @{}; clientInfo = @{ name = 'smoke'; version = '1' } }
  $ins = [string]$i.result.instructions
  Check 'initialize: instructions nennt whoami und Werkzeuge der Stufe Nur lesen' ($ins -match 'whoami' -and $ins.Contains('- `list_sessions` (') -and -not $ins.Contains('- `send_text` (') -and $ins -match 'Read only') ($ins.Substring(0, [Math]::Min(200, $ins.Length)))
  $ver = [string]$i.result.serverInfo.version
  $spFile = "$data\system-prompt.md"
  [IO.File]::WriteAllText($spFile, 'Eigener Prompt {{version}} / {{level}} / {{gibtsnicht}}', (New-Object Text.UTF8Encoding $true))
  try { $script:bridge.StandardInput.Close(); [void]$script:bridge.WaitForExit(2000) } catch {}
  if (-not $script:bridge.HasExited) { $script:bridge.Kill() }
  Start-Bridge
  $i2 = Rpc 'initialize' @{ protocolVersion = '2025-06-18'; capabilities = @{}; clientInfo = @{ name = 'smoke'; version = '1' } }
  Check 'initialize: eigene system-prompt.md (mit BOM) gerendert, unbekannter Platzhalter bleibt' ([string]$i2.result.instructions -eq "Eigener Prompt $ver / Read only / {{gibtsnicht}}") ([string]$i2.result.instructions)
  Remove-Item -LiteralPath $spFile -ErrorAction SilentlyContinue

  Write-Host "== Kommandozeile: --version, --help, Fehler, Datenordner, Profile, PANELDECK_DATA" -ForegroundColor Cyan
  $script:prestart = $null
  # Ausgabe ueber umgeleitete Pipes (die GUI-.exe schreibt dann dorthin statt in eine Konsole oder ein Meldungsfenster).
  function Run-Exe([string]$argline, $envs = @{}) {
    $psi = New-Object System.Diagnostics.ProcessStartInfo
    $psi.FileName = "$dir\paneldeck.exe"; $psi.Arguments = $argline
    $psi.RedirectStandardOutput = $true; $psi.RedirectStandardError = $true; $psi.UseShellExecute = $false; $psi.CreateNoWindow = $true
    $psi.StandardOutputEncoding = [Text.Encoding]::UTF8; $psi.StandardErrorEncoding = [Text.Encoding]::UTF8
    foreach ($k in $envs.Keys) { $psi.EnvironmentVariables[$k] = $envs[$k] }
    $p = [System.Diagnostics.Process]::Start($psi)
    $o = $p.StandardOutput.ReadToEndAsync(); $e = $p.StandardError.ReadToEndAsync()
    if (-not $p.WaitForExit(20000)) { $p.Kill(); return [pscustomobject]@{ code = -1; out = ''; err = 'TIMEOUT' } }
    [pscustomobject]@{ code = $p.ExitCode; out = $o.Result; err = $e.Result }
  }
  # Weitere Hub-Instanz (minimiert) mit eigenen Argumenten; wartet auf ipc.json im erwarteten Datenordner.
  function Start-ExtraHub([string]$argline, [string]$dataDir) {
    Remove-Item -LiteralPath "$dataDir\ipc.json" -ErrorAction SilentlyContinue
    $h = if ($argline) { Start-Process "$dir\paneldeck.exe" -ArgumentList $argline -PassThru -WindowStyle Minimized } else { Start-Process "$dir\paneldeck.exe" -PassThru -WindowStyle Minimized }
    $script:extra += $h
    $t0 = Get-Date
    while (-not (Test-Path "$dataDir\ipc.json") -and ((Get-Date) - $t0).TotalSeconds -lt 20) { Start-Sleep -Milliseconds 200 }
    Start-Sleep 2
    $h
  }
  # Eine Bruecke mit Argumenten/Umgebung starten, list_sessions aufrufen und den Fehlercode liefern
  # (Stufe Aus: 'mcp_disabled' = der richtige Hub wurde erreicht; 'hub_not_running' = keiner gefunden).
  function Probe-Bridge([string]$argline, $envs = @{}) {
    $keep = $script:bridge
    $psi = New-Object System.Diagnostics.ProcessStartInfo
    $psi.FileName = "$dir\paneldeck.exe"; $psi.Arguments = $argline
    $psi.RedirectStandardInput = $true; $psi.RedirectStandardOutput = $true; $psi.RedirectStandardError = $true
    $psi.UseShellExecute = $false; $psi.CreateNoWindow = $true; $psi.StandardOutputEncoding = [Text.Encoding]::UTF8
    foreach ($k in $envs.Keys) { $psi.EnvironmentVariables[$k] = $envs[$k] }
    $script:bridge = [System.Diagnostics.Process]::Start($psi); $script:rpc = 0
    try {
      [void](Rpc 'initialize' @{ protocolVersion = '2025-06-18'; capabilities = @{}; clientInfo = @{ name = 'smoke'; version = '1' } })
      $r = Tool 'list_sessions'
      [pscustomobject]@{ code = (Code $r); text = $r.text }
    } finally {
      try { $script:bridge.StandardInput.Close(); [void]$script:bridge.WaitForExit(3000) } catch {}
      if (-not $script:bridge.HasExited) { $script:bridge.Kill() }
      $script:bridge = $keep; $script:rpc = 0
    }
  }

  $v = Run-Exe '--version'
  Check '--version: Exit-Code 0 und Versionsnummer' ($v.code -eq 0 -and $v.out -match '^Paneldeck \d+\.\d+\.\d+') "$($v.code) $($v.out)"
  $hl = Run-Exe '--help'
  Check '--help: Exit-Code 0, englisch, alle Optionen' ($hl.code -eq 0 -and $hl.out -match 'Usage:' -and $hl.out -match '--data-dir' -and $hl.out -match '--profile' -and $hl.out -match 'PANELDECK_DATA' -and $hl.out -match '--reset-settings') "$($hl.code)"
  $hd = Run-Exe '-? --lang de'
  Check '-? --lang de: deutsche Hilfe mit Umlauten (UTF-8)' ($hd.code -eq 0 -and $hd.out -match 'Aufruf:' -and $hd.out -match ('Einstellungen zur' + [char]0xFC + 'cksetzen')) "$($hd.code)"
  $u = Run-Exe '--gibts-nicht'
  Check 'unbekannte Option: Exit-Code 2, Meldung auf stderr' ($u.code -eq 2 -and $u.err -match '--gibts-nicht' -and $u.err -match '--help') "$($u.code) $($u.err)"
  Check 'fehlender Wert: Exit-Code 2' ((Run-Exe '--data-dir').code -eq 2)
  Check 'ungueltiger Profilname: Exit-Code 2' ((Run-Exe '--profile "a b"').code -eq 2)
  Check '--lang xx: Exit-Code 2' ((Run-Exe '--lang xx').code -eq 2)
  Check 'mcp mit Start-Option: Exit-Code 2' ((Run-Exe 'mcp --tray').code -eq 2)
  [IO.File]::WriteAllText("$work\blocker", 'x')
  $bad = Run-Exe "--data-dir `"$work\blocker\sub`""
  Check 'unbrauchbares --data-dir: Exit-Code 1, Ordner in der Meldung (kein Rueckfall)' ($bad.code -eq 1 -and $bad.err -match 'blocker') "$($bad.code) $($bad.err)"
  $bb = Run-Exe "--data-dir `"$work\bb`" --backup-dir `"$work\blocker\bk`""
  Check 'unbrauchbares --backup-dir: Exit-Code 1' ($bb.code -eq 1 -and $bb.err -match 'backup') "$($bb.code) $($bb.err)"
  $ue = Run-Exe '--gibts-nicht --lang de'
  Check 'Fehlermeldung beachtet --lang trotz Fehler' ($ue.code -eq 2 -and $ue.err -match 'Unbekannte Option') $ue.err
  $me = Run-Exe 'mcp --tray'
  Check 'mcp-Fehler nur auf stderr (stdout bleibt leer)' ($me.code -eq 2 -and $me.out -eq '' -and $me.err -match '--tray') "$($me.out)|$($me.err)"

  # Standard-Hub (Stufe Aus) + Hub mit --data-dir + Hub mit --profile p1 + Hub mit PANELDECK_DATA, alle gleichzeitig
  Set-Level 'Off'
  $dd = "$work\dd"
  [void](Start-ExtraHub "--data-dir `"$dd`"" $dd)
  Check 'Hub mit --data-dir legt ipc.json im angegebenen Ordner an' (Test-Path "$dd\ipc.json")
  Check 'mcp --data-dir findet diesen Hub' ((Probe-Bridge "mcp --data-dir `"$dd`"").code -eq 'mcp_disabled')
  Check 'PANELDECK_HUB_DATA (Terminal eines Hubs) findet diesen Hub' ((Probe-Bridge 'mcp' @{ PANELDECK_HUB_DATA = $dd; PANELDECK_HUB_BASE = $dd }).code -eq 'mcp_disabled')
  $hpq = Probe-Bridge 'mcp --profile q' @{ PANELDECK_HUB_DATA = $dd; PANELDECK_HUB_BASE = $dd }
  Check 'mcp --profile im Hub-Terminal: Profil unter der Basis dieses Hubs' ($hpq.code -eq 'hub_not_running' -and $hpq.text -match [regex]::Escape("$dd\profiles\q")) $hpq.text
  $nf = Probe-Bridge "mcp --data-dir `"$work\leer`""
  Check 'mcp mit leerem Ordner: hub_not_running nennt den Ordner' ($nf.code -eq 'hub_not_running' -and $nf.text -match [regex]::Escape("$work\leer")) $nf.text
  $p1 = "$data\profiles\p1"
  $hp = Start-ExtraHub '--profile p1' $p1
  Check 'Hub mit --profile p1 laeuft parallel zum Standard-Hub' ((Test-Path "$p1\ipc.json") -and -not $hp.HasExited -and -not $script:hub.HasExited)
  $port0 = ([IO.File]::ReadAllText("$data\ipc.json") | ConvertFrom-Json).port; $port1 = ([IO.File]::ReadAllText("$p1\ipc.json") | ConvertFrom-Json).port
  Check 'Standard-Hub und Profil-Hub haben verschiedene Ports' ($port0 -and $port1 -and $port0 -ne $port1) "$port0 / $port1"
  Check 'mcp --profile p1 findet den Profil-Hub' ((Probe-Bridge 'mcp --profile p1').code -eq 'mcp_disabled')
  Check 'mcp ohne Angabe findet weiter den Standard-Hub' ((Code (Tool 'list_sessions')) -eq 'mcp_disabled')
  $ed = "$work\envdata"
  $env:PANELDECK_DATA = $ed
  try { [void](Start-ExtraHub '' $ed) } finally { Remove-Item Env:PANELDECK_DATA -ErrorAction SilentlyContinue }
  Check 'PANELDECK_DATA: Hub nutzt den Ordner aus der Umgebung' (Test-Path "$ed\ipc.json")
  Check 'PANELDECK_DATA: mcp findet diesen Hub' ((Probe-Bridge 'mcp' @{ PANELDECK_DATA = $ed }).code -eq 'mcp_disabled')
  Check '--data-dir schlaegt PANELDECK_DATA' ((Probe-Bridge "mcp --data-dir `"$work\leer`"" @{ PANELDECK_DATA = $ed }).code -eq 'hub_not_running')

  # --reset-settings: settings.json wird gesichert, Standardwerte, Zustand bleibt
  $rs = "$work\rs"; New-Item -ItemType Directory -Force $rs | Out-Null
  [IO.File]::WriteAllText("$rs\settings.json", '{"language":"De","mcp_level":"Read"}', (New-Object Text.UTF8Encoding $false))
  [IO.File]::WriteAllText("$rs\state.json", '{}', (New-Object Text.UTF8Encoding $false))
  [void](Start-ExtraHub "--data-dir `"$rs`" --reset-settings --no-tray" $rs)
  $bak = @(Get-ChildItem -LiteralPath $rs -Filter 'settings.json.bak-*')
  Check '--reset-settings: Sicherung settings.json.bak-<Zeit> mit altem Inhalt' ($bak.Count -eq 1 -and $bak[0].Name -match '^settings\.json\.bak-\d{8}-\d{6}$' -and ([IO.File]::ReadAllText($bak[0].FullName) -match 'Read')) (($bak | ForEach-Object Name) -join ',')
  Check '--reset-settings: state.json bleibt' (Test-Path "$rs\state.json")
  Check '--reset-settings bei laufendem Hub: Exit-Code 1' ((Run-Exe "--data-dir `"$rs`" --reset-settings").code -eq 1)
  foreach ($h in $script:extra) { if (-not $h.HasExited) { $h.Kill() } }
  Start-Sleep -Milliseconds 800
  $after = if (Test-Path "$rs\settings.json") { [IO.File]::ReadAllText("$rs\settings.json") } else { '' }
  Check '--reset-settings: keine alte Stufe in settings.json' ($after -notmatch '"mcp_level":\s*"Read"') $after

  Write-Host "== Weitere Fenster (#110): Sitzung in Fenster 2, Kind eines Agenten dort" -ForegroundColor Cyan
  Stop-Hub
  $st = [ordered]@{
    sessions = @([ordered]@{ id = 900; name = 'Fenster2-Agent'; custom_name = $true; shell_id = 'powershell'; cwd = "$proj"; project = 1; window = 2 })
    next_id = 901; projects = @([ordered]@{ id = 1; name = 'Standard' }); active_project = 1
    extra_windows = @([ordered]@{ id = 2; tab = 'terminals'; project = 1; focused = 900 })
  }
  [IO.File]::WriteAllText("$data\state.json", ($st | ConvertTo-Json -Depth 8), (New-Object Text.UTF8Encoding $false))
  Set-Level 'Control' -Tray
  [void](Rpc 'initialize' @{ protocolVersion = '2025-06-18'; capabilities = @{}; clientInfo = @{ name = 'smoke'; version = '1' } })
  Start-Sleep 4
  $w2 = ((Tool 'list_sessions').data.sessions | Where-Object { $_.id -eq 900 })
  Check 'list_sessions: wiederhergestellte Sitzung in Fenster 2 (window = 2)' ($w2.window -eq 2) ($w2 | ConvertTo-Json -Compress)
  $A = 900
  $kid2 = InA 'create_session' @{ shell = 'powershell'; cwd = "$proj"; name = 'Kind-F2' }; $K2 = $kid2.structuredContent.id
  Check 'create_session aus einer Sitzung in Fenster 2 angelegt' ($K2 -gt 0 -and $kid2.structuredContent.parent -eq 900) ($kid2 | ConvertTo-Json -Compress -Depth 5)
  $k2 = ((Tool 'list_sessions').data.sessions | Where-Object { $_.id -eq $K2 })
  Check 'Kind-Sitzung liegt im Fenster des Eltern-Agenten (window = 2)' ($k2.window -eq 2) ($k2 | ConvertTo-Json -Compress)
  $kx = Tool 'create_session' @{ shell = 'powershell'; cwd = "$proj"; name = 'Von-aussen-F' }
  Check 'create_session von aussen: Hauptfenster (window = 0)' (((Tool 'list_sessions').data.sessions | Where-Object { $_.id -eq $kx.data.id }).window -eq 0)
  [void](InA 'close_session' @{ session = $K2 }); [void](Tool 'close_session' @{ session = $kx.data.id })

  Write-Host "== Chat-Sitzung (#63): Wiederherstellung aus state.json, kind/model, Terminal-Werkzeuge" -ForegroundColor Cyan
  Stop-Hub
  $st = [ordered]@{
    sessions = @(
      [ordered]@{ id = 950; name = 'Smoke-Chat'; custom_name = $true; cwd = "$proj"; project = 1; kind = 'chat'; chat = [ordered]@{ server = 'echo'; model = 'smoke-model' } },
      [ordered]@{ id = 951; name = 'Smoke-Term'; custom_name = $true; shell_id = 'powershell'; cwd = "$proj"; project = 1 },
      [ordered]@{ id = 952; name = 'Smoke-Big'; custom_name = $true; cwd = "$proj"; project = 1; kind = 'chat'; chat = [ordered]@{ server = 'echo'; model = 'smoke-model' } })
    next_id = 953; projects = @([ordered]@{ id = 1; name = 'Standard' }); active_project = 1
  }
  New-Item -ItemType Directory -Force "$data\chats" | Out-Null
  $hist = [ordered]@{ version = 1; messages = @([ordered]@{ role = 'user'; content = 'hi'; ts = 1 }, [ordered]@{ role = 'assistant'; content = 'smoke-answer'; ts = 2 }) }
  [IO.File]::WriteAllText("$data\chats\950.json", ($hist | ConvertTo-Json -Depth 8), (New-Object Text.UTF8Encoding $false))
  # Grosser Verlauf (40 Nachrichten zu je ~1,5 KB) fuer read_chat ueber ein Chat-Modell: die neuesten muessen ankommen.
  $bigMsgs = @(0..39 | ForEach-Object { [ordered]@{ role = $(if ($_ % 2) { 'assistant' } else { 'user' }); content = ("big-{0:D2} " -f $_) + ('w' * 1500); ts = 10 + $_ } })
  [IO.File]::WriteAllText("$data\chats\952.json", ([ordered]@{ version = 1; messages = $bigMsgs } | ConvertTo-Json -Depth 8), (New-Object Text.UTF8Encoding $false))
  [IO.File]::WriteAllText("$data\state.json", ($st | ConvertTo-Json -Depth 8), (New-Object Text.UTF8Encoding $false))
  Set-Level 'Control' -Tray
  [void](Rpc 'initialize' @{ protocolVersion = '2025-06-18'; capabilities = @{}; clientInfo = @{ name = 'smoke'; version = '1' } })
  Start-Sleep 3
  $ss = @((Tool 'list_sessions').data.sessions)
  $ch = $ss | Where-Object { $_.id -eq 950 }; $te = $ss | Where-Object { $_.id -eq 951 }
  Check 'list_sessions: Chat aus state.json wiederhergestellt (kind = chat, model)' ($ch.kind -eq 'chat' -and $ch.model -eq 'smoke-model') ($ch | ConvertTo-Json -Compress)
  Check 'list_sessions: wiederhergestelltes Terminal hat kind = terminal' ($te.kind -eq 'terminal' -and $null -eq $te.model) ($te | ConvertTo-Json -Compress)
  Check 'read_terminal auf einem Chat -> not_a_terminal' ((Code (Tool 'read_terminal' @{ session = 950 })) -eq 'not_a_terminal')
  Check 'send_text auf einem Chat -> not_a_terminal' ((Code (Tool 'send_text' @{ session = 950; text = 'x' })) -eq 'not_a_terminal')
  Check 'Chatverlauf chats/950.json bleibt beim Start erhalten' ([IO.File]::ReadAllText("$data\chats\950.json") -match 'smoke-answer')

  Write-Host "== Chats ueber MCP (#63): Fake-Ollama-Server, create_session kind=chat, send_chat_message, read_chat" -ForegroundColor Cyan
  Stop-Hub
  # Kleiner Ollama-Ersatz auf 127.0.0.1 (TcpListener, eigener Runspace): /api/tags, /api/show, /api/chat (NDJSON).
  # /api/chat: Antwort "pong"; "slow" in der letzten Nutzer-Nachricht -> 3 s Pause; "tools please" mit tools-Liste ->
  # Werkzeugaufruf list_sessions; "proxy please <id>" -> send_text in Sitzung <id>; "chain please <chat> <id>" ->
  # send_chat_message an <chat> mit "proxy please <id>"; "read please <chat>" -> read_chat (last 40);
  # nach einer tool-Nachricht -> "done". Jede Anfrage landet in $script:fakeLog.
  $fake = {
    param($listener, $log)
    $enc = New-Object Text.UTF8Encoding $false
    while ($true) {
      # Pending() statt eines blockierenden Accept: nach $listener.Stop() wirft es, und die Schleife endet sofort.
      try { if (-not $listener.Pending()) { Start-Sleep -Milliseconds 50; continue }; $client = $listener.AcceptTcpClient() } catch { break }
      try {
        $st = $client.GetStream(); $st.ReadTimeout = 5000
        $buf = New-Object byte[] 65536; $ms = New-Object IO.MemoryStream; $he = -1
        while ($he -lt 0) {
          $n = $st.Read($buf, 0, $buf.Length); if ($n -le 0) { break }
          $ms.Write($buf, 0, $n); $he = [Text.Encoding]::ASCII.GetString($ms.ToArray()).IndexOf("`r`n`r`n")
        }
        if ($he -lt 0) { $client.Close(); continue }
        $all = $ms.ToArray(); $head = [Text.Encoding]::ASCII.GetString($all, 0, $he)
        $len = 0; if ($head -match '(?im)^content-length:\s*(\d+)') { $len = [int]$Matches[1] }
        while ($all.Length - ($he + 4) -lt $len) { $n = $st.Read($buf, 0, $buf.Length); if ($n -le 0) { break }; $ms.Write($buf, 0, $n); $all = $ms.ToArray() }
        $body = $enc.GetString($all, $he + 4, $all.Length - $he - 4)
        $path = ($head -split ' ')[1]
        [void]$log.Add("$path $body")
        $type = 'application/json'; $status = '200 OK'
        if ($path -eq '/api/tags') { $out = '{"models":[{"name":"fake-model"}]}' }
        elseif ($path -eq '/api/show') { $out = '{"capabilities":["completion","tools"],"model_info":{"general.architecture":"llama","llama.context_length":8192}}' }
        elseif ($path -eq '/api/chat') {
          $j = $null; try { $j = $body | ConvertFrom-Json } catch {}
          $msgs = @(); if ($j) { $msgs = @($j.messages) }
          $lu = -1; for ($i = 0; $i -lt $msgs.Count; $i++) { if ($msgs[$i].role -eq 'user') { $lu = $i } }
          $last = if ($lu -ge 0) { [string]$msgs[$lu].content } else { '' }
          $toolAfter = $false; for ($i = $lu + 1; $i -lt $msgs.Count; $i++) { if ($msgs[$i].role -eq 'tool') { $toolAfter = $true } }
          $hasTools = $j -and $j.PSObject.Properties.Name -contains 'tools' -and @($j.tools).Count -gt 0
          $done = '{"model":"fake-model","message":{"role":"assistant","content":""},"done":true,"done_reason":"stop","prompt_eval_count":5,"eval_count":1}'
          if ($toolAfter) { $first = '{"model":"fake-model","message":{"role":"assistant","content":"done"},"done":false}' }
          elseif ($hasTools -and $last -match 'chain please (\d+) (\d+)') { $first = '{"model":"fake-model","message":{"role":"assistant","content":"","tool_calls":[{"function":{"name":"send_chat_message","arguments":{"session":' + $Matches[1] + ',"text":"proxy please ' + $Matches[2] + '"}}}]},"done":false}' }
          elseif ($hasTools -and $last -match 'read please (\d+)') { $first = '{"model":"fake-model","message":{"role":"assistant","content":"","tool_calls":[{"function":{"name":"read_chat","arguments":{"session":' + $Matches[1] + ',"last":40}}}]},"done":false}' }
          elseif ($hasTools -and $last -match 'spawn please') { $first = '{"model":"fake-model","message":{"role":"assistant","content":"","tool_calls":[{"function":{"name":"create_session","arguments":{"shell":"powershell","name":"Chat-Kind"}}}]},"done":false}' }
          elseif ($hasTools -and $last -match 'proxy please (\d+)') { $first = '{"model":"fake-model","message":{"role":"assistant","content":"","tool_calls":[{"function":{"name":"send_text","arguments":{"session":' + $Matches[1] + ',"text":"echo proxy-leak"}}}]},"done":false}' }
          elseif ($hasTools -and $last -match 'tools please') { $first = '{"model":"fake-model","message":{"role":"assistant","content":"","tool_calls":[{"function":{"name":"list_sessions","arguments":{}}}]},"done":false}' }
          else {
            if ($last -match 'slow') { Start-Sleep -Seconds 3 }
            $first = '{"model":"fake-model","message":{"role":"assistant","content":"pong"},"done":false}'
          }
          $out = "$first`n$done`n"; $type = 'application/x-ndjson'
        } else { $status = '404 Not Found'; $out = '{"error":"not found"}' }
        $ob = $enc.GetBytes($out)
        $hb = [Text.Encoding]::ASCII.GetBytes("HTTP/1.1 $status`r`nContent-Type: $type`r`nContent-Length: $($ob.Length)`r`nConnection: close`r`n`r`n")
        $st.Write($hb, 0, $hb.Length); $st.Write($ob, 0, $ob.Length); $st.Flush()
      } catch {} finally { $client.Close() }
    }
  }
  $script:fakeListener = New-Object Net.Sockets.TcpListener([Net.IPAddress]::Loopback, 0); $script:fakeListener.Start()
  $fakePort = $script:fakeListener.LocalEndpoint.Port
  $script:fakeLog = [Collections.ArrayList]::Synchronized((New-Object Collections.ArrayList))
  $script:fakePs = [powershell]::Create(); [void]$script:fakePs.AddScript($fake).AddArgument($script:fakeListener).AddArgument($script:fakeLog)
  $script:fakeAsync = $script:fakePs.BeginInvoke()
  # Server in settings.json eintragen (vor jedem Start; als Text, damit ConvertTo-Json von PowerShell 5.1 nichts verbiegt).
  $srvJson = '{"id":"srv-smoke","name":"Fake","kind":"ollama","url":"http://127.0.0.1:' + $fakePort + '","models":[{"name":"fake-model","level":"Control","level_chosen":true,"ask_write":false,"tools":true}],"last_models":[{"name":"fake-model","tools":true,"context_length":131072},{"name":"fake-raw","tools":true,"context_length":131072}]}'
  $script:prestart = {
    $f = "$data\settings.json"
    $j = [IO.File]::ReadAllText($f).TrimStart([char]0xFEFF) | ConvertFrom-Json
    foreach ($p in @('llm_servers', 'last_chat_model')) { if ($j.PSObject.Properties.Name -contains $p) { $j.PSObject.Properties.Remove($p) } }
    $t = ($j | ConvertTo-Json -Depth 10) -replace '^\s*\{', ('{"llm_servers":[' + $srvJson + '],')
    [IO.File]::WriteAllText($f, $t, (New-Object Text.UTF8Encoding $false))
  }
  Set-Level 'Control' -Tray
  [void](Rpc 'initialize' @{ protocolVersion = '2025-06-18'; capabilities = @{}; clientInfo = @{ name = 'smoke'; version = '1' } })
  Start-Sleep 2
  $names = @((Rpc 'tools/list').result.tools | ForEach-Object name)
  Check 'tools/list (Steuern): read_chat, send_chat_message, list_models' ($names -contains 'read_chat' -and $names -contains 'send_chat_message' -and $names -contains 'list_models')
  $lm = Tool 'list_models'
  $fs = @($lm.data.servers) | Where-Object { $_.id -eq 'srv-smoke' }
  Check 'list_models: Fake-Server mit Modell und Host:Port, ohne Schluessel und Adresse' ($fs.name -eq 'Fake' -and $fs.host -eq "127.0.0.1:$fakePort" -and @($fs.models).Count -eq 2 -and $fs.models[0].name -eq 'fake-model' -and $fs.models[0].level -eq 'Control' -and $fs.models[0].level_chosen -eq $true -and $fs.models[1].level_chosen -eq $false -and $lm.text -notmatch 'http://' -and $lm.text -notmatch 'api_key') $lm.text
  $bad = Tool 'create_session' @{ kind = 'chat'; server = 'Fake'; model = 'gibts-nicht' }
  Check 'create_session kind=chat, unbekanntes Modell -> bad_args nennt die Modelle' ((Code $bad) -eq 'bad_args' -and $bad.text -match 'fake-model' -and $bad.text -notmatch 'http://') $bad.text
  $nm = Tool 'create_session' @{ kind = 'chat'; model = 'gibts-nicht' }
  Check 'create_session kind=chat, Modell, das kein Server hat -> bad_args nennt die Server' ((Code $nm) -eq 'bad_args' -and $nm.text -match 'No configured server' -and $nm.text -match 'Fake') $nm.text
  Check 'create_session kind=chat, unbekannter Server -> bad_args' ((Code (Tool 'create_session' @{ kind = 'chat'; server = 'Nirgends' })) -eq 'bad_args')
  Check 'create_session kind=chat mit shell -> bad_args' ((Code (Tool 'create_session' @{ kind = 'chat'; shell = 'powershell' })) -eq 'bad_args')
  $f0 = (Tool 'list_sessions').data.focused
  $cc = Tool 'create_session' @{ kind = 'chat'; server = 'Fake'; model = 'fake-model'; name = 'Smoke-LLM'; cwd = "$proj" }
  Check 'create_session kind=chat' (-not $cc.err -and $cc.data.kind -eq 'chat' -and $cc.data.model -eq 'fake-model' -and $cc.data.server -eq 'srv-smoke') $cc.text
  $C = $cc.data.id
  $lsc = Tool 'list_sessions'
  $lc = ($lsc.data.sessions | Where-Object { $_.id -eq $C })
  Check 'create_session kind=chat ist leise: Fokus unveraendert, nicht minimiert' ($lsc.data.focused -eq $f0 -and $lc.minimized -eq $false -and $lc.focused -eq $false) ($lc | ConvertTo-Json -Compress)
  Check 'list_sessions: neuer Chat (kind = chat, model, vom Agenten angelegt, ready)' ($lc.kind -eq 'chat' -and $lc.model -eq 'fake-model' -and $lc.created_by_agent -eq $true -and $lc.status -eq 'ready') ($lc | ConvertTo-Json -Compress)
  $sm = Tool 'send_chat_message' @{ session = $C; text = 'ping' }
  Check 'send_chat_message: ok mit Index der Nachricht' (-not $sm.err -and $sm.data.ok -eq $true -and $sm.data.index -eq 0) $sm.text
  $w = Tool 'wait_for_status' @{ session = $C; status = @('ready'); timeout_secs = 20 } 30000
  Check 'wait_for_status auf einem Chat: ready' ($w.data.matched -eq $true -and $w.data.status -eq 'ready') $w.text
  $rc = Tool 'read_chat' @{ session = $C }
  $m = @($rc.data.messages)
  Check 'read_chat: Nutzer "ping", Antwort "pong"' ($rc.data.total -eq 2 -and $m[0].role -eq 'user' -and $m[0].content -eq 'ping' -and $m[1].role -eq 'assistant' -and $m[1].content -eq 'pong') $rc.text
  Check 'read_chat: Status ready, Inhalt als nicht vertrauenswuerdig markiert' ($rc.data.status -eq 'ready' -and $rc.data.note -match 'untrusted')
  Check 'send_chat_message: Nachricht als vom Agenten gesendet markiert' ($m[0].note -eq 'Sent by agent external agent') ($m[0] | ConvertTo-Json -Compress)
  Check 'Fake-Server bekam /api/chat mit Systemprompt und "ping"' (@($script:fakeLog | Where-Object { $_ -like '/api/chat *' -and $_ -match '"system"' -and $_ -match 'ping' }).Count -ge 1) (($script:fakeLog | Select-Object -Last 1) -replace '(.{300}).*', '$1')
  $s1 = Tool 'send_chat_message' @{ session = $C; text = 'slow ping' }
  $s2 = Tool 'send_chat_message' @{ session = $C; text = 'zu frueh' }
  Check 'send_chat_message waehrend einer laufenden Antwort -> busy' (-not $s1.err -and (Code $s2) -eq 'busy') "$($s1.text) / $($s2.text)"
  Check 'read_chat waehrend der Antwort: working' ((Tool 'read_chat' @{ session = $C; last = 1 }).data.status -eq 'working')
  $w = Tool 'wait_for_status' @{ session = $C; status = @('ready'); timeout_secs = 20 } 30000
  $rc = Tool 'read_chat' @{ session = $C; last = 2 }
  Check 'nach dem Warten: Antwort auf "slow ping" da, "zu frueh" nicht gesendet' ($w.data.matched -eq $true -and $rc.data.total -eq 4 -and $rc.data.messages[0].content -eq 'slow ping' -and $rc.data.messages[1].content -eq 'pong' -and $rc.data.more_above -eq $true) $rc.text
  $term = ((Tool 'list_sessions').data.sessions | Where-Object { $_.kind -eq 'terminal' } | Select-Object -First 1).id
  if (-not $term) { $term = (Tool 'create_session' @{ shell = 'powershell'; cwd = "$proj"; name = 'smoke-term-chat' }).data.id; Start-Sleep 3 }
  Check 'read_chat auf einem Terminal -> not_a_chat' ((Code (Tool 'read_chat' @{ session = $term })) -eq 'not_a_chat')
  Check 'send_chat_message an ein Terminal -> not_a_chat' ((Code (Tool 'send_chat_message' @{ session = $term; text = 'x' })) -eq 'not_a_chat')
  Check 'send_chat_message an einen Chat ohne eingerichteten Server -> no_model' ((Code (Tool 'send_chat_message' @{ session = 950; text = 'x' })) -eq 'no_model')
  Check 'read_chat liest den wiederhergestellten Chat' ((Tool 'read_chat' @{ session = 950 }).data.messages[1].content -eq 'smoke-answer')
  Check 'send_chat_message mit leerem Text -> bad_args' ((Code (Tool 'send_chat_message' @{ session = $C; text = '  ' })) -eq 'bad_args')
  $hadTools = @($script:fakeLog | Where-Object { $_ -like '/api/chat *' -and $_ -match '"tools"\s*:\s*\[\s*\{' }).Count -gt 0
  Check 'Modell bekommt Werkzeuge (Stufe Steuern, level_chosen)' $hadTools
  Check 'Stufe Steuern ohne eigene Kontextlaenge: num_ctx nach Bedarf (16384/32768), kein Kontext-Hinweis' (@($script:fakeLog | Where-Object { $_ -like '/api/chat *' -and $_ -match '"num_ctx"\s*:\s*(16384|32768)' }).Count -ge 1 -and -not ((Tool 'read_chat' @{ session = $C; last = 50 }).text -match 'larger than the model|too small for its tool level'))
  [void](Tool 'send_chat_message' @{ session = $C; text = 'tools please' })
  $w = Tool 'wait_for_status' @{ session = $C; status = @('ready'); timeout_secs = 40 } 50000
  $rc = Tool 'read_chat' @{ session = $C; last = 10 }
  $tm = @($rc.data.messages | Where-Object { $_.role -eq 'tool' -and $_.tool_name -eq 'list_sessions' })
  $lastA = @($rc.data.messages | Where-Object { $_.role -eq 'assistant' })[-1]
  Check 'Werkzeug-Schleife: Chat ruft list_sessions auf und antwortet "done"' ($w.data.matched -eq $true -and $tm.Count -ge 1 -and $tm[0].status -eq 'ok' -and $lastA.content -eq 'done') $rc.text
  # Kein Umweg: ein Agent in Sitzung $A laesst den Chat send_text in $A aufrufen -> forbidden, nichts getippt.
  $pa = Tool 'create_session' @{ shell = 'powershell'; cwd = "$proj"; name = 'Proxy-Agent' }; $A = $pa.data.id; Start-Sleep 4
  $px = InA 'send_chat_message' @{ session = $C; text = "proxy please $A" }
  Check 'Agent in einer Sitzung sendet an den Chat (Hinweis mit Sitzungsname)' ($px.isError -ne $true) ($px | ConvertTo-Json -Compress -Depth 5)
  $w = Tool 'wait_for_status' @{ session = $C; status = @('ready'); timeout_secs = 40 } 50000
  $rc = Tool 'read_chat' @{ session = $C; last = 6 }
  $pt = @($rc.data.messages | Where-Object { $_.role -eq 'tool' -and $_.tool_name -eq 'send_text' })
  $pu = @($rc.data.messages | Where-Object { $_.role -eq 'user' })[-1]
  Check 'Umweg-Schutz: send_text des Chats in die Sitzung des Agenten -> forbidden (abgewiesen vor Freigabe/Ausfuehrung)' ($pt.Count -ge 1 -and $pt[-1].status -eq 'denied' -and $pt[-1].content -match 'cannot write into that agent' -and $pu.note -eq 'Sent by agent Proxy-Agent') $rc.text
  # Kette: Agent $A -> Chat $C -> (send_chat_message) Chat $C2 -> send_text in $A: auch zwei Glieder weiter verboten.
  $C2 = (Tool 'create_session' @{ kind = 'chat'; server = 'Fake'; model = 'fake-model'; name = 'Smoke-LLM2' }).data.id
  $px = InA 'send_chat_message' @{ session = $C; text = "chain please $C2 $A" }
  $w = Tool 'wait_for_status' @{ session = $C; status = @('ready'); timeout_secs = 40 } 50000
  $w2 = Tool 'wait_for_status' @{ session = $C2; status = @('ready'); timeout_secs = 40 } 50000
  $rc1 = Tool 'read_chat' @{ session = $C; last = 4 }
  $sc = @($rc1.data.messages | Where-Object { $_.role -eq 'tool' -and $_.tool_name -eq 'send_chat_message' })
  $rc2 = Tool 'read_chat' @{ session = $C2; last = 6 }
  $pt2 = @($rc2.data.messages | Where-Object { $_.role -eq 'tool' -and $_.tool_name -eq 'send_text' })
  $pu2 = @($rc2.data.messages | Where-Object { $_.role -eq 'user' })[-1]
  Check 'Umweg-Schutz ueber eine Kette: Chat -> Chat -> send_text in die Sitzung des Agenten -> forbidden' ($px.isError -ne $true -and $w2.data.matched -eq $true -and $sc.Count -ge 1 -and $sc[-1].status -eq 'ok' -and $pu2.note -eq 'Sent by agent Smoke-LLM' -and $pt2.Count -ge 1 -and $pt2[-1].status -eq 'denied' -and $pt2[-1].content -match 'cannot write into that agent') "$($rc1.text) /// $($rc2.text)"
  Start-Sleep 1
  Check 'Umweg-Schutz: in der Sitzung des Agenten wurde nichts getippt' ((Tool 'read_terminal' @{ session = $A; lines = 200 }).data.text -notmatch 'proxy-leak')
  # Markierung (#152): Agent $A -> Chat $C legt per create_session ein Terminal an; ein Agent dort darf weder in $A
  # noch in $C schreiben (bis 2.2.0 war ein solches Terminal nicht markiert).
  [void](InA 'send_chat_message' @{ session = $C; text = 'spawn please' })
  $w = Tool 'wait_for_status' @{ session = $C; status = @('ready'); timeout_secs = 40 } 50000
  $T = @((Tool 'list_sessions').data.sessions) | Where-Object { $_.parent -eq $C -and $_.kind -eq 'terminal' } | Select-Object -First 1
  Check 'Markierung: Terminal eines Chats der Kette erbt sie (agent_chain = Agent + Chat)' ($null -ne $T -and @($T.agent_chain) -contains $A -and @($T.agent_chain) -contains $C) ($T | ConvertTo-Json -Compress)
  if ($T) {
    Start-Sleep 4
    $tw = InS $T.id 'send_text' @{ session = $A; text = 'echo taint-leak' }
    Check 'Markierung: Agent im Terminal des Chats -> send_text in den urspruenglichen Agenten forbidden' ($tw.isError -eq $true -and $tw.content[0].text -like 'forbidden*' -and $tw.content[0].text -match 'agent chain') ($tw | ConvertTo-Json -Compress -Depth 5)
    $tc = InS $T.id 'send_chat_message' @{ session = $C; text = 'x' }
    Check 'Markierung: ... und nicht in den Chat der Kette' ($tc.isError -eq $true -and $tc.content[0].text -like 'forbidden*') ($tc | ConvertTo-Json -Compress -Depth 5)
    Check 'Markierung: in der Sitzung des Agenten wurde nichts getippt' ((Tool 'read_terminal' @{ session = $A; lines = 200 }).data.text -notmatch 'taint-leak')
  }
  Check 'wait_for_status auf die eigene Sitzung -> bad_args' ((InA 'wait_for_status' @{ session = $A; status = @('ready'); timeout_secs = 5 }).content[0].text -like 'bad_args*')
  [void](Tool 'close_session' @{ session = $C2 })
  [void](Tool 'close_session' @{ session = $A })
  # read_chat ueber ein Chat-Modell (Ergebnis gekuerzt): die neuesten Nachrichten und der Status kommen an, nicht die aeltesten.
  [void](Tool 'send_chat_message' @{ session = $C; text = 'read please 952' })
  $w = Tool 'wait_for_status' @{ session = $C; status = @('ready'); timeout_secs = 40 } 50000
  $rc = Tool 'read_chat' @{ session = $C; last = 4 }
  $rt = @($rc.data.messages | Where-Object { $_.role -eq 'tool' -and $_.tool_name -eq 'read_chat' })
  Check 'read_chat ueber ein Chat-Modell: neueste Nachrichten, more_above und Status statt der aeltesten' ($w.data.matched -eq $true -and $rt.Count -ge 1 -and $rt[-1].status -eq 'ok' -and $rt[-1].content -match 'big-39' -and $rt[-1].content -match '"more_above":true' -and $rt[-1].content -match '"status":"ready"' -and $rt[-1].content -notmatch 'big-00') (($rc.text) -replace '(.{600}).*', '$1')
  Check 'read_chat: Feld paused (false)' ($rc.data.paused -eq $false -and $rc.data.PSObject.Properties.Name -contains 'pause_note')
  # Stufe des Modells nie gewaehlt: die einmalige Frage erscheint, der Agent bekommt permission_pending.
  $raw = Tool 'create_session' @{ kind = 'chat'; server = 'Fake'; model = 'fake-raw'; name = 'Smoke-Raw' }
  $rq = Tool 'send_chat_message' @{ session = $raw.data.id; text = 'hallo' }
  $rr = Tool 'read_chat' @{ session = $raw.data.id }
  Check 'Stufe nie gewaehlt: send_chat_message -> permission_pending, Chat im Status approval' ((Code $rq) -eq 'permission_pending' -and $rq.text -match 'tool level has not been chosen' -and $rr.data.status -eq 'approval' -and $rr.data.total -eq 0) "$($rq.text) / $($rr.text)"
  $wq = Tool 'wait_for_status' @{ session = $raw.data.id; status = @('ready'); timeout_secs = 1 }
  Check 'wait_for_status bei offener Frage: Zeitlimit mit needs_you' ($wq.data.timed_out -eq $true -and $wq.data.needs_you -eq $true -and $wq.data.status -eq 'approval') $wq.text
  [void](Tool 'close_session' @{ session = $raw.data.id })
  Check 'close_session: selbst angelegter Chat' (-not (Tool 'close_session' @{ session = $C }).err)
  Set-Level 'Read' -Tray
  [void](Rpc 'initialize' @{ protocolVersion = '2025-06-18'; capabilities = @{}; clientInfo = @{ name = 'smoke'; version = '1' } })
  Start-Sleep 2
  $names = @((Rpc 'tools/list').result.tools | ForEach-Object name)
  Check 'Nur lesen: read_chat und list_models ja, send_chat_message nein' ($names -contains 'read_chat' -and $names -contains 'list_models' -and $names -notcontains 'send_chat_message')
  Check 'Nur lesen: send_chat_message verboten (forbidden)' ((Code (Tool 'send_chat_message' @{ session = 950; text = 'x' })) -eq 'forbidden')
  Check 'Nur lesen: create_session kind=chat verboten (forbidden)' ((Code (Tool 'create_session' @{ kind = 'chat' })) -eq 'forbidden')
  $script:prestart = $null

  Write-Host "== Minimierte Bereiche (#142, #162): Wiederherstellung, kaputtes Layout, focus_session mit placement" -ForegroundColor Cyan
  Stop-Hub
  # 960 ist minimiert (mit `slot` wie in 2.2.0 – seit 2.3.0 deprecated, nur durchgereicht); das Layout enthaelt eine unbekannte Sitzung (777), ein Duplikat und Anteil null (NaN)
  $bad = [ordered]@{ Split = [ordered]@{ dir = 'Horizontal'; ratio = $null; a = @{ Leaf = 961 }; b = [ordered]@{ Split = [ordered]@{ dir = 'Vertical'; ratio = 0.5; a = @{ Leaf = 777 }; b = @{ Leaf = 961 } } } } }
  $st = [ordered]@{
    sessions = @(
      [ordered]@{ id = 960; name = 'Smoke-Min'; custom_name = $true; shell_id = 'powershell'; cwd = "$proj"; project = 1; minimized = $true; slot = [ordered]@{ sibling = @(961); dir = 'Horizontal'; before = $false; ratio = 0.5 } },
      [ordered]@{ id = 961; name = 'Smoke-Sicht'; custom_name = $true; shell_id = 'powershell'; cwd = "$proj"; project = 1 })
    next_id = 962; projects = @([ordered]@{ id = 1; name = 'Standard'; layout = $bad; focused = 960 }); active_project = 1
  }
  [IO.File]::WriteAllText("$data\state.json", ($st | ConvertTo-Json -Depth 12), (New-Object Text.UTF8Encoding $false))
  Set-Level 'Control' -Tray
  [void](Rpc 'initialize' @{ protocolVersion = '2025-06-18'; capabilities = @{}; clientInfo = @{ name = 'smoke'; version = '1' } })
  Start-Sleep 3
  $ss = @((Tool 'list_sessions').data.sessions)
  $mn = $ss | Where-Object { $_.id -eq 960 }; $vs = $ss | Where-Object { $_.id -eq 961 }
  Check 'Kaputtes Layout (unbekannt, doppelt, Anteil null): Hub startet, beide Sitzungen da' ($ss.Count -eq 2 -and $null -ne $mn -and $null -ne $vs) ($ss | ConvertTo-Json -Compress)
  Check 'list_sessions: minimiert aus state.json wiederhergestellt (minimized = true)' ($mn.minimized -eq $true -and $vs.minimized -eq $false) ($ss | ConvertTo-Json -Compress)
  Check 'Fokus zeigt nicht auf die minimierte Sitzung' ($mn.focused -eq $false -and $vs.focused -eq $true) ($ss | ConvertTo-Json -Compress)
  $fr = Tool 'focus_session' @{ session = 960; placement = 'solo' }
  Start-Sleep -Milliseconds 600
  $ss = @((Tool 'list_sessions').data.sessions)
  $mn = $ss | Where-Object { $_.id -eq 960 }; $vs = $ss | Where-Object { $_.id -eq 961 }
  Check 'focus_session placement=solo oeffnet die minimierte Sitzung allein (minimized = false, fokussiert)' (-not $fr.err -and $fr.data.placement -eq 'solo' -and $mn.minimized -eq $false -and $mn.focused -eq $true) ($fr.text + ' ' + ($mn | ConvertTo-Json -Compress))
  Check 'focus_session solo: der bisher sichtbare Bereich ist jetzt minimiert (laeuft weiter)' ($vs.minimized -eq $true -and $vs.focused -eq $false -and $vs.status -ne 'exited') ($vs | ConvertTo-Json -Compress)
  $fr = Tool 'focus_session' @{ session = 961; placement = 'solo' }
  Start-Sleep -Milliseconds 600
  $ss = @((Tool 'list_sessions').data.sessions)
  $mn = $ss | Where-Object { $_.id -eq 960 }; $vs = $ss | Where-Object { $_.id -eq 961 }
  Check 'focus_session solo zurueck: Plaetze getauscht (961 sichtbar, 960 minimiert)' (-not $fr.err -and $vs.minimized -eq $false -and $vs.focused -eq $true -and $mn.minimized -eq $true) ($ss | ConvertTo-Json -Compress)
  $fr = Tool 'focus_session' @{ session = 961 }
  Start-Sleep -Milliseconds 300
  $ss = @((Tool 'list_sessions').data.sessions)
  $mn = $ss | Where-Object { $_.id -eq 960 }
  Check 'focus_session auf eine sichtbare Sitzung minimiert nichts' (-not $fr.err -and $mn.minimized -eq $true -and @($ss | Where-Object { $_.minimized }).Count -eq 1) ($ss | ConvertTo-Json -Compress)

  Write-Host "== Platzierung fuer Agenten (#162): Standard replace, split_right, create_session mit placement" -ForegroundColor Cyan
  # Bruecke von aussen (keine eigene Sitzung): aktiver Bereich = der fokussierte (961).
  $fr = Tool 'focus_session' @{ session = 960 }
  Start-Sleep -Milliseconds 600
  $ss = @((Tool 'list_sessions').data.sessions)
  $mn = $ss | Where-Object { $_.id -eq 960 }; $vs = $ss | Where-Object { $_.id -eq 961 }
  Check 'focus_session ohne placement = replace (Antwort placement)' (-not $fr.err -and $fr.data.placement -eq 'replace') $fr.text
  Check 'replace: minimierte 960 ersetzt den aktiven Bereich (sichtbar, nicht minimiert; Fokus folgt, weil der fokussierte Bereich ersetzt wurde)' ($mn.shown -eq $true -and $mn.focused -eq $true -and $mn.minimized -eq $false) ($mn | ConvertTo-Json -Compress)
  Check 'replace: der ersetzte Bereich 961 ist jetzt minimiert (laeuft weiter); sonst nichts minimiert' ($vs.shown -eq $false -and $vs.minimized -eq $true -and @($ss | Where-Object { $_.minimized }).Count -eq 1 -and $vs.status -ne 'exited') ($ss | ConvertTo-Json -Compress)
  $fr = Tool 'focus_session' @{ session = 961; placement = 'split_right' }
  Start-Sleep -Milliseconds 600
  $ss = @((Tool 'list_sessions').data.sessions)
  $mn = $ss | Where-Object { $_.id -eq 960 }; $vs = $ss | Where-Object { $_.id -eq 961 }
  Check 'split_right: aktiver Bereich geteilt, beide sichtbar, nichts minimiert' (-not $fr.err -and $fr.data.placement -eq 'split_right' -and $mn.shown -eq $true -and $vs.shown -eq $true -and @($ss | Where-Object { $_.minimized }).Count -eq 0) ($ss | ConvertTo-Json -Compress)
  Check 'split_right: Tastaturfokus des Nutzers bleibt (960), 961 nicht fokussiert' ($mn.focused -eq $true -and $vs.focused -eq $false) ($ss | ConvertTo-Json -Compress)
  $fr = Tool 'focus_session' @{ session = 960; placement = 'split_down' }
  Start-Sleep -Milliseconds 300
  $ss = @((Tool 'list_sessions').data.sessions)
  Check 'split_* auf eine schon sichtbare Sitzung: Layout und Fokus bleiben (beide sichtbar)' (-not $fr.err -and @($ss | Where-Object { $_.shown }).Count -eq 2 -and ($ss | Where-Object { $_.id -eq 960 }).focused -eq $true) ($ss | ConvertTo-Json -Compress)
  Check 'focus_session mit unbekanntem placement: bad_args' ((Code (Tool 'focus_session' @{ session = 960; placement = 'left' })) -eq 'bad_args')
  $n0 = @((Tool 'list_sessions').data.sessions).Count
  Check 'create_session mit unbekanntem placement: bad_args, nichts angelegt' ((Code (Tool 'create_session' @{ shell = 'powershell'; placement = 'everywhere' })) -eq 'bad_args' -and @((Tool 'list_sessions').data.sessions).Count -eq $n0)
  # 960 fokussiert (links), 961 rechts: replace mit einer neuen Sitzung ersetzt nur 960
  $c = Tool 'create_session' @{ shell = 'powershell'; cwd = "$proj"; name = 'Smoke-Place'; placement = 'replace' }
  Start-Sleep -Milliseconds 800
  $ss = @((Tool 'list_sessions').data.sessions)
  $nw = $ss | Where-Object { $_.id -eq $c.data.id }; $mn = $ss | Where-Object { $_.id -eq 960 }; $vs = $ss | Where-Object { $_.id -eq 961 }
  Check 'create_session placement=replace: neue Sitzung sichtbar und fokussiert, ersetzt nur den aktiven Bereich' (-not $c.err -and $c.data.placement -eq 'replace' -and $nw.shown -eq $true -and $nw.focused -eq $true -and $mn.shown -eq $false -and $vs.shown -eq $true) ($c.text + ' ' + ($ss | ConvertTo-Json -Compress))
  $q = Tool 'create_session' @{ shell = 'powershell'; cwd = "$proj"; name = 'Smoke-Quiet' }
  Start-Sleep -Milliseconds 500
  $ss = @((Tool 'list_sessions').data.sessions)
  $qq = $ss | Where-Object { $_.id -eq $q.data.id }
  Check 'create_session ohne placement bleibt leise (nicht sichtbar, Fokus unveraendert)' (-not $q.err -and $null -eq $q.data.placement -and $qq.shown -eq $false -and ($ss | Where-Object { $_.id -eq $c.data.id }).focused -eq $true) ($q.text + ' ' + ($qq | ConvertTo-Json -Compress))

  Write-Host "== Umgebung der Terminals (#175): Registry (Standard) und 'Umgebung von Paneldeck erben'" -ForegroundColor Cyan
  # Eine Variable nur im Prozess des Hubs und eine Benutzervariable aus HKCU\Environment (nicht vom Isolationsprofil
  # gesetzt), die im Prozess des Hubs einen eigenen Wert bekommt: Standard = portable-pty frischt sie aus der Registry auf,
  # mit inherit_env bleibt der Wert des Prozesses.
  $profVars = @(($ptyEnv -split "`n") | ForEach-Object { ($_ -split '=', 2)[0].ToUpper() })
  $regVar = $null
  $hk = Get-Item -LiteralPath 'HKCU:\Environment' -ErrorAction SilentlyContinue
  if ($hk) { $regVar = @($hk.GetValueNames() | Where-Object { $_ -match '^[A-Za-z_][A-Za-z0-9_]*$' -and $profVars -notcontains $_.ToUpper() -and $_ -notmatch '^(PATH|PATHEXT|PSModulePath|TEMP|TMP|PANELDECK_.*)$' -and "$($hk.GetValue($_))" -ne '' }) | Select-Object -First 1 }
  $ownVal = 'aus-dem-prozess-statt-registry'
  # Vorherigen Wert merken und nach der Schleife wiederherstellen (die folgenden Abschnitte laufen mit der Standardeinstellung).
  $sj = [IO.File]::ReadAllText("$data\settings.json").TrimStart([char]0xFEFF) | ConvertFrom-Json
  $prevInherit = if ($sj.PSObject.Properties.Name -contains 'inherit_env') { $sj.inherit_env } else { $null }
  foreach ($inherit in @($false, $true)) {
    Stop-Hub
    $f = "$data\settings.json"
    $j = [IO.File]::ReadAllText($f).TrimStart([char]0xFEFF) | ConvertFrom-Json
    if ($j.PSObject.Properties.Name -contains 'inherit_env') { $j.inherit_env = $inherit } else { $j | Add-Member inherit_env $inherit }
    [IO.File]::WriteAllText($f, ($j | ConvertTo-Json -Depth 10), (New-Object Text.UTF8Encoding $false))
    $oldVal = if ($regVar) { [Environment]::GetEnvironmentVariable($regVar) } else { $null }
    if ($regVar) { [Environment]::SetEnvironmentVariable($regVar, $ownVal) }
    $env:PD_SMOKE_INHERIT = 'aus-dem-prozess'
    try { Set-Level 'Control' -Tray } finally { if ($regVar) { [Environment]::SetEnvironmentVariable($regVar, $oldVal) }; Remove-Item Env:PD_SMOKE_INHERIT -ErrorAction SilentlyContinue }
    [void](Rpc 'initialize' @{ protocolVersion = '2025-06-18'; capabilities = @{}; clientInfo = @{ name = 'smoke'; version = '1' } })
    $c = Tool 'create_session' @{ shell = 'powershell'; cwd = "$proj"; name = "Smoke-Env-$inherit" }
    # Auf den Prompt warten (sichtbarer Bildschirm zeigt "PS ...>"), statt eine feste Zeit zu schlafen.
    $t0 = Get-Date
    do { Start-Sleep -Milliseconds 300; $scr = "$((Tool 'read_terminal' @{ session = $c.data.id; screen = $true }).data.text)" } while ($scr -notmatch 'PS [^\r\n]*>' -and ((Get-Date) - $t0).TotalSeconds -lt 30)
    if ($scr -notmatch 'PS [^\r\n]*>') { Write-Host "  info  inherit_env: kein Prompt nach 30 s" -ForegroundColor Yellow }
    $rv = if ($regVar) { "`$env:$regVar" } else { "''" }
    # Ein Wert je Zeile (mit Marke davor): eine einzige lange Zeile, die genau am Zeilenende umbricht, liefert bei run_command
    # leere Ausgabe (Befund, siehe Bericht zu #175) - das soll diese Pruefung nicht verdecken, aber auch nicht ausloesen.
    $probe = '"v0=$env:PD_SMOKE_INHERIT"; "v1=$(' + $rv + ')"; "v2=$env:PANELDECK_SESSION"; "v3=$env:USERPROFILE"; "v4=" + (@(Get-Command claude -All -CommandType Application,ExternalScript | ForEach-Object Source) -join '','')'
    # Bis zu drei Versuche; jeder leere Versuch wird mit quality und dem sichtbaren Bildschirm protokolliert (keine stille
    # Wiederholung: ein leeres Ergebnis trotz Prompt ist ein Befund von run_command, kein Fehler dieser Pruefung).
    for ($try = 1; $try -le 3; $try++) {
      $r = Tool 'run_command' @{ session = $c.data.id; command = $probe; timeout_secs = 25 }
      if ("$($r.data.output)".Trim()) { break }
      $scr = "$((Tool 'read_terminal' @{ session = $c.data.id; screen = $true }).data.text)".Trim() -replace '\s+', ' '
      if ($scr.Length -gt 300) { $scr = $scr.Substring($scr.Length - 300) }
      Write-Host "  info  inherit_env: Versuch $try ohne Ausgabe (quality=$($r.data.quality), finished=$($r.data.finished), timed_out=$($r.data.timed_out), duration_ms=$($r.data.duration_ms)); Bildschirm: $scr" -ForegroundColor Yellow
      Start-Sleep 2
    }
    $o = "$($r.data.output)".Trim()
    $parts = @(0..4 | ForEach-Object { $k = "v$_="; $l = @($o -split "`r?`n" | Where-Object { $_.StartsWith($k) }) | Select-Object -First 1; if ($l) { $l.Substring($k.Length).Trim() } else { $null } })
    if (-not $o) { $o = "(leer) create: $($c.text) | run_command: $($r.text)" }
    $label = if ($inherit) { 'an' } else { 'aus' }
    Check "inherit_env $($label): Paneldecks Variable PANELDECK_SESSION = eigene Sitzung" (-not $c.err -and $parts.Count -ge 5 -and $parts[2] -eq "$($c.data.id)") $o
    Check "inherit_env $($label): Isolation haelt (Schein-Profil, claude nur der Ersatz)" ($parts.Count -ge 5 -and $parts[3] -eq $script:iso.Profile -and (Only-Fake ($parts[4] -replace ',', "`n"))) $o
    Check "inherit_env $($label): Variable nur im Prozess kommt an" ($parts[0] -eq 'aus-dem-prozess') $o
    if (-not $regVar) { Skip "inherit_env $($label): Benutzervariable aus Registry vs. Prozess" 'keine geeignete Variable in HKCU\Environment' }
    elseif ($inherit) { Check "inherit_env an: Wert des Prozesses bleibt ($regVar, nicht aus der Registry)" ($parts[1] -eq $ownVal) $o }
    else { Check "inherit_env aus: Wert frisch aus der Registry ($regVar, nicht der des Prozesses)" ($parts[1] -ne $ownVal -and $parts[1] -ne '') $o }
    # Git Bash bekommt dieselbe Umgebung: Shell-Integration (PROMPT_COMMAND/PS0) muss auch mit geerbter Umgebung laden.
    if (Test-Path "$env:ProgramFiles\Git\bin\bash.exe") {
      $g = Tool 'create_session' @{ shell = 'gitbash'; cwd = "$proj"; name = "Smoke-Env-Bash-$inherit" }
      $t0 = Get-Date
      do { Start-Sleep -Milliseconds 500; $r = Tool 'run_command' @{ session = $g.data.id; command = 'echo env-bash-ok'; timeout_secs = 20 } } while ((Code $r) -eq 'no_shell_integration' -and ((Get-Date) - $t0).TotalSeconds -lt 30)
      $info = "$([int]((Get-Date) - $t0).TotalSeconds) s: $($r.text)"
      if (-not ($r.data.finished -and "$($r.data.output)".Trim() -eq 'env-bash-ok')) {
        $info += ' | Bildschirm: ' + "$((Tool 'read_terminal' @{ session = $g.data.id; screen = $true }).data.text)".Trim()
      }
      Check "inherit_env $($label): run_command gitbash (Shell-Integration geladen)" (-not $g.err -and $r.data.finished -and "$($r.data.output)".Trim() -eq 'env-bash-ok') $info
    }
  }
  # Einstellung wie vorher (Standard aus) - die folgenden Abschnitte pruefen nicht #175.
  Stop-Hub
  $sj = [IO.File]::ReadAllText("$data\settings.json").TrimStart([char]0xFEFF) | ConvertFrom-Json
  if ($null -eq $prevInherit) { $sj.PSObject.Properties.Remove('inherit_env') } else { $sj.inherit_env = $prevInherit }
  [IO.File]::WriteAllText("$data\settings.json", ($sj | ConvertTo-Json -Depth 10), (New-Object Text.UTF8Encoding $false))

  Write-Host "== Konsolen-Host und DLL-Suche (#183)" -ForegroundColor Cyan
  # Eine ladbare conpty.dll im Arbeitsordner des Hubs (wie "In Paneldeck oeffnen" auf einem fremden Ordner) darf nie
  # geladen werden: DLLs nur aus dem Programmordner und System32, die mitgelieferte ConPTY nur per vollem Pfad.
  $plant = New-Item -ItemType Directory -Force "$work\plant"
  function Start-ConPtyHub {
    $script:hubCwd = "$plant"
    Set-Level 'Control' -Empty
    $script:hubCwd = $null
    [void](Rpc 'initialize' @{ protocolVersion = '2025-06-18'; capabilities = @{}; clientInfo = @{ name = 'smoke'; version = '1' } })
  }
  function Get-HubConPty {
    $all = @((Get-Process -Id $script:hub.Id).Modules | ForEach-Object { $_.FileName })
    [pscustomobject]@{
      All  = $all
      Dlls = @($all | Where-Object { (Split-Path -Leaf $_) -eq 'conpty.dll' })
      Kids = @(Get-CimInstance Win32_Process -Filter "ParentProcessId=$($script:hub.Id)" | Where-Object { $_.Name -eq 'OpenConsole.exe' })
    }
  }
  if ($hasConPty) {
    Copy-Item "$dir\conpty.dll" "$plant\conpty.dll"
    # A: ohne mitgelieferte Dateien neben der .exe (aeltere Installation). Frueher lud portable-pty "conpty.dll" per Name
    #    ueber die Suchreihenfolge - mit dem Arbeitsordner darin.
    Rename-Item "$dir\conpty.dll" 'conpty.dll.off'; Rename-Item "$dir\OpenConsole.exe" 'OpenConsole.exe.off'
    try {
      Start-ConPtyHub
      $c = Tool 'create_session' @{ shell = 'cmd'; cwd = "$plant"; name = 'smoke-plant' }
      Check 'DLL-Suche: create_session im Ordner mit fremder conpty.dll' (-not $c.err) $c.text
      if (-not $c.err) {
        Start-Sleep 3
        $me = @((Tool 'list_sessions').data.sessions | Where-Object { $_.id -eq $c.data.id })
        Check 'DLL-Suche: ohne mitgelieferte Dateien console_host = system' ($me.Count -eq 1 -and $me[0].console_host -eq 'system') ($me | ConvertTo-Json -Compress)
        $r = Tool 'run_command' @{ session = $c.data.id; command = 'echo plant-ok'; timeout_secs = 25 }
        Check 'DLL-Suche: Sitzung laeuft (ConPTY von Windows)' ($r.data.finished -and $r.data.output.Trim() -eq 'plant-ok') $r.text
      }
      $h = Get-HubConPty
      Check 'DLL-Suche: conpty.dll aus dem Arbeitsordner nicht geladen (keine conpty.dll, kein OpenConsole.exe)' ($h.Dlls.Count -eq 0 -and $h.Kids.Count -eq 0) "dlls=$($h.Dlls -join ', ') kids=$($h.Kids.Count)"
      Stop-Hub
    } finally {
      Rename-Item "$dir\conpty.dll.off" 'conpty.dll'; Rename-Item "$dir\OpenConsole.exe.off" 'OpenConsole.exe'
    }
    # B: mit mitgelieferten Dateien; Einstellung je nach -ConPty.
    Start-ConPtyHub
    foreach ($sh in @('cmd', 'powershell')) {
      $c = Tool 'create_session' @{ shell = $sh; cwd = "$plant"; name = "smoke-conpty-$sh" }
      Check "ConPTY: create_session $sh" (-not $c.err) $c.text
      if ($c.err) { continue }
      Start-Sleep 3
      $want = if ($useBundled) { 'bundled' } else { 'system' }
      $me = @((Tool 'list_sessions').data.sessions | Where-Object { $_.id -eq $c.data.id })
      Check "ConPTY: list_sessions console_host = $want ($sh)" ($me.Count -eq 1 -and $me[0].console_host -eq $want) ($me | ConvertTo-Json -Compress)
      $r = Tool 'run_command' @{ session = $c.data.id; command = 'echo conpty-ok'; timeout_secs = 25 }
      Check "ConPTY: run_command $sh (Ausgabe)" ($r.data.finished -and $r.data.output.Trim() -eq 'conpty-ok') "out='$($r.data.output)' q=$($r.data.quality)"
      $r = Tool 'run_command' @{ session = $c.data.id; command = 'cmd /c exit 4'; timeout_secs = 25 }
      if ($sh -eq 'cmd') { Check 'ConPTY: run_command cmd (Befehl beendet)' ($r.data.finished) $r.text }
      else { Check 'ConPTY: run_command powershell (Exit-Code 4)' ($r.data.exit_code -eq 4) $r.text }
    }
    # Git Bash und Vollbildprogramme (alternativer Bildschirm) ueber die gewaehlte ConPTY.
    if (Test-Path "$env:ProgramFiles\Git\bin\bash.exe") {
      $g = Tool 'create_session' @{ shell = 'gitbash'; cwd = "$plant"; name = 'smoke-conpty-bash' }
      Check 'ConPTY: create_session gitbash' (-not $g.err) $g.text
      if (-not $g.err) {
        $gid = $g.data.id
        # Git Bash (Login-Shell, Git-Prompt) braucht unter Last mehrere Sekunden bis zum ersten Prompt und fuer jeden
        # weiteren: auf Zustaende warten statt fester Pausen (sonst landen Eingaben vor dem Prompt bzw. im falschen Programm).
        $t0 = Get-Date
        do { Start-Sleep -Milliseconds 500; $r = Tool 'run_command' @{ session = $gid; command = 'echo bash-ok'; timeout_secs = 25 } } while ((Code $r) -eq 'no_shell_integration' -and ((Get-Date) - $t0).TotalSeconds -lt 30)
        Check 'ConPTY: run_command gitbash (Ausgabe)' ($r.data.finished -and $r.data.output.Trim() -eq 'bash-ok') $r.text
        $r = Tool 'run_command' @{ session = $gid; command = 'false'; timeout_secs = 25 }
        Check 'ConPTY: run_command gitbash (Exit 1)' ($r.data.exit_code -eq 1) $r.text
        # Sichtbarer Bildschirm, bis $pattern passt (oder Zeitlimit). Prompt der Bash = letzte Zeile "$".
        function Wait-Screen([string]$pattern, [int]$secs = 20) {
          $t = Get-Date
          do { Start-Sleep -Milliseconds 300; $s = "$((Tool 'read_terminal' @{ session = $gid; screen = $true }).data.text)" } while ($s -notmatch $pattern -and ((Get-Date) - $t).TotalSeconds -lt $secs)
          $s
        }
        $bashPrompt = '(?s)\$\s*$'
        [void](Wait-Screen $bashPrompt)
        [IO.File]::WriteAllLines("$plant\zeilen.txt", [string[]](1..300 | ForEach-Object { "zeile-$_" }))
        [void](Tool 'send_text' @{ session = $gid; text = 'less zeilen.txt' })
        $scr = Wait-Screen '(?m)^zeile-1\b'
        Check 'ConPTY: less im alternativen Bildschirm zeigt die Datei' ($scr -match '(?m)^zeile-1\b' -and $scr -notmatch 'bash-ok') $scr
        [void](Tool 'send_text' @{ session = $gid; text = 'q'; submit = $false })
        [void](Wait-Screen $bashPrompt)
        $r = Tool 'run_command' @{ session = $gid; command = 'echo nach-less'; timeout_secs = 15 }
        Check 'ConPTY: nach less zurueck in der Shell' ($r.data.finished -and $r.data.output.Trim() -eq 'nach-less') $r.text
        if (Test-Path "$env:ProgramFiles\Git\usr\bin\vim.exe") {
          # vim beendet sich selbst (send_text ist ein Einfuegen und landete in vim als Text).
          [void](Wait-Screen $bashPrompt)
          [void](Tool 'send_text' @{ session = $gid; text = 'vim -u NONE -c redraw -c "sleep 4" -c "qa!" zeilen.txt' })
          $scr = Wait-Screen '(?m)^zeile-1\b'
          Check 'ConPTY: vim zeigt die Datei' ($scr -match '(?m)^zeile-1\b') $scr
          [void](Wait-Screen $bashPrompt 25)
          $r = Tool 'run_command' @{ session = $gid; command = 'echo nach-vim'; timeout_secs = 15 }
          Check 'ConPTY: nach vim zurueck in der Shell' ($r.data.finished -and $r.data.output.Trim() -eq 'nach-vim') $r.text
        }
      }
    }
    # WSL (falls eine Distribution eingerichtet ist; docker-desktop-Distributionen haben keine Shell).
    $distro = @(& wsl.exe -l -q 2>$null | ForEach-Object { ($_ -replace "`0", '').Trim() } | Where-Object { $_ -and $_ -notlike 'docker-desktop*' }) | Select-Object -First 1
    if ($distro) {
      $w = Tool 'create_session' @{ shell = "wsl:$distro"; cwd = "$plant"; name = 'smoke-conpty-wsl' }
      Check "ConPTY: create_session wsl:$distro" (-not $w.err) $w.text
      if (-not $w.err) {
        # Ein kalter WSL-Start kann dauern: warten, bis die Shell-Integration da ist.
        $t0 = Get-Date
        do {
          Start-Sleep 3
          $r = Tool 'run_command' @{ session = $w.data.id; command = 'echo wsl-ok'; timeout_secs = 40 }
        } while ((Code $r) -eq 'no_shell_integration' -and ((Get-Date) - $t0).TotalSeconds -lt 60)
        Check 'ConPTY: run_command wsl (Ausgabe)' ($r.data.finished -and $r.data.output.Trim() -eq 'wsl-ok') $r.text
      }
    } else { Skip 'ConPTY: WSL' 'keine WSL-Distribution' }
    $h = Get-HubConPty
    # Zur Diagnose der eingeschraenkten DLL-Suche: Module ausserhalb von System32/WinSxS (Grafiktreiber, Erweiterungen).
    $foreign = @($h.All | Where-Object { $_ -notlike "$env:SystemRoot\System32\*" -and $_ -notlike "$env:SystemRoot\WinSxS\*" })
    Write-Host "      Module ausserhalb System32 ($($h.All.Count) gesamt): $($foreign -join '; ')" -ForegroundColor DarkGray
    Check 'DLL-Suche: OpenGL geladen (Darstellung laeuft)' ([bool]($h.All | Where-Object { $_ -like '*\opengl32.dll' }))
    Check 'DLL-Suche: conpty.dll aus dem Arbeitsordner nicht geladen' (-not ($h.Dlls | Where-Object { $_ -like "$plant\*" })) ($h.Dlls -join ', ')
    if ($useBundled) {
      Check 'Mitgelieferte conpty.dll aus dem Programmordner geladen' ($h.Dlls -contains "$dir\conpty.dll") ($h.Dlls -join ', ')
      Check 'OpenConsole.exe aus dem Programmordner ist Konsolen-Host der Sitzungen' ($h.Kids.Count -ge 2 -and -not ($h.Kids | Where-Object { $_.ExecutablePath -ne "$dir\OpenConsole.exe" })) (($h.Kids | ForEach-Object ExecutablePath) -join ', ')
    } else {
      Check 'Einstellung aus: keine conpty.dll geladen, kein OpenConsole.exe' ($h.Dlls.Count -eq 0 -and $h.Kids.Count -eq 0) "dlls=$($h.Dlls -join ', ') kids=$($h.Kids.Count)"
    }
  } else {
    Skip 'Konsolen-Host und DLL-Suche' 'keine conpty.dll/OpenConsole.exe neben der .exe (tools\fetch-conpty.ps1)'
  }
} catch {
  # Abbruch (z. B. Sicherheitspruefung): als Fehler zaehlen, aufraeumen und die Schutzpruefung trotzdem ausfuehren.
  $script:fail++; Write-Host "  ABBRUCH  $($_.Exception.Message)" -ForegroundColor Red
} finally {
  if ($script:fakeListener) { try { $script:fakeListener.Stop() } catch {} }
  if ($script:fakePs) {
    try { if ($script:fakeAsync.AsyncWaitHandle.WaitOne(8000)) { [void]$script:fakePs.EndInvoke($script:fakeAsync) } else { [void]$script:fakePs.BeginStop($null, $null) } } catch {}
    try { $script:fakePs.Dispose() } catch {}
  }
  Stop-Hub
  foreach ($h in $script:extra) { if ($h -and -not $h.HasExited) { $h.Kill() } }
  if (Test-Path -LiteralPath $calls) { Write-Host "Ersatz-claude, Aufrufe (fakebin\calls.txt):" -ForegroundColor DarkGray; Get-Content -LiteralPath $calls | ForEach-Object { Write-Host "  $_" -ForegroundColor DarkGray } }
  else { Write-Host "Ersatz-claude wurde nicht aufgerufen (fakebin\calls.txt fehlt)." -ForegroundColor DarkGray }
  Exit-IsolatedTestEnv $script:iso
  if (-not $KeepFiles) { Remove-Item -LiteralPath $work -Recurse -Force -ErrorAction SilentlyContinue }
}
# Schutzpruefung: das echte ~/.claude des aufrufenden Nutzers muss unberuehrt sein.
Write-Host "== Schutzpruefung: echtes ~/.claude ($realHome)" -ForegroundColor Cyan
$guard = Test-RealClaudeUntouched $realSnap $tag -Strict:$StrictClaudeConfig
foreach ($m in $guard.Warnings) { Write-Host "  WARNUNG  $m" -ForegroundColor Yellow }
Check 'Echtes ~/.claude unberuehrt (kein projects-Ordner des Tests, .claude.json/settings.json ohne Testordner)' ($guard.Violations.Count -eq 0) ($guard.Violations -join '; ')
if ($guard.Violations.Count) {
  Write-Host ""
  Write-Host "!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!" -ForegroundColor Red
  Write-Host "!!! SICHERHEIT: Der Smoke-Test hat das ECHTE Claude-Profil beruehrt:" -ForegroundColor Red
  foreach ($m in $guard.Violations) { Write-Host "!!!   $m" -ForegroundColor Red }
  Write-Host "!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!" -ForegroundColor Red
}
Write-Host ""
Write-Host ("Ergebnis: {0} bestanden, {1} fehlgeschlagen, {2} uebersprungen" -f $script:pass, $script:fail, $script:skip) -ForegroundColor $(if ($script:fail -eq 0) { 'Green' } else { 'Red' })
exit $(if ($script:fail -eq 0) { 0 } else { 1 })
