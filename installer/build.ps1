# Compila Swiftbeat en release y genera installer\Output\Swiftbeat-Setup-<versión>.exe
# Uso: powershell -ExecutionPolicy Bypass -File installer\build.ps1 [-SkipBuild]
param([switch]$SkipBuild)
$ErrorActionPreference = 'Stop'
$root = Split-Path $PSScriptRoot -Parent

$version = (Select-String -Path (Join-Path $root 'Cargo.toml') -Pattern '^version\s*=\s*"([^"]+)"' | Select-Object -First 1).Matches[0].Groups[1].Value
Write-Host "Swiftbeat $version"

if (-not $SkipBuild) {
    Push-Location $root
    try {
        cargo build --release
        if ($LASTEXITCODE -ne 0) { throw "cargo build falló" }
    } finally { Pop-Location }
}

$iscc = @(
    (Get-Command iscc -ErrorAction SilentlyContinue).Source,
    "$env:LOCALAPPDATA\Programs\Inno Setup 6\ISCC.exe",
    "${env:ProgramFiles(x86)}\Inno Setup 6\ISCC.exe",
    "$env:ProgramFiles\Inno Setup 6\ISCC.exe"
) | Where-Object { $_ -and (Test-Path $_) } | Select-Object -First 1
if (-not $iscc) { throw "No se encontró Inno Setup 6. Instálalo con: winget install JRSoftware.InnoSetup" }

& $iscc "/DAppVersion=$version" (Join-Path $PSScriptRoot 'swiftbeat.iss')
if ($LASTEXITCODE -ne 0) { throw "ISCC falló" }
Get-Item (Join-Path $PSScriptRoot "Output\Swiftbeat-Setup-$version.exe")
