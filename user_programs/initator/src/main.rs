// Initator M1: foreground native services, durable child events, bounded retries.
use rustos_service_core::{Completion, Policy, ProcessKey, Restart, State, Supervisor};
use userlib::{recordln as println, supervision, sys};
fn main() {
    let identity = supervision::current().expect("native process identity");
    assert_eq!(
        identity.pid, 1,
        "Initator must be the first userspace process"
    );
    println!("[initator] PID 1 entered Ring 3");
    let mut mode = [0; 32];
    let length = sys::read_file("/ETC/INIT.RUN", &mut mode).unwrap_or(0);
    let orphan_test = &mode[..length] == b"orphans";
    let fault_test = &mode[..length] == b"fault-service";
    let testing = orphan_test || fault_test || &mode[..length] == b"two-services";
    let policy = Policy {
        restart: Restart::OnFailure,
        delay: 2,
        maximum_delay: 8,
        burst: 3,
        window: 100,
        healthy_reset: 50,
    };
    let mut manager = Supervisor::new([policy; 2]).expect("compiled service definitions");
    loop {
        // Drain completions before admitting replacement starts. A bounded batch
        // prevents one source monopolizing the manager; pending readiness is durable.
        for _ in 0..128 {
            match supervision::next_child().expect("owned child event") {
                None => break,
                Some(supervision::ChildEvent::Adopted(key)) => {
                    let _ = supervision::terminate(key);
                }
                Some(supervision::ChildEvent::Completed {
                    process,
                    cause,
                    status,
                }) => {
                    println!(
                        "[initator] completion pid={} cause={:?} status={} tick={}",
                        process.pid, cause, status, sys::ticks()
                    );
                    let cause = match cause {
                        supervision::Cause::Exited => Completion::Exit(status),
                        supervision::Cause::Killed => Completion::Killed,
                        supervision::Cause::Fault => Completion::Fault(status as u16),
                    };
                    manager.completed(
                        ProcessKey {
                            pid: process.pid,
                            generation: process.generation,
                        },
                        cause,
                        sys::ticks(),
                    );
                }
            }
        }
        while let Some(start) = manager.next_start(sys::ticks()) {
            let path = if start.unit == 0 {
                "/BIN/GLOW.SVC"
            } else {
                "/BIN/FLICKER.SVC"
            };
            let args: &[&str] = if orphan_test && start.unit == 1 {
                &["--orphans"]
            } else if fault_test && start.unit == 1 {
                &["--fault"]
            } else if testing && start.unit == 1 {
                &["--fail"]
            } else {
                &[]
            };
            match sys::spawn_with_env(path, args, &[]) {
                Ok(pid) => {
                    let key = ProcessKey { pid, generation: 1 };
                    let cancelled = manager
                        .spawned(start, key, sys::ticks())
                        .expect("reserved launch ownership");
                    if cancelled {
                        let _ =
                            supervision::terminate(supervision::ProcessKey { pid, generation: 1 });
                    }
                    println!(
                        "[initator] started unit={} run={} pid={} tick={}",
                        start.unit, start.run, pid, sys::ticks()
                    );
                }
                Err(()) => {
                    manager
                        .spawn_failed(start, sys::ticks())
                        .expect("reserved failed launch");
                }
            }
        }
        if testing && manager.state(1) == Some(State::Failed) {
            assert!(matches!(manager.state(0), Some(State::Running(_))));
            println!("[initator-test] restart-limit reached; glow remains running");
            // Keep supervising. The host also asserts a later Glow heartbeat.
        }
        let timeout = match manager.next_deadline() {
            None => 0,
            Some(deadline) => match deadline.checked_sub(sys::ticks()) {
                Some(remaining) if remaining > 0 => remaining,
                _ => continue, // zero means infinite to the existing wait ABI
            },
        };
        sys::wait_events(sys::Events::CHILD, timeout).expect("child/deadline wait");
    }
}
