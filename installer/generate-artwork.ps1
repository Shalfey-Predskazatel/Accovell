param([switch]$GraphicsOnly)

Add-Type -AssemblyName System.Drawing

$assets = Join-Path $PSScriptRoot 'assets'
New-Item -ItemType Directory -Force -Path $assets | Out-Null

# Native icons are generated from the same SVG master that the React surface
# renders.  Use the generated 128px layer here because System.Drawing does not
# rasterize SVG itself; this keeps installer artwork visually identical to the
# taskbar, tray and in-app Account Relay mark.
$brandIconPath = Join-Path $PSScriptRoot '..\src-tauri\icons\128x128.png'
$script:brandIcon = [System.Drawing.Image]::FromFile((Resolve-Path -LiteralPath $brandIconPath))

function New-BrandMark([System.Drawing.Graphics]$Graphics, [single]$X, [single]$Y, [single]$Size) {
    $Graphics.DrawImage($script:brandIcon, [System.Drawing.RectangleF]::new($X, $Y, $Size, $Size))
}

$welcome = [System.Drawing.Bitmap]::new(494, 314, [System.Drawing.Imaging.PixelFormat]::Format24bppRgb)
$g = [System.Drawing.Graphics]::FromImage($welcome)
$g.SmoothingMode = [System.Drawing.Drawing2D.SmoothingMode]::AntiAlias
$g.Clear([System.Drawing.Color]::FromArgb(16, 10, 24))
$bounds = [System.Drawing.Rectangle]::new(0, 0, 164, 314)
$gradient = [System.Drawing.Drawing2D.LinearGradientBrush]::new($bounds, [System.Drawing.Color]::FromArgb(17, 8, 26), [System.Drawing.Color]::FromArgb(65, 27, 82), 42.0)
$g.FillRectangle($gradient, $bounds)
$glow = [System.Drawing.SolidBrush]::new([System.Drawing.Color]::FromArgb(42, 200, 99, 210))
$g.FillEllipse($glow, -42, 116, 245, 245)
$ring = [System.Drawing.Pen]::new([System.Drawing.Color]::FromArgb(108, 236, 158, 228), 1.5)
$g.DrawEllipse($ring, 26, 42, 112, 112)
$g.DrawEllipse($ring, 39, 55, 86, 86)
New-BrandMark $g 59 75 46
$small = [System.Drawing.Font]::new('Segoe UI', 7.5, [System.Drawing.FontStyle]::Bold)
$caption = [System.Drawing.SolidBrush]::new([System.Drawing.Color]::FromArgb(250, 228, 248))
$muted = [System.Drawing.SolidBrush]::new([System.Drawing.Color]::FromArgb(196, 157, 205))
$g.DrawString('CODEX ACCOUNTS', $small, $caption, [single]18, [single]173)
$g.DrawString('LOCAL SETUP', $small, $muted, [single]31, [single]190)
for ($i = 0; $i -lt 3; $i++) {
    $y = 229 + 24 * $i
    $g.FillEllipse([System.Drawing.SolidBrush]::new([System.Drawing.Color]::FromArgb(235, 151, 226)), 25, $y, 8, 8)
    $g.FillRectangle([System.Drawing.SolidBrush]::new([System.Drawing.Color]::FromArgb(111, 70, 124)), 42, $y + 2, (72 - 9 * $i), 4)
}
$welcome.Save((Join-Path $assets 'welcome.bmp'), [System.Drawing.Imaging.ImageFormat]::Bmp)
$g.Dispose(); $gradient.Dispose(); $glow.Dispose(); $ring.Dispose(); $small.Dispose(); $caption.Dispose(); $muted.Dispose(); $welcome.Dispose()

