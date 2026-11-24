<#
.SYNOPSIS
  Erzeugt alle Screenshots für README und Anleitungen (docs/images/*.png und docs/images/de/*.png) aus der aktuellen
  Software – ohne Maus, Tastatur oder Bildschirmkopie. Läuft vor jedem Release (tools/release-prep.sh).

.DESCRIPTION
  1. baut (falls nötig) die Anwendung mit dem Feature `readme-shots` (Startansicht per Umgebungsvariable PANELDECK_SHOT,
     siehe src\app\shot_start.rs und src\app\planning_ui\shot_view.rs; nie im Release),
  2. erzeugt je Sprache eine Demo-Datenbank (`cargo test --release demo_shots -- --ignored`, Projekt "Acme Shop",
     siehe src\planning\demo_shots); weitere Beispieldaten (Ordner acme-shop, Prozesse, Transkripte, Theme-Datei)
     legt die Startansicht selbst im Arbeitsordner an,
  3. startet je Ansicht EINE Instanz aus einer Kopie der .exe (eigener Datenordner, Schein-USERPROFILE/APPDATA/TEMP)
     in der Startansicht,
  4. nimmt das Bild mit dem MCP-Werkzeug `screenshot` auf (die Anwendung rendert sich selbst – das Fenster muss nur
     sichtbar sein),
  5. beendet die Instanz, rahmt das Bild (tools\readme_shot_post.py) und legt es unter docs\images\ (en) bzw.
     docs\images\de\ (de) ab.
  Die UI-Sperre (%TEMP%\paneldeck-ui-lock = /tmp/paneldeck-ui-lock in Git Bash) wird solange gehalten, wie Instanzen laufen.
  Es werden keine echten Daten, keine echte Registry und kein echtes ~/.claude benutzt.
  Danach prüft tools\check_docs_images.py, dass kein Platzhalter `<!-- screenshot: … -->` übrig ist und alle Bilder da sind.

.PARAMETER Only
  Nur diese Bilder (Namen ohne .png, Platzhalter * erlaubt), z. B. -Only 'editor*','app'.

.PARAMETER Exe
  Fertig gebaute .exe mit dem Feature readme-shots (dann wird nicht gebaut).

.PARAMETER WorkRoot
  Ort des Arbeitsordners (Standard: $env:PANELDECK_TEST_ROOT, sonst %TEMP%) – wegen Defender E:\paneldeck-test nehmen.

.EXAMPLE
  powershell -ExecutionPolicy Bypass -File tools\docs-shots.ps1 -WorkRoot E:\paneldeck-test
  powershell -ExecutionPolicy Bypass -File tools\docs-shots.ps1 -Lang de -Only 'theme-*' -NoBuild
  powershell -ExecutionPolicy Bypass -File tools\docs-shots.ps1 -List
#>
param(
  [string[]]$Lang = @('en', 'de'),
  [string[]]$Only = @(),
  [string]$Exe = '',
  [string]$TargetDir = '',
  [string]$WorkRoot = '',
  [int]$Width = 1240,
  [int]$Height = 800,
  [string]$OutDir = "$PSScriptRoot\..\docs\images",
  [string]$RawDir = '',
  [switch]$NoBuild,
  [switch]$KeepWork,
  [switch]$NoCheck,
  [switch]$List
)
$ErrorActionPreference = 'Stop'

# ---- Ansichten: Bild (ohne .png) -> PANELDECK_SHOT, Fenstergröße, zusätzliche Wartezeit (s), Sitzung per MCP benennen
# Bilder der README und der Anleitungen; neue Ansichten in src\app\shot_start.rs (bzw. planning_ui\shot_view.rs) ergänzen.
$AllViews = @(
  # README / Terminals / Claude Code / Agenten
  @{ Name = 'app';                 Spec = 'app';                 W = 1400; H = 860;  Wait = 4; Rename = $false }
  @{ Name = 'kosten';              Spec = 'kosten';              W = 1240; H = 860;  Wait = 4; Rename = $false }
  @{ Name = 'agenten';             Spec = 'agenten';             W = 1100; H = 1060 }
  # Todo-Liste (Demo-Datenbank "Acme Shop")
  @{ Name = 'todo-tickets';        Spec = 'tickets:{TICKET}';    W = 1300; H = 1260 }
  @{ Name = 'todo-dashboard';      Spec = 'dashboard' }
  @{ Name = 'maint-overview';      Spec = 'overview';            Wait = 2 }
  @{ Name = 'maint-page';          Spec = 'maint-page';          W = 1300; H = 860; Wait = 3 }
  @{ Name = 'maint-fix';           Spec = 'maint-fix';           W = 1300; H = 900; Wait = 4 }
  @{ Name = 'maint-duplicate';     Spec = 'maint-duplicate';     W = 1300; H = 900; Wait = 4 }
  @{ Name = 'maint-database';      Spec = 'maint-database';      W = 1300; H = 900; Wait = 5 }
  # Daten & Sicherung
  @{ Name = 'cleanup';             Spec = 'cleanup';             H = 900; Wait = 2 }
  @{ Name = 'cleanup-confirm';     Spec = 'cleanup-confirm';     H = 900; Wait = 2 }
  @{ Name = 'cleanup-result';      Spec = 'cleanup-result' }
  # Editor
  @{ Name = 'editor';              Spec = 'editor';              W = 1400; H = 860; Wait = 2 }
  @{ Name = 'editor-find';         Spec = 'editor-find';         W = 1400; H = 860; Wait = 3 }
  @{ Name = 'editor-disk-changed'; Spec = 'editor-disk-changed'; W = 1400; H = 860; Wait = 4 }
  @{ Name = 'editor-search';       Spec = 'editor-search';       W = 1400; H = 860; Wait = 3 }
  @{ Name = 'editor-syntax';       Spec = 'editor-syntax';       W = 1400; H = 860; Wait = 3 }
  @{ Name = 'editor-format';       Spec = 'editor-format';       W = 1400; H = 860; Wait = 3 }
  @{ Name = 'editor-settings';     Spec = 'editor-settings';     W = 1100; H = 900 }
  # Reiter Zeit (#164)
  @{ Name = 'time-alarms';         Spec = 'time-alarms';         W = 1300; H = 820; Wait = 2 }
  @{ Name = 'time-timers';         Spec = 'time-timers';         W = 1300; H = 820; Wait = 2 }
  @{ Name = 'time-clocks';         Spec = 'time-clocks';         W = 1300; H = 820; Wait = 2 }
  # Chats
  @{ Name = 'chat-images';         Spec = 'chat-images';         W = 1300; H = 860; Wait = 3 }
  # Prozesse
  @{ Name = 'processes';           Spec = 'processes';           H = 1000; Wait = 6 }
  @{ Name = 'process-new';         Spec = 'process-new';         H = 900; Wait = 2 }
  @{ Name = 'process-output';      Spec = 'process-output';      H = 1000; Wait = 6 }
  # Chats
  @{ Name = 'chat-compacted';      Spec = 'chat-compacted';      W = 1240; H = 860; Wait = 2 }
  # Themes
  @{ Name = 'theme-appearance';    Spec = 'theme-appearance';    W = 1100; H = 860 }
  @{ Name = 'theme-generate';      Spec = 'theme-generate';      W = 1100; H = 860 }
  @{ Name = 'theme-editor';        Spec = 'theme-editor';        W = 1400; H = 900 }
  @{ Name = 'theme-color';         Spec = 'theme-color';         W = 1400; H = 900 }
  @{ Name = 'theme-checks';        Spec = 'theme-checks';        W = 1400; H = 900; Wait = 2 }
  # Kalender (#164; bis zum Zeit-Reiter ueber dem Fenster gezeichnet)
  @{ Name = 'time-calendar-month';  Spec = 'calendar-month';     W = 1400; H = 900;  Wait = 3 }
  @{ Name = 'time-calendar-dialog'; Spec = 'calendar-dialog';    W = 1300; H = 1040; Wait = 3 }
  # Chats (#355, #356)
  @{ Name = 'chat-background-command'; Spec = 'chat-background-command'; W = 1400; H = 860; Wait = 3 }
  @{ Name = 'chat-ask-form';       Spec = 'chat-ask-form';       W = 1100; H = 860; Wait = 2 }
  # Chats (#358)
  @{ Name = 'chat-plan';           Spec = 'chat-plan';           W = 1300; H = 900;  Wait = 2 }
  # Tutorial
  @{ Name = 'tutorial-card';       Spec = 'tutorial-card';       Wait = 1 }
  @{ Name = 'tutorial-tour';       Spec = 'tutorial-tour:basics/basics.sidebar'; Wait = 2 }
)

if ($List) { $AllViews | ForEach-Object { '{0,-22} PANELDECK_SHOT={1}' -f $_.Name, $_.Spec }; return }
$Views = @($AllViews | Where-Object { $v = $_; ($Only.Count -eq 0) -or ($Only | Where-Object { $v.Name -like $_ }) })
if ($Views.Count -eq 0) { throw "keine Ansicht passt zu -Only $($Only -join ',') (Liste: -List)" }

$root = (Resolve-Path "$PSScriptRoot\..").Path
$OutDir = [IO.Path]::GetFullPath($OutDir)
if (-not $WorkRoot) { $WorkRoot = if ($env:PANELDECK_TEST_ROOT) { $env:PANELDECK_TEST_ROOT } else { $env:TEMP } }
if (-not $TargetDir) { $TargetDir = "$root\target\shots" }
$lock = Join-Path $env:TEMP 'paneldeck-ui-lock'
$work = Join-Path $WorkRoot ('pd-docs-shots-' + [guid]::NewGuid().ToString('N').Substring(0, 8))
$script:inst = $null
$script:haveLock = $false
$failed = @()
Add-Type -MemberDefinition @'
[DllImport("user32.dll")] public static extern bool MoveWindow(IntPtr h, int x, int y, int w, int hh, bool r);
[DllImport("user32.dll")] public static extern bool IsWindowVisible(IntPtr h);
[DllImport("user32.dll")] public static extern bool IsIconic(IntPtr h);
[DllImport("user32.dll")] public static extern bool ShowWindow(IntPtr h, int cmd);
'@ -Name W -Namespace DocShots

# Hauptfenster der Instanz (0, solange es noch keins gibt).
function Get-InstHwnd { if (-not $script:inst) { return [IntPtr]::Zero }; $script:inst.Refresh(); $script:inst.MainWindowHandle }
# Warten, bis das Fenster sichtbar und nicht minimiert ist (höchstens $sec Sekunden); $true, wenn es so weit ist.
function Wait-Visible([int]$sec = 30) {
  $t0 = Get-Date
  while (((Get-Date) - $t0).TotalSeconds -lt $sec) {
    $h = Get-InstHwnd
    if ($h -ne [IntPtr]::Zero -and [DocShots.W]::IsWindowVisible($h) -and -not [DocShots.W]::IsIconic($h)) { return $true }
    Start-Sleep -Milliseconds 300
  }
  return $false
}
# Fenster zeigen (falls verborgen/minimiert), ohne Maus oder Tastatur: SW_SHOWNOACTIVATE bzw. SW_RESTORE.
function Show-Inst {
  $h = Get-InstHwnd
  if ($h -eq [IntPtr]::Zero) { return }
  if ([DocShots.W]::IsIconic($h)) { [void][DocShots.W]::ShowWindow($h, 9) } elseif (-not [DocShots.W]::IsWindowVisible($h)) { [void][DocShots.W]::ShowWindow($h, 4) }
}

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

function Mcp-Screenshot($exe, $png, [bool]$rename) {
  $psi = New-Object System.Diagnostics.ProcessStartInfo
  $psi.FileName = $exe; $psi.Arguments = 'mcp'
  $psi.RedirectStandardInput = $true; $psi.RedirectStandardOutput = $true; $psi.RedirectStandardError = $true
  $psi.UseShellExecute = $false; $psi.CreateNoWindow = $true; $psi.StandardOutputEncoding = [Text.Encoding]::UTF8
  $b = [System.Diagnostics.Process]::Start($psi)
  function Send($o) { $l = ($o | ConvertTo-Json -Depth 10 -Compress); $by = (New-Object Text.UTF8Encoding $false).GetBytes($l + "`n"); $b.StandardInput.BaseStream.Write($by, 0, $by.Length); $b.StandardInput.BaseStream.Flush() }
  function Recv($ms = 30000) { $t = $b.StandardOutput.ReadLineAsync(); if ($t.Wait($ms)) { $t.Result | ConvertFrom-Json } else { $null } }
  try {
    Send @{ jsonrpc = '2.0'; id = 1; method = 'initialize'; params = @{ protocolVersion = '2025-06-18'; capabilities = @{}; clientInfo = @{ name = 'docs-shots'; version = '1' } } }
    [void](Recv)
    Send @{ jsonrpc = '2.0'; method = 'notifications/initialized' }
    if ($rename) {
      # die Sitzung für die Titelleiste benennen (sonst steht dort der Pfad der Shell)
      Send @{ jsonrpc = '2.0'; id = 3; method = 'tools/call'; params = @{ name = 'list_sessions'; arguments = @{} } }
      $ls = Recv
      $sid = @($ls.result.structuredContent.sessions)[0].id
      if ($sid) {
        Send @{ jsonrpc = '2.0'; id = 4; method = 'tools/call'; params = @{ name = 'rename_session'; arguments = @{ session = $sid; name = 'api-server' } } }
        [void](Recv)
      }
      Start-Sleep 2
    }
    # Fenster noch nicht sichtbar (window_hidden/not_visible) oder Zeitüberschreitung: ein paar Mal neu versuchen
    $img = $null; $r = $null
    for ($try = 1; $try -le 6 -and -not $img; $try++) {
      if ($try -gt 1) { Show-Inst; Start-Sleep 2 }
      Send @{ jsonrpc = '2.0'; id = (10 + $try); method = 'tools/call'; params = @{ name = 'screenshot'; arguments = @{} } }
      $r = Recv
      $img = $r.result.content | Where-Object type -eq 'image' | Select-Object -First 1
      if (-not $img) { Write-Host "  Screenshot-Versuch $try ohne Bild: $(($r | ConvertTo-Json -Depth 5 -Compress) -replace '(.{300}).*', '$1')" }
    }
    if (-not $img) { throw ('kein Bild: ' + ($r | ConvertTo-Json -Depth 5 -Compress)) }
    [IO.File]::WriteAllBytes($png, [Convert]::FromBase64String($img.data))
  } finally {
    try { $b.StandardInput.Close(); [void]$b.WaitForExit(2000) } catch {}
    if (-not $b.HasExited) { $b.Kill() }
  }
}

try {
  # ---- Bauen
  if ($Exe) {
    $shotsExe = (Resolve-Path $Exe).Path
  } else {
    $shotsExe = "$TargetDir\release\paneldeck.exe"
    if (-not $NoBuild -or -not (Test-Path $shotsExe)) {
      Push-Location $root
      $ErrorActionPreference = 'Continue'   # cargo schreibt Fortschritt nach stderr (Windows PowerShell 5.1 machte daraus einen Fehler)
      & cargo build --release --features readme-shots --target-dir "$TargetDir" 2>&1 | ForEach-Object { "$_" }
      $code = $LASTEXITCODE
      $ErrorActionPreference = 'Stop'
      Pop-Location
      if ($code -ne 0) { throw 'cargo build fehlgeschlagen' }
    }
  }
  New-Item -ItemType Directory -Force $work | Out-Null
  if ($RawDir) { New-Item -ItemType Directory -Force $RawDir | Out-Null }
  Get-Lock
  # ---- Demo-Datenbanken (zuerst, solange USERPROFILE noch das echte ist: cargo braucht es)
  foreach ($lg in $Lang) {
    $seed = Join-Path $work "seed-$lg"; New-Item -ItemType Directory -Force $seed | Out-Null
    $env:PANELDECK_DEMO_DIR = "$seed\paneldeck-data"; $env:PANELDECK_DEMO_LANG = $lg
    Push-Location $root
    $ErrorActionPreference = 'Continue'
    & cargo test --release demo_shots -- --ignored --nocapture 2>&1 | ForEach-Object { "$_" } | Select-String 'Demo-Datenbank|FAILED|panicked'
    $ErrorActionPreference = 'Stop'
    Pop-Location
    Remove-Item Env:\PANELDECK_DEMO_DIR, Env:\PANELDECK_DEMO_LANG
    if (-not (Test-Path "$seed\paneldeck-data\paneldeck.db")) { throw "Demo-Datenbank ($lg) wurde nicht erzeugt" }
    # MCP-Stufe "Steuern" (Screenshot-Werkzeug und Sitzung umbenennen) in die Einstellungen schreiben
    $sf = "$seed\paneldeck-data\settings.json"
    $j = [IO.File]::ReadAllText($sf).TrimStart([char]0xFEFF) | ConvertFrom-Json
    if ($j.PSObject.Properties.Name -contains 'mcp_level') { $j.mcp_level = 'Control' } else { $j | Add-Member mcp_level 'Control' }
    [IO.File]::WriteAllText($sf, ($j | ConvertTo-Json -Depth 10), (New-Object Text.UTF8Encoding $false))
  }
  # ---- Aufnahmen: je Ansicht ein frischer Ordner (Kopie der Demo-Daten), damit keine Ansicht die nächste beeinflusst
  $ticket = 'SHOP-23'  # "Payment step with saved cards" (der Test demo_shots prüft die Nummer)
  foreach ($lg in $Lang) {
    foreach ($v in $Views) {
      $name = $v.Name
      $spec = $v.Spec.Replace('{TICKET}', $ticket)
      $w = if ($v.W) { $v.W } else { $Width }; $h = if ($v.H) { $v.H } else { $Height }
      $wait = if ($v.Wait) { $v.Wait } else { 0 }
      $rename = if ($v.ContainsKey('Rename')) { [bool]$v.Rename } else { $true }
      Write-Host "[$lg] $name (PANELDECK_SHOT=$spec)"
      $app = Join-Path $work "$lg-$name"
      New-Item -ItemType Directory -Force $app | Out-Null
      Copy-Item $shotsExe "$app\paneldeck.exe"
      Copy-Item "$work\seed-$lg\paneldeck-data" "$app\paneldeck-data" -Recurse
      # Schein-Profil: USERPROFILE/APPDATA/TEMP zeigen in den Arbeitsordner (Hooks, ~/.claude, Shell-Ordner des Nutzers bleiben unberührt)
      $prof = Join-Path $app 'profile'
      New-Item -ItemType Directory -Force "$prof\AppData\Roaming", "$prof\AppData\Local", "$prof\Temp" | Out-Null
      $saved = @{ USERPROFILE = $env:USERPROFILE; APPDATA = $env:APPDATA; LOCALAPPDATA = $env:LOCALAPPDATA; TEMP = $env:TEMP; TMP = $env:TMP }
      try {
        $env:USERPROFILE = $prof; $env:APPDATA = "$prof\AppData\Roaming"; $env:LOCALAPPDATA = "$prof\AppData\Local"
        $env:TEMP = "$prof\Temp"; $env:TMP = "$prof\Temp"; $env:PANELDECK_SHOT = $spec
        $script:inst = Start-Process "$app\paneldeck.exe" -PassThru
      } finally {
        Remove-Item Env:\PANELDECK_SHOT -ErrorAction SilentlyContinue
        foreach ($k in $saved.Keys) { Set-Item "Env:\$k" $saved[$k] }
      }
      try {
        $t0 = Get-Date
        while (-not (Test-Path "$app\paneldeck-data\ipc.json") -and ((Get-Date) - $t0).TotalSeconds -lt 30) { Start-Sleep -Milliseconds 300 }
        if (-not (Test-Path "$app\paneldeck-data\ipc.json")) { throw 'Instanz meldet sich nicht (ipc.json fehlt)' }
        if (-not (Wait-Visible 30)) { Show-Inst; if (-not (Wait-Visible 10)) { throw 'Fenster wird nicht sichtbar' } }
        Start-Sleep 2
        [void][DocShots.W]::MoveWindow((Get-InstHwnd), 40, 20, $w, $h, $true)
        Start-Sleep (3 + $wait)
        [void](Wait-Visible 10)
        $raw = Join-Path $work "$lg-$name.png"
        Mcp-Screenshot "$app\paneldeck.exe" $raw $rename
      } catch {
        Write-Warning "[$lg] $name fehlgeschlagen: $_"
        $failed += "$lg/$name"
        continue
      } finally {
        Stop-Inst
      }
      if ($RawDir) { Copy-Item $raw (Join-Path $RawDir "$lg-$name.png") -Force }
      $dst = if ($lg -eq 'de') { Join-Path $OutDir "de\$name.png" } else { Join-Path $OutDir "$name.png" }
      New-Item -ItemType Directory -Force (Split-Path $dst) | Out-Null
      & python -I "$root\tools\readme_shot_post.py" $raw $dst
      if ($LASTEXITCODE -ne 0) { $failed += "$lg/$name (Nachbearbeitung)" }
    }
  }
} finally {
  Stop-Inst
  if ($script:haveLock) { Remove-Item $lock -Recurse -Force -ErrorAction SilentlyContinue }
  if (-not $KeepWork) { Remove-Item $work -Recurse -Force -ErrorAction SilentlyContinue }
  Get-ChildItem Env:\PANELDECK_SHOT -ErrorAction SilentlyContinue | ForEach-Object { Remove-Item "Env:\$($_.Name)" }
}
if ($failed.Count -gt 0) { Write-Host "Fehlgeschlagen: $($failed -join ', ')"; exit 1 }
if (-not $NoCheck) {
  $env:PYTHONUTF8 = '1'
  & python -I "$root\tools\check_docs_images.py"
  if ($LASTEXITCODE -ne 0) { exit 1 }
}
Write-Host "Fertig: $($Views.Count) Ansicht(en) x $($Lang.Count) Sprache(n) nach $OutDir"
