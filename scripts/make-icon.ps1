param([string]$OutputDirectory = (Join-Path $PSScriptRoot '..\src-tauri\icons'))
$ErrorActionPreference = 'Stop'
Add-Type -AssemblyName System.Drawing
$null = New-Item -ItemType Directory -Force -Path $OutputDirectory
foreach ($size in @(32, 256)) {
    $bitmap = New-Object System.Drawing.Bitmap($size, $size)
    $graphics = [System.Drawing.Graphics]::FromImage($bitmap)
    $graphics.SmoothingMode = 'AntiAlias'
    $graphics.ScaleTransform($size / 256.0, $size / 256.0)
    $path = New-Object System.Drawing.Drawing2D.GraphicsPath
    $path.AddArc(8, 8, 68, 68, 180, 90)
    $path.AddArc(180, 8, 68, 68, 270, 90)
    $path.AddArc(180, 180, 68, 68, 0, 90)
    $path.AddArc(8, 180, 68, 68, 90, 90)
    $path.CloseFigure()
    $brush = New-Object System.Drawing.Drawing2D.LinearGradientBrush(
        (New-Object System.Drawing.Point(0, 0)), (New-Object System.Drawing.Point(256, 256)),
        [System.Drawing.Color]::FromArgb(148, 99, 240), [System.Drawing.Color]::FromArgb(89, 52, 182))
    $graphics.FillPath($brush, $path)
    $pen = New-Object System.Drawing.Pen([System.Drawing.Color]::White, 15)
    $pen.StartCap = 'Round'
    $pen.EndCap = 'Round'
    $graphics.DrawLine($pen, 67, 84, 111, 125)
    $graphics.DrawLine($pen, 111, 125, 67, 166)
    $graphics.DrawLine($pen, 137, 166, 188, 166)
    $bitmap.Save((Join-Path $OutputDirectory "$size.png"), [System.Drawing.Imaging.ImageFormat]::Png)
    $pen.Dispose()
    $brush.Dispose()
    $path.Dispose()
    $graphics.Dispose()
    $bitmap.Dispose()
}
$png = [System.IO.File]::ReadAllBytes((Join-Path $OutputDirectory '256.png'))
$stream = [System.IO.File]::Create((Join-Path $OutputDirectory 'icon.ico'))
$writer = New-Object System.IO.BinaryWriter($stream)
$writer.Write([uint16]0)
$writer.Write([uint16]1)
$writer.Write([uint16]1)
$writer.Write([byte]0)
$writer.Write([byte]0)
$writer.Write([byte]0)
$writer.Write([byte]0)
$writer.Write([uint16]1)
$writer.Write([uint16]32)
$writer.Write([uint32]$png.Length)
$writer.Write([uint32]22)
$writer.Write($png)
$writer.Dispose()
