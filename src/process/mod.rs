//! PID-owned resources and native thread ownership, independent of run queues.
use alloc::{boxed::Box, collections::VecDeque, vec::Vec};
use x86_64::structures::paging::PhysFrame;
/// PID identifies shared process resources and privileges.
pub type Pid = u32;
/// Generation-checked native execution identifier.
pub type Tid = u64;
/// Process resource lifetime, independent of each thread's execution state.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProcessState {
    /// Resources are live and may admit new threads.
    Live,
    /// All threads have been stopped; waiting for reaping and quiescence.
    Zombie,
}
/// Execution state of one native thread.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ThreadState {
    /// Runnable execution context or live process.
    Ready,
    /// Thread executing on exactly one CPU.
    Running,
    /// Thread parked until an event.
    Blocked,
    /// Finished execution, pending reclamation/reaping.
    Zombie,
}
/// Thread wait condition; only scalar addresses/identities survive blocking.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BlockReason {
    /// No active wait.
    None,
    /// Focused keyboard input.
    Keyboard,
    /// PID completion.
    WaitPid(Pid),
    /// Absolute PIT tick deadline.
    Timer(u64),
    /// Same-process thread completion.
    Join(Tid),
    /// Process-private atomic-word address and optional deadline.
    Word(u64, Option<u64>),
    /// Process mailbox readiness and optional PIT deadline.
    Mailbox(Option<u64>),
}
/// PID-owned resources, shared by all threads in this address space.
pub struct Process {
    /// Resource/privilege identity.
    pub pid: Pid,
    /// Live or zombie process status.
    pub state: ProcessState,
    /// Owned P4 frame.
    pub p4_frame: PhysFrame,
    /// Process completion status.
    pub exit_code: u64,
    /// Private surface frames (included in owned_frames), empty for PID 0.
    pub surface: Vec<PhysFrame>,
    /// Console drawing cursor.
    pub text_cursor: (usize, usize),
    /// Console may draw on the private surface.
    pub console_enabled: bool,
    /// Published drawing revision.
    pub surface_revision: u64,
    pub(crate) owned_frames: Vec<PhysFrame>,
    pub(crate) stack_slots: u64,
    pub(crate) inbox: [u64; 16],
    pub(crate) inbox_head: usize,
    pub(crate) inbox_len: usize,
    pub(crate) io_peer: Option<(Pid, bool)>,
    pub(crate) io_inbox: VecDeque<u32>,
}
pub(crate) struct UserStack {
    pub(crate) slot: usize,
    pub(crate) user_base: u64,
    pub(crate) frames: Vec<PhysFrame>,
}
pub(crate) struct Thread {
    pub(crate) tid: Tid,
    pub(crate) pid: Pid,
    pub(crate) state: ThreadState,
    pub(crate) reason: BlockReason,
    pub(crate) saved_kernel_rsp: u64,
    pub(crate) kernel_stack: Box<[u128]>,
    pub(crate) user_stack: Option<UserStack>,
    pub(crate) affinity: u64,
    pub(crate) detached: bool,
    pub(crate) joiner: Option<Tid>,
}
impl Thread {
    pub(crate) fn kernel_stack_top(&self) -> u64 {
        self.kernel_stack.as_ptr() as u64 + (self.kernel_stack.len() * 16) as u64
    }
}

/// Bounded live PCB slots keyed by monotonically allocated PID. Reaping reuses
/// storage without reusing the externally visible PID or retaining empty history.
pub(crate) struct ProcessTable {
    slots: Vec<Option<Process>>,
}
impl ProcessTable {
    pub(crate) const fn new() -> Self {
        Self { slots: Vec::new() }
    }
    pub(crate) fn reserve(&mut self) {
        self.slots.reserve_exact(128);
    }
    pub(crate) fn insert(&mut self, process: Process) {
        if let Some(slot) = self.slots.iter_mut().find(|slot| slot.is_none()) {
            *slot = Some(process);
        } else {
            assert!(
                self.slots.len() < 128,
                "process quota checked before construction"
            );
            self.slots.push(Some(process));
        }
    }
    pub(crate) fn get(&self, pid: usize) -> Option<&Option<Process>> {
        self.slots
            .iter()
            .find(|slot| slot.as_ref().map(|p| p.pid as usize) == Some(pid))
    }
    pub(crate) fn get_mut(&mut self, pid: usize) -> Option<&mut Option<Process>> {
        self.slots
            .iter_mut()
            .find(|slot| slot.as_ref().map(|p| p.pid as usize) == Some(pid))
    }
    pub(crate) fn iter(&self) -> core::slice::Iter<'_, Option<Process>> {
        self.slots.iter()
    }
}
impl core::ops::Index<usize> for ProcessTable {
    type Output = Option<Process>;
    fn index(&self, pid: usize) -> &Self::Output {
        self.get(pid).expect("live process identity")
    }
}
impl core::ops::IndexMut<usize> for ProcessTable {
    fn index_mut(&mut self, pid: usize) -> &mut Self::Output {
        self.get_mut(pid).expect("live process identity")
    }
}
