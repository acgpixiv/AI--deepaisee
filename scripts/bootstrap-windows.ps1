$ErrorActionPreference = "Stop"
$ProgressPreference = "SilentlyContinue"

# Install portable development dependencies under the repository. System-wide
# MSVC/Windows SDK and the NVIDIA driver remain external prerequisites.
$repo = Split-Path -Parent $PSScriptRoot
. (Join-Path $PSScriptRoot "activate-local.ps1")

# ONNX Runtime is kept beside the project and bundled with the desktop app.
# This is used by the optional CLIP/CLAP ONNX models; it does not modify the
# system installation.
& (Join-Path $PSScriptRoot "install-onnxruntime.ps1")
$tools = Join-Path $repo ".tools"
$downloads = Join-Path $tools "downloads"
New-Item -ItemType Directory -Force -Path $downloads | Out-Null

$env:RUSTUP_DIST_SERVER = "https://rsproxy.cn"
$env:RUSTUP_UPDATE_ROOT = "https://rsproxy.cn/rustup"

function Download-File([string]$url, [string]$path) {
  if (-not (Test-Path $path)) {
    Write-Host "Downloading $url"
    Invoke-WebRequest -Uri $url -OutFile $path
  }
}

# Rustup is installed into .tools/cargo and .tools/rustup by environment.
if (-not (Get-Command rustup -ErrorAction SilentlyContinue)) {
  $rustupExe = Join-Path $downloads "rustup-init.exe"
  Download-File "https://static.rust-lang.org/rustup/dist/x86_64-pc-windows-msvc/rustup-init.exe" $rustupExe
  & $rustupExe -y --default-toolchain stable-x86_64-pc-windows-msvc --profile minimal --no-modify-path
  . (Join-Path $PSScriptRoot "activate-local.ps1")
}
& rustup toolchain install stable --profile minimal --target x86_64-pc-windows-msvc
& rustup default stable-x86_64-pc-windows-msvc

# Portable Node.js, CMake and Ninja. Existing project-local copies win over
# system installations; versions can be changed here without touching C:.
if (-not (Test-Path (Join-Path $tools "node\node-v22.14.0-win-x64\node.exe"))) {
  $nodeZip = Join-Path $downloads "node-v22.14.0-win-x64.zip"
  Download-File "https://nodejs.org/dist/v22.14.0/node-v22.14.0-win-x64.zip" $nodeZip
  $nodeStage = Join-Path $tools "node"
  New-Item -ItemType Directory -Force -Path $nodeStage | Out-Null
  Expand-Archive -LiteralPath $nodeZip -DestinationPath $nodeStage -Force
}
if (-not (Test-Path (Join-Path $tools "cmake\bin\cmake.exe"))) {
  $cmakeZip = Join-Path $downloads "cmake-4.4.4-windows-x86_64.zip"
  Download-File "https://github.com/Kitware/CMake/releases/download/v4.4.4/cmake-4.4.4-windows-x86_64.zip" $cmakeZip
  $cmakeStage = Join-Path $tools "cmake-staging"
  New-Item -ItemType Directory -Force -Path $cmakeStage | Out-Null
  Expand-Archive -LiteralPath $cmakeZip -DestinationPath $cmakeStage -Force
  $cmakeExtracted = Get-ChildItem $cmakeStage -Directory -Filter "cmake-*-windows-x86_64" | Select-Object -First 1
  if ($cmakeExtracted) {
    Move-Item $cmakeExtracted.FullName (Join-Path $tools "cmake") -Force
  }
}
if (-not (Test-Path (Join-Path $tools "ninja\ninja.exe"))) {
  $ninjaZip = Join-Path $downloads "ninja-win.zip"
  Download-File "https://github.com/ninja-build/ninja/releases/latest/download/ninja-win.zip" $ninjaZip
  New-Item -ItemType Directory -Force -Path (Join-Path $tools "ninja") | Out-Null
  Expand-Archive -LiteralPath $ninjaZip -DestinationPath (Join-Path $tools "ninja") -Force
}

# Re-activate so the newly installed commands are visible in this process.
. (Join-Path $PSScriptRoot "activate-local.ps1")

$cudaRoot = if ($env:DEEPAISEE_CUDA_ROOT -and (Test-Path (Join-Path $env:DEEPAISEE_CUDA_ROOT "bin\nvcc.exe"))) {
  $env:DEEPAISEE_CUDA_ROOT
} else {
  Get-ChildItem "C:\Program Files\NVIDIA GPU Computing Toolkit\CUDA\v*" -Directory -ErrorAction SilentlyContinue |
    Sort-Object Name -Descending | Select-Object -First 1 -ExpandProperty FullName
}
if ($cudaRoot) {
  $cudaVersion = Split-Path $cudaRoot -Leaf
  $env:CUDA_PATH = $cudaRoot
  $env:CUDA_HOME = $cudaRoot
  $cudaVersionVariable = "CUDA_PATH_" + $cudaVersion.Replace('.', '_').ToUpperInvariant()
  Set-Item -Path "Env:$cudaVersionVariable" -Value $cudaRoot
  $env:Path = "$cudaRoot\bin;$env:Path"
  if (Test-Path (Join-Path $cudaRoot "bin\x64")) { $env:Path = "$(Join-Path $cudaRoot 'bin\x64');$env:Path" }
}

function Require-Command([string]$name, [string]$hint) {
  if (-not (Get-Command $name -ErrorAction SilentlyContinue)) {
    throw "$name is not available. $hint"
  }
}

Require-Command node "Install Node.js 20 or newer."
Require-Command npm "Install Node.js 20 or newer."
Require-Command cargo "Restart PowerShell after installing Rustup."
Require-Command cmake "Restart PowerShell after installing CMake."

$llamaDir = Join-Path $repo "vendor\llama.cpp"
if (-not (Test-Path (Join-Path $llamaDir "include\llama.h"))) {
  git clone --depth 1 --branch b9999 https://github.com/ggml-org/llama.cpp.git $llamaDir
}

Push-Location $repo
try {
  npm ci
  cargo fetch --manifest-path src-tauri\Cargo.toml
} finally {
  Pop-Location
}

Write-Host "Dependencies are ready under $tools."
Write-Host "For a new shell: . .\scripts\activate-local.ps1"
Write-Host "Build (auto-integrated CUDA when Toolkit is found): powershell -ExecutionPolicy Bypass -File scripts/build-release.ps1"
Write-Host "Build (single exe with Candle + CUDA): npm run tauri build -- --features llama,candle,candle-cuda,custom-protocol"
Write-Host "llama.cpp-only lightweight build: npm run tauri build -- --features llama,custom-protocol"
Write-Host "Force llama.cpp CUDA detection: `$env:DEEPAISEE_CUDA='1'; npm run tauri build -- --features llama,candle,candle-cuda,custom-protocol"
Write-Host "Integrated GPUs: install a Vulkan SDK under .tools/vulkan; it is auto-detected, or force with `$env:DEEPAISEE_VULKAN='1'"
