@echo off
rem ===============================================================
rem  DeepDark_Oracle launcher - ASCII only, do not put Chinese here.
rem  Chinese text lives in launcher.ps1 (UTF-8 with BOM).
rem ===============================================================
cd /d "%~dp0"

if not exist "launcher.ps1" (
    echo [launcher.ps1 is missing] - please keep this file next to DeepDark_Oracle.exe
    pause
    exit /b 1
)

set "PS=%SystemRoot%\System32\WindowsPowerShell\v1.0\powershell.exe"
if not exist "%PS%" set "PS=powershell.exe"

title DeepDark_Oracle
"%PS%" -NoLogo -NoProfile -ExecutionPolicy Bypass -File "%~dp0launcher.ps1"
