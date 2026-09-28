@echo off
chcp 65001 >nul
setlocal EnableExtensions EnableDelayedExpansion
title PowerBench

REM ===========================================================================
REM  PowerBench - запуск одним двойным щелчком.
REM  Проверяет инструменты, при необходимости ставит зависимости и запускает
REM  приложение. Параметры можно поменять в настройках самого приложения.
REM ===========================================================================

set "ROOT=%~dp0"
if "%ROOT:~-1%"=="\" set "ROOT=%ROOT:~0,-1%"
set "APP=%ROOT%\app"

REM Режим запуска: dev (по умолчанию) или release.
set "MODE=dev"
if /I "%~1"=="release" set "MODE=release"
if /I "%~1"=="--release" set "MODE=release"

echo.
echo   PowerBench
echo   ---------
echo   Корень проекта: %ROOT%
echo   Режим:          %MODE%
echo.

REM --- 1. Проверка Rust toolchain ------------------------------------------------
where cargo >nul 2>nul
if errorlevel 1 (
    echo [ошибка] не найден cargo - Rust.
    echo         Установите Rust: https://rustup.rs/
    echo         После установки перезапустите PowerBench.
    echo.
    pause
    exit /b 1
)

REM --- 2. Проверка Node.js / npm ----------------------------------------------
where node >nul 2>nul
if errorlevel 1 (
    echo [ошибка] не найден node - Node.js.
    echo         Установите Node.js LTS: https://nodejs.org/
    echo         После установки перезапустите PowerBench.
    echo.
    pause
    exit /b 1
)

for /f "tokens=*" %%v in ('node --version 2^>nul') do set "NODE_VER=%%v"
for /f "tokens=*" %%v in ('cargo --version 2^>nul') do set "RUST_VER=%%v"
echo   Node.js:  !NODE_VER!
echo   Rust:     !RUST_VER!
echo.

REM --- 3. Установка зависимостей при первом запуске ----------------------------
if not exist "%APP%\node_modules\" (
    echo   Первый запуск: устанавливаю зависимости ^(это один раз, займёт минуту^)...
    pushd "%APP%"
    call npm install
    if errorlevel 1 (
        popd
        echo.
        echo [ошибка] не удалось установить зависимости. Проверьте подключение к сети.
        echo.
        pause
        exit /b 1
    )
    popd
    echo   Готово.
    echo.
)

pushd "%APP%"

if /I "%MODE%"=="release" (
    REM --- 4a. Релизная сборка: готовый .exe без режима разработчика ------------
    echo   Собираю релизную версию ^(первая сборка занимает несколько минут^)...
    echo.
    call npm run tauri build
    set "EXITCODE=%ERRORLEVEL%"
    if not "!EXITCODE!"=="0" (
        echo.
        echo [ошибка] сборка не удалась, код !EXITCODE!.
    ) else (
        echo.
        echo Готово. Установщик лежит в app\src-tauri\target\release\bundle\
        echo и в ..\..\target\release\bundle\ ^(в зависимости от версии Tauri^).
    )
    goto :done
)

REM --- 4b. Режим разработки ----------------------------------------------------
echo   Запускаю PowerBench...
echo   Окно закроется вместе с этим окном консоли.
echo.
call npm run tauri dev
set "EXITCODE=%ERRORLEVEL%"

:done
popd

echo.
if not "%EXITCODE%"=="0" (
    echo [ошибка] завершилось с кодом %EXITCODE%.
) else (
    echo Завершено без ошибок.
)
echo.
pause
exit /b %EXITCODE%
