$ErrorActionPreference = "Stop"

# Keep the native runtime under the repository. Nothing is removed by this
# script; an existing runtime is left in place and a downloaded archive is
# only expanded into a new project-local directory.
$repo = Split-Path -Parent $PSScriptRoot
$version = if ($env:DEEPAISEE_ORT_VERSION) { $env:DEEPAISEE_ORT_VERSION } else { "1.20.1" }
$destination = Join-Path $repo ".tools\onnxruntime"
$native = Join-Path $destination "onnxruntime.dll"
$archive = Join-Path $repo ".tools\downloads\onnxruntime-$version.zip"

New-Item -ItemType Directory -Force -Path (Split-Path -Parent $archive) | Out-Null
New-Item -ItemType Directory -Force -Path $destination | Out-Null

# Reuse the checked-in/portable DLL when it is already present. This avoids a
# second 13 MB copy and still makes the installation explicit and verifiable.
$portable = Join-Path $repo "onnxruntime.dll"
if ((Test-Path $portable) -and -not (Test-Path $native)) {
  Copy-Item -LiteralPath $portable -Destination $native
}

if (-not (Test-Path $native)) {
  $uri = "https://www.nuget.org/api/v2/package/Microsoft.ML.OnnxRuntime/$version"
  if (-not (Test-Path $archive)) {
    Write-Host "Downloading ONNX Runtime $version to $archive"
    Invoke-WebRequest -Uri $uri -OutFile $archive
  }
  $extract = Join-Path $destination "package-$version"
  New-Item -ItemType Directory -Force -Path $extract | Out-Null
  Expand-Archive -LiteralPath $archive -DestinationPath $extract -Force
  $candidate = Get-ChildItem -LiteralPath $extract -Filter "onnxruntime.dll" -File -Recurse | Select-Object -First 1
  if (-not $candidate) {
    throw "NuGet 包中没有找到 onnxruntime.dll"
  }
  Copy-Item -LiteralPath $candidate.FullName -Destination $native
}

$length = (Get-Item -LiteralPath $native).Length
if ($length -lt 1000000) {
  throw "onnxruntime.dll 文件异常（$length bytes）：$native"
}
Write-Host "ONNX Runtime installed in project: $native ($length bytes)"
Write-Host "Set DEEPAISEE_ORT_DLL to override this file for a portable build."
