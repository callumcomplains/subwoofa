# Draws the installer artwork (the two bitmaps the NSIS installer shows). Run it from this folder whenever the look
# changes: powershell -File make-art.ps1
Add-Type -AssemblyName System.Drawing

$ground = [System.Drawing.ColorTranslator]::FromHtml("#0f1011")
$panel  = [System.Drawing.ColorTranslator]::FromHtml("#1d2023")
$rule   = [System.Drawing.ColorTranslator]::FromHtml("#292d30")
$amber  = [System.Drawing.ColorTranslator]::FromHtml("#ff7f1a")
$ink    = [System.Drawing.ColorTranslator]::FromHtml("#e8e6e1")
$dim    = [System.Drawing.ColorTranslator]::FromHtml("#8a8f94")

function New-Canvas($w, $h) {
    $bmp = New-Object System.Drawing.Bitmap $w, $h, ([System.Drawing.Imaging.PixelFormat]::Format24bppRgb)
    $g = [System.Drawing.Graphics]::FromImage($bmp)
    $g.SmoothingMode = "AntiAlias"
    $g.TextRenderingHint = "AntiAliasGridFit"
    $g.Clear($ground)
    return @($bmp, $g)
}

# The app's mark: an amber square with a dark square cut into it and a small amber square in the middle.
function Draw-Mark($g, $x, $y, $size) {
    $amberBrush = New-Object System.Drawing.SolidBrush $amber
    $groundBrush = New-Object System.Drawing.SolidBrush $ground
    $g.FillRectangle($amberBrush, $x, $y, $size, $size)
    $inset = $size * 0.2
    $g.FillRectangle($groundBrush, $x + $inset, $y + $inset, $size - 2 * $inset, $size - 2 * $inset)
    $core = $size * 0.4
    $g.FillRectangle($amberBrush, $x + ($size - $core) / 2, $y + ($size - $core) / 2, $core, $core)
}

function Pick-Font($names, $size, $style) {
    foreach ($n in $names) {
        $f = New-Object System.Drawing.Font $n, $size, $style, ([System.Drawing.GraphicsUnit]::Pixel)
        if ($f.Name -eq $n) { return $f }
    }
    return New-Object System.Drawing.Font "Segoe UI", $size, $style, ([System.Drawing.GraphicsUnit]::Pixel)
}
$label = @("Bahnschrift", "Segoe UI")
$mono = @("Cascadia Mono", "Consolas")
$spaced = New-Object System.Drawing.StringFormat
$spaced.FormatFlags = [System.Drawing.StringFormatFlags]::NoWrap

# Sidebar, 164 x 314: shown on the welcome and finish pages.
$c = New-Canvas 164 314; $bmp = $c[0]; $g = $c[1]
# faint concentric squares behind the mark, like the rings of a speaker cone
$pen = New-Object System.Drawing.Pen $rule, 1
for ($i = 1; $i -le 6; $i++) {
    $s = 40 + $i * 44
    $g.DrawRectangle($pen, 82 - $s / 2, 118 - $s / 2, $s, $s)
}
Draw-Mark $g 52 88 60
$wordmark = Pick-Font $label 24 ([System.Drawing.FontStyle]::Bold)
$text = "SUBWOOFA"
$g.DrawString($text, $wordmark, (New-Object System.Drawing.SolidBrush $ink), 14, 170)
$tag = Pick-Font $mono 11 ([System.Drawing.FontStyle]::Regular)
$g.DrawString("every channel,", $tag, (New-Object System.Drawing.SolidBrush $dim), 16, 204)
$g.DrawString("every codec", $tag, (New-Object System.Drawing.SolidBrush $amber), 16, 220)
$g.FillRectangle((New-Object System.Drawing.SolidBrush $amber), 0, 306, 164, 8)
$bmp.Save("$PSScriptRoot\sidebar.bmp", [System.Drawing.Imaging.ImageFormat]::Bmp)
$g.Dispose(); $bmp.Dispose()

# Header, 150 x 57: shown at the top of the other pages.
$c = New-Canvas 150 57; $bmp = $c[0]; $g = $c[1]
Draw-Mark $g 14 13 30
$head = Pick-Font $label 15 ([System.Drawing.FontStyle]::Bold)
$g.DrawString("SUBWOOFA", $head, (New-Object System.Drawing.SolidBrush $ink), 54, 14)
$small = Pick-Font $mono 9 ([System.Drawing.FontStyle]::Regular)
$g.DrawString("every codec", $small, (New-Object System.Drawing.SolidBrush $amber), 55, 34)
$g.FillRectangle((New-Object System.Drawing.SolidBrush $amber), 0, 54, 150, 3)
$bmp.Save("$PSScriptRoot\header.bmp", [System.Drawing.Imaging.ImageFormat]::Bmp)
$g.Dispose(); $bmp.Dispose()
Write-Host "wrote sidebar.bmp and header.bmp"
