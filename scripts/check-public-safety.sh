#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$root"

files=()
while IFS= read -r file; do
  [[ -f "$file" ]] && files+=("$file")
done < <(git ls-files --cached --others --exclude-standard)

if ((${#files[@]} == 0)); then
  echo "public-safety: no files to inspect"
  exit 0
fi

status=0

# Public native-client values are allowed only in their explicitly owned module.
python3 - <<'PY'
from pathlib import Path
import re
import subprocess
source = Path('packages/gchatui/src/official_oauth.rs')
if source.exists():
    values = re.findall(r'pub const (?:CLIENT_ID|DESKTOP_CLIENT_VALUE): &str =\s*"([^"]+)";', source.read_text())
    if len(values) != 2:
        raise SystemExit('public-safety: invalid official client metadata module')
    paths = subprocess.check_output(['git', 'ls-files', '--cached', '--others', '--exclude-standard', '-z']).split(b'\0')
    for raw in paths:
        if not raw:
            continue
        path = Path(raw.decode())
        if path == source or not path.is_file():
            continue
        data = path.read_bytes()
        if any(value.encode() in data for value in values):
            raise SystemExit('public-safety: official client metadata outside approved module')
PY

if rg --line-number --ignore-case \
  --glob '!scripts/check-public-safety.sh' \
  --glob '!packages/gchatui/src/**/*test*' \
  --glob '!tests/**' \
  '(refresh[_ -]?token|access[_ -]?token|client[_ -]?secret|authorization)["'\'']?[[:space:]]*:[[:space:]]*["'\''][A-Za-z0-9_./+==-]{8,}' \
  "${files[@]}"; then
  echo "public-safety: possible committed credential material" >&2
  status=1
fi

while IFS= read -r match; do
  address="${match##*:}"
  case "${address,,}" in
    bradensteffaniak@gmail.com)
      case "${match%%:*}" in
        packages/site/src/main.rs|scripts/check-public-safety.sh) ;;
        *) echo "public-safety: approved contact outside website source: $match" >&2; status=1 ;;
      esac
      ;;
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
