# Creates the Python environment used by the AI server and the RAW plugin,
# both in development and in the portable installation.
param(
    [string]$Target = 'G:\DDreams\modules\python',
    [string]$BasePython = (Join-Path $env:LOCALAPPDATA 'Programs\Python\Python314\python.exe')
)

$ErrorActionPreference = 'Stop'
$RepoRoot = Split-Path -Parent $PSScriptRoot
$Python = Join-Path $Target 'Scripts\python.exe'
if (-not (Test-Path -LiteralPath $Python -PathType Leaf)) {
    if (-not (Test-Path -LiteralPath $BasePython -PathType Leaf)) { throw "Base Python not found: $BasePython" }
    & $BasePython -m venv $Target
    if ($LASTEXITCODE -ne 0) { throw 'venv creation failed' }
}
& $Python -m pip install --disable-pip-version-check -r (Join-Path $RepoRoot 'ai\requirements.txt')
if ($LASTEXITCODE -ne 0) { throw 'pip install failed' }
& $Python -c "import torch, open_clip, rawpy, PIL, pillow_heif, numpy, transformers, sentencepiece; print('torch', torch.__version__, 'cuda', torch.cuda.is_available())"
if ($LASTEXITCODE -ne 0) { throw 'import check failed' }
Write-Output "Python environment ready: $Target"
