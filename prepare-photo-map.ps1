[CmdletBinding()]
param(
    [Parameter(Mandatory)][ValidateNotNullOrEmpty()][string]$ExeDirectory,
    [string]$MapRepo = 'D:\mytools\3wa_webgl_map'
)

$ErrorActionPreference = 'Stop'
$pin = 'bef84f8c2ff5ff237e2f918ef2bba6b00667ef53'
$assets = [ordered]@{
    'easymap.js' = '44E5FC2833D3F750802EEE36916EDA915C0F018CE8F810AF8CB83ADD780603AE'
    'dg_webgl_point.js' = '1125DBD5E9FFB40DF6B03C4DA57CAB6BC35C30629A04AE0B6D766523E5AA101F'
}
$root = (Get-Item -LiteralPath $ExeDirectory -ErrorAction Stop).FullName
if (-not (Test-Path -LiteralPath (Join-Path $root 'mypicsee.exe') -PathType Leaf)) {
    throw 'Specify the folder containing the isolated mypicsee.exe build.'
}
$destination = Join-Path $root 'picsee-map-assets'
$archive = Join-Path ([IO.Path]::GetTempPath()) ('picsee-map-'+[guid]::NewGuid()+'.zip')
try {
    $names = @($assets.Keys | ForEach-Object { 'js/easymap_webgl/'+$_ })
    & git -C $MapRepo archive --format=zip -o $archive $pin -- @names
    if ($LASTEXITCODE -ne 0) { throw "Can't export pinned local map commit $pin." }
    Add-Type -AssemblyName System.IO.Compression.FileSystem
    $zip = [IO.Compression.ZipFile]::OpenRead($archive)
    try {
        $bytes = @{}
        foreach ($name in $assets.Keys) {
            $entry = $zip.GetEntry('js/easymap_webgl/'+$name)
            if (-not $entry -or $entry.Length -gt 512KB) { throw "Missing or oversized asset: $name" }
            $input = $entry.Open()
            $memory = [IO.MemoryStream]::new()
            try { $input.CopyTo($memory); $bytes[$name] = $memory.ToArray() }
            finally { $input.Dispose(); $memory.Dispose() }
            $sha = [Security.Cryptography.SHA256]::Create()
            try { $actual = [BitConverter]::ToString($sha.ComputeHash($bytes[$name])).Replace('-', '') }
            finally { $sha.Dispose() }
            if ($actual -ne $assets[$name]) { throw "Pinned asset did not match: $name" }
            $existing = Join-Path $destination $name
            if ((Test-Path -LiteralPath $existing) -and (Get-FileHash -LiteralPath $existing -Algorithm SHA256).Hash -ne $assets[$name]) {
                throw "Won't overwrite a different local asset: $existing"
            }
        }
        New-Item -ItemType Directory -Force -Path $destination | Out-Null
        foreach ($name in $assets.Keys) {
            $path = Join-Path $destination $name
            if (-not (Test-Path -LiteralPath $path)) {
                $file = [IO.File]::Open($path, [IO.FileMode]::CreateNew)
                try { $file.Write($bytes[$name], 0, $bytes[$name].Length) }
                finally { $file.Dispose() }
            }
        }
        Write-Output "Prepared pinned local assets in $destination"
    } finally { $zip.Dispose() }
} finally {
    if (Test-Path -LiteralPath $archive) { Remove-Item -LiteralPath $archive }
}
