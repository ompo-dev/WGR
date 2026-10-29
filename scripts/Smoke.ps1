param([string]$Executable = (Join-Path $PSScriptRoot '../WGR.exe'))
$ErrorActionPreference = 'Stop'
$project = (Resolve-Path (Join-Path $PSScriptRoot '..')).Path
$exe = (Resolve-Path -LiteralPath $Executable).Path
$data = Join-Path $project 'artifacts/smoke-data'
$reports = @()
for ($run = 1; $run -le 3; $run++) {
    $reportPath = Join-Path $project "artifacts/release-smoke-$run.json"
    if (Test-Path -LiteralPath $reportPath) { Move-Item -LiteralPath $reportPath -Destination ($reportPath + '.' + [guid]::NewGuid().ToString('N') + '.previous') }
    foreach($capture in @([IO.Path]::ChangeExtension($reportPath,'png'),(Join-Path (Split-Path $reportPath) ([IO.Path]::GetFileNameWithoutExtension($reportPath)+'-library.png')))) {if(Test-Path -LiteralPath $capture){Move-Item -LiteralPath $capture -Destination ($capture+'.'+[guid]::NewGuid().ToString('N')+'.previous')}}
    $arguments = @('--data-dir', ('"' + $data + '"'), '--smoke', ('"' + $reportPath + '"'))
    $process = Start-Process -FilePath $exe -ArgumentList $arguments -WindowStyle Normal -PassThru
    if (-not $process.WaitForExit(60000)) { $process.Kill(); throw 'O smoke WGR excedeu 60 segundos; somente o processo de teste foi encerrado.' }
    if (-not (Test-Path -LiteralPath $reportPath)) { throw "Sem relatório de smoke: $reportPath" }
    $report = Get-Content -LiteralPath $reportPath -Raw | ConvertFrom-Json
    if (-not $report.passed) { throw "Player smoke falhou: $($report.last_notice)" }
    $native = $report.first_frame.backend -eq 'native'
    if ($native) {
        if ($report.first_frame.engine -ne 'wasmtime-native-gpui' -or -not $report.first_frame.binary_sha256) {throw 'A fixture não comprovou execução do binário no host Rust.'}
    } else {
        if (-not $report.first_frame.worker.nested) {throw 'Worker criado dentro de Worker não respondeu.'}
        if (-not $report.first_frame.download_cache.quota -or -not $report.first_frame.download_cache.reopened) {throw 'Cache persistente não passou a validação de quota/reabertura.'}
        if (-not $report.first_frame.download_cache.fragments) {throw 'Cache não reuniu intervalos adjacentes ou duplicou bytes.'}
        if ($run -gt 1 -and -not $report.first_frame.download_cache.restored) {throw 'Downloads em cache não foram restaurados na reabertura.'}
    }
    if (-not $report.native_key_sent -or -not $report.input_received) { throw 'Input Win32 não comprovado.' }
    if ($report.diagnostic.capabilities.AudioWorklet -eq $true -and $report.audio.state -ne 'running') { throw 'AudioWorklet existe, mas o áudio não chegou a running.' }
    if ($report.diagnostic.capabilities.WebGPU.available -eq $true -and -not $report.webgpu.rendered) { throw 'WebGPU existe, mas não houve submissão de quadro.' }
    if ($run -gt 1 -and -not $report.first_frame.restored) { throw 'Save não restaurado após reabertura.' }
    if ($run -gt 1 -and $report.diagnostic.game_id -ne $reports[0].diagnostic.game_id) { throw 'UUID do jogo não persistiu.' }
    $reports += $report
}
$times = @($reports | ForEach-Object { [double]$_.diagnostic.first_frame_ms } | Sort-Object)
$summary = [ordered]@{
    passed = $true
    runs = 3
    runtime = $reports[0].diagnostic.runtime
    first_frame_median_ms = $times[1]
    first_frame_min_ms = $times[0]
    first_frame_max_ms = $times[2]
    context = 'WGR/Órbita local; mesmo perfil entre execuções; não é comparação com Edge ou medição física de apresentação.'
    physical_gamepad_tested = $false
    human_listening_tested = $false
    save_restored = $reports[1].first_frame.restored -and $reports[2].first_frame.restored
}
$summary | ConvertTo-Json -Depth 12 | Set-Content -LiteralPath (Join-Path $project 'artifacts/smoke-summary.json') -Encoding utf8
Write-Output ($summary | ConvertTo-Json -Depth 12)
