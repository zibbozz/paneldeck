<#
  Prüft Release-Dateien mit Microsoft Defender auf Fehlalarme (False Positives), bevor sie veröffentlicht werden.
  Vorgabe des Nutzers (2026-11-15): vor jedem Release paneldeck.exe UND den Installer prüfen; bei einem Treffer
  hilft Claude beim Formular https://www.microsoft.com/en-us/wdsi/filesubmission?persona=SoftwareDeveloper
  (Feldwerte gibt dieses Skript am Ende aus).

  Aufruf:  powershell -NoProfile -ExecutionPolicy Bypass -File tools\defender-scan.ps1 dist\paneldeck-2.1.0.exe dist\Paneldeck-Setup-2.1.0.exe

  Jede Datei wird in einen frischen Ordner unter -WorkRoot (Standard: %TEMP%, also NICHT im Defender-Ausnahme-Ordner
  E:\paneldeck-test) kopiert und dort mit MpCmdRun ohne Entfernen gescannt (-DisableRemediation). Entfernt der
  Echtzeitschutz die Kopie schon vorher, zählt das ebenfalls als Treffer. Die Originale bleiben unberührt.
  Exit-Code: 0 = alles sauber · 1 = mindestens ein Treffer · 3 = Defender nicht verfügbar (Prüfung nicht möglich).
#>
[CmdletBinding(PositionalBinding = $false)]
param(
  [Parameter(Mandatory = $true, ValueFromRemainingArguments = $true)][string[]]$Files,
  [string]$WorkRoot = $env:TEMP
)
$ErrorActionPreference = 'Stop'

$mp = Join-Path $env:ProgramFiles 'Windows Defender\MpCmdRun.exe'
if (-not (Test-Path $mp)) { Write-Host "Defender (MpCmdRun.exe) nicht gefunden – Prüfung nicht möglich." -ForegroundColor Yellow; exit 3 }

$sig = $null
try { $s = Get-MpComputerStatus; $sig = $s.AntivirusSignatureVersion; $eng = $s.AMEngineVersion; $plat = $s.AMProductVersion } catch { }

$found = @()
foreach ($f in $Files) {
  if (-not (Test-Path $f)) { Write-Host "Datei fehlt: $f" -ForegroundColor Red; exit 1 }
  $full = (Resolve-Path $f).Path
  $name = Split-Path $full -Leaf
  $hash = (Get-FileHash $full -Algorithm SHA256).Hash
  $dir = Join-Path $WorkRoot ('pd-defscan-' + [guid]::NewGuid().ToString('N').Substring(0, 8))
  New-Item -ItemType Directory -Force $dir | Out-Null
  $copy = Join-Path $dir $name
  $threat = $null
  try {
    $since = Get-Date
    $copyErr = $null
    for ($i = 0; $i -lt 3; $i++) {
      try { Copy-Item $full $copy -ErrorAction Stop; $copyErr = $null; break } catch { $copyErr = $_.Exception.Message; Start-Sleep -Seconds 2 }
    }
    Start-Sleep -Seconds 2
    if ($copyErr -or -not (Test-Path $copy)) {
      # Nur eine echte Defender-Erkennung zählt als Treffer; sonst ist die Prüfung gescheitert (Exit 3).
      $det = Get-MpThreatDetection -ErrorAction SilentlyContinue | Where-Object { $_.InitialDetectionTime -ge $since.AddSeconds(-5) -and (($_.Resources -join ' ') -like "*$dir*") }
      if ($det) {
        $tid = ($det | Select-Object -First 1).ThreatID
        $tn = (Get-MpThreat -ErrorAction SilentlyContinue | Where-Object ThreatID -eq $tid | Select-Object -First 1).ThreatName
        $threat = if ($tn) { "$tn (Echtzeitschutz)" } else { "ThreatID $tid (Echtzeitschutz)" }
      } else {
        Write-Host "Kopie von $name fehlgeschlagen, keine Defender-Erkennung: $copyErr" -ForegroundColor Yellow; exit 3
      }
    }
    if (-not $threat) {
      $out = & $mp -Scan -ScanType 3 -File $copy -DisableRemediation 2>&1 | Out-String
      if ($LASTEXITCODE -eq 2 -or $out -match 'found [1-9]') {
        $m = [regex]::Match($out, 'Threat\s+:\s*(.+)')
        $threat = if ($m.Success) { $m.Groups[1].Value.Trim() } else { '(Treffer, Name unbekannt)' }
      } elseif ($LASTEXITCODE -ne 0) {
        Write-Host "MpCmdRun-Fehler ($LASTEXITCODE) bei $name`n$out" -ForegroundColor Yellow; exit 3
      }
    }
  } finally {
    Remove-Item -Recurse -Force $dir -ErrorAction SilentlyContinue
  }
  if ($threat) {
    Write-Host ("ERKANNT  {0}  → {1}" -f $name, $threat) -ForegroundColor Red
    $found += [pscustomobject]@{ Name = $name; Path = $full; Threat = $threat; Sha256 = $hash }
  } else {
    Write-Host ("sauber   {0}  (SHA-256 {1})" -f $name, $hash) -ForegroundColor Green
  }
}

if ($found.Count -eq 0) { Write-Host "Defender: keine Erkennung (Signaturen $sig)."; exit 0 }

Write-Host ""
Write-Host "== Fehlalarm bei Microsoft melden: https://www.microsoft.com/en-us/wdsi/filesubmission?persona=SoftwareDeveloper" -ForegroundColor Cyan
Write-Host "   Security product: Microsoft Defender Antivirus (Windows) · Was ist die Datei: Incorrectly detected as malware/malicious"
Write-Host "   Definition version: $sig (Engine $eng, Platform $plat)"
foreach ($x in $found) {
  Write-Host ("   Datei: {0}`n     Detection name: {1}`n     SHA-256: {2}" -f $x.Path, $x.Threat, $x.Sha256)
}
Write-Host "   Text für 'Additional information': siehe CLAUDE.md, Abschnitt Release → Defender-Prüfung (Claude hilft beim Ausfüllen)."
exit 1
