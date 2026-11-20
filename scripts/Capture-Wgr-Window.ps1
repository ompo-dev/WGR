param(
    [Parameter(Mandatory=$true)][int]$GameProcessId,
    [Parameter(Mandatory=$true)][string]$Output,
    [switch]$BringToFront
)
$ErrorActionPreference = 'Stop'
$proc = Get-Process -Id $GameProcessId -ErrorAction Stop
if ($proc.ProcessName -ne 'WGR') { throw 'Capture only the verified WGR process.' }
if (Test-Path -LiteralPath $Output) { throw 'Screenshot already exists; choose a fresh filename.' }
Add-Type -AssemblyName System.Drawing
Add-Type -TypeDefinition @'
using System;
using System.Runtime.InteropServices;
public static class WgrWindowCapture {
    [StructLayout(LayoutKind.Sequential)]
    public struct Rect { public int Left, Top, Right, Bottom; }
    [DllImport("user32.dll")] public static extern bool GetWindowRect(IntPtr hwnd, out Rect rect);
    [DllImport("user32.dll")] public static extern bool IsWindowVisible(IntPtr hwnd);
    [DllImport("user32.dll")] public static extern bool IsIconic(IntPtr hwnd);
    [DllImport("user32.dll")] public static extern bool SetForegroundWindow(IntPtr hwnd);
    [DllImport("user32.dll")] public static extern IntPtr GetForegroundWindow();
    [DllImport("user32.dll")] public static extern uint GetWindowThreadProcessId(IntPtr hwnd, out uint processId);
}
'@
$hwnd = $proc.MainWindowHandle
if ($hwnd -eq [IntPtr]::Zero -or -not [WgrWindowCapture]::IsWindowVisible($hwnd) -or [WgrWindowCapture]::IsIconic($hwnd)) {
    throw 'WGR window unavailable, hidden or minimized.'
}
if ($BringToFront) { [void][WgrWindowCapture]::SetForegroundWindow($hwnd); Start-Sleep -Milliseconds 450 }
$active = [WgrWindowCapture]::GetForegroundWindow()
[uint32]$owner = 0
[void][WgrWindowCapture]::GetWindowThreadProcessId($active, [ref]$owner)
$child = @(Get-CimInstance Win32_Process -Filter "Name='wgr-rage.exe'" | Where-Object { $_.ParentProcessId -eq $GameProcessId } | ForEach-Object { [uint32]$_.ProcessId })
if ($owner -ne $GameProcessId -and $owner -notin $child) {
    throw "WGR is occluded by another foreground window (owner PID $owner); refusing to capture potentially private unrelated screen content."
}
$rect = [WgrWindowCapture+Rect]::new()
if (-not [WgrWindowCapture]::GetWindowRect($hwnd, [ref]$rect)) { throw 'GetWindowRect failed' }
$w = $rect.Right - $rect.Left
$h = $rect.Bottom - $rect.Top
if ($w -lt 200 -or $h -lt 200 -or $w -gt 7680 -or $h -gt 4320) { throw 'Unexpected WGR window dimensions.' }
$image = [System.Drawing.Bitmap]::new($w, $h)
$graphics = [System.Drawing.Graphics]::FromImage($image)
try {
    $graphics.CopyFromScreen($rect.Left, $rect.Top, 0, 0, [System.Drawing.Size]::new($w, $h))
    $image.Save($Output, [System.Drawing.Imaging.ImageFormat]::Png)
} finally {
    $graphics.Dispose()
    $image.Dispose()
}
$file = Get-Item -LiteralPath $Output
[pscustomobject]@{
    file = $file.FullName
    size_bytes = $file.Length
    pixels = "$w x $h"
    captured_at = (Get-Date).ToString('o')
    process_id = $GameProcessId
} | ConvertTo-Json
