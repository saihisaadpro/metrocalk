# Mean RGB (and spread) over a square patch of a captured PNG, as JSON on stdout.
#
# For asserting a rendered colour rather than describing it: a screenshot shows that the background "looks
# dark", but only a number shows that it is the byte the author typed. `spread` is the max per-channel
# range across the patch, so a caller can tell "this really is a flat fill" from "I sampled a gradient".
param(
  [Parameter(Mandatory = $true)][string]$Path,
  [Parameter(Mandatory = $true)][int]$X,
  [Parameter(Mandatory = $true)][int]$Y,
  [int]$Radius = 8
)

$ErrorActionPreference = "Stop"
Add-Type -AssemblyName System.Drawing

$bitmap = [System.Drawing.Bitmap]::FromFile((Resolve-Path -LiteralPath $Path))
try {
  $left = [Math]::Max(0, $X - $Radius)
  $top = [Math]::Max(0, $Y - $Radius)
  $right = [Math]::Min($bitmap.Width - 1, $X + $Radius)
  $bottom = [Math]::Min($bitmap.Height - 1, $Y + $Radius)

  $sum = @{ r = 0.0; g = 0.0; b = 0.0 }
  $min = @{ r = 255; g = 255; b = 255 }
  $max = @{ r = 0; g = 0; b = 0 }
  $count = 0
  for ($j = $top; $j -le $bottom; $j++) {
    for ($i = $left; $i -le $right; $i++) {
      $p = $bitmap.GetPixel($i, $j)
      $sum.r += $p.R; $sum.g += $p.G; $sum.b += $p.B
      if ($p.R -lt $min.r) { $min.r = $p.R }; if ($p.R -gt $max.r) { $max.r = $p.R }
      if ($p.G -lt $min.g) { $min.g = $p.G }; if ($p.G -gt $max.g) { $max.g = $p.G }
      if ($p.B -lt $min.b) { $min.b = $p.B }; if ($p.B -gt $max.b) { $max.b = $p.B }
      $count++
    }
  }
  if ($count -eq 0) { throw "Empty sample region at ($X,$Y) r=$Radius in '$Path'." }

  $spread = [Math]::Max([Math]::Max($max.r - $min.r, $max.g - $min.g), $max.b - $min.b)
  [pscustomobject]@{
    r      = [Math]::Round($sum.r / $count, 3)
    g      = [Math]::Round($sum.g / $count, 3)
    b      = [Math]::Round($sum.b / $count, 3)
    spread = $spread
    pixels = $count
  } | ConvertTo-Json -Compress
}
finally { $bitmap.Dispose() }
