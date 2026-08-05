# Pixel-based visual regression for OS-composited viewport captures. It never updates baselines: callers must
# explicitly promote reviewed images, then this gate compares future candidates with measurable thresholds.
[CmdletBinding()]
param(
    [Parameter(Mandatory = $true)][string]$BaselineDir,
    [Parameter(Mandatory = $true)][string]$CandidateDir,
    [string]$OutputJson = "visual-diff.json",
    [double]$MaxRmse = 12.0,
    [double]$MaxChangedFraction = 0.08,
    [int]$PerChannelTolerance = 12,
    [int]$SampleStep = 2,
    [switch]$ReportOnly
)

$ErrorActionPreference = "Stop"
Add-Type -AssemblyName System.Drawing
$baselinePath = (Resolve-Path -LiteralPath $BaselineDir).Path
$candidatePath = (Resolve-Path -LiteralPath $CandidateDir).Path
$results = [Collections.Generic.List[object]]::new()
$failed = $false

foreach ($baselineFile in (Get-ChildItem -LiteralPath $baselinePath -File -Filter *.png | Sort-Object Name)) {
    $candidateFile = Join-Path $candidatePath $baselineFile.Name
    if (-not (Test-Path -LiteralPath $candidateFile)) {
        $results.Add([pscustomobject]@{ name = $baselineFile.Name; status = "missing" })
        $failed = $true
        continue
    }

    $baseline = [Drawing.Bitmap]::FromFile($baselineFile.FullName)
    $source = [Drawing.Bitmap]::FromFile($candidateFile)
    $candidate = $source
    $resized = $false
    try {
        if ($source.Width -ne $baseline.Width -or $source.Height -ne $baseline.Height) {
            $resized = $true
            $candidate = [Drawing.Bitmap]::new($baseline.Width, $baseline.Height)
            $graphics = [Drawing.Graphics]::FromImage($candidate)
            try {
                $graphics.InterpolationMode = [Drawing.Drawing2D.InterpolationMode]::HighQualityBicubic
                $graphics.PixelOffsetMode = [Drawing.Drawing2D.PixelOffsetMode]::HighQuality
                $graphics.DrawImage($source, 0, 0, $baseline.Width, $baseline.Height)
            } finally {
                $graphics.Dispose()
            }
        }

        $sumSquare = 0.0
        $sumAbsolute = 0.0
        $changed = 0
        $pixels = 0
        $step = [Math]::Max(1, $SampleStep)
        for ($y = 0; $y -lt $baseline.Height; $y += $step) {
            for ($x = 0; $x -lt $baseline.Width; $x += $step) {
                $a = $baseline.GetPixel($x, $y)
                $b = $candidate.GetPixel($x, $y)
                $dr = [Math]::Abs([int]$a.R - [int]$b.R)
                $dg = [Math]::Abs([int]$a.G - [int]$b.G)
                $db = [Math]::Abs([int]$a.B - [int]$b.B)
                $sumAbsolute += $dr + $dg + $db
                $sumSquare += $dr * $dr + $dg * $dg + $db * $db
                if ([Math]::Max($dr, [Math]::Max($dg, $db)) -gt $PerChannelTolerance) { $changed++ }
                $pixels++
            }
        }
        $rmse = [Math]::Sqrt($sumSquare / [Math]::Max(1, $pixels * 3))
        $meanAbsolute = $sumAbsolute / [Math]::Max(1, $pixels * 3)
        $changedFraction = $changed / [double][Math]::Max(1, $pixels)
        $status = if ($rmse -le $MaxRmse -and $changedFraction -le $MaxChangedFraction) { "pass" } else { "changed" }
        if ($status -ne "pass") { $failed = $true }
        $results.Add([pscustomobject]@{
            name = $baselineFile.Name
            status = $status
            baselineSize = "$($baseline.Width)x$($baseline.Height)"
            candidateSize = "$($source.Width)x$($source.Height)"
            normalizedForComparison = $resized
            sampledPixels = $pixels
            rmse = [Math]::Round($rmse, 4)
            meanAbsoluteError = [Math]::Round($meanAbsolute, 4)
            changedFraction = [Math]::Round($changedFraction, 6)
            perChannelTolerance = $PerChannelTolerance
        })
    } finally {
        if ($candidate -ne $source) { $candidate.Dispose() }
        $source.Dispose()
        $baseline.Dispose()
    }
}

$report = [pscustomobject]@{
    baseline = $baselinePath
    candidate = $candidatePath
    thresholds = [pscustomobject]@{
        maxRmse = $MaxRmse
        maxChangedFraction = $MaxChangedFraction
        perChannelTolerance = $PerChannelTolerance
        sampleStep = $SampleStep
    }
    comparisons = @($results)
}
$outputPath = [IO.Path]::GetFullPath((Join-Path (Get-Location) $OutputJson))
$outputParent = Split-Path -Parent $outputPath
if ($outputParent) { [IO.Directory]::CreateDirectory($outputParent) | Out-Null }
$report | ConvertTo-Json -Depth 6 | Set-Content -LiteralPath $outputPath -Encoding utf8
$results | Format-Table name,status,rmse,meanAbsoluteError,changedFraction,baselineSize,candidateSize -AutoSize

if ($failed -and -not $ReportOnly) {
    throw "Visual regression threshold failed. Baselines were not modified; inspect $outputPath"
}
