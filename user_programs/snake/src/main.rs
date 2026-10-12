//! Snake game for `self_rust_os`.
//!
//! Runs in Ring 3, draws directly to the framebuffer mapped at 0x90_0000.
//! Uses SYS_TICKS for timing and SYS_READ_NB for non-blocking input.
//!
//! Controls: WASD or Arrow keys to move, Q to quit.

#![no_std]
#![no_main]

use core::arch::asm;
use core::panic::PanicInfo;

/// Syscall numbers.
const SYS_EXIT: u64 = 0;
const SYS_WRITE: u64 = 1;
const SYS_TICKS: u64 = 3;
const SYS_READ_NB: u64 = 4;

/// Framebuffer mapped at 0x90_0000 (320x200, 8-bit color).
const FB_ADDR: usize = 0x90_0000;
const FB_WIDTH: usize = 320;
const FB_HEIGHT: usize = 200;

/// Game grid: 40x25 cells of 8x8 pixels.
const GRID_W: usize = 40;
const GRID_H: usize = 25;
const CELL: usize = 8;

/// Colors (VGA palette indices).
const BLACK: u8 = 0;
const GREEN: u8 = 2;
const RED: u8 = 4;
const WHITE: u8 = 15;
const DARK_GREEN: u8 = 10;

/// Ticks per move (PIT at ~18.2 Hz; 4 ticks ≈ 220ms).
const MOVE_TICKS: u64 = 4;

#[inline(always)]
unsafe fn syscall(num: u64, arg1: u64, arg2: u64, arg3: u64) -> u64 {
    let result: u64;
    asm!(
        "int 0x80",
        inlateout("rax") num => result,
        in("rdi") arg1,
        in("rsi") arg2,
        in("rdx") arg3,
        lateout("rcx") _,
        lateout("r11") _,
        options(nostack),
    );
    result
}

fn write(buf: &[u8]) {
    unsafe {
        syscall(SYS_WRITE, buf.as_ptr() as u64, buf.len() as u64, 0);
    }
}

fn ticks() -> u64 {
    unsafe { syscall(SYS_TICKS, 0, 0, 0) }
}

fn read_nb() -> Option<u8> {
    let mut buf = [0u8; 1];
    let n = unsafe { syscall(SYS_READ_NB, 0, buf.as_mut_ptr() as u64, 1) };
    if n == 1 {
        Some(buf[0])
    } else {
        None
    }
}

fn exit(code: u64) -> ! {
    unsafe {
        syscall(SYS_EXIT, code, 0, 0);
    }
    loop {
        unsafe { asm!("hlt", options(nomem, nostack)) };
    }
}

/// Sets a pixel in the framebuffer.
fn set_pixel(x: usize, y: usize, color: u8) {
    if x < FB_WIDTH && y < FB_HEIGHT {
        unsafe {
            core::ptr::write((FB_ADDR + y * FB_WIDTH + x) as *mut u8, color);
        }
    }
}

/// Fills a rectangle.
fn fill_rect(x: usize, y: usize, w: usize, h: usize, color: u8) {
    for dy in 0..h {
        for dx in 0..w {
            set_pixel(x + dx, y + dy, color);
        }
    }
}

/// Clears the screen.
fn clear(color: u8) {
    unsafe {
        core::ptr::write_bytes(FB_ADDR as *mut u8, color, FB_WIDTH * FB_HEIGHT);
    }
}

#[derive(Clone, Copy, PartialEq)]
struct Pos {
    x: usize,
    y: usize,
}

#[derive(Clone, Copy, PartialEq)]
enum Dir {
    Up,
    Down,
    Left,
    Right,
}

struct Game {
    snake: [Pos; 100],
    len: usize,
    dir: Dir,
    food: Pos,
    score: u32,
    seed: u64,
}

impl Game {
    fn new() -> Self {
        let mut g = Self {
            snake: [Pos { x: 0, y: 0 }; 100],
            len: 3,
            dir: Dir::Right,
            food: Pos { x: 10, y: 10 },
            score: 0,
            seed: 12345,
        };
        // Initial snake in the middle.
        for i in 0..3 {
            g.snake[i] = Pos {
                x: 20 - i,
                y: 12,
            };
        }
        g.place_food();
        g
    }

    /// Simple LCG for food placement.
    fn rand(&mut self) -> usize {
        self.seed = self.seed.wrapping_mul(6364136223846793005).wrapping_add(1);
        (self.seed >> 32) as usize
    }

