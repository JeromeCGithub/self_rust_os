//! PS/2 mouse driver (IRQ12).
//!
//! Initializes the PS/2 auxiliary port, enables the mouse, and decodes the
//! standard 3-byte PS/2 mouse packets into a clamped cursor position and
//! button state.
//!
//! Packet format (3 bytes):
//! ```text
//! byte 0: Yov Xov Ysgn Xsgn 1 MidBtn RightBtn LeftBtn
//! byte 1: X movement (8-bit two's complement, low 8 bits of 9-bit value)
//! byte 2: Y movement (8-bit two's complement, low 8 bits of 9-bit value)
//! ```
//! Bit 3 of byte 0 is always 1 and is used to detect loss of packet sync.
//! X/Y are 9-bit signed values (sign bit in byte 0, magnitude in byte 1/2).
//! Y is positive-up in PS/2; it is flipped to positive-down for the screen.
//!
//! The public API ([`position`], [`buttons`]) is safe to call from any
//! context, including syscall handlers: it disables interrupts while taking
//! the device lock, so it can never deadlock against the IRQ handler.

use spin::Mutex;
use x86_64::instructions::port::Port;

use crate::{gfx, serial_println};

/// PS/2 data port (read: device output, write: device input).
const DATA_PORT: u16 = 0x60;
/// PS/2 status port (read) / command port (write).
const CMD_PORT: u16 = 0x64;

/// Controller command: enable the auxiliary (mouse) port.
const CMD_ENABLE_AUX: u8 = 0xA8;
/// Controller command: next data-port write goes to the mouse.
const CMD_WRITE_TO_MOUSE: u8 = 0xD4;
/// Controller command: read the command byte.
const CMD_READ_CMD_BYTE: u8 = 0x20;
/// Controller command: write the command byte.
const CMD_WRITE_CMD_BYTE: u8 = 0x60;

/// Command byte bit: enable IRQ12 (mouse interrupt).
const CMD_BYTE_IRQ12: u8 = 0x02;
/// Command byte bit: disable mouse clock (must be *cleared* to enable).
const CMD_BYTE_DISABLE_MOUSE: u8 = 0x20;

/// Mouse device command: set default settings.
const MOUSE_SET_DEFAULTS: u8 = 0xF6;
/// Mouse device command: enable data reporting (start sending packets).
const MOUSE_ENABLE_REPORTING: u8 = 0xF4;

/// Device acknowledgement byte.
const MOUSE_ACK: u8 = 0xFA;

/// Status register bit: output buffer full (data available to read).
const STATUS_OUTPUT_FULL: u8 = 0x01;
/// Status register bit: input buffer full (controller busy, do not write).
const STATUS_INPUT_FULL: u8 = 0x02;
/// Status register bit: set if the pending output byte came from the mouse
/// (clear = keyboard). Shared with the IRQ12 handler.
pub(crate) const STATUS_MOUSE_DATA: u8 = 0x20;
/// Status register bit: set if the output buffer holds a byte to read.
pub(crate) const STATUS_OUTPUT_READY: u8 = STATUS_OUTPUT_FULL;

/// Screen bounds for the cursor. These are dynamic now (from FbInfo),
/// so we query them at runtime instead of using compile-time consts.

/// Number of port-poll iterations before giving up (avoids hanging if the
/// controller or mouse is absent).
const PORT_TIMEOUT: u32 = 100_000;

/// Returns the screen width from the framebuffer info.
/// Falls back to 1280 (720p) if the framebuffer is not initialized (unit tests).
fn screen_width() -> i32 {
    gfx::try_fb_info().map(|info| info.width as i32).unwrap_or(1280)
}

/// Returns the screen height from the framebuffer info.
/// Falls back to 720 (720p) if the framebuffer is not initialized (unit tests).
fn screen_height() -> i32 {
    gfx::try_fb_info().map(|info| info.height as i32).unwrap_or(720)
}

/// Cursor position and button state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MouseState {
    /// Cursor X, clamped to `[0, screen_width)`.
    pub x: i32,
    /// Cursor Y, clamped to `[0, screen_height)` (positive-down).
    pub y: i32,
    /// Buttons: bit 0 = left, bit 1 = right, bit 2 = middle.
    pub buttons: u8,
}

