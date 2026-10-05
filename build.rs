//! Build script: selects the linker script for the GRUB boot path.
//!
//! With the `grub` cargo feature, the kernel is loaded by GRUB2 as a
//! Multiboot2 ELF, so it needs a custom linker script that places the
//! Multiboot2 header first and uses `multiboot2_entry` as the entry point.
//! Without the feature, the default linker script is used (the `bootloader`
//! crate's `cargo bootimage` flow).

fn main() {
    #[cfg(feature = "grub")]
    {
        let manifest_dir = std::env::var("CARGO_MANIFEST_DIR").expect("missing manifest dir");
        let script = format!("{manifest_dir}/src/boot/grub-linker.ld");
        println!("cargo:rustc-link-arg=-T{script}");
        // Re-run if the linker script changes.
        println!("cargo:rerun-if-changed=src/boot/grub-linker.ld");
    }

    #[cfg(not(feature = "grub"))]
    {
        println!("cargo:rerun-if-changed=build.rs");
    }
}
