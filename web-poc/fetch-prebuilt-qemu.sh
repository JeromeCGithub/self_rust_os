#!/usr/bin/env bash
# Download prebuilt QEMU Wasm artifacts instead of building them.
#
# The binaries come from ktock/qemu-wasm-demo-images, the asset repo
# behind the public qemu-wasm demo page: out.js + the x86_64 emulator
# .wasm/.worker.js of the Alpine demo. That QEMU build targets an
# x86_64 guest with TCG, exports the Emscripten FS API used by the POC
# page, and wires stdio to xterm-pty — the same configuration
# ./build-qemu-wasm.sh produces from source.
#
# Pinned to a specific commit so the POC is reproducible. Note the demo
# build was configured for the Alpine demo; if it ever misbehaves with
# this guest, build from source with ./build-qemu-wasm.sh.
#
# Usage: ./fetch-prebuilt-qemu.sh

set -euo pipefail

DEMO_IMAGES_COMMIT="b7c549b5e6f4c376f76483a03e983214421434ad"
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
HTDOCS="${SCRIPT_DIR}/htdocs"
# Temp dir under this script's directory, NOT /tmp: /tmp is often a
# small tmpfs and the emulator .wasm alone is ~41 MB (a full checkout
# of the demo repo is ~150 MB and dies with a bus error there).
# Sparse-checkout is limited to the three files actually needed.
TMP="$(mktemp -d "${SCRIPT_DIR}/.fetch-tmp.XXXXXX")"
trap 'rm -rf "${TMP}"' EXIT

echo "[prebuilt] Fetching qemu-wasm demo images @ ${DEMO_IMAGES_COMMIT}..."
git clone --quiet --filter=blob:none --sparse \
    https://github.com/ktock/qemu-wasm-demo-images.git "${TMP}/images"
git -C "${TMP}/images" sparse-checkout set --no-cone \
    '/alpine-x86_64/out.js' \
    '/alpine-x86_64/qemu-system-x86_64.wasm' \
    '/alpine-x86_64/qemu-system-x86_64.worker.js'
git -C "${TMP}/images" checkout --quiet "${DEMO_IMAGES_COMMIT}"

mkdir -p "${HTDOCS}"
cp "${TMP}/images/alpine-x86_64/out.js" "${HTDOCS}/out.js"
cp "${TMP}/images/alpine-x86_64/qemu-system-x86_64.wasm" "${HTDOCS}/"
cp "${TMP}/images/alpine-x86_64/qemu-system-x86_64.worker.js" "${HTDOCS}/"

echo "[prebuilt] Staging BIOS ROMs into htdocs/pack-src/..."
QEMU_WASM_COMMIT="0ef7b4e2814b231705d8371dd7997f5b72e70baf"
mkdir -p "${HTDOCS}/pack-src"
for f in bios-256k.bin vgabios-stdvga.bin kvmvapic.bin linuxboot_dma.bin; do
    curl -fsSL -o "${HTDOCS}/pack-src/${f}" \
        "https://raw.githubusercontent.com/ktock/qemu-wasm/${QEMU_WASM_COMMIT}/pc-bios/${f}"
done

echo "[prebuilt] Done. Run ./package-image.sh, then ./serve.py."
