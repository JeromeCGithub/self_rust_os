//! Device drivers and bus discovery. All code here executes in Ring 0.
//! Core services own processes, memory allocation, filesystems and protocols;
//! drivers own device registers, transport state and device DMA buffers.
pub mod block;
pub mod input;
pub mod net;
pub mod pci;
pub mod tty;
pub mod video;
pub mod char;
pub mod rtc;
