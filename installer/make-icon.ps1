# Genera assets/swiftbeat.png (256 px) y assets/swiftbeat.ico (16-256 px):
# una "S" negra en mayuscula sobre un circulo #444554.
# Uso: powershell -ExecutionPolicy Bypass -File installer\make-icon.ps1
Add-Type -AssemblyName System.Drawing
$assets = Join-Path $PSScriptRoot "..\assets"
New-Item -ItemType Directory -Force $assets | Out-Null

function New-Logo([int]$size) {
    $bmp = New-Object System.Drawing.Bitmap $size, $size
    $g = [System.Drawing.Graphics]::FromImage($bmp)
    $g.SmoothingMode = 'AntiAlias'
    $g.TextRenderingHint = 'AntiAliasGridFit'
    $g.Clear([System.Drawing.Color]::Transparent)
    $bg = New-Object System.Drawing.SolidBrush ([System.Drawing.Color]::FromArgb(0x44, 0x45, 0x54))
    $g.FillEllipse($bg, 0, 0, $size - 1, $size - 1)
    $font = New-Object System.Drawing.Font("Segoe UI Black", [single]($size * 0.86), [System.Drawing.FontStyle]::Bold, [System.Drawing.GraphicsUnit]::Pixel)
    $fmt = New-Object System.Drawing.StringFormat
    $fmt.Alignment = 'Center'
    $fmt.LineAlignment = 'Center'
    $rect = New-Object System.Drawing.RectangleF 0, ([single](-$size * 0.02)), $size, $size
    $g.DrawString("S", $font, [System.Drawing.Brushes]::Black, $rect, $fmt)
    $g.Dispose()
    return $bmp
}

function Get-PngBytes($bmp) {
    $ms = New-Object System.IO.MemoryStream
    $bmp.Save($ms, [System.Drawing.Imaging.ImageFormat]::Png)
    return , $ms.ToArray()
}

$big = New-Logo 256
$big.Save((Join-Path $assets "swiftbeat.png"), [System.Drawing.Imaging.ImageFormat]::Png)

# ICO con entradas PNG (formato admitido desde Windows Vista).
$sizes = 16, 24, 32, 48, 64, 128, 256
$images = foreach ($s in $sizes) { , (Get-PngBytes (New-Logo $s)) }
$out = New-Object System.IO.MemoryStream
$w = New-Object System.IO.BinaryWriter $out
$w.Write([uint16]0); $w.Write([uint16]1); $w.Write([uint16]$sizes.Count)
$offset = 6 + 16 * $sizes.Count
for ($i = 0; $i -lt $sizes.Count; $i++) {
    $s = $sizes[$i]; $len = $images[$i].Length
    $dim = if ($s -ge 256) { 0 } else { $s }
    $w.Write([byte]$dim); $w.Write([byte]$dim); $w.Write([byte]0); $w.Write([byte]0)
    $w.Write([uint16]1); $w.Write([uint16]32); $w.Write([uint32]$len); $w.Write([uint32]$offset)
    $offset += $len
}
foreach ($img in $images) { $w.Write($img) }
$w.Flush()
[System.IO.File]::WriteAllBytes((Join-Path $assets "swiftbeat.ico"), $out.ToArray())
"Iconos generados en $assets"
