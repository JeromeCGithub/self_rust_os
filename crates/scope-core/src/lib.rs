//! Serialized kernel scope bookkeeping; no syscalls, allocation or user pointers.
//! A kernel owns one registry for its lifetime. All caller keys must be derived
//! from live kernel context, never from request payloads. Charges belong to that
//! registry and are retained until actual resource release, including rollback.
//!
//! The hierarchy is system root -> session root -> service leaf, or a system
//! service leaf directly under the system root. Only init creates sessions; an
//! appointed session leader has its service factory, not its parent's close right.
//! Native wrappers must additionally enforce caller capabilities and validate a
//! leader's actual process membership during atomic publication. No decoded data
//! or user-selected process key grants authority.
//!
//! Usage includes descendants. Reservations check every ancestor and component
//! before changing anything. Non-system charges leave the configured recovery
//! floor available to the system root; sessions cannot consume the final four
//! scope slots. IDs never wrap or repeat. Closing prevents growth but accepts
//! release. Kernel callers must stop execution and retain charges through physical
//! reclamation and in-flight operations; this library does neither itself.
//!
//! Closure is durable until explicitly acknowledged. Child scope records are
//! acknowledged before their parent becomes closed. A dead manager's outstanding
//! scope controls pass to init. These events never consume process completion.
//! This crate is not a userspace ABI. The native scheduler adapter currently
//! accounts process/thread reservations, private frames and selected persistent
//! allocations, IPC handles/inboxes and descriptor/buffer leases; filesystem
//! grants/remaining allocation paths, socket and audio accounting remain separate
//! work. Host tests alone do not prove kernel containment.
#![no_std]
pub use rustos_process_abi::supervision::ProcessKey;
/// Fixed scope table, including the reserved system root.
pub const MAX_SCOPES: usize = 64;
/// Slots unavailable to sessions, preserving system recovery capacity.
pub const RECOVERY_SCOPES: usize = 4;
/// Maximum service records per session, including Closing and unacknowledged
/// Closed records. The session root is separate; empty leaves also consume slots.
pub const MAX_SESSION_SERVICES: usize = 16;
pub const SYSTEM: u64 = rustos_process_abi::principal::SYSTEM_SCOPE;
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
/// Bookkeeping/authority failure. No errno or syscall encoding is implied.
pub enum Error {
    Invalid,
    Denied,
    Missing,
    Closed,
    Limit,
    Sequence,
    InitDeath,
}
/// Counts include unpublished reservations and resources awaiting reclamation.
/// Pages count private physical frames; kernel_bytes accounts heap allocations
/// separately. The native adapter must charge every relevant allocation path.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Usage {
    pub processes: u64,
    pub threads: u64,
    pub pages: u64,
    pub kernel_bytes: u64,
    pub descriptors: u64,
    pub ipc_handles: u64,
    pub sockets: u64,
    pub audio_streams: u64,
}
impl Usage {
    pub const ZERO: Self = Self {
        processes: 0,
        threads: 0,
        pages: 0,
        kernel_bytes: 0,
        descriptors: 0,
        ipc_handles: 0,
        sockets: 0,
        audio_streams: 0,
    };
}
macro_rules! resources {
    ($($field:ident),+) => {
        impl Usage {
            fn fits(self, ceiling:Self)->bool {$(self.$field<=ceiling.$field)&&+}
            fn permits_growth(self, delta:Self, ceiling:Self)->bool {
                $((delta.$field==0 || self.$field<=ceiling.$field))&&+
            }
            fn increase_over(self, before:Self)->Self {Self{$($field:self.$field.saturating_sub(before.$field)),+}}
            fn room(self, used:Self)->Self {Self{$($field:self.$field.saturating_sub(used.$field)),+}}
            fn minimum(self, other:Self)->Self {Self{$($field:self.$field.min(other.$field)),+}}
            fn add(self, rhs:Self)->Result<Self,Error> {
                Ok(Self{$($field:self.$field.checked_add(rhs.$field).ok_or(Error::Limit)?),+})
            }
            fn subtract(self, rhs:Self)->Result<Self,Error> {
                Ok(Self{$($field:self.$field.checked_sub(rhs.$field).ok_or(Error::Invalid)?),+})
            }
        }
    }
}
resources!(
    processes,
    threads,
    pages,
    kernel_bytes,
    descriptors,
    ipc_handles,
    sockets,
    audio_streams
);
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
/// The bounded hierarchy permits no children beneath a service leaf.
pub enum Kind {
    System,
    Session,
    Service,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
/// Closed includes no charged resources and no remaining child records.
pub enum State {
    Open,
    Closing,
    Closed,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
/// First transition wins; a requested close is not rewritten by later exits.
pub enum CloseCause {
    Requested,
    OwnerDeath,
    LeaderDeath,
}
/// Descriptive data; possessing this record grants no authority.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Snapshot {
    pub id: u64,
    pub parent: Option<u64>,
    pub owner: ProcessKey,
    pub leader: Option<ProcessKey>,
    pub user: u32,
    pub session: u64,
    pub kind: Kind,
    pub state: State,
    pub cause: Option<CloseCause>,
    pub limit: Usage,
    pub used: Usage,
}
/// Opaque linear charge, belonging to one kernel registry. It cannot be forged,
/// cloned or implicitly released: kernel resource owners return it on teardown.
///
/// ```compile_fail
/// use rustos_scope_core::{Registry, Usage, SYSTEM};
/// let mut registry = Registry::new();
/// let charge = registry.reserve(SYSTEM, Usage::ZERO).unwrap();
/// let duplicate = charge.clone();
/// ```
///
/// ```compile_fail
/// use rustos_scope_core::{Charge, Usage};
/// let forged = Charge { scope: 1, used: Usage::ZERO };
/// ```
#[must_use = "retain the charge until resources are released, then return it to the registry"]
#[derive(Debug)]
pub struct Charge {
    scope: u64,
    used: Usage,
}
impl Charge {
    pub fn scope(&self) -> u64 {
        self.scope
    }
    pub fn used(&self) -> Usage {
        self.used
    }
}
/// One allocation-free registry per kernel lifetime. Serialize every operation
/// with the kernel's scheduler/resource lock; caller authentication is external.
pub struct Registry {
    slots: [Option<Snapshot>; MAX_SCOPES],
    next: u64,
    recovery: Usage,
}
impl Default for Registry {
    fn default() -> Self {
        Self::new()
    }
}
impl Registry {
    pub const fn new() -> Self {
        Self {
            slots: [None; MAX_SCOPES],
            next: 2,
            recovery: Usage::ZERO,
        }
    }
    /// Called once by trusted boot code; no userspace identity-setting operation.
    pub fn bootstrap(
        &mut self,
        init: ProcessKey,
        total: Usage,
        recovery: Usage,
    ) -> Result<(), Error> {
        if self.slots[0].is_some() || ProcessKey::decode(init.encode()) != Some(init) {
            return Err(Error::Invalid);
        }
        if !recovery.fits(total) {
            return Err(Error::Invalid);
        }
        self.slots[0] = Some(Snapshot {
            id: SYSTEM,
            parent: None,
            owner: init,
            leader: Some(init),
            user: 0,
            session: 0,
            kind: Kind::System,
            state: State::Open,
            cause: None,
            limit: total,
            used: Usage::ZERO,
        });
        self.recovery = recovery;
        Ok(())
    }
    fn index(&self, id: u64) -> Result<usize, Error> {
        self.slots
            .iter()
            .position(|slot| slot.is_some_and(|s| s.id == id))
            .ok_or(Error::Missing)
    }
    fn view(&self, id: u64) -> Result<Snapshot, Error> {
        self.slots[self.index(id)?].ok_or(Error::Missing)
    }
    fn valid(key: ProcessKey) -> bool {
        ProcessKey::decode(key.encode()) == Some(key)
    }
    fn insert(&mut self, mut snapshot: Snapshot, recovery: bool) -> Result<u64, Error> {
        if !recovery && self.slots.iter().flatten().count() >= MAX_SCOPES - RECOVERY_SCOPES {
            return Err(Error::Limit);
        }
        let slot = self
            .slots
            .iter()
            .position(Option::is_none)
            .ok_or(Error::Limit)?;
        let next = self.next.checked_add(1).ok_or(Error::Sequence)?;
        snapshot.id = self.next;
        if snapshot.kind == Kind::Session {
            snapshot.session = snapshot.id;
        }
        self.slots[slot] = Some(snapshot);
        self.next = next;
        Ok(snapshot.id)
    }
    fn child(
        &mut self,
        caller: ProcessKey,
        parent: u64,
        user: Option<u32>,
        limit: Usage,
    ) -> Result<u64, Error> {
        let base = self.view(parent)?;
        let session = user.is_some();
        let permitted = match (base.kind, session) {
            (Kind::System, _) => base.owner == caller,
            (Kind::Session, false) => base.leader == Some(caller),
            _ => false,
        };
        if !permitted {
            return Err(Error::Denied);
        }
        if base.state != State::Open {
            return Err(Error::Closed);
        }
        if user.is_some_and(|id| id == 0 || id == u32::MAX) {
            return Err(Error::Invalid);
        }
        if base.kind == Kind::Session
            && self
                .slots
                .iter()
                .flatten()
                .filter(|node| node.parent == Some(parent))
                .count()
                >= MAX_SESSION_SERVICES
        {
            return Err(Error::Limit);
        }
        let ceiling = if session {
            base.limit.subtract(self.recovery)?
        } else {
            base.limit
        };
        if !limit.fits(ceiling) {
            return Err(Error::Limit);
        }
        self.insert(
            Snapshot {
                id: 0,
                parent: Some(parent),
                owner: caller,
                leader: None,
                user: user.unwrap_or(base.user),
                session: base.session,
                kind: if session {
                    Kind::Session
                } else {
                    Kind::Service
                },
                state: State::Open,
                cause: None,
                limit,
                used: Usage::ZERO,
            },
            base.kind == Kind::System && !session,
        )
    }
    /// Only the kernel-selected init can create identities/sessions.
    pub fn create_session(
        &mut self,
        caller: ProcessKey,
        user: u32,
        limit: Usage,
    ) -> Result<u64, Error> {
        self.child(caller, SYSTEM, Some(user), limit)
    }
    /// Factory authority belongs to init or the explicitly appointed session
    /// leader. A service scope cannot be used as another recursive factory.
    pub fn create_service(
        &mut self,
        caller: ProcessKey,
        parent: u64,
        limit: Usage,
    ) -> Result<u64, Error> {
        self.child(caller, parent, None, limit)
    }
    /// Bind only the actual freshly constructed process, atomically at publication.
    /// The native constructor must also validate its membership before this call.
    pub fn appoint_leader(
        &mut self,
        caller: ProcessKey,
        scope: u64,
        leader: ProcessKey,
    ) -> Result<(), Error> {
        let index = self.index(scope)?;
        let duplicate = self
            .slots
            .iter()
            .flatten()
            .any(|node| node.leader == Some(leader));
        let node = self.slots[index].as_mut().ok_or(Error::Missing)?;
        if node.owner != caller || node.kind == Kind::System {
            return Err(Error::Denied);
        }
        if node.state != State::Open {
            return Err(Error::Closed);
        }
        if !Self::valid(leader) || leader == caller || node.leader.is_some() || duplicate {
            return Err(Error::Invalid);
        }
        node.leader = Some(leader);
        Ok(())
    }
    /// Owners can inspect their objects; session leaders can inspect their own
    /// session root without acquiring its controller's close authority.
    pub fn status(&self, caller: ProcessKey, scope: u64) -> Result<Snapshot, Error> {
        let view = self.view(scope)?;
        if view.owner != caller && !(view.kind == Kind::Session && view.leader == Some(caller)) {
            return Err(Error::Denied);
        }
        Ok(view)
    }
    /// Return the leaf/session/system ancestry; the bounded hierarchy has depth 3.
    fn lineage(&self, scope: u64) -> Result<([usize; 3], usize), Error> {
        let mut chain = [0; 3];
        let mut id = scope;
        for count in 0..3 {
            let index = self.index(id)?;
            chain[count] = index;
            if let Some(parent) = self.slots[index].ok_or(Error::Missing)?.parent {
                id = parent;
            } else {
                return Ok((chain, count + 1));
            }
        }
        Err(Error::Invalid)
    }
    fn charge(&mut self, scope: u64, used: Usage) -> Result<(), Error> {
        let (chain, count) = self.lineage(scope)?;
        // Validate every component of every ancestor before changing any counter.
        for &index in &chain[..count] {
            let node = self.slots[index].ok_or(Error::Missing)?;
            if node.state != State::Open {
                return Err(Error::Closed);
            }
            let ceiling = if node.kind == Kind::System && scope != SYSTEM {
                node.limit.subtract(self.recovery)?
            } else {
                node.limit
            };
            // Init can deliberately consume its recovery floor in one domain.
            // A zero delta in that domain must not block unrelated resources.
            if !node.used.add(used)?.permits_growth(used, ceiling) {
                return Err(Error::Limit);
            }
        }
        for &index in &chain[..count] {
            let node = self.slots[index].as_mut().ok_or(Error::Missing)?;
            node.used = node.used.add(used)?;
        }
        Ok(())
    }
    /// Kernel-side remaining room after every ancestor/recovery constraint.
    /// Choose the scope from kernel ownership metadata, not unvalidated input.
    pub fn available(&self, scope: u64) -> Result<Usage, Error> {
        let (chain, count) = self.lineage(scope)?;
        let mut result = self.view(scope)?.limit;
        for &index in &chain[..count] {
            let node = self.slots[index].ok_or(Error::Missing)?;
            if node.state != State::Open {
                return Err(Error::Closed);
            }
            let ceiling = if node.kind == Kind::System && scope != SYSTEM {
                node.limit.subtract(self.recovery)?
            } else {
                node.limit
            };
            result = result.minimum(ceiling.room(node.used));
        }
        Ok(result)
    }
    /// Reserve before allocation/publication; a failed constructor returns the
    /// opaque charge after rollback. Native callers choose scope from kernel data.
    pub fn reserve(&mut self, scope: u64, used: Usage) -> Result<Charge, Error> {
        self.charge(scope, used)?;
        Ok(Charge { scope, used })
    }
    /// Reserve an additional delta before a live resource allocation. Failure
    /// preserves both the charge and every aggregate counter.
    pub fn grow(&mut self, charge: &mut Charge, used: Usage) -> Result<(), Error> {
        let total = charge.used.add(used)?;
        self.charge(charge.scope, used)?;
        charge.used = total;
        Ok(())
    }
    /// Release only after the corresponding resources really ceased to exist.
    /// Closing scopes accept release, but cannot acquire replacement resources.
    pub fn release_part(&mut self, charge: &mut Charge, used: Usage) -> Result<(), Error> {
        let remaining = charge.used.subtract(used)?;
        if used == Usage::ZERO {
            return Ok(());
        }
        let (chain, count) = self.lineage(charge.scope)?;
        for &index in &chain[..count] {
            self.slots[index]
                .ok_or(Error::Missing)?
                .used
                .subtract(used)?;
        }
        for &index in &chain[..count] {
            let node = self.slots[index].as_mut().ok_or(Error::Missing)?;
            node.used = node.used.subtract(used)?;
        }
        charge.used = remaining;
        self.pump();
        Ok(())
    }
    /// Return the entire charge after actual destruction or allocation rollback.
    pub fn release(&mut self, mut charge: Charge) -> Result<(), Error> {
        let used = charge.used;
        self.release_part(&mut charge, used)
    }
    fn mark_closing(&mut self, scope: u64, cause: CloseCause) {
        for index in 0..MAX_SCOPES {
            let Some(node) = self.slots[index] else {
                continue;
            };
            let selected = self.lineage(node.id).is_ok_and(|(chain, count)| {
                chain[..count]
                    .iter()
                    .any(|&i| self.slots[i].is_some_and(|s| s.id == scope))
            });
            if selected && node.state == State::Open {
                self.slots[index] = Some(Snapshot {
                    state: State::Closing,
                    cause: Some(cause),
                    ..node
                });
            }
        }
    }
    fn pump(&mut self) {
        // Closed child records remain until acknowledged, so a parent becomes
        // observable only after its descendants' records have been released.
        for index in 0..MAX_SCOPES {
            let Some(node) = self.slots[index] else {
                continue;
            };
            if node.state == State::Closing
                && node.used == Usage::ZERO
                && !self
                    .slots
                    .iter()
                    .flatten()
                    .any(|child| child.parent == Some(node.id))
            {
                self.slots[index] = Some(Snapshot {
                    state: State::Closed,
                    ..node
                });
            }
        }
    }
    /// Begin terminal closure of an owned scope and descendants, idempotently.
    /// The system root cannot be closed through this interface.
    pub fn close(&mut self, caller: ProcessKey, scope: u64) -> Result<(), Error> {
        let node = self.view(scope)?;
        if node.owner != caller || node.kind == Kind::System {
            return Err(Error::Denied);
        }
        self.mark_closing(scope, CloseCause::Requested);
        self.pump();
        Ok(())
    }
    /// Trusted kernel death notification, not a request containing a chosen PID.
    /// Stops new reservations; kernel callers must terminate affected execution.
    /// Reassign orphaned scope controls to init without consuming process status.
    pub fn owner_died(&mut self, owner: ProcessKey) -> Result<(), Error> {
        if !Self::valid(owner) {
            return Err(Error::Invalid);
        }
        let init = self.view(SYSTEM)?.owner;
        if owner == init {
            return Err(Error::InitDeath);
        }
        for index in 0..MAX_SCOPES {
            if let Some(node) = self.slots[index] {
                if node.owner == owner {
                    self.mark_closing(node.id, CloseCause::OwnerDeath);
                    if let Some(scope) = self.slots[index].as_mut() {
                        scope.owner = init;
                    }
                }
            }
        }
        for index in 0..MAX_SCOPES {
            if let Some(node) = self.slots[index] {
                if node.leader == Some(owner) {
                    self.mark_closing(node.id, CloseCause::LeaderDeath);
                }
            }
        }
        self.pump();
        Ok(())
    }
    /// Trusted kernel reaper lookup for a process's kernel-owned scope identity.
    /// This is not an authorization API or a userspace PID selector.
    pub fn controller(&self, scope: u64) -> Option<ProcessKey> {
        self.view(scope).ok().map(|node| node.owner)
    }
    /// Kernel-side victim selection; not an authorization or process lookup API.
    pub fn is_closing(&self, scope: u64) -> bool {
        self.view(scope).is_ok_and(|node| node.state != State::Open)
    }
    /// Peek without consuming: failed userspace copies must not lose completion.
    pub fn next_closed(&self, caller: ProcessKey) -> Option<Snapshot> {
        self.slots
            .iter()
            .flatten()
            .find(|node| node.state == State::Closed && node.owner == caller)
            .copied()
    }
    /// Explicit acknowledgement releases a closed scope record, never child status.
    pub fn ack_closed(&mut self, caller: ProcessKey, scope: u64) -> Result<(), Error> {
        let index = self.index(scope)?;
        let node = self.slots[index].ok_or(Error::Missing)?;
        if node.owner != caller || node.kind == Kind::System {
            return Err(Error::Denied);
        }
        if node.state != State::Closed {
            return Err(Error::Closed);
        }
        self.slots[index] = None;
        self.pump();
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn sequence_exhaustion_is_atomic_and_never_wraps_or_issues_reserved_ids() {
        let init = ProcessKey {
            pid: 1,
            generation: 1,
        };
        let mut registry = Registry::new();
        registry.bootstrap(init, Usage::ZERO, Usage::ZERO).unwrap();
        registry.next = u64::MAX - 1;
        let last = registry.create_service(init, SYSTEM, Usage::ZERO).unwrap();
        assert_eq!(last, u64::MAX - 1);
        registry.close(init, last).unwrap();
        registry.ack_closed(init, last).unwrap();
        assert_eq!(
            registry.create_service(init, SYSTEM, Usage::ZERO),
            Err(Error::Sequence)
        );
        assert_eq!(registry.next, u64::MAX);
        assert_eq!(registry.slots.iter().flatten().count(), 1);
        assert!(registry.next_closed(init).is_none());
    }
}

mod transaction;
pub use transaction::{Growth, Reservation};

mod transfer;
