param(
  [string]$Exe = "D:\Serenity\project\i\rust\proxy-forward\target\release\devroute.exe",
  [string]$OutDir = "D:\Serenity\project\i\rust\proxy-forward\docs"
)
$ErrorActionPreference = "Stop"
Add-Type -AssemblyName System.Drawing
Add-Type @"
using System;
using System.Runtime.InteropServices;
public class Win {
  [DllImport("user32.dll")] public static extern bool SetProcessDPIAware();
  [DllImport("user32.dll")] public static extern bool SetForegroundWindow(IntPtr h);
  [StructLayout(LayoutKind.Sequential)] public struct RECT { public int Left, Top, Right, Bottom; }
  [DllImport("user32.dll")] public static extern bool GetWindowRect(IntPtr h, out RECT r);
}
"@
[Win]::SetProcessDPIAware() | Out-Null

function Shot([string]$theme, [string]$lang, [string]$outFile) {
  $work = Join-Path $env:TEMP ("devroute_shot_" + [guid]::NewGuid().ToString("N"))
  New-Item -ItemType Directory -Force $work | Out-Null
  Set-Content -Encoding ascii (Join-Path $work "config.toml") @"
listen_addr = "127.0.0.1:1080"
theme = "$theme"
language = "$lang"
auto_proxy = false

[[rules]]
matcher = { addr = "192.168.120.177:81", path_prefix = "/api" }
forward = { addr = "127.0.0.1:8686", path_prefix = "" }

[[rules]]
matcher = { addr = "192.168.120.181:81", path_prefix = "/api" }
forward = { addr = "127.0.0.1:8886", path_prefix = "" }
"@
  $p = Start-Process -FilePath $Exe -WorkingDirectory $work -PassThru
  $h = [IntPtr]::Zero
  for ($i = 0; $i -lt 50; $i++) {
    Start-Sleep -Milliseconds 200
    $p.Refresh()
    if ($p.MainWindowHandle -ne 0) { $h = $p.MainWindowHandle; break }
  }
  if ($h -eq [IntPtr]::Zero) { throw "window not found ($theme/$lang)" }
  [Win]::SetForegroundWindow($h) | Out-Null
  Start-Sleep -Milliseconds 800
  $r = New-Object Win+RECT
  if (-not [Win]::GetWindowRect($h, [ref]$r)) { throw "GetWindowRect failed" }
  $w = $r.Right - $r.Left; $ht = $r.Bottom - $r.Top
  if ($w -le 0 -or $ht -le 0) { throw "bad rect $w x $ht" }
  $bmp = New-Object System.Drawing.Bitmap($w, $ht)
  $g = [System.Drawing.Graphics]::FromImage($bmp)
  $g.CopyFromScreen($r.Left, $r.Top, 0, 0, $bmp.Size)
  $bmp.Save($outFile, [System.Drawing.Imaging.ImageFormat]::Png)
  $g.Dispose(); $bmp.Dispose()
  Stop-Process -Id $p.Id -Force
  $p.WaitForExit(5000) | Out-Null
  try { Remove-Item -Recurse -Force $work -ErrorAction Stop } catch { Write-Output "(cleanup skipped: $($_.Exception.Message))" }
  Write-Output "saved $outFile ($w x $ht)"
  Start-Sleep -Milliseconds 600
}

Shot "light" "zh" (Join-Path $OutDir "screenshot-light.png")
Shot "dark"  "zh" (Join-Path $OutDir "screenshot-dark.png")
Shot "light" "en" (Join-Path $OutDir "screenshot-light-en.png")
Shot "dark"  "en" (Join-Path $OutDir "screenshot-dark-en.png")
