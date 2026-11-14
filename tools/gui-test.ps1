# GUI-Testhilfen für Paneldeck (Windows PowerShell 5.1). Dot-source: . tools\gui-test.ps1
# Fenster finden (Get-HubWin <PID>), nach vorn holen (Focus/Top), Screenshot auch bei verdecktem Fenster (ShotPW),
# Tastatur (Press 'ctrl+shift+t', TypeText), Maus (Click, Drag, Wheel, MoveAbs).
# WICHTIG: Während gesteuert wird, darf der Nutzer weder Maus noch Tastatur benutzen und kein Fenster über den Hub legen.
# Neue Features immer zusätzlich zu tools\smoke-mcp.ps1 (MCP) auch so per Maus/Tastatur prüfen.
# Win32-Helfer für GUI-Tests (dot-source: . .\w.ps1)
Add-Type -AssemblyName System.Drawing, System.Windows.Forms
if (-not ([System.Management.Automation.PSTypeName]'W32').Type) {
Add-Type -TypeDefinition @"
using System; using System.Runtime.InteropServices; using System.Text; using System.Collections.Generic;
public class W32 {
  [DllImport("user32.dll")] public static extern bool SetProcessDPIAware();
  [DllImport("user32.dll")] public static extern bool SetForegroundWindow(IntPtr h);
  [DllImport("user32.dll")] public static extern bool ShowWindow(IntPtr h, int c);
  [DllImport("user32.dll")] public static extern bool GetWindowRect(IntPtr h, out RECT r);
  [DllImport("user32.dll")] public static extern bool MoveWindow(IntPtr h, int x, int y, int w, int hh, bool r);
  [DllImport("user32.dll")] public static extern bool SetCursorPos(int x, int y);
  [DllImport("user32.dll")] public static extern void mouse_event(uint f, int dx, int dy, int d, UIntPtr e);
  [DllImport("user32.dll")] public static extern void keybd_event(byte vk, byte sc, uint f, UIntPtr e);
  [DllImport("user32.dll")] public static extern uint SendInput(uint n, INPUT[] i, int size);
  [DllImport("user32.dll")] public static extern bool IsWindowVisible(IntPtr h);
  [DllImport("user32.dll")] public static extern bool PrintWindow(IntPtr h, IntPtr hdc, uint flags);
  [DllImport("user32.dll", CharSet=CharSet.Unicode)] public static extern int GetWindowTextW(IntPtr h, StringBuilder s, int n);
  [DllImport("user32.dll")] public static extern uint GetWindowThreadProcessId(IntPtr h, out uint pid);
  [DllImport("user32.dll")] public static extern bool EnumWindows(EnumProc p, IntPtr l);
  public delegate bool EnumProc(IntPtr h, IntPtr l);
  [StructLayout(LayoutKind.Sequential)] public struct RECT { public int L, T, R, B; }
  [StructLayout(LayoutKind.Sequential)] public struct INPUT { public uint type; public KI ki; public long pad; }
  [StructLayout(LayoutKind.Sequential)] public struct KI { public ushort vk, sc; public uint fl, time; public IntPtr extra; }
  public static List<IntPtr> WindowsOf(uint pid) {
    var l = new List<IntPtr>();
    EnumWindows((h, _) => { uint p; GetWindowThreadProcessId(h, out p); if (p == pid && IsWindowVisible(h)) l.Add(h); return true; }, IntPtr.Zero);
    return l;
  }
  public static string Title(IntPtr h) { var s = new StringBuilder(256); GetWindowTextW(h, s, 256); return s.ToString(); }
  public static void Key(ushort vk, bool up) {
    var i = new INPUT[1]; i[0].type = 1; i[0].ki.vk = vk; i[0].ki.fl = up ? 2u : 0u;
    // erweiterte Tasten (Pfeile, Einfg, Entf, Pos1, Ende, Bild)
    if ((vk >= 0x21 && vk <= 0x28) || vk == 0x2D || vk == 0x2E) i[0].ki.fl |= 1u;
    SendInput(1, i, Marshal.SizeOf(typeof(INPUT)));
  }
  public static void Uni(char c) {
    var i = new INPUT[2];
    i[0].type = 1; i[0].ki.sc = c; i[0].ki.fl = 4; i[1].type = 1; i[1].ki.sc = c; i[1].ki.fl = 6;
    SendInput(2, i, Marshal.SizeOf(typeof(INPUT)));
  }
}
"@
}
[void][W32]::SetProcessDPIAware()
$script:VK = @{ ctrl=0x11; shift=0x10; alt=0x12; enter=0x0D; esc=0x1B; tab=0x09; back=0x08; del=0x2E; ins=0x2D; left=0x25; up=0x26; right=0x27; down=0x28; home=0x24; end=0x23; f2=0x71; f5=0x74; win=0x5B; space=0x20 }
function Get-HubWin($procId) { foreach ($h in [W32]::WindowsOf([uint32]$procId)) { if ([W32]::Title($h) -match 'Paneldeck') { return $h } } }
function Focus($h) { [void][W32]::ShowWindow($h, 9); [void][W32]::SetForegroundWindow($h); Start-Sleep -Milliseconds 250 }
function Rect($h) { $r = New-Object W32+RECT; [void][W32]::GetWindowRect($h, [ref]$r); $r }
function Shot($h, $path) {
  $r = Rect $h; $w = $r.R - $r.L; $hh = $r.B - $r.T
  $bmp = New-Object System.Drawing.Bitmap $w, $hh; $g = [System.Drawing.Graphics]::FromImage($bmp)
  $g.CopyFromScreen($r.L, $r.T, 0, 0, $bmp.Size); $g.Dispose(); $bmp.Save($path); $bmp.Dispose()
}
function VkOf($k) { if ($script:VK.ContainsKey($k)) { [uint16]$script:VK[$k] } elseif ($k.Length -eq 1) { [uint16][char]$k.ToUpper() } else { throw "Taste? $k" } }
# Press 'ctrl+shift+t' / 'enter' / 'a'
function Press($combo) {
  $keys = $combo.ToLower().Split('+'); $vks = $keys | ForEach-Object { VkOf $_ }
  foreach ($v in $vks) { [W32]::Key($v, $false); Start-Sleep -Milliseconds 30 }
  [array]::Reverse($vks); foreach ($v in $vks) { [W32]::Key($v, $true); Start-Sleep -Milliseconds 30 }
  Start-Sleep -Milliseconds 120
}
function TypeText($s) { foreach ($c in $s.ToCharArray()) { [W32]::Uni($c); Start-Sleep -Milliseconds 15 }; Start-Sleep -Milliseconds 100 }
function Click($x, $y, $btn = 'left', [int]$n = 1) {
  [void][W32]::SetCursorPos($x, $y); Start-Sleep -Milliseconds 80
  $d, $u = if ($btn -eq 'right') { 0x8, 0x10 } else { 0x2, 0x4 }
  1..$n | ForEach-Object { [W32]::mouse_event($d, 0, 0, 0, [UIntPtr]::Zero); [W32]::mouse_event($u, 0, 0, 0, [UIntPtr]::Zero); Start-Sleep -Milliseconds 60 }
  Start-Sleep -Milliseconds 150
}
function Wheel($x, $y, $delta) { [void][W32]::SetCursorPos($x, $y); [W32]::mouse_event(0x800, 0, 0, $delta, [UIntPtr]::Zero); Start-Sleep -Milliseconds 150 }
function Drag($x1, $y1, $x2, $y2) {
  [void][W32]::SetCursorPos($x1, $y1); Start-Sleep -Milliseconds 80; [W32]::mouse_event(0x2, 0, 0, 0, [UIntPtr]::Zero)
  foreach ($t in 1..12) { [void][W32]::SetCursorPos([int]($x1 + ($x2 - $x1) * $t / 12), [int]($y1 + ($y2 - $y1) * $t / 12)); Start-Sleep -Milliseconds 25 }
  [W32]::mouse_event(0x4, 0, 0, 0, [UIntPtr]::Zero); Start-Sleep -Milliseconds 200
}

