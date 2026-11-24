param([Parameter(Mandatory=$true)][string]$Zip)
$ErrorActionPreference='Stop'
Add-Type -AssemblyName System.IO.Compression
Add-Type -AssemblyName System.IO.Compression.FileSystem
$path=(Resolve-Path -LiteralPath $Zip).Path
$sums=[IO.Path]::ChangeExtension($path,'.sha256')
$hashes=@{}
foreach($line in Get-Content -LiteralPath $sums){
    if($line -notmatch '^([0-9a-fA-F]{64})  ([^/\\]+)$'){throw "Checksum inválido: $line"}
    if($hashes.ContainsKey($Matches[2])){throw 'Checksum duplicado.'}
    $hashes[$Matches[2]]=$Matches[1].ToLowerInvariant()
}
if($hashes[[IO.Path]::GetFileName($path)] -ne (Get-FileHash -LiteralPath $path -Algorithm SHA256).Hash.ToLowerInvariant()){throw 'Checksum do ZIP não confere.'}
$archive=[IO.Compression.ZipFile]::OpenRead($path)
try {
    $prefix=[IO.Path]::GetFileNameWithoutExtension($path) + '/'
    # Windows ZipFile may emit backslash separators on older .NET versions.
    $entries=@{}
    foreach($entry in $archive.Entries){
        $name=$entry.FullName.Replace('\','/')
        if($entries.ContainsKey($name)){throw "Entrada duplicada: $name"}
        $entries[$name]=$entry
    }
    foreach($name in @('WGR.exe','wgr-rage.exe','portable.flag','BUILD.txt','WGR-source.zip','LICENSE','THIRD_PARTY_NOTICES.md','docs/DEPENDENCIES.md')){
        $entry=$entries[$prefix+$name]
        if(-not $entry -or $entry.Length -le 0){throw "Arquivo ausente ou vazio: $name"}
    }
    foreach($name in @('WGR.exe','wgr-rage.exe')){
        $stream=$entries[$prefix+$name].Open()
        $sha=[Security.Cryptography.SHA256]::Create()
        try{$hash=[BitConverter]::ToString($sha.ComputeHash($stream)).Replace('-','').ToLowerInvariant()}
        finally{$sha.Dispose();$stream.Dispose()}
        if($hashes[$name] -ne $hash){throw "Checksum interno de $name não confere."}
    }
}finally{$archive.Dispose()}
Write-Output "Pacote validado: $path (launcher, runtime nativo e ZIP)."
