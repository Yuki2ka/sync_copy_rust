@echo on
taskkill /F /IM sync_copy.exe >nul 2>&1
cargo build
if %errorlevel% neq 0 exit /b %errorlevel%
".\target\debug\sync_copy.exe"
