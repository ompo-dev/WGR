param(
    [string]$Executable = (Join-Path $PSScriptRoot '../WGR.exe'),
    [ValidateSet('0','30','60')][string]$Fps = '0',
    [ValidateSet('gta5','env_test')][string]$Map = 'gta5',
    [switch]$GpuLogs,
    [switch]$Stress,
    [switch]$Tuned,
    [ValidateRange(0,1000)][double]$MinimumFps = 0,
    [string]$Name = 'uncapped'
)
$ErrorActionPreference = 'Stop'
$project = (Resolve-Path (Join-Path $PSScriptRoot '..')).Path
$exe = (Resolve-Path -LiteralPath $Executable).Path
if ($Name -notmatch '^[a-zA-Z0-9_-]{1,40}$') { throw 'Nome de medição inválido.' }
if (Get-Process WGR -ErrorAction SilentlyContinue) { throw 'Feche a instância WGR existente antes de medir uma única engine.' }
$reportPath = Join-Path $project "artifacts/perf-$Name.json"
if (Test-Path -LiteralPath $reportPath) { throw 'Use um nome novo para preservar a medição anterior.' }
$before = (Get-PSDrive C).Free
$samples = [Collections.Generic.List[object]]::new()
$arguments = @('--gta-smoke','--gta-fps',$Fps,'--smoke',('"'+$reportPath+'"'))
if ($Map -eq 'env_test') { $arguments += @('--gta-map','env_test') }
if ($GpuLogs) { $arguments += '--gta-logs' }
# This is the interactive product under test, not a background helper. An
# occluded/hidden window changes browser timers and invalidates presentation tests.
$benchmarkStarted = Get-Date
$isStress = [bool]($Stress -or $env:WGR_TEST_STRESS -eq '1')
$overrides = @{}
if ($isStress) { $overrides['WGR_TEST_STRESS']='1'; $overrides['WGR_FRAME_PACING']='1' }
if ($Tuned) {
    $overrides['WGR_PARALLEL_FINISH']='1'
    $overrides['WGR_ENCODER_WORKERS']='6'
    $overrides['WGR_ENCODER_BATCH_PASSES']='16'
    $overrides['WGR_LOCAL_BIND_CACHE']='1'
    $overrides['WGR_LOCAL_BIND_EPOCHS']='0'
    $overrides['WGR_SHARED_UNIFORM_LAYOUTS']='1'
    $overrides['WGR_UNIFORM_CHUNK_MIB']='4'
    $overrides['WGR_SHADER_AWARE_WARMUP']='0'
    $overrides['WGR_HOT_PIPELINES']=$null
    $overrides['MUGUET_PERF_DIAGNOSTICS']=$null
}
$savedEnv=@{}
$rendererConfig=[ordered]@{}
try {
    foreach($key in $overrides.Keys) {
        $savedEnv[$key]=[Environment]::GetEnvironmentVariable($key,'Process')
        [Environment]::SetEnvironmentVariable($key,$overrides[$key],'Process')
    }
    foreach($key in @('WGR_PARALLEL_FINISH','WGR_ENCODER_WORKERS','WGR_ENCODER_BATCH_PASSES','WGR_LOCAL_BIND_CACHE','WGR_LOCAL_BIND_EPOCHS','WGR_SHARED_UNIFORM_LAYOUTS','WGR_UNIFORM_CHUNK_MIB','WGR_SHADER_AWARE_WARMUP','WGR_HOT_PIPELINES','MUGUET_PERF_DIAGNOSTICS')) {
        $rendererConfig[$key]=[Environment]::GetEnvironmentVariable($key,'Process')
    }
    $process = Start-Process -FilePath $exe -ArgumentList $arguments -WindowStyle Normal -PassThru
} finally {
    foreach($key in $savedEnv.Keys) { [Environment]::SetEnvironmentVariable($key,$savedEnv[$key],'Process') }
}
$watch = [Diagnostics.Stopwatch]::StartNew()
while (-not $process.HasExited -and $watch.Elapsed.TotalSeconds -lt 930) {
    # Follow only this process tree; other applications also use WebView2.
    $rows = @(Get-CimInstance Win32_Process -Filter "name='msedgewebview2.exe' OR name='WGR.exe' OR name='wgr-rage.exe'")
    $ids = [Collections.Generic.HashSet[int]]::new(); [void]$ids.Add($process.Id)
    do {
        $added = $false
        foreach ($row in $rows) {
            if ($ids.Contains([int]$row.ParentProcessId) -and $ids.Add([int]$row.ProcessId)) { $added = $true }
        }
    } while ($added)
    $members = @(foreach ($idValue in $ids) {
        $item = Get-Process -Id $idValue -ErrorAction SilentlyContinue
        if ($item) {
            $native = $rows | Where-Object ProcessId -eq $idValue | Select-Object -First 1
            [pscustomobject]@{ pid=$item.Id; name=$item.ProcessName; private_bytes=$item.PrivateMemorySize64; working_bytes=$item.WorkingSet64; cpu_seconds=$item.TotalProcessorTime.TotalSeconds; read_transfer_bytes=$native.ReadTransferCount; write_transfer_bytes=$native.WriteTransferCount }
        }
    })
    $gpu = @(Get-CimInstance Win32_PerfFormattedData_GPUPerformanceCounters_GPUProcessMemory -ErrorAction SilentlyContinue | Where-Object { $_.Name -match '^pid_(\d+)_' -and $ids.Contains([int]$Matches[1]) } | ForEach-Object { [pscustomobject]@{instance=$_.Name;dedicated_bytes=[long]$_.DedicatedUsage;shared_bytes=[long]$_.SharedUsage;committed_bytes=[long]$_.TotalCommitted} })
    $memory = Get-CimInstance Win32_PerfFormattedData_PerfOS_Memory | Select-Object AvailableMBytes,CommittedBytes,CommitLimit,PagesInputPersec,PagesOutputPersec
    $samples.Add([pscustomobject]@{ seconds=[math]::Round($watch.Elapsed.TotalSeconds,1); members=$members; private_bytes=($members|Measure-Object private_bytes -Sum).Sum; working_bytes=($members|Measure-Object working_bytes -Sum).Sum; gpu_process_memory=$gpu; free_disk_bytes=(Get-PSDrive C).Free; system_memory=$memory; visible_main_window=($process.MainWindowHandle -ne [IntPtr]::Zero) })
    $samples | ConvertTo-Json -Depth 5 | Set-Content -LiteralPath (Join-Path $project "artifacts/perf-$Name-resources.json")
    Start-Sleep -Seconds 5
    $process.Refresh()
}
if (-not $process.HasExited) { [void]$process.CloseMainWindow(); throw 'Benchmark excedeu o prazo; fechamento solicitado somente à janela de teste.' }
$report = Get-Content -LiteralPath $reportPath -Raw | ConvertFrom-Json
$values = @($report.fps_samples | Where-Object { $_.at_ms -ge ($report.world_at_ms + 4000) } | ForEach-Object { [double]$_.fps } | Sort-Object)
if ($isStress) { $MinimumFps = [math]::Max(60,$MinimumFps) }
$vehicles = 0; $bursts = 0; $stressFinished = $false; $peakExisting=0; $peakOnScreen=0; $fullLoadSamples=0
$requiredVehicles=18; $requiredBursts=36
$frameIntervals = @()
if ($isStress -and $report.diagnostic.game_id) {
    $logPath = Join-Path $env:LOCALAPPDATA ('WGR/profiles/'+$report.diagnostic.game_id+'/native-logs/output.log')
    if ((Test-Path -LiteralPath $logPath) -and (Get-Item -LiteralPath $logPath).LastWriteTime -ge $benchmarkStarted) {
        foreach ($line in (Get-Content -LiteralPath $logPath | Select-String 'native_stress_|"type":"game-fps"')) {
            if ($line.Line -match 'WGR_EVENT (\{.*\})') {
                $event = $Matches[1] | ConvertFrom-Json
                if ($event.type -eq 'native_stress_vehicle') { $vehicles = [math]::Max($vehicles,[int]$event.count) }
                if ($event.type -eq 'native_stress_burst') { $bursts = [math]::Max($bursts,[int]$event.count) }
                if ($event.type -eq 'native_stress_finished') { $stressFinished = $true }
                if ($event.type -eq 'native_stress_observation') {
                    $peakExisting=[math]::Max($peakExisting,[int]$event.existing)
                    $peakOnScreen=[math]::Max($peakOnScreen,[int]$event.on_screen)
                    if ($event.existing -ge $requiredVehicles -and $event.on_screen -ge $requiredVehicles -and $event.bursts -gt 0) { $fullLoadSamples++ }
                }
                if ($event.type -eq 'game-fps' -and $event.payload.world_elapsed_ms -ge 4000 -and $event.payload.frame_intervals) {
                    $frameIntervals += $event.payload.frame_intervals
                }
            }
        }
    }
}
$stressComplete = -not $isStress -or ($vehicles -ge $requiredVehicles -and $bursts -ge $requiredBursts -and $stressFinished -and $fullLoadSamples -ge 5)
$measuredFrames = ($frameIntervals | Measure-Object count -Sum).Sum
$budgetMisses = ($frameIntervals | Measure-Object over_60fps_budget -Sum).Sum
$frameOverflow = ($frameIntervals | Measure-Object overflow -Sum).Sum
$framePacingPassed = $frameIntervals.Count -ge 20 -and $measuredFrames -gt 0 -and $budgetMisses -eq 0 -and $frameOverflow -eq 0
$performancePassed = $report.world_confirmed -and $values.Count -ge 20 -and $stressComplete -and $values[0] -ge $MinimumFps -and (-not $isStress -or $framePacingPassed)
$gpuTotals = @($samples | ForEach-Object {
    if (@($_.gpu_process_memory).Count -gt 0) {
        [pscustomobject]@{
            dedicated_bytes = (@($_.gpu_process_memory) | Measure-Object dedicated_bytes -Sum).Sum
            shared_bytes = (@($_.gpu_process_memory) | Measure-Object shared_bytes -Sum).Sum
            committed_bytes = (@($_.gpu_process_memory) | Measure-Object committed_bytes -Sum).Sum
        }
    }
})
$summary = [ordered]@{
    name=$Name; map=$Map; fps_option=$Fps; world_confirmed=$report.world_confirmed
    renderer_profile=if($Tuned){'tuned-experimental'}else{'inherited'}; renderer_environment=$rendererConfig
    gpu_logs=[bool]$GpuLogs
    first_frame_ms=$report.diagnostic.first_frame_ms; world_ms=$report.world_at_ms
    samples=$values.Count; fps_median=if($values.Count){$values[[int][math]::Floor($values.Count/2)]}else{$null}
    required_minimum_fps=$MinimumFps; performance_gate_passed=[bool]$performancePassed
    stress_enabled=$isStress; stress_complete=$stressComplete; stress_vehicles=$vehicles; stress_bursts=$bursts
    stress_required_vehicles=$requiredVehicles; stress_required_bursts=$requiredBursts; stress_peak_existing=$peakExisting; stress_peak_on_screen=$peakOnScreen; stress_full_load_samples=$fullLoadSamples
    frame_pacing_windows=$frameIntervals.Count; frame_intervals_measured=$measuredFrames; frames_over_16_667ms=$budgetMisses
    frame_interval_max_ms=if($measuredFrames){($frameIntervals|Measure-Object max_us -Maximum).Maximum/1000}else{$null}
    frame_interval_worst_window_p99_ms=if($measuredFrames){($frameIntervals|Measure-Object p99_us -Maximum).Maximum/1000}else{$null}
    frame_pacing_passed=[bool]$framePacingPassed
    fps_p10=if($values.Count){$values[[int][math]::Floor(($values.Count-1)*0.1)]}else{$null}
    fps_min=if($values.Count){$values[0]}else{$null}; fps_max=if($values.Count){$values[-1]}else{$null}
    linear_memory=$report.diagnostic.memory; configuration=$report.diagnostic.configuration
    cache=$report.cache; peak_private_bytes=($samples|Measure-Object private_bytes -Maximum).Maximum
    peak_working_bytes=($samples|Measure-Object working_bytes -Maximum).Maximum
    gpu_samples=$gpuTotals.Count
    peak_gpu_dedicated_bytes=($gpuTotals|Measure-Object dedicated_bytes -Maximum).Maximum
    peak_gpu_shared_bytes=($gpuTotals|Measure-Object shared_bytes -Maximum).Maximum
    peak_gpu_committed_bytes=($gpuTotals|Measure-Object committed_bytes -Maximum).Maximum
    free_disk_before=$before; free_disk_after=(Get-PSDrive C).Free
    visible_main_window_observed=(@($samples | Where-Object visible_main_window).Count -gt 0)
    measurement='30 s após evento de mundo; FPS de submissão reportado pelo port, não apresentação física. Working sets podem conter páginas compartilhadas. Contadores GPU por processo/adapter quando disponíveis: não equivalem ao tamanho de heap Wasm, nem são necessariamente a memória física exclusiva de texturas.'
}
$summary | ConvertTo-Json -Depth 5 | Set-Content -LiteralPath (Join-Path $project "artifacts/perf-$Name-summary.json")
$summary | ConvertTo-Json -Depth 5
if (-not $report.world_confirmed) { throw 'O mundo não foi confirmado; FPS de loading não conta.' }
if (($isStress -or $MinimumFps -gt 0) -and -not $performancePassed) { throw 'Gate de desempenho reprovado: carga incompleta, amostras insuficientes, FPS abaixo do mínimo ou frames acima de 16,667 ms.' }