$welcomeLight = [System.Drawing.Bitmap]::new(494, 314, [System.Drawing.Imaging.PixelFormat]::Format24bppRgb)
$g = [System.Drawing.Graphics]::FromImage($welcomeLight)
$g.SmoothingMode = [System.Drawing.Drawing2D.SmoothingMode]::AntiAlias
$g.Clear([System.Drawing.Color]::FromArgb(248, 242, 252))
$bounds = [System.Drawing.Rectangle]::new(0, 0, 164, 314)
$gradient = [System.Drawing.Drawing2D.LinearGradientBrush]::new($bounds, [System.Drawing.Color]::FromArgb(249, 245, 255), [System.Drawing.Color]::FromArgb(231, 211, 250), 42.0)
$g.FillRectangle($gradient, $bounds)
$glow = [System.Drawing.SolidBrush]::new([System.Drawing.Color]::FromArgb(44, 136, 61, 196))
$g.FillEllipse($glow, -42, 116, 245, 245)
$ring = [System.Drawing.Pen]::new([System.Drawing.Color]::FromArgb(94, 113, 61, 172), 1.5)
$g.DrawEllipse($ring, 26, 42, 112, 112)
$g.DrawEllipse($ring, 39, 55, 86, 86)
New-BrandMark $g 59 75 46
$small = [System.Drawing.Font]::new('Segoe UI', 7.5, [System.Drawing.FontStyle]::Bold)
$caption = [System.Drawing.SolidBrush]::new([System.Drawing.Color]::FromArgb(58, 31, 77))
$muted = [System.Drawing.SolidBrush]::new([System.Drawing.Color]::FromArgb(103, 73, 123))
$g.DrawString('CODEX ACCOUNTS', $small, $caption, [single]18, [single]173)
$g.DrawString('LOCAL SETUP', $small, $muted, [single]31, [single]190)
for ($i = 0; $i -lt 3; $i++) {
    $y = 229 + 24 * $i
    $g.FillEllipse([System.Drawing.SolidBrush]::new([System.Drawing.Color]::FromArgb(131, 61, 210)), 25, $y, 8, 8)
    $g.FillRectangle([System.Drawing.SolidBrush]::new([System.Drawing.Color]::FromArgb(202, 178, 221)), 42, $y + 2, (72 - 9 * $i), 4)
}
$welcomeLight.Save((Join-Path $assets 'welcome-light.bmp'), [System.Drawing.Imaging.ImageFormat]::Bmp)
$g.Dispose(); $gradient.Dispose(); $glow.Dispose(); $ring.Dispose(); $small.Dispose(); $caption.Dispose(); $muted.Dispose(); $welcomeLight.Dispose()

$header = [System.Drawing.Bitmap]::new(150, 57, [System.Drawing.Imaging.PixelFormat]::Format24bppRgb)
$g = [System.Drawing.Graphics]::FromImage($header)
$g.SmoothingMode = [System.Drawing.Drawing2D.SmoothingMode]::AntiAlias
$g.Clear([System.Drawing.Color]::FromArgb(33, 18, 47))
New-BrandMark $g 8 11 35
$title = [System.Drawing.Font]::new('Segoe UI', 9.4, [System.Drawing.FontStyle]::Bold)
$sub = [System.Drawing.Font]::new('Segoe UI', 6.0, [System.Drawing.FontStyle]::Regular)
$ink = [System.Drawing.SolidBrush]::new([System.Drawing.Color]::FromArgb(248, 239, 252))
$soft = [System.Drawing.SolidBrush]::new([System.Drawing.Color]::FromArgb(205, 185, 212))
$g.DrawString('CODEX', $title, $ink, [single]51, [single]14)
$g.DrawString('LOCAL APP', $sub, $soft, [single]51, [single]32)
$header.Save((Join-Path $assets 'header.bmp'), [System.Drawing.Imaging.ImageFormat]::Bmp)
$g.Dispose(); $title.Dispose(); $sub.Dispose(); $ink.Dispose(); $soft.Dispose(); $header.Dispose()

