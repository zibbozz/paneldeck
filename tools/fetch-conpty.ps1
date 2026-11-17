<#
.SYNOPSIS
  Holt die mitgelieferte ConPTY (conpty.dll + OpenConsole.exe, x64) aus dem NuGet-Paket Microsoft.Windows.Console.ConPTY
  (MIT, Microsoft) und prueft sie gegen fest eingetragene SHA-256-Werte.

.DESCRIPTION
  Paneldeck legt beide Dateien neben paneldeck.exe (Installer und portables ZIP). Sie stehen nicht im Git: dieses Skript
  laedt das Paket in der festgelegten Version (einmal, Zwischenspeicher dist\conpty-cache\<version>), prueft die Pruefsumme
  des Pakets und der beiden Dateien und kopiert sie in die Zielordner. Stimmt eine Pruefsumme nicht, bricht es ab (Exit 1)
  und kopiert nichts.
  Aufrufer: tools\release.sh und tools\package.sh (Ziel dist\conpty), fuer Entwicklungslaeufe von Hand:
    powershell -ExecutionPolicy Bypass -File tools\fetch-conpty.ps1 -Dest target\release
  (bzw. <CARGO_TARGET_DIR>\release), damit target\release\paneldeck.exe und der Smoke-Test die mitgelieferte ConPTY nutzen.
  Neue Version: $Version und die drei SHA-256-Werte unten anpassen (Werte im Paket pruefen, Signatur: Microsoft Corporation),
  THIRD_PARTY.md und tools\license-extra\conpty-MIT.txt pruefen, python tools\gen_licenses.py.

.EXAMPLE
  powershell -ExecutionPolicy Bypass -File tools\fetch-conpty.ps1
  powershell -ExecutionPolicy Bypass -File tools\fetch-conpty.ps1 -Dest target\release,dist\conpty
#>
param(
  # Zielordner (einer oder mehrere); Standard: dist\conpty
  [string[]]$Dest = @("$PSScriptRoot\..\dist\conpty"),
  # Zwischenspeicher fuer das Paket (Standard: dist\conpty-cache)
  [string]$Cache = "$PSScriptRoot\..\dist\conpty-cache",
  # Nur pruefen, ob die Dateien in den Zielordnern vorhanden und richtig sind (nichts laden, nichts kopieren)
  [switch]$Check
)
$ErrorActionPreference = 'Stop'

# Festgelegte Version (neueste stabile beim Eintragen) und Pruefsummen.
$Version = '1.25.261109001'
$NupkgSha256 = '3d77f535df848e44225c2a8912ac844fecb23f3057732b639f3034b587496f09'
$Files = [ordered]@{
  # Name im Ziel = (Pfad im Paket, SHA-256)
  'conpty.dll'      = @('runtimes/win-x64/native/conpty.dll', 'e971856f89748dc3b458125e9afdd79b5e9574a7c937676116d3cbbd3b71f492')
  'OpenConsole.exe' = @('build/native/runtimes/x64/OpenConsole.exe', 'b17e48b07ef153b420f0a6e178bacd31fb4c3390e21ae746586213fc0719a775')
}
$Url = "https://www.nuget.org/api/v2/package/Microsoft.Windows.Console.ConPTY/$Version"

function Get-Sha256([string]$path) { (Get-FileHash -Algorithm SHA256 -LiteralPath $path).Hash.ToLowerInvariant() }
function Test-Files([string]$dir) {
  foreach ($n in $Files.Keys) {
    $f = Join-Path $dir $n
    if (-not (Test-Path -LiteralPath $f -PathType Leaf)) { return $false }
    if ((Get-Sha256 $f) -ne $Files[$n][1]) { return $false }
  }
  $true
}

if ($Check) {
  $bad = @($Dest | Where-Object { -not (Test-Files $_) })
  if ($bad.Count) { Write-Host "ConPTY $Version fehlt oder passt nicht in: $($bad -join ', ')" -ForegroundColor Red; exit 1 }
  Write-Host "ConPTY $Version vorhanden und geprueft: $($Dest -join ', ')"
  exit 0
}

