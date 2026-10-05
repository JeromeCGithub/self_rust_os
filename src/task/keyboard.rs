//! Keyboard input handling.

use core::{
    pin::Pin,
    task::{Context, Poll},
};

use conquer_once::spin::OnceCell;
use crossbeam_queue::ArrayQueue;
use futures_util::{task::AtomicWaker, Stream, StreamExt};
use pc_keyboard::{layouts, HandleControl, Keyboard, ScancodeSet1};

use crate::{print, println};

static SCANCODE_QUEUE: OnceCell<ArrayQueue<u8>> = OnceCell::uninit();

static WAKER: AtomicWaker = AtomicWaker::new();

pub(crate) fn add_scancode(scancode: u8) {
    if let Ok(queue) = SCANCODE_QUEUE.try_get() {
        if let Err(_) = queue.push(scancode) {
            println!("WARNING: scancode queue full; dropping keyboard input");
        } else {
            WAKER.wake();
        }
    } else {
        println!("WARNING: scancode queue uninitialized");
    }
}

/// Tries to pop a scancode from the queue for synchronous syscall reads.
///
/// Returns `None` if the queue is empty or not yet initialized.
pub fn try_pop_scancode() -> Option<u8> {
    SCANCODE_QUEUE.try_get().ok()?.pop()
}

/// Concrete implementation of a stream for keyboard scancode queue.
pub struct ScancodeStream {
    _private: (),
}

impl ScancodeStream {
    /// Create a new scan code stream.
    pub fn new() -> Self {
        SCANCODE_QUEUE
            .try_init_once(|| ArrayQueue::new(100))
            .expect("ScancodeStream::new should only be called once");
        ScancodeStream { _private: () }
    }
}

impl Stream for ScancodeStream {
    type Item = u8;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let queue = SCANCODE_QUEUE
            .try_get()
            .expect("WARNING: scancode queue not initialized");

        if let Some(scancode) = queue.pop() {
            return Poll::Ready(Some(scancode));
        }

        WAKER.register(&cx.waker());

        match queue.pop() {
            Some(scancode) => {
                WAKER.take();
                Poll::Ready(Some(scancode))
            }
            None => Poll::Pending,
        }
    }
}

/// Task that handle when a key is pressed
pub async fn print_keypresses() {
    let mut scancodes = ScancodeStream::new();
    let mut keyboard = Keyboard::new(
        ScancodeSet1::new(),
        layouts::Us104Key,
        HandleControl::Ignore,
    );

    while let Some(scancode) = scancodes.next().await {
        if let Ok(Some(key_event)) = keyboard.add_byte(scancode) {
            if let Some(key) = keyboard.process_keyevent(key_event) {
                match key {
                    pc_keyboard::DecodedKey::RawKey(key) => print!("{:?}", key),
                    pc_keyboard::DecodedKey::Unicode(character) => print!("{}", character),
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{try_pop_scancode, ScancodeStream};

    #[test_case]
    fn test_scancode_queue_push_pop() {
        // Initialize the queue (only once per test run).
        let _stream = ScancodeStream::new();

        // Queue should be empty initially (or have leftovers drained).
        while try_pop_scancode().is_some() {}

        // Push and pop a scancode.
        super::add_scancode(0x1E); // 'a' key press
        assert_eq!(try_pop_scancode(), Some(0x1E));
        assert_eq!(try_pop_scancode(), None);
    }
}
