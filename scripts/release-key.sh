#!/usr/bin/env bash
# Creates Ukoo's release key on this machine, once. Keep the file, and an offline backup.
#   scripts/release-key.sh ~/ukoo-release.key
set -euo pipefail
ROOT=$(cd "$(dirname "$0")/.." && pwd)
(cd "$ROOT/release-tool" && cargo build --release -q)
"$ROOT/release-tool/target/release/release-tool" keygen "${1:?usage: release-key.sh <private-key-file>}"
