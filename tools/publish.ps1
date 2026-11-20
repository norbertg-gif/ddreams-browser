param(
    [string]$Catalog,
    [string]$InstallRoot = 'C:\DDreams',
    [string]$ExifToolSource = (Join-Path (Split-Path -Parent $PSScriptRoot) 'tools\exiftool')
)

$ErrorActionPreference = 'Stop'
$RepoRoot = Split-Path -Parent $PSScriptRoot
$ExeCandidates = @(
    (Join-Path $RepoRoot 'src-tauri\target\release\ddreams-browser.exe'),
    (Join-Path $RepoRoot 'target\release\ddreams-browser.exe')
)
$ReleaseExe = $ExeCandidates | Where-Object { Test-Path -LiteralPath $_ -PathType Leaf } | Select-Object -First 1
if (-not $ReleaseExe) { throw 'Release exe not found. Build the app first; this script does not build it.' }

$InstallExe = Join-Path $InstallRoot 'ddreams-browser.exe'
$CatalogDir = Join-Path $InstallRoot 'catalog'
$InstallCatalog = Join-Path $CatalogDir 'catalog.sqlite'
$BackupDir = Join-Path $InstallRoot 'backups'
$CacheDir = Join-Path $InstallRoot 'cache'
$ModulesDir = Join-Path $InstallRoot 'modules'
$InstallExifToolDir = Join-Path $ModulesDir 'exiftool'
$InstallAiDir = Join-Path $ModulesDir 'ai'
$InstallExifTool = Join-Path $InstallExifToolDir 'exiftool.exe'
$SourceExifTool = Join-Path $ExifToolSource 'exiftool.exe'
if (-not (Test-Path -LiteralPath $SourceExifTool -PathType Leaf)) { throw "ExifTool source not found: $SourceExifTool" }
$catalogExists = Test-Path -LiteralPath $InstallCatalog -PathType Leaf
if ($catalogExists -and $Catalog) { throw "Refusing to overwrite existing DDreams catalog: $InstallCatalog" }
if (-not $catalogExists) {
    if (-not $Catalog) { throw 'First install requires -Catalog <path>.' }
    if (-not (Test-Path -LiteralPath $Catalog -PathType Leaf)) { throw "Catalog source not found: $Catalog" }
}

$runningInstall = Get-Process -Name 'ddreams-browser' -ErrorAction SilentlyContinue | Where-Object {
    try { $_.Path -and ([IO.Path]::GetFullPath($_.Path) -ieq [IO.Path]::GetFullPath($InstallExe)) } catch { $false }
}
if ($runningInstall) { throw "Close $InstallExe before publishing." }

foreach ($directory in @($InstallRoot, $CatalogDir, $BackupDir, $CacheDir, $ModulesDir)) {
    New-Item -ItemType Directory -Path $directory -Force | Out-Null
}
New-Item -ItemType Directory -Path $InstallAiDir -Force | Out-Null
Copy-Item -LiteralPath $ReleaseExe -Destination $InstallExe -Force
Write-Output "Copied app: $InstallExe"

if (-not (Test-Path -LiteralPath $InstallCatalog -PathType Leaf)) {
    Copy-Item -LiteralPath $Catalog -Destination $InstallCatalog
    Write-Output "Installed catalog: $InstallCatalog"
} else {
    Write-Output "Preserved existing catalog: $InstallCatalog"
}

