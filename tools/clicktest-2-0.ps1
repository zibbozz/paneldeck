# Klicktest-Hilfen fuer Paneldeck 2.0.0 (Windows PowerShell 5.1). Dot-source: . tools\clicktest-2-0.ps1
# Reines Testwerkzeug (kein Teil des Programms/Releases): erlaubt Klicktests auch bei GESPERRTEM Bildschirm. Fuer Mausklicks per
# Fensternachricht braucht es eine GEPATCHTE winit-Kopie (siehe unten, Zwei Eingabewege 2); diese Kopie ist NICHT im Repo.
# Baut auf tools\gui-test.ps1 auf. Startet EINE Testinstanz aus einer Kopie der .exe (eigener Datenordner, Schein-USERPROFILE),
# platziert das Fenster, klickt in Fensterkoordinaten, macht Screenshots (PrintWindow), spricht per MCP-Bruecke mit der Instanz.
# Zustand zwischen Aufrufen steht in $CT\state.json (PID), weil jeder Aufruf eine neue PowerShell ist.
# VORSICHT: Echte Maus und Tastatur werden benutzt. Nur nach Freigabe durch den Nutzer und unter der Sperre /tmp/paneldeck-ui-lock.
#
# Zwei Eingabewege:
#  1) Echte Eingabe (gui-test.ps1: Click/Press/TypeText ueber SendInput) - funktioniert nur bei entsperrtem Bildschirm.
#  2) Fensternachrichten (PC/PD/PW/PK/PK2/PT/PT2/PDot, SClick): funktionieren auch bei GESPERRTEM Bildschirm (Nutzer unterwegs), ohne die
#     Maus des Nutzers zu bewegen. Einschraenkungen: kein Nicht-Client-Bereich (Fensterrahmen, Snap-Layouts), keine nativen Datei-Dialoge,
#     Umlaute/Sonderzeichen gehen beim Tippen verloren (winit liest WM_CHAR nur nach echtem Tastendruck).
#     Mausklicks per PostMessage gehen NUR mit einer winit-Variante ohne TrackMouseEvent (sonst kommt bei gesperrtem Bildschirm sofort
#     WM_MOUSELEAVE und egui verwirft den Zeiger): crates.io-winit 0.30.13 kopieren, in event_loop.rs (WM_MOUSEMOVE, PointerMoveKind::Enter)
#     den TrackMouseEvent-Aufruf entfernen, mit `cargo build --release --config patch.crates-io.winit.path=...` bauen und die .exe
#     per $env:CT_EXE angeben. Beobachtung ueber das MCP-Werkzeug `screenshot` (S <name>), nicht ueber PrintWindow.
#  Beispiel: . tools\clicktest-2-0.ps1; CT-Start -Lang De; P-Focus; PC 261 104; S tickets; CT-Cpu 10; CT-Stop
. "$PSScriptRoot\gui-test.ps1"
$script:CT = "$env:TEMP\ptest"
$script:SHOTS = if ($env:CT_SHOTS) { $env:CT_SHOTS } else { "$script:CT\shots" }
$script:EXE = if ($env:CT_EXE) { $env:CT_EXE } else { (Resolve-Path "$PSScriptRoot\..\target\release\paneldeck.exe").Path }
$script:WX = 40; $script:WY = 30; $script:WW = 1500; $script:WH = 960

function CT-State { $f = "$script:CT\state.json"; if (Test-Path $f) { Get-Content $f -Raw | ConvertFrom-Json } else { $null } }
function CT-Proc { $s = CT-State; if ($s) { Get-Process -Id $s.pid -ErrorAction SilentlyContinue } }

