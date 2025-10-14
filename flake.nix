{
  description = "Dev-only flake for self_rust_os (reads toolchain from rust-toolchain.toml)";

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

       rustToolchain = pkgs.rust-bin.fromRustupToolchainFile ./rust-toolchain.toml;
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
          ];
        };
      }
    );
}
