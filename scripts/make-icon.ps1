<#
.SYNOPSIS
    Draws the app icon (signal bars on a dark tile) and writes assets\app.ico.

.DESCRIPTION
    The icon is drawn from code rather than kept only as a binary, so it can be
    changed and regenerated. Each size is drawn separately, snapped to whole
    pixels, so the bars stay crisp at 16px instead of being a blurred scale-down.
    Run it after changing the design and commit the .ico.
#>
$ErrorActionPreference = 'Stop'
Add-Type -AssemblyName System.Drawing

$root = Split-Path $PSScriptRoot
$out = Join-Path $root 'assets\app.ico'
$sizes = 16, 20, 24, 32, 40, 48, 64, 256

$tile = [System.Drawing.Color]::FromArgb(0x1F, 0x23, 0x28)
$on = [System.Drawing.Color]::FromArgb(0x3D, 0xDC, 0x84)
$off = [System.Drawing.Color]::FromArgb(0x5F, 0x63, 0x68)

function New-RoundedRect([float]$x, [float]$y, [float]$w, [float]$h, [float]$r) {
    $path = New-Object System.Drawing.Drawing2D.GraphicsPath
    if ($r -le 0) { $path.AddRectangle((New-Object System.Drawing.RectangleF $x, $y, $w, $h)); return $path }
    $d = 2 * $r
    $path.AddArc($x, $y, $d, $d, 180, 90)
    $path.AddArc($x + $w - $d, $y, $d, $d, 270, 90)
    $path.AddArc($x + $w - $d, $y + $h - $d, $d, $d, 0, 90)
    $path.AddArc($x, $y + $h - $d, $d, $d, 90, 90)
    $path.CloseFigure()
    $path
}

function Get-IconPng([int]$size) {
    $bmp = New-Object System.Drawing.Bitmap $size, $size, ([System.Drawing.Imaging.PixelFormat]::Format32bppArgb)
    $g = [System.Drawing.Graphics]::FromImage($bmp)
    $g.SmoothingMode = 'AntiAlias'
    $g.PixelOffsetMode = 'HighQuality'
    $g.Clear([System.Drawing.Color]::Transparent)

    # The design is on an 80-unit grid (as in the concept sketch); edges are
    # rounded to whole pixels at each size.
    $s = $size / 80.0
    $g.FillPath((New-Object System.Drawing.SolidBrush $tile), (New-RoundedRect 0 0 $size $size ([Math]::Max(2, [Math]::Round(18 * $s)))))

    # Four bars, rising left to right; the last is grey ("room to improve").
    $barW = [Math]::Max(2, [Math]::Round(10 * $s))
    $gap = [Math]::Max(1, [Math]::Round(5 * $s))
    $total = 4 * $barW + 3 * $gap
    $left = [Math]::Round(($size - $total) / 2.0)
    $bottom = [Math]::Round(64 * $s)
    $maxH = $bottom - [Math]::Round(14 * $s)
    $radius = if ($size -ge 32) { [Math]::Round(2 * $s) } else { 0 }
    for ($i = 0; $i -lt 4; $i++) {
        $h = [Math]::Max(2, [Math]::Round($maxH * ($i + 1) / 4.0))
        $x = $left + $i * ($barW + $gap)
        $color = if ($i -lt 3) { $on } else { $off }
        $g.FillPath((New-Object System.Drawing.SolidBrush $color), (New-RoundedRect $x ($bottom - $h) $barW $h $radius))
    }
    $g.Dispose()

    $stream = New-Object System.IO.MemoryStream
    $bmp.Save($stream, [System.Drawing.Imaging.ImageFormat]::Png)
    $bmp.Dispose()
    , $stream.ToArray()
}

# ICO container with PNG-compressed images (supported since Windows Vista).
$images = foreach ($size in $sizes) { , (Get-IconPng $size) }
$file = New-Object System.IO.MemoryStream
$writer = New-Object System.IO.BinaryWriter $file
$writer.Write([uint16]0); $writer.Write([uint16]1); $writer.Write([uint16]$sizes.Count)
$offset = 6 + 16 * $sizes.Count
for ($i = 0; $i -lt $sizes.Count; $i++) {
    $dim = if ($sizes[$i] -ge 256) { 0 } else { $sizes[$i] }
    $writer.Write([byte]$dim); $writer.Write([byte]$dim)
    $writer.Write([byte]0); $writer.Write([byte]0)
    $writer.Write([uint16]1); $writer.Write([uint16]32)
    $writer.Write([uint32]$images[$i].Length); $writer.Write([uint32]$offset)
    $offset += $images[$i].Length
}
foreach ($image in $images) { $writer.Write($image) }
$writer.Flush()

New-Item -ItemType Directory -Force (Split-Path $out) | Out-Null
[IO.File]::WriteAllBytes($out, $file.ToArray())
Write-Host "Wrote $out ($($sizes -join ', ') px)"
