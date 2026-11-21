@echo off
setlocal
chcp 65001 >nul 2>&1
cd /d "%~dp0"
if not exist "%~dp0mq.exe" (
    echo [MQ] Error: mq.exe not found in "%~dp0"
    pause
    exit /b 1
)
start "" "%~dp0mq.exe" %*
endlocal
exit /b 0
