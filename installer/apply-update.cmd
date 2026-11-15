@echo off
cd /d "%~dp0"
powershell -NoProfile -ExecutionPolicy Bypass -File "%~dp0apply-update.ps1"
if errorlevel 1 (
  echo.
  echo The update did not finish.
  pause
  exit /b 1
)
echo.
pause
