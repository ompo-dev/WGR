param([switch]$SkipBuild, [string]$BuildDir, [string]$DistDir)
$ErrorActionPreference = 'Stop'
$project = (Resolve-Path (Join-Path $PSScriptRoot '..')).Path
Push-Location $project
try {
    if (-not $SkipBuild) {
        & (Join-Path $PSScriptRoot 'Validate.ps1') -SkipSmoke
    }
    $dist = if ($DistDir) { [IO.Path]::GetFullPath($DistDir) } else { Join-Path $project 'dist' }
    New-Item -ItemType Directory -Path $dist -Force | Out-Null
    $baseName = 'WGR-0.1.0-win-x64'
    $output = Join-Path $dist $baseName
    if (Test-Path -LiteralPath $output) { $baseName += '-' + (Get-Date -Format 'yyyyMMdd-HHmmss'); $output = Join-Path $dist $baseName }
    New-Item -ItemType Directory -Path $output -Force | Out-Null
    # Both executables must come from the same validated build directory.
    # Never mix a stale target launcher with a newer root runtime.
    $binDir=if($BuildDir){Join-Path ([IO.Path]::GetFullPath($BuildDir)) 'release'}else{$project}
    $binary=Join-Path $binDir 'WGR.exe'
    if(-not(Test-Path -LiteralPath $binary)){throw 'Executável validado não encontrado.'}
    Copy-Item -LiteralPath $binary -Destination $output
    $native=Join-Path $binDir 'wgr-rage.exe'
    if(-not(Test-Path -LiteralPath $native)){throw 'Runtime nativo validado não encontrado.'}
    Copy-Item -LiteralPath $native -Destination $output
    foreach ($item in @('README.md','LICENSE','THIRD_PARTY_NOTICES.md','docs')) { Copy-Item -LiteralPath $item -Destination $output -Recurse }
    New-Item -ItemType Directory -Path (Join-Path $output 'licenses') -Force | Out-Null
    Copy-Item -LiteralPath assets/licenses -Destination (Join-Path $output 'licenses/third-party') -Recurse
    & (Join-Path $PSScriptRoot 'Collect-Licenses.ps1') -OutputRoot $output
    Copy-Item -LiteralPath assets/momor/icons/LICENSES -Destination (Join-Path $output 'licenses/Momor-Lucide-ISC-MIT.txt')
    [IO.File]::WriteAllText((Join-Path $output 'portable.flag'), 'WGR portable')
    [IO.File]::WriteAllText((Join-Path $output 'BUILD.txt'), "WGR 0.1.0`r`nTarget: x86_64-pc-windows-msvc`r`nRust: 1.98.1`r`nBuild: " + (git rev-parse HEAD 2>$null))
    $sourceZip = Join-Path $output 'WGR-source.zip'
    Add-Type -AssemblyName System.IO.Compression
    Add-Type -AssemblyName System.IO.Compression.FileSystem
    $archive = [IO.Compression.ZipFile]::Open($sourceZip, [IO.Compression.ZipArchiveMode]::Create)
    try {
        foreach ($relative in (git ls-files | Select-Object -Unique)) {
            $path = Join-Path $project $relative
            if ((Test-Path -LiteralPath $path -PathType Leaf) -and $relative -notmatch '^artifacts/') { [IO.Compression.ZipFileExtensions]::CreateEntryFromFile($archive,$path,$relative,[IO.Compression.CompressionLevel]::Optimal) | Out-Null }
        }
    } finally { $archive.Dispose() }
    $zip = Join-Path $dist ($baseName + '.zip')
    [IO.Compression.ZipFile]::CreateFromDirectory($output,$zip,[IO.Compression.CompressionLevel]::Optimal,$true)
    Get-FileHash -LiteralPath (Join-Path $output 'WGR.exe'),(Join-Path $output 'wgr-rage.exe'),$zip -Algorithm SHA256 | ForEach-Object { "$($_.Hash.ToLower())  $([IO.Path]::GetFileName($_.Path))" } | Set-Content -LiteralPath (Join-Path $dist ($baseName + '.sha256')) -Encoding ascii
    & (Join-Path $PSScriptRoot 'Verify-Package.ps1') -Zip $zip
    Write-Output "Executável: $(Join-Path $output 'WGR.exe')"
    Write-Output "Portable ZIP: $zip"
} finally { Pop-Location }
