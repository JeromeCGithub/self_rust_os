//! Roll back resource reservations and unpublished leader binding on early return.
use crate::{Charge, Error, ProcessKey, Registry, Usage};
/// One unpublished allocation/publication transaction. Destroy any real resources
/// before dropping this guard; commit transfers the charge to their native owner.
/// The exclusive registry borrow prevents concurrent closure/publication.
#[must_use = "commit the charge with its resources or roll back both"]
pub struct Reservation<'a> {
    registry: &'a mut Registry,
    charge: Option<Charge>,
    leader: Option<ProcessKey>,
}
impl core::fmt::Debug for Reservation<'_> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("Reservation")
            .field("charge", &self.charge)
            .field("leader", &self.leader)
            .finish_non_exhaustive()
    }
}
impl Reservation<'_> {
    /// Remaining room after this transaction's reservations and ancestor pressure.
    pub fn available(&self) -> Result<Usage, Error> {
        self.registry.available(
            self.charge
                .as_ref()
                .expect("unconsumed reservation owns its charge")
                .scope,
        )
    }
    /// Reserve an additional delta; failed growth leaves all counts unchanged.
    pub fn grow(&mut self, used: Usage) -> Result<(), Error> {
        self.registry.grow(
            self.charge
                .as_mut()
                .expect("unconsumed reservation owns its charge"),
            used,
        )
    }
    /// Bind the actual new process before a final fallible endpoint-grant commit.
    /// Failure/drop restores the unbound scope; success retains the appointment.
    pub fn appoint_leader(&mut self, caller: ProcessKey, leader: ProcessKey) -> Result<(), Error> {
        let scope = self
            .charge
            .as_ref()
            .expect("unconsumed reservation owns its charge")
            .scope;
        self.registry.appoint_leader(caller, scope, leader)?;
        self.leader = Some(leader);
        Ok(())
    }
    /// Validate admission of existing resources from a native parent before
    /// committing its endpoint moves under the same external resource lock.
    pub fn can_receive(&self, source: &Charge, used: Usage) -> Result<(), Error> {
        self.registry
            .can_transfer(source, self.charge.as_ref().expect("pending charge"), used)
    }
    /// Finalize a previously validated native object handoff. On error source
    /// stays intact and the reservation is returned: destroy real construction
    /// resources before dropping that guard to release quota/leader appointment.
    /// Serialize object/charge publication and avoid fallible work after moving
    /// objects. A failed earlier object move simply drops the original guard
    /// after construction rollback.
    pub fn commit_receiving(
        mut self,
        source: &mut Charge,
        used: Usage,
    ) -> Result<Charge, (Error, Self)> {
        if let Err(error) =
            self.registry
                .transfer(source, self.charge.as_mut().expect("pending charge"), used)
        {
            return Err((error, self));
        }
        Ok(self.charge.take().expect("reservation commits once"))
    }
    /// Retain usage and any leader binding; the resource owner must later release
    /// this charge only after destroying all corresponding resources.
    pub fn commit(mut self) -> Charge {
        self.charge
            .take()
            .expect("reservation commits its unique charge once")
    }
}
impl Drop for Reservation<'_> {
    fn drop(&mut self) {
        if let Some(charge) = self.charge.take() {
            if let Some(leader) = self.leader {
                // The guard holds the only mutable registry reference and offers
                // no operation that can remove or replace this appointment.
                let index = self
                    .registry
                    .index(charge.scope)
                    .expect("reserved scope exists");
                let node = self.registry.slots[index]
                    .as_mut()
                    .expect("reserved scope exists");
                assert_eq!(
                    node.leader,
                    Some(leader),
                    "unpublished leader binding belongs to guard"
                );
                node.leader = None;
            }
            // Every reserved delta is still represented in all ancestors: the
            // guard cannot mutate ownership or release counts behind its charge.
            self.registry
                .release(charge)
                .expect("reservation rollback retains its exact charge");
        }
    }
}
/// An allocation delta against an existing process/resource owner's charge.
/// Roll back real allocations before drop; commit retains the additional charge.
#[must_use = "commit growth only after publishing owned resources"]
pub struct Growth<'a> {
    registry: &'a mut Registry,
    charge: &'a mut Charge,
    rollback: Usage,
}
impl Growth<'_> {
    /// Remaining room after this transaction's reservations and ancestor pressure.
    pub fn available(&self) -> Result<Usage, Error> {
        self.registry.available(self.charge.scope)
    }
    /// Reserve an additional delta; failed growth leaves all counts unchanged.
    pub fn grow(&mut self, used: Usage) -> Result<(), Error> {
        let rollback = self.rollback.add(used)?;
        self.registry.grow(self.charge, used)?;
        self.rollback = rollback;
        Ok(())
    }
    /// Keep the successful deltas on the original owner's charge.
    pub fn commit(mut self) {
        self.rollback = Usage::ZERO;
    }
}
impl Drop for Growth<'_> {
    fn drop(&mut self) {
        // The exclusive charge/registry borrows prevent outside release. Only
        // this guard's successful deltas are subtracted, preserving older owners.
        self.registry
            .release_part(self.charge, self.rollback)
            .expect("growth rollback retains its exact outstanding delta");
    }
}
impl Registry {
    /// Reserve before fallible construction; errors/drop roll back bookkeeping.
    /// Physical resource rollback must precede dropping the returned guard.
    pub fn transaction(&mut self, scope: u64, used: Usage) -> Result<Reservation<'_>, Error> {
        let charge = self.reserve(scope, used)?;
        Ok(Reservation {
            registry: self,
            charge: Some(charge),
            leader: None,
        })
    }
    /// Reserve a delta before heap/thread/resource growth; old usage survives drop.
    pub fn growth<'a>(
        &'a mut self,
        charge: &'a mut Charge,
        used: Usage,
    ) -> Result<Growth<'a>, Error> {
        self.grow(charge, used)?;
        Ok(Growth {
            registry: self,
            charge,
            rollback: used,
        })
    }
}
