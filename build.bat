@echo off
rem Ручная сборка релиза astro-stacker: портативная папка и zip рядом с ней.
rem Всё готовое кладётся в подпапку build.
rem
rem Отдельный шаг сборки нужен из-за плагинов: бинарник сам по себе не читает
rem ни одного формата кадра, форматы приходят подключаемыми библиотеками, и
rem собранная папка должна лежать так, как их ищет хост. Проверка в конце
rem запускает собранный бинарник и убеждается, что он свой плагин видит.
rem
rem В папку кладутся обе программы: командная строка и окно. Плагины у них
rem общие - и то, и другое ищет их одинаково, сначала в подпапке plugins рядом
rem с собой, потом рядом с собой, - поэтому одна папка plugins обслуживает оба.
rem
rem   build.bat         полная сборка: тесты, clippy, релиз, архив
rem   build.bat quick   только релиз и архив, без проверок
setlocal enabledelayedexpansion
cd /d "%~dp0"

rem cargo нужен в PATH: если Rust ставился через rustup, он тут.
set "PATH=%USERPROFILE%\.cargo\bin;%PATH%"
where cargo >nul 2>&1
if not "%ERRORLEVEL%"=="0" (
  echo [ОШИБКА] cargo не найден в PATH. Установи Rust: https://rustup.rs
  exit /b 1
)

rem Окно собирается через Tauri, а тот запускается из npm: фронтенд собирается
rem до бинарника и попадает в него.
where npm >nul 2>&1
if not "%ERRORLEVEL%"=="0" (
  echo [ОШИБКА] npm не найден в PATH. Он нужен для сборки окна.
  echo          Установи Node.js: https://nodejs.org
  exit /b 1
)

rem Версия берётся из корневого Cargo.toml - единственный источник правды,
rem все крейты наследуют её через workspace.package.
for /f "tokens=2 delims==" %%v in ('findstr /r /c:"^version = " Cargo.toml') do (
  if not defined VER set "VER=%%v"
)
set "VER=%VER: =%"
set "VER=%VER:"=%"
if "%VER%"=="" (
  echo [ОШИБКА] не удалось прочитать версию из Cargo.toml
  exit /b 1
)

set "OUT=build"
set "NAME=astro-stacker_%VER%_x64"
set "STAGE=%OUT%\%NAME%"
set "ZIP=%OUT%\%NAME%.zip"

echo.
echo === Сборка astro-stacker %VER% ===
echo.

rem Старые сборки убираются по имени, а не сносом всей папки build: туда мог
rem положить что-то и человек. Убирается заранее, иначе в папке остались бы
rem файлы прошлой версии, а архив собрался бы поверх них и увёз бы их с собой.
if not exist "%OUT%" mkdir "%OUT%"
for /d %%d in ("%OUT%\astro-stacker_*_x64") do rd /s /q "%%~d"
del /f /q "%OUT%\astro-stacker_*_x64.zip" >nul 2>&1
if exist "%ZIP%" (
  echo [ОШИБКА] не удалось удалить %ZIP%
  echo          Скорее всего он открыт в другой программе.
  exit /b 1
)
if exist "%STAGE%" (
  echo [ОШИБКА] не удалось очистить %STAGE%
  exit /b 1
)

if /i "%~1"=="quick" goto :build

echo --- Тесты ---
cargo test
set "STEP_ERR=%ERRORLEVEL%"
if not "%STEP_ERR%"=="0" (
  echo.
  echo [ОШИБКА] тесты не прошли, код %STEP_ERR%
  exit /b %STEP_ERR%
)

echo.
echo --- Clippy ---
rem Предупреждения в собранном коде не допускаются, поэтому -D warnings:
rem без него clippy напечатает замечание и вернёт ноль.
cargo clippy --all-targets -- -D warnings
set "STEP_ERR=%ERRORLEVEL%"
if not "%STEP_ERR%"=="0" (
  echo.
  echo [ОШИБКА] clippy ругается, код %STEP_ERR%
  exit /b %STEP_ERR%
)

rem Окно - отдельный воркспейс со своим Cargo.lock, поэтому команда выше его
rem не видит и его замечания оставались непрочитанными.
echo.
echo --- Clippy окна ---
pushd apps\desktop\src-tauri
cargo clippy --all-targets -- -D warnings
set "STEP_ERR=%ERRORLEVEL%"
popd
if not "%STEP_ERR%"=="0" (
  echo.
  echo [ОШИБКА] clippy ругается на окно, код %STEP_ERR%
  exit /b %STEP_ERR%
)

:build
echo.
echo --- Релиз ---
cargo build --release
set "STEP_ERR=%ERRORLEVEL%"
if not "%STEP_ERR%"=="0" (
  echo.
  echo [ОШИБКА] сборка завершилась с кодом %STEP_ERR%
  exit /b %STEP_ERR%
)

