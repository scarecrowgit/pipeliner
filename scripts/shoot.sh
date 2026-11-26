#!/usr/bin/env bash
# Screenshots of the real app, headless: a private wlroots compositor (cage, no screen),
# software Vulkan (lavapipe) and grim. Nothing appears on your desktop and your own data
# is never touched: the app runs on a generated fixture (or a data dir you pass) with
# PIPELINER_STILL, so the engine doesn't start.
#
#   scripts/shoot.sh -o /tmp/shots                      # fixture + every screen
#   scripts/shoot.sh -o /tmp/shots review:@review_pr    # one screen
#   scripts/shoot.sh -o /tmp/shots -d <dir with data/ and config/> board
#
# Routes: home review chat board timeline pipelines agents memory policies settings,
# `name:<id>` to open an item, `name:@key` to take the id from <dir>/routes.json (written
# by pipeliner-fixture: review_pick review_pr review_plan chat task_running task_failed
# pipeline agent).
#
# Output: <out>/<route>.png, plus <out>/compare/<route>.png — the app next to the mockup
# render (design/renders/<screen>.png) when there is one.
#
# Tool rows in task threads opened: PIPELINER_EXPAND_TOOLS=1 scripts/shoot.sh ... task:@task_running
# Images in the chat composer: PIPELINER_DRAFT_IMAGES=a.png:b.png scripts/shoot.sh ... chat:@chat
#
# Options: -b <dir with the pipeliner and pipeliner-fixture binaries> (default: build
# debug ones with cargo), -s WxH (1440x900), -w seconds to wait before the shot (8).
set -euo pipefail

here=$(cd "$(dirname "$0")/.." && pwd)
out="" dir="" bin="" size="1440x900" wait_s=8
while getopts "o:d:b:s:w:h" o; do
  case $o in
    o) out=$OPTARG ;; d) dir=$OPTARG ;; b) bin=$OPTARG ;; s) size=$OPTARG ;; w) wait_s=$OPTARG ;;
    *) sed -n '2,23p' "$0"; exit 0 ;;
  esac
done
shift $((OPTIND - 1))
[ -n "$out" ] || { echo "shoot.sh: -o <out dir> is required" >&2; exit 2; }

# The tools come from nix when they aren't installed.
if ! command -v cage >/dev/null || ! command -v grim >/dev/null || ! command -v wlr-randr >/dev/null; then
  exec nix-shell -p cage grim wlr-randr imagemagick --run "$(printf '%q ' "$0" -o "$out" ${dir:+-d "$dir"} ${bin:+-b "$bin"} -s "$size" -w "$wait_s" "$@")"
fi

mkdir -p "$out"
out=$(cd "$out" && pwd)

if [ -z "$bin" ]; then
  (cd "$here" && nix-shell shell.nix --run 'cargo build -q -p pipeliner -p harness-engine --bin pipeliner --bin pipeliner-fixture')
  bin="${CARGO_TARGET_DIR:-$here/target}/debug"
fi
[ -x "$bin/pipeliner" ] || { echo "shoot.sh: no $bin/pipeliner" >&2; exit 2; }

if [ -z "$dir" ]; then
  dir="$out/fixture"
  rm -rf "$dir"
  "$bin/pipeliner-fixture" "$dir" --repo "$here" >/dev/null
fi
dir=$(cd "$dir" && pwd)

if [ $# -eq 0 ]; then
  set -- home review review:@review_pick review:@review_pr review:@review_plan chat:@chat \
    board timeline task:@task_running task:@task_failed pipelines:@pipeline agents:@agent \
    settings memory policies
fi

# Mockup render that shows the same screen.
mockup() {
  case $1 in
    task) echo task-thread ;; pipelines) echo pipeline-editor ;; *) echo "$1" ;;
  esac
}

# Wayland socket paths must fit in 108 bytes.
rt=$(mktemp -d /tmp/plshot.XXXX)
trap 'rm -rf "$rt"' EXIT

for route in "$@"; do
  name=${route%%:*}
  id=${route#*:}; [ "$id" = "$route" ] && id=""
  if [ "${id:0:1}" = "@" ]; then
    key=${id:1}
    id=$(python3 -c 'import json,sys; print(json.load(open(sys.argv[1])).get(sys.argv[2],""))' "$dir/routes.json" "$key")
    [ -n "$id" ] || { echo "shoot.sh: no '$key' in $dir/routes.json" >&2; continue; }
  fi
  slug=$(echo "$route" | sed 's/[:@\/]\+/-/g')
  png="$out/$slug.png"
  rm -f "$png"
  XDG_RUNTIME_DIR=$rt WLR_BACKENDS=headless WLR_RENDERER=pixman WLR_LIBINPUT_NO_DEVICES=1 \
  VK_DRIVER_FILES=${VK_DRIVER_FILES:-/run/opengl-driver/share/vulkan/icd.d/lvp_icd.x86_64.json} \
  PIPELINER_DATA_DIR=$dir/data PIPELINER_CONFIG_DIR=$dir/config PIPELINER_CACHE_DIR=$dir/cache \
  PIPELINER_STILL=1 PIPELINER_ROUTE="$name${id:+:$id}" \
  timeout -s KILL $((wait_s + 40)) env -u WAYLAND_DISPLAY -u DISPLAY cage -- sh -c "
    wlr-randr --output HEADLESS-1 --custom-mode $size >/dev/null 2>&1
    '$bin/pipeliner' >'$out/$slug.log' 2>&1 &
    app=\$!
    sleep $wait_s
    grim '$png'
    kill \$app
  " >/dev/null 2>&1 || true
  if [ -s "$png" ]; then
    echo "$png"
    m="$here/design/renders/$(mockup "$name").png"
    if [ -f "$m" ] && command -v magick >/dev/null; then
      mkdir -p "$out/compare"
      magick "$png" \( "$m" -resize "${size%x*}x" -crop "${size}+0+0" +repage \) -background '#202020' -splice 0x0 +append \
        "$out/compare/$slug.png" 2>/dev/null || true
    fi
  else
    echo "shoot.sh: no screenshot for $route (log: $out/$slug.log)" >&2
  fi
done
