param([switch]$SkipSmoke, [switch]$KeepBuild, [switch]$SkipPublish, [string]$TargetDir)
$ErrorActionPreference = 'Stop'
$project = (Resolve-Path (Join-Path $PSScriptRoot '..')).Path
$previousPath = $env:PATH
if (-not (Get-Command cargo -ErrorAction SilentlyContinue)) {
    $toolchain = Get-Content -LiteralPath (Join-Path $project 'rust-toolchain.toml') -Raw
    if ($toolchain -notmatch 'channel\s*=\s*"([^"]+)"') { throw 'Versão do toolchain Rust ausente.' }
    $rustup = if ($env:RUSTUP_HOME) { $env:RUSTUP_HOME } else { Join-Path $HOME '.rustup' }
    $bin = Join-Path $rustup "toolchains/$($Matches[1])-x86_64-pc-windows-msvc/bin"
    if (-not (Test-Path -LiteralPath (Join-Path $bin 'cargo.exe'))) { throw 'Cargo ausente. Instale o toolchain definido em rust-toolchain.toml.' }
    $env:PATH = "$bin;$env:PATH"
}
# Keep large intermediates out of OneDrive; one release profile and one compiler.
$ownsBuild = -not $TargetDir
$build = if ($TargetDir) { [IO.Path]::GetFullPath($TargetDir) } else { [IO.Path]::GetFullPath((Join-Path $env:TEMP ('opencode/wgr-validation-' + [guid]::NewGuid().ToString('N')))) }
$tempRoot = [IO.Path]::GetFullPath($env:TEMP).TrimEnd('\') + '\'
if ($ownsBuild -and -not $build.StartsWith($tempRoot, [StringComparison]::OrdinalIgnoreCase)) { throw 'Pasta temporária inválida.' }
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
    if (-not $SkipPublish) {
        Copy-Item -LiteralPath $exe -Destination (Join-Path $project 'WGR.exe') -Force
        Copy-Item -LiteralPath (Join-Path $build 'release/wgr-rage.exe') -Destination (Join-Path $project 'wgr-rage.exe') -Force
    }
    Write-Output "Validação concluída. Executável validado: $exe. Logs em artifacts/."
} finally {
    if ($ownsBuild -and -not $KeepBuild -and (Test-Path -LiteralPath $build)) {
        cargo clean --target-dir $build *> artifacts/build-cleanup.log
    }
    Pop-Location
    $env:PATH = $previousPath
}
