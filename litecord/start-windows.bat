@echo off
title LiteCord server
cd /d "%~dp0"
where py >nul 2>nul
if %errorlevel%==0 (
  py -3 server.py %*
  goto done
)
where python >nul 2>nul
if %errorlevel%==0 (
  python server.py %*
  goto done
)
echo.
echo LiteCord needs Python 3, which isn't installed.
echo Get it free from https://www.python.org/downloads/
echo While installing, tick "Add python.exe to PATH". Then double-click this file again.
:done
echo.
pause
