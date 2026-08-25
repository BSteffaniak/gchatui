#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$root"

cargo build --quiet
expect ./scripts/smoke-pty.exp ./target/debug/gchatui
