# Holt die Lucide-Icons (https://lucide.dev, ISC) in einer festen Version und erzeugt die eingebetteten Dateien neu:
#   assets/fonts/lucide.ttf, assets/fonts/LICENSE-Lucide.txt, tools/license-extra/lucide-LICENSE.txt, assets/lucide/icons.tsv
#
# Verwendung (PowerShell, im Repository):  powershell -ExecutionPolicy Bypass -File tools\update-lucide.ps1 [-WorkRoot <ordner>]
#
# Quellen (beide mit fest eingetragenem SHA-256; weicht er ab, bricht das Skript ab):
#   - npm-Paket lucide-static <Version>: Schrift font/lucide.ttf, Zeichen-Tabelle font/info.json, Stichwoerter tags.json, LICENSE
#   - Quell-Stand von GitHub zum Tag <Version>: icons/*.json (Kategorien je Icon) und categories/*.json (Kategorie-Titel).
#     Die npm-Pakete enthalten keine Kategorien. Hinweis: GitHub erzeugt das Archiv selbst; aendert sich dessen Hash bei
#     gleichem Tag, den Inhalt pruefen und den Hash hier neu eintragen.
# Neue Version: $Version und beide Hashes aendern, Skript laufen lassen, danach
#   python tools/gen_licenses.py   und   cargo test --release   (die Tests pruefen Tabelle und Schrift).
# Laufzeit-Netzwerk gibt es keins: Paneldeck bettet die erzeugten Dateien ein.
param([string]$WorkRoot = $env:TEMP)

$ErrorActionPreference = 'Stop'
$Version = '1.66.0'
$NpmUrl = "https://registry.npmjs.org/lucide-static/-/lucide-static-$Version.tgz"
$NpmSha256 = '6a1debc19076123ece73f6729a9643ac90aab9c9db15f7c16debf72a9a02a82a'
$SrcUrl = "https://codeload.github.com/lucide-icons/lucide/tar.gz/refs/tags/$Version"
$SrcSha256 = '5b9428e376f32a5be2e58b0f55627d838fa97d978dec43698ec31ffcf79ab182'

$Root = Split-Path -Parent $PSScriptRoot
# Heruntergeladenes in einen neuen, leeren Ordner (nie im Repository entpacken).
$Work = Join-Path $WorkRoot ("paneldeck-lucide-" + [guid]::NewGuid().ToString('N').Substring(0, 8))
New-Item -ItemType Directory -Force $Work | Out-Null
Write-Host "Arbeitsordner: $Work"

function Get-Checked([string]$Url, [string]$File, [string]$Sha) {
  $path = Join-Path $Work $File
  & curl.exe -sSfL -o $path $Url
  if ($LASTEXITCODE -ne 0) { throw "Download fehlgeschlagen: $Url" }
  $h = (Get-FileHash -Algorithm SHA256 $path).Hash.ToLowerInvariant()
  if ($h -ne $Sha) { throw "SHA-256 von $File passt nicht: $h (erwartet $Sha)" }
  Write-Host "ok  $File  $h"
  return $path
}

$npm = Get-Checked $NpmUrl "lucide-static-$Version.tgz" $NpmSha256
$src = Get-Checked $SrcUrl "lucide-$Version.tar.gz" $SrcSha256

$npmDir = Join-Path $Work 'npm'
$srcDir = Join-Path $Work 'src'
New-Item -ItemType Directory -Force $npmDir, $srcDir | Out-Null
& tar.exe -xzf $npm -C $npmDir package/LICENSE package/tags.json package/font/info.json package/font/lucide.ttf
if ($LASTEXITCODE -ne 0) { throw 'Entpacken von lucide-static fehlgeschlagen' }
& tar.exe -xzf $src -C $srcDir --strip-components 1 "lucide-$Version/categories" "lucide-$Version/icons"
if ($LASTEXITCODE -ne 0) { throw 'Entpacken des Quell-Stands fehlgeschlagen' }

$py = (Get-Command python -ErrorAction SilentlyContinue)
if (-not $py) { throw 'python nicht gefunden' }
$env:PYTHONUTF8 = '1'
& $py.Source -I (Join-Path $PSScriptRoot 'lucide_table.py') (Join-Path $npmDir 'package') $srcDir $Version $Root
if ($LASTEXITCODE -ne 0) { throw 'Erzeugen der Tabelle fehlgeschlagen' }

foreach ($f in 'assets\fonts\lucide.ttf', 'assets\lucide\icons.tsv') {
  $p = Join-Path $Root $f
  Write-Host ("{0,-28} {1,9} Bytes  SHA-256 {2}" -f $f, (Get-Item $p).Length, (Get-FileHash -Algorithm SHA256 $p).Hash.ToLowerInvariant())
}
Remove-Item -Recurse -Force $Work
Write-Host 'Fertig. Danach: python tools/gen_licenses.py und cargo test --release'
