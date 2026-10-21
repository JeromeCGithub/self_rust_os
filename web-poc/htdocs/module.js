// QEMU command line for the self_rust_os browser POC.
//
// The guest is the *unmodified* 64-bit OS: SeaBIOS boots GRUB2 from the
// MBR of grub-disk.img, GRUB loads /kernel.elf with Multiboot2, exactly
// like native QEMU. The disk is attached as an IDE drive because the
// kernel talks ATA PIO to it.
//
// Display: this first milestone uses -nographic, so the emulated serial
// port (COM1) is wired to the page terminal. The kernel mirrors its boot
// log and panic messages to COM1, and the window manager reports its
// startup there too, which is enough to validate the whole boot chain
// (SeaBIOS -> GRUB -> Multiboot2 -> kernel -> scheduler -> WM) inside
// the browser. Rendering the VGA framebuffer on a canvas is the next
// milestone; see ../README.md.
if (typeof Module === 'undefined') {
    Module = {};
}
Module['preRun'] = Module['preRun'] || [];
Module['arguments'] = [
    '-nographic', '-M', 'pc', '-m', '128M', '-accel', 'tcg,tb-size=500',
    '-L', '/pack/',
    '-nic', 'none',
    '-boot', 'order=c',
    '-drive', 'file=/pack/grub-disk.img,format=raw,if=ide,index=0',
];