echo.
echo --- Окно ---
rem Зависимости фронтенда ставятся только если их нет: npm install при
rem каждой сборке - это минуты на то, что уже лежит на диске.
if not exist "apps\desktop\node_modules" (
  echo   ставлю зависимости фронтенда
  pushd apps\desktop
  call npm install
  set "STEP_ERR=!ERRORLEVEL!"
  popd
  if not "!STEP_ERR!"=="0" (
    echo [ОШИБКА] npm install не прошёл, код !STEP_ERR!
    exit /b !STEP_ERR!
  )
)

rem --no-bundle: в папку нужен переносимый exe, а не установщик. Установщик
rem собирается тем же tauri build без этого ключа, но это другая поставка.
rem call обязателен: npm это .cmd, и без него batch сюда уже не вернётся.
pushd apps\desktop
call npm run tauri -- build --no-bundle
set "STEP_ERR=!ERRORLEVEL!"
popd
if not "!STEP_ERR!"=="0" (
  echo.
  echo [ОШИБКА] сборка окна завершилась с кодом !STEP_ERR!
  exit /b !STEP_ERR!
)

set "REL=target\release"
set "EXE=%REL%\astro-stacker.exe"
set "PLUGIN=%REL%\astro_format_canon.dll"
rem У окна свой воркспейс, поэтому и target свой.
set "GUI=apps\desktop\src-tauri\target\release\astro-stacker-desktop.exe"

for %%f in ("%EXE%" "%PLUGIN%" "%GUI%") do (
  if not exist "%%~f" (
    echo [ОШИБКА] не собралось: %%~f
    exit /b 1
  )
)

rem Хост ищет плагины в подпапке plugins рядом с бинарником, потом рядом с ним
rem самим. Кладём в plugins: так видно, что это отдельные библиотеки, а не части
rem программы.
mkdir "%STAGE%\plugins"
copy /y "%EXE%" "%STAGE%\" >nul
if not "%ERRORLEVEL%"=="0" (
  echo [ОШИБКА] не удалось скопировать astro-stacker.exe
  exit /b 1
)
copy /y "%PLUGIN%" "%STAGE%\plugins\" >nul
if not "%ERRORLEVEL%"=="0" (
  echo [ОШИБКА] не удалось скопировать плагин
  exit /b 1
)
copy /y "%GUI%" "%STAGE%\" >nul
if not "%ERRORLEVEL%"=="0" (
  echo [ОШИБКА] не удалось скопировать окно
  exit /b 1
)
if exist "README.md" copy /y "README.md" "%STAGE%\" >nul

rem Настоящая проверка, а не формальность: бинарник без своего плагина
rem запускается и работает - просто молча не читает ни одного формата кадра.
rem Такую сборку можно отдать и узнать о поломке только от того, кто её открыл.
rem
rem Проверяется командная строка, но отвечает она за обе программы: плагины
rem ищет общий код ядра, от каталога рядом с exe, а обе программы лежат в
rem одном каталоге. Чего эта проверка не покрывает - окно само: оно оконное,
rem из скрипта его не спросить, и ему вдобавок нужен WebView2 на машине, где
rem его запустят.
echo.
echo --- Проверка ---
"%STAGE%\astro-stacker.exe" plugins | findstr /i "canon" >nul
if not "%ERRORLEVEL%"=="0" (
  echo [ОШИБКА] собранный astro-stacker не видит плагин Canon.
  echo          Значит, папка plugins лежит не там, где её ищет хост.
  exit /b 1
)
echo   плагин Canon загружается

rem tar есть в Windows 10 и новее и умеет zip. PowerShell - запасной путь.
rem -C нужен, чтобы внутри архива лежала сама папка, а не build\папка.
where tar >nul 2>&1
if "%ERRORLEVEL%"=="0" (
  tar -a -c -f "%ZIP%" -C "%OUT%" "%NAME%"
) else (
  powershell -NoProfile -Command "Compress-Archive -Path '%STAGE%' -DestinationPath '%ZIP%' -Force"
)
if not exist "%ZIP%" (
  echo [ОШИБКА] архив не создался
  exit /b 1
)

echo.
echo === Готово ===
for %%f in ("%ZIP%") do (
  set /a KB=%%~zf/1024
  echo   %%~f  ^(!KB! КБ, %%~tf^)
)
echo   %STAGE%\  - распакованная папка, запускается прямо из неё
echo.
echo   astro-stacker.exe --help          список команд
echo   astro-stacker.exe plugins         какие форматы читаются
echo   astro-stacker-desktop.exe         окно
echo.
endlocal
