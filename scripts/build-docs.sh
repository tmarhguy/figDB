#!/usr/bin/env bash
# Build the static technical manual with Asciidoctor.
# No system dependencies are installed here; a clear error is printed instead.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
SRC="$ROOT/docs/index.adoc"
OUT="$ROOT/build/docs"

if ! command -v asciidoctor >/dev/null 2>&1; then
  cat >&2 <<'EOF'
ERROR: asciidoctor is not installed.

Install the minimum dependency and retry:

  macOS (Homebrew):  brew install asciidoctor
  macOS (gem):       gem install asciidoctor
  Debian/Ubuntu:     sudo apt-get install -y asciidoctor
  Fedora:            sudo dnf install -y asciidoctor
  Generic (Ruby):     gem install asciidoctor

Then run: make docs
EOF
  exit 1
fi

mkdir -p "$OUT/theme" "$OUT/images"

if [[ ! -f "$SRC" ]]; then
  echo "ERROR: missing source $SRC" >&2
  exit 1
fi

# Version stamp in the footer ("Version <sha>" plus the build timestamp).
# The sha is derived from git at build time and falls back to the build
# date outside a git checkout. Passed as attributes so docs/index.adoc
# stays free of hardcoded versions. (revdate is forwarded for converters
# that honor it; the HTML5 footer renders the build time.)
REVDATE="$(git -C "$ROOT" log -1 --format=%cs 2>/dev/null || date +%F)"
REVNUM="$(git -C "$ROOT" rev-parse --short HEAD 2>/dev/null || echo unversioned)"

# NOTE: no --safe-mode flag on purpose. Safe mode jails output inside the
# document directory, which is incompatible with the required out-of-tree
# build (generated HTML must not live under docs/). The inputs are this
# repo's own sources, so the default unsafe mode is appropriate here.
asciidoctor \
  --backend html5 \
  --attribute docinfo=shared \
  --attribute revdate="$REVDATE" \
  --attribute revnumber="$REVNUM" \
  -D "$OUT" \
  -o index.html \
  "$SRC"

# Theme assets live beside the generated HTML (paths in docinfo.html
# assume <output>/theme/...). docinfo.html itself is injected at build
# time and is not needed at runtime.
cp "$ROOT/docs/theme/docs.css" "$OUT/theme/docs.css"
cp "$ROOT/docs/theme/nav.js" "$OUT/theme/nav.js"
cp "$ROOT/docs/theme/search.js" "$OUT/theme/search.js"

# Figures referenced via :imagesdir: images.
if compgen -G "$ROOT/docs/images/*" >/dev/null; then
  cp "$ROOT/docs/images/"* "$OUT/images/"
fi

if [[ ! -s "$OUT/index.html" ]]; then
  echo "ERROR: build produced no output at $OUT/index.html" >&2
  exit 1
fi

echo "Docs built: $OUT/index.html"
