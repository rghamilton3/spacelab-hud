#!/usr/bin/env bash
# Extracts the aarch64 sysroot from Slint's cross-compilation Docker image.
# Run once before cross-compiling; re-run to refresh after image updates.
set -euo pipefail

SYSROOT="$HOME/.local/share/pi-sysroot-aarch64"
IMAGE="ghcr.io/slint-ui/slint/aarch64-unknown-linux-gnu"

echo "Pulling $IMAGE..."
docker pull "$IMAGE"

CONTAINER=$(docker create "$IMAGE")
trap 'docker rm -f "$CONTAINER" 2>/dev/null || true' EXIT

echo "Extracting aarch64 libs and headers..."
mkdir -p "$SYSROOT/usr/lib"
docker cp "$CONTAINER:/usr/lib/aarch64-linux-gnu" "$SYSROOT/usr/lib/aarch64-linux-gnu"
docker cp "$CONTAINER:/usr/include" "$SYSROOT/usr/include"

echo "Done. Sysroot at $SYSROOT"