/// A decoded 3-byte PS/2 mouse packet.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MouseEvent {
    /// X movement, 9-bit signed (positive = right).
    pub dx: i16,
    /// Y movement, 9-bit signed (positive = up, PS/2 convention).
    pub dy: i16,
    /// Buttons: bit 0 = left, bit 1 = right, bit 2 = middle.
    pub buttons: u8,
}

/// Internal device state: public cursor state plus packet assembler.
struct MouseDevice {
    state: MouseState,
    packet: [u8; 3],
    packet_index: usize,
}

impl MouseDevice {
    const fn new() -> Self {
        Self {
            // Start the cursor at a reasonable default; it will be clamped
            // to the actual screen size on first movement.
            state: MouseState {
                x: 640,
                y: 360,
                buttons: 0,
            },
            packet: [0; 3],
            packet_index: 0,
        }
    }
}

static MOUSE: Mutex<MouseDevice> = Mutex::new(MouseDevice::new());

/// Returns the current cursor position `(x, y)`.
///
/// Safe to call from any context (disables interrupts while reading).
pub fn position() -> (i32, i32) {
    x86_64::instructions::interrupts::without_interrupts(|| {
        let dev = MOUSE.lock();
        (dev.state.x, dev.state.y)
    })
}

/// Returns the current button state: bit 0 = left, bit 1 = right,
/// bit 2 = middle.
///
/// Safe to call from any context (disables interrupts while reading).
pub fn buttons() -> u8 {
    x86_64::instructions::interrupts::without_interrupts(|| MOUSE.lock().state.buttons)
}

/// Parses one 3-byte PS/2 mouse packet.
///
/// Returns `None` when the packet is invalid (bit 3 of byte 0 must always
/// be 1); the caller should then resynchronize by discarding bytes until a
/// byte with bit 3 set arrives as the first byte of a packet.
pub fn parse_packet(bytes: [u8; 3]) -> Option<MouseEvent> {
    let b0 = bytes[0];
    if b0 & 0x08 == 0 {
        return None;
    }
    let buttons = b0 & 0x07;
    // 9-bit two's complement: sign bit in byte 0, low 8 bits in byte 1/2.
    let dx = sign_extend_9bit(bytes[1], b0 & 0x10 != 0);
    let dy = sign_extend_9bit(bytes[2], b0 & 0x20 != 0);
    // On overflow the movement bytes are unreliable; report no movement.
    let dx = if b0 & 0x40 != 0 { 0 } else { dx };
    let dy = if b0 & 0x80 != 0 { 0 } else { dy };
    Some(MouseEvent { dx, dy, buttons })
}

/// Sign-extends an 8-bit magnitude with a separate 9th sign bit into `i16`.
fn sign_extend_9bit(magnitude: u8, sign: bool) -> i16 {
    let v = magnitude as i16;
    if sign {
        v - 256
    } else {
        v
    }
}

/// Feeds one raw byte from the mouse data port into the packet assembler.
///
/// Called from the IRQ12 handler. Silently drops bytes until packet sync is
/// re-established (a first byte with bit 3 set).
pub(crate) fn handle_byte(byte: u8) {
    let mut dev = MOUSE.lock();
    if dev.packet_index == 0 && byte & 0x08 == 0 {
        // Out of sync: ignore this byte and wait for a valid first byte.
        return;
    }
    let index = dev.packet_index;
    dev.packet[index] = byte;
    dev.packet_index = index + 1;
    if dev.packet_index == 3 {
        dev.packet_index = 0;
        if let Some(ev) = parse_packet(dev.packet) {
            // PS/2 Y is positive-up; screen Y is positive-down.
            // At 720p, apply a sensitivity multiplier (2x) so the cursor
            // doesn't feel sluggish covering 4x the pixels.
            let dx = i32::from(ev.dx) * 2;
            let dy = i32::from(ev.dy) * 2;
            dev.state.x = (dev.state.x + dx).clamp(0, screen_width() - 1);
            dev.state.y = (dev.state.y - dy).clamp(0, screen_height() - 1);
            dev.state.buttons = ev.buttons;
        }
    }
}