$headerLight = [System.Drawing.Bitmap]::new(150, 57, [System.Drawing.Imaging.PixelFormat]::Format24bppRgb)
$g = [System.Drawing.Graphics]::FromImage($headerLight)
$g.SmoothingMode = [System.Drawing.Drawing2D.SmoothingMode]::AntiAlias
$g.Clear([System.Drawing.Color]::FromArgb(244, 236, 250))
New-BrandMark $g 8 11 35
$title = [System.Drawing.Font]::new('Segoe UI', 9.4, [System.Drawing.FontStyle]::Bold)
$sub = [System.Drawing.Font]::new('Segoe UI', 6.0, [System.Drawing.FontStyle]::Regular)
$ink = [System.Drawing.SolidBrush]::new([System.Drawing.Color]::FromArgb(58, 31, 77))
$soft = [System.Drawing.SolidBrush]::new([System.Drawing.Color]::FromArgb(103, 73, 123))
$g.DrawString('CODEX', $title, $ink, [single]51, [single]14)
$g.DrawString('LOCAL APP', $sub, $soft, [single]51, [single]32)
$headerLight.Save((Join-Path $assets 'header-light.bmp'), [System.Drawing.Imaging.ImageFormat]::Bmp)
$g.Dispose(); $title.Dispose(); $sub.Dispose(); $ink.Dispose(); $soft.Dispose(); $headerLight.Dispose()

function Write-ToneWav([string]$Path, [int[]]$Notes) {
    $sampleRate = 22050
    $noteSamples = [int]($sampleRate * 0.068)
    $gapSamples = [int]($sampleRate * 0.012)
    $sampleCount = $Notes.Count * ($noteSamples + $gapSamples)
    $dataLength = $sampleCount * 2
    $stream = [System.IO.File]::Create($Path)
    $writer = [System.IO.BinaryWriter]::new($stream)
    $writer.Write([Text.Encoding]::ASCII.GetBytes('RIFF'))
    $writer.Write([uint32](36 + $dataLength))
    $writer.Write([Text.Encoding]::ASCII.GetBytes('WAVEfmt '))
    $writer.Write([uint32]16); $writer.Write([uint16]1); $writer.Write([uint16]1)
    $writer.Write([uint32]$sampleRate); $writer.Write([uint32]($sampleRate * 2))
    $writer.Write([uint16]2); $writer.Write([uint16]16)
    $writer.Write([Text.Encoding]::ASCII.GetBytes('data')); $writer.Write([uint32]$dataLength)
    foreach ($note in $Notes) {
        for ($sample = 0; $sample -lt $noteSamples; $sample++) {
            $time = $sample / [double]$sampleRate
            $progress = $sample / [double]$noteSamples
            $attack = [Math]::Min(1.0, $time / 0.008)
            $release = [Math]::Min(1.0, (1.0 - $progress) / 0.10)
            $envelope = $attack * $release * (0.78 + 0.22 * (1.0 - $progress))
            $fundamental = [Math]::Sin(2.0 * [Math]::PI * $note * $time)
            $overtone = [Math]::Sin(2.0 * [Math]::PI * 2.0 * $note * $time)
            $value = ($fundamental + 0.12 * $overtone) * $envelope * 0.12
            $writer.Write([int16]($value * [int16]::MaxValue))
        }
        for ($sample = 0; $sample -lt $gapSamples; $sample++) { $writer.Write([int16]0) }
    }
    $writer.Dispose()
}

if (-not $GraphicsOnly) {
    Write-ToneWav (Join-Path $assets 'welcome.wav') @(659, 784, 988)
    Write-ToneWav (Join-Path $assets 'location.wav') @(523, 698, 880)
    Write-ToneWav (Join-Path $assets 'install.wav') @(740, 880, 1047)
    Write-ToneWav (Join-Path $assets 'finish.wav') @(587, 880, 1047)

    Write-ToneWav (Join-Path $assets 'language.wav') @(784, 988, 1175)
    Write-ToneWav (Join-Path $assets 'browse.wav') @(587, 740, 932)
    Write-ToneWav (Join-Path $assets 'theme-light.wav') @(523, 659, 880, 1047)
    Write-ToneWav (Join-Path $assets 'theme-dark.wav') @(880, 659, 523, 392)
    Write-ToneWav (Join-Path $assets 'sound-on.wav') @(659, 880, 1175)
    Write-ToneWav (Join-Path $assets 'sound-off.wav') @(1175, 880, 659)
    Write-ToneWav (Join-Path $assets 'cancel.wav') @(587, 466, 349)
}
