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
  [switch]$NoClipboard,
  # zsh/fish-Integration (#57) in eigenen WSL-Test-Distributionen (nie der eigenen): Name einer Distribution mit Login-Shell
  # zsh bzw. fish. Ohne Angabe werden diese Pruefungen uebersprungen.
  [string]$WslZsh = $env:PANELDECK_TEST_WSL_ZSH,
  [string]$WslFish = $env:PANELDECK_TEST_WSL_FISH,
  # Tutorial (#143): Test-Build mit der Test-Schnittstelle des Tutorials, gebaut mit
  #   cargo build --release --features tutorial-test --target-dir E:\paneldeck-test\target-tutorial-test
  # Ohne Angabe wird der Tutorial-Teil uebersprungen. Die Ziele (Rechtecke) werden nur mit -Visible geprueft
  # (ein minimiertes Fenster hat keine Flaeche).
  [string]$TutorialExe = ''
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
# Farbabfragen (OSC 10/11) kommen nur mit der mitgelieferten ConPTY beim Programm an (#183).
$script:bundledConpty = $useBundled
# Datenordner der Test-Instanz (portabel neben der Kopie). Ein gesetztes PANELDECK_DATA bzw. PANELDECK_HUB_DATA
# eines umgebenden Paneldeck-Terminals wuerde Hub und Bruecke sonst auf andere Ordner lenken.
$data = "$dir\paneldeck-data"
# Themes (#309): ein gueltiges eigenes Theme mit Oberflaechenfarben und eine ungueltige Datei (fuer theme_list/theme_get).
New-Item -ItemType Directory -Force "$data\themes" | Out-Null
$ansi = '"ansi":["#000000","#cc3333","#33aa33","#ccaa33","#3366cc","#aa33aa","#33aaaa","#cccccc","#555555","#ff5555","#55ff55","#ffff55","#5599ff","#ff55ff","#55ffff","#ffffff"]'
[IO.File]::WriteAllText("$data\themes\smoke.json", '{"name":"Smoke Theme","dark":true,"fg":"#e0e0e0","bg":"#202020",' + $ansi + ',"ui":{"accent":"#0080ff","border":"@text/20","future_token":"#123456"},"future_field":1}', (New-Object Text.UTF8Encoding $false))
[IO.File]::WriteAllText("$data\themes\broken.json", '{"name":"Broken","fg":"white","bg":"#000000",' + $ansi + '}', (New-Object Text.UTF8Encoding $false))
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
  $all = @($p) + @($script:extraProfiles)
  if ($j.PSObject.Properties.Name -contains 'profiles') { $j.profiles = $all } else { $j | Add-Member profiles $all }
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
$script:extraProfiles = @()
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
# #193: eine Ausgabezeile, die genau die Terminalbreite fuellt (ConPTY laesst den Zeilenvorschub weg, die Zeile gilt als
# umbrochen), kam als leere Ausgabe mit quality exact an. Geprueft: Breite-1, Breite, Breite+1, 2 x Breite (PowerShell, Git Bash).
# #197: Bei vollem Verlauf (Einstellung 1000 Zeilen, siehe Set-Level) verschoben sich alle absoluten Zeilen mit jedem
# Rollen; danach fand run_command das Echo nicht mehr (leere Ausgabe, quality heuristic). Verlauf fuellen, dann pruefen.
function Test-FullHistory([int]$sid, [string]$sh) {
  $fill = if ($sh -eq 'gitbash') { 'seq 1 1500' } else { '1..1500' }
  [void](Tool 'run_command' @{ session = $sid; command = $fill; timeout_secs = 40 })
  foreach ($i in 1..3) {
    $r = Tool 'run_command' @{ session = $sid; command = "echo voll-$i"; timeout_secs = 25 }
    Check "run_command $sh (voller Verlauf, Befehl ${i}: Ausgabe exakt)" ($r.data.finished -and $r.data.output -eq "voll-$i" -and $r.data.quality -eq 'exact') "out='$($r.data.output)' q=$($r.data.quality)"
  }
  Test-WrapWidth $sid $sh 'voller Verlauf: '
}
function Test-WrapWidth([int]$sid, [string]$sh, [string]$prefix = '') {
  $cols = (Tool 'read_terminal' @{ session = $sid; lines = 1 }).data.cols
  if (-not $cols) { Skip "${prefix}run_command $sh (Zeile in Terminalbreite)" 'cols unbekannt'; return }
  foreach ($n in @(($cols - 1), $cols, ($cols + 1), (2 * $cols))) {
    $cmd = if ($sh -eq 'gitbash') { "printf 'w%.0s' `$(seq 1 $n); echo" } else { "'w' * $n" }
    $r = Tool 'run_command' @{ session = $sid; command = $cmd; timeout_secs = 25 }
    $want = 'w' * $n
    Check "${prefix}run_command $sh (Zeile mit $n Zeichen bei $cols Spalten: Ausgabe exakt)" ($r.data.finished -and $r.data.output -eq $want -and $r.data.quality -eq 'exact') "len=$("$($r.data.output)".Length) q=$($r.data.quality) finished=$($r.data.finished)"
  }
}
# #264: Mit der ConPTY von Windows (aelterer conhost) blieb das Terminal nach einer Zeile in genau Terminalbreite, die
# allein in einem Frame von conhost endet, eine Zeile hinter conhost: Die Leerzeile vor dem Git-Bash-Prompt fehlte, und
# ein spaeterer Befehl, den conhost nach Enter absolut neu zeichnet (laenger als eine Zeile), erschien doppelt.
# `sleep 1` trennt die volle Zeile vom Zeilenvorschub (eigener Frame). Nur Sitzungen mit console_host = system; zeichnet
# ein neuerer conhost ohnehin richtig, gelten die Pruefungen genauso.
function Test-ConhostWrap([int]$sid, [string]$prefix = '') {
  $name = "${prefix}#264 Git Bash, Zeile in Terminalbreite (System-ConPTY)"
  $me = @((Tool 'list_sessions').data.sessions | Where-Object { $_.id -eq $sid })
  if ($me.Count -ne 1 -or $me[0].console_host -ne 'system') { Skip $name "console_host = $(if ($me.Count) { $me[0].console_host } else { '?' })"; return }
  $cols = (Tool 'read_terminal' @{ session = $sid; lines = 1 }).data.cols
  if (-not $cols) { Skip $name 'cols unbekannt'; return }
  [void](Tool 'run_command' @{ session = $sid; command = 'clear'; timeout_secs = 15 })
  $r = Tool 'run_command' @{ session = $sid; command = "printf 'x%.0s' `$(seq 1 `$COLUMNS); sleep 1; echo"; timeout_secs = 25 }
  $x = 'x' * $cols
  Check "$name`: Ausgabe exakt" ($r.data.finished -and $r.data.output -eq $x -and $r.data.quality -eq 'exact') "len=$("$($r.data.output)".Length) q=$($r.data.quality) finished=$($r.data.finished)"
  # Lange Pause wie beim Nutzer, dann ein Befehl laenger als eine Zeile.
  Start-Sleep 3
  $r = Tool 'run_command' @{ session = $sid; command = "echo nach-264 # $('z' * $cols)"; timeout_secs = 25 }
  Check "$name`: naechster Befehl exakt" ($r.data.finished -and $r.data.output -eq 'nach-264' -and $r.data.quality -eq 'exact') "out='$($r.data.output)' q=$($r.data.quality)"
  Start-Sleep -Milliseconds 500
  $lines = @("$((Tool 'read_terminal' @{ session = $sid; screen = $true }).data.text)" -split "`r?`n" | ForEach-Object { $_.TrimEnd() })
  $i = [array]::IndexOf($lines, $x)
  $blank = $i -ge 0 -and $i + 2 -lt $lines.Count -and $lines[$i + 1] -eq '' -and $lines[$i + 2] -ne ''
  Check "$name`: Leerzeile vor dem Prompt bleibt" $blank ($lines -join ' | ')
  $echoes = @($lines | Where-Object { $_ -match '^\$ echo nach-264 #' }).Count
  Check "$name`: Echo des naechsten Befehls genau einmal" ($echoes -eq 1) "Echos=${echoes}: $($lines -join ' | ')"
}
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
# Tastenkodierung (#183): send_keys kodiert wie die Tastatur. Ein klassisches Konsolenprogramm ([Console]::ReadKey =
# ReadConsoleInput) und ein Programm mit VT-Eingabe (node im Rohmodus wie Claude Code, ohne und mit kitty-Flags).
function Test-Keys([int]$sid) {
  $out = Join-Path $work 'keys-classic.txt'
  Remove-Item -LiteralPath $out -ErrorAction SilentlyContinue
  $cmd = '$r = 1..4 | ForEach-Object { $k = [Console]::ReadKey($true); ''{0}/{1}/{2}'' -f $k.Key, $k.Modifiers, [int]$k.KeyChar }; [IO.File]::WriteAllText(''' + $out + ''', ($r -join '' ''))'
  [void](Tool 'send_text' @{ session = $sid; text = $cmd; submit = $true })
  Start-Sleep -Milliseconds 1500
  foreach ($k in @('ctrl+backspace', 'alt+.', 'ctrl+space', 'shift+enter')) { [void](Tool 'send_keys' @{ session = $sid; keys = @($k) }); Start-Sleep -Milliseconds 300 }
  for ($i = 0; $i -lt 50 -and -not (Test-Path -LiteralPath $out); $i++) { Start-Sleep -Milliseconds 100 }
  $got = if (Test-Path -LiteralPath $out) { [IO.File]::ReadAllText($out) } else { '<nichts>' }
  # Umschalt+Enter kommt bei klassischen Programmen als Alt+Enter an (ESC CR, damit VT-Programme es behalten).
  Check 'send_keys (#183): klassisches Programm bekommt Strg+Rueck, Alt+., Strg+Leertaste (win32-input)' ($got -eq 'Backspace/Control/127 OemPeriod/Alt/46 Spacebar/Control/32 Enter/Alt/13') $got
  if (-not (Get-Command node -ErrorAction SilentlyContinue)) { Skip 'send_keys (#183): Programm mit VT-Eingabe' 'node fehlt'; return }
  $js = Join-Path $work 'keys-vt.js'
  [IO.File]::WriteAllText($js, @'
const fs = require('fs');
const out = process.argv[2], fl = process.argv[3] || '';
process.stdin.setRawMode(true); process.stdin.resume();
const got = [];
process.stdin.on('data', d => { for (const c of d) got.push(c); });
if (fl) process.stdout.write('\x1b[>' + fl + 'u');
process.stdout.write('keys-ready\r\n');
setTimeout(() => { if (fl) process.stdout.write('\x1b[<u'); fs.writeFileSync(out, got.map(c => c.toString(16).padStart(2, '0')).join(' ')); process.exit(0); }, 3500);
'@)
  foreach ($case in @(@('', '1b 0d 1b 2e 08'), @('1', '1b 5b 31 33 3b 32 75 1b 5b 34 36 3b 33 75 1b 5b 31 32 37 3b 35 75'))) {
    $fl = $case[0]; $want = $case[1]
    $out = Join-Path $work "keys-vt$fl.txt"
    Remove-Item -LiteralPath $out -ErrorAction SilentlyContinue
    [void](Tool 'send_text' @{ session = $sid; text = "node '$js' '$out' '$fl'"; submit = $true })
    Start-Sleep -Milliseconds 1500
    foreach ($k in @('shift+enter', 'alt+.', 'ctrl+backspace')) { [void](Tool 'send_keys' @{ session = $sid; keys = @($k) }); Start-Sleep -Milliseconds 300 }
    for ($i = 0; $i -lt 60 -and -not (Test-Path -LiteralPath $out); $i++) { Start-Sleep -Milliseconds 100 }
    $got = if (Test-Path -LiteralPath $out) { [IO.File]::ReadAllText($out) } else { '<nichts>' }
    $what = if ($fl) { 'mit kitty-Flags: CSI 13;2u, CSI 46;3u, CSI 127;5u' } else { 'ohne kitty: ESC CR, ESC ., 0x08' }
    Check "send_keys (#183): VT-Programm (node) $what" ($got -eq $want) $got
    Start-Sleep -Milliseconds 500
  }
}
function Set-Level($level, [switch]$Empty, [switch]$Tray) {
  Stop-Hub
  $f = "$data\settings.json"
  New-Item -ItemType Directory -Force "$data" | Out-Null
  $j = if (Test-Path $f) { [IO.File]::ReadAllText($f).TrimStart([char]0xFEFF) | ConvertFrom-Json } else { [pscustomobject]@{} }
  if ($j.PSObject.Properties.Name -contains 'mcp_level') { $j.mcp_level = $level } else { $j | Add-Member mcp_level $level }
  # Festes Theme, das nicht das erste der Liste ist: Farbabfragen (OSC 11) muessen dessen Werte melden (#183).
  $j | Add-Member -NotePropertyName theme -NotePropertyValue 'Dracula' -Force
  if ($ConPty -eq 'system') { $j | Add-Member -Force bundled_conpty $false }
  # Kleinster Verlauf (1000 Zeilen), damit Test-FullHistory ihn schnell fuellt (#197).
  $j | Add-Member -Force scrollback_lines 1000
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
      # Die Registry-Auffrischung darf den Benutzer nicht durch HKLM-Werte ersetzen (dort steht USERNAME=SYSTEM).
      $r = Tool 'run_command' @{ session = $id; command = '"$env:USERNAME|$env:USERDOMAIN"'; timeout_secs = 25 }
      $want = "$env:USERNAME|$env:USERDOMAIN"
      Check 'Terminal: USERNAME = aktueller Benutzer (USERDOMAIN ebenso)' ($r.data.output.Trim() -eq $want) "'$($r.data.output.Trim())' statt '$want'"
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
      Test-WrapWidth $id $sh
      Test-ConhostWrap $id
      Test-FullHistory $id $sh
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
      Test-WrapWidth $id $sh
      Test-FullHistory $id $sh
      Test-Osc52 $id $sh
      # Name/Version des Terminals in der Umgebung der Shell (#183)
      $r = Tool 'run_command' @{ session = $id; command = '"$env:TERM_PROGRAM|$env:TERM_PROGRAM_VERSION"'; timeout_secs = 25 }
      Check "TERM_PROGRAM=Paneldeck und TERM_PROGRAM_VERSION in $sh" ($r.data.output.Trim() -match '^Paneldeck\|\d+\.\d+\.\d+') "'$($r.data.output)'"
      # Farbabfrage OSC 11: Antwort kommt als Eingabe zurueck (nur mit der mitgelieferten ConPTY; die des Systems
      # reicht die Abfrage nicht an das Terminal weiter)
      if ($script:bundledConpty) {
        $q = '[Console]::Write("$([char]27)]11;?$([char]27)\"); $s = ''''; $t0 = [DateTime]::Now; while (([DateTime]::Now - $t0).TotalMilliseconds -lt 2000) { while ([Console]::KeyAvailable) { $s += [Console]::ReadKey($true).KeyChar }; if ($s -match ''rgb:[0-9a-f/]+'') { break }; Start-Sleep -Milliseconds 50 }; ''reply='' + ($s -replace "$([char]27)", ''ESC'' -replace "$([char]7)", ''BEL'')'
        $r = Tool 'run_command' @{ session = $id; command = $q; timeout_secs = 25 }
        # Sitzung per MCP angelegt und nie gezeichnet: Antwort trotzdem mit dem Theme der Sitzung (Dracula, 282a36).
        Check "OSC 11 (Hintergrundfarbe abfragen) meldet das Theme der Sitzung ($sh, mitgelieferte ConPTY, ungezeichnet)" ($r.data.output -match 'reply=ESC\]11;rgb:2828/2a2a/3636') "'$($r.data.output)'"
        $q = '[Console]::Write("$([char]27)[16t"); $s = ''''; $t0 = [DateTime]::Now; while (([DateTime]::Now - $t0).TotalMilliseconds -lt 2000) { while ([Console]::KeyAvailable) { $s += [Console]::ReadKey($true).KeyChar }; if ($s -match ''t$'') { break }; Start-Sleep -Milliseconds 50 }; ''reply='' + ($s -replace "$([char]27)", ''ESC'')'
        $r = Tool 'run_command' @{ session = $id; command = $q; timeout_secs = 25 }
        Check "CSI 16t (Zellgroesse) meldet eine echte Groesse ($sh, ungezeichnet)" ($r.data.output -match 'reply=ESC\[6;[1-9]\d*;[1-9]\d*t') "'$($r.data.output)'"
      } else {
        Skip "OSC 11 (Hintergrundfarbe abfragen) wird beantwortet ($sh)" 'keine mitgelieferte ConPTY (conpty.dll + OpenConsole.exe) neben der .exe'
      }
      $r = Tool 'run_command' @{ session = $id; command = 'Start-Sleep 4'; timeout_secs = 1 }
      Check "run_command $sh (Zeitlimit)" ($r.data.timed_out -eq $true -and -not $r.data.finished) $r.text
      Start-Sleep 5
      Check 'send_text' (-not (Tool 'send_text' @{ session = $id; text = 'echo per-send_text'; submit = $true }).err); Start-Sleep 1
      Check 'send_keys' (-not (Tool 'send_keys' @{ session = $id; keys = @('ctrl+l') }).err)
      Check 'send_keys ungueltige Taste' ((Code (Tool 'send_keys' @{ session = $id; keys = @('ctrl+alt+entf') })) -eq 'bad_args')
      Test-Keys $id
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

  Write-Host "== WSL: zsh und fish (#57)" -ForegroundColor Cyan
  # Nur in eigenen Test-Distributionen, nie in der des Nutzers: -WslZsh/-WslFish (bzw. PANELDECK_TEST_WSL_ZSH/_FISH) nennen
  # je eine Distribution, deren Login-Shell zsh bzw. fish ist. Ohne Angabe wird der Teil uebersprungen.
  $wslAll = [regex]::Matches($avail, 'wsl:[^,\s]+') | ForEach-Object { $_.Value.TrimEnd('.') }
  foreach ($c in @(@{ sh = 'zsh'; distro = $WslZsh; ver = 'echo "zsh:$ZSH_VERSION"'; left = 'echo "[${ZDOTDIR-unset}][${PANELDECK_ZSH_ZDOTDIR-}]"'; leftWant = '[unset][]' },
                   @{ sh = 'fish'; distro = $WslFish; ver = 'echo "fish:$FISH_VERSION"'; left = 'set -q PANELDECK_FISH_INIT; or echo "[gone]"'; leftWant = '[gone]' })) {
    $sh = $c.sh
    if (-not $c.distro) { Skip "WSL $sh (#57)" "keine Test-Distribution (-Wsl$((Get-Culture).TextInfo.ToTitleCase($sh)) <name> bzw. PANELDECK_TEST_WSL_$($sh.ToUpper()): eigene Distribution mit Login-Shell $sh, nie die eigene)"; continue }
    $id = "wsl:$($c.distro)"
    if ($wslAll -notcontains $id) { Check "WSL ${sh}: Test-Distribution $($c.distro) erkannt" $false "verfuegbar: $($wslAll -join ', ')"; continue }
    # Die Login-Shell wird beim ersten Start abgefragt; fish bekommt die Integration erst, wenn sie bekannt ist (dann neu anlegen).
    $sid = $null; $r = $null
    for ($try = 1; $try -le 4 -and -not $sid; $try++) {
      $w = Tool 'create_session' @{ shell = $id; cwd = '/tmp'; name = "smoke-$sh" } 60000
      if ($w.err) { Check "WSL ${sh}: create_session" $false $w.text; break }
      Start-Sleep 6
      $r = Tool 'run_command' @{ session = $w.data.id; command = $c.ver; timeout_secs = 30 } 45000
      if ($r.data.finished) { $sid = $w.data.id } else { [void](Tool 'close_session' @{ session = $w.data.id }); Start-Sleep 3 }
    }
    if (-not $sid) { Check "WSL ${sh}: Shell-Integration aktiv (run_command)" $false $r.text; continue }
    Check "WSL ${sh}: laeuft wirklich $sh, Ausgabe exakt" ($r.data.output -match "^${sh}:\d" -and $r.data.quality -eq 'exact') "out='$($r.data.output)' q=$($r.data.quality)"
    $r = Tool 'run_command' @{ session = $sid; command = 'false'; timeout_secs = 25 }
    Check "WSL ${sh}: Exit-Code 1, leere Ausgabe" ($r.data.exit_code -eq 1 -and $r.data.output -eq '') $r.text
    $r = Tool 'run_command' @{ session = $sid; command = 'printf "x\ny\n"'; timeout_secs = 25 }
    Check "WSL ${sh}: zweiter Befehl sauber, Exit 0" ($r.data.output -eq "x`ny" -and $r.data.exit_code -eq 0 -and $r.data.quality -eq 'exact') $r.text
    $r = Tool 'run_command' @{ session = $sid; command = 'sleep 2'; timeout_secs = 25 }
    Check "WSL ${sh}: Dauer gemessen (sleep 2)" ($r.data.finished -and $r.data.duration_ms -ge 1500) $r.text
    # Ausgabe ohne Zeilenende: zsh haengt PROMPT_EOL_MARK (%) an, run_command entfernt sie
    $r = Tool 'run_command' @{ session = $sid; command = 'printf x'; timeout_secs = 25 }
    Check "WSL ${sh}: printf x liefert genau x" ($r.data.output -eq 'x' -and $r.data.quality -eq 'exact') "out='$($r.data.output)' q=$($r.data.quality)"
    $r = Tool 'run_command' @{ session = $sid; command = $c.left; timeout_secs = 25 }
    Check "WSL ${sh}: Hilfsvariablen aufgeraeumt (ZDOTDIR wiederhergestellt)" ($r.data.output -eq $c.leftWant) "out='$($r.data.output)'"
    [void](Tool 'run_command' @{ session = $sid; command = 'cd /usr'; timeout_secs = 25 })
    $l = (Tool 'list_sessions').data.sessions | Where-Object { $_.id -eq $sid }
    Check "WSL ${sh}: Ordner folgt (cd /usr)" ($l.cwd -like '*usr') "cwd='$($l.cwd)'"
    $f = if ($sh -eq 'zsh') { "$data\shell-integration\zsh\.zshenv" } else { "$data\shell-integration\paneldeck.fish" }
    Check "WSL ${sh}: Hilfsdatei im Datenordner" (Test-Path -LiteralPath $f) $f
    if ($sh -eq 'zsh') {
      # Eigenes ZDOTDIR (nur in /tmp der Test-Distribution): dieselbe Huelle mit PANELDECK_ZSH_ZDOTDIR laedt die .zshrc von dort
      [void](Tool 'run_command' @{ session = $sid; command = "mkdir -p /tmp/pd-zdot && echo 'PD_OWN_RC=ok' > /tmp/pd-zdot/.zshrc"; timeout_secs = 25 })
      $wz = ("$data\shell-integration\zsh" -replace "'", "")
      [void](Tool 'run_command' @{ session = $sid; command = "exec env PANELDECK_ZSH_ZDOTDIR=/tmp/pd-zdot ZDOTDIR=`"`$(wslpath '$wz')`" zsh -i"; timeout_secs = 30 })
      $r = Tool 'run_command' @{ session = $sid; command = 'echo "[$PD_OWN_RC][$ZDOTDIR]"'; timeout_secs = 25 }
      Check 'WSL zsh: eigenes ZDOTDIR des Nutzers gilt (dessen .zshrc geladen, Marken weiter da)' ($r.data.output -eq '[ok][/tmp/pd-zdot]' -and $r.data.quality -eq 'exact') "out='$($r.data.output)' q=$($r.data.quality)"
    }
    Check "WSL ${sh}: close_session" (-not (Tool 'close_session' @{ session = $sid }).err)
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
    $o = $null; try { $o = $r.data.output | ConvertFrom-Json } catch { }
    if (-not $o) { Write-Host "      InS $tool`: keine JSON-Antwort: $($r.text)" -ForegroundColor DarkGray }
    $o.result
  }
  function InA($tool, $targs) { InS $A $tool $targs }
  # Losgeloester Absender: eine kurzlebige Shell (in Sitzung $s per run_command, ohne $s vom Test selbst gestartet)
  # startet per Start-Process eine PowerShell und endet sofort; diese schickt nach 3 s eine rohe Anfrage ohne Aufrufer
  # (Token aus ipc.json) an den Hub. Rueckgabe: die rohe Antwort ('' ohne Antwort).
  # $flag: statt 3 s zu warten, bis diese Datei existiert (hoechstens 90 s); dann liefert die Funktion sofort den Pfad
  # der Ausgabedatei (Aufrufer liest sie spaeter, z. B. nachdem die Sitzung geschlossen wurde).
  function Detached($s, [string]$tool, [string]$argsJson, [string]$flag = '') {
    $n = [guid]::NewGuid().ToString('N').Substring(0, 6)
    $out = "$work\det-$n.txt"; $ps = "$work\det-$n.ps1"
    $wait = if ($flag) { "`$t0 = Get-Date; while (-not (Test-Path '$flag') -and ((Get-Date) - `$t0).TotalSeconds -lt 90) { Start-Sleep -Milliseconds 200 }" } else { 'Start-Sleep -Seconds 3' }
    [IO.File]::WriteAllText($ps, @"
$wait
`$j = Get-Content -Raw '$data\ipc.json' | ConvertFrom-Json
`$c = New-Object Net.Sockets.TcpClient('127.0.0.1', [int]`$j.port)
`$s = `$c.GetStream()
`$b = [Text.Encoding]::UTF8.GetBytes('{"token":"' + `$j.token + '","cmd":"mcp","tool":"$tool","args":$argsJson}')
`$s.Write(`$b, 0, `$b.Length); `$c.Client.Shutdown(1)
[IO.File]::WriteAllText('$out', (New-Object IO.StreamReader(`$s)).ReadToEnd())
"@, (New-Object Text.UTF8Encoding $false))
    $inner = "Start-Process powershell -WindowStyle Hidden -ArgumentList '-NoProfile','-ExecutionPolicy','Bypass','-File','$ps'"
    if ($s) { [void](Tool 'run_command' @{ session = $s; command = "powershell -NoProfile -Command ""$inner"""; timeout_secs = 30 }) }
    else { $p = Start-Process powershell -PassThru -WindowStyle Hidden -ArgumentList '-NoProfile', '-Command', $inner; [void]$p.WaitForExit(15000) }
    if ($flag) { return $out }
    $deadline = (Get-Date).AddSeconds(25)
    while (-not (Test-Path $out) -and (Get-Date) -lt $deadline) { Start-Sleep -Milliseconds 300 }
    if (Test-Path $out) { [IO.File]::ReadAllText($out) } else { '' }
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
  # Agenten-Kette, gelockert (#152, Entscheidung des Nutzers): Der Eltern-Agent wurde vom Nutzer gestartet (keine Kette),
  # also ist sein Kind nicht markiert und darf in ihn zurueckschreiben.
  $kw = InS $K 'whoami' @{}
  Check 'Kette gelockert: Kind eines vom Nutzer gestarteten Agenten ist frei (agent_chain leer)' ($kw.structuredContent.session -eq $K -and @($kw.structuredContent.agent_chain).Count -eq 0) ($kw | ConvertTo-Json -Compress -Depth 5)
  $kb = InS $K 'rename_session' @{ session = $A; name = 'Eltern-Agent' }
  Check 'Kette gelockert: Agent im Kind darf in den Eltern-Agenten zurueckschreiben (rename_session)' ($kb.isError -ne $true) ($kb | ConvertTo-Json -Compress -Depth 5)
  # Job Objects (#168): ein losgeloester Enkel (eine kurzlebige Shell im Kind startet per Start-Process eine PowerShell und
  # endet sofort; deren Elternteil gibt es dann nicht mehr) wird ueber den Job des Kindes zugeordnet - frueher "verwaist".
  $det = Detached $K 'whoami' '{}'
  Check 'Job: losgeloester Enkel (Start-Process nach Ende seiner Shell) -> der Sitzung des Kindes zugeordnet' ($det -match ('"session":' + $K + '[,}]') -and $det -match '"identified_by":"process"') $det
  # Gegenprobe: derselbe losgeloeste Start vom Test selbst (ausserhalb jeder Sitzung) -> keiner Sitzung zugeordnet.
  $out = Detached $null 'whoami' '{}'
  Check 'Job: losgeloester Prozess ausserhalb jeder Sitzung -> keiner Sitzung zugeordnet' ($out -match '"session":null' -and $out -match '"identified_by":"none"') $out
  # Hintergrundprozess (#168): das Kind startet eine versteckte PowerShell mit eigener Konsole (ueberlebt das Schliessen);
  # close_session per MCP fragt nie, laesst sie weiterlaufen und meldet sie in remaining_processes.
  $bgMark = "$work\bg-$([guid]::NewGuid().ToString('N').Substring(0, 6)).txt"
  $bgCmd = "Start-Process powershell -WindowStyle Hidden -ArgumentList '-NoProfile','-Command','`$PID | Out-File -Encoding ascii ''$bgMark''; Start-Sleep 120'"
  [void](Tool 'run_command' @{ session = $K; command = $bgCmd; timeout_secs = 30 })
  $t0 = Get-Date; while (-not (Test-Path $bgMark) -and ((Get-Date) - $t0).TotalSeconds -lt 15) { Start-Sleep -Milliseconds 200 }
  # Out-File kann die Datei kurz noch offen halten: Lesen wiederholen.
  $bgPid = 0
  for ($i = 0; $i -lt 25 -and $bgPid -eq 0 -and (Test-Path $bgMark); $i++) { try { $bgPid = [int]([IO.File]::ReadAllText($bgMark).Trim()) } catch { Start-Sleep -Milliseconds 200 } }
  $cl = InA 'close_session' @{ session = $K }
  $rem = @($cl.structuredContent.remaining_processes)
  Check 'close_session (MCP) fragt nicht und meldet den Hintergrundprozess in remaining_processes' ($cl.isError -ne $true -and $bgPid -gt 0 -and @($rem | Where-Object { $_.pid -eq $bgPid -and $_.name -match '^powershell' }).Count -eq 1) ($cl | ConvertTo-Json -Compress -Depth 6)
  Check 'close_session (MCP): Hintergrundprozess laeuft weiter (Weiterlaufen lassen)' ($bgPid -gt 0 -and $null -ne (Get-Process -Id $bgPid -ErrorAction SilentlyContinue))
  if ($bgPid -gt 0) { Stop-Process -Id $bgPid -Force -ErrorAction SilentlyContinue }
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
    $ct = Wait-FakeFile $calls 'claude\.cmd'
    Check 'Claude-Start erreicht den Ersatz (fakebin\calls.txt: Hooks per --settings aus dem Test-Datenordner)' ($ct -match 'claude\.cmd' -and $ct -match ('--settings "?[^ ]*' + [regex]::Escape($tag) + '/hub/paneldeck-data/claude-hooks\.json')) "calls.txt='$ct'"
    # #302: der erste Start einer neuen Sitzung beginnt immer neu: nur claude mit den Hook-Einstellungen (und Optionen),
    # ohne --session-id/--resume/--continue.
    Check 'Erster Start einer neuen Claude-Sitzung: kein --session-id/--resume/--continue (#302)' ($ct -notmatch '--session-id' -and $ct -notmatch '--resume' -and $ct -notmatch '--continue') "calls.txt='$ct'"
    Check 'Schein-Profil: keine globalen Hooks in ~/.claude/settings.json angelegt' (-not (Test-Path "$($script:iso.Profile)\.claude\settings.json"))
    # Schluessel des Starts (#260): der Ersatz schreibt PANELDECK_SESSION_KEY in calls.txt; ohne ihn verwirft der Hub Hooks.
    $hookKey = ''
    foreach ($ln in ($ct -split "`r?`n")) { if ($ln -match 'claude\.cmd' -and $ln -match 'key=([0-9a-f]{16,})') { $hookKey = $Matches[1] } }
    Check 'Schluessel des Starts im Ersatz-claude (PANELDECK_SESSION_KEY)' ($hookKey.Length -ge 16) "calls.txt='$ct'"
    # Konversation des Starts (#302): ohne --session-id ordnet der erste SessionStart des gestarteten Claude sie zu;
    # danach zaehlen Meldungen anderer Konversationen (claude -p) nicht (#260).
    $script:hookSid = [guid]::NewGuid().ToString()
    # $key = $null: Meldung ganz ohne Schluessel (globale Hooks einer aelteren paneldeck.exe).
    function Hook($evt, $extra = @{}, $key = $hookKey, $sid = $script:hookSid) {
      $msg = @{ token = $hookTok; session = [uint64]$cid; payload = (@{ hook_event_name = $evt; session_id = $sid } + $extra) }
      if ($null -ne $key) { $msg.key = $key }
      $body = $msg | ConvertTo-Json -Compress -Depth 5
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
    # #260: Meldungen ohne bzw. mit fremdem Schluessel oder aus einer anderen Konversation aendern den Status nicht.
    $stOf = { ((Tool 'list_sessions').data.sessions | Where-Object { $_.id -eq $cid }).status }
    Hook 'Notification' @{ notification_type = 'permission_prompt'; message = 'gefaelscht' } ''; Start-Sleep 0.5
    Check 'Hook ohne Schluessel wird verworfen (#260)' ((& $stOf) -eq 'ready') "status=$(& $stOf)"
    Hook 'Notification' @{ notification_type = 'permission_prompt'; message = 'gefaelscht' } '0123456789abcdef0123456789abcdef'; Start-Sleep 0.5
    Check 'Hook mit fremdem Schluessel wird verworfen (#260)' ((& $stOf) -eq 'ready') "status=$(& $stOf)"
    Hook 'UserPromptSubmit' @{} $hookKey 'nested-claude-p'; Start-Sleep 0.5
    Check 'Hook einer anderen Konversation (claude -p) wird verworfen (#260)' ((& $stOf) -eq 'ready') "status=$(& $stOf)"
    Hook 'Notification' @{ notification_type = 'permission_prompt'; message = 'alt' } $null; Start-Sleep 0.5
    Check 'Hook ganz ohne Schluessel zaehlt nicht mehr, sobald Meldungen mit Schluessel kamen (#260)' ((& $stOf) -eq 'ready') "status=$(& $stOf)"
    # --- Status ohne passenden Hook: Claude meldet nicht jedes Ende (kein Stop nach Esc, kein Stop fuer Hintergrund-Agenten ...)
    $rowOf = { (Tool 'list_sessions').data.sessions | Where-Object { $_.id -eq $cid } }
    # Wartet bis $ms auf den Status $want; gibt den letzten Status zurueck.
    function Wait-Status([string]$want, [int]$ms = 12000) {
      $t0 = Get-Date
      do { $s = & $stOf; if ($s -eq $want) { return $s }; Start-Sleep -Milliseconds 400 } while (((Get-Date) - $t0).TotalMilliseconds -lt $ms)
      $s
    }
    $askBash = @{ notification_type = 'permission_prompt'; message = 'Claude needs your permission to use Bash' }
    # Hintergrund-Agent nach dem Ende der Runde: Werkzeug-Meldung -> working, sein Ende (SubagentStop) -> ready.
    Hook 'PostToolUse' @{ tool_name = 'Bash'; agent_id = 'bg1' }; Start-Sleep 0.5
    Check 'Hintergrund-Agent arbeitet nach dem Stop -> working' ((& $stOf) -eq 'working') "status=$(& $stOf)"
    Hook 'SubagentStop'; Start-Sleep 0.5
    Check 'SubagentStop ausserhalb einer Runde -> ready (vorher blieb working stehen)' ((& $stOf) -eq 'ready') "status=$(& $stOf)"
    # Automatisches Zusammenfassen mitten in der Runde: SessionStart(compact) heisst nicht "bereit".
    Hook 'UserPromptSubmit'; Hook 'SessionStart' @{ source = 'compact' }; Start-Sleep 0.5
    Check 'SessionStart(compact) mitten in der Runde bleibt working' ((& $stOf) -eq 'working') "status=$(& $stOf)"
    Hook 'SubagentStop'; Start-Sleep 0.3
    Check 'SubagentStop in einer Runde bleibt working' ((& $stOf) -eq 'working') "status=$(& $stOf)"
    Hook 'Stop'; Start-Sleep 0.5
    Hook 'Notification' @{ notification_type = 'auth_success'; message = 'Auth ok' }; Start-Sleep 0.5
    Check 'Benachrichtigung auth_success aendert den Status nicht (vorher input)' ((& $stOf) -eq 'ready') "status=$(& $stOf)"
    # Offene Freigabe-Frage: die Meldung eines anderen, gleichzeitig laufenden Werkzeugs ueberschreibt sie nicht.
    Hook 'UserPromptSubmit'; Hook 'Notification' $askBash; Start-Sleep 0.5
    Check 'Freigabe-Frage -> approval' ((& $stOf) -eq 'approval') "status=$(& $stOf)"
    Hook 'PostToolUse' @{ tool_name = 'Read'; agent_id = 'sub1' }; Hook 'PreToolUse' @{ tool_name = 'Bash' }; Start-Sleep 0.5
    Check 'Meldungen anderer Werkzeuge lassen approval stehen (vorher working)' ((& $stOf) -eq 'approval') "status=$(& $stOf)"
    Hook 'PostToolUse' @{ tool_name = 'Bash' }; Start-Sleep 0.5
    Check 'Das gefragte Werkzeug ist fertig -> working' ((& $stOf) -eq 'working') "status=$(& $stOf)"
    Hook 'Stop'; Start-Sleep 0.5
    # Unterbrechung mit Esc: kein Stop-Hook. Das Transkript belegt das Ende der Runde -> ready; die Warteschlange wartet.
    # Ein Beleg zaehlt nur, wenn er in der Datei HINTER dem Stand bei der letzten Meldung steht (#282): die alte
    # Unterbrechung am Anfang der Datei nicht.
    $u8n = New-Object Text.UTF8Encoding $false
    $trFile = "$work\smoke-transcript.jsonl"
    $trPrompt = '{"type":"user","message":{"role":"user","content":"los"}}' + "`n"
    $trStop = '{"type":"user","message":{"role":"user","content":[{"type":"text","text":"[Request interrupted by user]"}]}}' + "`n"
    [IO.File]::WriteAllText($trFile, ($trPrompt + $trStop), $u8n)
    Hook 'UserPromptSubmit' @{ transcript_path = $trFile }; Start-Sleep 0.5
    $q2 = Tool 'queue_prompt' @{ session = $cid; prompt = 'nach-der-unterbrechung' }
    Check 'queue_prompt waehrend working: wartet' ($q2.data.sent_immediately -eq $false -and $q2.data.queue_len -eq 1) $q2.text
    Start-Sleep 6
    Check 'Ohne Stop bleibt working; eine Unterbrechung von vor der Meldung ist kein Beleg (#282)' ((& $stOf) -eq 'working') "status=$(& $stOf)"
    [IO.File]::AppendAllText($trFile, ($trPrompt + $trStop), $u8n)
    $st = Wait-Status 'ready' 15000
    Check 'Unterbrechung ohne Stop-Hook: Status wird ready (Beleg im Transkript; vorher blieb working stehen)' ($st -eq 'ready') "status=$st"
    Start-Sleep 1
    Check 'Nach der Unterbrechung wartet die Warteschlange (nichts gesendet)' ((& $rowOf).queue_len -eq 1 -and (Wait-FakeFile $inputs 'nach-der-unterbrechung' 1) -notmatch 'nach-der-unterbrechung') "queue_len=$((& $rowOf).queue_len)"
    # Ein Stop ohne abgeschickten Prompt davor (Runde von einem Hintergrund-Ergebnis geweckt) laesst sie auch nicht los (#282).
    Hook 'PreToolUse' @{ tool_name = 'Bash' }; Hook 'Stop'; Start-Sleep 1
    Check 'Stop einer Runde ohne Prompt: Warteschlange wartet weiter (#282)' ((& $rowOf).queue_len -eq 1 -and (& $stOf) -eq 'ready') "queue_len=$((& $rowOf).queue_len) status=$(& $stOf)"
    Hook 'UserPromptSubmit'; Hook 'Stop'
    Check 'Nach dem Stop einer Runde mit Prompt laeuft die Warteschlange weiter' ((Wait-FakeFile $inputs 'nach-der-unterbrechung' 10000) -match 'nach-der-unterbrechung')
    # Runde des gesendeten Prompts (ein zweiter Stop ohne Meldung dazwischen gaelte als doppelt zugestellt).
    Hook 'UserPromptSubmit'; Start-Sleep 0.3; Hook 'Stop'; Start-Sleep 0.5
    Check 'Runde des Warteschlangen-Prompts beendet -> ready' ((& $stOf) -eq 'ready') "status=$(& $stOf)"
    # Vom Hub angenommenes "arbeitet" (send_text mit Enter), das keine Meldung bestaetigt (lokaler Befehl): zurueck auf ready.
    [void](Tool 'send_text' @{ session = $cid; text = '/model' }); Start-Sleep 0.5
    Check 'send_text mit Enter -> working (angenommen)' ((& $stOf) -eq 'working') "status=$(& $stOf)"
    $t0 = Get-Date; $st = Wait-Status 'ready' 20000; $took = ((Get-Date) - $t0).TotalSeconds
    Check 'Unbestaetigtes working faellt auf ready zurueck (nach ca. 10 s, vorher nie)' ($st -eq 'ready' -and $took -ge 6) "status=$st nach $([int]$took) s"
    # #282 (HIGH-1): Ein Programm taeuscht den Prompt der Shell vor (OSC 133;A), waehrend Claude laeuft und eine
    # Freigabe-Frage offen ist. Ohne Bestaetigung (im Vordergrund der Shell laeuft noch etwas) aendert das nichts.
    [void](Tool 'send_text' @{ session = $cid; text = 'spoof-shell-prompt' })
    Hook 'UserPromptSubmit'; Hook 'Notification' $askBash; Start-Sleep 0.5
    Check 'Freigabe-Frage offen (vor dem vorgetaeuschten Prompt)' ((& $stOf) -eq 'approval') "status=$(& $stOf)"
    $t0 = Get-Date; while ((& $rowOf).cwd -notmatch 'spoof-prompt' -and ((Get-Date) - $t0).TotalSeconds -lt 12) { Start-Sleep -Milliseconds 400 }
    Check 'Die vorgetaeuschten Sequenzen kommen im Hub an (Ordnermeldung OSC 9;9)' ((& $rowOf).cwd -match 'spoof-prompt') "cwd=$((& $rowOf).cwd)"
    Start-Sleep 5
    Check 'Vorgetaeuschter Prompt hebt die Freigabe-Frage nicht auf (#282)' ((& $stOf) -eq 'approval') "status=$(& $stOf)"
    Check 'Vorgetaeuschter Prompt: send_text bleibt verweigert (#282)' ((Code (Tool 'send_text' @{ session = $cid; text = 'ja' })) -eq 'permission_pending')
    Hook 'SessionStart' @{ source = 'startup' } $hookKey 'nested-after-spoof'; Start-Sleep 0.5
    Check 'Vorgetaeuschter Prompt: ein claude -p uebernimmt die Sitzung nicht (#282, #260)' ((& $stOf) -eq 'approval') "status=$(& $stOf)"
    Hook 'PostToolUse' @{ tool_name = 'Bash' }; Start-Sleep 0.5
    Check 'Vorgetaeuschter Prompt: Meldungen der Konversation zaehlen weiter (#282)' ((& $stOf) -eq 'working') "status=$(& $stOf)"
    Hook 'Stop'; Start-Sleep 0.5
    # #282 (MEDIUM-3/4): Solange unten im Bild eine Rueckfrage steht, macht weder "Claude wartet auf deine Eingabe"
    # (idle_prompt) noch ein Beleg im Transkript aus approval ein ready. Die Zeile im Ersatz-claude sieht so aus.
    [void](Tool 'send_text' @{ session = $cid; text = 'Do you want to proceed?' }); Start-Sleep 0.5
    Hook 'UserPromptSubmit' @{ transcript_path = $trFile }; Hook 'Notification' $askBash; Start-Sleep 0.5
    Hook 'Notification' @{ notification_type = 'idle_prompt'; message = 'Claude is waiting for your input' }; Start-Sleep 0.5
    Check 'idle_prompt laesst eine sichtbare Rueckfrage stehen (#282)' ((& $stOf) -eq 'approval') "status=$(& $stOf)"
    [IO.File]::AppendAllText($trFile, ($trPrompt + $trStop), $u8n)
    Start-Sleep 6
    Check 'Beleg im Transkript beendet eine sichtbare Rueckfrage nicht (#282)' ((& $stOf) -eq 'approval') "status=$(& $stOf)"
    Hook 'Stop'; Start-Sleep 0.5
    Check 'Stop beendet die Rueckfrage -> ready' ((& $stOf) -eq 'ready') "status=$(& $stOf)"

    # Claude beendet (SessionEnd) und im selben Terminal neu gestartet: neue Konversation wird wieder zugeordnet (Review
    # zu #260). Die Warteschlange wird angehalten und laeuft fuer den neuen Claude nicht von selbst weiter (#282): ein
    # `claude -p` naehme den Prompt nicht an, und die Shell fuehrte ihn danach als Befehl aus.
    Hook 'UserPromptSubmit'; Start-Sleep 0.3
    $q3 = Tool 'queue_prompt' @{ session = $cid; prompt = 'nach-dem-ende' }
    Check 'queue_prompt vor dem Ende von Claude: wartet' ($q3.data.sent_immediately -eq $false -and $q3.data.queue_len -eq 1) $q3.text
    Hook 'SessionEnd' @{ reason = 'prompt_input_exit' }; Start-Sleep 0.3
    $script:hookSid = [guid]::NewGuid().ToString()
    Hook 'SessionStart' @{ source = 'startup' }; Start-Sleep 0.5
    Check 'Neu gebundener Claude meldet sich bereit' ((& $stOf) -eq 'ready') "status=$(& $stOf)"
    Hook 'UserPromptSubmit'; Start-Sleep 0.5
    Check 'Neu gestarteter Claude im selben Terminal wird zugeordnet (UserPromptSubmit -> working)' ((& $stOf) -eq 'working') "status=$(& $stOf)"
    Hook 'Stop'; Start-Sleep 1.5
    Check 'Stop des neuen Claude -> ready' ((& $stOf) -eq 'ready') "status=$(& $stOf)"
    Check 'Nach dem Ende von Claude laeuft die Warteschlange nicht von selbst weiter (#282)' ((& $rowOf).queue_len -eq 1 -and (Wait-FakeFile $inputs 'nach-dem-ende' 1) -notmatch 'nach-dem-ende') "queue_len=$((& $rowOf).queue_len)"
    # Claude endet ohne SessionEnd (abgeschossen): die Shell steht wieder am Prompt und in ihrem Vordergrund laeuft nichts
    # mehr -> Status weg, neuer Claude wird zugeordnet.
    [void](Tool 'send_text' @{ session = $cid; text = 'exit-fake-claude' })
    $st = Wait-Status 'unknown' 12000
    Check 'Claude beendet ohne SessionEnd: Status faellt am (bestaetigten) Prompt der Shell weg' ($st -eq 'unknown') "status=$st"
    Hook 'Stop'; Start-Sleep 0.5
    Check 'Nachzuegler des beendeten Claude zaehlt nicht' ((& $stOf) -eq 'unknown') "status=$(& $stOf)"
    $script:hookSid = [guid]::NewGuid().ToString()
    Hook 'SessionStart' @{ source = 'startup' }; Start-Sleep 0.5
    Check 'Neuer Claude nach einem Ende ohne SessionEnd wird zugeordnet (vorher fuer immer fremd)' ((& $stOf) -eq 'ready') "status=$(& $stOf)"
    Hook 'UserPromptSubmit'; Start-Sleep 0.4; Hook 'Stop'; Start-Sleep 0.5
    Check 'Der neue Claude meldet seinen Status' ((& $stOf) -eq 'ready') "status=$(& $stOf)"
    Check 'Die Warteschlange bleibt angehalten (nichts in die Shell getippt)' ((& $rowOf).queue_len -eq 1 -and (Wait-FakeFile $inputs 'nach-dem-ende' 1) -notmatch 'nach-dem-ende') "queue_len=$((& $rowOf).queue_len)"

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
    $re = '(?m)args=.*--model haiku --effort high --permission-mode plan --agent smoke-agent\r?$'
    $ct = Wait-FakeFile $calls $re
    Check 'Ersatz-claude bekommt die geprueften Optionen (fakebin\calls.txt)' ($ct -match $re) "calls.txt='$ct'"
    [void](Tool 'close_session' @{ session = $coid })
  }
  # Ohne permission_mode: ausdruecklich "default" (sonst gaelte permissions.defaultMode aus ~/.claude/settings.json).
  $cd = Tool 'create_session' @{ claude = $true; shell = 'powershell'; cwd = "$proj"; name = 'smoke-copts2'; model = 'sonnet' }
  Check 'create_session ohne permission_mode meldet "default"' (-not $cd.err -and $cd.data.permission_mode -eq 'default') $cd.text
  if ($script:lastOk) {
    $re2 = '(?m)args=.*--model sonnet --permission-mode default\r?$'
    $ct2 = Wait-FakeFile $calls $re2
    Check 'Ersatz-claude bekommt --permission-mode default' ($ct2 -match $re2) "calls.txt='$ct2'"
    [void](Tool 'close_session' @{ session = $cd.data.id })
  }
  Check 'kein bypassPermissions in fakebin\calls.txt' ((Wait-FakeFile $calls 'bypassPermissions' 1) -notmatch 'bypassPermissions|dangerously')
  $pl = Tool 'create_session' @{ shell = 'cmd'; cwd = "$proj"; name = 'smoke-plain' }
  $plr = (Tool 'list_sessions').data.sessions | Where-Object { $_.id -eq $pl.data.id }
  Check 'list_sessions: ohne Claude sind die Optionen null' ($plr.PSObject.Properties.Name -contains 'effort' -and $null -eq $plr.model -and $null -eq $plr.effort -and $null -eq $plr.permission_mode -and $null -eq $plr.agent)
  Check 'list_sessions: ohne Claude keine claude_flags/claude_user_options' ($plr.PSObject.Properties.Name -contains 'claude_flags' -and @($plr.claude_flags).Count -eq 0 -and @($plr.claude_user_options).Count -eq 0)
  [void](Tool 'close_session' @{ session = $pl.data.id })

  Write-Host "== Weitere Claude-Optionen (#182)" -ForegroundColor Cyan
  Check 'create_session: claude_flags ohne claude -> bad_args' ((Code (Tool 'create_session' @{ shell = 'cmd'; claude_flags = @('chrome') })) -eq 'bad_args')
  foreach ($bad in @('add-dir', '--dangerously-skip-permissions', 'mcp-config', 'allowed-tools', 'resume', 'settings', 'remote-control', 'safe-mode', 'autocompact', 'plugin-dir', '--strict-mcp-config', 'system-prompt')) {
    $r = Tool 'create_session' @{ claude = $true; cwd = "$proj"; claude_flags = @($bad) }
    Check "create_session: claude_flags [$bad] -> bad_args (never allowed)" ((Code $r) -eq 'bad_args' -and $r.text -match 'never allowed via MCP') $r.text
  }
  $r = Tool 'create_session' @{ claude = $true; cwd = "$proj"; claude_flags = @('chrome', 'no-chrome') }
  Check 'create_session: chrome + no-chrome -> bad_args' ((Code $r) -eq 'bad_args' -and $r.text -match 'mutually exclusive') $r.text
  Check 'create_session: claude_flags als String -> bad_args' ((Code (Tool 'create_session' @{ claude = $true; cwd = "$proj"; claude_flags = 'chrome' })) -eq 'bad_args')
  Check 'create_session: unbekannter Schalter -> bad_args' ((Code (Tool 'create_session' @{ claude = $true; cwd = "$proj"; claude_flags = @('chrome; calc') })) -eq 'bad_args')
  foreach ($k in @('add_dir', 'mcp_config', 'append_system_prompt', 'allowed_tools', 'safe_mode', 'plugin_dir', 'plugin_dirs', 'mcp_configs', 'strict_mcp_config', 'system_prompt')) {
    $r = Tool 'create_session' @{ claude = $true; cwd = "$proj"; $k = "$proj" }
    Check "create_session: Parameter $k -> bad_args" ((Code $r) -eq 'bad_args' -and $r.text -match 'not allowed via MCP') $r.text
  }
  Check 'create_session (Chat): claude_flags -> bad_args' ((Code (Tool 'create_session' @{ kind = 'chat'; claude_flags = @('ide') })) -eq 'bad_args')
  $cf = Tool 'create_session' @{ claude = $true; shell = 'powershell'; cwd = "$proj"; name = 'smoke-flags'; claude_flags = @('verbose', '--chrome', 'ide') }
  Check 'create_session mit claude_flags' (-not $cf.err -and (@($cf.data.claude_flags) -join ',') -eq 'chrome,ide,verbose') $cf.text
  if ($script:lastOk) {
    $cfl = (Tool 'list_sessions').data.sessions | Where-Object { $_.id -eq $cf.data.id }
    Check 'list_sessions meldet claude_flags' ((@($cfl.claude_flags) -join ',') -eq 'chrome,ide,verbose' -and @($cfl.claude_user_options).Count -eq 0) ($cfl | ConvertTo-Json -Compress)
    $re3 = '(?m)args=.*--permission-mode default --chrome --ide --verbose\r?$'
    $ct3 = Wait-FakeFile $calls $re3
    Check 'Ersatz-claude bekommt --chrome --ide --verbose (fakebin\calls.txt)' ($ct3 -match $re3) "calls.txt='$ct3'"
    [void](Tool 'close_session' @{ session = $cf.data.id })
  }
  Check 'keine verbotenen Flags in fakebin\calls.txt' ((Wait-FakeFile $calls 'add-dir|mcp-config|dangerously| --allowed-tools|remote-control' 1) -notmatch 'add-dir|mcp-config|dangerously| --allowed-tools|remote-control')

  # Standards des Nutzers mit Rechte erweiternden Wertoptionen: gelten fuer Sitzungen des Nutzers (hier: --open --claude
  # von aussen), nie fuer Sitzungen eines Agenten. Neustart mit vorbereiteter settings.json.
  Write-Host "== Weitere Claude-Optionen: Standards des Nutzers (#182)" -ForegroundColor Cyan
  $mcpCfg = "$work\smoke-mcp-config.json"
  [IO.File]::WriteAllText($mcpCfg, '{"mcpServers":{}}', (New-Object Text.UTF8Encoding $false))
  # #289: mehrere MCP-Konfigurationen, Plugin-Ordner, --strict-mcp-config und ein eigener (mehrzeiliger) System-Prompt.
  $mcpCfg2 = "$work\smoke-mcp-config2.json"
  [IO.File]::WriteAllText($mcpCfg2, '{"mcpServers":{}}', (New-Object Text.UTF8Encoding $false))
  $plugDir = "$work\smoke-plugin"
  New-Item -ItemType Directory -Force $plugDir | Out-Null
  $script:prestart = {
    $f = "$data\settings.json"; $j = [IO.File]::ReadAllText($f).TrimStart([char]0xFEFF) | ConvertFrom-Json
    $d = [pscustomobject]@{ flags = @('verbose', 'no-chrome', 'safe-mode'); add_dirs = @("$proj", "$work\smoke-missing-dir"); plugin_dirs = @($plugDir, "$work\smoke-missing-plugin"); allowed_tools = 'Read'; disallowed_tools = 'WebFetch'; mcp_configs = @($mcpCfg, $mcpCfg2); strict_mcp_config = $true; system_prompt = "Smoke prompt`nsecond line"; fallback_model = 'haiku' }
    $j | Add-Member -Force claude_defaults $d
    $j | Add-Member -Force default_shell 'powershell'
    [IO.File]::WriteAllText($f, ($j | ConvertTo-Json -Depth 10), (New-Object Text.UTF8Encoding $false))
    # Terminal-Vorlagen (#244): mit Isolationsprofil 1 (Schein-PATH); riskante Optionen, die ein Agent nie bekommt.
    $tpl = @(
      [pscustomobject]@{ name = 'smoke-tpl'; terminal = $true; grid = $false; items = @([pscustomobject]@{ name = 'API'; shell_id = 'powershell'; cwd = "$proj"; claude = $true; profile = 1; group = 'tplgrp'; prompt = ''; tags = @('smoke'); color = @(1, 2, 3); notes = 'tpl-note'; claude_opts = [pscustomobject]@{ model = 'haiku'; permission_mode = 'bypassPermissions'; flags = @('ide', 'safe-mode'); add_dirs = @("$proj"); plugin_dirs = @($plugDir); allowed_tools = 'Read'; mcp_config = $mcpCfg; system_prompt = 'Template prompt' } }) },
      [pscustomobject]@{ name = 'smoke-plain'; terminal = $true; items = @([pscustomobject]@{ name = ''; shell_id = 'cmd'; cwd = "$proj"; claude = $false; profile = 1; group = 'tplplain' }) },
      [pscustomobject]@{ name = 'smoke-admin'; terminal = $true; items = @([pscustomobject]@{ name = 'adm'; shell_id = 'cmd'; cwd = "$proj"; claude = $false; profile = 77 }) },
      [pscustomobject]@{ name = 'smoke-group'; items = @([pscustomobject]@{ name = 'a'; shell_id = 'cmd'; profile = 1 }, [pscustomobject]@{ name = 'b'; shell_id = 'cmd'; profile = 1 }) },
      # Profile mit eigenem Programm bzw. gefaehrlichem Startbefehl (Review #244): fuer Agenten nie.
      [pscustomobject]@{ name = 'smoke-custom'; terminal = $true; items = @([pscustomobject]@{ name = 'cust'; shell_id = 'cmd'; cwd = "$proj"; claude = $false; profile = 78 }) },
      [pscustomobject]@{ name = 'smoke-danger'; terminal = $true; items = @([pscustomobject]@{ name = 'dng'; shell_id = 'cmd'; cwd = "$proj"; claude = $false; profile = 79 }) }
    )
    [IO.File]::WriteAllText("$data\templates.json", (ConvertTo-Json -InputObject $tpl -Depth 10), (New-Object Text.UTF8Encoding $false))
  }
  $script:extraProfiles = @(
    [pscustomobject]@{ id = 77; name = 'Smoke-Admin'; env = $ptyEnv; admin = $true },
    [pscustomobject]@{ id = 78; name = 'Smoke-Custom'; env = $ptyEnv; program = "$env:SystemRoot\System32\cmd.exe"; args = '/k echo smoke-custom' },
    [pscustomobject]@{ id = 79; name = 'Smoke-Danger'; env = $ptyEnv; startup = 'claude --dangerously-skip-permissions' }
  )
  Set-Level 'Control'
  $script:extraProfiles = @()
  # Beim naechsten Neustart die Standards wieder entfernen (spaetere Abschnitte erwarten keine).
  $script:prestart = {
    $f = "$data\settings.json"; $j = [IO.File]::ReadAllText($f).TrimStart([char]0xFEFF) | ConvertFrom-Json
    $j.PSObject.Properties.Remove('claude_defaults'); $j.PSObject.Properties.Remove('default_shell')
    [IO.File]::WriteAllText($f, ($j | ConvertTo-Json -Depth 10), (New-Object Text.UTF8Encoding $false))
    Remove-Item -LiteralPath "$data\templates.json" -ErrorAction SilentlyContinue
  }
  [void](Rpc 'initialize' @{ protocolVersion = '2025-06-18'; capabilities = @{}; clientInfo = @{ name = 'smoke'; version = '1' } })
  # Nach dem Neustart erneut bestaetigen, dass claude im PowerShell-Terminal nur der Ersatz ist.
  $iso2 = Tool 'create_session' @{ shell = 'powershell'; cwd = "$proj"; name = 'smoke-iso2' }
  Start-Sleep 4
  $r = Tool 'run_command' @{ session = $iso2.data.id; command = 'Get-Command claude -All | ForEach-Object { if ($_.CommandType -in ''Application'', ''ExternalScript'') { $_.Source } else { "$($_.CommandType):$($_.Name)" } }'; timeout_secs = 25 }
  Check 'Isolation nach Neustart: claude im PowerShell-Terminal ist nur der Ersatz' (Only-Fake $r.data.output) $r.data.output
  [void](Tool 'close_session' @{ session = $iso2.data.id })
  if (-not $script:lastOk) { throw 'SICHERHEIT: claude nach dem Neustart nicht als Ersatz bestaetigt - keine Claude-Sitzung' }
  $chr = Tool 'create_session' @{ claude = $true; shell = 'powershell'; cwd = "$proj"; claude_flags = @('chrome') }
  Check 'Agent: chrome gegen no-chrome des Nutzers -> bad_args' ((Code $chr) -eq 'bad_args' -and $chr.text -match 'turned Chrome off') $chr.text
  $ag = Tool 'create_session' @{ claude = $true; shell = 'powershell'; cwd = "$proj"; name = 'smoke-risky'; claude_flags = @('ide') }
  $uo = @($ag.data.claude_user_options)
  Check 'Agenten-Sitzung: Schalter des Nutzers + eigene, nur harmlose Wertoptionen' (-not $ag.err -and (@($ag.data.claude_flags) -join ',') -eq 'no-chrome,ide,verbose' -and ($uo -join ',') -eq 'disallowed_tools,fallback_model') $ag.text
  if ($script:lastOk) {
    $re4 = '(?m)args=.*--permission-mode default --no-chrome --ide --verbose --disallowed-tools WebFetch --fallback-model haiku\r?$'
    $ct4 = Wait-FakeFile $calls $re4
    $line4 = @(($ct4 -split "`r?`n") | Where-Object { $_ -match $re4 }) | Select-Object -Last 1
    Check 'Ersatz-claude (Agent): ohne --add-dir/--plugin-dir/--allowed-tools/--mcp-config/--strict-mcp-config/--system-prompt/--safe-mode' ($line4 -and $line4 -notmatch '--add-dir|--plugin-dir|--mcp-config|--strict-mcp-config|--system-prompt| --allowed-tools|--safe-mode') "calls.txt='$ct4'"
    [void](Tool 'close_session' @{ session = $ag.data.id })
  }
  Write-Host "== Terminal-Vorlagen: create_session mit template (#244)" -ForegroundColor Cyan
  $t1 = Tool 'create_session' @{ template = 'smoke-tpl' }
  Check 'create_session mit template: Name, Ordner, Shell und Claude aus der Vorlage' (-not $t1.err -and $t1.data.name -eq 'API' -and $t1.data.group -eq 'tplgrp' -and $t1.data.template -eq 'smoke-tpl' -and $t1.data.claude -and $t1.data.shell -eq 'powershell') $t1.text
  $tuo = @($t1.data.claude_user_options); $tfl = @($t1.data.claude_flags)
  Check 'create_session mit template: Optionen der Vorlage, nie bypassPermissions/--safe-mode/riskante Wertoptionen' (-not $t1.err -and $t1.data.model -eq 'haiku' -and $t1.data.permission_mode -eq 'default' -and $tfl -contains 'ide' -and $tfl -notcontains 'safe-mode' -and -not ($tuo | Where-Object { $_ -in 'add_dir', 'plugin_dir', 'allowed_tools', 'mcp_config', 'strict_mcp_config', 'system_prompt' })) $t1.text
  if ($script:lastOk) {
    $re6 = '(?m)args=.*--model haiku --permission-mode default .*\r?$'
    $ct6 = Wait-FakeFile $calls $re6
    $line6 = @(($ct6 -split "`r?`n") | Where-Object { $_ -match $re6 }) | Select-Object -Last 1
    Check 'Ersatz-claude (Vorlage per Agent): ohne --add-dir/--plugin-dir/--allowed-tools/--mcp-config/--system-prompt/--safe-mode/bypassPermissions' ($line6 -and $line6 -notmatch '--add-dir|--plugin-dir|--mcp-config|--system-prompt| --allowed-tools|--safe-mode|bypass') "calls.txt='$ct6'"
  }
  $t2 = Tool 'create_session' @{ template = 'SMOKE-TPL'; claude = $false }
  Check 'template: zweiter Start zaehlt hoch (API #2), Name ohne Gross-/Kleinschreibung, claude=false gewinnt' (-not $t2.err -and $t2.data.name -eq 'API #2' -and -not $t2.data.claude) $t2.text
  $t3 = Tool 'create_session' @{ template = 'smoke-plain'; name = 'tpl-eigener-name'; group = 'tplg2' }
  Check 'template: Parameter des Aufrufs gehen vor (name, group), Shell aus der Vorlage' (-not $t3.err -and $t3.data.name -eq 'tpl-eigener-name' -and $t3.data.group -eq 'tplg2' -and $t3.data.shell -eq 'cmd' -and -not $t3.data.claude) $t3.text
  $t4 = Tool 'create_session' @{ template = 'smoke-plain' }
  Check 'template ohne Sitzungsnamen: Name der Vorlage' (-not $t4.err -and $t4.data.name -eq 'smoke-plain' -and $t4.data.group -eq 'tplplain') $t4.text
  $u = Tool 'create_session' @{ template = 'gibt-es-nicht' }
  Check 'template: unbekannte Vorlage -> bad_args mit den Terminal-Vorlagen' ((Code $u) -eq 'bad_args' -and $u.text -match 'smoke-tpl' -and $u.text -notmatch 'smoke-group') $u.text
  $g = Tool 'create_session' @{ template = 'smoke-group' }
  Check 'template: Gruppen-Vorlage -> bad_args' ((Code $g) -eq 'bad_args' -and $g.text -match 'group template') $g.text
  $ad = Tool 'create_session' @{ template = 'smoke-admin' }
  Check 'template: Administrator-Profil der Vorlage -> admin_profile' ((Code $ad) -eq 'admin_profile') $ad.text
  $cu = Tool 'create_session' @{ template = 'smoke-custom' }
  Check 'template: Profil mit eigenem Programm (nicht Standardprofil) -> bad_args' ((Code $cu) -eq 'bad_args' -and $cu.text -match 'own program') $cu.text
  $dg = Tool 'create_session' @{ template = 'smoke-danger' }
  Check 'template: Profil mit dangerously im Startbefehl -> bad_args' ((Code $dg) -eq 'bad_args' -and $dg.text -match 'dangerously') $dg.text
  $ign = @($t1.data.template_ignored)
  Check 'template: template_ignored nennt die weggelassenen Optionen' (-not $t1.err -and $ign -contains 'add_dir' -and $ign -contains 'allowed_tools' -and $ign -contains 'mcp_config' -and $ign -contains 'safe-mode' -and $ign -contains 'plugin_dir' -and $ign -contains 'system_prompt') $t1.text
  Check 'template ohne riskante Optionen: template_ignored leer' (-not $t4.err -and ($t4.data.PSObject.Properties.Name -contains 'template_ignored') -and @($t4.data.template_ignored).Count -eq 0) $t4.text
  Check 'template mit kind=chat -> bad_args' ((Code (Tool 'create_session' @{ template = 'smoke-plain'; kind = 'chat' })) -eq 'bad_args')
  foreach ($x in @($t1, $t2, $t3, $t4)) { if ($x.data.id) { [void](Tool 'close_session' @{ session = $x.data.id }) } }
  # Sitzung des Nutzers: Start von aussen (wie aus dem Explorer) per --open --claude -> alle Standards.
  $before = @((Tool 'list_sessions').data.sessions | ForEach-Object id)
  # Der zweite Aufruf reicht nur per IPC an den laufenden Hub weiter und endet sofort (sonst nach 15 s beenden).
  $op = Start-Process "$dir\paneldeck.exe" -ArgumentList '--open', "`"$proj`"", '--claude' -WindowStyle Hidden -PassThru
  if (-not $op.WaitForExit(15000)) { $op.Kill() }
  $us = $null; $t0 = Get-Date
  while (-not $us -and ((Get-Date) - $t0).TotalSeconds -lt 15) { Start-Sleep -Milliseconds 500; $us = (Tool 'list_sessions').data.sessions | Where-Object { $before -notcontains $_.id -and $_.claude } | Select-Object -First 1 }
  Check 'Sitzung des Nutzers (--open --claude): alle Wertoptionen der Standards' ($us -and (@($us.claude_user_options) -join ',') -eq 'add_dir,plugin_dir,allowed_tools,disallowed_tools,mcp_config,strict_mcp_config,system_prompt,fallback_model' -and (@($us.claude_flags) -join ',') -eq 'no-chrome,verbose,safe-mode') ($us | ConvertTo-Json -Compress)
  if ($script:lastOk) {
    $re5 = '(?m)args=.*--no-chrome --verbose --safe-mode --add-dir "?' + [regex]::Escape("$proj") + '"? --plugin-dir "?' + [regex]::Escape($plugDir) + '"? --allowed-tools Read --disallowed-tools WebFetch --mcp-config "?' + [regex]::Escape($mcpCfg) + '"? "?' + [regex]::Escape($mcpCfg2) + '"? --strict-mcp-config --system-prompt "Smoke prompt second line" --fallback-model haiku\r?$'
    $ct5 = Wait-FakeFile $calls $re5
    Check 'Ersatz-claude (Nutzer) bekommt --add-dir/--plugin-dir/--allowed-tools/mehrere --mcp-config/--strict-mcp-config/--system-prompt gequotet (fakebin\calls.txt)' ($ct5 -match $re5) "calls.txt='$ct5'"
    [void](Tool 'close_session' @{ session = $us.id })
  }

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
    $expectTools = @('list_projects', 'get_project', 'search_tickets', 'get_ticket', 'get_time_entries', 'list_sprints', 'get_sprint', 'list_versions', 'maintenance_report', 'create_ticket', 'update_ticket', 'transition_ticket', 'add_comment', 'log_time', 'link_ticket', 'link_terminal')
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
    Write-Host "== Wartung: maintenance_report, Archiv, busy (#308)" -ForegroundColor Cyan
    # Zweites Projekt MNT mit bekannten Problemen: MNT-1 nie begonnen und alt (ignoriert), MNT-2 in Arbeit und alt, MNT-3/MNT-4
    # fast gleiche Titel, MNT-5 archiviert (taucht nirgends auf), MNT-6 mit geloeschtem Eltern-Ticket.
    $seedM = @'
import sqlite3, sys, datetime
c = sqlite3.connect(sys.argv[1])
fresh = datetime.datetime.now().strftime('%Y-%m-%dT%H:%M:%S')
old = '2024-01-01T10:00:00'
c.execute("INSERT INTO projects (key,name,description,color,work_dir,use_sprints,use_versions,next_number,position,created_at) VALUES ('MNT','Maint','','#335577','',0,0,7,2,?)", (fresh,))
pid = c.execute("SELECT id FROM projects WHERE key='MNT'").fetchone()[0]
st = {}
for i, (n, cat) in enumerate([('To Do', 'todo'), ('In Progress', 'progress'), ('Done', 'done')]):
    c.execute("INSERT INTO statuses (project_id,name,color,category,position) VALUES (?,?,?,?,?)", (pid, n, '#888888', cat, i))
    st[cat] = c.execute('SELECT last_insert_rowid()').fetchone()[0]
c.execute("INSERT INTO ticket_types (project_id,name,color,workflow,system,position) VALUES (?,?,?,1,0,0)", (pid, 'Task', '#3366cc'))
tt = c.execute('SELECT last_insert_rowid()').fetchone()[0]
for i, cat in enumerate(['todo', 'progress', 'done']):
    c.execute("INSERT INTO type_statuses (type_id,status_id,position) VALUES (?,?,?)", (tt, st[cat], i))
c.execute("INSERT INTO work_kinds (project_id,name,position) VALUES (?,?,0)", (pid, 'Development'))
rows = [(1, 'Write the onboarding guide', 'todo', old, 0), (2, 'Refactor the session store', 'progress', old, 0),
        (3, 'Login fails on Safari', 'todo', fresh, 0), (4, 'Login fails in Safari', 'todo', fresh, 0),
        (5, 'Archived old item', 'todo', old, 1), (6, 'Child without parent', 'todo', fresh, 0)]
for n, title, cat, upd, arch in rows:
    c.execute("INSERT INTO tickets (project_id,number,type_id,title,status_id,rank,created_at,updated_at,archived) VALUES (?,?,?,?,?,?,?,?,?)", (pid, n, tt, title, st[cat], n, old, upd, arch))
c.execute('PRAGMA foreign_keys=OFF')
c.execute("UPDATE tickets SET parent_id = 99999 WHERE project_id = ? AND number = 6", (pid,))
c.execute("INSERT INTO maintenance_ignored (project_id, finding, ignored_at) VALUES (?, 'stale_new:MNT-1', ?)", (pid, fresh))
c.execute("INSERT INTO maintenance_settings (project_id, wont_do_status_id) VALUES (?, ?)", (pid, st['done']))
c.commit()
c.close()
'@
    $seedMFile = "$work\seed-maint.py"; [IO.File]::WriteAllText($seedMFile, $seedM, (New-Object Text.UTF8Encoding $false))
    $script:prestart = { $env:PYTHONUTF8 = '1'; & python -I $seedMFile "$data\paneldeck.db"; if ($LASTEXITCODE -ne 0) { throw 'seed-maint.py fehlgeschlagen' } }
    Set-Level 'Read'
    $script:prestart = $null
    [void](Rpc 'initialize' @{ protocolVersion = '2025-06-18'; capabilities = @{}; clientInfo = @{ name = 'smoke'; version = '1' } })
    $names = @((Rpc 'tools/list').result.tools | ForEach-Object name)
    Check 'Wartung: maintenance_report ab Stufe Nur lesen in tools/list' ($names -contains 'maintenance_report')
    $mr = Tool 'maintenance_report' @{ project = 'MNT'; category = 'tickets' }
    $mids = @($mr.data.findings | ForEach-Object id | Sort-Object)
    Check 'maintenance_report: Ticket-Befunde wie erwartet (ignorierter fehlt)' (-not $mr.err -and $mr.data.total -eq 3 -and ($mids -join ',') -eq 'orphaned_subtask:MNT-6,possible_duplicate:MNT-3:MNT-4,stale_in_progress:MNT-2' -and $mr.data.ignored_hidden -eq 1 -and $mr.data.truncated -eq $false) "$($mids -join ',') total=$($mr.data.total) hidden=$($mr.data.ignored_hidden)"
    $dup = $mr.data.findings | Where-Object kind -eq 'possible_duplicate'
    Check 'maintenance_report: Duplikat mit Partner, Schwere und Kategorie' ($dup.ticket -eq 'MNT-3' -and $dup.other_ticket -eq 'MNT-4' -and $dup.severity -eq 'hint' -and $dup.category -eq 'tickets' -and $mr.data.counts.tickets -eq 3)
    Check 'maintenance_report: archiviertes Ticket taucht nicht auf' (@($mr.data.findings | Where-Object { $_.ticket -eq 'MNT-5' }).Count -eq 0)
    $mi = Tool 'maintenance_report' @{ project = 'MNT'; category = 'tickets'; include_ignored = $true }
    Check 'maintenance_report: include_ignored zeigt den ignorierten Befund' ($mi.data.total -eq 4 -and @($mi.data.findings | Where-Object { $_.id -eq 'stale_new:MNT-1' -and $_.ignored }).Count -eq 1) "total=$($mi.data.total)"
    $ml = Tool 'maintenance_report' @{ project = 'MNT'; category = 'tickets'; limit = 1 }
    Check 'maintenance_report: limit kuerzt mit truncated und total' ($ml.data.returned -eq 1 -and $ml.data.truncated -eq $true -and $ml.data.total -eq 3)
    $me = Tool 'maintenance_report' @{ project = 'MNT'; category = 'tickets'; severity = 'error' }
    Check 'maintenance_report: Filter Schwere (error = verwaiste Unteraufgabe)' ($me.data.total -eq 1 -and $me.data.findings[0].kind -eq 'orphaned_subtask')
    $ma = Tool 'maintenance_report' @{ project = 'MNT' }
    Check 'maintenance_report: ohne Filter alle Kategorien, hoechstens 200' (-not $ma.err -and $ma.data.total -ge 3 -and $ma.data.returned -le 200 -and $null -ne $ma.data.counts.database -and $null -ne $ma.data.counts.configuration)
    Check 'maintenance_report: System-Projekt TODO -> forbidden' ((Code (Tool 'maintenance_report' @{ project = 'TODO' })) -eq 'forbidden')
    Check 'maintenance_report: unbekannte Kategorie -> bad_args' ((Code (Tool 'maintenance_report' @{ project = 'MNT'; category = 'nope' })) -eq 'bad_args')
    $sa = Tool 'search_tickets' @{ project = 'MNT' }
    $sb = Tool 'search_tickets' @{ project = 'MNT'; include_archived = $true }
    Check 'search_tickets: archivierte nur mit include_archived, Feld archived' ($sa.data.total -eq 5 -and $sb.data.total -eq 6 -and @($sb.data.tickets | Where-Object { $_.key -eq 'MNT-5' -and $_.archived -eq $true }).Count -eq 1 -and @($sa.data.tickets | Where-Object { $_.archived -eq $false }).Count -eq 5) "ohne=$($sa.data.total) mit=$($sb.data.total)"
    Check 'search_tickets: query archived = true' ((Tool 'search_tickets' @{ project = 'MNT'; query = 'archived = true' }).data.total -eq 1)
    Check 'get_ticket: Feld archived' ((Tool 'get_ticket' @{ ticket = 'MNT-5' }).data.archived -eq $true)

    # Schreibsperre der Wartung: PANELDECK_TEST_MAINT_BUSY=1 haelt sie dauerhaft (nur in der Umgebung dieses Hub-Starts)
    $env:PANELDECK_TEST_MAINT_BUSY = '1'
    try { Set-Level 'Control' } finally { Remove-Item Env:PANELDECK_TEST_MAINT_BUSY -ErrorAction SilentlyContinue }
    [void](Rpc 'initialize' @{ protocolVersion = '2025-06-18'; capabilities = @{}; clientInfo = @{ name = 'smoke'; version = '1' } })
    $bz = Tool 'create_ticket' @{ project = 'MNT'; type = 'Task'; title = 'waehrend der Wartung' }
    Check 'Wartung schreibt: create_ticket -> busy mit retry_after_ms' ((Code $bz) -eq 'busy' -and $bz.data.error.details.retry_after_ms -eq 2000) $bz.text
    Check 'Wartung schreibt: add_comment -> busy' ((Code (Tool 'add_comment' @{ ticket = 'MNT-3'; body = 'x' })) -eq 'busy')
    Check 'Wartung schreibt: Lesen geht weiter (get_ticket, maintenance_report)' (-not (Tool 'get_ticket' @{ ticket = 'MNT-3' }).err -and -not (Tool 'maintenance_report' @{ project = 'MNT'; category = 'tickets' }).err)
    Check 'Wartung schreibt: nichts angelegt' ((Tool 'search_tickets' @{ project = 'MNT'; include_archived = $true }).data.total -eq 6)
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

  Write-Host "== Fortsetzen nach Neustart (#302): Programmstart mit Fortsetzen/Aus/Auswaehlen" -ForegroundColor Cyan
  if (-not $script:psClaudeFake) { throw 'SICHERHEIT: claude im PowerShell-Terminal des Hubs nicht als Ersatz bestaetigt - keine Claude-Sitzung' }
  Stop-Hub
  # Stand der Chat-Pruefung oben merken (die folgenden Abschnitte brauchen ihn) und am Ende zurueckschreiben.
  $prevState = [IO.File]::ReadAllText("$data\state.json")
  # Je Sitzung ein eigener Arbeitsordner (die Zeilen in calls.txt unterscheiden sich an cwd=). Konversationen liegen im
  # Schein-Profil unter .claude\projects\<Ordnername> (wie Claude Code: alles ausser a-z A-Z 0-9 wird '-').
  $rsDirs = @{}
  foreach ($n in 'r302-cont', 'r302-empty', 'r302-pick', 'r302-off') { $rsDirs[$n] = (New-Item -ItemType Directory -Force "$work\$n").FullName.TrimEnd('\') }
  $slugOf = { param($p) $p -replace '[^A-Za-z0-9]', '-' }
  $u8r = New-Object Text.UTF8Encoding $false
  foreach ($n in 'r302-cont', 'r302-off') {
    $pd = Join-Path "$($script:iso.Profile)\.claude\projects" (& $slugOf $rsDirs[$n])
    New-Item -ItemType Directory -Force $pd | Out-Null
    [IO.File]::WriteAllText((Join-Path $pd "$([guid]::NewGuid()).jsonl"), '{"type":"user","message":{"role":"user","content":"smoke 302"}}' + "`n", $u8r)
  }
  $st = [ordered]@{
    sessions = @(
      [ordered]@{ id = 970; name = 'R302-Cont'; custom_name = $true; shell_id = 'powershell'; cwd = $rsDirs['r302-cont']; project = 1; claude = $true },
      [ordered]@{ id = 971; name = 'R302-Empty'; custom_name = $true; shell_id = 'powershell'; cwd = $rsDirs['r302-empty']; project = 1; claude = $true },
      [ordered]@{ id = 972; name = 'R302-Pick'; custom_name = $true; shell_id = 'powershell'; cwd = $rsDirs['r302-pick']; project = 1; claude = $true; claude_resume_mode = 'pick' },
      [ordered]@{ id = 973; name = 'R302-Off'; custom_name = $true; shell_id = 'powershell'; cwd = $rsDirs['r302-off']; project = 1; claude = $true; claude_continue = $false; claude_resume_mode = 'off' })
    next_id = 974; projects = @([ordered]@{ id = 1; name = 'Standard' }); active_project = 1
  }
  [IO.File]::WriteAllText("$data\state.json", ($st | ConvertTo-Json -Depth 8), $u8r)
  Set-Level 'Control' -Tray
  [void](Rpc 'initialize' @{ protocolVersion = '2025-06-18'; capabilities = @{}; clientInfo = @{ name = 'smoke'; version = '1' } })
  # Zeile des Ersatz-claude fuer einen Arbeitsordner (die letzte; wartet bis zu 20 s). Starten mehrere Ersatz-claude
  # gleichzeitig, kann ein `>>` von cmd an der gerade offenen calls.txt scheitern: dann zaehlt der Bildschirm der Sitzung
  # (dort steht die eingetippte Befehlszeile samt Ausgabe des Ersatzes).
  $lineOf = {
    param($n, $sid)
    $re = '(?m)^claude\.cmd cwd=' + [regex]::Escape($rsDirs[$n]) + ' .*$'
    $txt = Wait-FakeFile $calls $re 20000
    $l = @(($txt -split "`r?`n") | Where-Object { $_ -match ('^claude\.cmd cwd=' + [regex]::Escape($rsDirs[$n]) + ' ') }) | Select-Object -Last 1
    if ($l) { return $l }
    $scr = (Tool 'read_terminal' @{ session = $sid; lines = 40 }).data.text
    if ($scr -match 'Fake Claude') { return "terminal: $(($scr -split "`r?`n" | Where-Object { $_ -match 'claude' }) -join ' ')" }
    ''
  }
  $lc = & $lineOf 'r302-cont' 970
  Check 'Programmstart, Fortsetzen mit Konversation im Ordner: claude --continue (#302)' ($lc -match '--continue(\s|$)' -and $lc -notmatch '--session-id|--resume') "line='$lc'"
  $le = & $lineOf 'r302-empty' 971
  Check 'Programmstart, Fortsetzen ohne Konversation: neu, ohne --continue/--session-id (#302)' ($le -and $le -notmatch '--cont|--sess|--resu') "line='$le'"
  $lp = & $lineOf 'r302-pick' 972
  Check 'Programmstart, Auswaehlen ohne Konversation: ohne Rueckfrage neu (#302)' ($lp -and $lp -notmatch '--cont|--sess|--resu') "line='$lp'"
  $lo = & $lineOf 'r302-off' 973
  Check 'Programmstart, Aus: neu trotz Konversation im Ordner (#302)' ($lo -and $lo -notmatch '--cont|--sess|--resu') "line='$lo'"
  # Die Sitzungen behalten ihren Modus (state.json bleibt lesbar fuer aeltere Versionen: claude_continue)
  Stop-Hub
  $sj = [IO.File]::ReadAllText("$data\state.json").TrimStart([char]0xFEFF) | ConvertFrom-Json
  $sp = $sj.sessions | Where-Object { $_.id -eq 972 }; $so = $sj.sessions | Where-Object { $_.id -eq 973 }; $sc = $sj.sessions | Where-Object { $_.id -eq 970 }
  Check 'state.json: claude_resume_mode bleibt (pick/off), claude_continue fuer aeltere Versionen (#302)' ($sp.claude_resume_mode -eq 'pick' -and $so.claude_resume_mode -eq 'off' -and $so.claude_continue -eq $false -and $null -eq $sc.claude_resume_mode) ($sj.sessions | ConvertTo-Json -Compress -Depth 4)
  [IO.File]::WriteAllText("$data\state.json", $prevState, $u8r)

  Write-Host "== Chats ueber MCP (#63): Fake-Ollama-Server, create_session kind=chat, send_chat_message, read_chat" -ForegroundColor Cyan
  Stop-Hub
  # Kleiner Ollama-Ersatz auf 127.0.0.1 (TcpListener, eigener Runspace): /api/tags, /api/show, /api/chat (NDJSON).
  # /api/chat: Antwort "pong"; "slow" in der letzten Nutzer-Nachricht -> 3 s Pause; "tools please" mit tools-Liste ->
  # Werkzeugaufruf list_sessions; "proxy please <id>" -> send_text in Sitzung <id>; "chain please <chat> <id>" ->
  # send_chat_message an <chat> mit "proxy please <id>"; "read please <chat>" -> read_chat (last 40);
  # #340: "text call please" -> Aufruf als Text (<tool_call>{...}</tool_call>, list_sessions); "typed call please <chat>"
  # -> read_chat mit Zahlen als Text ("950", "2"); "loop please" -> immer wieder read_terminal mit session "abc" (auch
  # nach Werkzeug-Ergebnissen, fuer den Schleifen-Schutz);
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
          if ($hasTools -and $last -match 'loop please') { $first = '{"model":"fake-model","message":{"role":"assistant","content":"","tool_calls":[{"function":{"name":"read_terminal","arguments":{"session":"abc"}}}]},"done":false}' }
          elseif ($toolAfter) { $first = '{"model":"fake-model","message":{"role":"assistant","content":"done"},"done":false}' }
          # #338 Agenten-Werkzeuge: "agent <x> please" -> ein Aufruf des jeweiligen Werkzeugs im Projektordner des Chats.
          elseif ($hasTools -and $last -match 'agent (read|grep|todos|write|edit|outside|env|cmd|bg|ask) please') {
            $call = switch ($Matches[1]) {
              'read' { '{"name":"read_file","arguments":{"path":"notes.txt"}}' }
              'grep' { '{"name":"grep","arguments":{"pattern":"needle","output_mode":"files_with_matches"}}' }
              'todos' { '{"name":"update_todos","arguments":{"todos":[{"content":"Smoke step","status":"in_progress"},{"content":"Done step","status":"done"}]}}' }
              'write' { '{"name":"write_file","arguments":{"path":"out/new.txt","content":"agent-written\n"}}' }
              'edit' { '{"name":"edit_file","arguments":{"path":"notes.txt","old_string":"hello needle","new_string":"hello edited"}}' }
              'outside' { '{"name":"read_file","arguments":{"path":"../outside.txt"}}' }
              'env' { '{"name":"read_file","arguments":{"path":".env"}}' }
              'cmd' { '{"name":"run_command","arguments":{"command":"echo agent-cmd-ok"}}' }
              # #355/#356: Befehl im Hintergrund, Fragen mit Auswahl
              'bg' { '{"name":"run_command","arguments":{"command":"echo agent-bg-ok","background":true}}' }
              'ask' { '{"name":"ask_user","arguments":{"questions":[{"question":"Pick one?","options":[{"label":"A"},{"label":"B","recommended":true}]}]}}' }
            }
            $first = '{"model":"fake-model","message":{"role":"assistant","content":"","tool_calls":[{"function":' + $call + '}]},"done":false}'
          }
          elseif ($hasTools -and $last -match 'text call please') { $first = '{"model":"fake-model","message":{"role":"assistant","content":"Let me check.\n<tool_call>\n{\"name\": \"list_sessions\", \"arguments\": {}}\n</tool_call>"},"done":false}' }
          elseif ($hasTools -and $last -match 'typed call please (\d+)') { $first = '{"model":"fake-model","message":{"role":"assistant","content":"","tool_calls":[{"function":{"name":"read_chat","arguments":{"session":"' + $Matches[1] + '","last":"2"}}}]},"done":false}' }
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
  $srvJson = '{"id":"srv-smoke","name":"Fake","kind":"ollama","url":"http://127.0.0.1:' + $fakePort + '","models":[{"name":"fake-model","level":"Control","level_chosen":true,"ask_write":false,"tools":true},{"name":"fake-raw","context_length":6000}],"last_models":[{"name":"fake-model","tools":true,"context_length":131072},{"name":"fake-raw","tools":true,"context_length":131072}]}'
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
  # #305: eine alte eigene Kontextlaenge, die keine Reglerstufe ist (6000), bleibt unveraendert; sonst die vom Server erkannte.
  Check 'list_models: context_length (erkannt 131072, alte eigene 6000 unveraendert)' ($fs.models[0].context_length -eq 131072 -and $fs.models[1].name -eq 'fake-raw' -and $fs.models[1].context_length -eq 6000) $lm.text
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
  Check 'Stufe Steuern ohne eigene Kontextlaenge: num_ctx nach Bedarf (32768/65536, #340), kein Kontext-Hinweis' (@($script:fakeLog | Where-Object { $_ -like '/api/chat *' -and $_ -match '"num_ctx"\s*:\s*(32768|65536)' }).Count -ge 1 -and -not ((Tool 'read_chat' @{ session = $C; last = 50 }).text -match 'larger than the model|too small for its tool level'))
  [void](Tool 'send_chat_message' @{ session = $C; text = 'tools please' })
  $w = Tool 'wait_for_status' @{ session = $C; status = @('ready'); timeout_secs = 40 } 50000
  $rc = Tool 'read_chat' @{ session = $C; last = 10 }
  $tm = @($rc.data.messages | Where-Object { $_.role -eq 'tool' -and $_.tool_name -eq 'list_sessions' })
  $lastA = @($rc.data.messages | Where-Object { $_.role -eq 'assistant' })[-1]
  Check 'Werkzeug-Schleife: Chat ruft list_sessions auf und antwortet "done"' ($w.data.matched -eq $true -and $tm.Count -ge 1 -and $tm[0].status -eq 'ok' -and $lastA.content -eq 'done') $rc.text
  # #340: Aufruf als Text (<tool_call>...</tool_call>) wird erkannt und wie ein echter ausgefuehrt; der Aufruf-Text faellt
  # aus der Antwort, der Aufruf steht als tool_calls darin.
  [void](Tool 'send_chat_message' @{ session = $C; text = 'text call please' })
  $w = Tool 'wait_for_status' @{ session = $C; status = @('ready'); timeout_secs = 40 } 50000
  $rc = Tool 'read_chat' @{ session = $C; last = 4 }
  $tm = @($rc.data.messages | Where-Object { $_.role -eq 'tool' -and $_.tool_name -eq 'list_sessions' })
  $ca = @($rc.data.messages | Where-Object { $_.role -eq 'assistant' -and $null -ne $_.tool_calls -and @($_.tool_calls).Count -ge 1 })
  $lastA = @($rc.data.messages | Where-Object { $_.role -eq 'assistant' })[-1]
  Check 'Textaufruf (#340): <tool_call>{list_sessions} wird ausgefuehrt, Text faellt aus der Antwort, danach "done"' ($w.data.matched -eq $true -and $tm.Count -ge 1 -and $tm[-1].status -eq 'ok' -and $ca.Count -ge 1 -and $ca[-1].tool_calls[0].name -eq 'list_sessions' -and $ca[-1].content -eq 'Let me check.' -and $ca[-1].content -notmatch 'tool_call' -and $lastA.content -eq 'done') $rc.text
  # #340: falsche Typen ("950", "2") werden normalisiert statt abgewiesen (vorher: bad_args "session (number) is missing").
  [void](Tool 'send_chat_message' @{ session = $C; text = 'typed call please 950' })
  $w = Tool 'wait_for_status' @{ session = $C; status = @('ready'); timeout_secs = 40 } 50000
  $rc = Tool 'read_chat' @{ session = $C; last = 4 }
  $tm = @($rc.data.messages | Where-Object { $_.role -eq 'tool' -and $_.tool_name -eq 'read_chat' })
  $ca = @($rc.data.messages | Where-Object { $_.role -eq 'assistant' -and $null -ne $_.tool_calls -and @($_.tool_calls).Count -ge 1 })
  $args0 = if ($ca.Count -ge 1) { $ca[-1].tool_calls[0].arguments } else { $null }
  Check 'Normalisierung (#340): read_chat mit session "950" und last "2" laeuft mit Zahlen (ok, Argumente korrigiert gespeichert)' ($w.data.matched -eq $true -and $tm.Count -ge 1 -and $tm[-1].status -eq 'ok' -and $tm[-1].content -match 'smoke-answer' -and $null -ne $args0 -and $args0.session -eq 950 -and -not ($args0.session -is [string]) -and $args0.last -eq 2 -and -not ($args0.last -is [string])) $rc.text
  # #340: Schleifen-Schutz - derselbe fehlerhafte Aufruf dreimal in Folge haelt die Runde an (paused, pause_note).
  [void](Tool 'send_chat_message' @{ session = $C; text = 'loop please' })
  $w = Tool 'wait_for_status' @{ session = $C; status = @('ready'); timeout_secs = 40 } 50000
  $rc = Tool 'read_chat' @{ session = $C; last = 10 }
  $lu = -1; $ms = @($rc.data.messages); for ($i = 0; $i -lt $ms.Count; $i++) { if ($ms[$i].role -eq 'user') { $lu = $i } }
  $bt = @($ms | Select-Object -Skip ($lu + 1) | Where-Object { $_.role -eq 'tool' -and $_.tool_name -eq 'read_terminal' })
  Check 'Schleifen-Schutz (#340): 3x derselbe fehlerhafte Aufruf -> angehalten mit Hinweis, je bad_args mit Beispiel' ($w.data.matched -eq $true -and $rc.data.paused -eq $true -and $rc.data.pause_note -match 'same failing tool call \(read_terminal\) 3 times in a row' -and $bt.Count -eq 3 -and @($bt | Where-Object { $_.status -eq 'error' -and $_.content -match 'bad_args' -and $_.content -match 'must be an integer' -and $_.content -match 'Correct example' }).Count -eq 3) $rc.text
  # #338 Agenten-Werkzeuge nur im Chat: eigener Chat mit Projektordner $proj\agent (ask_write des Fake-Modells ist aus,
  # schreibende Werkzeuge laufen also ohne Freigabe; sensible Dateien fragen trotzdem).
  $agDir = New-Item -ItemType Directory -Force "$proj\agent"
  [IO.File]::WriteAllText("$agDir\notes.txt", "hello needle`nsecond line`n", (New-Object Text.UTF8Encoding $false))
  [IO.File]::WriteAllText("$agDir\.env", "TOKEN=needle-secret`n", (New-Object Text.UTF8Encoding $false))
  [IO.File]::WriteAllText("$proj\outside.txt", "outside-content`n", (New-Object Text.UTF8Encoding $false))
  $tl = @((Rpc 'tools/list').result.tools | ForEach-Object name)
  Check 'Agenten-Werkzeuge (#338) nicht im externen MCP-Server' (-not ($tl -contains 'read_file' -or $tl -contains 'grep' -or $tl -contains 'edit_file' -or $tl -contains 'update_todos'))
  $AG = (Tool 'create_session' @{ kind = 'chat'; server = 'Fake'; model = 'fake-model'; name = 'Smoke-Agent'; cwd = "$agDir" }).data.id
  $agentStep = {
    param($text, $tool, $timeout = 40)
    [void](Tool 'send_chat_message' @{ session = $AG; text = $text })
    $w = Tool 'wait_for_status' @{ session = $AG; status = @('ready'); timeout_secs = $timeout } (($timeout + 10) * 1000)
    $rc = Tool 'read_chat' @{ session = $AG; last = 4 }
    $t = @($rc.data.messages | Where-Object { $_.role -eq 'tool' -and $_.tool_name -eq $tool })
    @{ ok = ($w.data.matched -eq $true); tool = $(if ($t.Count -ge 1) { $t[-1] } else { $null }); text = $rc.text }
  }
  $r = & $agentStep 'agent read please' 'read_file'
  Check 'Agent read_file (#338): Inhalt mit Zeilennummern, frei (ohne Freigabe)' ($r.ok -and $r.tool.status -eq 'ok' -and $r.tool.content -match 'hello needle' -and $r.tool.content -match '\\t' -and $r.tool.content -match 'total_lines') $r.text
  Check 'Agent-Werkzeuge in der Anfrage an das Modell (read_file, grep)' (@($script:fakeLog | Where-Object { $_ -like '/api/chat *' -and $_ -match '"name"\s*:\s*"read_file"' -and $_ -match '"name"\s*:\s*"grep"' }).Count -ge 1)
  $r = & $agentStep 'agent grep please' 'grep'
  Check 'Agent grep (#338): findet notes.txt, ueberspringt die sensible .env' ($r.ok -and $r.tool.status -eq 'ok' -and $r.tool.content -match 'notes.txt' -and $r.tool.content -notmatch 'needle-secret' -and $r.tool.content -match '"sensitive":1') $r.text
  $r = & $agentStep 'agent todos please' 'update_todos'
  Start-Sleep 1
  $cj = if (Test-Path "$data\chats\$AG.json") { [IO.File]::ReadAllText("$data\chats\$AG.json") } else { '' }
  Check 'Agent update_todos (#338): Liste im Chat gespeichert, "done" korrigiert' ($r.ok -and $r.tool.status -eq 'ok' -and $r.tool.content -match 'corrected' -and $cj -match '"todos":\[\{"content":"Smoke step","status":"in_progress"\},\{"content":"Done step","status":"completed"\}\]') "$($r.text) /// $cj"
  $r = & $agentStep 'agent write please' 'write_file'
  $nf = "$agDir\out\new.txt"
  Check 'Agent write_file (#338): Datei mit Ordner angelegt' ($r.ok -and $r.tool.status -eq 'ok' -and (Test-Path $nf) -and [IO.File]::ReadAllText($nf) -eq "agent-written`n") $r.text
  $r = & $agentStep 'agent edit please' 'edit_file'
  Check 'Agent edit_file (#338): genau eine Ersetzung' ($r.ok -and $r.tool.status -eq 'ok' -and [IO.File]::ReadAllText("$agDir\notes.txt") -eq "hello edited`nsecond line`n") $r.text
  $r = & $agentStep 'agent outside please' 'read_file'
  Check 'Agent read_file ausserhalb des Projektordners -> outside_project, nichts gelesen' ($r.ok -and $r.tool.status -eq 'error' -and $r.tool.content -match 'outside_project' -and $r.text -notmatch 'outside-content') $r.text
  $r = & $agentStep 'agent cmd please' 'run_command' 90
  $kids = @((Tool 'list_sessions').data.sessions) | Where-Object { $_.parent -eq $AG -and $_.kind -eq 'terminal' }
  Check 'Agent run_command ohne session (#338): eigenes Terminal des Chats im Projektordner, Ausgabe kommt zurueck' ($r.ok -and $r.tool.status -eq 'ok' -and $r.tool.content -match 'agent-cmd-ok' -and @($kids).Count -eq 1 -and $kids[0].name -like '*commands*') "$($r.text) /// $($kids | ConvertTo-Json -Compress)"
  $r2 = & $agentStep 'agent cmd please' 'run_command' 60
  $kids2 = @((Tool 'list_sessions').data.sessions) | Where-Object { $_.parent -eq $AG -and $_.kind -eq 'terminal' }
  Check 'Agent run_command: das Terminal des Chats wird wiederverwendet' ($r2.ok -and $r2.tool.status -eq 'ok' -and @($kids2).Count -eq 1) "$($r2.text) /// $($kids2 | ConvertTo-Json -Compress)"
  # #355: Befehl im Hintergrund - sofort eine Kennung, eigenes Terminal unter dem Chat, am Ende eine Nachricht von Paneldeck.
  $r = & $agentStep 'agent bg please' 'run_command' 60
  $kids3 = @((Tool 'list_sessions').data.sessions) | Where-Object { $_.parent -eq $AG -and $_.kind -eq 'terminal' -and $_.name -like '*bg1*' }
  Check 'Agent run_command background (#355): sofort id bg1 (running), eigenes Terminal "bg1" unter dem Chat' ($r.ok -and $r.tool.status -eq 'ok' -and $r.tool.content -match '"id":"bg1"' -and $r.tool.content -match '"status":"running"' -and @($kids3).Count -eq 1) "$($r.text) /// $($kids3 | ConvertTo-Json -Compress)"
  $ev = $null
  for ($i = 0; $i -lt 40 -and -not $ev; $i++) {
    $rc = Tool 'read_chat' @{ session = $AG; last = 8 }
    $ev = @($rc.data.messages | Where-Object { $_.role -eq 'system' -and $null -ne $_.event -and $_.event.id -eq 'bg1' }) | Select-Object -Last 1
    if (-not $ev) { Start-Sleep 1 }
  }
  Check 'Hintergrund-Befehl endet (#355): Nachricht von Paneldeck im Chat (event exited, Exit-Code 0, ohne Ausgabe)' ($null -ne $ev -and $ev.event.state -eq 'exited' -and $ev.event.exit_code -eq 0 -and $ev.content -match 'command_output' -and ($ev.content -replace '`[^`]*`', '') -notmatch 'agent-bg-ok') ($rc.text)
  [void](Tool 'wait_for_status' @{ session = $AG; status = @('ready'); timeout_secs = 30 } 40000)
  # #356: Fragen mit Auswahl - eigener Chat; er wartet auf den Nutzer (approval), externe Agenten schreiben dann nicht hinein.
  $AK = (Tool 'create_session' @{ kind = 'chat'; server = 'Fake'; model = 'fake-model'; name = 'Smoke-Ask'; cwd = "$agDir" }).data.id
  [void](Tool 'send_chat_message' @{ session = $AK; text = 'agent ask please' })
  $w = Tool 'wait_for_status' @{ session = $AK; status = @('approval'); timeout_secs = 30 } 40000
  $sk = Tool 'send_chat_message' @{ session = $AK; text = 'x' }
  $rk = Tool 'read_chat' @{ session = $AK; last = 3 }
  $ka = @($rk.data.messages | Where-Object { $_.role -eq 'assistant' -and $null -ne $_.tool_calls -and @($_.tool_calls).Count -ge 1 }) | Select-Object -Last 1
  Check 'Agent ask_user (#356): Chat wartet auf den Nutzer (approval), send_chat_message -> permission_pending' ($w.data.matched -eq $true -and $w.data.needs_you -eq $true -and (Code $sk) -eq 'permission_pending' -and $null -ne $ka -and $ka.tool_calls[0].name -eq 'ask_user') "$($w.text) / $($sk.text) / $($rk.text)"
  [void](Tool 'close_session' @{ session = $AK })
  # Sensible Datei: fragt auch ohne ask_write; der Chat wartet (approval), der Inhalt erscheint nirgends.
  [void](Tool 'send_chat_message' @{ session = $AG; text = 'agent env please' })
  $w = Tool 'wait_for_status' @{ session = $AG; status = @('approval'); timeout_secs = 30 } 40000
  $rc = Tool 'read_chat' @{ session = $AG; last = 3 }
  Check 'Agent read_file .env (#338): wartet auf Freigabe trotz ask_write=false, Inhalt nicht gelesen' ($w.data.matched -eq $true -and $w.data.needs_you -eq $true -and $rc.text -notmatch 'needle-secret') $rc.text
  foreach ($k in @($kids2)) { [void](Tool 'close_session' @{ session = $k.id }) }
  Check 'Chat mit offener Freigabe schliessen (vom Agenten angelegt)' ((Tool 'close_session' @{ session = $AG }).err -ne $true)
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
    # Agenten-Kette (#152) im Terminal $T (Glied der Kette $A -> $C): whoami/list_sessions nennen sie, nur Lesen in $A.
    $Ti = $T.id
    $kw = InS $Ti 'whoami' @{}
    Check 'Kette: whoami im Glied nennt agent_chain (Agent + Chat)' ($kw.structuredContent.session -eq $Ti -and @($kw.structuredContent.agent_chain) -contains $A -and @($kw.structuredContent.agent_chain) -contains $C) ($kw | ConvertTo-Json -Compress -Depth 5)
    Check 'Kette: Glied -> rename_session des Vorfahren forbidden' ((InS $Ti 'rename_session' @{ session = $A; name = 'x' }).content[0].text -like 'forbidden*')
    Check 'Kette: Glied darf den Vorfahren lesen' ((InS $Ti 'read_terminal' @{ session = $A; lines = 5 }).isError -ne $true)
    foreach ($t in @(@('run_command', @{ session = $A; command = 'echo kind-leak' }), @('send_keys', @{ session = $A; keys = @('enter') }), @('queue_prompt', @{ session = $A; prompt = 'x' }), @('focus_session', @{ session = $A }))) {
      $kr = InS $Ti $t[0] $t[1]
      Check "Kette: Glied -> $($t[0]) in den Vorfahren forbidden" ($kr.isError -eq $true -and $kr.content[0].text -like 'forbidden*') ($kr | ConvertTo-Json -Compress -Depth 5)
    }
    # Gefaelschter/fehlender Aufrufer: rohe TCP-Anfrage mit dem Token aus ipc.json, ohne caller/caller_pid, aus $T ->
    # der Hub ermittelt den Absender selbst (TCP-Tabelle + Job Object bzw. Prozessbaum) und weist ab.
    $rawCmd = "`$j = Get-Content -Raw '$data\ipc.json' | ConvertFrom-Json; `$c = New-Object Net.Sockets.TcpClient('127.0.0.1', [int]`$j.port); `$s = `$c.GetStream(); `$b = [Text.Encoding]::UTF8.GetBytes('{""token"":""' + `$j.token + '"",""cmd"":""mcp"",""tool"":""send_text"",""caller"":null,""caller_pid"":4,""args"":{""session"":$A,""text"":""echo raw-leak""}}'); `$s.Write(`$b, 0, `$b.Length); `$c.Client.Shutdown(1); (New-Object IO.StreamReader(`$s)).ReadToEnd()"
    $raw = Tool 'run_command' @{ session = $Ti; command = $rawCmd; timeout_secs = 30 }
    Check 'Kette: rohe TCP-Anfrage ohne Aufrufer aus dem Glied -> zugeordnet und forbidden' ($raw.data.output -match '"code":"forbidden"' -and $raw.data.output -match 'agent chain') $raw.text
    # Losgeloester Enkel des Glieds (#168): ueber den Job von $T zugeordnet -> als Glied der Kette abgewiesen.
    $det = Detached $Ti 'send_text' ('{"session":' + $A + ',"text":"echo detached-leak"}')
    Check 'Job: losgeloester Enkel eines Glieds -> der Kette zugeordnet, forbidden in den Vorfahren' ($det -match '"code":"forbidden"' -and $det -match 'agent chain') $det
    # Uebrig gebliebener Prozess einer geschlossenen Sitzung (#218): Der Hub behaelt ihren Job, der Prozess handelt als
    # die geschlossene Sitzung mit ihrer Kette -> forbidden in den Vorfahren. (Ein Prozess ganz ohne Job ist extern,
    # solange jede Sitzung einen Job hat - siehe den Check "Job: losgeloester Prozess ausserhalb jeder Sitzung -> keiner
    # Sitzung zugeordnet" weiter oben: identified_by "none".)
    $nn = (InS $Ti 'create_session' @{ shell = 'powershell'; cwd = "$proj"; name = 'smoke-retired' }).structuredContent.id
    Start-Sleep 4
    $flag = "$work\retired-go-$([guid]::NewGuid().ToString('N').Substring(0, 6)).txt"
    $retOut = Detached $nn 'send_text' ('{"session":' + $A + ',"text":"echo retired-leak"}') $flag
    Start-Sleep 2
    $rc2 = InS $Ti 'close_session' @{ session = $nn }
    Check 'Geschlossene Sitzung mit losgeloestem Prozess (close_session)' ($rc2.isError -ne $true) ($rc2 | ConvertTo-Json -Compress -Depth 5)
    [IO.File]::WriteAllText($flag, 'go')
    $deadline = (Get-Date).AddSeconds(25)
    while (-not (Test-Path $retOut) -and (Get-Date) -lt $deadline) { Start-Sleep -Milliseconds 300 }
    $det = if (Test-Path $retOut) { [IO.File]::ReadAllText($retOut) } else { '' }
    Check 'Job: uebrig gebliebener Prozess einer geschlossenen Sitzung -> deren Kette, forbidden in den Vorfahren' ($det -match '"code":"forbidden"' -and $det -match 'agent chain') $det
    # paneldeck --open aus einem Terminal (#152): markiert nur, wenn das Terminal selbst eine Kette hat.
    $before = @((Tool 'list_sessions').data.sessions | ForEach-Object id)
    [void](Tool 'run_command' @{ session = $Ti; command = "& '$dir\paneldeck.exe' --open '$proj'"; timeout_secs = 30 })
    Start-Sleep 3
    $o1 = @((Tool 'list_sessions').data.sessions) | Where-Object { $before -notcontains $_.id } | Select-Object -First 1
    Check '--open aus einem Glied der Kette: neue Sitzung erbt die Kette samt Glied' ($null -ne $o1 -and @($o1.agent_chain) -contains $Ti -and @($o1.agent_chain) -contains $A) ($o1 | ConvertTo-Json -Compress)
    $before = @((Tool 'list_sessions').data.sessions | ForEach-Object id)
    [void](Tool 'run_command' @{ session = $A; command = "& '$dir\paneldeck.exe' --open '$proj'"; timeout_secs = 30 })
    Start-Sleep 3
    $o2 = @((Tool 'list_sessions').data.sessions) | Where-Object { $before -notcontains $_.id } | Select-Object -First 1
    Check '--open aus einem Terminal ohne Kette: neue Sitzung bleibt frei' ($null -ne $o2 -and @($o2.agent_chain).Count -eq 0) ($o2 | ConvertTo-Json -Compress)
    Start-Sleep 1
    Check 'Kette: im Vorfahren wurde nichts getippt' ((Tool 'read_terminal' @{ session = $A; lines = 200 }).data.text -notmatch 'kind-leak|raw-leak|detached-leak|orphan-leak')
  }
  Check 'wait_for_status auf die eigene Sitzung -> bad_args' ((InA 'wait_for_status' @{ session = $A; status = @('ready'); timeout_secs = 5 }).content[0].text -like 'bad_args*')
  Check 'Kette: von aussen (ohne Kette) bleibt Schreiben in den Vorfahren erlaubt' (-not (Tool 'send_text' @{ session = $A; text = ' '; submit = $false }).err)
  [void](Tool 'close_session' @{ session = $C2 })
  [void](Tool 'close_session' @{ session = $A })
  # read_chat ueber ein Chat-Modell (Ergebnis gekuerzt): die neuesten Nachrichten und der Status kommen an, nicht die aeltesten.
  [void](Tool 'send_chat_message' @{ session = $C; text = 'read please 952' })
  $w = Tool 'wait_for_status' @{ session = $C; status = @('ready'); timeout_secs = 40 } 50000
  $rc = Tool 'read_chat' @{ session = $C; last = 4 }
  $rt = @($rc.data.messages | Where-Object { $_.role -eq 'tool' -and $_.tool_name -eq 'read_chat' })
  $rtc = if ($rt.Count -ge 1) { [string]$rt[-1].content } else { '' }
  $diag = 'matched=' + $w.data.matched + ' n=' + $rt.Count + ' b39=' + ($rtc -match 'big-39') + ' more=' + ($rtc -match '"more_above":true') + ' ready=' + ($rtc -match '"status":"ready"') + ' b00=' + ($rtc -match 'big-00') + ' len=' + $rtc.Length + ' tail=' + $(if ($rtc.Length -gt 500) { $rtc.Substring($rtc.Length - 500) } else { $rtc })
  Check 'read_chat ueber ein Chat-Modell: neueste Nachrichten, more_above und Status statt der aeltesten' ($w.data.matched -eq $true -and $rt.Count -ge 1 -and $rt[-1].status -eq 'ok' -and $rt[-1].content -match 'big-39' -and $rt[-1].content -match '"more_above":true' -and $rt[-1].content -match '"status":"ready"' -and $rt[-1].content -notmatch 'big-00') $diag
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
  # Themes (#309): nur lesen, Stufe Lesen.
  Check 'Nur lesen: theme_list und theme_get vorhanden' ($names -contains 'theme_list' -and $names -contains 'theme_get')
  $tl = Tool 'theme_list'
  $mine = @($tl.data.themes) | Where-Object { $_.name -eq 'Smoke Theme' }
  $od = @($tl.data.themes) | Where-Object { $_.name -eq 'One Dark' }
  Check 'theme_list: eigenes Theme mit Datei, eingebautes ohne, aktives gesetzt' ($mine.builtin -eq $false -and $mine.file -eq 'smoke.json' -and $od.builtin -eq $true -and $null -eq $od.file -and [string]$tl.data.active -ne '' -and @(@($tl.data.themes) | Where-Object { $_.active }).Count -eq 1) $tl.text
  $inv = @($tl.data.invalid) | Where-Object { $_.file -eq 'broken.json' }
  Check 'theme_list: ungueltige Datei mit Grund, nicht waehlbar, ohne Pfad' ($inv -and [string]$inv.error -match 'fg' -and -not (@($tl.data.themes) | Where-Object { $_.name -eq 'Broken' }) -and $tl.text -notmatch [regex]::Escape($dir)) $tl.text
  $tg = Tool 'theme_get' @{ name = 'Smoke Theme' }
  Check 'theme_get: Terminal, aufgeloeste Farben, gesetzte Werte, unbekannte ignoriert' ($tg.data.terminal.bg -eq '#202020' -and @($tg.data.terminal.ansi).Count -eq 16 -and $tg.data.colors.accent -eq '#0080ff' -and $tg.data.colors.link -eq '#0080ff' -and $tg.data.overrides.border -eq '@text/20' -and $null -eq $tg.data.overrides.future_token -and ([string]$tg.data.colors.border).Length -eq 9) $tg.text
  $ta = Tool 'theme_get'
  Check 'theme_get ohne Namen: aktives Theme' ($ta.data.name -eq $tl.data.active -and $ta.data.active -eq $true) $ta.text
  Check 'theme_get: unbekannter Name -> not_found' ((Code (Tool 'theme_get' @{ name = 'Gibts nicht' })) -eq 'not_found')
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
      else { Check 'ConPTY: run_command powershell (Exit-Code 4)' ($r.data.exit_code -eq 4) $r.text; Test-WrapWidth $c.data.id $sh 'ConPTY: ' }
      if ($sh -eq 'powershell') {
        # Fortschritt (OSC 9;4, #183): list_sessions meldet ihn, der naechste Prompt entfernt ihn wieder. Mit ST statt BEL
        # beendet, damit keine Glocke (Benachrichtigung) ausgeloest wird.
        $sid = $c.data.id
        $me = @((Tool 'list_sessions').data.sessions | Where-Object { $_.id -eq $sid })
        Check 'OSC 9;4: list_sessions hat das Feld progress (null ohne Fortschritt)' ($me.Count -eq 1 -and ($me[0].PSObject.Properties.Name -contains 'progress') -and $null -eq $me[0].progress) ($me | ConvertTo-Json -Compress)
        [void](Tool 'send_text' @{ session = $sid; text = '[Console]::Write("$([char]27)]9;4;1;42$([char]27)\"); Start-Sleep 6'; submit = $true })
        $pg = $null
        for ($i = 0; $i -lt 40 -and -not $pg; $i++) { Start-Sleep -Milliseconds 150; $pg = @((Tool 'list_sessions').data.sessions | Where-Object { $_.id -eq $sid })[0].progress }
        if ($useBundled -or $pg) {
          Check "OSC 9;4: list_sessions progress = normal/42 ($want)" ($pg.state -eq 'normal' -and $pg.percent -eq 42) ($pg | ConvertTo-Json -Compress)
          $gone = $false
          for ($i = 0; $i -lt 60 -and -not $gone; $i++) { Start-Sleep -Milliseconds 200; $gone = $null -eq @((Tool 'list_sessions').data.sessions | Where-Object { $_.id -eq $sid })[0].progress }
          Check 'OSC 9;4: Fortschritt nach dem naechsten Prompt entfernt' $gone
        } else { Skip 'OSC 9;4: list_sessions progress' 'ConPTY von Windows reicht OSC 9;4 nicht durch' }
      }
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
        Test-ConhostWrap $gid 'ConPTY: '
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

  Write-Host "== Aufraeumen (#307): cleanup_sizes und vorgemerkter Lauf im Test-Datenordner" -ForegroundColor Cyan
  Set-Level 'Read' -Empty
  $names = @((Rpc 'tools/list').result.tools | ForEach-Object name)
  Check 'cleanup_sizes: in tools/list (Lesen), kein Werkzeug zum Loeschen' ($names -contains 'cleanup_sizes' -and -not ($names | Where-Object { $_ -match 'clean|delete|purge|reset' -and $_ -ne 'cleanup_sizes' })) ($names -join ',')
  New-Item -ItemType Directory -Force "$data\clipboard", "$data\scrollback", "$data\chats" | Out-Null
  [IO.File]::WriteAllBytes("$data\clipboard\clip-1-0.png", (New-Object byte[] 2048))
  [IO.File]::WriteAllText("$data\crash.log", 'smoke')
  [IO.File]::WriteAllText("$data\state.json.bak-20250101-000000", '{}')
  [IO.File]::WriteAllText("$data\scrollback\99999.txt", 'alt')
  [IO.File]::WriteAllText("$data\chats\99998.json", '{}')
  $r = Tool 'cleanup_sizes'
  $it = @{}; foreach ($x in @($r.data.items)) { $it[$x.id] = $x }
  Check 'cleanup_sizes: Antwort mit Bereichen und Gruppen' (-not $r.err -and $it.Count -ge 10 -and $it['database'].group -eq 'content' -and $it['factory_reset'].needs_typing) $r.text
  Check 'cleanup_sizes: Zwischenablage 2048 B, 1 Datei, vorausgewaehlt' ($it['clipboard_images'].bytes -eq 2048 -and $it['clipboard_images'].files -eq 1 -and $it['clipboard_images'].preselected) ($it['clipboard_images'] | ConvertTo-Json -Compress)
  Check 'cleanup_sizes: verwaister Verlauf gezaehlt' ($it['scrollback_orphaned'].files -ge 1 -and $it['chats_orphaned'].files -ge 1)
  Check 'cleanup_sizes: Datenordner-Summe und Profile' ($r.data.data_folder.bytes -gt 0 -and $null -ne $r.data.profiles.count)
  Check 'cleanup_sizes: keine Pfade in der Antwort' ($r.text -notmatch [regex]::Escape($data) -and $r.text -notmatch [regex]::Escape($data.Replace('\', '\\')))
  # Vorgemerkter Lauf (wie nach "Loeschen" mit Neustart): wird beim naechsten Start vor dem Laden ausgefuehrt. Integration
  # (autostart) wird aus der Vormerkung nie ausgefuehrt - Registry bleibt unberuehrt.
  Stop-Hub
  $hadDb = Test-Path -LiteralPath "$data\paneldeck.db"
  $pending = @{ version = 1; items = @('clipboard_images', 'crash_log', 'old_backup_files', 'scrollback_orphaned', 'chats_orphaned', 'database', 'autostart'); opts = @{ keep_backups = 3; backup_db = $true } } | ConvertTo-Json -Depth 5
  [IO.File]::WriteAllText("$data\cleanup-pending.json", $pending, (New-Object Text.UTF8Encoding $false))
  Set-Level 'Read' -Empty
  Check 'Aufraeumen beim Start: Vormerkung verbraucht' (-not (Test-Path -LiteralPath "$data\cleanup-pending.json"))
  Check 'Aufraeumen beim Start: Dateien geloescht' (-not (Test-Path "$data\clipboard\clip-1-0.png") -and -not (Test-Path "$data\crash.log") -and -not (Test-Path "$data\state.json.bak-20250101-000000") -and -not (Test-Path "$data\scrollback\99999.txt") -and -not (Test-Path "$data\chats\99998.json"))
  if ($hadDb) {
    $pre = @(Get-ChildItem -LiteralPath "$data\backups" -Filter 'pre-cleanup-*.db' -ErrorAction SilentlyContinue)
    Check 'Aufraeumen beim Start: Datenbank vorher gesichert (pre-cleanup-*.db)' ($pre.Count -ge 1) ($pre.Name -join ',')
  } else { Skip 'Aufraeumen: Datenbank-Sicherung' 'keine paneldeck.db im Test-Datenordner' }
  $r = Tool 'cleanup_sizes'
  $it = @{}; foreach ($x in @($r.data.items)) { $it[$x.id] = $x }
  Check 'Aufraeumen beim Start: Hub laeuft, Zwischenablage leer' (-not $r.err -and $it['clipboard_images'].files -eq 0) $r.text
  Write-Host "== Prozessliste (#243)" -ForegroundColor Cyan
  # Eintraege von Hand in processes.json (Agenten koennen keine anlegen). Der Wert der Umgebungsvariable per DPAPI (aktueller
  # Benutzer) wie in Paneldeck; harmlose Programme: ping, ein TCP-Listener auf 127.0.0.1, exit 3, ein unbekannter Befehl.
  Add-Type -AssemblyName System.Security
  $secret = 'geheim-smoke-42'
  $blob = [Convert]::ToBase64String([Security.Cryptography.ProtectedData]::Protect([Text.Encoding]::UTF8.GetBytes($secret), $null, 'CurrentUser'))
  $pdefs = @(
    @{ id = 1; name = 'Ping'; command = 'ping -n 30 127.0.0.1'; group = 'G'; order = 2 },
    @{ id = 2; name = 'Http'; command = '$l=[Net.Sockets.TcpListener]::new([Net.IPAddress]::Loopback,0); $l.Start(); Write-Output ("Listening on http://127.0.0.1:" + $l.LocalEndpoint.Port + "/"); Write-Output "ERROR: smoke-fehlerzeile"; Start-Sleep 60'; group = 'G'; order = 1 },
    @{ id = 3; name = 'Crash'; command = 'Write-Output vor-dem-absturz; exit 3' },
    @{ id = 4; name = 'Missing'; command = 'gibtsnicht-pd-smoke-xyz' },
    @{ id = 5; name = 'Env'; command = 'Write-Output ("env=" + $env:PD_SMOKE_SECRET); Start-Sleep 30'; env = @(@{ name = 'PD_SMOKE_SECRET'; value_protected = $blob }) },
    @{ id = 6; name = 'Zaeh'; command = 'Start-Process ping -ArgumentList ''-n'',''117'',''127.0.0.1'' -WindowStyle Hidden; [Console]::TreatControlCAsInput=$true; Write-Output zaeh-laeuft; while($true){ Start-Sleep 1 }' },
    @{ id = 7; name = 'Cmd'; command = 'echo "a b" && exit 4'; shell = 'cmd' },
    @{ id = 8; name = 'Auto'; command = 'Write-Output auto-gestartet; Start-Sleep 60'; autostart = 'paneldeck' }
  )
  New-Item -ItemType Directory -Force $data | Out-Null
  [IO.File]::WriteAllText("$data\processes.json", (@{ version = 1; processes = $pdefs } | ConvertTo-Json -Depth 6), (New-Object Text.UTF8Encoding $false))
  function Get-Proc([string]$n) { (Tool 'list_processes').data.processes | Where-Object name -eq $n }
  function Wait-Proc([string]$n, [string[]]$want, [int]$sec = 15) {
    $t0 = Get-Date
    do { $p = Get-Proc $n; if ($p -and $want -contains $p.status) { return $p }; Start-Sleep -Milliseconds 300 } while (((Get-Date) - $t0).TotalSeconds -lt $sec)
    $p
  }
  function Wait-Output([string]$n, [string]$pattern, [int]$sec = 15) {
    $t0 = Get-Date
    do { $o = Tool 'read_process_output' @{ process = $n; lines = 200 }; if ("$($o.data.text)" -match $pattern) { return $o }; Start-Sleep -Milliseconds 300 } while (((Get-Date) - $t0).TotalSeconds -lt $sec)
    $o
  }
  # Kind mit eigener (verborgener) Konsole: bekommt kein Strg+C, endet nur mit dem Job Object.
  function Get-Marker { @(Get-CimInstance Win32_Process -Filter "Name='PING.EXE'" | Where-Object { $_.CommandLine -like '*-n 117 127.0.0.1*' }) }

  Set-Level 'Read' -Empty
  $l = Tool 'list_processes'
  Check 'Prozesse (Lesen): list_processes liefert alle Eintraege, nach dem Start gestoppt' (-not $l.err -and @($l.data.processes).Count -eq 8 -and -not (@($l.data.processes) | Where-Object { $_.status -ne 'stopped' -and $_.name -ne 'Auto' })) $l.text
  $p = Wait-Proc 'Auto' @('running') 5
  Check 'Prozesse: Autostart "Mit Paneldeck" laeuft' ($p.status -eq 'running' -and $p.autostart -eq 'paneldeck') ($p | ConvertTo-Json -Compress)
  Check 'Prozesse: Gruppe G gemeldet' (@($l.data.groups) -contains 'G') ($l.data.groups -join ',')
  $e = @($l.data.processes) | Where-Object name -eq 'Env'
  Check 'Prozesse: env_names ohne Werte (kein Klartext, kein Blob)' (@($e.env_names) -contains 'PD_SMOKE_SECRET' -and $l.text -notmatch [regex]::Escape($secret) -and $l.text -notmatch [regex]::Escape($blob.Substring(0, 20))) $l.text
  Check 'Prozesse: Hinweis nicht vertrauenswuerdig' ($l.data.note -match 'untrusted|vertrauensw') $l.data.note
  Check 'Prozesse (Lesen): start_process -> forbidden' ((Code (Tool 'start_process' @{ process = 'Ping' })) -eq 'forbidden')

  Set-Level 'Control' -Empty
  Check 'Prozesse: start_process ohne process/group -> bad_args' ((Code (Tool 'start_process' @{})) -eq 'bad_args')
  Check 'Prozesse: start_process mit process und group -> bad_args' ((Code (Tool 'start_process' @{ process = 'Ping'; group = 'G' })) -eq 'bad_args')
  Check 'Prozesse: unbekannter Prozess -> not_found' ((Code (Tool 'start_process' @{ process = 'gibtsnicht' })) -eq 'not_found')
  Check 'Prozesse: unbekannte Gruppe -> not_found' ((Code (Tool 'stop_process' @{ group = 'gibtsnicht' })) -eq 'not_found')
  Check 'Prozesse: read_process_output vor dem Start (leer, gestoppt)' ((Tool 'read_process_output' @{ process = 1 }).data.status -eq 'stopped')

  $s = Tool 'start_process' @{ process = 'Ping' }
  Check 'Prozesse: start_process Ping -> running' (-not $s.err -and $s.data.processes[0].status -eq 'running') $s.text
  $o = Wait-Output 'Ping' '127\.0\.0\.1'
  Check 'Prozesse: Ausgabe von ping lesbar (read_process_output)' ("$($o.data.text)" -match '127\.0\.0\.1' -and $o.data.note -match 'untrusted|vertrauensw') "$($o.data.text)"
  Check 'Prozesse: started_at gesetzt' ((Get-Proc 'Ping').started_at -gt 0)
  $s = Tool 'stop_process' @{ process = 'Ping' }
  Check 'Prozesse: stop_process -> stopping' (-not $s.err -and $s.data.processes[0].status -in @('stopping', 'stopped')) $s.text
  $p = Wait-Proc 'Ping' @('stopped') 10
  Check 'Prozesse: Strg+C stoppt ping (Status stopped, nicht crashed)' ($p.status -eq 'stopped') ($p | ConvertTo-Json -Compress)

  [void](Tool 'start_process' @{ process = 'Crash' })
  $p = Wait-Proc 'Crash' @('crashed', 'exited', 'failed') 15
  Check 'Prozesse: exit 3 -> crashed, exit_code 3' ($p.status -eq 'crashed' -and $p.exit_code -eq 3) ($p | ConvertTo-Json -Compress)
  $o = Tool 'read_process_output' @{ process = 3 }
  Check 'Prozesse: Ausgabe nach dem Ende weiter lesbar' ("$($o.data.text)" -match 'vor-dem-absturz') "$($o.data.text)"
  [void](Tool 'start_process' @{ process = 'Missing' })
  $p = Wait-Proc 'Missing' @('crashed', 'exited', 'failed') 15
  Check 'Prozesse: unbekannter Befehl -> failed ohne exit_code' ($p.status -eq 'failed' -and $null -eq $p.exit_code) ($p | ConvertTo-Json -Compress)
  [void](Tool 'start_process' @{ process = 'Cmd' })
  $p = Wait-Proc 'Cmd' @('crashed', 'exited', 'failed') 15
  $o = Tool 'read_process_output' @{ process = 'Cmd' }
  Check 'Prozesse: cmd mit && und Anfuehrungszeichen, exit 4 -> crashed 4' ($p.status -eq 'crashed' -and $p.exit_code -eq 4 -and "$($o.data.text)" -match '"a b"') "$($p.status)/$($p.exit_code): $($o.data.text)"

  $s = Tool 'start_process' @{ group = 'G' }
  Check 'Prozesse: start_process group -> Reihenfolge Http, Ping' (-not $s.err -and ($s.data.processes.name -join ',') -eq 'Http,Ping') $s.text
  $p = Wait-Proc 'Http' @('running') 5
  $t0 = Get-Date
  while (-not @($p.urls).Count -and ((Get-Date) - $t0).TotalSeconds -lt 15) { Start-Sleep -Milliseconds 400; $p = Get-Proc 'Http' }
  $url = @($p.urls) | Select-Object -First 1
  Check 'Prozesse: lokale Adresse erkannt (urls)' ($url -match '^http://127\.0\.0\.1:\d+/$') ($p | ConvertTo-Json -Compress)
  Check 'Prozesse: Fehlerzeile gezaehlt (error_lines)' ($p.error_lines -ge 1) "error_lines=$($p.error_lines)"
  $port = if ($url -match ':(\d+)/') { [int]$Matches[1] } else { 0 }
  $ok = $false; if ($port) { try { $c = [Net.Sockets.TcpClient]::new('127.0.0.1', $port); $ok = $c.Connected; $c.Close() } catch {} }
  Check 'Prozesse: Listener des Prozesses erreichbar' $ok "port=$port"
  $s = Tool 'stop_process' @{ group = 'G' }
  Check 'Prozesse: stop_process group in umgekehrter Reihenfolge' (-not $s.err -and ($s.data.processes.name -join ',') -eq 'Ping,Http') $s.text
  $a = Wait-Proc 'Http' @('stopped') 12; $b = Wait-Proc 'Ping' @('stopped') 12
  Check 'Prozesse: Gruppe gestoppt' ($a.status -eq 'stopped' -and $b.status -eq 'stopped') "$($a.status)/$($b.status)"

  [void](Tool 'start_process' @{ process = 'Env' })
  $o = Wait-Output 'Env' 'env='
  Check 'Prozesse: verschluesselte Umgebungsvariable kommt an' ("$($o.data.text)" -match "env=$secret") "$($o.data.text)"
  $l = Tool 'list_processes'
  Check 'Prozesse: Wert nie in list_processes' ($l.text -notmatch [regex]::Escape($secret))
  $r = Tool 'restart_process' @{ process = 'Env' }
  $p = Wait-Proc 'Env' @('running') 12
  Check 'Prozesse: restart_process -> wieder running' (-not $r.err -and $p.status -eq 'running') ($p | ConvertTo-Json -Compress)
  [void](Tool 'stop_process' @{ process = 'Env' })

  [void](Tool 'start_process' @{ process = 'Zaeh' })
  [void](Wait-Output 'Zaeh' 'zaeh-laeuft')
  $t0 = Get-Date; while (-not @(Get-Marker).Count -and ((Get-Date) - $t0).TotalSeconds -lt 10) { Start-Sleep -Milliseconds 300 }
  Check 'Prozesse: Kindprozess (eigene Konsole, ohne Strg+C) laeuft' (@(Get-Marker).Count -ge 1)
  [void](Tool 'stop_process' @{ process = 'Zaeh' })
  Start-Sleep 2
  Check 'Prozesse: ignoriert Strg+C -> nach 2 s noch stopping' ((Get-Proc 'Zaeh').status -eq 'stopping') (Get-Proc 'Zaeh').status
  $p = Wait-Proc 'Zaeh' @('stopped') 12
  Check 'Prozesse: nach 5 s ganzer Baum beendet (stopped)' ($p.status -eq 'stopped') $p.status
  Start-Sleep 1
  Check 'Prozesse: Kindprozess mit beendet (Job Object)' (@(Get-Marker).Count -eq 0) "$(@(Get-Marker).Count) uebrig"

  [void](Tool 'start_process' @{ process = 'Zaeh' })
  $t0 = Get-Date; while (-not @(Get-Marker).Count -and ((Get-Date) - $t0).TotalSeconds -lt 10) { Start-Sleep -Milliseconds 300 }
  Stop-Hub
  Start-Sleep 2
  Check 'Prozesse: Hub beendet (kill) -> Programme enden mit (KILL_ON_JOB_CLOSE)' (@(Get-Marker).Count -eq 0) "$(@(Get-Marker).Count) uebrig"
  Get-Marker | ForEach-Object { Stop-Process -Id $_.ProcessId -Force -ErrorAction SilentlyContinue }
  Remove-Item -LiteralPath "$data\processes.json" -ErrorAction SilentlyContinue

  Write-Host "== Editor (#54): editor_* ueber MCP (Stufen, Schalter, Pfadregeln, Speichern, grosse Datei), --edit" -ForegroundColor Cyan
  # Testdateien im Arbeitsordner; die grosse Datei entsteht erst hier (ueber der Schwelle large_file_mb = 1).
  $edDir = Join-Path $work 'editor'
  New-Item -ItemType Directory -Force $edDir | Out-Null
  $utf8 = New-Object Text.UTF8Encoding $false
  $edA = Join-Path $edDir 'notes.txt'; [IO.File]::WriteAllText($edA, "alpha`r`nbeta`r`ngamma`r`n", $utf8)
  $edB = Join-Path $edDir 'gone.txt'; [IO.File]::WriteAllText($edB, "will be deleted`n", $utf8)
  $edC = Join-Path $edDir 'changed.txt'; [IO.File]::WriteAllText($edC, "one`n", $utf8)
  $edD = Join-Path $edDir 'from-cli.txt'; [IO.File]::WriteAllText($edD, "opened with --edit`n", $utf8)
  # Formatieren (#381): kompaktes JSON, JSONC mit Kommentar, ungueltiges JSON, XML.
  $edJ = Join-Path $edDir 'data.json'; [IO.File]::WriteAllText($edJ, "{`"b`":1,`"a`":[1,2]}`n", $utf8)
  $edJc = Join-Path $edDir 'tsconfig.json'; [IO.File]::WriteAllText($edJc, "{`n  // keep me`n  `"x`": 1,`n}`n", $utf8)
  $edBad = Join-Path $edDir 'bad.json'; [IO.File]::WriteAllText($edBad, "{`"a`": 1,,}`n", $utf8)
  $edX = Join-Path $edDir 'doc.xml'; [IO.File]::WriteAllText($edX, "<a><b>1</b><!-- c --></a>`n", $utf8)
  $edBig = Join-Path $edDir 'big.log'
  $sw = New-Object IO.StreamWriter($edBig, $false, $utf8)
  for ($i = 1; $i -le 40000; $i++) { $sw.WriteLine("line $i of a large file generated at runtime for the editor smoke test") }
  $sw.Close()
  # Schalter der Editor-Einstellungen vor jedem Start setzen (settings.json -> editor).
  $script:edSw = @{ open = $false; edit = $false; save = $false; format = $false }
  $script:prestart = {
    $f = "$data\settings.json"; $j = [IO.File]::ReadAllText($f).TrimStart([char]0xFEFF) | ConvertFrom-Json
    $ed = [pscustomobject]@{ large_file_mb = 1; agent_open = $script:edSw.open; agent_edit = $script:edSw.edit; agent_save = $script:edSw.save; agent_format = [bool]$script:edSw.format }
    $j | Add-Member -Force editor $ed
    [IO.File]::WriteAllText($f, ($j | ConvertTo-Json -Depth 10), (New-Object Text.UTF8Encoding $false))
  }
  # Stufe Lesen: lesen ja, schreiben nein (auch mit eingeschalteten Schaltern).
  $script:edSw = @{ open = $true; edit = $true; save = $true; format = $true }
  Set-Level 'Read' -Empty
  [void](Rpc 'initialize' @{ protocolVersion = '2025-06-18'; capabilities = @{}; clientInfo = @{ name = 'smoke'; version = '1' } })
  Start-Sleep 2
  $names = @((Rpc 'tools/list').result.tools | ForEach-Object name)
  Check 'Editor, Nur lesen: editor_list/editor_read ja, open/edit/save nein' ($names -contains 'editor_list' -and $names -contains 'editor_read' -and $names -notcontains 'editor_open' -and $names -notcontains 'editor_edit' -and $names -notcontains 'editor_save') ($names -join ',')
  $el = Tool 'editor_list'
  Check 'Editor, Nur lesen: editor_list ohne offene Dokumente, Schalter gemeldet' (-not $el.err -and @($el.data.docs).Count -eq 0 -and $el.data.agents.open -eq $true) $el.text
  Check 'Editor, Nur lesen: editor_open verboten (Stufe)' ((Code (Tool 'editor_open' @{ path = $edA })) -eq 'forbidden')
  # format_file (#381): validate ab Lesen, alles andere erst bei Steuern.
  Check 'Format, Nur lesen: format_file in tools/list' ($names -contains 'format_file') ($names -join ',')
  Check 'Format, Nur lesen: format_file op=format verboten (Stufe)' ((Code (Tool 'format_file' @{ path = $edJ; op = 'format' })) -eq 'forbidden')
  Check 'Format, Nur lesen: format_file op=validate erlaubt (Datei nicht offen -> not_found)' ((Code (Tool 'format_file' @{ path = $edJ; op = 'validate' })) -eq 'not_found')
  # Stufe Steuern, Schalter aus: forbidden mit Hinweis auf den Schalter.
  $script:edSw = @{ open = $false; edit = $false; save = $false; format = $false }
  Set-Level 'Control' -Empty
  [void](Rpc 'initialize' @{ protocolVersion = '2025-06-18'; capabilities = @{}; clientInfo = @{ name = 'smoke'; version = '1' } })
  Start-Sleep 2
  $names = @((Rpc 'tools/list').result.tools | ForEach-Object name)
  Check 'Editor, Steuern: alle fuenf editor_* in tools/list' (@('editor_list', 'editor_read', 'editor_open', 'editor_edit', 'editor_save' | Where-Object { $names -contains $_ }).Count -eq 5) ($names -join ',')
  $eo = Tool 'editor_open' @{ path = $edA }
  Check 'Editor, Schalter aus: editor_open forbidden mit Hinweis auf den Schalter' ((Code $eo) -eq 'forbidden' -and $eo.text -match 'Agents may open files|Agenten d') $eo.text
  Check 'Editor, Schalter aus: editor_edit forbidden' ((Code (Tool 'editor_edit' @{ doc = 1; old_text = 'a'; new_text = 'b' })) -eq 'forbidden')
  Check 'Editor, Schalter aus: editor_save forbidden' ((Code (Tool 'editor_save' @{ doc = 1 })) -eq 'forbidden')
  $ff = Tool 'format_file' @{ path = $edJ; op = 'format' }
  Check 'Format, Schalter aus: format_file op=format forbidden mit Hinweis auf den Schalter' ((Code $ff) -eq 'forbidden' -and $ff.text -match 'Agents may format|Agenten d') $ff.text
  Check 'Format: format_file ohne op -> bad_args' ((Code (Tool 'format_file' @{ path = $edJ })) -eq 'bad_args')
  Check 'Editor, Schalter aus: Datei unveraendert' ([IO.File]::ReadAllText($edA) -eq "alpha`r`nbeta`r`ngamma`r`n")
  # Steuern, alle Schalter an.
  $script:edSw = @{ open = $true; edit = $true; save = $true; format = $true }
  Set-Level 'Control' -Empty
  [void](Rpc 'initialize' @{ protocolVersion = '2025-06-18'; capabilities = @{}; clientInfo = @{ name = 'smoke'; version = '1' } })
  Start-Sleep 2
  foreach ($bad in @('\\localhost\c$\Windows\win.ini', '\\wsl.localhost\Ubuntu\etc\hosts', '\\.\PhysicalDrive0', '\\?\C:\Windows\win.ini', 'C:\NUL', "$edDir\con.txt")) {
    Check "Editor: editor_open lehnt $bad ab (forbidden)" ((Code (Tool 'editor_open' @{ path = $bad })) -eq 'forbidden')
  }
  Check 'Editor: editor_open mit relativem Pfad -> bad_args' ((Code (Tool 'editor_open' @{ path = 'notes.txt' })) -eq 'bad_args')
  Check 'Editor: editor_open mit Ordner -> bad_args' ((Code (Tool 'editor_open' @{ path = $edDir })) -eq 'bad_args')
  Check 'Editor: editor_open fehlende Datei -> not_found' ((Code (Tool 'editor_open' @{ path = (Join-Path $edDir 'fehlt.txt') })) -eq 'not_found')
  $oa = Tool 'editor_open' @{ path = $edA }
  $A = $oa.data.doc
  Check 'Editor: editor_open oeffnet die Datei in einem neuen Tab' (-not $oa.err -and $A -and $oa.data.already_open -eq $false -and $oa.data.opened_by_agent -eq $true -and $oa.data.eol -eq 'crlf') $oa.text
  $oa2 = Tool 'editor_open' @{ path = $edA.ToUpper() }
  Check 'Editor: zweites editor_open (andere Schreibweise) -> derselbe Tab, already_open' ($oa2.data.doc -eq $A -and $oa2.data.already_open -eq $true) $oa2.text
  $rd = Tool 'editor_read' @{ doc = $A }
  Check 'Editor: editor_read liefert Zeilen mit \n und Hinweis untrusted' ($rd.data.text -eq "alpha`nbeta`ngamma`n" -and $rd.data.start_line -eq 1 -and $rd.data.note -match 'untrusted|vertrauensw') $rd.text
  $rp = Tool 'editor_read' @{ doc = $edA; offset = 2; limit = 1 }
  Check 'Editor: editor_read ueber den Pfad mit offset/limit' ($rp.data.text -eq 'beta' -and $rp.data.more_below -eq $true) $rp.text
  Check 'Editor: editor_edit mehrdeutig -> bad_args' ((Code (Tool 'editor_edit' @{ doc = $A; old_text = 'a'; new_text = 'x' })) -eq 'bad_args')
  Check 'Editor: editor_edit nicht gefunden -> not_found' ((Code (Tool 'editor_edit' @{ doc = $A; old_text = 'delta'; new_text = 'x' })) -eq 'not_found')
  $ed1 = Tool 'editor_edit' @{ doc = $A; old_text = 'beta'; new_text = 'BETA' }
  Check 'Editor: editor_edit ungespeichert, vom Agenten markiert' (-not $ed1.err -and $ed1.data.dirty -eq $true -and $ed1.data.agent_edited -eq $true -and $ed1.data.replaced -eq 1) $ed1.text
  Check 'Editor: Datei auf der Platte noch unveraendert' ([IO.File]::ReadAllText($edA) -eq "alpha`r`nbeta`r`ngamma`r`n")
  $ed2 = Tool 'editor_edit' @{ doc = $A; start_line = 1; end_line = 0; text = 'top' }
  Check 'Editor: editor_edit mit Zeilen (einfuegen)' (-not $ed2.err -and (Tool 'editor_read' @{ doc = $A; limit = 2 }).data.text -eq "top`nalpha") $ed2.text
  $sv = Tool 'editor_save' @{ doc = $A }
  Check 'Editor: editor_save speichert die offene Datei' (-not $sv.err -and $sv.data.saved -eq $true) $sv.text
  Check 'Editor: gespeichert mit den Zeilenenden der Datei (CRLF)' ([IO.File]::ReadAllText($edA) -eq "top`r`nalpha`r`nBETA`r`ngamma`r`n") ([IO.File]::ReadAllText($edA))
  Check 'Editor: editor_save ohne Aenderungen -> saved false' ((Tool 'editor_save' @{ doc = $A }).data.saved -eq $false)
  # Unbenannter Tab: anlegen ja, speichern nur der Nutzer.
  $un = Tool 'editor_open' @{ untitled = $true; text = "draft`r`ntext" }
  Check 'Editor: unbenannter Tab mit Text' (-not $un.err -and $un.data.untitled -eq $true -and $null -eq $un.data.path) $un.text
  Check 'Editor: unbenannter Tab nicht speicherbar (forbidden)' ((Code (Tool 'editor_save' @{ doc = $un.data.doc })) -eq 'forbidden')
  # Auf der Platte geloescht: nicht speichern.
  $ob = Tool 'editor_open' @{ path = $edB }
  [void](Tool 'editor_edit' @{ doc = $ob.data.doc; old_text = 'deleted'; new_text = 'kept' })
  Remove-Item -LiteralPath $edB -Force
  Start-Sleep 4
  Check 'Editor: geloeschte Datei nicht speicherbar (forbidden), nicht neu angelegt' ((Code (Tool 'editor_save' @{ doc = $ob.data.doc })) -eq 'forbidden' -and -not (Test-Path -LiteralPath $edB))
  # Auf der Platte geaendert + ungespeichert: conflict, der Nutzer entscheidet.
  $oc = Tool 'editor_open' @{ path = $edC }
  [void](Tool 'editor_edit' @{ doc = $oc.data.doc; old_text = 'one'; new_text = 'mine' })
  [IO.File]::WriteAllText($edC, "theirs`n", $utf8)
  Start-Sleep 4
  $lc = @((Tool 'editor_list').data.docs) | Where-Object { $_.id -eq $oc.data.doc }
  Check 'Editor: Plattenaenderung erkannt (disk = changed)' ($lc.disk -eq 'changed') ($lc | ConvertTo-Json -Compress)
  Check 'Editor: Speichern bei Plattenaenderung -> conflict, Datei bleibt' ((Code (Tool 'editor_save' @{ doc = $oc.data.doc })) -eq 'conflict' -and [IO.File]::ReadAllText($edC) -eq "theirs`n")
  # format_file (#381): offene JSON/XML-Dokumente, Ergebnis ungespeichert und vom Agenten markiert.
  $oj = Tool 'editor_open' @{ path = $edJ }
  $J = $oj.data.doc
  $fv = Tool 'format_file' @{ path = $edJ; op = 'validate' }
  Check 'Format: validate -> valid, nichts geaendert' (-not $fv.err -and $fv.data.valid -eq $true -and $fv.data.changed -eq $false -and $fv.data.kind -eq 'json' -and $fv.data.dirty -eq $false) $fv.text
  $fm = Tool 'format_file' @{ doc = $J; op = 'format' }
  Check 'Format: format -> geaendert, ungespeichert, vom Agenten markiert' (-not $fm.err -and $fm.data.changed -eq $true -and $fm.data.dirty -eq $true -and $fm.data.agent_edited -eq $true -and $fm.data.lines -gt 2) $fm.text
  Check 'Format: Datei auf der Platte unveraendert' ([IO.File]::ReadAllText($edJ) -eq "{`"b`":1,`"a`":[1,2]}`n")
  $fs = Tool 'format_file' @{ doc = $J; op = 'sort_keys' }
  $tj = (Tool 'editor_read' @{ doc = $J }).data.text
  Check 'Format: sort_keys sortiert die Schluessel' (-not $fs.err -and $tj.IndexOf('"a"') -lt $tj.IndexOf('"b"')) $tj
  $fn = Tool 'format_file' @{ doc = $J; op = 'minify' }
  Check 'Format: minify -> eine Zeile' (-not $fn.err -and ((Tool 'editor_read' @{ doc = $J }).data.text.Trim() -notmatch "`n")) $fn.text
  [void](Tool 'editor_open' @{ path = $edBad })
  $fb = Tool 'format_file' @{ path = $edBad; op = 'format' }
  Check 'Format: ungueltig -> valid false mit Zeile/Spalte, nichts geaendert' (-not $fb.err -and $fb.data.valid -eq $false -and $fb.data.changed -eq $false -and $fb.data.error.line -eq 1 -and $fb.data.error.column -ge 1 -and $fb.data.dirty -eq $false) $fb.text
  [void](Tool 'editor_open' @{ path = $edJc })
  Check 'Format: JSONC minify ohne remove_comments -> bad_args' ((Code (Tool 'format_file' @{ path = $edJc; op = 'minify' })) -eq 'bad_args')
  $fc = Tool 'format_file' @{ path = $edJc; op = 'format' }
  Check 'Format: JSONC format behaelt den Kommentar' (-not $fc.err -and $fc.data.kind -eq 'jsonc' -and (Tool 'editor_read' @{ doc = $edJc }).data.text -match 'keep me') $fc.text
  $fcm = Tool 'format_file' @{ path = $edJc; op = 'minify'; remove_comments = $true }
  Check 'Format: JSONC minify mit remove_comments entfernt ihn' (-not $fcm.err -and (Tool 'editor_read' @{ doc = $edJc }).data.text -notmatch 'keep me') $fcm.text
  [void](Tool 'editor_open' @{ path = $edX })
  $fx = Tool 'format_file' @{ path = $edX; op = 'format' }
  Check 'Format: XML format behaelt den Kommentar' (-not $fx.err -and $fx.data.kind -eq 'xml' -and (Tool 'editor_read' @{ doc = $edX }).data.text -match '<!-- c -->') $fx.text
  Check 'Format: XML sort_keys -> bad_args' ((Code (Tool 'format_file' @{ path = $edX; op = 'sort_keys' })) -eq 'bad_args')
  Check 'Format: Text ist weder JSON noch XML -> not_supported' ((Code (Tool 'format_file' @{ path = $edA; op = 'validate' })) -eq 'not_supported')
  # Grosse Datei (zur Laufzeit erzeugt): lesen ja (gedeckelt), bearbeiten nein.
  $og = Tool 'editor_open' @{ path = $edBig }
  Check 'Editor: grosse Datei geoeffnet (large)' (-not $og.err -and $og.data.large -eq $true -and $og.data.lines -ge 40000) $og.text
  $rg = Tool 'editor_read' @{ doc = $og.data.doc; offset = 39995; limit = 10 }
  Check 'Editor: grosse Datei, Ende lesbar' ($rg.data.text -match 'line 40000 of' -and $rg.data.more_below -eq $false) $rg.text
  $rm = Tool 'editor_read' @{ doc = $og.data.doc; limit = 5000 }
  Check 'Editor: editor_read hoechstens 2000 Zeilen / 128 KB' ((($rm.data.end_line - $rm.data.start_line + 1) -le 2000) -and $rm.data.text.Length -le 131072 -and $rm.data.more_below -eq $true) "$($rm.data.start_line)-$($rm.data.end_line)"
  Check 'Editor: grosse Datei nicht bearbeitbar (too_large)' ((Code (Tool 'editor_edit' @{ doc = $og.data.doc; old_text = 'line 1 of'; new_text = 'x' })) -eq 'too_large')
  Check 'Format: grosse Datei nicht formatierbar (too_large)' ((Code (Tool 'format_file' @{ doc = $og.data.doc; op = 'format' })) -eq 'too_large')
  # --edit aus einer zweiten Instanz: Datei erscheint als Tab (nicht vom Agenten geoeffnet).
  $second = Start-Process "$dir\paneldeck.exe" -PassThru -WindowStyle Hidden -ArgumentList '--edit', "`"$edD`""
  [void]$second.WaitForExit(10000)
  Start-Sleep 2
  $ld = @((Tool 'editor_list').data.docs) | Where-Object { $_.path -and $_.path -ieq $edD }
  Check 'Editor: paneldeck --edit oeffnet die Datei im laufenden Hub' ($ld -and $ld.opened_by_agent -eq $false) ((@((Tool 'editor_list').data.docs) | ForEach-Object path) -join ',')
  Check 'Editor: zweite Instanz hat sich beendet (Weiterleitung)' ($second.HasExited) ''
  # Protokoll ohne Dateiinhalt
  $log = if (Test-Path "$data\mcp-log.jsonl") { [IO.File]::ReadAllText("$data\mcp-log.jsonl") } else { '' }
  Check 'Editor: mcp-log.jsonl nennt editor_*, aber keinen Dateiinhalt' ($log -match 'editor_edit' -and $log -notmatch 'BETA' -and $log -notmatch 'draft') ''
  Check 'Format: mcp-log.jsonl nennt format_file, aber keinen Dateiinhalt' ($log -match 'format_file' -and $log -notmatch 'keep me') ''
  $script:prestart = $null

  Write-Host "== Shell-Integration: Hilfsdateien beim Beenden geloescht (#57)" -ForegroundColor Cyan
  # Eigener Hub ohne Sitzungen (keine Rueckfrage beim Schliessen); Fenster normal schliessen statt Prozess beenden.
  Set-Level 'Control' -Empty
  if ($script:hub -and -not $script:hub.HasExited) {
    $si = "$data\shell-integration"
    New-Item -ItemType Directory -Force "$si\zsh" | Out-Null
    [IO.File]::WriteAllText("$si\zsh\.zshenv", '# smoke')
    [void]$script:hub.CloseMainWindow()
    if ($script:hub.WaitForExit(15000)) {
      Check 'Beenden: Ordner shell-integration im Datenordner entfernt' (-not (Test-Path -LiteralPath $si)) $si
    } else {
      Skip 'Beenden: Hilfsdateien entfernt' 'Hub hat sich nicht ohne Rueckfrage beendet (wird abgebrochen)'
    }
  } else {
    Skip 'Beenden: Hilfsdateien entfernt' 'kein laufender Hub'
  }

  # ---------------------------------------------------------------- Tutorial (#143, nur mit dem Test-Build)
  # Die Release-.exe darf die Test-Schnittstelle nie enthalten (Feature tutorial-test, siehe tools/release.sh).
  $relBytes = [IO.File]::ReadAllBytes($Exe)
  $hookName = [Text.Encoding]::ASCII.GetString($relBytes) -match 'PANELDECK_TUTORIAL_TEST'
  Check 'Tutorial: gepruefte .exe ohne Test-Schnittstelle (kein PANELDECK_TUTORIAL_TEST)' (-not $hookName)
  if (-not $TutorialExe) {
    Skip 'Tutorial (#143): Touren, Ziele, Erkennung, Aufraeumen' 'kein -TutorialExe (Test-Build: cargo build --release --features tutorial-test)'
  } else {
    Write-Host "== Tutorial (#143): Test-Build $TutorialExe" -ForegroundColor Cyan
    Stop-Hub
    $TutorialExe = (Resolve-Path $TutorialExe).Path
    $u8 = New-Object Text.UTF8Encoding $false
    $mainDir = $dir; $mainData = $data
    $dir = New-Item -ItemType Directory -Force "$work\tut"
    Copy-Item $TutorialExe "$dir\paneldeck.exe"
    if ($hasConPty) { Copy-Item "$exeDir\conpty.dll", "$exeDir\OpenConsole.exe" $dir }
    $data = "$dir\paneldeck-data"
    New-Item -ItemType Directory -Force $data | Out-Null
    $hook = "$dir\hook"
    New-Item -ItemType Directory -Force $hook | Out-Null
    # Steuern fuer die MCP-Pruefungen; Profil 1 = Schein-Umgebung fuer jedes Terminal (das Projekt "Tutorial" erbt das
    # Standardprofil des aktiven Projekts).
    $ts = [pscustomobject]@{ mcp_level = 'Control'; scrollback_lines = 1000; profiles = @([pscustomobject]@{ id = 1; name = 'Smoke-Isolation'; env = $ptyEnv }) }
    if ($ConPty -eq 'system') { $ts | Add-Member -Force bundled_conpty $false }
    [IO.File]::WriteAllText("$data\settings.json", ($ts | ConvertTo-Json -Depth 10), $u8)
    [IO.File]::WriteAllText("$data\state.json", '{"projects":[{"id":1,"name":"Standard","profile":1}],"active_project":1,"next_id":1}', $u8)
    function Tut-Cmd([string]$line) {
      $f = "$hook\tutorial-cmd.txt"; $t0 = Get-Date
      while ((Test-Path -LiteralPath $f) -and ((Get-Date) - $t0).TotalSeconds -lt 5) { Start-Sleep -Milliseconds 100 }
      [IO.File]::WriteAllText("$f.tmp", $line + "`n", (New-Object Text.UTF8Encoding $false))
      # Die Instanz loescht die vorige Befehlsdatei; solange Windows sie noch als "Loeschen ausstehend" fuehrt,
      # schlaegt das Umbenennen fehl – dann kurz warten und erneut versuchen, statt den Befehl zu verlieren.
      for ($try = 0; $try -lt 50; $try++) {
        try { Move-Item -Force -LiteralPath "$f.tmp" -Destination $f -ErrorAction Stop; return }
        catch { Start-Sleep -Milliseconds 100 }
      }
      Write-Host "  WARN  Tutorial-Befehl '$line' liess sich nicht schreiben"
    }
    function Tut-Status {
      $f = "$hook\tutorial-status.json"
      if (Test-Path -LiteralPath $f) { try { [IO.File]::ReadAllText($f) | ConvertFrom-Json } catch { $null } } else { $null }
    }
    function Tut-Wait([scriptblock]$cond, [int]$ms = 12000) {
      $t0 = Get-Date
      do {
        $s = Tut-Status
        if ($s -and (& $cond $s)) { return $s }
        Start-Sleep -Milliseconds 150
      } while (((Get-Date) - $t0).TotalMilliseconds -lt $ms)
      Tut-Status
    }
    function Tut-Sessions { @((Tool 'list_sessions').data.sessions | Where-Object { $_.project -eq 'Tutorial' }) }
    $env:PANELDECK_TUTORIAL_TEST = $hook
    $style = if ($Visible) { 'Normal' } else { 'Minimized' }
    $script:hub = Start-Process "$dir\paneldeck.exe" -PassThru -WindowStyle $style
    Remove-Item Env:PANELDECK_TUTORIAL_TEST
    $t0 = Get-Date
    while (-not (Test-Path "$data\ipc.json") -and ((Get-Date) - $t0).TotalSeconds -lt 20) { Start-Sleep -Milliseconds 200 }
    Start-Sleep 2
    Start-Bridge
    [void](Rpc 'initialize' @{ protocolVersion = '2025-06-18'; capabilities = @{}; clientInfo = @{ name = 'smoke-tutorial'; version = '1' } })

    $s = Tut-Wait { param($x) $x.card }
    Check 'Tutorial: Karte "Kurze Tour?" beim ersten Start' ($s -and $s.card) ($s | ConvertTo-Json -Depth 3 -Compress)
    Tut-Cmd 'card later'
    $s = Tut-Wait { param($x) -not $x.card }
    Check 'Tutorial: "Spaeter" blendet die Karte aus' ($s -and -not $s.card)
    $saved = if (Test-Path "$data\tutorial.json") { [IO.File]::ReadAllText("$data\tutorial.json") | ConvertFrom-Json } else { $null }
    Check 'Tutorial: tutorial.json im Datenordner (card = later)' ($saved -and $saved.card -eq 'later')
    $cat = @($s.catalogue)
    Check 'Tutorial: Katalog mit kurzer Tour und Kapiteln' ($cat.Count -ge 7 -and $cat[0].id -eq 'basics') "$($cat.Count) Touren"
    $expert0 = [bool]$s.expert

    foreach ($tour in $cat) {
      $tid = $tour.id; $steps = @($tour.steps)
      Tut-Cmd "start $tid"
      $s = Tut-Wait { param($x) $x.running -and $x.run.tour -eq $tid -and $x.run.index -eq 1 -and $x.run.ready }
      Check "Tutorial ${tid}: startet bei Schritt 1/$($steps.Count)" ($s -and $s.run.total -eq $steps.Count -and $s.project.exists -and $s.project.active) ($s.run | ConvertTo-Json -Compress)
      $missingSpots = @()
      for ($i = 1; $i -le $steps.Count; $i++) {
        $s = Tut-Wait { param($x) $x.running -and $x.run.index -eq $i -and $x.run.ready }
        if (-not ($s -and $s.running -and $s.run.index -eq $i)) { Check "Tutorial ${tid}: Schritt $i erreicht" $false ($s | ConvertTo-Json -Depth 3 -Compress); break }
        $r = $s.run
        if ($r.spot) {
          if ($Visible) { if (-not $r.spot_found) { $missingSpots += "$($r.step) ($($r.spot))" } }
        }
        $auto = $null
        switch ($r.step) {
          'basics.new_terminal' { $auto = { [void](Tool 'create_session' @{ name = 'Smoke-Tutorial' }) } }
          'basics.todo_add' { $auto = { [void](Tool 'add_todo' @{ title = 'Smoke-Tutorial-Todo' }) } }
          'todo.list' { $auto = { [void](Tool 'add_todo' @{ title = 'Smoke-Tutorial-Todo-2' }) } }
        }
        if ($auto) {
          & $auto
          $n = Tut-Wait { param($x) -not $x.running -or $x.run.index -gt $i } 15000
          Check "Tutorial ${tid}: $($r.step) geht nach der Aktion per MCP von selbst weiter" ($n -and (-not $n.running -or $n.run.index -gt $i)) ($n.run | ConvertTo-Json -Compress)
        } else {
          Tut-Cmd 'next'
          $n = Tut-Wait { param($x) -not $x.running -or $x.run.index -gt $i }
          if (-not ($n -and (-not $n.running -or $n.run.index -gt $i))) {
            $pending = Test-Path -LiteralPath "$hook\tutorial-cmd.txt"
            Write-Host "  WARN  Tutorial ${tid}: 'next' bei $($r.step) ohne Wirkung (Befehlsdatei noch da: $pending) - zweiter Versuch"
            # Der Test-Hook kann einen Befehl verbrauchen, waehrend die Tour den Schritt gerade neu aufbaut; einmal wiederholen.
            Tut-Cmd 'next'
            [void](Tut-Wait { param($x) -not $x.running -or $x.run.index -gt $i })
          }
        }
      }
      if ($Visible) {
        Check "Tutorial ${tid}: jedes Ziel sichtbar" ($missingSpots.Count -eq 0) ($missingSpots -join ', ')
      } else {
        Skip "Tutorial ${tid}: jedes Ziel sichtbar" 'Fenster minimiert (mit -Visible pruefen)'
      }
      $s = Tut-Wait { param($x) -not $x.running -and $x.ask -eq 'finished' }
      Check "Tutorial ${tid}: am Ende die Rueckfrage zum Tutorial-Projekt" ($s -and $s.ask -eq 'finished') ($s | ConvertTo-Json -Depth 2 -Compress)
      Check "Tutorial ${tid}: Fortschritt beendet" ($s -and $s.progress.$tid.finished)
      Check "Tutorial ${tid}: Expertenmodus wie vorher" ($s -and [bool]$s.expert -eq $expert0)
      if ($tid -eq 'basics') {
        # @(…): eine einzelne Sitzung kaeme sonst als PSCustomObject zurueck, und dessen .Count ist in PowerShell 5.1 $null.
        $mine = @(Tut-Sessions)
        Check 'Tutorial basics: Sitzungen im Projekt "Tutorial" (list_sessions)' ($mine.Count -ge 1) "$($mine.Count)"
        Check 'Tutorial basics: in der Tour angelegte To-dos gemerkt' (@($s.todos).Count -ge 1) "$(@($s.todos).Count)"
      }
      Tut-Cmd 'end delete'
      $s = Tut-Wait { param($x) -not $x.ask -and -not $x.project.exists }
      Check "Tutorial ${tid}: Loeschen entfernt das Projekt" ($s -and -not $s.ask -and -not $s.project.exists -and @($s.todos).Count -eq 0 -and $null -eq $s.plan_project) ($s | ConvertTo-Json -Depth 2 -Compress)
      Start-Sleep -Milliseconds 500
      Check "Tutorial ${tid}: keine Sitzung mehr im Projekt Tutorial" (@(Tut-Sessions).Count -eq 0)
    }
    $todos = Tool 'list_todos' @{ state = 'all'; limit = 200 }
    Check 'Tutorial: To-dos der Tour geloescht (list_todos)' (-not $todos.err -and -not ("$($todos.text)" -match 'Smoke-Tutorial-Todo')) "$($todos.text)"

    # Beenden, behalten, fortsetzen, zurueck
    Tut-Cmd 'start basics'
    [void](Tut-Wait { param($x) $x.running -and $x.run.index -eq 1 -and $x.run.ready })
    Tut-Cmd 'next'; [void](Tut-Wait { param($x) $x.run.index -eq 2 })
    Tut-Cmd 'back'
    $s = Tut-Wait { param($x) $x.run.index -eq 1 }
    Check 'Tutorial: Zurueck' ($s -and $s.run.index -eq 1)
    Tut-Cmd 'next'; [void](Tut-Wait { param($x) $x.run.index -eq 2 })
    Tut-Cmd 'skip'
    $s = Tut-Wait { param($x) $x.run.index -eq 3 }
    Check 'Tutorial: Ueberspringen merkt den Schritt' ($s -and $s.run.index -eq 3 -and @($s.progress.basics.skipped) -contains 'basics.sidebar') ($s.progress | ConvertTo-Json -Compress)
    Tut-Cmd 'quit'
    $s = Tut-Wait { param($x) -not $x.running -and $x.ask -eq 'paused' }
    Check 'Tutorial: Beenden fragt nach dem Projekt' ($s -and $s.ask -eq 'paused')
    Tut-Cmd 'end keep'
    $s = Tut-Wait { param($x) -not $x.ask }
    Check 'Tutorial: Behalten laesst das Projekt stehen' ($s -and $s.project.exists)
    Tut-Cmd 'continue'
    $s = Tut-Wait { param($x) $x.running -and $x.run.ready }
    Check 'Tutorial: Fortsetzen beim gemerkten Schritt' ($s -and $s.run.tour -eq 'basics' -and $s.run.step -eq 'basics.new_terminal') ($s.run | ConvertTo-Json -Compress)
    Tut-Cmd 'quit'; [void](Tut-Wait { param($x) $x.ask })
    Tut-Cmd 'end delete'; [void](Tut-Wait { param($x) -not $x.ask -and -not $x.project.exists })

    # Was ist neu
    Tut-Cmd 'whatsnew_from 2.5.0'
    $s = Tut-Wait { param($x) $x.hint }
    Check 'Tutorial: Hinweis "Neu in ..." nach einem Update' ($s -and $s.hint) "$($s.hint)"
    Tut-Cmd 'whatsnew'
    $s = Tut-Wait { param($x) $x.running -and $x.run.tour -eq 'whatsnew' }
    $since = @{}; foreach ($t in $cat) { foreach ($st in $t.steps) { $since[$st.id] = $st.since } }
    $old = @($s.run.steps | Where-Object { [version]$since[$_] -le [version]'2.5.0' })
    Check 'Tutorial: "Was ist neu" nur mit Schritten nach 2.5.0' ($s -and $s.run.total -ge 1 -and $old.Count -eq 0) "$($s.run.steps -join ', ')"
    Tut-Cmd 'quit'; [void](Tut-Wait { param($x) $x.ask })
    Tut-Cmd 'end delete'; [void](Tut-Wait { param($x) -not $x.ask })
    Tut-Cmd 'whatsnew_from 2.5.0'; [void](Tut-Wait { param($x) $x.hint })
    Tut-Cmd 'dismiss'
    $s = Tut-Wait { param($x) -not $x.hint }
    Check 'Tutorial: Hinweis laesst sich ausblenden' ($s -and -not $s.hint)
    Stop-Hub
    $dir = $mainDir; $data = $mainData
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
