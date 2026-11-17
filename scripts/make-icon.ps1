# Draws the Tether mark (a lens with a cable) and writes:
#   assets\icon-256.png   window icon
#   assets\tether.ico     exe icon (16-256 px, PNG-compressed entries)
# Same geometry as the 512-unit app icon: rounded black square, white ring, dot, cable.

$ErrorActionPreference = 'Stop'
Add-Type -AssemblyName System.Drawing
$root = Split-Path $PSScriptRoot -Parent
$assets = Join-Path $root 'assets'
New-Item -ItemType Directory $assets -Force | Out-Null

function Draw-Icon([int]$size) {
    $bmp = New-Object System.Drawing.Bitmap $size, $size, ([System.Drawing.Imaging.PixelFormat]::Format32bppArgb)
    $g = [System.Drawing.Graphics]::FromImage($bmp)
    $g.SmoothingMode = 'AntiAlias'
    $g.PixelOffsetMode = 'HighQuality'
    $g.Clear([System.Drawing.Color]::Transparent)
    $u = $size / 512.0
    $black = New-Object System.Drawing.SolidBrush ([System.Drawing.Color]::FromArgb(255, 10, 10, 10))
    $white = New-Object System.Drawing.SolidBrush ([System.Drawing.Color]::FromArgb(255, 250, 250, 250))

    # Rounded square
    $r = 112 * $u; $d = 2 * $r; $s = $size - 0.01
    $path = New-Object System.Drawing.Drawing2D.GraphicsPath
    $path.AddArc(0, 0, $d, $d, 180, 90); $path.AddArc($s - $d, 0, $d, $d, 270, 90)
    $path.AddArc($s - $d, $s - $d, $d, $d, 0, 90); $path.AddArc(0, $s - $d, $d, $d, 90, 90)
    $path.CloseFigure()
    $g.FillPath($black, $path)

    # Small sizes get thicker strokes so the mark stays readable.
    $boost = if ($size -le 24) { 1.35 } elseif ($size -le 48) { 1.15 } else { 1.0 }
    $pen = New-Object System.Drawing.Pen ([System.Drawing.Color]::FromArgb(255, 250, 250, 250)), (36 * $u * $boost)
    $pen.StartCap = 'Round'; $pen.EndCap = 'Round'
    $g.DrawEllipse($pen, (144 * $u), (104 * $u), (224 * $u), (224 * $u))
    $dot = 34 * $u * $boost
    $g.FillEllipse($white, (256 * $u - $dot), (216 * $u - $dot), (2 * $dot), (2 * $dot))
    $g.DrawLine($pen, (256 * $u), (346 * $u), (256 * $u), (430 * $u))
    $g.Dispose()
    return $bmp
}

$sizes = 16, 24, 32, 48, 64, 128, 256
$pngs = @{}
foreach ($sz in $sizes) {
    $bmp = Draw-Icon $sz
    $ms = New-Object System.IO.MemoryStream
    $bmp.Save($ms, [System.Drawing.Imaging.ImageFormat]::Png)
    $pngs[$sz] = $ms.ToArray()
    if ($sz -eq 256) {
        $bmp.Save((Join-Path $assets 'icon-256.png'), [System.Drawing.Imaging.ImageFormat]::Png)
        $big = Draw-Icon 512; $big.Save((Join-Path $assets 'icon-512.png'), [System.Drawing.Imaging.ImageFormat]::Png); $big.Dispose()
    }
    $bmp.Dispose()
}

# ICO container: header, directory, then the PNG blobs.
$out = New-Object System.IO.MemoryStream
$w = New-Object System.IO.BinaryWriter $out
$w.Write([uint16]0); $w.Write([uint16]1); $w.Write([uint16]$sizes.Count)
$offset = 6 + 16 * $sizes.Count
foreach ($sz in $sizes) {
    $b = if ($sz -ge 256) { 0 } else { $sz }
    $w.Write([byte]$b); $w.Write([byte]$b); $w.Write([byte]0); $w.Write([byte]0)
    $w.Write([uint16]1); $w.Write([uint16]32)
    $w.Write([uint32]$pngs[$sz].Length); $w.Write([uint32]$offset)
    $offset += $pngs[$sz].Length
}
foreach ($sz in $sizes) { $w.Write($pngs[$sz]) }
$w.Flush()
[IO.File]::WriteAllBytes((Join-Path $assets 'tether.ico'), $out.ToArray())
Write-Host "Wrote assets\icon-256.png and assets\tether.ico"