# Startet eine frische Instanz. -Keep: vorhandenen Datenordner behalten (Neustart-Test). -Lang De|En. -Level Off|Read|Control.
function CT-Start([string]$Lang = 'En', [switch]$Keep, [string]$Level = 'Control', [string]$Seed = '') {
  CT-Stop
  $hub = "$script:CT\hub"; $prof = "$script:CT\profile"
  if (-not $Keep) { Remove-Item $hub -Recurse -Force -ErrorAction SilentlyContinue }
  New-Item -ItemType Directory -Force $hub, $prof, $script:SHOTS, "$prof\.claude" | Out-Null
  Copy-Item $script:EXE "$hub\paneldeck.exe" -Force
  $data = "$hub\paneldeck-data"; New-Item -ItemType Directory -Force $data | Out-Null
  $f = "$data\settings.json"
  $j = if (Test-Path $f) { [IO.File]::ReadAllText($f).TrimStart([char]0xFEFF) | ConvertFrom-Json } else { [pscustomobject]@{} }
  foreach ($kv in @{ language = $Lang; mcp_level = $Level }.GetEnumerator()) { if ($j.PSObject.Properties.Name -contains $kv.Key) { $j.($kv.Key) = $kv.Value } else { $j | Add-Member $kv.Key $kv.Value } }
  [IO.File]::WriteAllText($f, ($j | ConvertTo-Json -Depth 10), (New-Object Text.UTF8Encoding $false))
  if ($Seed) { $env:PYTHONUTF8 = '1'; & python -I $Seed "$data\paneldeck.db" }
  $old = @{ U = $env:USERPROFILE; H = $env:HOME }
  $env:USERPROFILE = $prof; $env:HOME = $prof
  $p = Start-Process "$hub\paneldeck.exe" -PassThru -WorkingDirectory $hub
  $env:USERPROFILE = $old.U; $env:HOME = $old.H
  [pscustomobject]@{ pid = $p.Id } | ConvertTo-Json | Set-Content "$script:CT\state.json"
  $t0 = Get-Date; $h = $null
  while (-not $h -and ((Get-Date) - $t0).TotalSeconds -lt 30) { Start-Sleep -Milliseconds 400; $h = Get-HubWin $p.Id }
  Start-Sleep 2
  CT-Place
  "started pid=$($p.Id) hwnd=$h"
}
function CT-Stop {
  $p = CT-Proc; if ($p) { $p.Kill(); Start-Sleep -Milliseconds 700 }
  Get-CimInstance Win32_Process -Filter "Name='paneldeck.exe'" | Where-Object { $_.ExecutablePath -like "$script:CT\*" } | ForEach-Object { Stop-Process -Id $_.ProcessId -Force -ErrorAction SilentlyContinue }
  Remove-Item "$script:CT\state.json" -ErrorAction SilentlyContinue
}
function CT-Win { $p = CT-Proc; if ($p) { Get-HubWin $p.Id } }
function CT-Place { $h = CT-Win; if ($h) { [void][W32]::MoveWindow($h, $script:WX, $script:WY, $script:WW, $script:WH, $true); Start-Sleep -Milliseconds 400 } }
# Fenster nach vorn (Top) fuer Klicks; am Ende eines Aufrufs CT-Release
function CT-Up { $h = CT-Win; Focus $h; Top $h; Start-Sleep -Milliseconds 200 }
function CT-Release { $h = CT-Win; if ($h) { Untop $h } }
# Klick in Fensterkoordinaten (Pixel relativ zur linken oberen Fensterecke)
function C([int]$x, [int]$y, [string]$btn = 'left', [int]$n = 1) { $r = Rect (CT-Win); Click ($r.L + $x) ($r.T + $y) $btn $n }
function M([int]$x, [int]$y) { $r = Rect (CT-Win); [void][W32]::SetCursorPos($r.L + $x, $r.T + $y); Start-Sleep -Milliseconds 700 }
function D([int]$x1, [int]$y1, [int]$x2, [int]$y2) { $r = Rect (CT-Win); Drag ($r.L + $x1) ($r.T + $y1) ($r.L + $x2) ($r.T + $y2) }
function Wh([int]$x, [int]$y, [int]$delta) { $r = Rect (CT-Win); Wheel ($r.L + $x) ($r.T + $y) $delta }
function Snap([string]$name) { $h = CT-Win; ShotPW $h "$script:SHOTS\$name.png"; "$script:SHOTS\$name.png" }
# Prozess-CPU in % ueber $sec Sekunden Leerlauf
function CT-Cpu([int]$sec = 10) {
  $p = CT-Proc; $a = $p.TotalProcessorTime.TotalMilliseconds; $t0 = Get-Date; Start-Sleep $sec
  $p.Refresh(); $b = $p.TotalProcessorTime.TotalMilliseconds
  [math]::Round(($b - $a) / (((Get-Date) - $t0).TotalMilliseconds * [Environment]::ProcessorCount) * 100, 2)
}

