$ErrorActionPreference = "Stop"
$ProgressPreference = "SilentlyContinue"

# Portable Python is optional. It lives inside the project and does not change
# PATH, registry entries, or the system Python installation.
$repo = Split-Path -Parent $PSScriptRoot
$version = if ($env:DEEPAISEE_PYTHON_VERSION) { $env:DEEPAISEE_PYTHON_VERSION } else { "3.12.10" }
$tools = Join-Path $repo ".tools"
$destination = Join-Path $tools "python"
$downloads = Join-Path $tools "downloads"
$archive = Join-Path $downloads "python-$version-embed-amd64.zip"
$python = Join-Path $destination "python.exe"

New-Item -ItemType Directory -Force -Path $downloads | Out-Null
New-Item -ItemType Directory -Force -Path $destination | Out-Null
if (-not (Test-Path $python)) {
  if (-not (Test-Path $archive)) {
    $uri = "https://www.python.org/ftp/python/$version/python-$version-embed-amd64.zip"
    Write-Host "Downloading portable Python $version to $archive"
    Invoke-WebRequest -Uri $uri -OutFile $archive
  }
  Expand-Archive -LiteralPath $archive -DestinationPath $destination -Force
}

if (-not (Test-Path $python)) {
  throw "Python sidecar installation failed: python.exe was not found"
}
Write-Host "Python sidecar installed in project: $python"
Write-Host "Transformers/PyTorch adapters are optional and are not installed automatically."
