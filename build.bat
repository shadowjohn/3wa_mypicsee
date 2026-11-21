@echo off
setlocal enableextensions
if /i "%~1"=="--help" goto :help
pushd "%~dp0" || exit /b 1
where cargo.exe >nul 2>&1
if errorlevel 1 if exist "%USERPROFILE%\.cargo\bin\cargo.exe" set "PATH=%USERPROFILE%\.cargo\bin;%PATH%"
where cargo.exe >nul 2>&1
if errorlevel 1 (
  echo Rust Cargo not found. Install the x64 Rust MSVC toolchain and Visual Studio C++ Build Tools.
  goto :fail
)
set "TARGET=x86_64-pc-windows-msvc"
cargo build --locked --release --target %TARGET% --target-dir target
if errorlevel 1 (
  echo Build failed. Check Rust MSVC and Visual Studio Desktop development with C++.
  goto :fail
)
if not exist "dist\." mkdir "dist" || goto :fail
copy /y "target\%TARGET%\release\mypicsee.exe" "dist\3wa_mypicsee.exe" >nul || goto :fail
copy /y "LICENSE" "dist\LICENSE" >nul || goto :fail
copy /y "THIRD_PARTY_NOTICES.txt" "dist\THIRD_PARTY_NOTICES.txt" >nul || goto :fail
echo Built: "%CD%\dist\3wa_mypicsee.exe"
popd
exit /b 0
:fail
popd
exit /b 1
:help
echo Usage: build.bat
echo Builds Windows x64 MSVC release to dist\3wa_mypicsee.exe.
exit /b 0