# Screenshot per PrintWindow: funktioniert auch, wenn das Fenster verdeckt ist
function ShotPW($h, $path) {
  $r = Rect $h; $w = $r.R - $r.L; $hh = $r.B - $r.T
  $bmp = New-Object System.Drawing.Bitmap $w, $hh; $g = [System.Drawing.Graphics]::FromImage($bmp); $hdc = $g.GetHdc()
  [void][W32]::PrintWindow($h, $hdc, 2); $g.ReleaseHdc($hdc); $g.Dispose(); $bmp.Save($path); $bmp.Dispose()
}

# --- Ergänzungen: Fenster nach ganz oben, absolute Mausbewegung (für OLE-Drag & Drop) ---
Add-Type -MemberDefinition '[DllImport("user32.dll")] public static extern bool SetWindowPos(IntPtr h, IntPtr after, int x, int y, int cx, int cy, uint f);' -Name U4 -Namespace X -ErrorAction SilentlyContinue
# Fenster dauerhaft vor alle anderen (sonst landen Klicks auf dem Fenster des Nutzers!). Danach Untop.
function Top($h) { [void][X.U4]::SetWindowPos($h,[IntPtr](-1),0,0,0,0,0x53); Start-Sleep -Milliseconds 300 }
function Untop($h) { [void][X.U4]::SetWindowPos($h,[IntPtr](-2),0,0,0,0,0x53) }
# Absolute Mausbewegung (Bildschirmgröße anpassen!). Nötig, damit der Explorer einen echten Drag beginnt (SetCursorPos genügt nicht).
function MoveAbs($x,$y,$sw=3440,$sh=1440) { [W32]::mouse_event(0x8001, [int]($x*65535/$sw), [int]($y*65535/$sh), 0, [UIntPtr]::Zero) }