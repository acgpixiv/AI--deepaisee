$ErrorActionPreference = "Stop"
$ProgressPreference = "SilentlyContinue"

# Build one portable Tauri application. CUDA is selected at build time only to
# include Candle's kernels; the actual GPU index is selected by the app at run time.
$repo = Split-Path -Parent $PSScriptRoot
. (Join-Path $PSScriptRoot "activate-local.ps1")

function Find-CudaRoot {
  $candidates = @()
  if ($env:DEEPAISEE_CUDA_ROOT) { $candidates += $env:DEEPAISEE_CUDA_ROOT }
  if ($env:CUDA_PATH) { $candidates += $env:CUDA_PATH }
  if ($env:CUDA_HOME) { $candidates += $env:CUDA_HOME }
  $candidates += (Join-Path $repo ".tools\cuda")
  $candidates += (Join-Path $repo ".tools\cuda-toolkit")
  $candidates += (Get-ChildItem "C:\Program Files\NVIDIA GPU Computing Toolkit\CUDA\v*" -Directory -ErrorAction SilentlyContinue |
    Sort-Object Name -Descending | Select-Object -ExpandProperty FullName)
  foreach ($candidate in $candidates) {
    if ($candidate -and (Test-Path (Join-Path $candidate "bin\nvcc.exe"))) {
      return (Resolve-Path -LiteralPath $candidate).Path
    }
  }
  return $null
}

function Stage-CudaRuntime([string]$cudaRoot) {
  $runtimeSource = Join-Path $cudaRoot "bin\x64"
  $runtimeTarget = Join-Path $repo ".tools\cuda-runtime"
  New-Item -ItemType Directory -Force -Path $runtimeTarget | Out-Null

  $patterns = @(
    "cublas64_*.dll",
    "cublasLt64_*.dll",
    "cudart64_*.dll",
    "curand64_*.dll",
    "nvrtc64_*.dll",
    "nvrtc-builtins64_*.dll",
    "nvJitLink_*.dll"
  )
  $files = foreach ($pattern in $patterns) {
    Get-ChildItem -LiteralPath $runtimeSource -Filter $pattern -File -ErrorAction SilentlyContinue
  }
  foreach ($required in @("cublas64_*.dll", "cublasLt64_*.dll", "cudart64_*.dll", "curand64_*.dll")) {
    if (-not ($files | Where-Object { $_.Name -like $required })) {
      throw "CUDA Toolkit is missing the required runtime library: $required"
    }
  }
  foreach ($file in $files | Sort-Object Name -Unique) {
    Copy-Item -LiteralPath $file.FullName -Destination (Join-Path $runtimeTarget $file.Name) -Force
  }

  $openMpRuntime = Join-Path $env:VCToolsRedistDir "x64\Microsoft.VC143.OpenMP\vcomp140.dll"
  if (-not (Test-Path -LiteralPath $openMpRuntime)) {
    throw "Visual Studio x64 OpenMP runtime not found: $openMpRuntime"
  }
  Copy-Item -LiteralPath $openMpRuntime -Destination (Join-Path $runtimeTarget "vcomp140.dll") -Force
  Write-Host "Staged $($files.Count + 1) CUDA/MSVC runtime DLLs in $runtimeTarget"
}

Push-Location $repo
try {
  $runtimeDir = Join-Path $repo ".tools\cuda-runtime"
  New-Item -ItemType Directory -Force -Path $runtimeDir | Out-Null
  $cudaRoot = Find-CudaRoot
  $features = "llama,candle,custom-protocol"
  if ($cudaRoot) {
    $env:CUDA_PATH = $cudaRoot
    $env:CUDA_HOME = $cudaRoot
    $env:DEEPAISEE_CUDA = "1"
    $env:Path = "$(Join-Path $cudaRoot 'bin');$env:Path"
    $features = "llama,candle,candle-cuda,custom-protocol"
    Stage-CudaRuntime $cudaRoot
    Write-Host "Building integrated CUDA release with toolkit: $cudaRoot"
  } else {
    $env:DEEPAISEE_CUDA = "0"
    Write-Host "CUDA Toolkit not found; building the native Candle/llama.cpp release without CUDA kernels."
  }

  $tauriCli = Join-Path $repo "node_modules\.bin\tauri.cmd"
  & $tauriCli build --features $features @args
  if ($LASTEXITCODE -ne 0) {
    throw "Tauri release build failed with exit code $LASTEXITCODE"
  }
  if ($cudaRoot) {
    Copy-Item -Path (Join-Path $runtimeDir "*.dll") -Destination (Join-Path $repo "src-tauri\target\release") -Force
  }
} finally {
  Pop-Location
}
