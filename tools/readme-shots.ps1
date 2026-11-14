<#
.SYNOPSIS
  Erzeugt die README-Screenshots der Todo-Liste (Expertenmodus) aus einer Demo-Datenbank – ohne Maus, Tastatur oder Bildschirmkopie.

.DESCRIPTION
  1. baut (falls nötig) die Anwendung mit dem Feature `readme-shots` nach target\shots (Startansicht per Umgebungsvariable PANELDECK_SHOT),
  2. erzeugt je Sprache eine Demo-Datenbank (`cargo test --release demo_shots -- --ignored`, Projekt "Acme Shop", siehe src\planning\demo_shots),
  3. startet je Ansicht EINE Instanz aus einer Kopie der .exe (eigener Datenordner, Schein-USERPROFILE) in der Startansicht,
  4. nimmt das Bild mit dem MCP-Werkzeug `screenshot` auf (die Anwendung rendert sich selbst – das Fenster muss nur sichtbar sein),
  5. beendet die Instanz, rahmt das Bild (tools\readme_shot_post.py) und legt es unter docs\images\ (en) bzw. docs\images\de\ (de) ab.
  Die UI-Sperre (%TEMP%\paneldeck-ui-lock = /tmp/paneldeck-ui-lock in Git Bash) wird solange gehalten, wie Instanzen laufen.
  Es werden keine echten Daten, keine echte Registry und kein echtes ~/.claude benutzt.

.EXAMPLE
  powershell -ExecutionPolicy Bypass -File tools\readme-shots.ps1
  powershell -ExecutionPolicy Bypass -File tools\readme-shots.ps1 -Lang de -Views board -NoBuild
#>
param(
  [string[]]$Lang = @('en', 'de'),
  # Ansicht = Name der Datei (ohne .png) -> Wert von PANELDECK_SHOT
  [hashtable]$Views = [ordered]@{ 'todo-tickets' = 'tickets:{TICKET}'; 'todo-dashboard' = 'dashboard' },
  # Fenstergröße (Breite, Höhe) je Ansicht; ohne Eintrag gilt -Width/-Height. Das Ticket-Panel ist hoch, damit Kommentare und Zeiten zu sehen sind.
  [hashtable]$Sizes = @{ 'todo-tickets' = @(1300, 1260) },
  [int]$Width = 1240,
  [int]$Height = 800,
  [string]$OutDir = "$PSScriptRoot\..\docs\images",
  [string]$RawDir = '',
  [switch]$NoBuild,
  [switch]$KeepWork
)
$ErrorActionPreference = 'Stop'
$root = (Resolve-Path "$PSScriptRoot\..").Path
$OutDir = [IO.Path]::GetFullPath($OutDir)
$lock = Join-Path $env:TEMP 'paneldeck-ui-lock'
$work = Join-Path $env:TEMP ('pd-readme-shots-' + [guid]::NewGuid().ToString('N').Substring(0, 8))
$script:inst = $null
$script:haveLock = $false
Add-Type -MemberDefinition '[DllImport("user32.dll")] public static extern bool MoveWindow(IntPtr h, int x, int y, int w, int hh, bool r);' -Name W -Namespace Shots

function Get-Lock {
  while ($true) {
    try { New-Item -ItemType Directory $lock -ErrorAction Stop | Out-Null; $script:haveLock = $true; return }
    catch {
      $age = ((Get-Date) - (Get-Item $lock -ErrorAction SilentlyContinue).CreationTime).TotalMinutes
      if ($age -gt 10 -and -not (Get-Process paneldeck -ErrorAction SilentlyContinue)) { Remove-Item $lock -Force -ErrorAction SilentlyContinue; continue }
      Write-Host 'warte auf die UI-Sperre ...'; Start-Sleep 5
    }
  }
}
function Stop-Inst { if ($script:inst -and -not $script:inst.HasExited) { $script:inst.Kill(); [void]$script:inst.WaitForExit(3000) }; $script:inst = $null }

