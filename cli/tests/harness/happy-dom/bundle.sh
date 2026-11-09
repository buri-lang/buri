#!/bin/sh
# Rebuilds `happy-dom.mjs` and `LICENSES.txt` from the versions `bun.lock` pins.
#
# The page tests import the bundle, so a test run needs no install and no
# network. Run this after moving the pin in `package.json`, with `--update` so
# `bun.lock` follows it, and commit all four files together.
set -eu
here=$(cd "$(dirname "$0")" && pwd)
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
cp "$here/package.json" "$here/entry.mjs" "$work/"
if [ "${1:-}" = "--update" ]; then
  (cd "$work" && bun install --ignore-scripts)
  cp "$work/bun.lock" "$here/bun.lock"
else
  cp "$here/bun.lock" "$work/"
  (cd "$work" && bun install --frozen-lockfile --ignore-scripts)
fi
version=$(sed -n 's/^ *"version": "\(.*\)",$/\1/p' "$work/node_modules/happy-dom/package.json")
(cd "$work" && bun build entry.mjs --target=node --format=esm --outfile=bundle.mjs)
{
  echo "// happy-dom $version and its dependencies, bundled by bundle.sh from bun.lock."
  echo "// Generated: edit package.json and run bundle.sh rather than this file."
  cat "$work/bundle.mjs"
} >"$here/happy-dom.mjs"
{
  for package in "$work"/node_modules/*/ "$work"/node_modules/@*/*/; do
    name=${package#"$work/node_modules/"}
    case "$name" in @types/* | undici-types/) continue ;; esac
    for license in "$package"LICENSE*; do
      [ -f "$license" ] || continue
      echo "==> ${name%/}"
      cat "$license"
      echo
    done
  done
} >"$here/LICENSES.txt"