    fn place_food(&mut self) {
        loop {
            let x = self.rand() % GRID_W;
            let y = self.rand() % GRID_H;
            // Don't place on snake.
            let mut on_snake = false;
            for i in 0..self.len {
                if self.snake[i].x == x && self.snake[i].y == y {
                    on_snake = true;
                    break;
                }
            }
            if !on_snake {
                self.food = Pos { x, y };
                break;
            }
        }
    }

    fn update(&mut self) -> bool {
        // Returns false on collision (game over).
        let head = self.snake[0];
        let new_head = match self.dir {
            Dir::Up => Pos {
                x: head.x,
                y: head.y.wrapping_sub(1),
            },
            Dir::Down => Pos {
                x: head.x,
                y: head.y + 1,
            },
            Dir::Left => Pos {
                x: head.x.wrapping_sub(1),
                y: head.y,
            },
            Dir::Right => Pos {
                x: head.x + 1,
                y: head.y,
            },
        };

        // Wall collision.
        if new_head.x >= GRID_W || new_head.y >= GRID_H {
            return false;
        }

        // Self collision.
        for i in 0..self.len {
            if self.snake[i].x == new_head.x && self.snake[i].y == new_head.y {
                return false;
            }
        }

        // Move snake.
        for i in (1..self.len).rev() {
            self.snake[i] = self.snake[i - 1];
        }
        self.snake[0] = new_head;

        // Eat food?
        if new_head.x == self.food.x && new_head.y == self.food.y {
            if self.len < 100 {
                self.len += 1;
            }
            self.score += 1;
            self.place_food();
        }

        true
    }

    fn draw(&self) {
        clear(BLACK);

        // Draw food.
        fill_rect(
            self.food.x * CELL,
            self.food.y * CELL,
            CELL,
            CELL,
            RED,
        );

        // Draw snake.
        for i in 0..self.len {
            let color = if i == 0 { GREEN } else { DARK_GREEN };
            fill_rect(
                self.snake[i].x * CELL,
                self.snake[i].y * CELL,
                CELL,
                CELL,
                color,
            );
        }

        // Draw border.
        for x in 0..FB_WIDTH {
            set_pixel(x, 0, WHITE);
            set_pixel(x, FB_HEIGHT - 1, WHITE);
        }
        for y in 0..FB_HEIGHT {
            set_pixel(0, y, WHITE);
            set_pixel(FB_WIDTH - 1, y, WHITE);
        }
    }
}

#[no_mangle]
#[link_section = ".text.start"]
pub extern "C" fn _start() -> ! {
    write(b"Snake game starting...\n");
    write(b"Controls: WASD / Arrows, Q to quit\n");

    let mut game = Game::new();
    let mut last_move = ticks();

    loop {
        // Non-blocking input.
        while let Some(c) = read_nb() {
            match c {
                b'w' | b'W' => {
                    if game.dir != Dir::Down {
                        game.dir = Dir::Up;
                    }
                }
                b's' | b'S' => {
                    if game.dir != Dir::Up {
                        game.dir = Dir::Down;
                    }
                }
                b'a' | b'A' => {
                    if game.dir != Dir::Right {
                        game.dir = Dir::Left;
                    }
                }
                b'd' | b'D' => {
                    if game.dir != Dir::Left {
                        game.dir = Dir::Right;
                    }
                }
                b'q' | b'Q' => {
                    write(b"Quitting...\n");
                    exit(0);
                }
                _ => {}
            }
        }

        // Game tick.
        let now = ticks();
        if now.wrapping_sub(last_move) >= MOVE_TICKS {
            last_move = now;
            if !game.update() {
                // Game over.
                clear(BLACK);
                write(b"Game Over! Score: ");
                // Simple score output via serial/write.
                let mut score_buf = [0u8; 10];
                let mut s = game.score;
                let mut i = 0;
                if s == 0 {
                    score_buf[0] = b'0';
                    i = 1;
                } else {
                    let mut digits = [0u8; 10];
                    let mut d = 0;
                    while s > 0 {
                        digits[d] = b'0' + (s % 10) as u8;
                        s /= 10;
                        d += 1;
                    }
                    for j in (0..d).rev() {
                        score_buf[i] = digits[j];
                        i += 1;
                    }
                }
                write(&score_buf[..i]);
                write(b"\n");
                // Wait a bit then exit.
                let end = ticks() + 36; // ~2 seconds.
                while ticks() < end {}
                exit(0);
            }
            game.draw();
        }
    }
}

#[panic_handler]
fn panic(_info: &PanicInfo) -> ! {
    write(b"PANIC in snake!\n");
    exit(1);
}
