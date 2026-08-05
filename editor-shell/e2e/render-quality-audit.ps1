# Inventory every raster artifact, compute cheap visual-health signals, and emit contact sheets for human
# inspection. This deliberately uses only Windows' built-in System.Drawing so it can run in the packaged
# app's normal CI/developer environment without an image-tool dependency.
[CmdletBinding()]
param(
    [string]$Root = ".",
    [string]$OutDir = "evidence/render-quality-audit/inventory",
    [int]$Columns = 4,
    [int]$Rows = 4
)

$ErrorActionPreference = "Stop"
Add-Type -AssemblyName System.Drawing

$rootPath = (Resolve-Path -LiteralPath $Root).Path
$outPath = [IO.Path]::GetFullPath((Join-Path (Get-Location) $OutDir))
[IO.Directory]::CreateDirectory($outPath) | Out-Null

function Get-RelativeAuditPath([string]$Path) {
    if ($Path.StartsWith($rootPath, [StringComparison]::OrdinalIgnoreCase)) {
        return $Path.Substring($rootPath.Length).TrimStart([char]'\', [char]'/')
    }
    return $Path
}

$images = Get-ChildItem -LiteralPath $rootPath -Recurse -Force -File |
    Where-Object {
        $_.Extension -match '^\.(png|jpe?g|bmp)$' -and
        $_.FullName -notmatch '[\\/](node_modules|target|\.git)[\\/]' -and
        -not $_.FullName.StartsWith($outPath, [StringComparison]::OrdinalIgnoreCase)
    } |
    Sort-Object FullName

$records = [Collections.Generic.List[object]]::new()
foreach ($file in $images) {
    $bitmap = $null
    try {
        $bitmap = [Drawing.Bitmap]::FromFile($file.FullName)
        $sampleStepX = [Math]::Max(1, [int][Math]::Ceiling($bitmap.Width / 96.0))
        $sampleStepY = [Math]::Max(1, [int][Math]::Ceiling($bitmap.Height / 96.0))
        $colors = [Collections.Generic.HashSet[int]]::new()
        $sum = 0.0
        $sumSq = 0.0
        $dark = 0
        $light = 0
        $transparent = 0
        $highFrequency = 0
        $comparisons = 0
        $count = 0
        $background = $bitmap.GetPixel(0, 0)
        $contentMinX = $bitmap.Width
        $contentMinY = $bitmap.Height
        $contentMaxX = -1
        $contentMaxY = -1
        $borderContent = 0
        $borderSamples = 0
        for ($y = 0; $y -lt $bitmap.Height; $y += $sampleStepY) {
            $previous = $null
            for ($x = 0; $x -lt $bitmap.Width; $x += $sampleStepX) {
                $pixel = $bitmap.GetPixel($x, $y)
                $luma = 0.2126 * $pixel.R + 0.7152 * $pixel.G + 0.0722 * $pixel.B
                [void]$colors.Add(($pixel.R -shl 16) -bor ($pixel.G -shl 8) -bor $pixel.B)
                $sum += $luma
                $sumSq += $luma * $luma
                if ($luma -le 4) { $dark++ }
                if ($luma -ge 251) { $light++ }
                if ($pixel.A -le 4) { $transparent++ }
                $backgroundDelta = [Math]::Max(
                    [Math]::Abs([int]$pixel.R - [int]$background.R),
                    [Math]::Max([Math]::Abs([int]$pixel.G - [int]$background.G), [Math]::Abs([int]$pixel.B - [int]$background.B))
                )
                $nearBorder = $x -lt (2 * $sampleStepX) -or $y -lt (2 * $sampleStepY) -or $x -ge ($bitmap.Width - 2 * $sampleStepX) -or $y -ge ($bitmap.Height - 2 * $sampleStepY)
                if ($nearBorder) {
                    $borderSamples++
                    if ($backgroundDelta -gt 24 -and $pixel.A -gt 4) { $borderContent++ }
                }
                if ($backgroundDelta -gt 24 -and $pixel.A -gt 4) {
                    $contentMinX = [Math]::Min($contentMinX, $x)
                    $contentMinY = [Math]::Min($contentMinY, $y)
                    $contentMaxX = [Math]::Max($contentMaxX, $x)
                    $contentMaxY = [Math]::Max($contentMaxY, $y)
                }
                if ($null -ne $previous) {
                    if ([Math]::Abs($luma - $previous) -ge 32) { $highFrequency++ }
                    $comparisons++
                }
                $previous = $luma
                $count++
            }
        }
        $mean = if ($count) { $sum / $count } else { 0 }
        $variance = if ($count) { [Math]::Max(0, $sumSq / $count - $mean * $mean) } else { 0 }
        $relative = Get-RelativeAuditPath $file.FullName
        $flags = [Collections.Generic.List[string]]::new()
        if ($bitmap.Width -lt 16 -or $bitmap.Height -lt 16) { $flags.Add("tiny") }
        if ($colors.Count -le 4 -or [Math]::Sqrt($variance) -lt 1.0) { $flags.Add("near-blank") }
        if ($count -and (($dark / $count) -gt 0.98 -or ($light / $count) -gt 0.98)) { $flags.Add("clipped") }
        $records.Add([pscustomobject]@{
            path = $relative
            directory = [IO.Path]::GetDirectoryName($relative)
            width = $bitmap.Width
            height = $bitmap.Height
            aspect = [Math]::Round($bitmap.Width / [double]$bitmap.Height, 4)
            bytes = $file.Length
            sha256 = (Get-FileHash -LiteralPath $file.FullName -Algorithm SHA256).Hash.ToLowerInvariant()
            sampledColors = $colors.Count
            meanLuma = [Math]::Round($mean, 3)
            lumaStdDev = [Math]::Round([Math]::Sqrt($variance), 3)
            darkFraction = if ($count) { [Math]::Round($dark / $count, 5) } else { 0 }
            lightFraction = if ($count) { [Math]::Round($light / $count, 5) } else { 0 }
            transparentFraction = if ($count) { [Math]::Round($transparent / $count, 5) } else { 0 }
            highFrequencyFraction = if ($comparisons) { [Math]::Round($highFrequency / $comparisons, 5) } else { 0 }
            contentBounds = if ($contentMaxX -ge 0) { @($contentMinX, $contentMinY, $contentMaxX, $contentMaxY) } else { @() }
            borderContentFraction = if ($borderSamples) { [Math]::Round($borderContent / $borderSamples, 5) } else { 0 }
            flags = @($flags)
        })
    } catch {
        $records.Add([pscustomobject]@{
            path = Get-RelativeAuditPath $file.FullName
            directory = ""
            width = 0
            height = 0
            aspect = 0
            bytes = $file.Length
            sha256 = ""
            sampledColors = 0
            meanLuma = 0
            lumaStdDev = 0
            darkFraction = 0
            lightFraction = 0
            transparentFraction = 0
            highFrequencyFraction = 0
            contentBounds = @()
            borderContentFraction = 0
            flags = @("decode-error: $($_.Exception.Message)")
        })
    } finally {
        if ($null -ne $bitmap) { $bitmap.Dispose() }
    }
}

$records | ConvertTo-Json -Depth 5 | Set-Content -LiteralPath (Join-Path $outPath "raster-inventory.json") -Encoding utf8
$records | Export-Csv -LiteralPath (Join-Path $outPath "raster-inventory.csv") -NoTypeInformation -Encoding utf8

$duplicates = $records |
    Where-Object { $_.sha256 } |
    Group-Object sha256 |
    Where-Object Count -gt 1 |
    ForEach-Object { [pscustomobject]@{ sha256 = $_.Name; paths = @($_.Group.path) } }
$duplicates | ConvertTo-Json -Depth 4 | Set-Content -LiteralPath (Join-Path $outPath "exact-duplicates.json") -Encoding utf8

# A contact sheet per directory/page makes all captures reviewable at once while preserving filenames.
$tileWidth = 300
$tileHeight = 210
$labelHeight = 36
$pageSize = [Math]::Max(1, $Columns * $Rows)
$font = [Drawing.Font]::new("Segoe UI", 8, [Drawing.FontStyle]::Regular, [Drawing.GraphicsUnit]::Pixel)
$brush = [Drawing.SolidBrush]::new([Drawing.Color]::FromArgb(230, 230, 235))
$background = [Drawing.Color]::FromArgb(18, 20, 26)
try {
    foreach ($group in ($records | Where-Object { $_.width -gt 0 } | Group-Object directory)) {
        $safe = if ([string]::IsNullOrWhiteSpace($group.Name)) { "root" } else { $group.Name -replace '[^a-zA-Z0-9._-]+', '_' }
        for ($offset = 0; $offset -lt $group.Count; $offset += $pageSize) {
            $page = @($group.Group | Select-Object -Skip $offset -First $pageSize)
            $sheet = [Drawing.Bitmap]::new($Columns * $tileWidth, $Rows * ($tileHeight + $labelHeight))
            $graphics = [Drawing.Graphics]::FromImage($sheet)
            try {
                $graphics.Clear($background)
                $graphics.InterpolationMode = [Drawing.Drawing2D.InterpolationMode]::HighQualityBicubic
                $graphics.PixelOffsetMode = [Drawing.Drawing2D.PixelOffsetMode]::HighQuality
                for ($index = 0; $index -lt $page.Count; $index++) {
                    $record = $page[$index]
                    $sourcePath = Join-Path $rootPath $record.path
                    $source = [Drawing.Image]::FromFile($sourcePath)
                    try {
                        $column = $index % $Columns
                        $row = [Math]::Floor($index / $Columns)
                        $x = $column * $tileWidth
                        $y = $row * ($tileHeight + $labelHeight)
                        $scale = [Math]::Min(($tileWidth - 8) / $source.Width, ($tileHeight - 8) / $source.Height)
                        $drawWidth = [Math]::Max(1, [int]($source.Width * $scale))
                        $drawHeight = [Math]::Max(1, [int]($source.Height * $scale))
                        $drawX = $x + [int](($tileWidth - $drawWidth) / 2)
                        $drawY = $y + [int](($tileHeight - $drawHeight) / 2)
                        $graphics.DrawImage($source, $drawX, $drawY, $drawWidth, $drawHeight)
                        $name = [IO.Path]::GetFileName($record.path)
                        $label = "{0}  [{1}x{2}]" -f $name, $record.width, $record.height
                        $graphics.DrawString($label, $font, $brush, $x + 5, $y + $tileHeight + 4)
                    } finally {
                        $source.Dispose()
                    }
                }
            } finally {
                $graphics.Dispose()
            }
            $pageNumber = [int]($offset / $pageSize) + 1
            $sheet.Save((Join-Path $outPath ("contact-{0}-p{1:d2}.png" -f $safe, $pageNumber)), [Drawing.Imaging.ImageFormat]::Png)
            $sheet.Dispose()
        }
    }
} finally {
    $font.Dispose()
    $brush.Dispose()
}

$flagged = @($records | Where-Object { $_.flags.Count -gt 0 })
"Audited {0} rasters; {1} flagged; {2} exact duplicate groups; output: {3}" -f $records.Count, $flagged.Count, @($duplicates).Count, $outPath