function Mcp-Screenshot($exe, $png) {
  $psi = New-Object System.Diagnostics.ProcessStartInfo
  $psi.FileName = $exe; $psi.Arguments = 'mcp'
  $psi.RedirectStandardInput = $true; $psi.RedirectStandardOutput = $true; $psi.RedirectStandardError = $true
  $psi.UseShellExecute = $false; $psi.CreateNoWindow = $true; $psi.StandardOutputEncoding = [Text.Encoding]::UTF8
  $b = [System.Diagnostics.Process]::Start($psi)
  function Send($o) { $l = ($o | ConvertTo-Json -Depth 10 -Compress); $by = (New-Object Text.UTF8Encoding $false).GetBytes($l + "`n"); $b.StandardInput.BaseStream.Write($by, 0, $by.Length); $b.StandardInput.BaseStream.Flush() }
  function Recv($ms = 30000) { $t = $b.StandardOutput.ReadLineAsync(); if ($t.Wait($ms)) { $t.Result | ConvertFrom-Json } else { $null } }
  try {
    Send @{ jsonrpc = '2.0'; id = 1; method = 'initialize'; params = @{ protocolVersion = '2025-06-18'; capabilities = @{}; clientInfo = @{ name = 'readme-shots'; version = '1' } } }
    [void](Recv)
    Send @{ jsonrpc = '2.0'; method = 'notifications/initialized' }
    # die Sitzung für die Titelleiste benennen (sonst steht dort der Pfad der Shell)
    Send @{ jsonrpc = '2.0'; id = 3; method = 'tools/call'; params = @{ name = 'list_sessions'; arguments = @{} } }
    $ls = Recv
    $sid = $ls.result.structuredContent.sessions[0].id
    Send @{ jsonrpc = '2.0'; id = 4; method = 'tools/call'; params = @{ name = 'rename_session'; arguments = @{ session = $sid; name = 'api-server' } } }
    [void](Recv)
    Start-Sleep 2
    Send @{ jsonrpc = '2.0'; id = 2; method = 'tools/call'; params = @{ name = 'screenshot'; arguments = @{} } }
    $r = Recv
    $img = $r.result.content | Where-Object type -eq 'image' | Select-Object -First 1
    if (-not $img) { throw ('kein Bild: ' + ($r | ConvertTo-Json -Depth 5 -Compress)) }
    [IO.File]::WriteAllBytes($png, [Convert]::FromBase64String($img.data))
  } finally {
    try { $b.StandardInput.Close(); [void]$b.WaitForExit(2000) } catch {}
    if (-not $b.HasExited) { $b.Kill() }
  }
}