/// Initializes the PS/2 mouse.
///
/// Enables the auxiliary port, enables IRQ12 in the controller command byte
/// (preserving the keyboard's IRQ1 bit), sets mouse defaults, and enables
/// data reporting. Failures are logged to serial and are non-fatal: the
/// kernel keeps booting without a working mouse.
///
/// Must be called after the IDT/PICs are set up. Interrupts are disabled
/// internally while exchanging command bytes with the device so that ACK
/// bytes cannot be stolen by the IRQ handler mid-sequence.
pub fn init() {
    x86_64::instructions::interrupts::without_interrupts(|| {
        // SAFETY: PS/2 controller ports; bounded waits, no hangs.
        unsafe {
            // Discard any stale output byte (e.g. a self-test result) so it
            // cannot be mistaken for an ACK or packet byte later.
            let status: u8 = Port::new(CMD_PORT).read();
            if status & STATUS_OUTPUT_FULL != 0 {
                let _stale: u8 = Port::new(DATA_PORT).read();
            }

            // 1. Enable the auxiliary (mouse) port.
            if !write_cmd(CMD_ENABLE_AUX) {
                serial_println!("[mouse] controller did not accept ENABLE_AUX");
                return;
            }

            // 2. Enable IRQ12 and the mouse clock in the command byte,
            //    preserving all other bits (notably the keyboard's IRQ1).
            if !wait_write_ready() {
                serial_println!("[mouse] controller busy (read cmd byte)");
                return;
            }
            Port::new(CMD_PORT).write(CMD_READ_CMD_BYTE);
            if !wait_read_ready() {
                serial_println!("[mouse] no command byte response");
                return;
            }
            let mut cmd_byte: u8 = Port::new(DATA_PORT).read();
            cmd_byte |= CMD_BYTE_IRQ12;
            cmd_byte &= !CMD_BYTE_DISABLE_MOUSE;
            if !wait_write_ready() {
                serial_println!("[mouse] controller busy (write cmd byte)");
                return;
            }
            Port::new(CMD_PORT).write(CMD_WRITE_CMD_BYTE);
            if !wait_write_ready() {
                serial_println!("[mouse] controller busy (cmd byte data)");
                return;
            }
            Port::new(DATA_PORT).write(cmd_byte);

            // 3. Set defaults, then enable data reporting.
            if mouse_command(MOUSE_SET_DEFAULTS).is_err() {
                serial_println!("[mouse] SET_DEFAULTS failed");
                return;
            }
            if mouse_command(MOUSE_ENABLE_REPORTING).is_err() {
                serial_println!("[mouse] ENABLE_REPORTING failed");
                return;
            }

            serial_println!("[mouse] initialized (IRQ12, 3-byte packets)");
        }
    });
}

/// Sends a command byte to the mouse device and waits for its ACK.
///
/// Returns `Ok(())` on ACK, `Err(())` on timeout or non-ACK response.
unsafe fn mouse_command(cmd: u8) -> Result<(), ()> {
    if !wait_write_ready() {
        return Err(());
    }
    Port::new(CMD_PORT).write(CMD_WRITE_TO_MOUSE);
    if !wait_write_ready() {
        return Err(());
    }
    Port::new(DATA_PORT).write(cmd);
    if !wait_read_ready() {
        return Err(());
    }
    let ack: u8 = Port::new(DATA_PORT).read();
    if ack == MOUSE_ACK {
        Ok(())
    } else {
        serial_println!("[mouse] expected ACK, got {:#04x}", ack);
        Err(())
    }
}

/// Writes a controller command byte; returns false on timeout.
unsafe fn write_cmd(cmd: u8) -> bool {
    if !wait_write_ready() {
        return false;
    }
    Port::new(CMD_PORT).write(cmd);
    true
}

/// Spins until the controller input buffer is empty (ready for a write).
fn wait_write_ready() -> bool {
    for _ in 0..PORT_TIMEOUT {
        // SAFETY: status port read has no side effects.
        let status: u8 = unsafe { Port::new(CMD_PORT).read() };
        if status & STATUS_INPUT_FULL == 0 {
            return true;
        }
        core::hint::spin_loop();
    }
    false
}

