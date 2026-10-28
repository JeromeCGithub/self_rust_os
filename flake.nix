{
  description = "Development environment for self_rust_os.";

  # Pinned versions (update via `nix flake update`):
  # - nixpkgs: nixos-unstable (pinned in flake.lock)
  #   - QEMU: must be 11.1.1+ for 720p VBE support
  #   - Check with: nix develop -c qemu-system-x86_64 --version
  # - Rust: nightly-2025-01-30 (pinned in rust-toolchain.toml)
  # - cargo-bootimage: from nixpkgs
  #
  # To update QEMU to a specific version:
  #   1. Find the nixpkgs commit with the desired QEMU version
  #   2. Run: nix flake update --override-input nixpkgs github:NixOS/nixpkgs/<commit>

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
    flake-utils.url = "github:numtide/flake-utils";
    rust-overlay.url = "github:oxalica/rust-overlay";
  };

  outputs = { self, nixpkgs, flake-utils, rust-overlay }:
    flake-utils.lib.eachDefaultSystem (system:
      let
        pkgs = import nixpkgs {
          inherit system;
          overlays = [ rust-overlay.overlays.default ];
        };

        # Explicit extensions: rust-toolchain.toml also lists them, but the
        # override guarantees llvm-tools-preview (llvm-objcopy) is present
        # for tools/build-user.sh, regardless of TOML parsing.
        rustToolchain = (pkgs.rust-bin.fromRustupToolchainFile ./rust-toolchain.toml).override {
          extensions = [ "rust-src" "llvm-tools-preview" ];
        };
      in
      {
        devShells.default = pkgs.mkShell {
          name = "self_rust_os-dev";

          packages = [
            rustToolchain
            pkgs.rust-analyzer
            pkgs.qemu
            pkgs.nasm
            pkgs.lld
            pkgs.llvm
            pkgs.pkg-config
            pkgs.cargo-bootimage
            # For FAT32 disk image manipulation
            pkgs.mtools
          ];

          shellHook = ''
            echo "self_rust_os dev environment"
            echo "QEMU: $(qemu-system-x86_64 --version | head -1)"
            echo "Rust: $(rustc --version)"
            echo ""
            echo "Build commands:"
            echo "  cargo build --features grub"
            echo "  ./tools/build-user.sh wm portal snake forge hello"
            echo "  ./tools/build-grub-disk.sh"
            echo ""
            echo "Run (QEMU 11+ requires explicit VRAM for 720p):"
            echo "  qemu-system-x86_64 -drive file=target/grub-disk.img,format=raw -serial stdio -m 128 -vga std -global VGA.vgamem_mb=32"
          '';
        };
      }
    );
}