$SourceAiScript = Join-Path $RepoRoot 'ai\ddreams_ai.py'
if (-not (Test-Path -LiteralPath $SourceAiScript -PathType Leaf)) { throw "AI server script not found: $SourceAiScript" }
$InstallAiScript = Join-Path $InstallAiDir 'ddreams_ai.py'
Copy-Item -LiteralPath $SourceAiScript -Destination $InstallAiScript -Force
Write-Output "Copied AI server: $InstallAiScript"
Copy-Item -LiteralPath (Join-Path $RepoRoot 'ai\faces_schema.py') -Destination (Join-Path $InstallAiDir 'faces_schema.py') -Force
Write-Output "Copied faces schema: $(Join-Path $InstallAiDir 'faces_schema.py')"
# RAW preview plugin (LibRaw via rawpy); uses the Python runtime from modules\ai\ai.json.
$SourceRawScript = Join-Path $RepoRoot 'raw\ddreams_raw.py'
$InstallRawDir = Join-Path $ModulesDir 'raw'
New-Item -ItemType Directory -Path $InstallRawDir -Force | Out-Null
Copy-Item -LiteralPath $SourceRawScript -Destination (Join-Path $InstallRawDir 'ddreams_raw.py') -Force
Write-Output "Copied RAW plugin: $(Join-Path $InstallRawDir 'ddreams_raw.py')"
$AiConfig = Join-Path $InstallAiDir 'ai.json'
# The Python environment is created by tools\setup-python.ps1. An ai.json that still points
# outside the installation at the old shared environment is switched over.
$InstallPython = Join-Path $ModulesDir 'python\Scripts\python.exe'
# Relative to modules\ai, so the whole installation folder can be moved.
$RelativePython = '..\python\Scripts\python.exe'
if (-not (Test-Path -LiteralPath $InstallPython -PathType Leaf)) { throw "Python environment not found: $InstallPython. Run tools\setup-python.ps1 first." }
if (-not (Test-Path -LiteralPath $AiConfig -PathType Leaf) -or (Get-Content -LiteralPath $AiConfig -Raw | ConvertFrom-Json).python -ne $RelativePython) {
    [IO.File]::WriteAllText($AiConfig, (@{ python = $RelativePython } | ConvertTo-Json -Compress), [Text.UTF8Encoding]::new($false))
    Write-Output "Wrote AI config: $AiConfig"
}

$SourceSupport = Join-Path $ExifToolSource 'exiftool_files'
$copyExifTool = -not (Test-Path -LiteralPath $InstallExifTool -PathType Leaf)
if (-not $copyExifTool) {
    $copyExifTool = (Get-Item -LiteralPath $SourceExifTool).LastWriteTimeUtc -gt (Get-Item -LiteralPath $InstallExifTool).LastWriteTimeUtc
}
if ($SourceSupport -and (Test-Path -LiteralPath $SourceSupport -PathType Container) -and -not (Test-Path -LiteralPath (Join-Path $InstallExifToolDir 'exiftool_files') -PathType Container)) {
    $copyExifTool = $true
}
if ($copyExifTool) {
    New-Item -ItemType Directory -Path $InstallExifToolDir -Force | Out-Null
    Copy-Item -LiteralPath $SourceExifTool -Destination $InstallExifTool -Force
    if (Test-Path -LiteralPath $SourceSupport -PathType Container) {
        $DestinationSupport = Join-Path $InstallExifToolDir 'exiftool_files'
        New-Item -ItemType Directory -Path $DestinationSupport -Force | Out-Null
        Copy-Item -Path (Join-Path $SourceSupport '*') -Destination $DestinationSupport -Recurse -Force
    }
    Write-Output "Copied/updated ExifTool: $InstallExifTool"
} else {
    Write-Output "ExifTool is current: $InstallExifTool"
}

# A copy once left a file full of zero bytes; verify what was published.
foreach ($pair in @(
    @($ReleaseExe, $InstallExe),
    @($SourceAiScript, $InstallAiScript),
    @((Join-Path $RepoRoot 'ai\faces_schema.py'), (Join-Path $InstallAiDir 'faces_schema.py')),
    @($SourceRawScript, (Join-Path $InstallRawDir 'ddreams_raw.py'))
)) {
    if ((Get-FileHash -LiteralPath $pair[0]).Hash -ne (Get-FileHash -LiteralPath $pair[1]).Hash) { throw "Published file differs from its source: $($pair[1])" }
}
Write-Output "Portable installation ready: $InstallRoot"
