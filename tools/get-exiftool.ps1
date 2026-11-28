# Downloads the Windows ExifTool (64-bit) into tools\exiftool.
# The archive (from SourceForge, where exiftool.org links) is verified against the SHA2-256 sum
# published on exiftool.org. Old releases disappear, so the latest version is used by default.
# An existing exiftool.exe in the target folder is kept unless -Force is given.
param(
    [string]$Target = (Join-Path $PSScriptRoot 'exiftool'),
    [string]$Version,
    [switch]$Force
)

$ErrorActionPreference = 'Stop'
$ProgressPreference = 'SilentlyContinue'
$TargetExe = Join-Path $Target 'exiftool.exe'
if ((Test-Path -LiteralPath $TargetExe -PathType Leaf) -and -not $Force) {
    Write-Output "ExifTool already present: $TargetExe"
    return
}

$Base = 'https://exiftool.org'
if (-not $Version) { $Version = (Invoke-WebRequest "$Base/ver.txt" -UseBasicParsing).Content.Trim() }
if ($Version -notmatch '^\d+\.\d+$') { throw "Unexpected ExifTool version: $Version" }
$Name = "exiftool-${Version}_64.zip"

$Checksums = (Invoke-WebRequest "$Base/checksums-$Version.txt" -UseBasicParsing).Content
$Line = ($Checksums -split "`n") | Where-Object { $_ -like "SHA2-256($Name)=*" } | Select-Object -First 1
if (-not $Line) { throw "No SHA2-256 checksum for $Name on $Base/checksums.txt" }
$Expected = ($Line -split '=', 2)[1].Trim().ToLowerInvariant()

$Work = Join-Path ([IO.Path]::GetTempPath()) ("ddreams-exiftool-" + [Guid]::NewGuid().ToString('N'))
New-Item -ItemType Directory -Path $Work | Out-Null
try {
    $Zip = Join-Path $Work $Name
    # The archives are hosted on SourceForge; curl.exe (part of Windows 10+) gets the file
    # directly, while Invoke-WebRequest may receive SourceForge's HTML download page instead.
    $Url = "https://sourceforge.net/projects/exiftool/files/$Name/download"
    Write-Output "Downloading $Url ..."
    & curl.exe -fsSL -o $Zip $Url
    if ($LASTEXITCODE -ne 0) { throw "Download failed ($LASTEXITCODE): $Url" }
    $Actual = (Get-FileHash -LiteralPath $Zip -Algorithm SHA256).Hash.ToLowerInvariant()
    if ($Actual -ne $Expected) { throw "Checksum mismatch for ${Name}: expected $Expected, got $Actual" }

    $Extracted = Join-Path $Work 'x'
    Expand-Archive -LiteralPath $Zip -DestinationPath $Extracted
    $Root = Join-Path $Extracted "exiftool-${Version}_64"
    $SourceExe = Join-Path $Root 'exiftool(-k).exe'
    $SourceFiles = Join-Path $Root 'exiftool_files'
    if (-not (Test-Path -LiteralPath $SourceExe -PathType Leaf) -or -not (Test-Path -LiteralPath $SourceFiles -PathType Container)) {
        throw "Unexpected archive layout in $Name"
    }

    New-Item -ItemType Directory -Path $Target -Force | Out-Null
    $TargetFiles = Join-Path $Target 'exiftool_files'
    if (Test-Path -LiteralPath $TargetFiles) { Remove-Item -LiteralPath $TargetFiles -Recurse -Force }
    Copy-Item -LiteralPath $SourceFiles -Destination $TargetFiles -Recurse
    # The "(-k)" suffix makes ExifTool wait for a key press; DDreams runs it as exiftool.exe.
    Copy-Item -LiteralPath $SourceExe -Destination $TargetExe -Force
    Write-Output "Installed ExifTool $Version`: $TargetExe"
} finally {
    Remove-Item -LiteralPath $Work -Recurse -Force -ErrorAction SilentlyContinue
}
