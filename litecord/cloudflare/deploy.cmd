@echo off
rem Puts the web app live from the latest main on GitHub. Normally GitHub does this by itself
rem (.github/workflows/cloudflare.yml); this is the backup. Double-click it, or run it in a terminal.
rem The first time, wrangler opens the browser to sign in to the LiteCord Cloudflare account.
setlocal
cd /d "%~dp0"
git fetch -q origin main || goto fail
for /f %%i in ('git status --porcelain -- .') do (
  echo You have changes in litecord\cloudflare that aren't committed. Commit and push them first.
  goto fail
)
git pull -q --ff-only origin main || (echo Couldn't update from GitHub. Push or sort out your commits first. & goto fail)
for /f %%i in ('git rev-parse HEAD') do set HEAD=%%i
for /f %%i in ('git rev-parse origin/main') do set MAIN=%%i
if not "%HEAD%"=="%MAIN%" (echo You have commits that aren't on GitHub yet. Push them first: git push & goto fail)
for /f "delims=" %%i in ('git log -1 --format^="%%h %%s"') do echo Deploying %%i
call npx --yes wrangler@4.158.0 deploy || goto fail
echo Done.
pause
exit /b 0
:fail
pause
exit /b 1
