#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$root"

status=0
rust_files=()
while IFS= read -r file; do
  rust_files+=("$file")
done < <(find packages/gchatui/src -type f -name '*.rs' ! -name 'keybind.rs' ! -path '*/keybind/*' -print)

if ((${#rust_files[@]} > 0)); then
  if rg --line-number \
    'KeyCode::|KeyEvent|modifiers[[:space:]]*==|KeyStroke[[:space:]]*\{' \
    "${rust_files[@]}"; then
    echo "architecture: key chords must be owned by the keybinding registry" >&2
    status=1
  fi
fi

if find packages \( -name target -o -name node_modules -o -name .wrangler \) -prune -o -type d \( -name common -o -name shared -o -name core \) -print | grep -q .; then
  echo "architecture: vague common/shared/core ownership directory found" >&2
  status=1
fi

if ((status != 0)); then
  exit "$status"
fi

echo "architecture: PASS"
