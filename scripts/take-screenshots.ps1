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
  [DllImport("user32.dll")] public static extern bool PrintWindow(IntPtr h, IntPtr hdc, uint flags);
  [StructLayout(LayoutKind.Sequential)] public struct RECT { public int Left, Top, Right, Bottom; }
  [DllImport("user32.dll")] public static extern bool GetWindowRect(IntPtr h, out RECT r);
}
"@
[Win]::SetProcessDPIAware() | Out-Null
New-Item -ItemType Directory -Force $OutDir | Out-Null

function Shot([string]$theme, [string]$lang, [int]$tab, [string]$outFile, [string]$popup = "") {
  $work = Join-Path $env:TEMP ("devroute_shot_" + [guid]::NewGuid().ToString("N"))
  New-Item -ItemType Directory -Force $work | Out-Null
  Set-Content -Encoding ascii (Join-Path $work "config.toml") @"
proxy_listen = "127.0.0.1:1080"
proxy_enable = true
theme = "$theme"
language = "$lang"
auto_proxy = false

[[sites]]
enable = true
name = "192.168.120.177"
listen_addr = "127.0.0.1:8088"
upstream = "http://192.168.120.177:81"
insecure = false

[[sites.rules]]
path_prefix = "/api"
forward_addr = "127.0.0.1:8686"
forward_prefix = ""

[[sites.rules]]
path_prefix = "/upload"
forward_addr = "127.0.0.1:8686"
forward_prefix = "/v1"

[[sites]]
enable = false
name = "192.168.120.181"
listen_addr = "127.0.0.1:8089"
upstream = "https://192.168.120.181:81"
insecure = true

[[sites.rules]]
path_prefix = "/api"
forward_addr = "127.0.0.1:8886"
forward_prefix = ""

[[proxy_rules]]
matcher = { addr = "192.168.120.177:81", path_prefix = "/api" }
forward = { addr = "127.0.0.1:8686", path_prefix = "" }

[[proxy_rules]]
matcher = { addr = "192.168.120.181:81", path_prefix = "/api" }
forward = { addr = "127.0.0.1:8886", path_prefix = "" }
"@
  $env:DEVROUTE_UI_TAB = "$tab"
  # 传站点序号就让程序启动即展开该站点的规则弹窗（截图用，普通运行不读这个变量）
  $env:DEVROUTE_UI_POPUP = $popup
  $p = Start-Process -FilePath $Exe -WorkingDirectory $work -PassThru
  $h = [IntPtr]::Zero
  for ($i = 0; $i -lt 50; $i++) {
    Start-Sleep -Milliseconds 200
    $p.Refresh()
    if ($p.MainWindowHandle -ne 0) { $h = $p.MainWindowHandle; break }
  }
  if ($h -eq [IntPtr]::Zero) { throw "window not found ($theme/$lang)" }
  [Win]::SetForegroundWindow($h) | Out-Null
  # 展开弹窗的那几张要等弹窗自己的重绘兜底跑一轮，否则可能拍到半张卡片
  if ($popup -ne "") { Start-Sleep -Milliseconds 1200 } else { Start-Sleep -Milliseconds 800 }
  $r = New-Object Win+RECT
  if (-not [Win]::GetWindowRect($h, [ref]$r)) { throw "GetWindowRect failed" }
  $w = $r.Right - $r.Left; $ht = $r.Bottom - $r.Top
  if ($w -le 0 -or $ht -le 0) { throw "bad rect $w x $ht" }
  $bmp = New-Object System.Drawing.Bitmap($w, $ht)
  $g = [System.Drawing.Graphics]::FromImage($bmp)
  # PrintWindow(PW_RENDERFULLCONTENT) 抓窗口自身内容：桌面上有别的窗口压着也拍不到别人的界面，
  # 也不会把窗口外那 1px 隐形边框里的桌面像素混进来
  $hdc = $g.GetHdc()
  $ok = [Win]::PrintWindow($h, $hdc, 2)
  $g.ReleaseHdc($hdc)
  if ($ok) {
    $g.Dispose()
  } else {
    $g.CopyFromScreen($r.Left, $r.Top, 0, 0, $bmp.Size)
    $g.Dispose()
    Write-Output "(fallback: CopyFromScreen $theme/$lang)"
  }
  $bmp.Save($outFile, [System.Drawing.Imaging.ImageFormat]::Png)
  $bmp.Dispose()
  Stop-Process -Id $p.Id -Force
  $p.WaitForExit(5000) | Out-Null
  try { Remove-Item -Recurse -Force $work -ErrorAction Stop } catch { Write-Output "(cleanup skipped: $($_.Exception.Message))" }
  Write-Output "saved $outFile ($w x $ht)"
  Start-Sleep -Milliseconds 600
}

# 0 = 本地镜像页（默认页） 1 = 代理模式页
Shot "light" "zh" 0 (Join-Path $OutDir "screenshot-sites-light.png")
Shot "dark"  "zh" 0 (Join-Path $OutDir "screenshot-sites-dark.png")
Shot "light" "en" 0 (Join-Path $OutDir "screenshot-sites-light-en.png")
Shot "dark"  "en" 0 (Join-Path $OutDir "screenshot-sites-dark-en.png")
Shot "light" "zh" 1 (Join-Path $OutDir "screenshot-light.png")
Shot "dark"  "zh" 1 (Join-Path $OutDir "screenshot-dark.png")
Shot "light" "en" 1 (Join-Path $OutDir "screenshot-light-en.png")
Shot "dark"  "en" 1 (Join-Path $OutDir "screenshot-dark-en.png")
# 站点规则弹窗（第 5 个参数 = 展开第 0 个站点的规则）
Shot "light" "zh" 0 (Join-Path $OutDir "screenshot-dialog-light.png") "0"
Shot "dark"  "zh" 0 (Join-Path $OutDir "screenshot-dialog-dark.png") "0"
