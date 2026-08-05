# Never-black analyzer for the OS-captured composite shots: samples the CENTER of each PNG (the wgpu
# viewport region, inside the React panel chrome) and reports unique colors + brightness stats. A black-hole
# composite (the transparent-DOM trap) or a blank-white lock-screen capture shows up as uniqueColors≈1.
param([string]$Dir = ".shots-steprender")
Add-Type -AssemblyName System.Drawing
Get-ChildItem $Dir -Filter *.png | Sort-Object Name | ForEach-Object {
    $bmp = [System.Drawing.Bitmap]::FromFile($_.FullName)
    # Center crop: skip 25% margins (panels/header) — sample the stage.
    $x0 = [int]($bmp.Width * 0.28); $x1 = [int]($bmp.Width * 0.72)
    $y0 = [int]($bmp.Height * 0.25); $y1 = [int]($bmp.Height * 0.75)
    $colors = @{}; $sum = 0.0; $sumsq = 0.0; $n = 0
    for ($x = $x0; $x -lt $x1; $x += 12) {
        for ($y = $y0; $y -lt $y1; $y += 12) {
            $c = $bmp.GetPixel($x, $y)
            $colors["$($c.R),$($c.G),$($c.B)"] = 1
            $b = ($c.R + $c.G + $c.B) / 3.0
            $sum += $b; $sumsq += $b * $b; $n++
        }
    }
    $mean = $sum / $n
    $var = [Math]::Max(0, ($sumsq / $n) - ($mean * $mean))
    $verdict = if ($colors.Count -le 3) { "SUSPECT-BLANK" } else { "OK" }
    "{0,-34} {1,5} colors  mean={2,5:F1} sd={3,5:F1}  {4}" -f $_.Name, $colors.Count, $mean, [Math]::Sqrt($var), $verdict
    $bmp.Dispose()
}
