param([switch]$SkipSmoke, [switch]$KeepBuild)
$ErrorActionPreference = 'Stop'
$project = (Resolve-Path (Join-Path $PSScriptRoot '..')).Path
# Keep large intermediates out of OneDrive; one release profile and one compiler.
$build = [IO.Path]::GetFullPath((Join-Path $env:TEMP ('wgr-validation-' + [guid]::NewGuid().ToString('N'))))
$tempRoot = [IO.Path]::GetFullPath($env:TEMP).TrimEnd('\') + '\'
if (-not $build.StartsWith($tempRoot, [StringComparison]::OrdinalIgnoreCase)) { throw 'Pasta temporária inválida.' }
Push-Location $project
try {
    New-Item -ItemType Directory -Path artifacts -Force | Out-Null
    cargo fmt --all -- --check *> artifacts/fmt.log
    if ($LASTEXITCODE) { throw 'Rustfmt falhou; consulte artifacts/fmt.log.' }
    node tests/ram-io.mjs *> artifacts/ram-io.json
    if ($LASTEXITCODE) { throw 'Teste IO em memória falhou.' }
    node tests/session-observer.mjs *> artifacts/session-observer.json
    if ($LASTEXITCODE) { throw 'Teste de preparação da engine falhou.' }
    cargo clippy --release --workspace --all-targets --locked --jobs 1 --target-dir $build -- -D warnings *> artifacts/clippy.log
    if ($LASTEXITCODE) { throw 'Clippy falhou; consulte artifacts/clippy.log.' }
    cargo test --release --workspace --locked --jobs 1 --target-dir $build *> artifacts/tests.log
    if ($LASTEXITCODE) { throw 'Testes falharam; consulte artifacts/tests.log.' }
    cargo build --release --locked --jobs 1 --target-dir $build *> artifacts/release.log
    if ($LASTEXITCODE) { throw 'Build release falhou; consulte artifacts/release.log.' }
    $exe = Join-Path $build 'release/WGR.exe'
    & (Join-Path $PSScriptRoot 'Build-NativeRage.ps1') -TargetDir $build *> artifacts/native-rage-build.log
    if (-not $SkipSmoke) { & (Join-Path $PSScriptRoot 'Smoke.ps1') -Executable $exe }
    Copy-Item -LiteralPath $exe -Destination (Join-Path $project 'WGR.exe') -Force
    Copy-Item -LiteralPath (Join-Path $build 'release/wgr-rage.exe') -Destination (Join-Path $project 'wgr-rage.exe') -Force
    Write-Output 'Validação concluída. Executável: WGR.exe. Logs em artifacts/.'
} finally {
    if (-not $KeepBuild -and (Test-Path -LiteralPath $build)) {
        cargo clean --target-dir $build *> artifacts/build-cleanup.log
    }
    Pop-Location
}
