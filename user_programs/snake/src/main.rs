//! Snake game for `self_rust_os`, built on `userlib`.
//!
//! Runs in Ring 3, draws directly to the framebuffer mapped at 0x90_0000.
//! Uses `sys::ticks` for timing and `sys::read_nb` for non-blocking input.
//!
//! Controls: WASD or Arrow keys to move, Q to quit.

#![no_std]
#![no_main]

use userlib::{println, sys};

/// Framebuffer mapped at 0x90_0000 (32bpp).
const FB_ADDR: usize = 0x90_0000;

/// Global framebuffer geometry (set at startup).
static mut FB_WIDTH: usize = 1280;
static mut FB_HEIGHT: usize = 720;
static mut FB_PITCH: usize = 1280 * 4;

/// Game grid: scaled for 720p. 80x45 cells of 16x16 pixels.
const GRID_W: usize = 80;
const GRID_H: usize = 45;
const CELL: usize = 16;

/// Colors (32-bit XRGB).
const BLACK: u32 = 0x000000;
const GREEN: u32 = 0x00FF00;
const RED: u32 = 0xFF0000;
const WHITE: u32 = 0xFFFFFF;
const DARK_GREEN: u32 = 0x008800;

/// Ticks per move (PIT at ~18.2 Hz; 4 ticks ≈ 220ms).
const MOVE_TICKS: u64 = 4;

fn read_nb() -> Option<u8> {
    let mut buf = [0u8; 1];
    if sys::read_nb(sys::STDIN_FD, &mut buf) == 1 {
        Some(buf[0])
    } else {
        None
    }
}

/// Sets a pixel in the framebuffer (32bpp).
fn set_pixel(x: usize, y: usize, color: u32) {
    unsafe {
        if x < FB_WIDTH && y < FB_HEIGHT {
            let offset = y * FB_PITCH + x * 4;
            core::ptr::write((FB_ADDR + offset) as *mut u32, color);
        }
    }
}

/// Fills a rectangle.
fn fill_rect(x: usize, y: usize, w: usize, h: usize, color: u32) {
    for dy in 0..h {
        for dx in 0..w {
            set_pixel(x + dx, y + dy, color);
        }
    }
}

/// Clears the screen.
fn clear(color: u32) {
    unsafe {
        for y in 0..FB_HEIGHT {
            for x in 0..FB_WIDTH {
                set_pixel(x, y, color);
            }
        }
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
        for i in 0..3 {
            g.snake[i] = Pos {
                x: 40 - i,
                y: 22,
            };
        }
        g.place_food();
        g
    }

    fn rand(&mut self) -> usize {
        self.seed = self.seed.wrapping_mul(6364136223846793005).wrapping_add(1);
        (self.seed >> 32) as usize
    }

    fn place_food(&mut self) {
        loop {
            let x = self.rand() % GRID_W;
            let y = self.rand() % GRID_H;
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

        if new_head.x >= GRID_W || new_head.y >= GRID_H {
            return false;
        }

        for i in 0..self.len {
            if self.snake[i].x == new_head.x && self.snake[i].y == new_head.y {
                return false;
            }
        }

        for i in (1..self.len).rev() {
            self.snake[i] = self.snake[i - 1];
        }
        self.snake[0] = new_head;

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

        fill_rect(
            self.food.x * CELL,
            self.food.y * CELL,
            CELL,
            CELL,
            RED,
        );

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

        unsafe {
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
}

/// Entry point called by `userlib`'s runtime after `_start`.
#[no_mangle]
pub extern "C" fn user_main() {
    println!("Snake game starting (720p)...");
    println!("Controls: WASD / Arrows, Q to quit");

    // Get framebuffer geometry.
    if let Ok(info) = sys::fb_info() {
        unsafe {
            FB_WIDTH = info.width;
            FB_HEIGHT = info.height;
            FB_PITCH = info.pitch;
        }
    }

    let mut game = Game::new();
    let mut last_move = sys::ticks();

    loop {
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
                    println!("Quitting...");
                    sys::exit(0);
                }
                _ => {}
            }
        }

        let now = sys::ticks();
        if now.wrapping_sub(last_move) >= MOVE_TICKS {
            last_move = now;
            if !game.update() {
                clear(BLACK);
                println!("Game Over! Score: {}", game.score);
                let end = sys::ticks() + 36;
                while sys::ticks() < end {}
                sys::exit(0);
            }
            game.draw();
        }
    }
}
