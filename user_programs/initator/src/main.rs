// Initator: native PID 1. Policy lives in the host-tested service core.
extern crate alloc;
mod ports;
use alloc::{string::String, vec::Vec};
use rustos_service_core::{
    config::{MAX_CATALOG_BYTES, MAX_FILE_BYTES, MAX_UNITS},
    graph::{Catalog, ErrorKind},
    manager::{Action, Manager},
    transaction::Outcome,
    Completion, ProcessKey,
};
use userlib::{fs, recordln as println, supervision, sys};
const DIRECTORY: &str = "/ETC/INIT/SYSTEM";

// Keep each diagnostic below the bounded record writer's 256-byte capacity.
fn clip(text: &str, maximum: usize) -> &str {
    let mut end = text.len().min(maximum);
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    &text[..end]
}
fn load_catalog() -> Result<Catalog, ()> {
    // Two extra entries allow FAT dot entries; catalog files still have a strict
    // 64-unit limit. Reject a large directory before allocating its wire buffer.
    let entries =
        fs::read_dir_bounded(DIRECTORY, (MAX_UNITS + 2) * 272, MAX_UNITS + 2).map_err(|()| {
            println!(
                "[initator] configuration rejected directory={} reason=listing-or-budget",
                DIRECTORY
            )
        })?;
    let mut sources: Vec<(String, Vec<u8>)> = Vec::new();
    sources.try_reserve_exact(MAX_UNITS).map_err(|_| ())?;
    let mut total = 0usize;
    for entry in entries {
        if entry.name == "." || entry.name == ".." {
            continue;
        }
        if sources.len() >= MAX_UNITS {
            println!(
                "[initator] configuration rejected file={} reason=unit-count",
                clip(&entry.name, 64)
            );
            return Err(());
        }
        if entry.is_directory || entry.size > MAX_FILE_BYTES as u64 {
            println!(
                "[initator] configuration rejected file={} reason=file-or-budget",
                clip(&entry.name, 64)
            );
            return Err(());
        }
        total = total
            .checked_add(entry.size as usize)
            .filter(|&n| n <= MAX_CATALOG_BYTES)
            .ok_or_else(|| {
                println!(
                    "[initator] configuration rejected file={} reason=catalog-budget",
                    clip(&entry.name, 64)
                )
            })?;
        // Validate the basename before appending it to a trusted directory path.
        if entry.name.len() > 255
            || entry.name.contains('/')
            || entry.name.contains('\\')
            || entry.name.chars().any(|c| c.is_control())
            || !entry
                .name
                .rsplit_once('.')
                .is_some_and(|(_, ext)| ext.eq_ignore_ascii_case("unit"))
        {
            println!(
                "[initator] configuration rejected file={} reason=InvalidSource",
                clip(&entry.name, 64)
            );
            return Err(());
        }
        let mut path = String::new();
        path.try_reserve_exact(DIRECTORY.len() + 1 + entry.name.len())
            .map_err(|_| ())?;
        path.push_str(DIRECTORY);
        path.push('/');
        path.push_str(&entry.name);
        let mut file = fs::File::open(&path, fs::O_RDONLY).map_err(|()| {
            println!(
                "[initator] configuration rejected file={} reason=open",
                clip(&entry.name, 64)
            );
        })?;
        let mut bytes = Vec::new();
        bytes
            .try_reserve_exact(MAX_FILE_BYTES + 1)
            .map_err(|_| ())?;
        bytes.resize(MAX_FILE_BYTES + 1, 0);
        let mut length = 0;
        loop {
            let n = file.read(&mut bytes[length..]).map_err(|()| {
                println!(
                    "[initator] configuration rejected file={} reason=read",
                    clip(&entry.name, 64)
                );
            })?;
            if n > bytes.len() - length {
                return Err(());
            }
            if n == 0 {
                break;
            }
            length += n;
            if length > MAX_FILE_BYTES {
                println!(
                    "[initator] configuration rejected file={} reason=file-budget",
                    clip(&entry.name, 64)
                );
                return Err(());
            }
        }
        if length as u64 != entry.size {
            println!(
                "[initator] configuration rejected file={} reason=size-race",
                clip(&entry.name, 64)
            );
            return Err(());
        }
        bytes.truncate(length);
        sources.push((entry.name, bytes));
    }
    let mut views = Vec::new();
    views.try_reserve_exact(sources.len()).map_err(|_| ())?;
    for (name, bytes) in &sources {
        views.push((name.as_str(), bytes.as_slice()));
    }
    Catalog::load(&views).map_err(|error| {
        let (file, bytes) = views
            .get(error.source)
            .copied()
            .unwrap_or(("<catalog>", &[]));
        match error.kind {
            ErrorKind::Parse(error) => {
                let text = core::str::from_utf8(bytes).unwrap_or("");
                println!(
                    "[initator] configuration rejected file={} line={} key={} reason={:?}",
                    clip(file, 64),
                    error.line,
                    clip(error.key(text).unwrap_or("<none>"), 32),
                    error.kind
                );
            }
            ErrorKind::Cycle(path) => {
                println!(
                    "[initator] configuration rejected file={} reason=Cycle",
                    clip(file, 64)
                );
                for edge in path.windows(2) {
                    println!("[initator] cycle {} -> {}", edge[0], edge[1]);
                }
            }
            reason => println!(
                "[initator] configuration rejected file={} reason={:?}",
                clip(file, 64),
                reason
            ),
        }
    })
}
/// Configuration failure is recoverable: keep PID 1 alive without activating a
/// partial catalog. A control channel/reload is a later milestone.
fn recovery() -> ! {
    println!("[initator] configuration rejected; recovery mode; no services launched");
    loop {
        // No children are spawned on this path. Still drain durable notifications
        // rather than spinning if an unexpected event ever becomes pending.
        while matches!(supervision::next_child(), Ok(Some(_))) {}
        let _ = sys::wait_events(sys::Events::CHILD, 0);
    }
}
fn main() {
    let identity = supervision::current().expect("native process identity");
    assert_eq!(
        identity.pid, 1,
        "Initator must be the first userspace process"
    );
    println!("[initator] PID 1 entered Ring 3");
    let catalog = match load_catalog() {
        Ok(catalog) => catalog,
        Err(()) => recovery(),
    };
    println!("[initator] catalog validated units={}", catalog.len());
    let mut manager = Manager::new(catalog).expect("validated service policies");
    if let Err(error) = manager.start("boot") {
        println!(
            "[initator] configuration rejected boot target reason={:?}",
            error
        );
        recovery();
    }
    let mut ports = ports::Ports::new();
    let mut last_outcome = Outcome::Running;
    let mut states = [None; MAX_UNITS];
    loop {
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
                        process.pid,
                        cause,
                        status,
                        sys::ticks()
                    );
                    let completion = match cause {
                        supervision::Cause::Exited => Completion::Exit(status),
                        supervision::Cause::Killed => Completion::Killed,
                        supervision::Cause::Fault => Completion::Fault(status as u16),
                    };
                    ports.mark_exit(process);
                    manager.completed(
                        ProcessKey {
                            pid: process.pid,
                            generation: process.generation,
                        },
                        completion,
                        sys::ticks(),
                    );
                }
            }
        }
        ports
            .resume(sys::ticks())
            .expect("resume bounded port intake");
        ports.drain(&mut manager).expect("owned native port events");
        ports.finish_exits();
        let mut issued_action = false;
        if let Some(action) = manager
            .next_action(sys::ticks())
            .expect("service job invariants")
        {
            issued_action = true;
            match action {
                Action::Start(start) => {
                    let unit = manager
                        .catalog()
                        .unit(start.unit)
                        .expect("admitted catalog index");
                    let result = ports::Run::launch(unit, start);
                    match result {
                        Ok(run) => {
                            let key = run.key();
                            let pid = key.pid;
                            // M1's ABI allocates monotonic PIDs with generation 1;
                            // callers never infer identity from a recycled PID.
                            println!(
                                "[initator] started name={} unit={} run={} pid={} tick={}",
                                unit.name,
                                start.unit,
                                start.run,
                                pid,
                                sys::ticks()
                            );
                            manager
                                .spawned(start, key, sys::ticks())
                                .expect("reserved launch ownership");
                            ports
                                .install(start.unit, run)
                                .expect("owned per-run service grants");
                        }
                        Err(()) => {
                            println!(
                                "[initator] spawn failed name={} unit={}",
                                unit.name, start.unit
                            );
                            manager
                                .spawn_failed(start, sys::ticks())
                                .expect("reserved failed launch");
                        }
                    }
                }
                Action::Terminate { unit, process } => {
                    let name = &manager
                        .catalog()
                        .unit(unit)
                        .expect("stop catalog index")
                        .name;
                    println!(
                        "[initator] stopping name={} unit={} pid={} tick={} reason={:?}",
                        name,
                        unit,
                        process.pid,
                        sys::ticks(),
                        manager.run_status(unit).and_then(|r| r.failure)
                    );
                    // A process may already have exited; its durable completion
                    // remains the acknowledgement. Never acknowledge merely kill().
                    let _ = supervision::terminate(supervision::ProcessKey {
                        pid: process.pid,
                        generation: process.generation,
                    });
                }
            }
        }
        for (unit, previous) in states.iter_mut().enumerate().take(manager.catalog().len()) {
            let current = manager.state(unit);
            if current != *previous {
                if let Some(state) = current {
                    let name = &manager
                        .catalog()
                        .unit(unit)
                        .expect("state catalog index")
                        .name;
                    println!(
                        "[initator] state name={} unit={} state={:?}",
                        name, unit, state
                    );
                }
                *previous = current;
            }
        }
        let outcome = manager.outcome();
        if outcome != last_outcome {
            println!("[initator] activation outcome={:?}", outcome);
            last_outcome = outcome;
        }
        if issued_action {
            continue;
        }
        let deadline = manager
            .next_deadline()
            .into_iter()
            .chain(ports.next_deadline())
            .min();
        let timeout = match deadline {
            None => 0,
            Some(deadline) => match deadline.checked_sub(sys::ticks()) {
                Some(remaining) if remaining > 0 => remaining,
                _ => continue,
            },
        };
        sys::wait_events(sys::Events::CHILD | sys::Events::PORT, timeout)
            .expect("child/deadline wait");
    }
}
