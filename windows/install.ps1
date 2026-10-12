# jet installer for Windows (a lightweight alternative to the jet-setup.exe wizard).
# Puts jet.exe in %LOCALAPPDATA%\Programs\jet and adds that folder to the user PATH.
# No administrator rights needed.
#
# Run (in the folder next to jet.exe):
#   right-click install.ps1 -> "Run with PowerShell"
#   or:  powershell -ExecutionPolicy Bypass -File install.ps1

$ErrorActionPreference = 'Stop'

$dest = Join-Path $env:LOCALAPPDATA 'Programs\jet'
$exeSrc = Join-Path $PSScriptRoot 'jet.exe'

if (-not (Test-Path $exeSrc)) {
    Write-Host "jet.exe not found next to this script. Put install.ps1 in the same folder as jet.exe." -ForegroundColor Red
    exit 1
}

New-Item -ItemType Directory -Force -Path $dest | Out-Null
Copy-Item -Force $exeSrc (Join-Path $dest 'jet.exe')

$userPath = [Environment]::GetEnvironmentVariable('Path', 'User')
if ($null -eq $userPath) { $userPath = '' }
if ($userPath -notlike "*$dest*") {
    $newPath = if ($userPath -eq '') { $dest } else { "$userPath;$dest" }
    [Environment]::SetEnvironmentVariable('Path', $newPath, 'User')
    Write-Host "Added to PATH: $dest" -ForegroundColor Green
} else {
    Write-Host "Already on PATH: $dest"
}

Write-Host ""
Write-Host "Done. Installed: $dest\jet.exe" -ForegroundColor Green
Write-Host "Open a NEW terminal window and type:  jet help"
Write-Host ""

# jet goes to the network with the system curl, and unpacks with the system tar (or 7-Zip if tar is missing).
if (-not (Get-Command curl.exe -ErrorAction SilentlyContinue)) {
    Write-Host "WARNING: curl.exe not found — without it jet can't reach the registry." -ForegroundColor Yellow
}
$hasTar = [bool](Get-Command tar.exe -ErrorAction SilentlyContinue)
$hasSeven = (Test-Path "$env:ProgramFiles\7-Zip\7z.exe") -or (Test-Path "${env:ProgramFiles(x86)}\7-Zip\7z.exe") -or [bool](Get-Command 7z -ErrorAction SilentlyContinue)
if (-not $hasTar -and -not $hasSeven) {
    Write-Host "WARNING: neither tar.exe nor 7-Zip is present — nothing to unpack packages with." -ForegroundColor Yellow
    Write-Host "Install 7-Zip (7-zip.org) or use Windows with the stock tar.exe."
} elseif (-not $hasTar) {
    Write-Host "tar.exe is not present — unpacking will go through 7-Zip (the standard fallback path)."
}
