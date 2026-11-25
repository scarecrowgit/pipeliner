#!/usr/bin/env bash
# Render one static HTML design file to PNG with headless chromium.
# Usage: design/render.sh <in.html> <out.png> [W] [H]
# W defaults to 1440; H defaults to the height declared in the file's
# <meta name="design:size" content="WxH"> (falls back to 1200).
set -euo pipefail

if [ $# -lt 2 ]; then
  echo "usage: $0 <in.html> <out.png> [W] [H]" >&2
  exit 2
fi

IN=$(readlink -f "$1")
OUT=$(readlink -m "$2")
W=${3:-}
H=${4:-}

if [ -z "$W" ] || [ -z "$H" ]; then
  SIZE=$(grep -o '<meta name="design:size" content="[0-9]*x[0-9]*"' "$IN" | head -1 | grep -o '[0-9]*x[0-9]*' || true)
  W=${W:-${SIZE%x*}}
  H=${H:-${SIZE#*x}}
  W=${W:-1440}
  H=${H:-1200}
fi

mkdir -p "$(dirname "$OUT")"

FLAGS="--headless=new --disable-gpu --no-sandbox --hide-scrollbars --force-device-scale-factor=1 --window-size=$W,$H --virtual-time-budget=5000"

BROWSER=""
for c in chromium chromium-browser google-chrome google-chrome-stable; do
  if command -v "$c" >/dev/null 2>&1; then BROWSER=$c; break; fi
done

if [ -n "$BROWSER" ]; then
  env -u LD_LIBRARY_PATH "$BROWSER" $FLAGS --screenshot="$OUT" "file://$IN" >/dev/null 2>&1
else
  nix-shell -p chromium --run "env -u LD_LIBRARY_PATH chromium $FLAGS --screenshot='$OUT' 'file://$IN'" >/dev/null 2>&1
fi

[ -s "$OUT" ] || { echo "render failed: $OUT not produced" >&2; exit 1; }
echo "$OUT"
