@echo off
rem Patternhaus launcher: starts the app with the factory assets next to it.
rem   Patternhaus.bat [project_dir] [--buffer N] [--silent] [--demo-agent]
rem Without a project folder it opens (or creates) "Patternhaus Projects\first" in Documents.
setlocal
set "HERE=%~dp0"

if "%~1"=="" (
  "%HERE%ph-app.exe" --assets "%HERE%assets" "%USERPROFILE%\Documents\Patternhaus Projects\first"
) else (
  "%HERE%ph-app.exe" --assets "%HERE%assets" %*
)
