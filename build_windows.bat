@echo off
rem Build the Rust COM Port Watcher on Windows -> target\release\com-port-watcher-rs.exe
cd /d "%~dp0"
where cargo >nul 2>nul
if errorlevel 1 (
  echo cargo not found. Install Rust from https://rustup.rs first.
  pause
  exit /b 1
)
cargo build --release
if errorlevel 1 goto :fail
echo.
echo Done: target\release\com-port-watcher-rs.exe
pause
exit /b 0
:fail
echo.
echo Build FAILED - see messages above.
pause
exit /b 1
