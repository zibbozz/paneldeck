# Update-Test des Installers: alte Version installieren, Nutzerdaten/Autostart/Kontextmenü anlegen, neuen Installer darüberspielen.
#   .\tools\update-test.ps1 [-Old <Setup.exe>] [-New <Setup.exe>]
# Ohne -Old: der neueste andere Installer in dist\; gibt es keinen, wird ein "Vorgänger" aus der aktuellen .exe gebaut (mit angehängten Füllbytes).
# Ohne -New wird aus target\release\paneldeck.exe ein Probe-Installer gebaut (nach %TEMP%, nicht nach dist\).
# Alles läuft in einem Temp-Ordner mit Schein-Profil (USERPROFILE); die Registry-Einträge des Installers werden am Ende entfernt.
# Bricht ab, wenn auf dem Rechner schon eine echte Installation (Registry-Einträge von Paneldeck) existiert.
param([string]$Old = '', [string]$New = '')
$ErrorActionPreference = 'Stop'
$root = Split-Path -Parent $PSScriptRoot
$makensis = @('C:\Program Files (x86)\NSIS\makensis.exe', 'C:\Program Files\NSIS\makensis.exe') | Where-Object { Test-Path $_ } | Select-Object -First 1

$pass = 0; $fail = 0
function Check($name, $ok, $detail = '') {
  if ($ok) { $script:pass++; Write-Host "  ok    $name" } else { $script:fail++; Write-Host "  FEHL  $name  $detail" -ForegroundColor Red }
}
$regKeys = @(
  'HKCU:\Software\Paneldeck',
  'HKCU:\Software\Microsoft\Windows\CurrentVersion\Uninstall\Paneldeck',
  'HKCU:\Software\Classes\Directory\shell\Paneldeck', 'HKCU:\Software\Classes\Directory\shell\PaneldeckClaude',
  'HKCU:\Software\Classes\Directory\Background\shell\Paneldeck', 'HKCU:\Software\Classes\Directory\Background\shell\PaneldeckClaude')
$runKey = 'HKCU:\Software\Microsoft\Windows\CurrentVersion\Run'
$existing = @($regKeys | Where-Object { Test-Path $_ }) + @(if (Get-ItemProperty $runKey -Name Paneldeck -ErrorAction SilentlyContinue) { "$runKey\Paneldeck" })
if ($existing) { Write-Host "Abbruch: Paneldeck ist auf diesem Rechner installiert (Registry): $($existing -join ', ')" -ForegroundColor Yellow; exit 2 }

$tmp = Join-Path $env:TEMP ('hub-update-' + [guid]::NewGuid().ToString('N').Substring(0, 8))
$prof = Join-Path $tmp 'profile'; $inst = Join-Path $tmp 'inst'; $data = Join-Path $inst 'paneldeck-data'
New-Item -ItemType Directory -Force $prof, (Join-Path $prof 'Desktop'), (Join-Path $prof 'AppData\Roaming\Microsoft\Windows\Start Menu\Programs') | Out-Null
$realDesktopLnk = Join-Path ([Environment]::GetFolderPath('Desktop')) 'Paneldeck.lnk'
$realStart = Join-Path ([Environment]::GetFolderPath('Programs')) 'Paneldeck'
$beforeDesktop = Test-Path $realDesktopLnk; $beforeStart = Test-Path $realStart
$oldProfile = $env:USERPROFILE

function Install($setup, [string]$label) {
  $env:USERPROFILE = $prof
  try { $p = Start-Process $setup -ArgumentList '/S', "/D=$inst" -PassThru; if (-not $p.WaitForExit(120000)) { $p.Kill(); return -999 }; return $p.ExitCode }
  finally { $env:USERPROFILE = $oldProfile }
}
function Reg($path, $name = $null) { if ($name) { (Get-ItemProperty $path -Name $name -ErrorAction SilentlyContinue).$name } else { (Get-Item $path -ErrorAction SilentlyContinue).GetValue('') } }
# Nur die Nutzerdaten (nicht instance.lock/ipc.json u. ä., die ein laufender Hub hält).
function Hashes($dir) { $h = @{}; Get-ChildItem $dir -Recurse -File -ErrorAction SilentlyContinue | Where-Object { $_.Name -in 'settings.json','state.json','prompts.json' -or $_.DirectoryName -like '*\themes' } | ForEach-Object { $h[$_.FullName.Substring($dir.Length)] = (Get-FileHash $_.FullName).Hash }; $h }
function SameHashes($a, $b) { ($a.Keys.Count -eq $b.Keys.Count) -and -not ($a.Keys | Where-Object { $a[$_] -ne $b[$_] }) }

