//! Per-connection authorization; only the authenticated transport supplies identity.
use crate::{Invalid, ProcessKey};
use rustos_service_abi::control::{Access, Operation, Request};
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Denied {
    Peer,
    Run,
    Replay,
    Malformed,
    Permission,
}
pub struct Session {
    peer: ProcessKey,
    run: u64,
    access: Access,
    last: u64,
}
impl Session {
    pub fn new(peer: ProcessKey, run: u64, access: Access) -> Result<Self, Invalid> {
        if peer.generation == 0 || peer.pid == u32::MAX || run == 0 {
            return Err(Invalid);
        }
        Ok(Self {
            peer,
            run,
            access,
            last: 0,
        })
    }
    /// Validate provenance and syntax before consuming an ID. A well-formed
    /// denied operation does consume its ID; retries need a fresh request. This
    /// prevents reinterpreting the same request ID as a different operation.
    /// Authorization must precede catalog lookup, job mutation or journal access.
    pub fn admit(
        &mut self,
        sender: ProcessKey,
        run: u64,
        request: Request<'_>,
    ) -> Result<(), Denied> {
        if sender != self.peer {
            return Err(Denied::Peer);
        }
        if run != self.run {
            return Err(Denied::Run);
        }
        request.validate().map_err(|_| Denied::Malformed)?;
        if request.id <= self.last {
            return Err(Denied::Replay);
        }
        self.last = request.id;
        if self.access == Access::ReadOnly
            && !matches!(
                request.operation,
                Operation::Status { .. } | Operation::Logs { .. }
            )
        {
            return Err(Denied::Permission);
        }
        Ok(())
    }
}
