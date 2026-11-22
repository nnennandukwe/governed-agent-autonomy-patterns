#!/bin/bash
# Build an operator-owned, read-only image of the exact committed study.
set -euo pipefail
if [[ $# != 1 ]]; then
    echo 'Usage: bash make-live-image.sh NEW_DIRECTORY' >&2
    exit 2
fi
umask 077
source_dir=$(cd "$(dirname "$0")" && pwd -P)
revision=$(git -C "$source_dir" rev-parse HEAD)
test -z "$(git -C "$source_dir" status --porcelain)"
mkdir "$1"
output=$(cd "$1" && pwd -P)
mkdir "$output/mount"
staging=$(mktemp -d "$output/stage-XXXXXX")
git -C "$source_dir" archive "$revision" -- . | tar -x -C "$staging"
printf '%s\n' "$revision" > "$staging/REVISION"
hdiutil create -quiet -format UDRO -fs HFS+ -volname GAAPExperimentB -srcfolder "$staging" "$output/runner.dmg"
hdiutil attach -readonly -nobrowse -mountpoint "$output/mount" "$output/runner.dmg" >/dev/null
shasum -a 256 "$output/runner.dmg"
printf 'Study runner: %s/mount/live_study.py\n' "$output"
