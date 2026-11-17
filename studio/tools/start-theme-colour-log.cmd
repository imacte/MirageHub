@echo off
powershell.exe -NoLogo -NoProfile -ExecutionPolicy Bypass -File "%~dp0theme-colour-log.ps1" %*
set "theme_log_result=%ERRORLEVEL%"
echo.
pause
exit /b %theme_log_result%
