# Draws docs\banner.png (README header) and docs\social-preview.png (GitHub link preview).

$ErrorActionPreference = 'Stop'
Add-Type -AssemblyName System.Drawing
$root = Split-Path $PSScriptRoot -Parent
$docs = Join-Path $root 'docs'
New-Item -ItemType Directory $docs -Force | Out-Null

function Draw([int]$w, [int]$h, [string]$path) {
    $bmp = New-Object System.Drawing.Bitmap $w, $h
    $g = [System.Drawing.Graphics]::FromImage($bmp)
    $g.SmoothingMode = 'AntiAlias'; $g.TextRenderingHint = 'AntiAliasGridFit'; $g.PixelOffsetMode = 'HighQuality'
    $g.Clear([System.Drawing.Color]::FromArgb(255, 10, 10, 10))
    $white = [System.Drawing.Color]::FromArgb(255, 244, 244, 244)
    $muted = [System.Drawing.Color]::FromArgb(255, 150, 150, 150)

    # Faint grid lines for depth
    $gridPen = New-Object System.Drawing.Pen ([System.Drawing.Color]::FromArgb(255, 22, 22, 22)), 1
    for ($x = 0; $x -lt $w; $x += 40) { $g.DrawLine($gridPen, $x, 0, $x, $h) }
    for ($y = 0; $y -lt $h; $y += 40) { $g.DrawLine($gridPen, 0, $y, $w, $y) }

    # The mark
    $u = ($h * 0.62) / 512.0
    $ox = $w * 0.16 - 256 * $u; $oy = $h / 2.0 - 266 * $u
    $pen = New-Object System.Drawing.Pen $white, (36 * $u)
    $pen.StartCap = 'Round'; $pen.EndCap = 'Round'
    $g.DrawEllipse($pen, ($ox + 144 * $u), ($oy + 104 * $u), (224 * $u), (224 * $u))
    $g.DrawLine($pen, ($ox + 256 * $u), ($oy + 346 * $u), ($ox + 256 * $u), ($oy + 430 * $u))
    $g.FillEllipse((New-Object System.Drawing.SolidBrush $white), ($ox + 222 * $u), ($oy + 182 * $u), (68 * $u), (68 * $u))

    # Words
    $family = if ((New-Object System.Drawing.Text.InstalledFontCollection).Families.Name -contains 'Segoe UI Variable Display') { 'Segoe UI Variable Display' } else { 'Segoe UI' }
    $titleFont = New-Object System.Drawing.Font $family, ($h * 0.2), ([System.Drawing.FontStyle]::Bold), ([System.Drawing.GraphicsUnit]::Pixel)
    $subFont = New-Object System.Drawing.Font $family, ($h * 0.062), ([System.Drawing.FontStyle]::Regular), ([System.Drawing.GraphicsUnit]::Pixel)
    $tx = $w * 0.30
    $g.DrawString('Tether', $titleFont, (New-Object System.Drawing.SolidBrush $white), $tx, ($h * 0.28))
    $g.DrawString('Your Android phone as a webcam. Light, steady, over a cable.', $subFont, (New-Object System.Drawing.SolidBrush $muted), ($tx + $h * 0.012), ($h * 0.56))
    $g.Dispose()
    $bmp.Save($path, [System.Drawing.Imaging.ImageFormat]::Png)
    $bmp.Dispose()
}

Draw 1600 400 (Join-Path $docs 'banner.png')
Draw 1280 640 (Join-Path $docs 'social-preview.png')
Write-Host 'Wrote docs\banner.png and docs\social-preview.png'