# MCP: mehrere Aufrufe in einer Bruecke. $calls = @( @{ n='create_ticket'; a=@{...} }, ... ). Gibt je Aufruf das Textergebnis aus.
function Mcp([object[]]$calls) {
  $psi = New-Object System.Diagnostics.ProcessStartInfo
  $psi.FileName = "$script:CT\hub\paneldeck.exe"; $psi.Arguments = 'mcp'; $psi.WorkingDirectory = "$script:CT\hub"
  $psi.RedirectStandardInput = $true; $psi.RedirectStandardOutput = $true; $psi.RedirectStandardError = $true
  $psi.UseShellExecute = $false; $psi.CreateNoWindow = $true; $psi.StandardOutputEncoding = [Text.Encoding]::UTF8
  $psi.EnvironmentVariables['USERPROFILE'] = "$script:CT\profile"
  $b = [System.Diagnostics.Process]::Start($psi); $id = 0
  $send = { param($m) $bytes = (New-Object Text.UTF8Encoding $false).GetBytes(($m | ConvertTo-Json -Depth 20 -Compress) + "`n"); $b.StandardInput.BaseStream.Write($bytes, 0, $bytes.Length); $b.StandardInput.BaseStream.Flush() }
  $read = { $t = $b.StandardOutput.ReadLineAsync(); if ($t.Wait(30000)) { $t.Result | ConvertFrom-Json } else { $null } }
  & $send @{ jsonrpc = '2.0'; id = 1; method = 'initialize'; params = @{ protocolVersion = '2025-06-18'; capabilities = @{}; clientInfo = @{ name = 'clicktest'; version = '1' } } }; [void](& $read)
  $ni = [Text.Encoding]::UTF8.GetBytes("{`"jsonrpc`":`"2.0`",`"method`":`"notifications/initialized`"}`n"); $b.StandardInput.BaseStream.Write($ni, 0, $ni.Length); $b.StandardInput.BaseStream.Flush()
  $id = 1
  foreach ($c in $calls) {
    $id++; & $send @{ jsonrpc = '2.0'; id = $id; method = 'tools/call'; params = @{ name = $c.n; arguments = $c.a } }
    $r = & $read
    if ($r -and $r.result) {
      $img = $r.result.content | Where-Object type -eq 'image' | Select-Object -First 1
      if ($img) { $out = if ($c.o) { $c.o } else { "$script:SHOTS\mcp-$id.png" }; [IO.File]::WriteAllBytes($out, [Convert]::FromBase64String($img.data)); "image saved: $out" }
      $txt =($r.result.content | Where-Object type -eq 'text' | Select-Object -First 1).text; "$($c.n): $(if ($r.result.isError) { 'ERR ' })$txt" } else { "$($c.n): KEINE ANTWORT" }
  }
  try { $b.StandardInput.Close(); [void]$b.WaitForExit(2000) } catch {}; if (-not $b.HasExited) { $b.Kill() }
}

# ===== Synthetische Eingabe per PostMessage (funktioniert auch bei gesperrtem Bildschirm / ohne Eingabe-Desktop) =====
# Reale Maus/Tastatur (SendInput) scheitern, wenn der Windows-Bildschirm gesperrt ist (GetCursorPos/SetCursorPos/CopyFromScreen schlagen fehl).
# Dann werden die Fensternachrichten direkt an das Paneldeck-Fenster geschickt; Beobachtung per MCP-Werkzeug `screenshot` (Seeing: Shot).
if (-not ([System.Management.Automation.PSTypeName]'PM').Type) {
Add-Type -TypeDefinition @"
using System; using System.Runtime.InteropServices;
public class PM {
  [DllImport("user32.dll")] public static extern bool PostMessageW(IntPtr h, uint m, IntPtr w, IntPtr l);
  [DllImport("user32.dll")] public static extern uint MapVirtualKeyW(uint c, uint t);
  [DllImport("user32.dll")] public static extern IntPtr SetFocus(IntPtr h);
  [DllImport("user32.dll")] public static extern bool AttachThreadInput(uint a, uint b, bool f);
  [DllImport("user32.dll")] public static extern uint GetWindowThreadProcessId(IntPtr h, out uint p);
  [DllImport("kernel32.dll")] public static extern uint GetCurrentThreadId();
  [DllImport("user32.dll")] public static extern bool GetKeyboardState(byte[] k);
  [DllImport("user32.dll")] public static extern bool SetKeyboardState(byte[] k);
  [DllImport("user32.dll")] public static extern short VkKeyScanW(char c);
  public static IntPtr LP(int x, int y) { return (IntPtr)((y << 16) | (x & 0xFFFF)); }
}
"@
}
function P-Win { CT-Win }
function P-Post($m, $w, $l) { [void][PM]::PostMessageW((CT-Win), [uint32]$m, [IntPtr]$w, [IntPtr]$l) }
function P-Focus { P-Post 0x06 1 0; P-Post 0x07 0 0; P-Post 0x86 1 0 }   # WM_ACTIVATE, WM_SETFOCUS, WM_NCACTIVATE
function P-Move([int]$x, [int]$y) { P-Post 0x200 0 ([PM]::LP($x, $y)); Start-Sleep -Milliseconds 120 }
function PC([int]$x, [int]$y, [string]$btn = 'left', [int]$n = 1) {
  P-Move $x $y
  $d, $u, $w = if ($btn -eq 'right') { 0x204, 0x205, 2 } else { 0x201, 0x202, 1 }
  1..$n | ForEach-Object { P-Post $d $w ([PM]::LP($x, $y)); Start-Sleep -Milliseconds 70; P-Post $u 0 ([PM]::LP($x, $y)); Start-Sleep -Milliseconds 120 }
  Start-Sleep -Milliseconds 200
}
function PD([int]$x1, [int]$y1, [int]$x2, [int]$y2) {
  P-Move $x1 $y1; P-Post 0x201 1 ([PM]::LP($x1, $y1)); Start-Sleep -Milliseconds 120
  foreach ($t in 1..14) { P-Post 0x200 1 ([PM]::LP([int]($x1 + ($x2 - $x1) * $t / 14), [int]($y1 + ($y2 - $y1) * $t / 14))); Start-Sleep -Milliseconds 40 }
  Start-Sleep -Milliseconds 150; P-Post 0x202 0 ([PM]::LP($x2, $y2)); Start-Sleep -Milliseconds 300
}
function PW([int]$x, [int]$y, [int]$delta) { P-Move $x $y; $r = Rect (CT-Win); [void][PM]::PostMessageW((CT-Win), 0x20A, [IntPtr](($delta -band 0xFFFF) -shl 16), [PM]::LP($r.L + $x, $r.T + $y)); Start-Sleep -Milliseconds 200 }
function Vk($k) { VkOf $k }
function ScanCode($vk) { [PM]::MapVirtualKeyW([uint32]$vk, 0) }
function KeyMsg($vk, $up) {
  $sc = ScanCode $vk; $ext = if (($vk -ge 0x21 -and $vk -le 0x28) -or $vk -eq 0x2D -or $vk -eq 0x2E) { 1 } else { 0 }
  $l = 1 -bor ($sc -shl 16) -bor ($ext -shl 24); if ($up) { $l = $l -bor (1 -shl 30) -bor (1 -shl 31) }
  P-Post $(if ($up) { 0x101 } else { 0x100 }) $vk $l
}
# Taste/Kombination: PK 'ctrl+n', PK 'enter'. Modifikatoren werden zusaetzlich in den Tastaturzustand des UI-Threads geschrieben (GetKeyState).
function PK([string]$combo) {
  $keys = $combo.ToLower().Split('+'); $vks = @($keys | ForEach-Object { [int](Vk $_) })
  foreach ($v in $vks) { KeyMsg $v $false; Start-Sleep -Milliseconds 40 }
  $last = $vks[-1]
  if ($vks.Count -eq 1 -or $true) { }
  [array]::Reverse($vks); foreach ($v in $vks) { KeyMsg $v $true; Start-Sleep -Milliseconds 40 }
  Start-Sleep -Milliseconds 150
}
function PT([string]$s) {
  foreach ($c in $s.ToCharArray()) {
    $vkw = if ($c -match '[a-zA-Z0-9 ]') { [int][char]$c.ToString().ToUpper() } else { 0xE7 }
    if ($c -eq ' ') { $vkw = 0x20 }
    P-Post 0x100 $vkw ([IntPtr](1 -bor ((ScanCode $vkw) -shl 16))); P-Post 0x102 ([int][char]$c) ([IntPtr](1 -bor ((ScanCode $vkw) -shl 16))); P-Post 0x101 $vkw ([IntPtr](1 -bor ((ScanCode $vkw) -shl 16) -bor (3 -shl 30)))
    Start-Sleep -Milliseconds 25
  }
  Start-Sleep -Milliseconds 150
}
# Screenshot ueber das MCP-Werkzeug `screenshot` (egui-intern gerendert, unabhaengig vom Desktop-Zustand)
function S([string]$name) { $o = "$script:SHOTS\$name.png"; [void](Mcp @(@{ n = 'screenshot'; a = @{}; o = $o })); $o }

# ===== Maus per SendMessage + WM_PAINT (Fenster-Tricks, siehe Befund in clicktest-Bericht) =====
# Hintergrund: Bei gesperrtem Bildschirm schlaegt in winit set_cursor_flags fehl (GetClipCursor), deshalb loest jede WM_MOUSEMOVE
# CursorEntered + TrackMouseEvent aus, und Windows meldet sofort WM_MOUSELEAVE (-> egui PointerGone). Ein Klick gelingt nur, wenn der
# egui-Frame NACH Up und VOR dem Leave laeuft: also Move/Down/Up synchron senden und danach synchron WM_PAINT.
Add-Type -TypeDefinition 'using System;using System.Runtime.InteropServices;public class SM{[DllImport("user32.dll")]public static extern IntPtr SendMessageW(IntPtr h,uint m,IntPtr w,IntPtr l);}' -ErrorAction SilentlyContinue
function SM-Send($m, $w, $l) { [void][SM]::SendMessageW((CT-Win), [uint32]$m, [IntPtr]$w, [IntPtr]$l) }
function SM-Paint { SM-Send 0x000F 0 0; Start-Sleep -Milliseconds 60 }
function SClick([int]$x, [int]$y, [string]$btn = 'left', [int]$n = 1) {
  SM-Send 0x200 0 ([PM]::LP($x, $y)); SM-Paint
  $d, $u, $w = if ($btn -eq 'right') { 0x204, 0x205, 2 } else { 0x201, 0x202, 1 }
  1..$n | ForEach-Object { SM-Send $d $w ([PM]::LP($x, $y)); SM-Paint; SM-Send $u 0 ([PM]::LP($x, $y)); SM-Paint }
  Start-Sleep -Milliseconds 200
}

# Modifikatoren: Zustand im Tastatur-Eingabezustand des UI-Threads setzen (AttachThreadInput + SetKeyboardState), dann Taste posten.
function PK2([string]$combo) {
  $h = CT-Win; $keys = $combo.ToLower().Split('+'); $mods = @($keys | Where-Object { $_ -in 'ctrl', 'shift', 'alt' }); $main = $keys | Where-Object { $_ -notin 'ctrl', 'shift', 'alt' } | Select-Object -First 1
  $pidv = [uint32]0; $tid = [PM]::GetWindowThreadProcessId($h, [ref]$pidv); $me = [PM]::GetCurrentThreadId()
  [void][PM]::AttachThreadInput($me, $tid, $true)
  $st = New-Object byte[] 256; [void][PM]::GetKeyboardState($st)
  $map = @{ ctrl = @(0x11, 0xA2); shift = @(0x10, 0xA0); alt = @(0x12, 0xA4) }
  foreach ($m in $mods) { foreach ($v in $map[$m]) { $st[$v] = 0x80 } }; [void][PM]::SetKeyboardState($st)
  foreach ($m in $mods) { KeyMsg ([int]$script:VK[$m]) $false }
  if ($main) { $v = [int](Vk $main); KeyMsg $v $false; Start-Sleep -Milliseconds 60; KeyMsg $v $true }
  Start-Sleep -Milliseconds 60
  foreach ($m in $mods) { KeyMsg ([int]$script:VK[$m]) $true }
  [void][PM]::GetKeyboardState($st); foreach ($m in $mods) { foreach ($v in $map[$m]) { $st[$v] = 0 } }; [void][PM]::SetKeyboardState($st)
  [void][PM]::AttachThreadInput($me, $tid, $false)
  Start-Sleep -Milliseconds 200
}

function PDot { KeyMsg 0xBE $false; P-Post 0x102 46 ([IntPtr](1 -bor ((ScanCode 0xBE) -shl 16))); KeyMsg 0xBE $true; Start-Sleep -Milliseconds 300 }

# Text tippen mit richtiger Umschalt-/AltGr-Behandlung (VkKeyScan) und Modifikatorzustand im UI-Thread
function PT2([string]$text) {
  $h = CT-Win; $pidv = [uint32]0; $tid = [PM]::GetWindowThreadProcessId($h, [ref]$pidv); $me = [PM]::GetCurrentThreadId()
  [void][PM]::AttachThreadInput($me, $tid, $true); $st = New-Object byte[] 256
  foreach ($c in $text.ToCharArray()) {
    $r = [PM]::VkKeyScanW($c); $vk = $r -band 0xFF; $fl = ($r -shr 8) -band 0xFF
    if ($r -eq -1) { P-Post 0x102 ([int]$c) 1; Start-Sleep -Milliseconds 20; continue }
    [void][PM]::GetKeyboardState($st)
    $st[0x10] = if ($fl -band 1) { 0x80 } else { 0 }; $st[0xA0] = $st[0x10]
    $st[0x11] = if ($fl -band 2) { 0x80 } else { 0 }; $st[0xA2] = $st[0x11]
    $st[0x12] = if ($fl -band 4) { 0x80 } else { 0 }; $st[0xA4] = $st[0x12]
    [void][PM]::SetKeyboardState($st)
    $sc = ScanCode $vk
    P-Post 0x100 $vk ([IntPtr](1 -bor ($sc -shl 16))); P-Post 0x102 ([int]$c) ([IntPtr](1 -bor ($sc -shl 16))); P-Post 0x101 $vk ([IntPtr](1 -bor ($sc -shl 16) -bor (3 -shl 30)))
    Start-Sleep -Milliseconds 25
  }
  [void][PM]::GetKeyboardState($st); foreach ($v in 0x10,0xA0,0x11,0xA2,0x12,0xA4) { $st[$v] = 0 }; [void][PM]::SetKeyboardState($st)
  [void][PM]::AttachThreadInput($me, $tid, $false); Start-Sleep -Milliseconds 150
}
