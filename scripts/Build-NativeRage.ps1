param([Parameter(Mandatory=$true)][string]$TargetDir)
$ErrorActionPreference='Stop'
$project=(Resolve-Path (Join-Path $PSScriptRoot '..')).Path
$build=[IO.Path]::GetFullPath($TargetDir)
if((Get-PSDrive C).Free -lt 8GB){throw 'São necessários 8 GiB livres para compilar sem esgotar o disco.'}
$previousPolicy=$env:CMAKE_POLICY_VERSION_MINIMUM
try {
    # Opus bundled by the upstream requires CMake's documented legacy policy setting.
    $env:CMAKE_POLICY_VERSION_MINIMUM='3.5'
    cargo build --manifest-path (Join-Path $project 'vendor/muguet/Cargo.toml') --bin wgr-rage --release --locked --jobs 2 --target-dir $build
    if($LASTEXITCODE){throw 'Compilação do runtime nativo falhou.'}
} finally {$env:CMAKE_POLICY_VERSION_MINIMUM=$previousPolicy}
