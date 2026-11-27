#!/usr/bin/env bash
# Packages a release build into dist/<name>.zip:
#   beast-studio, beast-studio-cli, ph-plugin-scan (the plugin scan helper), assets/, THIRD_PARTY_NOTICES.md, LICENSE-*, README.txt, a launcher.
# Usage: packaging/package.sh <cargo target dir with release binaries> <name> [gpl]
# Run from the repository root. Used by .github/workflows/nightly.yml and release.yml.
set -euo pipefail
bin_dir="$1"
name="$2"
flavour="${3:-}"
exe=""
case "$(uname -s)" in
  MINGW*|MSYS*|CYGWIN*|Windows_NT) exe=".exe" ;;
esac
stage="dist/$name"
rm -rf "$stage" "dist/$name.zip"
mkdir -p "$stage"
cp "$bin_dir/beast-studio$exe" "$bin_dir/beast-studio-cli$exe" "$bin_dir/ph-plugin-scan$exe" "$stage/"
cp -r assets "$stage/assets"
cp THIRD_PARTY_NOTICES.md LICENSE-MIT LICENSE-APACHE "$stage/"
cp packaging/README.txt "$stage/README.txt"
cp -r packaging/icons "$stage/icons"
if [ "$flavour" = "gpl" ]; then
  cp packaging/GPL-NOTICE.txt "$stage/GPL-NOTICE.txt"
fi
if [ -n "$exe" ]; then
  cp "packaging/Beast Studio.bat" "$stage/"
else
  cp packaging/beast-studio.sh "$stage/"
  chmod +x "$stage/beast-studio.sh" "$stage/beast-studio" "$stage/beast-studio-cli" "$stage/ph-plugin-scan"
fi
git rev-parse HEAD > "$stage/COMMIT.txt"
cd dist
if command -v zip >/dev/null 2>&1; then
  zip -qr "$name.zip" "$name"
elif command -v 7z >/dev/null 2>&1; then
  7z a -tzip -bd "$name.zip" "$name" >/dev/null
else
  powershell -NoProfile -Command "Compress-Archive -Path '$name' -DestinationPath '$name.zip'"
fi
echo "dist/$name.zip"
