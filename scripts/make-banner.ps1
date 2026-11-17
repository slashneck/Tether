# Draws docs\banner.png (README header, same layout as the other apps' banners) and docs\social-preview.png.

$ErrorActionPreference = 'Stop'
Add-Type -AssemblyName System.Drawing
$root = Split-Path $PSScriptRoot -Parent
$docs = Join-Path $root 'docs'
New-Item -ItemType Directory $docs -Force | Out-Null

$tagline = 'One camera for every app.'
$tags = @('Webcam or phone', 'Cover hotkey', 'Free and open source')

function RoundedRect([float]$x, [float]$y, [float]$w, [float]$h, [float]$r) {
    $p = New-Object System.Drawing.Drawing2D.GraphicsPath
    $d = 2 * $r
    $p.AddArc($x, $y, $d, $d, 180, 90); $p.AddArc($x + $w - $d, $y, $d, $d, 270, 90)
    $p.AddArc($x + $w - $d, $y + $h - $d, $d, $d, 0, 90); $p.AddArc($x, $y + $h - $d, $d, $d, 90, 90)
    $p.CloseFigure()
    return $p
}

function Draw([int]$w, [int]$h, [string]$path, [float]$scale) {
    $bmp = New-Object System.Drawing.Bitmap $w, $h
    $g = [System.Drawing.Graphics]::FromImage($bmp)
    $g.SmoothingMode = 'AntiAlias'; $g.TextRenderingHint = 'AntiAliasGridFit'; $g.PixelOffsetMode = 'HighQuality'

    # Background: near black with a soft glow low in the middle.
    $g.Clear([System.Drawing.Color]::FromArgb(255, 9, 9, 9))
    $glow = New-Object System.Drawing.Drawing2D.GraphicsPath
    $glow.AddEllipse(($w * 0.1), ($h * 0.35), ($w * 0.8), ($h * 1.1))
    $gb = New-Object System.Drawing.Drawing2D.PathGradientBrush $glow
    $gb.CenterColor = [System.Drawing.Color]::FromArgb(255, 24, 24, 24)
    $gb.SurroundColors = @([System.Drawing.Color]::FromArgb(0, 9, 9, 9))
    $g.FillPath($gb, $glow)

    $white = [System.Drawing.Color]::FromArgb(255, 244, 244, 244)
    $muted = [System.Drawing.Color]::FromArgb(255, 160, 160, 160)
    $s = $scale

    # The mark, with a soft shadow under it.
    $u = 360 * $s / 512.0
    $cx = $w * 0.275; $cy = $h * 0.5
    $ox = $cx - 256 * $u; $oy = $cy - 260 * $u
    $shadow = New-Object System.Drawing.SolidBrush ([System.Drawing.Color]::FromArgb(150, 0, 0, 0))
    $g.FillEllipse($shadow, ($cx - 120 * $s), ($oy + 470 * $u), (240 * $s), (26 * $s))
    $pen = New-Object System.Drawing.Pen $white, (40 * $u)
    $pen.StartCap = 'Round'; $pen.EndCap = 'Round'
    $g.DrawEllipse($pen, ($ox + 144 * $u), ($oy + 104 * $u), (224 * $u), (224 * $u))
    $g.DrawLine($pen, ($ox + 256 * $u), ($oy + 350 * $u), ($ox + 256 * $u), ($oy + 436 * $u))
    $g.FillEllipse((New-Object System.Drawing.SolidBrush $white), ($ox + 220 * $u), ($oy + 180 * $u), (72 * $u), (72 * $u))

    # Words
    $family = if ((New-Object System.Drawing.Text.InstalledFontCollection).Families.Name -contains 'Segoe UI Variable Display') { 'Segoe UI Variable Display' } else { 'Segoe UI' }
    $px = [System.Drawing.GraphicsUnit]::Pixel
    $titleFont = New-Object System.Drawing.Font $family, (210 * $s), ([System.Drawing.FontStyle]::Bold), $px
    $subFont = New-Object System.Drawing.Font $family, (62 * $s), ([System.Drawing.FontStyle]::Regular), $px
    $tagFont = New-Object System.Drawing.Font $family, (34 * $s), ([System.Drawing.FontStyle]::Regular), $px
    $tx = $w * 0.415
    $g.DrawString('Tether', $titleFont, (New-Object System.Drawing.SolidBrush $white), ($tx - 14 * $s), ($cy - 250 * $s))
    $g.DrawString($tagline, $subFont, (New-Object System.Drawing.SolidBrush $muted), $tx, ($cy + 10 * $s))

    # Tags: outlined pills
    $x = $tx; $y = $cy + 125 * $s
    $border = New-Object System.Drawing.Pen ([System.Drawing.Color]::FromArgb(255, 58, 58, 58)), (2.5 * $s)
    foreach ($t in $tags) {
        $size = $g.MeasureString($t, $tagFont)
        $pw = $size.Width + 52 * $s; $ph = 66 * $s
        $pill = RoundedRect $x $y $pw $ph ($ph / 2)
        $g.FillPath((New-Object System.Drawing.SolidBrush ([System.Drawing.Color]::FromArgb(255, 14, 14, 14))), $pill)
        $g.DrawPath($border, $pill)
        $g.DrawString($t, $tagFont, (New-Object System.Drawing.SolidBrush $white), ($x + 26 * $s), ($y + ($ph - $size.Height) / 2))
        $x += $pw + 22 * $s
    }
    $g.Dispose()
    $bmp.Save($path, [System.Drawing.Imaging.ImageFormat]::Png)
    $bmp.Dispose()
}

Draw 2560 840 (Join-Path $docs 'banner.png') 1.0
Draw 1280 640 (Join-Path $docs 'social-preview.png') 0.62
Write-Host 'Wrote docs\banner.png and docs\social-preview.png'
