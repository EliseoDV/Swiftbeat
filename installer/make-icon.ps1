# Copia a assets/ el icono de la app del kit de marca (IconInfo/windows):
# swiftbeat.ico (16-256 px, con el corte pequeno hasta 32 px) y swiftbeat.png (256 px).
# Los originales se generan con IconInfo/fuente (ver IconInfo/README.md).
# Uso: powershell -ExecutionPolicy Bypass -File installer\make-icon.ps1
$root = Join-Path $PSScriptRoot ".."
$source = Join-Path $root "IconInfo\windows"
$assets = Join-Path $root "assets"
New-Item -ItemType Directory -Force $assets | Out-Null
foreach ($name in "swiftbeat.ico", "swiftbeat.png") {
    Copy-Item (Join-Path $source $name) (Join-Path $assets $name) -Force
    Write-Host "assets\$name actualizado"
}
