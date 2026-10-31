param(
  [Parameter(ValueFromRemainingArguments = $true)]
  [string[]] $CliArgs
)

$ErrorActionPreference = "Stop"
$repo = Split-Path -Parent $PSScriptRoot

# Make the normal `npm run tauri dev` command include the native Candle
# backend when the local CUDA toolkit is available. Explicit --features wins.
. (Join-Path $PSScriptRoot "activate-local.ps1")
# Vulkan's shader-generator currently requires a nested MSBuild project that
# is unreliable on this host. Keep the dependable CUDA path as the default;
# callers can opt into the integrated-GPU backend with DEEPAISEE_VULKAN=1.
if (-not $env:DEEPAISEE_VULKAN) { $env:DEEPAISEE_VULKAN = "0" }

# Keep the normal development command deterministic on Windows. CUDA is
# selected automatically when the toolkit is present; Vulkan remains an
# explicit build opt-in because its shader compiler can fail on non-ASCII
# checkout paths. Runtime device selection still exposes all compiled devices.
if (-not $env:DEEPAISEE_VULKAN) { $env:DEEPAISEE_VULKAN = "0" }

$tauri = Join-Path $repo "node_modules\.bin\tauri.cmd"
if (-not (Test-Path -LiteralPath $tauri)) {
  throw "Tauri CLI is missing. Run npm ci first."
}

$hasFeatures = $false
foreach ($arg in $CliArgs) {
  if ($arg -eq "--features" -or $arg -like "--features=*") {
    $hasFeatures = $true
    break
  }
}

$forward = @($CliArgs)
if (-not $hasFeatures -and $CliArgs.Count -gt 0 -and $CliArgs[0] -in @("dev", "build")) {
  $cuda = $env:DEEPAISEE_CUDA_ROOT
  if (-not $cuda) { $cuda = $env:CUDA_PATH }
  $cudaReady = $cuda -and (Test-Path -LiteralPath (Join-Path $cuda "bin\nvcc.exe"))
  $features = if ($cudaReady) { "llama,candle,candle-cuda" } else { "llama,candle" }
  $forward += @("--features", $features)
  Write-Host "Tauri features: $features"
}

& $tauri @forward
exit $LASTEXITCODE
