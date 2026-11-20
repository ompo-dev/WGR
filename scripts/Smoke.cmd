@echo off
powershell.exe -NoProfile -ExecutionPolicy RemoteSigned -File "%~dp0Smoke.ps1" %*
exit /b %ERRORLEVEL%
