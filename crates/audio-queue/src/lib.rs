//! Bounded AC97 descriptor accounting, independent of ports, pointers and the OS.
#![no_std]
use rustos_audio_abi::Error;
/// Descriptor count in the controller's fixed hardware ring.
pub const SLOTS: usize = 32;
/// FIFO accounting uses PCM frames (stereo pairs), not16-bit hardware samples.
#[derive(Clone, Default)]
pub struct Ring {
    head: usize,
    queued: usize,
    lengths: [u32; SLOTS],
    submitted: u64,
    completed: u64,
}
impl Ring {
    /// Reserve the next descriptor, applying backpressure before any mutation.
    pub fn push(&mut self, frames: u32) -> Result<usize, Error> {
        if frames == 0 || frames > 1024 {
            return Err(Error::Invalid);
        }
        if self.queued == SLOTS {
            return Err(Error::WouldBlock);
        }
        let submitted = self
            .submitted
            .checked_add(u64::from(frames))
            .ok_or(Error::Failed)?;
        let index = (self.head + self.queued) % SLOTS;
        self.lengths[index] = frames;
        self.queued += 1;
        self.submitted = submitted;
        Ok(index)
    }
    /// Apply a coherent hardware cursor/remaining-sample observation atomically.
    /// DCH with remaining samples is a pause; only DCH+zero completes the last slot.
    pub fn observe(&mut self, current: u8, halted: bool, samples_left: u16) -> Result<(), Error> {
        if current as usize >= SLOTS {
            return Err(Error::Failed);
        }
        if self.queued == 0 {
            return Ok(());
        }
        let distance = (current as usize + SLOTS - self.head) % SLOTS;
        if distance >= self.queued || u32::from(samples_left) > self.lengths[current as usize] * 2 {
            return Err(Error::Failed);
        }
        let completed = distance + usize::from(halted && samples_left == 0);
        for _ in 0..completed {
            self.completed += u64::from(self.lengths[self.head]);
            self.head = (self.head + 1) % SLOTS;
            self.queued -= 1;
        }
        Ok(())
    }
    /// PCM frames accepted during this lease.
    pub fn submitted(&self) -> u64 {
        self.submitted
    }
    /// Occupied descriptors.
    pub fn queued(&self) -> usize {
        self.queued
    }
    /// Fully consumed frames, before any current descriptor starts/resumes.
    pub fn completed(&self) -> u64 {
        self.completed
    }
    /// Rebase a quiesced controller to descriptor0, preserving the remaining
    /// current stereo frames. Returns the old head and consumed prefix frames
    /// so the driver can rotate buffers and adjust its first physical address.
    pub fn rebase(&mut self, samples_left: u16) -> Result<(usize, u32), Error> {
        let consumed = if self.queued == 0 {
            0
        } else {
            if samples_left == 0
                || samples_left % 2 != 0
                || u32::from(samples_left) > self.lengths[self.head] * 2
            {
                return Err(Error::Failed);
            }
            self.lengths[self.head] - u32::from(samples_left) / 2
        };
        let old_head = self.head;
        self.lengths.rotate_left(old_head);
        self.head = 0;
        if self.queued > 0 {
            self.lengths[0] -= consumed;
        }
        self.completed += u64::from(consumed);
        Ok((old_head, consumed))
    }
    /// Remaining whole frames in a live logical descriptor.
    pub fn frames(&self, index: usize) -> u32 {
        self.lengths[index]
    }
    /// Consumed PCM frames, excluding incomplete stereo pairs.
    pub fn played(&self, samples_left: u16) -> u64 {
        self.completed
            + if self.queued == 0 {
                0
            } else {
                u64::from(
                    self.lengths[self.head].saturating_sub(u32::from(samples_left).div_ceil(2)),
                )
            }
    }
}
