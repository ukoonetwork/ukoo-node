#!/usr/bin/env sh
# Downloads the pinned llama.cpp build into ./engine and verifies its SHA-256.
# Release packaging bundles the same files next to the ukoo-node binary.
set -eu

BUILD=b11090
case "$(uname -s)-$(uname -m)-${UKOO_ENGINE_FLAVOR:-cpu}" in
  Darwin-arm64-*)        ASSET=llama-$BUILD-bin-macos-arm64.tar.gz;          SHA=bca9a0ce332c84c380a23337aa155d283e5fcaf2003dfbb1dd2c5773ac2a5628 ;;
  Linux-x86_64-cuda)     ASSET=llama-$BUILD-bin-ubuntu-cuda-12.8-x64.tar.gz; SHA=45e791d9bb8ce7cf3b237c8833da9620fa05a90c4b2f8cf81e7131d928f99551 ;;
  Linux-x86_64-*)        ASSET=llama-$BUILD-bin-ubuntu-x64.tar.gz;           SHA=12d471909cbd1de581a69203d65614193bbbc4906804b8ae06b5ea6534c92dc4 ;;
  *) echo "No pinned engine for $(uname -s) $(uname -m) yet." >&2; exit 1 ;;
esac

cd "$(dirname "$0")/.."
TMP=$(mktemp -d)
trap 'rm -rf "$TMP"' EXIT

curl -fL --progress-bar -o "$TMP/$ASSET" "https://github.com/ggml-org/llama.cpp/releases/download/$BUILD/$ASSET"
echo "$SHA  $TMP/$ASSET" | shasum -a 256 -c -

rm -rf engine && mkdir engine
tar -xzf "$TMP/$ASSET" -C "$TMP"
mv "$TMP/llama-$BUILD"/* engine/
echo "Engine $BUILD installed in $(pwd)/engine"