try {
  # ---- Bauen
  $shotsExe = "$root\target\shots\release\paneldeck.exe"
  if (-not $NoBuild -or -not (Test-Path $shotsExe)) {
    Push-Location $root
    $ErrorActionPreference = 'Continue'   # cargo schreibt Fortschritt nach stderr (Windows PowerShell 5.1 machte daraus einen Fehler)
    & cargo build --release --features readme-shots --target-dir "$root\target\shots" 2>&1 | ForEach-Object { "$_" }
    $code = $LASTEXITCODE
    $ErrorActionPreference = 'Stop'
    if ($code -ne 0) { throw 'cargo build fehlgeschlagen' }
    Pop-Location
  }
  New-Item -ItemType Directory -Force $work | Out-Null
  if ($RawDir) { New-Item -ItemType Directory -Force $RawDir | Out-Null }
  Get-Lock
  # ---- Demo-Datenbanken (zuerst, solange USERPROFILE noch das echte ist: cargo braucht es)
  foreach ($lg in $Lang) {
    $app = Join-Path $work "app-$lg"; New-Item -ItemType Directory -Force $app | Out-Null
    Copy-Item $shotsExe "$app\paneldeck.exe"
    $env:PANELDECK_DEMO_DIR = "$app\paneldeck-data"; $env:PANELDECK_DEMO_LANG = $lg
    Push-Location $root
    $ErrorActionPreference = 'Continue'
    & cargo test --release demo_shots -- --ignored --nocapture 2>&1 | ForEach-Object { "$_" } | Select-String 'Demo-Datenbank|FAILED|panicked'
    $ErrorActionPreference = 'Stop'
    Pop-Location
    Remove-Item Env:\PANELDECK_DEMO_DIR, Env:\PANELDECK_DEMO_LANG
    if (-not (Test-Path "$app\paneldeck-data\paneldeck.db")) { throw "Demo-Datenbank ($lg) wurde nicht erzeugt" }
    # MCP-Stufe "Steuern" (Screenshot-Werkzeug und Sitzung umbenennen) in die Einstellungen der Kopie schreiben
    $sf = "$app\paneldeck-data\settings.json"
    $j = [IO.File]::ReadAllText($sf).TrimStart([char]0xFEFF) | ConvertFrom-Json
    if ($j.PSObject.Properties.Name -contains 'mcp_level') { $j.mcp_level = 'Control' } else { $j | Add-Member mcp_level 'Control' }
    [IO.File]::WriteAllText($sf, ($j | ConvertTo-Json -Depth 10), (New-Object Text.UTF8Encoding $false))
  }
  # ---- Aufnahmen
  $ticket = 'SHOP-23'  # "Payment step with saved cards" (der Test demo_shots prüft die Nummer)
  foreach ($lg in $Lang) {
    $app = Join-Path $work "app-$lg"
    # Schein-Profil: USERPROFILE/APPDATA zeigen in den Arbeitsordner (Hooks, ~/.claude, Shell-Ordner des Nutzers bleiben unberührt)
    $prof = Join-Path $work "profile-$lg"
    New-Item -ItemType Directory -Force "$prof\AppData\Roaming", "$prof\AppData\Local" | Out-Null
    foreach ($name in $Views.Keys) {
      $spec = $Views[$name].Replace('{TICKET}', $ticket)
      Remove-Item "$app\paneldeck-data\ipc.json" -ErrorAction SilentlyContinue
      $saved = @{ USERPROFILE = $env:USERPROFILE; APPDATA = $env:APPDATA; LOCALAPPDATA = $env:LOCALAPPDATA }
      $env:USERPROFILE = $prof; $env:APPDATA = "$prof\AppData\Roaming"; $env:LOCALAPPDATA = "$prof\AppData\Local"; $env:PANELDECK_SHOT = $spec
      $script:inst = Start-Process "$app\paneldeck.exe" -PassThru
      Remove-Item Env:\PANELDECK_SHOT
      $env:USERPROFILE = $saved.USERPROFILE; $env:APPDATA = $saved.APPDATA; $env:LOCALAPPDATA = $saved.LOCALAPPDATA
      $t0 = Get-Date
      while (-not (Test-Path "$app\paneldeck-data\ipc.json") -and ((Get-Date) - $t0).TotalSeconds -lt 30) { Start-Sleep -Milliseconds 300 }
      Start-Sleep 3
      $script:inst.Refresh()
      $size = if ($Sizes.ContainsKey($name)) { $Sizes[$name] } else { @($Width, $Height) }
      [void][Shots.W]::MoveWindow($script:inst.MainWindowHandle, 40, 20, $size[0], $size[1], $true)
      Start-Sleep 3
      $raw = Join-Path $work "$lg-$name.png"
      Mcp-Screenshot "$app\paneldeck.exe" $raw
      Stop-Inst
      if ($RawDir) { Copy-Item $raw (Join-Path $RawDir "$lg-$name.png") -Force }
      $dst = if ($lg -eq 'de') { Join-Path $OutDir "de\$name.png" } else { Join-Path $OutDir "$name.png" }
      New-Item -ItemType Directory -Force (Split-Path $dst) | Out-Null
      & python -I "$root\tools\readme_shot_post.py" $raw $dst
      if ($LASTEXITCODE -ne 0) { throw "Nachbearbeitung fehlgeschlagen: $name" }
    }
  }
} finally {
  Stop-Inst
  if ($script:haveLock) { Remove-Item $lock -Recurse -Force -ErrorAction SilentlyContinue }
  if (-not $KeepWork) { Remove-Item $work -Recurse -Force -ErrorAction SilentlyContinue }
  Get-ChildItem Env:\PANELDECK_SHOT -ErrorAction SilentlyContinue | ForEach-Object { Remove-Item "Env:\$($_.Name)" }
}
