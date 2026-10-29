param([Parameter(Mandatory)][string]$SourceRoot,[switch]$Write)
$ErrorActionPreference='Stop'
$project=(Resolve-Path (Join-Path $PSScriptRoot '..')).Path
$reference=(Resolve-Path -LiteralPath $SourceRoot).Path
$destination=Join-Path $project 'vendor/momor'
$crates=@('ui','ui_input','theme','component','icons','menu','ui_macros','syntax_theme','gpui_util','collections','refineable')
$operations=[Collections.Generic.List[object]]::new()
foreach($crate in $crates) {
    $folder=Join-Path $reference "crates/$crate"
    foreach($file in Get-ChildItem -LiteralPath $folder -Recurse -File) {
        if($file.LinkType){throw 'COPY rejeita links simbólicos.'}
        $relative=[IO.Path]::GetRelativePath($reference,$file.FullName)
        if($relative -match '(^|[\\/])(target|\.git|\.env)([\\/]|$)'){continue}
        $target=Join-Path $destination $relative
        if(-not $target.StartsWith($project+[IO.Path]::DirectorySeparatorChar,[StringComparison]::OrdinalIgnoreCase)){throw 'Destino fora do WGR.'}
        $operations.Add([ordered]@{operation='COPY_FILE';source=$relative.Replace('\','/');target=[IO.Path]::GetRelativePath($project,$target).Replace('\','/');sha256=(Get-FileHash -LiteralPath $file.FullName -Algorithm SHA256).Hash.ToLower();bytes=$file.Length})
    }
}
$plan=Join-Path $project 'docs/momor-copy-plan.json'
if(-not $Write){
    [ordered]@{mode='dry-run';source_commit=(git -C $reference rev-parse HEAD);source_root=$reference;destination_root=$destination;operations=$operations}|ConvertTo-Json -Depth 8|Set-Content -LiteralPath $plan -Encoding utf8
    $totalBytes=($operations|ForEach-Object { $_.bytes }|Measure-Object -Sum).Sum
    Write-Output "Dry-run: $($operations.Count) arquivos; $([math]::Round($totalBytes/1048576,2)) MiB. Plano: $plan"
    exit 0
}
$saved=Get-Content -LiteralPath $plan -Raw|ConvertFrom-Json
foreach($operation in $saved.operations){
    $source=Join-Path $reference $operation.source;$target=Join-Path $project $operation.target
    if((Get-FileHash -LiteralPath $source -Algorithm SHA256).Hash.ToLower() -ne $operation.sha256){throw "Origem mudou: $($operation.source)"}
    if(Test-Path -LiteralPath $target){throw "Destino existente preservado: $($operation.target)"}
}
foreach($operation in $saved.operations){$source=Join-Path $reference $operation.source;$target=Join-Path $project $operation.target;New-Item -ItemType Directory -Path (Split-Path $target) -Force|Out-Null;[IO.File]::Copy($source,$target,$false);if((Get-FileHash -LiteralPath $target -Algorithm SHA256).Hash.ToLower() -ne $operation.sha256){throw 'Hash da cópia divergente.'}}
Write-Output "COPY concluído: $($saved.operations.Count) arquivos com hashes conferidos. Origem preservada."