/// Spins until the controller output buffer is full (data ready to read).
fn wait_read_ready() -> bool {
    for _ in 0..PORT_TIMEOUT {
        // SAFETY: status port read has no side effects.
        let status: u8 = unsafe { Port::new(CMD_PORT).read() };
        if status & STATUS_OUTPUT_FULL != 0 {
            return true;
        }
        core::hint::spin_loop();
    }
    false
}

#[cfg(test)]
mod tests {
    use super::{handle_byte, parse_packet, screen_height, screen_width, MouseEvent};

    #[test_case]
    fn test_parse_packet_basic_movement() {
        // No buttons, dx=+5, dy=+3 (up), bit 3 set.
        let ev = parse_packet([0x08, 5, 3]).expect("valid packet");
        assert_eq!(
            ev,
            MouseEvent {
                dx: 5,
                dy: 3,
                buttons: 0
            }
        );
    }

    #[test_case]
    fn test_parse_packet_negative_movement() {
        // X sign bit set, magnitude 0xFF -> dx = -1.
        // Y sign bit set, magnitude 0xFE -> dy = -2.
        let ev = parse_packet([0x08 | 0x10 | 0x20, 0xFF, 0xFE]).expect("valid packet");
        assert_eq!(ev.dx, -1);
        assert_eq!(ev.dy, -2);
        assert_eq!(ev.buttons, 0);
    }

    #[test_case]
    fn test_parse_packet_buttons() {
        // Left + middle pressed, no movement.
        let ev = parse_packet([0x08 | 0x01 | 0x04, 0, 0]).expect("valid packet");
        assert_eq!(ev.buttons, 0x05);
    }

    #[test_case]
    fn test_parse_packet_rejects_bad_sync_bit() {
        // Bit 3 clear -> invalid first byte.
        assert_eq!(parse_packet([0x00, 5, 3]), None);
        assert_eq!(parse_packet([0xF7, 5, 3]), None);
    }

    #[test_case]
    fn test_parse_packet_overflow_zeroes_movement() {
        // X overflow: dx must be discarded even though bytes say +10.
        let ev = parse_packet([0x08 | 0x40, 10, 0]).expect("valid packet");
        assert_eq!(ev.dx, 0);
        // Y overflow likewise.
        let ev = parse_packet([0x08 | 0x80, 0, 10]).expect("valid packet");
        assert_eq!(ev.dy, 0);
    }

    #[test_case]
    fn test_handle_byte_moves_cursor() {
        // Reset to a known position via direct state manipulation is not
        // possible; instead move relative and check clamping behavior at
        // the edges by sending large movements.
        //
        // Send dx=+300 (two packets of +150): cursor must clamp to the
        // right edge rather than overshoot.
        for _ in 0..2 {
            for b in [0x08u8, 150, 0] {
                handle_byte(b);
            }
        }
        let (x, _) = super::position();
        assert_eq!(x, screen_width() - 1);

        // Send dy=+200 (up): cursor must clamp to the top edge (y=0),
        // since PS/2 up-positive maps to screen up = decreasing y.
        for b in [0x08u8, 0, 200] {
            handle_byte(b);
        }
        let (_, y) = super::position();
        assert_eq!(y, 0);
    }

    #[test_case]
    fn test_handle_byte_resyncs_on_garbage() {
        // Feed garbage bytes without bit 3 set; they must be dropped and
        // the next valid packet must still be decoded.
        handle_byte(0x00);
        handle_byte(0x02);
        // Valid packet: left button pressed, no movement.
        for b in [0x09u8, 0, 0] {
            handle_byte(b);
        }
        assert_eq!(super::buttons(), 0x01);
        // Release.
        for b in [0x08u8, 0, 0] {
            handle_byte(b);
        }
        assert_eq!(super::buttons(), 0x00);
    }

    #[test_case]
    fn test_handle_byte_partial_packet_keeps_state() {
        // An incomplete packet must not change the cursor or buttons.
        let before = (super::position(), super::buttons());
        handle_byte(0x08);
        handle_byte(0x0A);
        assert_eq!((super::position(), super::buttons()), before);
        // Complete it: dx=+10 applied now.
        let (x0, _) = super::position();
        handle_byte(0x00);
        let (x1, _) = super::position();
        assert_eq!(x1, (x0 + 20).min(screen_width() - 1));
    }
}
