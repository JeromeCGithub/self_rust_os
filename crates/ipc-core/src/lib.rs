//! Kernel channel object state. Callers serialize access and authenticate owners.
#![no_std]
extern crate alloc;
use alloc::{collections::VecDeque, vec::Vec};
pub use rustos_ipc_abi as abi;
use rustos_ipc_abi::{Binding, Event, Handle, Kind, ProcessKey, Rights};
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    Invalid,
    Denied,
    Full,
    Closed,
    Limit,
    Allocation,
    Sequence,
}
/// An event remains queued until the caller successfully copies it to userspace.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Pending {
    pub event: Event,
    pub token: u64,
}
/// Live endpoint-owned reservations, including empty inbox capacity. Native
/// kernels serialize snapshots with object publication/charge transfer.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Storage {
    /// Number of live owned endpoint objects.
    pub handles: usize,
    /// Reserved queue capacity in bytes, independent of queued message count.
    pub inbox_bytes: usize,
}
/// Minimum queue storage to reserve before a pair allocation. The actual
/// allocator capacity is returned by owner_usage and must also be charged.
pub const PAIR_INBOX_BYTES: usize = 2 * abi::MAX_MESSAGES * core::mem::size_of::<Pending>();
struct Endpoint {
    handle: Handle,
    owner: Option<ProcessKey>,
    rights: Rights,
    tag: Option<u32>,
    inbox: VecDeque<Pending>,
    closed_reported: bool,
    readable: bool,
    writable: Option<u64>,
    served: u64,
}
struct Channel {
    ends: [Endpoint; 2],
}
/// Access requires one external lock; no callbacks or pointers survive operations.
pub struct Registry {
    slots: Vec<Option<Channel>>,
    next_id: u64,
    turn: u64,
}
impl Default for Registry {
    fn default() -> Self {
        Self::new()
    }
}
impl Registry {
    pub const fn new() -> Self {
        Self {
            slots: Vec::new(),
            next_id: 1,
            turn: 0,
        }
    }
    fn valid(owner: ProcessKey) -> bool {
        ProcessKey::decode(owner.encode()) == Some(owner)
    }
    fn locate(&self, owner: ProcessKey, handle: Handle) -> Result<(usize, usize), Error> {
        if !Self::valid(owner) || !handle.valid() {
            return Err(Error::Denied);
        }
        self.slots
            .iter()
            .enumerate()
            .find_map(|(index, slot)| {
                slot.as_ref()?
                    .ends
                    .iter()
                    .enumerate()
                    .find(|(_, e)| e.owner == Some(owner) && e.handle == handle)
                    .map(|(side, _)| (index, side))
            })
            .ok_or(Error::Denied)
    }
    pub fn pair(&mut self, owner: ProcessKey) -> Result<[Handle; 2], Error> {
        if !Self::valid(owner) {
            return Err(Error::Invalid);
        }
        if self.handles(owner) > abi::MAX_HANDLES - 2 {
            return Err(Error::Limit);
        }
        let index = self
            .slots
            .iter()
            .position(Option::is_none)
            .unwrap_or(self.slots.len());
        if index >= abi::MAX_CHANNELS {
            return Err(Error::Limit);
        }
        let next = self
            .next_id
            .checked_add(2)
            .filter(|&id| id <= abi::MAX_ID)
            .ok_or(Error::Sequence)?;
        let mut a = VecDeque::new();
        let mut b = VecDeque::new();
        a.try_reserve_exact(abi::MAX_MESSAGES)
            .map_err(|_| Error::Allocation)?;
        b.try_reserve_exact(abi::MAX_MESSAGES)
            .map_err(|_| Error::Allocation)?;
        if index == self.slots.len() {
            self.slots.try_reserve(1).map_err(|_| Error::Allocation)?;
        }
        let handles = [Handle(self.next_id), Handle(self.next_id + 1)];
        let endpoint = |handle, inbox| Endpoint {
            handle,
            owner: Some(owner),
            rights: Rights::ALL,
            tag: None,
            inbox,
            closed_reported: false,
            readable: true,
            writable: None,
            served: 0,
        };
        let channel = Some(Channel {
            ends: [endpoint(handles[0], a), endpoint(handles[1], b)],
        });
        if index == self.slots.len() {
            self.slots.push(channel);
        } else {
            self.slots[index] = channel;
        }
        self.next_id = next;
        Ok(handles)
    }
    pub fn send(&mut self, owner: ProcessKey, handle: Handle, payload: &[u8]) -> Result<(), Error> {
        if payload.len() > abi::MAX_PAYLOAD {
            return Err(Error::Invalid);
        }
        let (index, side) = self.locate(owner, handle)?;
        let channel = self.slots[index].as_mut().ok_or(Error::Invalid)?;
        if !channel.ends[side].rights.contains(Rights::SEND) {
            return Err(Error::Denied);
        }
        let target = &mut channel.ends[1 - side];
        if target.owner.is_none() {
            return Err(Error::Closed);
        }
        if !target.rights.contains(Rights::RECEIVE) {
            return Err(Error::Denied);
        }
        if target.inbox.len() >= abi::MAX_MESSAGES {
            return Err(Error::Full);
        }
        let next = self
            .next_id
            .checked_add(1)
            .filter(|&id| id <= abi::MAX_ID)
            .ok_or(Error::Sequence)?;
        let event = Event::message(target.handle, owner, payload).map_err(|_| Error::Invalid)?;
        // Pair reserves the entire bounded queue. This insertion never allocates.
        target.inbox.push_back(Pending {
            event,
            token: self.next_id,
        });
        self.next_id = next;
        Ok(())
    }
    fn pending(channel: &Channel, side: usize) -> Option<Pending> {
        let endpoint = &channel.ends[side];
        let peer = &channel.ends[1 - side];
        if endpoint.owner.is_none() {
            return None;
        }
        if endpoint.readable && endpoint.rights.contains(Rights::RECEIVE) {
            if let Some(message) = endpoint.inbox.front() {
                return Some(*message);
            }
        }
        let kind_token =
            if peer.owner.is_none() && !endpoint.closed_reported && endpoint.inbox.is_empty() {
                Some((Kind::PeerClosed, peer.handle.0))
            } else if endpoint.rights.contains(Rights::SEND)
                && peer.owner.is_some()
                && peer.rights.contains(Rights::RECEIVE)
                && peer.inbox.len() < abi::MAX_MESSAGES
            {
                endpoint.writable.map(|token| (Kind::Writable, token))
            } else {
                None
            };
        kind_token.and_then(|(kind, token)| {
            Event::signal(endpoint.handle, kind)
                .ok()
                .map(|event| Pending { event, token })
        })
    }
    pub fn peek(&self, owner: ProcessKey) -> Result<Option<Pending>, Error> {
        if !Self::valid(owner) {
            return Err(Error::Denied);
        }
        if self.turn == u64::MAX {
            return Err(Error::Sequence);
        }
        let mut selected = None;
        for channel in self.slots.iter().flatten() {
            for side in 0..2 {
                let endpoint = &channel.ends[side];
                if endpoint.owner != Some(owner) {
                    continue;
                }
                if let Some(pending) = Self::pending(channel, side) {
                    if selected
                        .as_ref()
                        .map(|(turn, _)| endpoint.served < *turn)
                        .unwrap_or(true)
                    {
                        selected = Some((endpoint.served, pending));
                    }
                }
            }
        }
        Ok(selected.map(|(_, event)| event))
    }
    pub fn consume(&mut self, owner: ProcessKey, pending: Pending) -> Result<(), Error> {
        let (index, side) = self.locate(owner, pending.event.endpoint)?;
        let channel = self.slots[index].as_mut().ok_or(Error::Invalid)?;
        if Self::pending(channel, side) != Some(pending) {
            return Err(Error::Invalid);
        }
        let turn = self.turn.checked_add(1).ok_or(Error::Sequence)?;
        let endpoint = &mut channel.ends[side];
        match pending.event.kind {
            Kind::Message => {
                endpoint.inbox.pop_front();
            }
            Kind::PeerClosed => {
                endpoint.closed_reported = true;
                endpoint.writable = None;
            }
            Kind::Writable => {
                endpoint.writable = None;
            }
        }
        endpoint.served = turn;
        self.turn = turn;
        Ok(())
    }
    pub fn close(&mut self, owner: ProcessKey, handle: Handle) -> Result<(), Error> {
        let (index, side) = self.locate(owner, handle)?;
        self.close_at(index, side);
        Ok(())
    }
    fn close_at(&mut self, index: usize, side: usize) {
        if let Some(channel) = self.slots[index].as_mut() {
            let endpoint = &mut channel.ends[side];
            endpoint.owner = None;
            endpoint.tag = None;
            // The dead owner must not retain its bounded inbox allocation until
            // an unrelated peer closes. The peer's own queue remains intact so
            // its messages precede the durable PeerClosed notification.
            endpoint.inbox = VecDeque::new();
            endpoint.writable = None;
            channel.ends[1 - side].closed_reported = false;
            if channel.ends.iter().all(|e| e.owner.is_none()) {
                self.slots[index] = None;
            }
        }
    }
    /// Exit/kill revocation is infallible and requires no allocation or new IDs.
    pub fn close_owner(&mut self, owner: ProcessKey) {
        for index in 0..self.slots.len() {
            for side in 0..2 {
                if self.slots[index]
                    .as_ref()
                    .is_some_and(|c| c.ends[side].owner == Some(owner))
                {
                    self.close_at(index, side);
                }
            }
        }
    }
    pub fn ready(&self, owner: ProcessKey) -> bool {
        self.peek(owner).ok().flatten().is_some()
    }
    /// Select receive readiness independently from one-shot write readiness.
    pub fn set_interest(
        &mut self,
        owner: ProcessKey,
        handle: Handle,
        readable: bool,
        writable: bool,
    ) -> Result<(), Error> {
        let (index, side) = self.locate(owner, handle)?;
        self.interest(owner, handle, writable)?;
        self.slots[index].as_mut().ok_or(Error::Invalid)?.ends[side].readable = readable;
        Ok(())
    }
    pub fn interest(
        &mut self,
        owner: ProcessKey,
        handle: Handle,
        writable: bool,
    ) -> Result<(), Error> {
        let (index, side) = self.locate(owner, handle)?;
        let endpoint = &mut self.slots[index].as_mut().ok_or(Error::Invalid)?.ends[side];
        if writable && !endpoint.rights.contains(Rights::SEND) {
            return Err(Error::Denied);
        }
        if writable && endpoint.writable.is_none() {
            let next = self
                .next_id
                .checked_add(1)
                .filter(|&id| id <= abi::MAX_ID)
                .ok_or(Error::Sequence)?;
            endpoint.writable = Some(self.next_id);
            self.next_id = next;
        } else if !writable {
            endpoint.writable = None;
        }
        Ok(())
    }
    /// Validate the complete binding set, then publish every move together.
    /// Kernel callers bind only an unpublished child under the same scheduler lock.
    pub fn bindings(
        &mut self,
        parent: ProcessKey,
        child: ProcessKey,
        bindings: &[Binding],
    ) -> Result<(), Error> {
        let locations = self.binding_locations(parent, child, bindings)?;
        for (binding, (index, side)) in bindings.iter().zip(locations) {
            let endpoint = &mut self.slots[index].as_mut().ok_or(Error::Invalid)?.ends[side];
            endpoint.owner = Some(child);
            endpoint.rights = binding.rights;
            endpoint.tag = Some(binding.tag);
            endpoint.writable = None;
            endpoint.readable = true;
            if !endpoint.rights.contains(Rights::RECEIVE) {
                endpoint.inbox.clear();
            }
        }
        Ok(())
    }
    /// Advisory validation before resource construction; bindings rechecks at commit.
    pub fn validate_bindings(
        &self,
        parent: ProcessKey,
        child: ProcessKey,
        bindings: &[Binding],
    ) -> Result<(), Error> {
        self.binding_locations(parent, child, bindings).map(|_| ())
    }
    fn binding_locations(
        &self,
        parent: ProcessKey,
        child: ProcessKey,
        bindings: &[Binding],
    ) -> Result<[(usize, usize); abi::MAX_BINDINGS], Error> {
        if !Self::valid(parent) || !Self::valid(child) || parent == child {
            return Err(Error::Denied);
        }
        if bindings.len() > abi::MAX_BINDINGS
            || self.handles(child) + bindings.len() > abi::MAX_HANDLES
        {
            return Err(Error::Limit);
        }
        let mut locations = [(0, 0); abi::MAX_BINDINGS];
        for (n, binding) in bindings.iter().enumerate() {
            if binding.tag == 0
                || bindings[..n]
                    .iter()
                    .any(|other| other.tag == binding.tag || other.source == binding.source)
                || self.bootstrap(child, binding.tag).is_ok()
            {
                return Err(Error::Invalid);
            }
            let location = self.locate(parent, binding.source)?;
            let endpoint = &self.slots[location.0].as_ref().ok_or(Error::Invalid)?.ends[location.1];
            if !endpoint.rights.contains(Rights::TRANSFER)
                || !endpoint.rights.contains(binding.rights)
            {
                return Err(Error::Denied);
            }
            locations[n] = location;
        }
        Ok(locations)
    }
    /// Take one bootstrap grant. Repeating a lookup must not mint SDK ownership aliases.
    pub fn take_bootstrap(&mut self, owner: ProcessKey, tag: u32) -> Result<Handle, Error> {
        let handle = self.bootstrap(owner, tag)?;
        let (index, side) = self.locate(owner, handle)?;
        self.slots[index].as_mut().ok_or(Error::Invalid)?.ends[side].tag = None;
        Ok(handle)
    }
    pub fn bootstrap(&self, owner: ProcessKey, tag: u32) -> Result<Handle, Error> {
        if tag == 0 || !Self::valid(owner) {
            return Err(Error::Invalid);
        }
        self.slots
            .iter()
            .flatten()
            .flat_map(|c| c.ends.iter())
            .find(|e| e.owner == Some(owner) && e.tag == Some(tag))
            .map(|e| e.handle)
            .ok_or(Error::Denied)
    }
    /// Snapshot live objects owned by an authenticated process. Queue capacity
    /// remains charged even when rights attenuation discards its messages.
    pub fn owner_usage(&self, owner: ProcessKey) -> Result<Storage, Error> {
        if !Self::valid(owner) {
            return Err(Error::Denied);
        }
        let mut result = Storage::default();
        for endpoint in self
            .slots
            .iter()
            .flatten()
            .flat_map(|channel| channel.ends.iter())
        {
            if endpoint.owner == Some(owner) {
                result.handles += 1;
                result.inbox_bytes += endpoint.inbox.capacity() * core::mem::size_of::<Pending>();
            }
        }
        Ok(result)
    }
    /// Validate the whole proposed move and snapshot its existing reservations;
    /// no endpoint, ownership or queue changes until bindings commits.
    pub fn binding_usage(
        &self,
        parent: ProcessKey,
        child: ProcessKey,
        bindings: &[Binding],
    ) -> Result<Storage, Error> {
        let locations = self.binding_locations(parent, child, bindings)?;
        let mut result = Storage::default();
        for &(index, side) in &locations[..bindings.len()] {
            let endpoint = &self.slots[index].as_ref().ok_or(Error::Invalid)?.ends[side];
            result.handles += 1;
            result.inbox_bytes += endpoint.inbox.capacity() * core::mem::size_of::<Pending>();
        }
        Ok(result)
    }
    pub fn channels(&self) -> usize {
        self.slots.iter().filter(|slot| slot.is_some()).count()
    }
    pub fn handles(&self, owner: ProcessKey) -> usize {
        self.slots
            .iter()
            .flatten()
            .flat_map(|c| c.ends.iter())
            .filter(|e| e.owner == Some(owner))
            .count()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn fairness_exhaustion_is_reported_before_delivery_and_does_not_wrap() {
        let key = ProcessKey {
            pid: 1,
            generation: 1,
        };
        let mut registry = Registry::new();
        let [a, _] = registry.pair(key).unwrap();
        registry.send(key, a, b"x").unwrap();
        registry.turn = abi::MAX_ID;
        let pending = registry.peek(key).unwrap().unwrap();
        registry.consume(key, pending).unwrap();
        registry.send(key, a, b"next").unwrap();
        registry.turn = u64::MAX;
        assert_eq!(registry.peek(key), Err(Error::Sequence));
        registry.close_owner(key);
        assert_eq!(registry.channels(), 0);
    }
    #[test]
    fn opaque_handle_ids_never_reach_syscall_control_sentinels() {
        let key = ProcessKey {
            pid: 1,
            generation: 1,
        };
        let mut registry = Registry::new();
        registry.next_id = u64::MAX - 3;
        assert_eq!(registry.pair(key), Err(Error::Sequence));
        assert_eq!(registry.channels(), 0);
    }
    #[test]
    fn sequence_exhaustion_never_publishes_partial_objects_or_prevents_cleanup() {
        let key = ProcessKey {
            pid: 1,
            generation: 1,
        };
        let mut registry = Registry::new();
        let [a, _] = registry.pair(key).unwrap();
        registry.next_id = u64::MAX;
        assert_eq!(registry.pair(key), Err(Error::Sequence));
        assert_eq!(registry.channels(), 1);
        assert_eq!(registry.handles(key), 2);
        assert_eq!(registry.send(key, a, b"x"), Err(Error::Sequence));
        assert_eq!(registry.interest(key, a, true), Err(Error::Sequence));
        assert_eq!(registry.peek(key), Ok(None));
        registry.close_owner(key);
        assert_eq!(registry.channels(), 0);
    }
}
