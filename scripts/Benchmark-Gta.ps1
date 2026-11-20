param(
    [string]$Executable = (Join-Path $PSScriptRoot '../WGR.exe'),
    [ValidateSet('0','30','60')][string]$Fps = '0',
    [ValidateSet('gta5','env_test')][string]$Map = 'gta5',
    [switch]$GpuLogs,
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
$process = Start-Process -FilePath $exe -ArgumentList $arguments -WindowStyle Normal -PassThru
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
    gpu_logs=[bool]$GpuLogs
    first_frame_ms=$report.diagnostic.first_frame_ms; world_ms=$report.world_at_ms
    samples=$values.Count; fps_median=if($values.Count){$values[[int][math]::Floor($values.Count/2)]}else{$null}
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
