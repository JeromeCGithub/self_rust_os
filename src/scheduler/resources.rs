//! Scope resource reservations that can outlive a process or kernel entry gate.
//! SCHEDULER owns records/counters. Destructors only publish a unique release key
//! into a bounded crossbeam queue; they never lock or allocate. Its publication
//! synchronizes resource destruction with the collecting scheduler entry. A queued
//! key's record cannot be reused until collection, so a capacity-sized queue always
//! has room for every outstanding destructor. No hard IRQ creates or drops these leases.
use super::{ProcessState, Scheduler, SCHEDULER};
use alloc::{sync::Arc, vec::Vec};
use crossbeam_queue::ArrayQueue;
use rustos_scope_core::{Charge, Error, Registry, Usage, SYSTEM};
const MAX_LEASES: usize = 8192;
const RECOVERY_LEASES: usize = 64;
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Key {
    slot: u16,
    serial: u64,
}
/// Native reservation retained until the resource's final owner releases it.
/// Destroy owned buffers before this value (declare it first, or store it in the
/// last field). Dropping only queues release; scheduler collection returns quota.
/// A lease may outlive its process. It cannot be cloned or supplied by userspace.
#[must_use = "retain the lease until the actual resource is destroyed"]
pub struct ResourceLease {
    release: Option<(Key, Arc<ArrayQueue<Key>>)>,
}
impl core::fmt::Debug for ResourceLease {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        // Fault diagnostics must not dump the potentially full release queue.
        f.debug_struct("ResourceLease")
            .field("key", &self.release.as_ref().map(|(key, _)| key))
            .finish()
    }
}
impl ResourceLease {
    fn unscoped() -> Self {
        Self { release: None }
    }
}
impl Drop for ResourceLease {
    fn drop(&mut self) {
        if let Some((key, queue)) = self.release.take() {
            queue
                .push(key)
                .expect("each queued release owns an unreused record");
        }
    }
}
struct Record {
    serial: u64,
    charge: Charge,
}
pub(super) struct Pool {
    records: Vec<Option<Record>>,
    free: Vec<u16>,
    recovery: Vec<u16>,
    releases: Option<Arc<ArrayQueue<Key>>>,
    reserved_start: usize,
    next: u64,
}
impl Pool {
    pub const fn new() -> Self {
        Self {
            records: Vec::new(),
            free: Vec::new(),
            recovery: Vec::new(),
            releases: None,
            reserved_start: 0,
            next: 1,
        }
    }
    /// Boot-only bounded infrastructure. The final queue/Arc allocation is a
    /// fatal heap prerequisite, like the existing input/executor boot queues.
    fn initialize(&mut self, capacity: usize, recovery: usize) -> Result<(), &'static str> {
        if self.releases.is_some()
            || capacity == 0
            || capacity > u16::MAX as usize
            || recovery >= capacity
        {
            return Err("invalid resource lease pool initialization");
        }
        let mut records = Vec::new();
        let mut free = Vec::new();
        let mut reserved = Vec::new();
        records
            .try_reserve_exact(capacity)
            .map_err(|_| "lease records allocation failed")?;
        free.try_reserve_exact(capacity - recovery)
            .map_err(|_| "lease free list allocation failed")?;
        reserved
            .try_reserve_exact(recovery)
            .map_err(|_| "lease recovery list allocation failed")?;
        records.resize_with(capacity, || None);
        for slot in 0..capacity {
            if slot < capacity - recovery {
                free.push(slot as u16);
            } else {
                reserved.push(slot as u16);
            }
        }
        *self = Self {
            records,
            free,
            recovery: reserved,
            reserved_start: capacity - recovery,
            releases: Some(Arc::new(ArrayQueue::new(capacity))),
            next: 1,
        };
        Ok(())
    }
    pub(super) fn initialize_native(&mut self) -> Result<(), &'static str> {
        self.initialize(MAX_LEASES, RECOVERY_LEASES)
    }
    fn reserve(
        &mut self,
        scopes: &mut Registry,
        scope: u64,
        used: Usage,
    ) -> Result<ResourceLease, Error> {
        let queue = self.releases.as_ref().ok_or(Error::Missing)?;
        if used == Usage::ZERO {
            scopes.available(scope)?;
            return Ok(ResourceLease::unscoped());
        }
        let next = self.next.checked_add(1).ok_or(Error::Sequence)?;
        let list = if self.free.is_empty() && scope == SYSTEM {
            &mut self.recovery
        } else {
            &mut self.free
        };
        let slot = *list.last().ok_or(Error::Limit)?;
        let charge = scopes.reserve(scope, used)?;
        list.pop();
        self.records[slot as usize] = Some(Record {
            serial: self.next,
            charge,
        });
        let lease = ResourceLease {
            release: Some((
                Key {
                    slot,
                    serial: self.next,
                },
                Arc::clone(queue),
            )),
        };
        self.next = next;
        Ok(lease)
    }
    fn grow(
        &mut self,
        scopes: &mut Registry,
        key: Key,
        scope: u64,
        used: Usage,
    ) -> Result<(), Error> {
        let record = self
            .records
            .get_mut(key.slot as usize)
            .and_then(Option::as_mut)
            .filter(|record| record.serial == key.serial)
            .ok_or(Error::Missing)?;
        if record.charge.scope() != scope {
            return Err(Error::Denied);
        }
        scopes.grow(&mut record.charge, used)
    }
    pub(super) fn collect(&mut self, scopes: &mut Registry) -> usize {
        let Some(queue) = self.releases.as_ref() else {
            return 0;
        };
        let mut count = 0;
        while let Some(key) = queue.pop() {
            let record = self.records[key.slot as usize]
                .take()
                .expect("release owns a record");
            assert_eq!(
                record.serial, key.serial,
                "release key cannot alias a reused record"
            );
            if (key.slot as usize) < self.reserved_start {
                self.free.push(key.slot);
            } else {
                self.recovery.push(key.slot);
            }
            scopes
                .release(record.charge)
                .expect("released resource retains exact scope charge");
            count += 1;
        }
        count
    }
}
/// Reserve before allocating a persistent object or temporary syscall buffer.
/// Caller identity is derived from the live context; no PID/scope selector exists.
/// This takes SCHEDULER briefly: call before taking any filesystem/service lock.
/// Legacy boot returns an explicitly unscoped lease. Native limits are enforced.
pub fn reserve_resource(used: Usage) -> Result<ResourceLease, Error> {
    let mut s = SCHEDULER.lock();
    let pid = s.current_pid().ok_or(Error::Denied)?;
    let scope = s
        .processes
        .get(pid as usize)
        .and_then(Option::as_ref)
        .filter(|p| p.state == ProcessState::Live)
        .ok_or(Error::Denied)?
        .principal
        .scope;
    if s.init_pid.is_none() {
        return Ok(ResourceLease::unscoped());
    }
    let Scheduler {
        resources, scopes, ..
    } = &mut *s;
    resources.collect(scopes);
    resources.reserve(scopes, scope, used)
}
/// Charge additional actual allocation capacity before publishing/using a buffer.
/// As with reserve_resource, never call this while holding a service lock.
pub fn grow_resource(lease: &ResourceLease, used: Usage) -> Result<(), Error> {
    let mut s = SCHEDULER.lock();
    let pid = s.current_pid().ok_or(Error::Denied)?;
    let scope = s
        .processes
        .get(pid as usize)
        .and_then(Option::as_ref)
        .filter(|p| p.state == ProcessState::Live)
        .ok_or(Error::Denied)?
        .principal
        .scope;
    let Some((key, queue)) = lease.release.as_ref() else {
        return if s.init_pid.is_none() || used == Usage::ZERO {
            Ok(())
        } else {
            Err(Error::Invalid)
        };
    };
    let Scheduler {
        resources, scopes, ..
    } = &mut *s;
    if !resources
        .releases
        .as_ref()
        .is_some_and(|owned| Arc::ptr_eq(queue, owned))
    {
        return Err(Error::Denied);
    }
    resources.grow(scopes, *key, scope, used)
}
#[cfg(test)]
mod tests {
    use super::*;
    use rustos_scope_core::{ProcessKey, SYSTEM};
    fn usage(n: u64) -> Usage {
        Usage {
            kernel_bytes: n,
            descriptors: n,
            ..Usage::ZERO
        }
    }
    fn registry() -> (Registry, u64) {
        let mut scopes = Registry::new();
        let init = ProcessKey {
            pid: 1,
            generation: 1,
        };
        scopes.bootstrap(init, usage(100), usage(10)).unwrap();
        let scope = scopes.create_service(init, SYSTEM, usage(90)).unwrap();
        (scopes, scope)
    }
    #[test_case]
    fn release_queue_retains_quota_until_collection_and_preserves_recovery_records() {
        let (mut scopes, scope) = registry();
        let mut pool = Pool::new();
        pool.initialize(4, 1).unwrap();
        for _ in 0..16 {
            let a = pool.reserve(&mut scopes, scope, usage(1)).unwrap();
            let b = pool.reserve(&mut scopes, scope, usage(1)).unwrap();
            let c = pool.reserve(&mut scopes, scope, usage(1)).unwrap();
            assert!(matches!(
                pool.reserve(&mut scopes, scope, usage(1)),
                Err(Error::Limit)
            ));
            let heap = crate::allocator::allocated_bytes();
            {
                // Files.clear can drop the final Arc while SCHEDULER is held;
                // releasing a lease here must neither relock nor allocate.
                let _scheduler = SCHEDULER.lock();
                drop(a);
                drop(b);
                drop(c);
            }
            assert_eq!(crate::allocator::allocated_bytes(), heap);
            assert_eq!(
                scopes
                    .status(
                        ProcessKey {
                            pid: 1,
                            generation: 1
                        },
                        scope
                    )
                    .unwrap()
                    .used,
                usage(3)
            );
            let recovery = pool.reserve(&mut scopes, SYSTEM, usage(1)).unwrap();
            assert!(matches!(
                pool.reserve(&mut scopes, SYSTEM, usage(1)),
                Err(Error::Limit)
            ));
            drop(recovery); // Every record can be queued without queue overflow.
            assert_eq!(pool.collect(&mut scopes), 4);
            assert_eq!(
                scopes
                    .status(
                        ProcessKey {
                            pid: 1,
                            generation: 1
                        },
                        SYSTEM
                    )
                    .unwrap()
                    .used,
                Usage::ZERO
            );
        }
    }
    #[test_case]
    fn lease_growth_and_identity_reuse_fail_atomically_and_sequence_exhaustion_keeps_cleanup() {
        let (mut scopes, scope) = registry();
        let mut pool = Pool::new();
        pool.initialize(2, 1).unwrap();
        let lease = pool.reserve(&mut scopes, scope, usage(20)).unwrap();
        let old = lease.release.as_ref().unwrap().0;
        assert_eq!(
            pool.grow(&mut scopes, old, SYSTEM, usage(1)),
            Err(Error::Denied)
        );
        assert_eq!(
            pool.grow(&mut scopes, old, scope, usage(71)),
            Err(Error::Limit)
        );
        pool.grow(&mut scopes, old, scope, usage(10)).unwrap();
        drop(lease);
        assert_eq!(pool.collect(&mut scopes), 1);
        let lease = pool.reserve(&mut scopes, scope, usage(1)).unwrap();
        assert_eq!(
            pool.grow(&mut scopes, old, scope, usage(1)),
            Err(Error::Missing)
        );
        pool.next = u64::MAX;
        assert!(matches!(
            pool.reserve(&mut scopes, SYSTEM, usage(1)),
            Err(Error::Sequence)
        ));
        drop(lease);
        assert_eq!(pool.collect(&mut scopes), 1);
        assert_eq!(
            scopes
                .status(
                    ProcessKey {
                        pid: 1,
                        generation: 1
                    },
                    SYSTEM
                )
                .unwrap()
                .used,
            Usage::ZERO
        );
    }
    #[test_case]
    fn zero_reservations_do_not_consume_pool_records_or_admit_closed_scopes() {
        let (mut scopes, scope) = registry();
        let mut pool = Pool::new();
        assert!(pool.initialize(0, 0).is_err());
        pool.initialize(1, 0).unwrap();
        assert!(pool.initialize(1, 0).is_err());
        let empty = pool.reserve(&mut scopes, scope, Usage::ZERO).unwrap();
        let actual = pool.reserve(&mut scopes, scope, usage(1)).unwrap();
        assert!(matches!(
            pool.reserve(&mut scopes, scope, usage(1)),
            Err(Error::Limit)
        ));
        drop(empty);
        drop(actual);
        assert_eq!(pool.collect(&mut scopes), 1);
        scopes
            .close(
                ProcessKey {
                    pid: 1,
                    generation: 1,
                },
                scope,
            )
            .unwrap();
        assert!(matches!(
            pool.reserve(&mut scopes, scope, Usage::ZERO),
            Err(Error::Closed)
        ));
    }
}
