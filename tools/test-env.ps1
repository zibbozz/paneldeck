# Gemeinsame Isolation fuer Test-Skripte (per Punkt einbinden: . "$PSScriptRoot\test-env.ps1").
#
# Testinstanzen von Paneldeck duerfen NIE das echte Claude Code des Rechners starten oder die echte Konfiguration
# (~/.claude, ~/.claude.json, Anmeldedaten) sehen. Dafuer:
#   - New-FakeClaudeBin: Ordner mit einem Ersatz-`claude` (claude.cmd fuer PowerShell/cmd, `claude` als sh-Skript fuer
#     Git Bash). Jeder Aufruf schreibt eine Zeile nach <Ordner>\calls.txt, danach liest der Ersatz Eingabezeilen und
#     schreibt sie nach <Ordner>\input.txt (so laesst sich pruefen, dass queue_prompt angekommen ist). Die Zeile
#     "exit-fake-claude" beendet ihn (wie ein Claude, der ohne SessionEnd-Hook endet).
#   - Enter-IsolatedTestEnv: setzt fuer diesen Prozess (und damit alles, was er startet) ein Schein-Profil
#     (USERPROFILE, HOME, APPDATA, LOCALAPPDATA, CLAUDE_CONFIG_DIR), entfernt CLAUDECODE, CLAUDE_*, ANTHROPIC_* und
#     NoDefaultCurrentDirectoryInExePath und baut PATH als <FakeBin>;<alle Eintraege ohne ein claude*>.
#     Danach muss `Get-Command claude -All` ausschliesslich im FakeBin-Ordner landen, sonst Abbruch (fail closed).
#   - Exit-IsolatedTestEnv: stellt die vorherige Umgebung exakt wieder her.
#   - Get-RealClaudeSnapshot / Test-RealClaudeUntouched: Schutzpruefung, dass das echte ~/.claude unberuehrt blieb.

function New-FakeClaudeBin([string]$Dir) {
  New-Item -ItemType Directory -Force $Dir | Out-Null
  $cmd = @(
    '@echo off',
    'setlocal EnableExtensions DisableDelayedExpansion',
    '>>"%~dp0calls.txt" echo(claude.cmd cwd=%CD% key=%PANELDECK_SESSION_KEY% args=%*',
    'echo Fake Claude (Paneldeck test) - the real Claude Code is NOT started.',
    'set /a empty=0',
    ':loop',
    'set "line="',
    'set /p "line=> "',
    'if not defined line (set /a empty+=1) else (set /a empty=0)',
    'if %empty% gtr 500 exit /b 0',
    'if not defined line goto loop',
    'setlocal EnableDelayedExpansion',
    '>>"%~dp0input.txt" echo(!line!',
    'if "!line!"=="exit-fake-claude" (endlocal & exit /b 0)',
    'endlocal',
    'goto loop',
    '')
  [IO.File]::WriteAllText((Join-Path $Dir 'claude.cmd'), ($cmd -join "`r`n"), (New-Object Text.UTF8Encoding $false))
  $sh = @(
    '#!/bin/sh',
    'd=$(cd "$(dirname "$0")" && pwd)',
    'printf ''claude(sh) cwd=%s key=%s args=%s\n'' "$PWD" "$PANELDECK_SESSION_KEY" "$*" >> "$d/calls.txt"',
    'echo "Fake Claude (Paneldeck test) - the real Claude Code is NOT started."',
    'while IFS= read -r line; do printf ''%s\n'' "$line" >> "$d/input.txt"; [ "$line" = "exit-fake-claude" ] && exit 0; done',
    '')
  [IO.File]::WriteAllText((Join-Path $Dir 'claude'), ($sh -join "`n"), (New-Object Text.UTF8Encoding $false))
  (Resolve-Path $Dir).Path
}

