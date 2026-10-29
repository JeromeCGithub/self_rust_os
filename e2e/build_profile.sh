#!/usr/bin/env bash
# The desktop disk must actually contain the optimized kernel, not a debug ELF.
set -euo pipefail
cd "$(dirname "$0")/.."
fixture_dir=$(mktemp -d /tmp/rustos-profile-XXXXXX)
trap 'rm -rf "${fixture_dir}"' EXIT
cargo build --release --features grub
./tools/build-grub-disk.sh "${fixture_dir}/desktop.img"
mcopy -i "${fixture_dir}/desktop.img@@1048576" ::/kernel.elf "${fixture_dir}/kernel.elf"
cmp target/x86_64-self-rust-os/release/self_rust_os "${fixture_dir}/kernel.elf"
echo 'PASS: desktop image contains the release kernel'
