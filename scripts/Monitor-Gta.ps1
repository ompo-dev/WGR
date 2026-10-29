param(
    [Parameter(Mandatory=$true)][int]$GameProcessId,
    [ValidateRange(10,600)][int]$Seconds = 180,
    [Parameter(Mandatory=$true)][string]$Name,
    [string]$Report = ''
)
$ErrorActionPreference = 'Stop'
if ($Name -notmatch '^[a-zA-Z0-9_-]{1,40}$') { throw 'Nome de medição inválido.' }
$project = (Resolve-Path (Join-Path $PSScriptRoot '..')).Path
$output = Join-Path $project "artifacts/monitor-$Name.json"
$journal = Join-Path $project "artifacts/monitor-$Name-samples.ndjson"
if ((Test-Path -LiteralPath $output) -or (Test-Path -LiteralPath $journal)) { throw 'Use outro nome para preservar a medição.' }
$root = Get-Process -Id $GameProcessId
Add-Type -TypeDefinition 'using System;using System.Runtime.InteropServices;public static class WgrMonitorWindow{[DllImport("user32.dll")]public static extern IntPtr GetForegroundWindow();[DllImport("user32.dll")]public static extern bool IsIconic(IntPtr window);[DllImport("kernel32.dll")]static extern IntPtr OpenThread(uint access,bool inherit,uint id);[DllImport("kernel32.dll")]static extern int GetThreadDescription(IntPtr thread,out IntPtr name);[DllImport("kernel32.dll")]static extern bool CloseHandle(IntPtr handle);[DllImport("kernel32.dll")]static extern IntPtr LocalFree(IntPtr memory);public static string ThreadName(uint id){var thread=OpenThread(0x800,false,id);if(thread==IntPtr.Zero)return "";IntPtr name=IntPtr.Zero;try{return GetThreadDescription(thread,out name)>=0?Marshal.PtrToStringUni(name):"";}finally{if(name!=IntPtr.Zero)LocalFree(name);CloseHandle(thread);}}}'
if ($root.ProcessName -ne 'WGR') { throw 'O processo informado não é o WGR.' }
$samples = [Collections.Generic.List[object]]::new()
$eventPath = Join-Path $project "artifacts/monitor-$Name-events.ndjson"
$eventSeq = 0L; $eventBytes = 0L; $eventGaps = 0L
$threadPrevious = @{}; $threadNames = @{}
$watch = [Diagnostics.Stopwatch]::StartNew()
while (-not $root.HasExited -and $watch.Elapsed.TotalSeconds -lt $Seconds) {
    $sampleStart = [DateTimeOffset]::UtcNow.ToUnixTimeMilliseconds()
    $rows = @(Get-CimInstance Win32_Process -Filter "name='msedgewebview2.exe' OR name='WGR.exe' OR name='wgr-rage.exe'")
    $ids = [Collections.Generic.HashSet[int]]::new(); [void]$ids.Add($GameProcessId)
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
            $kind = if ($idValue -eq $GameProcessId) { 'launcher' } elseif ($native.Name -eq 'wgr-rage.exe') { 'wasmtime-wgpu-native' } elseif ($native.CommandLine -match '--type=([^ ]+)') { $Matches[1] } else { 'browser' }
            [pscustomobject]@{pid=$item.Id;kind=$kind;working_bytes=$item.WorkingSet64;private_bytes=$item.PrivateMemorySize64;cpu_seconds=$item.TotalProcessorTime.TotalSeconds;read_transfer_bytes=$native.ReadTransferCount;write_transfer_bytes=$native.WriteTransferCount}
        }
    })
    $threads = @(foreach ($member in $members | Where-Object kind -eq 'wasmtime-wgpu-native') {
        $engine = Get-Process -Id $member.pid -ErrorAction SilentlyContinue
        if ($engine) { foreach ($thread in $engine.Threads) { try {
            $key = "$($member.pid):$($thread.Id)"
            $cpu = $thread.TotalProcessorTime.TotalSeconds
            $at = $watch.Elapsed.TotalSeconds
            if (-not $threadNames.ContainsKey($key)) { $threadNames[$key] = [WgrMonitorWindow]::ThreadName([uint32]$thread.Id) }
            if ($threadPrevious.ContainsKey($key)) {
                $previous = $threadPrevious[$key]
                [pscustomobject]@{pid=$member.pid;tid=$thread.Id;name=$threadNames[$key];one_core_percent=[math]::Round(100*($cpu-$previous.cpu)/[math]::Max(0.001,$at-$previous.at),1)}
            }
            $threadPrevious[$key] = @{cpu=$cpu;at=$at}
        } catch { } } }
    })
    $threads = @($threads | Sort-Object one_core_percent -Descending | Select-Object -First 8)
    $gpu = @(Get-CimInstance Win32_PerfFormattedData_GPUPerformanceCounters_GPUProcessMemory -ErrorAction SilentlyContinue | Where-Object { $_.Name -match '^pid_(\d+)_' -and $ids.Contains([int]$Matches[1]) } | ForEach-Object { [pscustomobject]@{instance=$_.Name;dedicated_bytes=[long]$_.DedicatedUsage;shared_bytes=[long]$_.SharedUsage;committed_bytes=[long]$_.TotalCommitted} })
    $memory = Get-CimInstance Win32_PerfFormattedData_PerfOS_Memory | Select-Object AvailableMBytes,CommittedBytes,CommitLimit,PagesInputPersec,PagesOutputPersec
    $processPerf = @(Get-CimInstance Win32_PerfFormattedData_PerfProc_Process | Where-Object { $ids.Contains([int]$_.IDProcess) } | Select-Object IDProcess,PageFaultsPersec,IOReadBytesPersec,IOWriteBytesPersec)
    $disk = Get-CimInstance Win32_PerfFormattedData_PerfDisk_PhysicalDisk -Filter "Name='_Total'" | Select-Object DiskReadBytesPersec,DiskWriteBytesPersec,PercentDiskTime,CurrentDiskQueueLength
    $port = $null
    if ($Report -and (Test-Path -LiteralPath $Report)) { try { $port = Get-Content -LiteralPath $Report -Raw | ConvertFrom-Json } catch {} }
    if ($port.timeline) {
        $fresh = @($port.timeline | Where-Object {$_.seq -gt $eventSeq})
        if ($fresh.Count) {
            if ($fresh[0].seq -gt $eventSeq+1) { $eventGaps += $fresh[0].seq-$eventSeq-1 }
            $lines = ($fresh | ForEach-Object {ConvertTo-Json -InputObject $_ -Depth 8 -Compress}) -join "`n"
            $lines += "`n"
            $length = [Text.Encoding]::UTF8.GetByteCount($lines)
            if ($eventBytes+$length -le 8MB) { [IO.File]::AppendAllText($eventPath,$lines); $eventBytes += $length }
            $eventSeq = $fresh[-1].seq
        }
    }
    $samples.Add([pscustomobject]@{epoch_ms=$sampleStart;sample_end_epoch_ms=[DateTimeOffset]::UtcNow.ToUnixTimeMilliseconds();seconds=[math]::Round($watch.Elapsed.TotalSeconds,2);members=$members;native_threads=$threads;gpu=$gpu;system_memory=$memory;process_perf=$processPerf;physical_disk=$disk;port_elapsed=$port.elapsed_seconds;cache=$port.cache;gpu_memory=$port.diagnostic.gpu_memory;gpu_window=$port.diagnostic.gpu_window;fps=$port.port_frame_report.payload.fps;event_seq=$eventSeq;event_bytes=$eventBytes;event_gaps=$eventGaps;visible_main_window=($root.MainWindowHandle -ne [IntPtr]::Zero);foreground=([WgrMonitorWindow]::GetForegroundWindow() -eq $root.MainWindowHandle);minimized=[WgrMonitorWindow]::IsIconic($root.MainWindowHandle)})
    # At most 600 samples; only the owned process tree, no command lines/headers/keys.
    # Append each sample once; repeatedly serializing the entire session made
    # the monitor itself generate growing CPU and disk traffic during play.
    [IO.File]::AppendAllText($journal, ($samples[$samples.Count-1] | ConvertTo-Json -Depth 8 -Compress) + "`n")
    Start-Sleep -Milliseconds 1000
    $root.Refresh()
}
$samples | ConvertTo-Json -Depth 8 | Set-Content -LiteralPath $output
[pscustomobject]@{file=$output;samples=$samples.Count;process_id=$GameProcessId;game_left_open=(-not $root.HasExited)} | ConvertTo-Json
