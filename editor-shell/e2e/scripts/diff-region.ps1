# How much of one rectangle of the picture CHANGED between two captures, and which way.
#
# For asserting that a render cue actually reached the pixels. A screenshot proves a badge said
# something; only a number proves the object it named looks different from its neighbours. This is
# deliberately content-agnostic  -  it does not know what colour a hover is  -  so it cannot be satisfied by
# a tint the author picked to match the assertion. The direction of the change is reported separately,
# as the mean per-channel delta over the pixels that moved, so a caller can also say WHICH way.
#
# The rectangle matters as much as the difference: a capture of this app is the whole WINDOW, and the
# docks, badge and toasts inside it change for reasons that have nothing to do with the 3D. Pass the
# viewport region only. (Same lesson as the film gate's interdecile luma, measured on the window and
# pinned near 233 by the white docks  -  `window-scope-image-metrics`.)
param(
  [Parameter(Mandatory = $true)][string]$Before,
  [Parameter(Mandatory = $true)][string]$After,
  [Parameter(Mandatory = $true)][int]$X,
  [Parameter(Mandatory = $true)][int]$Y,
  [Parameter(Mandatory = $true)][int]$Width,
  [Parameter(Mandatory = $true)][int]$Height,
  # Per-channel absolute difference a pixel must exceed to count as changed. 12 is above the noise a
  # lossless PrintWindow capture of a static frame produces (measured 0) with room for the renderer's
  # own dithering, and far below the cue.
  [int]$Threshold = 12,
  # Sample every Nth pixel in each axis. The counts are reported as SAMPLED, never scaled up to an
  # estimate of the full rectangle: an extrapolated pixel count reads as a measurement and is not one.
  [int]$Step = 2
)

$ErrorActionPreference = "Stop"
Add-Type -AssemblyName System.Drawing

$a = [System.Drawing.Bitmap]::FromFile((Resolve-Path -LiteralPath $Before))
$b = [System.Drawing.Bitmap]::FromFile((Resolve-Path -LiteralPath $After))
try {
  if ($a.Width -ne $b.Width -or $a.Height -ne $b.Height) {
    throw "The two captures are different sizes ($($a.Width)x$($a.Height) vs $($b.Width)x$($b.Height))  -  the window moved or resized between them, and no pixel in one is about the same place in the other."
  }
  $left = [Math]::Max(0, $X)
  $top = [Math]::Max(0, $Y)
  $right = [Math]::Min($a.Width - 1, $X + $Width - 1)
  $bottom = [Math]::Min($a.Height - 1, $Y + $Height - 1)
  if ($right -lt $left -or $bottom -lt $top) {
    throw "The region ${X},${Y} ${Width}x${Height} does not intersect the $($a.Width)x$($a.Height) capture."
  }

  $sampled = 0
  $changed = 0
  $sumR = 0.0; $sumG = 0.0; $sumB = 0.0
  for ($j = $top; $j -le $bottom; $j += $Step) {
    for ($i = $left; $i -le $right; $i += $Step) {
      $p = $a.GetPixel($i, $j)
      $q = $b.GetPixel($i, $j)
      $sampled++
      $dr = [int]$q.R - [int]$p.R
      $dg = [int]$q.G - [int]$p.G
      $db = [int]$q.B - [int]$p.B
      if ([Math]::Abs($dr) -gt $Threshold -or [Math]::Abs($dg) -gt $Threshold -or [Math]::Abs($db) -gt $Threshold) {
        $changed++
        $sumR += $dr; $sumG += $dg; $sumB += $db
      }
    }
  }

  $meanR = if ($changed -gt 0) { [Math]::Round($sumR / $changed, 2) } else { 0 }
  $meanG = if ($changed -gt 0) { [Math]::Round($sumG / $changed, 2) } else { 0 }
  $meanB = if ($changed -gt 0) { [Math]::Round($sumB / $changed, 2) } else { 0 }
  $percent = if ($sampled -gt 0) { [Math]::Round(100.0 * $changed / $sampled, 3) } else { 0 }

  [ordered]@{
    sampled = $sampled
    changed = $changed
    percent = $percent
    step = $Step
    threshold = $Threshold
    meanDeltaR = $meanR
    meanDeltaG = $meanG
    meanDeltaB = $meanB
    region = "${left},${top} $(($right - $left) + 1)x$(($bottom - $top) + 1)"
  } | ConvertTo-Json -Compress
}
finally {
  $a.Dispose()
  $b.Dispose()
}
