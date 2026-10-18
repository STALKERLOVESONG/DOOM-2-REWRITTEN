@echo off
rem Build the game and the launcher, and put a ready-to-play folder in dist\DoomBevy.
rem Needs Rust (https://rustup.rs). The first build takes several minutes.
setlocal
cd /d "%~dp0"

cargo build --release -p doom_bevy -p doom_launcher || goto :fail

set OUT=dist\DoomBevy
if not exist "%OUT%\WADS" mkdir "%OUT%\WADS"
copy /y target\release\doom_bevy.exe "%OUT%\" >nul || goto :fail
copy /y target\release\doom_launcher.exe "%OUT%\" >nul || goto :fail
xcopy /e /i /y /q RESOURCES "%OUT%\RESOURCES" >nul || goto :fail
copy /y docs\PLAYING.txt "%OUT%\README.txt" >nul

rem Use the IWADs from this checkout's WADS folder (never committed).
if exist WADS\*.wad copy /y WADS\*.wad "%OUT%\WADS\" >nul

echo.
echo Done: %OUT%\doom_launcher.exe
start "" "%OUT%"
exit /b 0

:fail
echo.
echo Build failed; see the messages above.
pause
exit /b 1
