#!/usr/bin/env sh
# Beast Studio launcher: starts the app with the factory assets next to it.
#   ./beast-studio.sh [project_dir] [--buffer N] [--silent] [--demo-agent]
# Without a project folder it opens (or creates) ~/Beast Studio Projects/first; an existing
# ~/Beast Studio Projects/first from an earlier version is still opened.
here="$(cd "$(dirname "$0")" && pwd)"

if [ $# -eq 0 ]; then
  first="$HOME/Beast Studio Projects/first"
  if [ ! -e "$first" ] && [ -e "$HOME/Beast Studio Projects/first" ]; then
    first="$HOME/Beast Studio Projects/first"
  fi
  exec "$here/beast-studio" --assets "$here/assets" "$first"
else
  exec "$here/beast-studio" --assets "$here/assets" "$@"
fi
