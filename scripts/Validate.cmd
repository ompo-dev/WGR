@echo off
powershell.exe -NoProfile -ExecutionPolicy RemoteSigned -File "%~dp0Validate.ps1" %*
exit /b %ERRORLEVEL%
