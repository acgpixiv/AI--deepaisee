$ErrorActionPreference = "Stop"

$repo = Split-Path -Parent $PSScriptRoot
# Toolchains live in .tools by default; checkouts that unpacked them next to the
# repo (tools/) must keep working too, so probe both before giving up.
$tools = Join-Path $repo ".tools"
if (-not (Test-Path -LiteralPath (Join-Path $tools "cargo"))) {
  $fallbackTools = Join-Path $repo "tools"
  if (Test-Path -LiteralPath (Join-Path $fallbackTools "cargo")) { $tools = $fallbackTools }
}
$cargoHome = Join-Path $tools "cargo"
$rustupHome = Join-Path $tools "rustup"
$nodeHome = Join-Path $tools "node\node-v22.14.0-win-x64"
# Some checkouts keep a plain tools\node folder instead of the pinned versioned one.
if (-not (Test-Path (Join-Path $nodeHome "node.exe"))) { $nodeHome = Join-Path $tools "node" }
$cmakeHome = Join-Path $tools "cmake"
$ninjaHome = Join-Path $tools "ninja"
# The MSYS2 UCRT64 toolchain ships cmake/ninja/gcc and is the fallback on this host.
$ucrt64Bin = Join-Path $tools "msys64\ucrt64\bin"
$npmCache = Join-Path $tools "npm-cache"

$env:DEEPAISEE_REPO = $repo
$env:CARGO_HOME = $cargoHome
$env:RUSTUP_HOME = $rustupHome
# Keep an externally chosen target dir (CI / limited-disk hosts may redirect it),
# but normalize relative values. Cargo is often invoked from `src-tauri`; a
# relative `src-tauri/target` would otherwise become `src-tauri/src-tauri/target`
# and produce very long CMake paths on Windows.
if ($env:CARGO_TARGET_DIR) {
  if (-not [System.IO.Path]::IsPathRooted($env:CARGO_TARGET_DIR)) {
    $env:CARGO_TARGET_DIR = [System.IO.Path]::GetFullPath((Join-Path $repo $env:CARGO_TARGET_DIR))
  }
} else {
  $env:CARGO_TARGET_DIR = Join-Path $repo "src-tauri\target"
}
$env:npm_config_cache = $npmCache
$env:npm_config_prefix = Join-Path $tools "npm-global"

$localPaths = @(
  (Join-Path $cargoHome "bin"),
  $nodeHome,
  (Join-Path $cmakeHome "bin"),
  $ninjaHome,
  $ucrt64Bin
)

# Prefer a project-local CUDA Toolkit when one has been unpacked or installed.
$localCuda = Join-Path $tools "cuda"
$cudaRoot = if (Test-Path (Join-Path $localCuda "bin\nvcc.exe")) {
  $localCuda
} else {
  Get-ChildItem "C:\Program Files\NVIDIA GPU Computing Toolkit\CUDA\v*" -Directory -ErrorAction SilentlyContinue |
    Sort-Object Name -Descending | Select-Object -First 1 -ExpandProperty FullName
}

$localVulkan = Join-Path $tools "vulkan"
$vulkanRoot = if (Test-Path (Join-Path $localVulkan "Include\vulkan\vulkan.h")) {
  $localVulkan
} elseif ($env:VULKAN_SDK -and (Test-Path (Join-Path $env:VULKAN_SDK "Include\vulkan\vulkan.h"))) {
  $env:VULKAN_SDK
} else {
  $null
}
if ($vulkanRoot) {
  $env:VULKAN_SDK = $vulkanRoot
  $env:DEEPAISEE_VULKAN_ROOT = $vulkanRoot
  $localPaths += Join-Path $vulkanRoot "Bin"
}
if ($cudaRoot -and (Test-Path (Join-Path $cudaRoot "bin\nvcc.exe"))) {
  $env:CUDA_PATH = $cudaRoot
  $env:CUDA_HOME = $cudaRoot
  $env:DEEPAISEE_CUDA_ROOT = $cudaRoot
  $cudaVersion = Split-Path $cudaRoot -Leaf
  Set-Item -Path ("Env:CUDA_PATH_" + $cudaVersion.Replace('.', '_').ToUpperInvariant()) -Value $cudaRoot
  $localPaths += Join-Path $cudaRoot "bin"
  if (Test-Path (Join-Path $cudaRoot "bin\x64")) { $localPaths += Join-Path $cudaRoot "bin\x64" }
}

$env:Path = (($localPaths + ($env:Path -split ';')) | Where-Object { $_ -and (Test-Path $_) } | Select-Object -Unique) -join ';'

# CUDA's nvcc uses the MSVC host compiler for both Candle PTX and llama.cpp.
# A normal PowerShell session does not have the INCLUDE/LIB variables that
# VsDevCmd sets, so load the installed x64 toolchain here when available.
function Import-VisualStudioEnvironment {
  $vswhere = @(
    (Join-Path ${env:ProgramFiles(x86)} 'Microsoft Visual Studio\Installer\vswhere.exe'),
    (Join-Path $env:ProgramFiles 'Microsoft Visual Studio\Installer\vswhere.exe')
  ) | Where-Object { $_ -and (Test-Path -LiteralPath $_) } | Select-Object -First 1
  if (-not $vswhere) { return }

  $installation = (& $vswhere -latest -products * -requires Microsoft.VisualStudio.Component.VC.Tools.x86.x64 -property installationPath 2>$null | Select-Object -First 1)
  if (-not $installation) { return }
  $vsdevcmd = Join-Path $installation 'Common7\Tools\VsDevCmd.bat'
  if (-not (Test-Path -LiteralPath $vsdevcmd)) { return }

  $cmd = '"{0}" -arch=x64 -host_arch=x64 && set' -f $vsdevcmd
  foreach ($line in (& cmd.exe /d /s /c $cmd)) {
    if ($line -match '^([^=]+)=(.*)$') {
      [Environment]::SetEnvironmentVariable($Matches[1], $Matches[2], 'Process')
    }
  }
  $env:NVCC_CCBIN = 'cl.exe'
  Write-Host "Visual Studio x64 toolchain active: $installation"
}

Import-VisualStudioEnvironment
Write-Host "Project-local environment active: $repo"
Write-Host "Cargo home: $env:CARGO_HOME"
Write-Host "Cargo target: $env:CARGO_TARGET_DIR"
