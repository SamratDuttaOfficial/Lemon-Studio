@echo off
rem Copyright (C) 2026 Samrat Dutta
rem SPDX-License-Identifier: GPL-3.0-or-later
rem This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

rem Builds Lemon Studio and writes a Windows installer to dist\ at the repository root.
rem
rem   tools\installer\build.bat [--no-plugins]
rem
rem   1. Installs any missing prerequisites with winget (rustup, LLVM, VS Build Tools + C++ workload, Inno Setup).
rem   2. Builds lemonstudio.exe and Lemon Studio's VST3/CLAP plugins with the MSVC Rust toolchain.
rem   3. Compiles tools\installer\lemonstudio.iss into dist\LemonStudio-Setup-<version>-<commit>.exe (older installers
rem      are removed).
rem
rem   --no-plugins  leave the VST3/CLAP plugins out of the installer (Lemon Studio has them built in either way)
setlocal EnableExtensions

set "HERE=%~dp0"
set "HERE=%HERE:~0,-1%"
for %%r in ("%HERE%\..\..") do set "REPO=%%~fr"
set "STAGE=%REPO%\target\installer\stage"
set "DIST=%REPO%\dist"
set "ISS=%HERE%\lemonstudio.iss"
set "TOOLCHAIN=stable-x86_64-pc-windows-msvc"

set PLUGINS=1
:args
if "%~1"=="" goto :args_done
if /i "%~1"=="--no-plugins" (set PLUGINS=0& shift & goto :args)
echo unknown option: %~1
exit /b 2
:args_done

set "PATH=%USERPROFILE%\.cargo\bin;%PATH%"

rem ---------------------------------------------------------------------------------------------------------------------
call :step "Checking prerequisites"
call :ensure "rustup" Rustlang.Rustup :find_rustup || goto :fail
call :ensure "LLVM"   LLVM.LLVM       :find_llvm   || goto :fail
call :ensure "VS Build Tools (C++)" Microsoft.VisualStudio.2022.BuildTools :find_vctools ^
    --override "--quiet --wait --norestart --add Microsoft.VisualStudio.Workload.VCTools --includeRecommended" || goto :fail
call :ensure "Inno Setup" JRSoftware.InnoSetup :find_iscc || goto :fail
call :find_llvm
call :find_iscc

call :step "Selecting the %TOOLCHAIN% toolchain"
rustup toolchain install %TOOLCHAIN% --profile minimal || goto :fail

for /f "tokens=2 delims==" %%v in ('findstr /r /b /c:"version *=" "%REPO%\Cargo.toml"') do if not defined VERSION set "VERSION=%%~v"
set "VERSION=%VERSION: =%"
set "VERSION=%VERSION:"=%"
set "COMMIT=local"
for /f %%c in ('git -C "%REPO%" rev-parse --short HEAD 2^>nul') do set "COMMIT=%%c"

pushd "%REPO%"
call :step "Building Lemon Studio %VERSION% (%COMMIT%)"
cargo +%TOOLCHAIN% build --release -p os-app || (popd & goto :fail)
if "%PLUGINS%"=="1" (
    call :step "Building the VST3 and CLAP plugins"
    cargo +%TOOLCHAIN% run --release -p bundle-plugins || (popd & goto :fail)
)
popd

rem ---------------------------------------------------------------------------------------------------------------------
call :step "Staging files"
if exist "%STAGE%" rmdir /s /q "%STAGE%"
mkdir "%STAGE%\app" "%STAGE%\VST3" "%STAGE%\CLAP"
copy /y "%REPO%\target\release\lemonstudio.exe" "%STAGE%\app\" >nul || goto :fail
copy /y "%REPO%\LICENSE" "%STAGE%\app\LICENSE.txt" >nul || goto :fail
copy /y "%REPO%\THIRD-PARTY-NOTICES.txt" "%STAGE%\app\" >nul || goto :fail
if "%PLUGINS%"=="1" (
    rem Bundles are folders (VST3) or single files (CLAP on Windows); copy whichever each one is.
    for %%t in (vst3 clap) do (
        for /d %%p in ("%REPO%\target\bundled\*.%%t") do xcopy /e /i /q /y "%%p" "%STAGE%\%%t\%%~nxp\" >nul || goto :fail
        for %%p in ("%REPO%\target\bundled\*.%%t") do copy /y "%%p" "%STAGE%\%%t\" >nul || goto :fail
    )
)

call :step "Compiling the installer"
if not exist "%DIST%" mkdir "%DIST%"
del /q "%DIST%\LemonStudio-Setup-*.exe" 2>nul
"%ISCC%" /Q "/DAppVersion=%VERSION%" "/DCommit=%COMMIT%" "/DStage=%STAGE%" "/DRepo=%REPO%" "/DOutDir=%DIST%" ^
    "/DWithPlugins=%PLUGINS%" "%ISS%" || goto :fail

for %%f in ("%DIST%\LemonStudio-Setup-*.exe") do echo. & echo Done: %%~ff
exit /b 0

:fail
echo.
echo Build failed.
exit /b 1

rem ---------------------------------------------------------------------------------------------------------------------
:step
echo.
echo ==^> %~1
exit /b 0

rem :ensure <name> <winget id> <test label> [winget args...]
rem winget exits non-zero when the package is already installed, so success is judged by the test afterwards.
:ensure
call %~3 && goto :ensure_found
echo   Installing %~1 (%~2)...
set "WINGET_EXTRA="
if not "%~4"=="" set WINGET_EXTRA=%4 %5
winget install --id %~2 -e --accept-source-agreements --accept-package-agreements %WINGET_EXTRA%
call %~3 && exit /b 0
echo %~1 could not be installed. Install it manually and run this script again.
exit /b 1
:ensure_found
echo   %~1 found
exit /b 0

:find_rustup
where rustup >nul 2>&1
exit /b

:find_llvm
for %%d in ("%LIBCLANG_PATH%" "%ProgramFiles%\LLVM\bin" "%ProgramFiles(x86)%\LLVM\bin") do (
    if exist "%%~d\libclang.dll" (set "LIBCLANG_PATH=%%~d"& exit /b 0)
)
exit /b 1

:find_vctools
set "VSWHERE=%ProgramFiles(x86)%\Microsoft Visual Studio\Installer\vswhere.exe"
if not exist "%VSWHERE%" exit /b 1
set "VCPATH="
for /f "usebackq delims=" %%i in (`"%VSWHERE%" -products * -latest -requires Microsoft.VisualStudio.Component.VC.Tools.x86.x64 -property installationPath`) do set "VCPATH=%%i"
if defined VCPATH exit /b 0
exit /b 1

:find_iscc
for %%d in ("%LOCALAPPDATA%\Programs" "%ProgramFiles(x86)%" "%ProgramFiles%") do (
    for /d %%i in ("%%~d\Inno Setup*") do if exist "%%i\ISCC.exe" (set "ISCC=%%i\ISCC.exe"& exit /b 0)
)
exit /b 1
