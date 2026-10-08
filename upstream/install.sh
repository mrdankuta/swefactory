#!/usr/bin/env bash
# pi-factory installer: seed the factory into a new or existing project.
# Usage:
#   curl -fsSL https://raw.githubusercontent.com/mrdankuta/pi-factory/main/install.sh | bash -s -- [--stack elixir|node|python] [--dir /path/to/project] [--pin vX.Y.Z]
# Idempotent: never overwrites existing project files.
set -euo pipefail

STACK="elixir"
DEST="$PWD"
PIN="main"
GLOBAL_SKILLS=""

while [ $# -gt 0 ]; do
  case "$1" in
    --stack) STACK="${2:?}"; shift 2 ;;
    --dir) DEST="${2:?}"; shift 2 ;;
    --pin) PIN="${2:?}"; shift 2 ;;
    --global-skills) GLOBAL_SKILLS="--global-skills"; shift ;;
    *) echo "install.sh: unknown arg $1" >&2; exit 1 ;;
  esac
done

command -v git >/dev/null 2>&1 || { echo "install.sh: git required" >&2; exit 1; }

SRC="$(mktemp -d)/pi-factory"
trap 'rm -rf "$SRC"' EXIT
echo "fetching pi-factory@$PIN ..."
git clone --quiet --depth 1 --branch "$PIN" https://github.com/mrdankuta/pi-factory.git "$SRC" 2>/dev/null \
  || git clone --quiet --depth 1 https://github.com/mrdankuta/pi-factory.git "$SRC"

export FACTORY_SRC="$SRC"
mkdir -p "$DEST"
cd "$DEST"
# shellcheck disable=SC2086
bash "$SRC/bin/factory" init --stack "$STACK" $GLOBAL_SKILLS
