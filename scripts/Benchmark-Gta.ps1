param(
    [string]$Executable = (Join-Path $PSScriptRoot '../WGR.exe'),
    [ValidateSet('0','30','60')][string]$Fps = '0',
    [ValidateSet('gta5','env_test')][string]$Map = 'gta5',
    [switch]$GpuLogs,
    [switch]$Stress,
    [switch]$Tuned,
    [switch]$ResourceCounters,
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
    foreach($key in @('WGR_PARALLEL_FINISH','WGR_ENCODER_WORKERS','WGR_ENCODER_BATCH_PASSES','WGR_ENCODER_BATCH_DRAWS','WGR_LOCAL_BIND_CACHE','WGR_LOCAL_BIND_EPOCHS','WGR_SHARED_UNIFORM_LAYOUTS','WGR_UNIFORM_CHUNK_MIB','WGR_SHADER_AWARE_WARMUP','WGR_HOT_PIPELINES','MUGUET_PERF_DIAGNOSTICS','WGR_FRAME_PHASES','WGR_ASYNC_STATS','WGR_ASSET_HOT_CACHE_MIB','WGR_IO_WORKERS','WGR_GUEST_CORES','WGR_DISK_SINGLEFLIGHT')) {
        $rendererConfig[$key]=[Environment]::GetEnvironmentVariable($key,'Process')
    }
    $process = Start-Process -FilePath $exe -ArgumentList $arguments -WindowStyle Normal -PassThru
} finally {
    foreach($key in $savedEnv.Keys) { [Environment]::SetEnvironmentVariable($key,$savedEnv[$key],'Process') }
}
$watch = [Diagnostics.Stopwatch]::StartNew()
while (-not $process.HasExited -and $watch.Elapsed.TotalSeconds -lt 930) {
    $sampleStarted=$watch.Elapsed.TotalSeconds
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
            [pscustomobject]@{ pid=$item.Id; created=$native.CreationDate; name=$item.ProcessName; private_bytes=$item.PrivateMemorySize64; working_bytes=$item.WorkingSet64; cpu_seconds=$item.TotalProcessorTime.TotalSeconds; read_transfer_bytes=$native.ReadTransferCount; write_transfer_bytes=$native.WriteTransferCount }
        }
    })
    $gpu = @(Get-CimInstance Win32_PerfFormattedData_GPUPerformanceCounters_GPUProcessMemory -ErrorAction SilentlyContinue | Where-Object { $_.Name -match '^pid_(\d+)_' -and $ids.Contains([int]$Matches[1]) } | ForEach-Object { [pscustomobject]@{instance=$_.Name;dedicated_bytes=[long]$_.DedicatedUsage;shared_bytes=[long]$_.SharedUsage;committed_bytes=[long]$_.TotalCommitted} })
    $memory = Get-CimInstance Win32_PerfFormattedData_PerfOS_Memory | Select-Object AvailableMBytes,CommittedBytes,CommitLimit,PagesInputPersec,PagesOutputPersec
    $engines=@(); $disk=$null
    if ($ResourceCounters) {
        $engines=@(Get-CimInstance Win32_PerfFormattedData_GPUPerformanceCounters_GPUEngine -ErrorAction SilentlyContinue | Where-Object { $_.Name -match '^pid_(\d+)_(.+)$' -and $ids.Contains([int]$Matches[1]) } | ForEach-Object { [pscustomobject]@{instance=$_.Name;engine=($_.Name -replace '^pid_\d+_','');utilization_pct=[double]$_.UtilizationPercentage} })
        $disk=Get-CimInstance Win32_PerfFormattedData_PerfDisk_PhysicalDisk -Filter "Name='_Total'" -ErrorAction SilentlyContinue | Select-Object DiskReadBytesPersec,DiskWriteBytesPersec,DiskReadsPersec,DiskWritesPersec
    }
    $samples.Add([pscustomobject]@{ seconds=[math]::Round($watch.Elapsed.TotalSeconds,3); sample_duration_ms=[math]::Round(($watch.Elapsed.TotalSeconds-$sampleStarted)*1000,1); members=$members; private_bytes=($members|Measure-Object private_bytes -Sum).Sum; working_bytes=($members|Measure-Object working_bytes -Sum).Sum; gpu_process_memory=$gpu; gpu_engines=$engines; system_physical_disk=$disk; free_disk_bytes=(Get-PSDrive C).Free; system_memory=$memory; visible_main_window=($process.MainWindowHandle -ne [IntPtr]::Zero) })
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
$ioSamples=@()
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
                    if($event.payload.io){$ioSamples+=@([pscustomobject]@{seconds=$event.payload.world_elapsed_ms/1000.0;io=$event.payload.io})}
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
# CPU and process I/O deltas only for intervals with the same process set,
# entirely after the warmup. Never subtract counters across process lifetimes.
$logicalCpu=[Environment]::ProcessorCount
$steady=@($samples | Where-Object { $_.seconds -ge ($report.world_at_ms/1000.0 + 4) })
$cpuSeconds=0.0; $wallSeconds=0.0; $ioRead=0.0; $ioWrite=0.0; $ioSeconds=0.0; $intervalCount=0
for($i=1;$i -lt $steady.Count;$i++) {
    $a=$steady[$i-1];$b=$steady[$i];$dt=$b.seconds-$a.seconds
    if($dt -le 0 -or $a.members.Count -ne $b.members.Count){continue}
    $old=@{};foreach($m in $a.members){$old[[int]$m.pid]=$m}
    $valid=$true;$validIo=$true;$cpu=0.0;$read=0.0;$write=0.0
    foreach($m in $b.members){
        $p=$old[[int]$m.pid]
        if(!$p -or $null -eq $m.created -or $m.created -ne $p.created -or $m.cpu_seconds -lt $p.cpu_seconds){$valid=$false;break}
        $cpu+=$m.cpu_seconds-$p.cpu_seconds
        if($null -eq $m.read_transfer_bytes -or $null -eq $p.read_transfer_bytes -or $null -eq $m.write_transfer_bytes -or $null -eq $p.write_transfer_bytes -or $m.read_transfer_bytes -lt $p.read_transfer_bytes -or $m.write_transfer_bytes -lt $p.write_transfer_bytes){$validIo=$false}
        else{$read+=[double]$m.read_transfer_bytes-[double]$p.read_transfer_bytes;$write+=[double]$m.write_transfer_bytes-[double]$p.write_transfer_bytes}
    }
    if($valid){$cpuSeconds+=$cpu;$wallSeconds+=$dt;$intervalCount++;if($validIo){$ioRead+=$read;$ioWrite+=$write;$ioSeconds+=$dt}}
}
$enginePeaks=@(foreach($sample in $steady){if(@($sample.gpu_engines).Count){($sample.gpu_engines | Group-Object engine | ForEach-Object { [math]::Min(100,($_.Group | Measure-Object utilization_pct -Sum).Sum) } | Measure-Object -Maximum).Maximum}})
$diskSamples=@($steady | ForEach-Object { if($null -ne $_.system_physical_disk){$_.system_physical_disk} })
$assetDelta=[ordered]@{}
if($ioSamples.Count -ge 2){
    $firstIo=$ioSamples[0];$lastIo=$ioSamples[-1];$assetDelta['seconds']=$lastIo.seconds-$firstIo.seconds
    foreach($field in @('hot_hits','disk_hits','disk_coalesced_reads','disk_verified_payload_bytes','disk_lookup_ms','disk_verify_ms','disk_db_wait_ms','disk_store_ms','transport_fetches','decoded_asset_bytes','network_fetch_ms','disk_lru_batches','disk_lru_entries','hints_completed')){
        $beforeIo=$firstIo.io.$field;$afterIo=$lastIo.io.$field
        if($null -ne $beforeIo -and $null -ne $afterIo -and $afterIo -ge $beforeIo){$assetDelta[$field]=$afterIo-$beforeIo}
    }
}
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
    resource_counters=[bool]$ResourceCounters; resource_steady_samples=$steady.Count; resource_cpu_intervals=$intervalCount; logical_cpu_count=$logicalCpu
    steady_cpu_pct=if($wallSeconds -gt 0){100*$cpuSeconds/($wallSeconds*$logicalCpu)}else{$null}
    steady_cpu_core_ms_per_frame_estimate=if($wallSeconds -gt 0 -and $values.Count -gt 0 -and ($values|Measure-Object -Average).Average -gt 0){1000*$cpuSeconds/$wallSeconds/($values|Measure-Object -Average).Average}else{$null}
    steady_process_read_mib_per_sec=if($ioSeconds -gt 0){$ioRead/$ioSeconds/1MB}else{$null}
    steady_process_write_mib_per_sec=if($ioSeconds -gt 0){$ioWrite/$ioSeconds/1MB}else{$null}
    steady_gpu_engine_samples=$enginePeaks.Count
    steady_gpu_busiest_engine_mean_pct=if($enginePeaks.Count){($enginePeaks|Measure-Object -Average).Average}else{$null}
    steady_gpu_busiest_engine_peak_pct=if($enginePeaks.Count){($enginePeaks|Measure-Object -Maximum).Maximum}else{$null}
    steady_system_disk_read_mib_per_sec=if($diskSamples.Count){($diskSamples|Measure-Object DiskReadBytesPersec -Average).Average/1MB}else{$null}
    steady_system_disk_write_mib_per_sec=if($diskSamples.Count){($diskSamples|Measure-Object DiskWriteBytesPersec -Average).Average/1MB}else{$null}
    asset_io_steady_delta=$assetDelta
    asset_io_last_snapshot=if($ioSamples.Count){$ioSamples[-1].io}else{$null}
    minimum_available_ram_mib=($samples.system_memory|Measure-Object AvailableMBytes -Minimum).Minimum
    peak_system_pages_input_per_sec=($samples.system_memory|Measure-Object PagesInputPersec -Maximum).Maximum
    free_disk_before=$before; free_disk_after=(Get-PSDrive C).Free
    visible_main_window_observed=(@($samples | Where-Object visible_main_window).Count -gt 0)
    measurement='30 s após evento de mundo; FPS de submissão, não scanout. CPU por tempo de processador normalizado por CPUs lógicas, estimativa por frame usando FPS médio da janela oficial; amostras de recursos são mais esparsas. I/O do processo inclui arquivos, rede e dispositivos; disco físico e paginação são do sistema inteiro. Utilização GPU é o maior motor por amostra, não soma de motores distintos. Working sets podem conter páginas compartilhadas; memória GPU não equivale a heap Wasm.'
}
$summary | ConvertTo-Json -Depth 5 | Set-Content -LiteralPath (Join-Path $project "artifacts/perf-$Name-summary.json")
$summary | ConvertTo-Json -Depth 5
if (-not $report.world_confirmed) { throw 'O mundo não foi confirmado; FPS de loading não conta.' }
if (($isStress -or $MinimumFps -gt 0) -and -not $performancePassed) { throw 'Gate de desempenho reprovado: carga incompleta, amostras insuficientes, FPS abaixo do mínimo ou frames acima de 16,667 ms.' }
