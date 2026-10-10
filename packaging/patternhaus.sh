#!/usr/bin/env sh
# Patternhaus launcher: starts the app with the factory assets next to it.
#   ./patternhaus.sh [project_dir] [--buffer N] [--silent] [--demo-agent]
# Without a project folder it opens (or creates) ~/Patternhaus Projects/first.
here="$(cd "$(dirname "$0")" && pwd)"

if [ $# -eq 0 ]; then
  exec "$here/ph-app" --assets "$here/assets" "$HOME/Patternhaus Projects/first"
else
  exec "$here/ph-app" --assets "$here/assets" "$@"
fi
