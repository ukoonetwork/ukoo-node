#!/usr/bin/env bash
# Signs and publishes a ukoo-node release on the release-key holder's machine.
#   scripts/sign-release.sh node-v0.2.0 ~/ukoo-release.key
# Needs the GitHub CLI (gh), logged in with access to the repo.
set -euo pipefail
TAG=$1 KEY=$2
REPO=$(gh repo view --json nameWithOwner -q .nameWithOwner)
VERSION=${TAG#node-v}
ROOT=$(cd "$(dirname "$0")/.." && pwd)
WORK=$(mktemp -d)
trap 'rm -rf "$WORK"' EXIT

(cd "$ROOT/release-tool" && cargo build --release -q)
TOOL="$ROOT/release-tool/target/release/release-tool"

gh release download "$TAG" --repo "$REPO" --dir "$WORK" --pattern 'ukoo-node-*'
"$TOOL" manifest "$VERSION" b11090 "https://github.com/$REPO/releases/download/$TAG" "$WORK"
"$TOOL" sign "$KEY" "$WORK/manifest.json"
PUBLIC=$(gh variable get UKOO_RELEASE_PUBLIC_KEY --repo "$REPO")
"$TOOL" verify "$PUBLIC" "$WORK/manifest.json" "$WORK/manifest.json.sig"

cat "$WORK/manifest.json"
read -r -p "Publish $TAG to every operator? Type 'publish': " answer
[ "$answer" = publish ] || { echo "Not published."; exit 1; }
gh release upload "$TAG" "$WORK/manifest.json" "$WORK/manifest.json.sig" "$WORK/manifest.json.sig.bin" --repo "$REPO" --clobber
gh release edit "$TAG" --repo "$REPO" --draft=false --latest --notes "ukoo-node $VERSION, signed."
echo "Published $TAG."
