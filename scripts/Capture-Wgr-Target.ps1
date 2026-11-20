param(
    [Parameter(Mandatory=$true)][int]$GameProcessId,
    [Parameter(Mandatory=$true)][string]$Output
)
$ErrorActionPreference = 'Stop'
$proc = Get-Process -Id $GameProcessId -ErrorAction Stop
if ($proc.ProcessName -ne 'WGR') { throw 'Somente o processo WGR pode ser capturado.' }
if (Test-Path -LiteralPath $Output) { throw 'Arquivo de saída já existe.' }
Add-Type -AssemblyName System.Drawing
Add-Type -TypeDefinition @'
using System;
using System.Runtime.InteropServices;
public static class WgrTargetPrint {
    [StructLayout(LayoutKind.Sequential)]
    public struct Rect { public int Left, Top, Right, Bottom; }
    [DllImport("user32.dll")] public static extern bool GetWindowRect(IntPtr hwnd, out Rect rect);
    [DllImport("user32.dll")] public static extern bool PrintWindow(IntPtr hwnd, IntPtr hdc, uint flags);
}
'@
$hwnd = $proc.MainWindowHandle
$rect = [WgrTargetPrint+Rect]::new()
if ($hwnd -eq [IntPtr]::Zero -or -not [WgrTargetPrint]::GetWindowRect($hwnd, [ref]$rect)) {
    throw 'A janela do WGR não está disponível.'
}
$width = $rect.Right - $rect.Left
$height = $rect.Bottom - $rect.Top
if ($width -lt 200 -or $height -lt 200 -or $width -gt 7680 -or $height -gt 4320) {
    throw 'Dimensões inesperadas.'
}
# PrintWindow renders only the specified WGR HWND into our bitmap. It does
# not read the desktop, unrelated foreground windows or screen pixels.
$bitmap = [System.Drawing.Bitmap]::new($width, $height)
$graphics = [System.Drawing.Graphics]::FromImage($bitmap)
try {
    $dc = $graphics.GetHdc()
    try { $ok = [WgrTargetPrint]::PrintWindow($hwnd, $dc, 2) }
    finally { $graphics.ReleaseHdc($dc) }
    if (-not $ok) { throw 'PrintWindow não conseguiu capturar o HWND.' }
    $bitmap.Save($Output, [System.Drawing.Imaging.ImageFormat]::Png)
    [pscustomobject]@{ file=(Resolve-Path $Output).Path; pixels="$width x $height"; target_pid=$GameProcessId; method='target HWND only' } | ConvertTo-Json
} finally {
    $graphics.Dispose()
    $bitmap.Dispose()
}
