//! Per-connection authorization; only the authenticated transport supplies identity.
use crate::{Invalid, ProcessKey};
use rustos_service_abi::control::{Access, Operation, Request};
/// Global stop budget for native control requests, in the manager's clock ticks.
/// Direct host/native adapters may choose a different bounded budget explicitly.
pub const SHUTDOWN_TIMEOUT_TICKS: u64 = 180;
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
                Operation::Status { .. } | Operation::Logs { .. } | Operation::Job { .. }
            )
        {
            return Err(Denied::Permission);
        }
        Ok(())
    }
}

impl Session {
    /// One bounded request. Authorization precedes catalog lookup and mutation.
    /// Encode the borrowed snapshot before advancing the manager or journal.
    pub fn dispatch<'a, const N: usize>(
        &mut self,
        sender: ProcessKey,
        run: u64,
        request: Request<'_>,
        manager: &'a mut crate::manager::Manager,
        journal: &'a crate::journal::Journal<N>,
    ) -> rustos_service_abi::replies::Reply<'a> {
        use rustos_service_abi::replies::{Body, Code, Phase, Reply, LOG_CHUNK};
        let error = |code, detail| Reply {
            id: request.id,
            body: Body::Error { code, detail },
        };
        if let Err(denied) = self.admit(sender, run, request) {
            return error(
                match denied {
                    Denied::Permission | Denied::Peer | Denied::Run => Code::Denied,
                    Denied::Replay => Code::Stale,
                    Denied::Malformed => Code::Invalid,
                },
                0,
            );
        }
        let body = match request.operation {
            Operation::Start(name) | Operation::Stop(name) | Operation::Restart(name) => {
                let result = match request.operation {
                    Operation::Start(_) => manager.start(name),
                    Operation::Stop(_) => manager.stop(name),
                    _ => manager.restart(name),
                };
                match result {
                    Ok(job) => Body::Job {
                        job,
                        phase: job_phase(manager.outcome()),
                    },
                    Err(e) => {
                        return error(
                            match e {
                                crate::transaction::Error::Busy => Code::Busy,
                                crate::transaction::Error::Unknown => Code::Unknown,
                                crate::transaction::Error::InvalidResult => Code::Unsupported,
                                _ => Code::Limit,
                            },
                            0,
                        )
                    }
                }
            }
            Operation::Job { job } => {
                if manager.job_id() != Some(job) {
                    return error(Code::Stale, 0);
                }
                Body::Job {
                    job,
                    phase: job_phase(manager.outcome()),
                }
            }
            Operation::Status { index } => {
                let Some(unit) = manager.catalog().unit(index as usize) else {
                    return Reply {
                        id: request.id,
                        body: Body::End,
                    };
                };
                let status = manager.run_status(index as usize);
                let phase = match manager.state(index as usize) {
                    Some(crate::State::Spawning(_)) => Phase::Spawning,
                    Some(crate::State::Starting(_)) => Phase::Starting,
                    Some(crate::State::Running(_)) => Phase::Running,
                    Some(crate::State::Stopping(_)) => Phase::Stopping,
                    Some(crate::State::Backoff(_)) => Phase::Backoff,
                    Some(crate::State::Failed) => Phase::Failed,
                    None if manager.active(index as usize) => Phase::GroupActive,
                    _ => Phase::Inactive,
                };
                let process = status.and_then(|s| s.process).map(|p| rustos_ipc_key(p));
                let deadline = match manager.state(index as usize) {
                    Some(crate::State::Backoff(d)) => Some(d),
                    _ => status.and_then(|s| {
                        s.start_deadline
                            .into_iter()
                            .chain(s.watchdog_deadline)
                            .chain(s.stop_deadline)
                            .min()
                    }),
                };
                Body::Status {
                    index,
                    name: &unit.name,
                    phase,
                    run: status.map(|s| s.run).unwrap_or(0),
                    process,
                    deadline,
                }
            }
            Operation::Logs { cursor, offset } => {
                if offset != 0 && cursor == 0 {
                    return error(Code::Invalid, 0);
                }
                let result = journal.after(if offset == 0 { cursor } else { cursor - 1 });
                let record = match result {
                    Ok(Some(record)) => record,
                    Ok(None) if offset == 0 => {
                        return Reply {
                            id: request.id,
                            body: Body::End,
                        }
                    }
                    Ok(None) => return error(Code::Stale, 0),
                    Err(crate::journal::Error::Gap { oldest }) => return error(Code::Gap, oldest),
                    Err(_) => return error(Code::Invalid, 0),
                };
                if offset != 0 && record.sequence != cursor {
                    return error(Code::Gap, record.sequence);
                }
                let text = record.text();
                let start = offset as usize;
                if start > text.len()
                    || !text.is_char_boundary(start)
                    || start == text.len() && start != 0
                {
                    return error(Code::Invalid, 0);
                }
                let mut end = (start + LOG_CHUNK).min(text.len());
                while !text.is_char_boundary(end) {
                    end -= 1;
                }
                Body::Log {
                    sequence: record.sequence,
                    unit: record.unit as u32,
                    run: record.run,
                    process: rustos_ipc_key(record.process),
                    tick: record.tick,
                    level: record.level,
                    offset,
                    total: text.len() as u16,
                    text: &text[start..end],
                }
            }
            Operation::Shutdown => match manager.shutdown(SHUTDOWN_TIMEOUT_TICKS) {
                Ok(job) => Body::Job {
                    job,
                    phase: job_phase(manager.outcome()),
                },
                Err(_) => return error(Code::Limit, 0),
            },
        };
        Reply {
            id: request.id,
            body,
        }
    }
}
fn job_phase(outcome: crate::transaction::Outcome) -> rustos_service_abi::replies::JobPhase {
    use crate::transaction::Outcome;
    use rustos_service_abi::replies::JobPhase;
    match outcome {
        Outcome::Running => JobPhase::Running,
        Outcome::Succeeded => JobPhase::Succeeded,
        Outcome::Degraded => JobPhase::Degraded,
        Outcome::Failed => JobPhase::Failed,
    }
}
fn rustos_ipc_key(key: ProcessKey) -> rustos_service_abi::PeerKey {
    rustos_service_abi::PeerKey {
        pid: key.pid,
        generation: key.generation,
    }
}