# PATH ohne jeden Eintrag, in dem ein claude* liegt.
function Get-PathWithoutClaude([string]$PathValue) {
  $names = @('claude', 'claude.exe', 'claude.cmd', 'claude.bat', 'claude.ps1', 'claude.com')
  $keep = foreach ($e in ($PathValue -split ';')) {
    $p = $e.Trim().Trim('"')
    if (-not $p) { continue }
    $hit = $false
    foreach ($n in $names) { try { if (Test-Path -LiteralPath (Join-Path $p $n)) { $hit = $true; break } } catch {} }
    if (-not $hit) { $p }
  }
  ($keep -join ';')
}

# Wirft, wenn `claude` mit dem aktuellen PATH irgendwo ausserhalb von $FakeBin gefunden wird
# (ohne $FakeBin: wenn es ueberhaupt gefunden wird).
function Assert-OnlyFakeClaude([string]$FakeBin) {
  $hits = @(Get-Command claude -All -CommandType Application, ExternalScript -ErrorAction SilentlyContinue)
  $bad = @($hits | Where-Object { -not $FakeBin -or -not $_.Source.StartsWith($FakeBin.TrimEnd('\') + '\', [StringComparison]::OrdinalIgnoreCase) })
  if ($bad.Count) { throw "SICHERHEIT: 'claude' wird ausserhalb des Ersatz-Ordners gefunden: $(($bad | ForEach-Object Source) -join ', ')" }
  if ($FakeBin -and -not $hits.Count) { throw "SICHERHEIT: Ersatz-claude in $FakeBin wird nicht gefunden" }
}

function Enter-IsolatedTestEnv([string]$Root, [string]$FakeBin = '', [switch]$NoConfigDir) {
  $saved = @{}
  Get-ChildItem Env: | ForEach-Object { $saved[$_.Name] = $_.Value }
  $prof = Join-Path $Root 'profile'
  New-Item -ItemType Directory -Force $prof, "$prof\AppData\Roaming", "$prof\AppData\Local", "$prof\.claude", "$prof\Desktop", "$prof\Documents" | Out-Null
  Get-ChildItem Env: | Where-Object { $_.Name -match '^(CLAUDE|ANTHROPIC)' -or $_.Name -eq 'NoDefaultCurrentDirectoryInExePath' } |
    ForEach-Object { Remove-Item -LiteralPath "Env:$($_.Name)" -ErrorAction SilentlyContinue }
  $env:USERPROFILE = $prof; $env:HOME = $prof
  $env:APPDATA = "$prof\AppData\Roaming"; $env:LOCALAPPDATA = "$prof\AppData\Local"
  if (-not $NoConfigDir) { $env:CLAUDE_CONFIG_DIR = "$prof\.claude" }
  $clean = Get-PathWithoutClaude $saved['Path']
  $env:PATH = if ($FakeBin) { "$FakeBin;$clean" } else { $clean }
  try { Assert-OnlyFakeClaude $FakeBin } catch { Exit-IsolatedTestEnv ([pscustomobject]@{ Saved = $saved }); throw }
  [pscustomobject]@{ Saved = $saved; Profile = $prof; FakeBin = $FakeBin }
}

function Exit-IsolatedTestEnv($Ctx) {
  if (-not $Ctx) { return }
  Get-ChildItem Env: | Where-Object { -not $Ctx.Saved.ContainsKey($_.Name) } | ForEach-Object { Remove-Item -LiteralPath "Env:$($_.Name)" -ErrorAction SilentlyContinue }
  foreach ($k in $Ctx.Saved.Keys) { Set-Item -LiteralPath "Env:$k" -Value $Ctx.Saved[$k] }
}

# Zeilen NAME=WERT fuer die Umgebung eines Paneldeck-Profils, das jedes Terminal des Hubs isoliert.
# Noetig, weil portable-pty unter Windows die Umgebung jedes Terminals aus der Registry auffrischt: PATH wird zu
# HKLM-PATH + HKCU-PATH (dort liegt z. B. ~/.local/bin mit dem echten claude), und jeder Wert aus HKCU\Environment
# ueberschreibt die Prozessumgebung des Hubs. Profil-Variablen setzt Paneldeck danach - sie gewinnen.
# Variablen CLAUDE*/ANTHROPIC*/NoDefaultCurrentDirectoryInExePath aus der Registry werden leer gesetzt.
function Get-PtyIsolationEnv($Ctx) {
  $prof = $Ctx.Profile
  New-Item -ItemType Directory -Force "$prof\AppData\Local\Temp" | Out-Null
  $lines = [ordered]@{
    PATH = $env:PATH; USERPROFILE = $prof; HOME = $prof; APPDATA = "$prof\AppData\Roaming"; LOCALAPPDATA = "$prof\AppData\Local"
    CLAUDE_CONFIG_DIR = "$prof\.claude"; TEMP = "$prof\AppData\Local\Temp"; TMP = "$prof\AppData\Local\Temp"
  }
  foreach ($k in 'HKCU:\Environment', 'HKLM:\System\CurrentControlSet\Control\Session Manager\Environment') {
    $p = Get-Item -LiteralPath $k -ErrorAction SilentlyContinue
    if ($p) {
      foreach ($n in $p.GetValueNames()) {
        if (($n -match '^(CLAUDE|ANTHROPIC)' -or $n -eq 'NoDefaultCurrentDirectoryInExePath') -and -not $lines.Contains($n)) { $lines[$n] = '' }
      }
    }
  }
  ($lines.GetEnumerator() | ForEach-Object { "$($_.Key)=$($_.Value)" }) -join "`n"
}

# Zustand des echten Claude-Profils (vor dem Ueberschreiben von USERPROFILE aufrufen).
function Get-RealClaudeSnapshot([string]$RealHome) {
  $f = { param($p) if (Test-Path -LiteralPath $p) { (Get-Item -LiteralPath $p).LastWriteTimeUtc.Ticks } else { $null } }
  $pd = Join-Path $RealHome '.claude\projects'
  [pscustomobject]@{
    Home     = $RealHome
    Projects = @(if (Test-Path -LiteralPath $pd) { Get-ChildItem -LiteralPath $pd -Directory | ForEach-Object Name })
    Json     = & $f (Join-Path $RealHome '.claude.json')
    Settings = & $f (Join-Path $RealHome '.claude\settings.json')
  }
}

# Vergleicht mit dem Schnappschuss. Ergebnis: @{ Violations = @(...); Warnings = @(...) }.
# Verstoss: neuer Ordner in ~/.claude/projects, dessen Name $Tag (den Namen des Test-Arbeitsordners) enthaelt, oder eine
# geaenderte ~/.claude.json bzw. ~/.claude/settings.json, die $Tag enthaelt. Eine geaenderte Datei ohne $Tag ist nur eine
# Warnung (ein gleichzeitig laufendes, echtes Claude Code schreibt ~/.claude.json staendig) - mit -Strict ein Verstoss.
function Test-RealClaudeUntouched($Snap, [string]$Tag, [switch]$Strict) {
  $now = Get-RealClaudeSnapshot $Snap.Home
  $v = @(); $w = @()
  $new = @($now.Projects | Where-Object { $Snap.Projects -notcontains $_ })
  $ours = @($new | Where-Object { $_ -like "*$Tag*" -or $_ -like '*hubsmoke*' })
  if ($ours.Count) { $v += "neuer Ordner im echten ~/.claude/projects: $($ours -join ', ')" }
  foreach ($k in @(@{ n = 'Json'; f = '.claude.json' }, @{ n = 'Settings'; f = '.claude\settings.json' })) {
    if ($now.($k.n) -ne $Snap.($k.n)) {
      $p = Join-Path $Snap.Home $k.f
      $txt = try { [IO.File]::ReadAllText($p) } catch { '' }
      if ($txt.Contains($Tag)) { $v += "echte ~/$($k.f -replace '\\','/') geaendert und nennt den Testordner $Tag" }
      elseif ($Strict) { $v += "echte ~/$($k.f -replace '\\','/') geaendert (Zeitstempel)" }
      else { $w += "echte ~/$($k.f -replace '\\','/') hat einen neuen Zeitstempel, nennt den Testordner aber nicht (vermutlich ein gleichzeitig laufendes Claude Code)" }
    }
  }
  [pscustomobject]@{ Violations = $v; Warnings = $w }
}
