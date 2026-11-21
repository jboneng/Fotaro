# Build Fotaro's Windows installer: a release build, then Inno Setup.
# The installer lands in target\installer\Fotaro-<version>-windows-x86_64-setup.exe, the
# name the in-app updater looks for on a GitHub release.
#
#   powershell -File installer\build.ps1
#
# Needs Inno Setup 6 (https://jrsoftware.org/isinfo.php, or: winget install JRSoftware.InnoSetup).

$ErrorActionPreference = 'Stop'
$root = Split-Path $PSScriptRoot -Parent
Push-Location $root
try {
    $meta = cargo metadata --no-deps --format-version 1 | ConvertFrom-Json
    $version = ($meta.packages | Where-Object name -eq 'fotaro').version
    Write-Host "Building Fotaro $version"

    cargo build --release -p fotaro
    if ($LASTEXITCODE -ne 0) { throw 'cargo build failed' }

    $iscc = @(
        (Get-Command iscc -ErrorAction SilentlyContinue).Source,
        "${env:ProgramFiles(x86)}\Inno Setup 6\ISCC.exe",
        "$env:ProgramFiles\Inno Setup 6\ISCC.exe",
        "$env:LOCALAPPDATA\Programs\Inno Setup 6\ISCC.exe"
    ) | Where-Object { $_ -and (Test-Path $_) } | Select-Object -First 1
    if (-not $iscc) { throw 'Inno Setup 6 was not found (winget install JRSoftware.InnoSetup)' }

    & $iscc /Q "/DAppVersion=$version" installer\fotaro.iss
    if ($LASTEXITCODE -ne 0) { throw 'Inno Setup failed' }

    $out = Join-Path $root "target\installer\Fotaro-$version-windows-x86_64-setup.exe"
    Write-Host "Installer: $out"
    if ($env:GITHUB_OUTPUT) {
        "version=$version" >> $env:GITHUB_OUTPUT
        "installer=$out" >> $env:GITHUB_OUTPUT
    }
}
finally {
    Pop-Location
}