try {
  if (-not $New) {
    if (-not $makensis) { throw 'makensis nicht gefunden' }
    $New = Join-Path $tmp 'Paneldeck-Setup-probe.exe'
    & $makensis -V1 "-DVERSION=probe" "-DEXE=$root\target\release\paneldeck.exe" "-DOUTFILE=$New" "$root\installer\paneldeck.nsi" | Out-Null
    if (-not (Test-Path $New)) { throw 'Probe-Installer nicht gebaut' }
  }
  if (-not $Old) {
    $cand = Get-ChildItem (Join-Path $root 'dist\Paneldeck-Setup-*.exe') -ErrorAction SilentlyContinue | Where-Object { (Get-FileHash $_.FullName).Hash -ne (Get-FileHash $New).Hash } | Sort-Object LastWriteTime -Descending | Select-Object -First 1
    if ($cand) { $Old = $cand.FullName }
    else {
      if (-not $makensis) { throw 'makensis nicht gefunden (für den Vorgänger-Installer)' }
      $oldExe = Join-Path $tmp 'paneldeck-old.exe'
      Copy-Item "$root\target\release\paneldeck.exe" $oldExe
      $fs = [IO.File]::Open($oldExe, 'Append'); $fs.Write([byte[]](1..64), 0, 64); $fs.Close()   # Overlay: Programm läuft unverändert, Hash anders
      $Old = Join-Path $tmp 'Paneldeck-Setup-old.exe'
      & $makensis -V1 "-DVERSION=0.0.0-old" "-DEXE=$oldExe" "-DOUTFILE=$Old" "$root\installer\paneldeck.nsi" | Out-Null
    }
  }
  if (-not (Test-Path $Old)) { throw "Alter Installer fehlt: $Old" }
  $newExeHash = (Get-FileHash "$root\target\release\paneldeck.exe").Hash
  Write-Host "== Alt installieren ($(Split-Path -Leaf $Old))"
  $rc = Install $Old 'alt'
  Check 'Installer (alt) Exit 0' ($rc -eq 0) "rc=$rc"
  Check 'paneldeck.exe vorhanden' (Test-Path "$inst\paneldeck.exe")
  $oldExeHash = (Get-FileHash "$inst\paneldeck.exe").Hash
  $oldVersion = Reg 'HKCU:\Software\Microsoft\Windows\CurrentVersion\Uninstall\Paneldeck' 'DisplayVersion'
  Check 'Kontextmenü (alt) angelegt' ([bool](Reg 'HKCU:\Software\Classes\Directory\shell\Paneldeck'))
  Check 'Desktop-Verknüpfung im Schein-Profil' (Test-Path (Join-Path $prof 'Desktop\Paneldeck.lnk'))
  Check 'Autostart standardmäßig aus' (-not (Reg $runKey 'Paneldeck'))

  Write-Host '== Nutzerdaten und Autostart anlegen'
  New-Item -ItemType Directory -Force "$data\themes" | Out-Null
  $u8 = New-Object Text.UTF8Encoding($false)
  [IO.File]::WriteAllText("$data\settings.json", '{"language":"en","mcp_level":"control","font_size":17.5,"marker":"vor-dem-update"}', $u8)
  [IO.File]::WriteAllText("$data\state.json", '{"projects":[{"id":1,"name":"Mein Projekt"}],"note":"Sitzungen müssen bleiben"}', $u8)
  [IO.File]::WriteAllText("$data\prompts.json", '[{"title":"Test","text":"hallo"}]', $u8)
  [IO.File]::WriteAllText("$data\themes\mein-theme.json", '{"name":"Mein Theme"}', $u8)
  $runVal = '"' + $inst + '\paneldeck.exe" --tray'
  New-ItemProperty -Path $runKey -Name Paneldeck -Value $runVal -PropertyType String -Force | Out-Null
  $dataBefore = Hashes $data

  Write-Host '== Update bei ruhendem Hub'
  $rc = Install $New 'neu'
  Check 'Installer (neu) Exit 0' ($rc -eq 0) "rc=$rc"
  Check 'paneldeck.exe ist jetzt die neue' ((Get-FileHash "$inst\paneldeck.exe").Hash -eq $newExeHash)
  Check 'paneldeck.exe unterscheidet sich von der alten' ($newExeHash -ne $oldExeHash)
  Check 'Version in der Programmliste aktualisiert' ((Reg 'HKCU:\Software\Microsoft\Windows\CurrentVersion\Uninstall\Paneldeck' 'DisplayVersion') -ne $oldVersion)
  Check 'Einstellungen/Sitzungen/Prompts/Themes unverändert' (SameHashes $dataBefore (Hashes $data))
  Check 'Autostart bleibt erhalten' ((Reg $runKey 'Paneldeck') -eq $runVal)
  Check 'Kontextmenü bleibt (Ordner)' ((Reg 'HKCU:\Software\Classes\Directory\shell\Paneldeck\command') -like "*$inst\paneldeck.exe*--open*")
  Check 'Kontextmenü bleibt (Hintergrund, mit Claude)' ((Reg 'HKCU:\Software\Classes\Directory\Background\shell\PaneldeckClaude\command') -like '*--claude*')
  Check 'Deinstaller vorhanden' (Test-Path "$inst\Uninstall.exe")
  Check 'Lizenzen installiert (LICENSE, THIRD_PARTY_LICENSES.txt, licenses)' ((Test-Path "$inst\LICENSE") -and (Test-Path "$inst\THIRD_PARTY_LICENSES.txt") -and (Test-Path "$inst\licenses\LICENSE-Cascadia.txt"))

  Write-Host '== Update bei LAUFENDEM Hub (Datei gesperrt)'
  [void](Install $Old 'alt')   # zurück auf alt, Daten bleiben
  Check 'zurück auf alte exe' ((Get-FileHash "$inst\paneldeck.exe").Hash -eq $oldExeHash)
  $env:USERPROFILE = $prof
  $hub = Start-Process "$inst\paneldeck.exe" -ArgumentList '--tray' -PassThru
  $env:USERPROFILE = $oldProfile
  $t0 = Get-Date; while (-not (Test-Path "$data\ipc.json") -and ((Get-Date) - $t0).TotalSeconds -lt 20) { Start-Sleep -Milliseconds 300 }
  Check 'Hub läuft (ipc.json da)' ((Test-Path "$data\ipc.json") -and -not $hub.HasExited)
  $dataRunning = Hashes $data
  $rc = Install $New 'neu (laufend)'
  Write-Host "      Installer-Exit: $rc"
  Check 'Installer endet (kein Hängen)' ($rc -ne -999) "rc=$rc"
  Check 'neue exe liegt am Platz (trotz laufendem Hub)' ((Get-FileHash "$inst\paneldeck.exe").Hash -eq $newExeHash)
  Check 'Hub läuft weiter (Sitzungen nicht abgebrochen)' (-not $hub.HasExited)
  Check 'Nutzerdaten unverändert' (SameHashes $dataRunning (Hashes $data))
  Check 'Autostart bleibt erhalten' ((Reg $runKey 'Paneldeck') -eq $runVal)
  Check 'alte exe als paneldeck.exe.old zurückgeblieben (sie lief ja noch)' (Test-Path "$inst\paneldeck.exe.old")
  Write-Host '== Neuer Hub räumt die .old-Datei beim Start auf'
  Stop-Process -Id $hub.Id -Force; Start-Sleep -Milliseconds 800
  $env:USERPROFILE = $prof
  $hub2 = Start-Process "$inst\paneldeck.exe" -ArgumentList '--tray' -PassThru
  $env:USERPROFILE = $oldProfile
  $t0 = Get-Date; while ((Test-Path "$inst\paneldeck.exe.old") -and ((Get-Date) - $t0).TotalSeconds -lt 15) { Start-Sleep -Milliseconds 300 }
  Check 'paneldeck.exe.old ist nach dem Start weg' (-not (Test-Path "$inst\paneldeck.exe.old"))
}
finally {
  $env:USERPROFILE = $oldProfile
  Get-Process paneldeck -ErrorAction SilentlyContinue | Where-Object { $_.Path -like "$tmp*" } | Stop-Process -Force
  Start-Sleep -Milliseconds 700
  foreach ($k in $regKeys) { Remove-Item $k -Recurse -Force -ErrorAction SilentlyContinue }
  Remove-ItemProperty -Path $runKey -Name Paneldeck -ErrorAction SilentlyContinue
  Remove-Item $tmp -Recurse -Force -ErrorAction SilentlyContinue
  Check 'Aufgeräumt: kein Registry-Rest' (-not (@($regKeys | Where-Object { Test-Path $_ }).Count) -and -not (Get-ItemProperty $runKey -Name Paneldeck -ErrorAction SilentlyContinue))
  Check 'Echter Desktop/Startmenü unberührt' (((Test-Path $realDesktopLnk) -eq $beforeDesktop) -and ((Test-Path $realStart) -eq $beforeStart))
  Check 'Temp-Ordner entfernt' (-not (Test-Path $tmp))
}
Write-Host "`nErgebnis: $pass bestanden, $fail fehlgeschlagen"
if ($fail) { exit 1 }
