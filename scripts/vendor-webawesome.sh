#!/usr/bin/env bash
# Bundles the Web Awesome components a tool interface uses into ui/vendor/,
# so the interface works without a build step and without network access.
#
# Copy this script to scripts/ in the tool repository, list the components
# the interface uses in COMPONENTS and run it again after every change.
set -euo pipefail

WA_VERSION=3.14.0
ESBUILD_VERSION=0.28.2
COMPONENTS=(button callout card divider icon input option select switch tag)

root=$(cd "$(dirname "$0")/.." && pwd)
out="$root/ui/vendor"
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT

cd "$work"
npm init -y >/dev/null
npm install --silent --no-audit --no-fund \
  "@awesome.me/webawesome@$WA_VERSION" "esbuild@$ESBUILD_VERSION"

wa=node_modules/@awesome.me/webawesome/dist
{
  echo "import { registerTranslation, unregisterIconLibrary } from '@awesome.me/webawesome/dist/webawesome.js';"
  echo "import de from '@awesome.me/webawesome/dist/translations/de.js';"
  for c in "${COMPONENTS[@]}"; do
    echo "import '@awesome.me/webawesome/dist/components/$c/$c.js';"
  done
  echo "registerTranslation(de);"
  # The default icon library loads icons from a CDN. Only the built-in system
  # icons are used, so nothing is ever fetched from the network.
  echo "unregisterIconLibrary('default');"
} >entry.js
{
  echo "@import '$wa/styles/themes/default.css';"
  echo "@import '$wa/styles/utilities.css';"
} >entry.css

mkdir -p "$out"
npx esbuild entry.js --bundle --format=esm --minify --legal-comments=eof \
  --outfile="$out/webawesome.js"
npx esbuild entry.css --bundle --minify --legal-comments=eof \
  --outfile="$out/webawesome.css"
cp node_modules/@awesome.me/webawesome/LICENSE.md "$out/LICENSE-webawesome.md"
echo "Web Awesome $WA_VERSION: ${COMPONENTS[*]}" >"$out/VERSION"
ls -l "$out"
