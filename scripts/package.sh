#!/usr/bin/env bash
# Packages a built ukoo-node with its pinned llama.cpp engine for one platform.
# usage: package.sh <platform> <version> <path-to-ukoo-node-binary> <out-dir>
set -euo pipefail

PLATFORM=$1 VERSION=$2 BINARY=$3 OUT=$4
BUILD=b11090
BASE=https://github.com/ggml-org/llama.cpp/releases/download/$BUILD

# asset:sha256 pairs; CUDA builds also ship NVIDIA's runtime.
case "$PLATFORM" in
  macos-aarch64-metal) ASSETS="llama-$BUILD-bin-macos-arm64.tar.gz:bca9a0ce332c84c380a23337aa155d283e5fcaf2003dfbb1dd2c5773ac2a5628" ;;
  linux-x86_64-cpu)    ASSETS="llama-$BUILD-bin-ubuntu-x64.tar.gz:12d471909cbd1de581a69203d65614193bbbc4906804b8ae06b5ea6534c92dc4" ;;
  linux-x86_64-cuda)   ASSETS="llama-$BUILD-bin-ubuntu-cuda-12.8-x64.tar.gz:45e791d9bb8ce7cf3b237c8833da9620fa05a90c4b2f8cf81e7131d928f99551 cudart-llama-$BUILD-bin-ubuntu-cuda-12.8-x64.tar.gz:32cbf4fd8d88d2fbea360889600752ec573ed14ea7a624f7463c55342913f46d" ;;
  windows-x86_64-cpu)  ASSETS="llama-$BUILD-bin-win-cpu-x64.zip:dcfe8f2c33d9d7d0a895f68e2eab28d7c7fed8efd44c621f87c298a60cfb5d9c" ;;
  windows-x86_64-cuda) ASSETS="llama-$BUILD-bin-win-cuda-12.4-x64.zip:a15680a67c864c58cfc13a0962f034a7aee633d97381cb8293ea9b0668cd1da5 cudart-llama-bin-win-cuda-12.4-x64.zip:8c79a9b226de4b3cacfd1f83d24f962d0773be79f1e7b75c6af4ded7e32ae1d6" ;;
  *) echo "unknown platform $PLATFORM" >&2; exit 1 ;;
esac

WORK=$(mktemp -d)
trap 'rm -rf "$WORK"' EXIT
STAGE="$WORK/stage"
mkdir -p "$STAGE/engine" "$OUT"

for pair in $ASSETS; do
  asset=${pair%%:*} sha=${pair##*:}
  curl -fsSL -o "$WORK/$asset" "$BASE/$asset"
  echo "$sha  $WORK/$asset" | shasum -a 256 -c - >/dev/null
  mkdir -p "$WORK/x"
  case "$asset" in
    *.zip) unzip -q -o "$WORK/$asset" -d "$WORK/x" ;;
    *) tar -xzf "$WORK/$asset" -C "$WORK/x" ;;
  esac
done
# Linux and macOS archives wrap the files in llama-<build>/; Windows zips don't.
if [ -d "$WORK/x/llama-$BUILD" ]; then cp -R "$WORK/x/llama-$BUILD/." "$STAGE/engine/"; rm -rf "$WORK/x/llama-$BUILD"; fi
cp -R "$WORK/x/." "$STAGE/engine/"

cp "$BINARY" "$STAGE/"
NAME="ukoo-node-$VERSION-$PLATFORM"
case "$PLATFORM" in
  windows-*) (cd "$STAGE" && 7z a -tzip -bso0 "$OUT/$NAME.zip" .) ;;
  *) tar -czf "$OUT/$NAME.tar.gz" -C "$STAGE" . ;;
esac
echo "$OUT/$NAME.$( [[ $PLATFORM == windows-* ]] && echo zip || echo tar.gz)"
