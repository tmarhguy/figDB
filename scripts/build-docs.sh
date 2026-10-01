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

asciidoctor \
  -D "$OUT" \
  -o index.html \
  "$SRC"

# Theme assets live beside the generated HTML (paths in docinfo.html
# assume <output>/theme/...). docinfo.html itself is injected at build
# time and is not needed at runtime.
cp "$ROOT/docs/theme/docs.css" "$OUT/theme/docs.css"
cp "$ROOT/docs/theme/nav.js" "$OUT/theme/nav.js"

# Figures referenced via :imagesdir: images.
if compgen -G "$ROOT/docs/images/*" >/dev/null; then
  cp "$ROOT/docs/images/"* "$OUT/images/"
fi

if [[ ! -s "$OUT/index.html" ]]; then
  echo "ERROR: build produced no output at $OUT/index.html" >&2
  exit 1
fi

echo "Docs built: $OUT/index.html"
