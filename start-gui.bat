@echo off
rem Start the VisualFingerprint desktop app (Windows).
rem
rem   start-gui.bat                 open the last workspace (or the start page)
rem   start-gui.bat "D:\my-ws"      open a specific workspace folder

setlocal
cd /d "%~dp0"

if not exist ".venv\Scripts\pythonw.exe" (
    echo No virtual environment found in "%~dp0.venv".
    echo Set it up once with:
    echo.
    echo     python -m venv .venv
    echo     .venv\Scripts\pip install -e ".[gui]"
    echo.
    pause
    exit /b 1
)

".venv\Scripts\python.exe" -c "import PySide6" 2>nul
if errorlevel 1 (
    echo The GUI needs PySide6. Install it with:
    echo.
    echo     .venv\Scripts\pip install -e ".[gui]"
    echo.
    pause
    exit /b 1
)

where ffmpeg >nul 2>nul
if errorlevel 1 (
    echo WARNING: ffmpeg was not found on PATH. Images work, but video and audio need ffmpeg.
    echo Install it and restart this script. Starting anyway in 4 seconds...
    timeout /t 4 /nobreak >nul
)

if not "%~1"=="" set "VFP_HOME=%~1"

rem pythonw has no console window; "start" returns at once so this window closes.
start "" ".venv\Scripts\pythonw.exe" -m vfp.gui.app
exit /b 0
