# M15.11 — run the HDR-pipeline visual verification once per RENDERER CONFIGURATION.
#
# MSAA / SSAO / bloom / sky are read from the environment once at device creation, so they cannot be
# switched inside a session: proving all four SSAO×bloom routes (and both MSAA extremes, and the sky-off
# case that makes the clear colour visible) means launching the packaged .exe once per combination.
#
# Each run writes into editor-shell/e2e/.shots-hdr/<label>/ with its own audit.json recording the exact
# configuration, so the whole set can be reviewed side by side.
#
# Run:  powershell -NoProfile -ExecutionPolicy Bypass -File scripts/hdr-capture-all.ps1
#       ... -Only "ssao-off-bloom-off,msaa-off"     # a subset, by label
param(
  [string]$Only = ""
)

$ErrorActionPreference = "Stop"
$here = Split-Path -Parent $MyInvocation.MyCommand.Path
$e2e = Split-Path -Parent $here
$wdio = Join-Path $e2e "node_modules\@wdio\cli\bin\wdio.js"
if (-not (Test-Path -LiteralPath $wdio)) {
  throw "wdio not installed. Run `node bootstrap.mjs` in $e2e first."
}

# The route matrix, plus the configurations that change how the scene reaches it. `sky-off` is the only
# configuration in which the clear colour is on screen at all — the sky otherwise covers every background
# pixel — so it is the one that can show the background landing in the same colour space as the scene.
$configurations = @(
  @{ label = "ssao-off-bloom-off"; env = @{ MTK_SSAO = "off"; MTK_BLOOM = "off" } },
  @{ label = "ssao-on-bloom-off";  env = @{ MTK_SSAO = "on";  MTK_BLOOM = "off" } },
  @{ label = "ssao-off-bloom-on";  env = @{ MTK_SSAO = "off"; MTK_BLOOM = "on" } },
  @{ label = "ssao-on-bloom-on";   env = @{ MTK_SSAO = "on";  MTK_BLOOM = "on" } },
  @{ label = "msaa-off";           env = @{ MTK_MSAA = "off" } },
  # Asks for 8x; the renderer clamps to the highest count the DEVICE can actually use for both the HDR
  # colour target and the depth target, and logs the downgrade. On a device where that is 4x, this frame is
  # the highest-quality AA available — which is the thing worth capturing.
  @{ label = "msaa-highest";       env = @{ MTK_MSAA = "8" } },
  @{ label = "sky-off";            env = @{ MTK_SKY = "off" } }
)

if ($Only) {
  $wanted = $Only.Split(",") | ForEach-Object { $_.Trim() }
  $configurations = $configurations | Where-Object { $wanted -contains $_.label }
  if ($configurations.Count -eq 0) { throw "No configuration matched -Only '$Only'." }
}

# Reset EVERY switch to its default before each run, so a setting from the previous configuration cannot
# leak into the next one. Set to the explicit default rather than unset: `Remove-Item env:` is refused in
# some sandboxed shells, and an explicit value is in any case the clearer record of what was exercised.
$defaults = @{ MTK_SSAO = "on"; MTK_BLOOM = "on"; MTK_MSAA = "4"; MTK_SKY = "on" }

$results = @()
foreach ($configuration in $configurations) {
  foreach ($key in $defaults.Keys) { Set-Item "env:$key" $defaults[$key] }
  foreach ($key in $configuration.env.Keys) { Set-Item "env:$key" $configuration.env[$key] }
  $env:MTK_SHOT_DIR = $configuration.label

  $summary = ($configuration.env.GetEnumerator() | ForEach-Object { "$($_.Key)=$($_.Value)" }) -join " "
  Write-Output ""
  Write-Output "=== $($configuration.label)  [$summary] ==============================="

  Push-Location $e2e
  try {
    & node $wdio run wdio.hdr.conf.js
    $code = $LASTEXITCODE
  }
  finally { Pop-Location }

  $results += [pscustomobject]@{ Configuration = $configuration.label; Settings = $summary; ExitCode = $code }
  if ($code -ne 0) { Write-Output "!! $($configuration.label) FAILED (exit $code)" }
  Start-Sleep -Seconds 2   # let the previous app process fully exit before the next launch
}

foreach ($key in $defaults.Keys) { Set-Item "env:$key" $defaults[$key] }

Write-Output ""
Write-Output "=== HDR capture summary ==================================================="
$results | Format-Table -AutoSize
$failed = @($results | Where-Object { $_.ExitCode -ne 0 })
if ($failed.Count -gt 0) {
  throw "$($failed.Count) of $($results.Count) configuration(s) failed."
}
Write-Output "All $($results.Count) configuration(s) captured."
