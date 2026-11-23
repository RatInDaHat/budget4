@echo off
rem Builds Budget4 the first time (or after changes with "Budget4.bat rebuild"), then starts it.
rem Drag a .ynab4 budget folder onto this file to open it directly.
title Budget4
cd /d "%~dp0"

set EXE=target\release\budget4.exe
if /i "%~1"=="rebuild" (
  shift
  if exist "%EXE%" del "%EXE%"
)

if not exist "%EXE%" (
  where cargo >nul 2>nul || (echo Rust is not installed. Get it from https://rustup.rs and run this again. & pause & exit /b 1)
  where node >nul 2>nul || (echo Node.js is not installed. Get it from https://nodejs.org and run this again. & pause & exit /b 1)
  if not exist node_modules (
    echo Installing dependencies...
    call npm install || (pause & exit /b 1)
  )
  echo Building Budget4. The first build takes a few minutes...
  call npx tauri build --no-bundle || (pause & exit /b 1)
)

start "" "%EXE%" %1
