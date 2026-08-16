param(
    [Parameter(Mandatory = $true)][string]$Path
)
# Count pixels in the violet band of the Shape Studio torus material (B > R > G, saturated
# enough to exclude the grey ground and every other shape colour in the roles scene).
Add-Type -AssemblyName System.Drawing
$bmp = [System.Drawing.Bitmap]::new($Path)
$count = 0
for ($y = 0; $y -lt $bmp.Height; $y += 2) {
    for ($x = 0; $x -lt $bmp.Width; $x += 2) {
        $p = $bmp.GetPixel($x, $y)
        if ($p.B -gt ($p.R + 15) -and $p.R -gt ($p.G + 10) -and $p.B -ge 110 -and $p.R -ge 70) {
            $count += 1
        }
    }
}
$bmp.Dispose()
Write-Output ('{"violet":' + $count + '}')