$cacheDir = Join-Path $Cache $Version
New-Item -ItemType Directory -Force $cacheDir | Out-Null
$cacheDir = (Resolve-Path $cacheDir).Path
if (-not (Test-Files $cacheDir)) {
  $pkg = Join-Path $cacheDir "Microsoft.Windows.Console.ConPTY.$Version.nupkg"
  if (-not (Test-Path -LiteralPath $pkg) -or (Get-Sha256 $pkg) -ne $NupkgSha256) {
    Write-Host "Lade $Url"
    [Net.ServicePointManager]::SecurityProtocol = [Net.SecurityProtocolType]::Tls12
    $tmp = "$pkg.part"
    $ProgressPreference = 'SilentlyContinue'
    Invoke-WebRequest -UseBasicParsing -Uri $Url -OutFile $tmp
    $h = Get-Sha256 $tmp
    if ($h -ne $NupkgSha256) {
      Remove-Item -LiteralPath $tmp -Force
      Write-Host "SHA-256 des Pakets passt nicht: $h (erwartet $NupkgSha256)" -ForegroundColor Red
      exit 1
    }
    Move-Item -Force -LiteralPath $tmp -Destination $pkg
  }
  Add-Type -AssemblyName System.IO.Compression.FileSystem
  $zip = [IO.Compression.ZipFile]::OpenRead($pkg)
  try {
    foreach ($n in $Files.Keys) {
      $e = $zip.GetEntry($Files[$n][0])
      if (-not $e) { Write-Host "Paket enthaelt $($Files[$n][0]) nicht" -ForegroundColor Red; exit 1 }
      $out = Join-Path $cacheDir $n
      [IO.Compression.ZipFileExtensions]::ExtractToFile($e, $out, $true)
      $h = Get-Sha256 $out
      if ($h -ne $Files[$n][1]) {
        Remove-Item -LiteralPath $out -Force
        Write-Host "SHA-256 von $n passt nicht: $h (erwartet $($Files[$n][1]))" -ForegroundColor Red
        exit 1
      }
    }
  } finally { $zip.Dispose() }
}

# Signatur nur zur Information (die Pruefsummen sind massgeblich; die Kettenpruefung kann offline scheitern).
foreach ($n in $Files.Keys) {
  $s = Get-AuthenticodeSignature -LiteralPath (Join-Path $cacheDir $n)
  $who = if ($s.SignerCertificate) { $s.SignerCertificate.GetNameInfo('SimpleName', $false) } else { '-' }
  if ($s.Status -ne 'Valid' -or $who -ne 'Microsoft Corporation') {
    Write-Host "Hinweis: Signatur von $n ist $($s.Status) ($who)" -ForegroundColor Yellow
  }
}

foreach ($d in $Dest) {
  New-Item -ItemType Directory -Force $d | Out-Null
  foreach ($n in $Files.Keys) {
    $target = Join-Path $d $n
    # Reste frueherer Laeufe (<name>.old, <name>.old.<n>) aufraeumen; was noch laeuft bzw. geladen ist, bleibt liegen.
    Get-ChildItem -LiteralPath $d -File -ErrorAction SilentlyContinue | Where-Object { $_.Name -match ('^' + [regex]::Escape($n) + '\.old(\.\d+)?$') } |
      ForEach-Object { Remove-Item -LiteralPath $_.FullName -Force -ErrorAction SilentlyContinue }
    if ((Test-Path -LiteralPath $target) -and (Get-Sha256 $target) -eq $Files[$n][1]) { continue }
    # Eine laufende OpenConsole.exe bzw. geladene conpty.dll (Paneldeck aus diesem Ordner laeuft) laesst sich nicht
    # ueberschreiben, aber umbenennen - unter einem freien Namen <name>.old[.<n>].
    if (Test-Path -LiteralPath $target) {
      try { Remove-Item -LiteralPath $target -Force -ErrorAction Stop } catch {
        $aside = "$target.old"; $i = 0
        while (Test-Path -LiteralPath $aside) { $i++; $aside = "$target.old.$i" }
        Move-Item -LiteralPath $target -Destination $aside
      }
    }
    Copy-Item -LiteralPath (Join-Path $cacheDir $n) -Destination $target
  }
  Write-Host "ConPTY $Version -> $((Resolve-Path $d).Path)"
}
exit 0
