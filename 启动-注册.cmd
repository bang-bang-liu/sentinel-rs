@echo off
setlocal
cd /d "%~dp0"
powershell.exe -NoProfile -ExecutionPolicy Bypass -File "%~dp0start.ps1" %*
set "SENTINEL_EXIT_CODE=%ERRORLEVEL%"
echo.
echo Process finished. Exit code: %SENTINEL_EXIT_CODE%
pause >nul
exit /b %SENTINEL_EXIT_CODE%
