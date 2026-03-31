#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
DEFAULT_OUTPUT="$ROOT_DIR/target/render/scene.png"

OUTPUT_PATH="${1:-$DEFAULT_OUTPUT}"
if [[ $# -gt 0 ]]; then
  shift
fi

mkdir -p "$(dirname "$OUTPUT_PATH")"

cargo run -- --render-scene --output "$OUTPUT_PATH" "$@"

# if command -v xdg-open >/dev/null 2>&1; then
#   xdg-open "$OUTPUT_PATH" >/dev/null 2>&1 || true
# fi

printf 'Rendered scene: %s\n' "$OUTPUT_PATH"
