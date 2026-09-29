@echo off
REM ============================================================
REM  Builds SPClientFixes.dll (x64) from client-fixes\src and copies it to
REM  src-tauri\resources, where the launcher embeds it at build time.
REM
REM  Run this from the "x64 Native Tools Command Prompt for VS"
REM  (Start menu -> Visual Studio -> Tools). Only that shell knows cl.exe.
REM  Same result as the CMake build in README.md, without needing CMake.
REM
REM  Usage:   client-fixes\build.bat
REM ============================================================
setlocal
cd /d "%~dp0"

where cl >nul 2>&1
if errorlevel 1 (
    echo.
    echo [ERROR] cl.exe not found.
    echo   Start this script from the "x64 Native Tools Command Prompt for VS".
    echo.
    pause
    exit /b 1
)

if not exist build mkdir build
echo Building SPClientFixes.dll ...
cl /nologo /LD /O2 /EHsc /std:c++20 /DNDEBUG /DWIN32_LEAN_AND_MEAN /DNOMINMAX ^
   src\client_fixes.cpp src\custom_pak_signing.cpp src\standalone_options.cpp /Fo:build\ /Fe:build\SPClientFixes.dll /link bcrypt.lib
if errorlevel 1 ( echo. & echo [ERROR] Build failed. & pause & exit /b 1 )

if not exist ..\src-tauri\resources mkdir ..\src-tauri\resources
copy /y build\SPClientFixes.dll ..\src-tauri\resources\SPClientFixes.dll >nul
if errorlevel 1 ( echo. & echo [ERROR] Copy to src-tauri\resources failed. & pause & exit /b 1 )

echo.
echo [OK] Built and copied:
echo      src-tauri\resources\SPClientFixes.dll
echo.
pause
endlocal
