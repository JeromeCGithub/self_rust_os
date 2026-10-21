#!/usr/bin/env bash
# Build qemu-system-x86_64 as WebAssembly (ktock/qemu-wasm) and stage the
# artifacts into ./htdocs for the self_rust_os browser POC.
#
# This follows the upstream qemu-wasm README build recipe for an x86_64
# guest. Docker is required because the builder image carries the
# Emscripten toolchain and the prebuilt library dependencies.
#
# If Docker is not available, ./fetch-prebuilt-qemu.sh downloads the
# prebuilt emulator artifacts from the upstream demo instead.
#
# Usage: ./build-qemu-wasm.sh [path-to-qemu-wasm-checkout]
#        (clones the pinned commit into ./qemu-wasm if not given)

set -euo pipefail

QEMU_WASM_COMMIT="0ef7b4e2814b231705d8371dd7997f5b72e70baf"
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
HTDOCS="${SCRIPT_DIR}/htdocs"
SRC="${1:-${SCRIPT_DIR}/qemu-wasm}"

command -v docker >/dev/null || {
    echo "Error: docker is required for the from-source build." >&2
    echo "Without docker, use ./fetch-prebuilt-qemu.sh instead." >&2
    exit 1
}

if [ ! -d "${SRC}/.git" ]; then
    echo "[qemu-wasm] Cloning ktock/qemu-wasm @ ${QEMU_WASM_COMMIT}..."
    git clone https://github.com/ktock/qemu-wasm.git "${SRC}"
fi
git -C "${SRC}" checkout --quiet "${QEMU_WASM_COMMIT}"

echo "[qemu-wasm] Building the Emscripten builder image..."
docker build -t buildqemu - < "${SRC}/Dockerfile"

echo "[qemu-wasm] Starting the builder container..."
docker rm -f build-qemu-wasm >/dev/null 2>&1 || true
docker run --rm -d --name build-qemu-wasm -v "${SRC}:/qemu/:ro" buildqemu

echo "[qemu-wasm] Compiling qemu-system-x86_64 (this takes a while)..."
EXTRA_CFLAGS="-O3 -g -Wno-error=unused-command-line-argument -matomics -mbulk-memory -DNDEBUG -DG_DISABLE_ASSERT -D_GNU_SOURCE -sASYNCIFY=1 -pthread -sPROXY_TO_PTHREAD=1 -sFORCE_FILESYSTEM -sALLOW_TABLE_GROWTH -sTOTAL_MEMORY=2300MB -sWASM_BIGINT -sMALLOC=mimalloc --js-library=/build/node_modules/xterm-pty/emscripten-pty.js -sEXPORT_ES6=1 -sASYNCIFY_IMPORTS=ffi_call_js"
docker exec build-qemu-wasm emconfigure /qemu/configure --static \
    --target-list=x86_64-softmmu --cpu=wasm32 --cross-prefix= \
    --without-default-features --enable-system --with-coroutine=fiber \
    --enable-virtfs \
    --extra-cflags="${EXTRA_CFLAGS}" --extra-cxxflags="${EXTRA_CFLAGS}" \
    --extra-ldflags="-sEXPORTED_RUNTIME_METHODS=getTempRet0,setTempRet0,addFunction,removeFunction,TTY,FS"
docker exec build-qemu-wasm emmake make -j "$(nproc)" qemu-system-x86_64

echo "[qemu-wasm] Staging emulator artifacts into htdocs/..."
docker cp build-qemu-wasm:/build/qemu-system-x86_64 "${HTDOCS}/out.js"
docker cp build-qemu-wasm:/build/qemu-system-x86_64.wasm "${HTDOCS}/"
docker cp build-qemu-wasm:/build/qemu-system-x86_64.worker.js "${HTDOCS}/"
docker stop build-qemu-wasm >/dev/null

echo "[qemu-wasm] Staging BIOS ROMs into htdocs/pack-src/..."
mkdir -p "${HTDOCS}/pack-src"
cp "${SRC}/pc-bios/bios-256k.bin" \
   "${SRC}/pc-bios/vgabios-stdvga.bin" \
   "${SRC}/pc-bios/kvmvapic.bin" \
   "${SRC}/pc-bios/linuxboot_dma.bin" \
   "${HTDOCS}/pack-src/"

echo "[qemu-wasm] Done. Run ./package-image.sh, then ./serve.py."
