@echo off
rem deepaisee launcher (ASCII only: cmd.exe mis-parses non-ASCII .bat)
cd /d "%~dp0"
if not exist "deepaisee.exe" (
    echo deepaisee.exe not found in this folder.
    echo Please extract the whole package, not just this file.
    pause
    exit /b 1
)
if not exist "WebView2Loader.dll" (
    echo Missing WebView2Loader.dll next to deepaisee.exe
    echo Re-extract the package.
    pause
    exit /b 1
)
start "" "deepaisee.exe"
