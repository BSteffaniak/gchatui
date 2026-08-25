#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$root"

files=()
while IFS= read -r file; do
  files+=("$file")
done < <(git ls-files --cached --others --exclude-standard)

if ((${#files[@]} == 0)); then
  echo "public-safety: no files to inspect"
  exit 0
fi

status=0

if rg --line-number --ignore-case \
  --glob '!scripts/check-public-safety.sh' \
  --glob '!src/**/*test*' \
  --glob '!tests/**' \
  '(refresh[_ -]?token|access[_ -]?token|client[_ -]?secret|authorization)["'\'']?[[:space:]]*:[[:space:]]*["'\''][A-Za-z0-9_./+==-]{8,}' \
  "${files[@]}"; then
  echo "public-safety: possible committed credential material" >&2
  status=1
fi

while IFS= read -r match; do
  address="${match##*:}"
  case "${address,,}" in
    *@example.com|*@example.org|*@example.net) ;;
    *)
      echo "public-safety: non-example email address: $match" >&2
      status=1
      ;;
  esac
done < <(rg --line-number --only-matching \
  '[A-Za-z0-9.!#$%&*+/=?^_`{|}~-]+@[A-Za-z0-9.-]+\.[A-Za-z]{2,}' \
  "${files[@]}" || true)

if [[ -f .private-denylist ]]; then
  while IFS= read -r term; do
    [[ -z "$term" || "$term" == \#* ]] && continue
    if rg --line-number --fixed-strings --ignore-case -- "$term" "${files[@]}"; then
      echo "public-safety: local denylist match detected" >&2
      status=1
    fi
  done < .private-denylist
fi

if ((status != 0)); then
  exit "$status"
fi

echo "public-safety: PASS"
