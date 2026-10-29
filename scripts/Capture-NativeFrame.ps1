param(
    [Parameter(Mandatory=$true)][int]$GameProcessId,
    [Parameter(Mandatory=$true)][string]$NativeLog,
    [Parameter(Mandatory=$true)][ValidatePattern('^[a-zA-Z0-9_-]{1,48}$')][string]$Name
)
$ErrorActionPreference = 'Stop'
$project = (Resolve-Path (Join-Path $PSScriptRoot '..')).Path
$root = Get-Process -Id $GameProcessId
if ($root.Path -ne (Join-Path $project 'WGR.exe')) { throw 'Janela fora do WGR deste projeto.' }
$line = Get-Content -LiteralPath $NativeLog | Where-Object { $_ -match '"type":"native_window"' } | Select-Object -First 1
if (-not $line) { throw 'Janela nativa ausente do registro.' }
$event = $line.Substring(10) | ConvertFrom-Json
$native = Get-CimInstance Win32_Process -Filter "ProcessId=$($event.pid)"
if ($native.ExecutablePath -ne (Join-Path $project 'wgr-rage.exe') -or $native.ParentProcessId -ne $GameProcessId) { throw 'Engine fora do processo solicitado.' }
Add-Type -AssemblyName System.Drawing
Add-Type -TypeDefinition 'using System;using System.Runtime.InteropServices;public static class WgrSnapshot{[StructLayout(LayoutKind.Sequential)]public struct Rect{public int L,T,R,B;}[DllImport("user32.dll")]public static extern bool GetClientRect(IntPtr window,out Rect rect);[DllImport("user32.dll")]public static extern uint GetWindowThreadProcessId(IntPtr window,out uint process);[DllImport("user32.dll")]public static extern bool IsChild(IntPtr parent,IntPtr child);[DllImport("user32.dll")]public static extern bool PrintWindow(IntPtr window,IntPtr dc,uint flags);}'
$window = [IntPtr]([long]$event.hwnd)
$owner = 0u
[void][WgrSnapshot]::GetWindowThreadProcessId($window,[ref]$owner)
if ($owner -ne $event.pid -or -not [WgrSnapshot]::IsChild($root.MainWindowHandle,$window)) { throw 'A janela não pertence à engine.' }
$rect = [WgrSnapshot+Rect]::new()
if (-not [WgrSnapshot]::GetClientRect($window,[ref]$rect) -or $rect.R -le 0 -or $rect.B -le 0) { throw 'Janela sem dimensões válidas.' }
$output = Join-Path $project "artifacts/$Name.png"
if (Test-Path -LiteralPath $output) { throw 'Use outro nome para preservar a imagem.' }
$bitmap = [Drawing.Bitmap]::new($rect.R,$rect.B)
$graphics = [Drawing.Graphics]::FromImage($bitmap)
try {
    $dc = $graphics.GetHdc()
    try { $ok = [WgrSnapshot]::PrintWindow($window,$dc,2) } finally { $graphics.ReleaseHdc($dc) }
    if (-not $ok) { throw 'Falha ao capturar a janela da engine.' }
    $bitmap.Save($output)
} finally { $graphics.Dispose(); $bitmap.Dispose() }
[pscustomobject]@{file=$output;width=$rect.R;height=$rect.B;native_pid=$event.pid} | ConvertTo-Json
