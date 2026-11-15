<#
.SYNOPSIS
  Erzeugt die Bilder der Abfrage-Hilfe (ein englisches Bild je Beispiel, assets\query-help\query-help-<id>.png) – ohne Maus,
  Tastatur oder Bildschirmkopie.

.DESCRIPTION
  1. baut (falls nötig) die Anwendung mit dem Feature `readme-shots` nach target\shots (Startansicht per PANELDECK_SHOT),
  2. erzeugt je Beispiel einen frischen Datenordner aus `Example::demo` (`cargo test --release query_help_shot_data -- --ignored`,
     siehe src\planning\search\tests_seed.rs; der Test prüft vorher, dass die echte Suche genau die erwarteten Treffer liefert),
  3. startet je Beispiel EINE Instanz aus einer Kopie der .exe (eigener Datenordner, Schein-USERPROFILE) mit
     PANELDECK_SHOT=search-example:<id> (Reiter „Suche“, Abfrage des Beispiels ausgeführt),
  4. nimmt das Fenster mit dem MCP-Werkzeug `screenshot` auf (die Anwendung rendert sich selbst),
  5. beendet die Instanz und schneidet das Bild auf den Suchbereich zu (tools\query_help_shot_post.py: Zuschnitt, Palette).
  Die UI-Sperre (%TEMP%\paneldeck-ui-lock = /tmp/paneldeck-ui-lock in Git Bash) wird solange gehalten, wie Instanzen laufen.
  Danach die Bilder in src\app\planning_ui\help_shots.rs (SHOTS) eintragen.

.EXAMPLE
  powershell -ExecutionPolicy Bypass -File tools\query-help-shots.ps1
  powershell -ExecutionPolicy Bypass -File tools\query-help-shots.ps1 -Only basics-field-value,terminal-linked -NoBuild -RawDir C:\tmp\raw
#>
param(
  # nur diese Beispiel-IDs (Standard: alle)
  [string[]]$Only = @(),
  # Fenstergröße: 960 px breit zeigt die Spalte „Version · Milestone · Terminal“; der Zuschnitt (query_help_shot_post.py) gilt dafür
  [int]$Width = 960,
  [int]$Height = 900,
  [string]$OutDir = "$PSScriptRoot\..\assets\query-help",
  [string]$RawDir = '',
  [switch]$NoBuild,
  [switch]$KeepWork
)
$ErrorActionPreference = 'Stop'
$root = (Resolve-Path "$PSScriptRoot\..").Path
$OutDir = [IO.Path]::GetFullPath($OutDir)
$lock = Join-Path $env:TEMP 'paneldeck-ui-lock'
$work = Join-Path $env:TEMP ('pd-qhelp-shots-' + [guid]::NewGuid().ToString('N').Substring(0, 8))
$script:inst = $null
$script:haveLock = $false
Add-Type -MemberDefinition '[DllImport("user32.dll")] public static extern bool MoveWindow(IntPtr h, int x, int y, int w, int hh, bool r);' -Name W -Namespace QhShots

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
    Send @{ jsonrpc = '2.0'; id = 1; method = 'initialize'; params = @{ protocolVersion = '2025-06-18'; capabilities = @{}; clientInfo = @{ name = 'query-help-shots'; version = '1' } } }
    [void](Recv)
    Send @{ jsonrpc = '2.0'; method = 'notifications/initialized' }
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
    $ErrorActionPreference = 'Continue'   # cargo schreibt Fortschritt nach stderr
    & cargo build --release --features readme-shots --target-dir "$root\target\shots" 2>&1 | ForEach-Object { "$_" }
    $code = $LASTEXITCODE
    $ErrorActionPreference = 'Stop'
    if ($code -ne 0) { throw 'cargo build fehlgeschlagen' }
    Pop-Location
  }
  New-Item -ItemType Directory -Force $work, $OutDir | Out-Null
  if ($RawDir) { New-Item -ItemType Directory -Force $RawDir | Out-Null }
  # ---- Datenordner je Beispiel (solange USERPROFILE noch das echte ist: cargo braucht es)
  $data = Join-Path $work 'data'
  $env:PANELDECK_QHELP_DIR = $data
  if ($Only.Count -gt 0) { $env:PANELDECK_QHELP_ONLY = ($Only -join ',') }
  Push-Location $root
  $ErrorActionPreference = 'Continue'
  & cargo test --release query_help_shot_data -- --ignored --nocapture 2>&1 | ForEach-Object { "$_" } | Select-String 'Beispiel |FAILED|panicked'
  $code = $LASTEXITCODE
  $ErrorActionPreference = 'Stop'
  Pop-Location
  Remove-Item Env:\PANELDECK_QHELP_DIR, Env:\PANELDECK_QHELP_ONLY -ErrorAction SilentlyContinue
  if ($code -ne 0) { throw 'Beispieldaten wurden nicht erzeugt' }
  $ids = Get-ChildItem $data -Directory | ForEach-Object Name
  # ---- Aufnahmen
  Get-Lock
  $prof = Join-Path $work 'profile'
  New-Item -ItemType Directory -Force "$prof\AppData\Roaming", "$prof\AppData\Local" | Out-Null
  foreach ($id in $ids) {
    $app = Join-Path $data $id
    Copy-Item $shotsExe "$app\paneldeck.exe"
    $saved = @{ USERPROFILE = $env:USERPROFILE; APPDATA = $env:APPDATA; LOCALAPPDATA = $env:LOCALAPPDATA }
    $env:USERPROFILE = $prof; $env:APPDATA = "$prof\AppData\Roaming"; $env:LOCALAPPDATA = "$prof\AppData\Local"; $env:PANELDECK_SHOT = "search-example:$id"
    $script:inst = Start-Process "$app\paneldeck.exe" -PassThru
    Remove-Item Env:\PANELDECK_SHOT
    $env:USERPROFILE = $saved.USERPROFILE; $env:APPDATA = $saved.APPDATA; $env:LOCALAPPDATA = $saved.LOCALAPPDATA
    $t0 = Get-Date
    while (-not (Test-Path "$app\paneldeck-data\ipc.json") -and ((Get-Date) - $t0).TotalSeconds -lt 30) { Start-Sleep -Milliseconds 300 }
    Start-Sleep 3
    $script:inst.Refresh()
    [void][QhShots.W]::MoveWindow($script:inst.MainWindowHandle, 40, 20, $Width, $Height, $true)
    Start-Sleep 3
    $raw = Join-Path $work "$id.png"
    Mcp-Screenshot "$app\paneldeck.exe" $raw
    Stop-Inst
    if ($RawDir) { Copy-Item $raw (Join-Path $RawDir "$id.png") -Force }
    & python -I "$root\tools\query_help_shot_post.py" $raw (Join-Path $OutDir "query-help-$id.png")
    if ($LASTEXITCODE -ne 0) { throw "Nachbearbeitung fehlgeschlagen: $id" }
  }
} finally {
  Stop-Inst
  if ($script:haveLock) { Remove-Item $lock -Recurse -Force -ErrorAction SilentlyContinue }
  if (-not $KeepWork) { Remove-Item $work -Recurse -Force -ErrorAction SilentlyContinue }
  Get-ChildItem Env:\PANELDECK_SHOT -ErrorAction SilentlyContinue | ForEach-Object { Remove-Item "Env:\$($_.Name)" }
}
