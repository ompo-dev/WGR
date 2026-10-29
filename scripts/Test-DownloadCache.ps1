param([string]$Executable = (Join-Path $PSScriptRoot '../WGR.exe'))
$ErrorActionPreference = 'Stop'
$project = (Resolve-Path (Join-Path $PSScriptRoot '..')).Path
$exe = (Resolve-Path -LiteralPath $Executable).Path
$data = Join-Path $project 'artifacts/download-cache-data'
New-Item -ItemType Directory -Path $data -Force | Out-Null
$seed = Start-Process -FilePath $exe -ArgumentList @('--data-dir',('"'+$data+'"'),'--clear-cache') -WindowStyle Hidden -PassThru
if (-not $seed.WaitForExit(30000) -or $seed.ExitCode) {throw 'Não foi possível preparar a fixture de cache.'}
$fixture = Join-Path $data 'fixtures/orbit'
$html = [IO.File]::ReadAllText((Join-Path $project 'fixtures/orbit/index.html')).Replace('src="game.js"','src="cache-test-game.js"')
[IO.File]::WriteAllText((Join-Path $fixture 'cache-test.html'),$html)
$helper = Join-Path $project 'crates/wgr-http/src/ram_io.js'
$helperName = 'cache-test-io-' + (Get-FileHash -LiteralPath $helper -Algorithm SHA256).Hash.Substring(0,12) + '.js'
$game = [IO.File]::ReadAllText((Join-Path $project 'fixtures/orbit/game.js')).Replace("script.src='/__wgr_ram_io.js'",("script.src='"+$helperName+"'"))
[IO.File]::WriteAllText((Join-Path $fixture 'cache-test-game.js'),$game)
Copy-Item -LiteralPath $helper -Destination (Join-Path $fixture $helperName) -Force
$env:WGR_CACHE_TEST_ROOT=$data
@'
const {DatabaseSync}=require('node:sqlite');const path=require('node:path');
const db=new DatabaseSync(path.join(process.env.WGR_CACHE_TEST_ROOT,'data/wgr.sqlite'));
const row=db.prepare('SELECT id,json FROM games WHERE external_id=?').get('wgr-orbit');
const game=JSON.parse(row.json);game.entry_uri='cache-test.html';
db.prepare('UPDATE games SET json=? WHERE id=?').run(JSON.stringify(game),row.id);db.close();
'@ | node --disable-warning=ExperimentalWarning
if ($LASTEXITCODE) {throw 'Preparação SQLite falhou.'}
$results=@()
for($run=1;$run -le 3;$run++) {
    $reportPath=Join-Path $project "artifacts/download-cache-$run.json"
    if(Test-Path -LiteralPath $reportPath){Move-Item -LiteralPath $reportPath -Destination ($reportPath+'.'+[guid]::NewGuid().ToString('N')+'.previous')}
    $process=Start-Process -FilePath $exe -ArgumentList @('--data-dir',('"'+$data+'"'),'--smoke',('"'+$reportPath+'"')) -WindowStyle Hidden -PassThru
    if(-not $process.WaitForExit(60000)){$process.Kill();throw 'Somente a fixture de cache excedeu o prazo.'}
    $report=Get-Content -LiteralPath $reportPath -Raw|ConvertFrom-Json
    if(-not $report.passed -or -not $report.first_frame.download_cache.quota -or -not $report.first_frame.download_cache.reopened){throw "Cache não passou: $($report.last_notice)"}
    if(-not $report.first_frame.download_cache.fragments){throw 'Intervalos adjacentes/contidos não passaram.'}
    if($run -gt 1 -and -not $report.first_frame.download_cache.restored){throw 'Cache não persistiu ao fechar e reabrir o processo.'}
    $results += [pscustomobject]@{run=$run;passed=$report.passed;cache=$report.first_frame.download_cache;save_restored=$report.first_frame.restored}
}
$results|ConvertTo-Json -Depth 4|Set-Content -LiteralPath (Join-Path $project 'artifacts/download-cache-summary.json')
$results|ConvertTo-Json -Depth 4
