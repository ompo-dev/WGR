$ErrorActionPreference = 'Stop'
$project = (Resolve-Path (Join-Path $PSScriptRoot '..')).Path
Push-Location $project
try {
    $raw = cargo metadata --locked --format-version 1 --filter-platform x86_64-pc-windows-msvc
    if ($LASTEXITCODE) { throw 'Não foi possível inventariar as dependências.' }
    $metadata = $raw | ConvertFrom-Json -Depth 100
    $byId = @{}
    foreach ($package in $metadata.packages) { $byId[$package.id] = $package }
    $nodes = @{}
    foreach ($node in $metadata.resolve.nodes) { $nodes[$node.id] = $node }
    $active = [Collections.Generic.HashSet[string]]::new()
    $queue = [Collections.Generic.Queue[string]]::new()
    foreach ($member in $metadata.workspace_members) { $queue.Enqueue($member) }
    while ($queue.Count -gt 0) {
        $id = $queue.Dequeue()
        if (-not $active.Add($id)) { continue }
        foreach ($dependency in $nodes[$id].dependencies) { $queue.Enqueue($dependency) }
    }
    $directory = Join-Path $project 'assets/licenses/dependencies'
    New-Item -ItemType Directory -Path $directory -Force | Out-Null
    $lines = [Collections.Generic.List[string]]::new()
    $lines.Add('# Dependências Windows resolvidas')
    $lines.Add('')
    $lines.Add('Gerado a partir do Cargo.lock e metadata da plataforma Windows. Inclui dependências de desenvolvimento/build; não é uma auditoria jurídica ou de vulnerabilidades.')
    $lines.Add('')
    $lines.Add('| Pacote | Versão | Licença declarada | Repositório |')
    $lines.Add('|---|---|---|---|')
    foreach ($package in ($active | ForEach-Object { $byId[$_] } | Where-Object { $_.source } | Sort-Object name,version)) {
        $license = if ($package.license) { $package.license } else { 'Arquivo de licença do pacote' }
        $lines.Add("| $($package.name) | $($package.version) | $license | $($package.repository) |")
        $source = Split-Path -Parent $package.manifest_path
        $destination = Join-Path $directory "$($package.name)-$($package.version)"
        New-Item -ItemType Directory -Path $destination -Force | Out-Null
        foreach ($file in (Get-ChildItem -LiteralPath $source -File | Where-Object { $_.Name -match '^(LICENSE|LICENCE|COPYING|NOTICE|AUTHORS|UNLICENSE)' })) {
            Copy-Item -LiteralPath $file.FullName -Destination (Join-Path $destination $file.Name)
        }
        if ($package.license_file) {
            $licensePath = Join-Path $source $package.license_file
            if (Test-Path -LiteralPath $licensePath) { Copy-Item -LiteralPath $licensePath -Destination $destination }
        }
    }
    [IO.File]::WriteAllLines((Join-Path $project 'docs/DEPENDENCIES.md'), $lines, [Text.UTF8Encoding]::new($false))
    Write-Output "Inventário de $($active.Count) pacotes gravado."
} finally { Pop-Location }
