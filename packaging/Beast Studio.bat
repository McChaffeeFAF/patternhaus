@echo off
rem Beast Studio launcher: starts the app with the factory assets next to it.
rem   "Beast Studio.bat" [project_dir] [--buffer N] [--silent] [--demo-agent]
rem Without a project folder it opens (or creates) "Beast Studio Projects\first" in Documents;
rem an existing "Beast Studio Projects\first" from an earlier version is still opened.
setlocal
set "HERE=%~dp0"

if "%~1"=="" (
  set "FIRST=%USERPROFILE%\Documents\Beast Studio Projects\first"
  if not exist "%USERPROFILE%\Documents\Beast Studio Projects\first" if exist "%USERPROFILE%\Documents\Beast Studio Projects\first" set "FIRST=%USERPROFILE%\Documents\Beast Studio Projects\first"
  "%HERE%beast-studio.exe" --assets "%HERE%assets" "%FIRST%"
) else (
  "%HERE%beast-studio.exe" --assets "%HERE%assets" %*
)
