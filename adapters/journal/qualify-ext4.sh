#!/usr/bin/env bash
# Qualify against a new disposable filesystem on the GitHub-hosted Linux runner.
set -euo pipefail
if [[ $# != 4 || ${GITHUB_ACTIONS:-} != true || $(uname -s) != Linux || $(id -u) == 0 ]]; then
  echo 'Run on an ordinary-user Linux Actions job: qualify-ext4.sh RELEASE QUALIFICATION REFERENCE EVIDENCE_DIR' >&2
  exit 2
fi
release=$(realpath "$1")
qualification=$(realpath "$2")
reference=$(realpath "$3")
mkdir -p -- "$4"
evidence=$(realpath "$4")
script_dir=$(cd -- "$(dirname -- "$0")" && pwd)
for tool in mkfs.ext4 mount umount findmnt strace node; do
  command -v "$tool" > /dev/null
done
scratch=$(mktemp -d "${RUNNER_TEMP:?}/journal-ext4.XXXXXXXX")
image=$scratch/volume.img
volume=$scratch/mount
health=$scratch/health
mkdir -m 700 "$volume" "$health"
cleanup() {
  if mountpoint -q "$volume"; then
    sudo umount -- "$volume" || return 1
  fi
  rm -rf -- "$scratch"
}
trap cleanup EXIT
truncate -s 160M "$image"
mkfs.ext4 -q -F "$image"
sudo mount -t ext4 -o loop,data=ordered,barrier=1,nodev,nosuid "$image" "$volume"
sudo install -d -m 0700 -o "$(id -u)" -g "$(id -g)" "$volume/qualification"
{
  uname -a
  findmnt --target "$volume" -o TARGET,SOURCE,FSTYPE,OPTIONS,MAJ:MIN
  findmnt --target "$health" -o TARGET,SOURCE,FSTYPE,OPTIONS,MAJ:MIN
  sha256sum -- "$release" "$qualification" "$reference"
  git -C "$script_dir" rev-parse HEAD
} > "$evidence/platform.txt"
python3 -B "$script_dir/tests/qualify.py" --release "$release" --qualification "$qualification" \
  --reference "$reference" --root "$volume/qualification" --health-root "$health" \
  --evidence "$evidence" 2>&1 | tee "$evidence/process-tests.log"
python3 -B "$script_dir/tests/qual_executor_quiescence.py" --reference "$reference" \
  --root "$volume/qualification" --evidence "$evidence"
