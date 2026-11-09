// Spark: a transient native controller, usable only with an explicit grant.
use userlib::{
    ipc::{self, Endpoint, Kind},
    recordln,
    service::{
        abi::{
            self,
            control::{Access, Operation, Request},
            replies::{Body, Code, JobPhase, Phase, Reply},
        },
        Client, Level,
    },
    sys,
};
type Result<T> = core::result::Result<T, &'static str>;
struct Controller {
    endpoint: Endpoint,
    service: Client,
    peer: ipc::ProcessKey,
    run: u64,
    access: Access,
    next: u64,
}
fn deadline() -> Result<u64> {
    sys::ticks().checked_add(180).ok_or("clock exhausted")
}
fn wait(until: u64) -> Result<()> {
    let remaining = until
        .checked_sub(sys::ticks())
        .filter(|&n| n > 0)
        .ok_or("manager timeout")?;
    sys::wait_events(sys::Events::PORT, remaining).map_err(|_| "event wait")?;
    Ok(())
}
impl Controller {
    fn bootstrap() -> Result<Self> {
        let endpoint =
            Endpoint::bootstrap(abi::CONTROL_TAG).map_err(|_| "no explicit control grant")?;
        let mut service =
            Client::bootstrap().map_err(|_| "missing service initialization grant")?;
        let until = deadline()?;
        let mut hello = None;
        let mut ready = false;
        loop {
            while let Some(event) = ipc::next().map_err(|_| "port event")? {
                if Some(event.endpoint) == endpoint.raw() {
                    if event.kind != Kind::Message {
                        return Err("manager disconnected during handshake");
                    }
                    let reply = Reply::decode(&event.payload[..event.length])
                        .map_err(|_| "invalid Hello")?;
                    let Body::Hello { access, run } = reply.body else {
                        return Err("expected Hello");
                    };
                    if hello.is_some() {
                        return Err("duplicate Hello");
                    }
                    hello = Some((event.sender.ok_or("unauthenticated Hello")?, access, run));
                } else {
                    service
                        .on_event(&event)
                        .map_err(|_| "invalid service initialization")?;
                }
            }
            if service.run().is_some() && !ready {
                service
                    .log(Level::Info, "control initialized")
                    .map_err(|_| "initialization log")?;
                service.ready().map_err(|_| "service ready")?;
                ready = true;
            }
            if let Some((peer, access, run)) = hello {
                if ready {
                    if service.manager() != Some(peer) || service.run() != Some(run) {
                        return Err("grant manager/run mismatch");
                    }
                    return Ok(Self {
                        endpoint,
                        service,
                        peer,
                        access,
                        run,
                        next: 1,
                    });
                }
            }
            wait(until)?;
        }
    }
    fn rpc<'a>(
        &mut self,
        operation: Operation<'_>,
        bytes: &'a mut [u8; abi::MAX_FRAME],
    ) -> Result<Reply<'a>> {
        let until = deadline()?;
        loop {
            let id = self.next;
            self.next = self.next.checked_add(1).ok_or("request IDs exhausted")?;
            let length = self.exchange_frame(Request { id, operation }, bytes, until)?;
            if matches!(
                Reply::decode(&bytes[..length])
                    .map_err(|_| "invalid reply")?
                    .body,
                Body::Error {
                    code: Code::Busy,
                    ..
                }
            ) {
                if sys::ticks() >= until {
                    return Err("manager busy deadline");
                }
                sys::sleep_ticks(1).map_err(|_| "budget retry deadline")?;
                continue;
            }
            return Reply::decode(&bytes[..length]).map_err(|_| "invalid reply");
        }
    }
    fn exchange<'a>(
        &mut self,
        request: Request<'_>,
        bytes: &'a mut [u8; abi::MAX_FRAME],
    ) -> Result<Reply<'a>> {
        let length = self.exchange_frame(request, bytes, deadline()?)?;
        Reply::decode(&bytes[..length]).map_err(|_| "invalid reply")
    }
    fn exchange_frame(
        &mut self,
        request: Request<'_>,
        bytes: &mut [u8; abi::MAX_FRAME],
        until: u64,
    ) -> Result<usize> {
        let n = request.encode(bytes).map_err(|_| "invalid request")?;
        self.endpoint
            .send(&bytes[..n])
            .map_err(|_| "control backpressure or disconnect")?;
        loop {
            while let Some(event) = ipc::next().map_err(|_| "port event")? {
                if Some(event.endpoint) != self.endpoint.raw() {
                    self.service
                        .on_event(&event)
                        .map_err(|_| "service disconnected")?;
                    continue;
                }
                if event.kind == Kind::PeerClosed {
                    return Err("manager disconnected");
                }
                if event.kind != Kind::Message {
                    continue;
                }
                if event.sender != Some(self.peer) {
                    return Err("foreign response");
                }
                // Validate request correlation before copying the borrowed frame for return.
                let reply =
                    Reply::decode(&event.payload[..event.length]).map_err(|_| "invalid reply")?;
                if reply.id != request.id {
                    return Err("stale response");
                }
                bytes[..event.length].copy_from_slice(&event.payload[..event.length]);
                return Ok(event.length);
            }
            wait(until)?;
        }
    }
}
fn render(reply: Reply<'_>) {
    match reply.body {
        Body::Status {
            index,
            name,
            phase,
            run,
            process,
            deadline,
        } => recordln!(
            "[spark] status index={} name={} state={:?} run={} pid={:?} deadline={:?}",
            index,
            name,
            phase,
            run,
            process.map(|p| p.pid),
            deadline
        ),
        Body::Job { job, phase } => recordln!("[spark] job={} outcome={:?}", job, phase),
        Body::Log {
            sequence,
            unit,
            run,
            tick,
            offset,
            total,
            text,
            ..
        } => {
            recordln!(
                "[spark] log seq={} unit={} run={} tick={} offset={} total={}",
                sequence,
                unit,
                run,
                tick,
                offset,
                total
            );
            recordln!("{}", text);
        }
        Body::End => recordln!("[spark] end"),
        Body::Error { code, detail } => recordln!("[spark] error={:?} detail={}", code, detail),
        Body::Hello { .. } => {}
    }
}
fn probe_status(controller: &mut Controller) -> Result<()> {
    assert_eq!(controller.access, Access::ReadOnly);
    let mut bytes = [0; abi::MAX_FRAME];
    let mut beacon = false;
    let mut own = false;
    for index in 0..4 {
        let reply = controller.rpc(Operation::Status { index }, &mut bytes)?;
        render(reply);
        match reply.body {
            Body::Status {
                name: "beacon",
                phase: Phase::Running,
                process: Some(_),
                ..
            } => beacon = true,
            Body::Status {
                name: "spark",
                phase: Phase::Running,
                run,
                ..
            } => {
                assert_eq!(run, controller.run);
                own = true;
            }
            Body::End => {
                assert_eq!(index, 3);
            }
            _ => {}
        }
    }
    assert!(beacon && own);
    for op in [
        Operation::Stop("beacon"),
        Operation::Start("missing"),
        Operation::Restart("beacon"),
        Operation::Shutdown,
    ] {
        assert_eq!(
            controller.rpc(op, &mut bytes)?.body,
            Body::Error {
                code: Code::Denied,
                detail: 0
            }
        );
    }
    let last = controller.next - 1;
    let until = deadline()?;
    loop {
        let body = controller
            .exchange(
                Request {
                    id: last,
                    operation: Operation::Status { index: 0 },
                },
                &mut bytes,
            )?
            .body;
        if matches!(
            body,
            Body::Error {
                code: Code::Busy,
                ..
            }
        ) {
            if sys::ticks() >= until {
                return Err("replay admission deadline");
            }
            sys::sleep_ticks(1).map_err(|_| "replay admission wait")?;
            continue;
        }
        assert_eq!(
            body,
            Body::Error {
                code: Code::Stale,
                detail: 0
            }
        );
        break;
    }
    assert!(matches!(
        controller.rpc(Operation::Job { job: 1 }, &mut bytes)?.body,
        Body::Job {
            job: 1,
            phase: JobPhase::Succeeded
        }
    ));
    let mut cursor = 0;
    let mut offset = 0;
    let mut logged = false;
    for _ in 0..8 {
        let reply = controller.rpc(Operation::Logs { cursor, offset }, &mut bytes)?;
        match reply.body {
            Body::Log {
                sequence,
                offset: start,
                total,
                text,
                ..
            } => {
                render(reply);
                logged = true;
                let end = start as usize + text.len();
                cursor = sequence;
                offset = if end == total as usize { 0 } else { end as u16 };
            }
            Body::End => break,
            Body::Error {
                code: Code::Busy, ..
            } => {
                sys::sleep_ticks(1).map_err(|_| "budget wait")?;
            }
            other => panic!("unexpected log page: {:?}", other),
        }
    }
    assert!(logged);
    recordln!("[spark] status authorization PASS");
    Ok(())
}
fn wait_job(controller: &mut Controller, job: u64, bytes: &mut [u8; abi::MAX_FRAME]) -> Result<()> {
    for _ in 0..128 {
        match controller.rpc(Operation::Job { job }, bytes)?.body {
            Body::Job {
                phase: JobPhase::Succeeded,
                ..
            } => return Ok(()),
            Body::Job {
                phase: JobPhase::Running,
                ..
            } => {
                sys::sleep_ticks(1).map_err(|_| "job deadline wait")?;
            }
            _ => return Err("job failed or superseded"),
        }
    }
    Err("job completion deadline")
}
fn probe_manage(controller: &mut Controller) -> Result<()> {
    assert_eq!(controller.access, Access::Manage);
    let mut bytes = [0; abi::MAX_FRAME];
    let mut previous = 0;
    let mut frames = None;
    // First cycle warms image/allocator paths, then compare 16 complete cycles.
    for _ in 0..17 {
        let Body::Job { job, .. } = controller.rpc(Operation::Start("fern"), &mut bytes)?.body
        else {
            return Err("start rejected");
        };
        wait_job(controller, job, &mut bytes)?;
        let Body::Status {
            phase: Phase::Running,
            process: Some(key),
            run,
            ..
        } = controller
            .rpc(Operation::Status { index: 2 }, &mut bytes)?
            .body
        else {
            return Err("missing live process");
        };
        assert!(key.pid > previous);
        assert!(run != 0);
        previous = key.pid;
        let Body::Job { job, .. } = controller.rpc(Operation::Stop("fern"), &mut bytes)?.body
        else {
            return Err("stop rejected");
        };
        wait_job(controller, job, &mut bytes)?;
        assert!(matches!(
            controller
                .rpc(Operation::Status { index: 2 }, &mut bytes)?
                .body,
            Body::Status {
                phase: Phase::Inactive,
                process: None,
                ..
            }
        ));
        let current = userlib::thread::allocated_frames();
        if let Some(before) = frames {
            assert_eq!(
                current, before,
                "service frames must recover after acknowledged stop"
            );
        } else {
            frames = Some(current);
        }
    }
    recordln!(
        "[spark] management lifecycle PASS cycles=16 frames={}",
        frames.unwrap()
    );
    Ok(())
}
fn probe_pressure(controller: &mut Controller) -> Result<()> {
    assert_eq!(controller.access, Access::ReadOnly);
    let mut bytes = [0; abi::MAX_FRAME];
    for batch in 0..2 {
        for offset in 0..8 {
            let request = Request {
                id: batch * 8 + offset + 1,
                operation: Operation::Status { index: 0 },
            };
            let n = request.encode(&mut bytes).map_err(|_| "pressure encode")?;
            controller
                .endpoint
                .send(&bytes[..n])
                .map_err(|_| "pressure request capacity")?;
        }
        // No replies are read. The first eight fill the receive FIFO; the
        // second batch leaves PID 1 holding a pending reply and paused reads.
        sys::sleep_ticks(if batch == 0 { 4 } else { 40 }).map_err(|_| "pressure hold")?;
    }
    let until = deadline()?;
    let mut count = 0;
    while count < 16 {
        while let Some(event) = ipc::next().map_err(|_| "pressure event")? {
            if Some(event.endpoint) != controller.endpoint.raw() {
                controller
                    .service
                    .on_event(&event)
                    .map_err(|_| "pressure service protocol")?;
                continue;
            }
            assert_eq!(event.kind, Kind::Message);
            assert_eq!(event.sender, Some(controller.peer));
            let reply =
                Reply::decode(&event.payload[..event.length]).map_err(|_| "pressure reply")?;
            count += 1;
            assert_eq!(reply.id, count);
            assert!(matches!(
                reply.body,
                Body::Status {
                    phase: Phase::Running,
                    name: "beacon",
                    ..
                } | Body::Error {
                    code: Code::Busy,
                    ..
                }
            ));
        }
        if count < 16 {
            wait(until)?;
        }
    }
    controller.next = 17;
    controller
        .endpoint
        .send(&[255])
        .map_err(|_| "malformed fixture send")?;
    loop {
        while let Some(event) = ipc::next().map_err(|_| "closure event")? {
            if Some(event.endpoint) == controller.endpoint.raw() {
                assert_eq!(event.kind, Kind::PeerClosed);
                recordln!("[spark] reply pressure PASS messages=16 closed=true");
                return Ok(());
            }
            controller
                .service
                .on_event(&event)
                .map_err(|_| "closure service protocol")?;
        }
        wait(until)?;
    }
}
fn probe_restart(controller: &mut Controller) -> Result<()> {
    assert_eq!(controller.access, Access::Manage);
    let mut bytes = [0; abi::MAX_FRAME];
    let Body::Status {
        phase: Phase::Running,
        process: Some(old),
        run: old_run,
        ..
    } = controller
        .rpc(Operation::Status { index: 0 }, &mut bytes)?
        .body
    else {
        return Err("missing prerequisite");
    };
    let Body::Job { job, .. } = controller
        .rpc(Operation::Restart("beacon"), &mut bytes)?
        .body
    else {
        return Err("restart rejected");
    };
    wait_job(controller, job, &mut bytes)?;
    let Body::Status {
        phase: Phase::Running,
        process: Some(new),
        run: new_run,
        ..
    } = controller
        .rpc(Operation::Status { index: 0 }, &mut bytes)?
        .body
    else {
        return Err("missing replacement");
    };
    assert_ne!(old, new);
    assert_ne!(old_run, new_run);
    assert!(matches!(
        controller
            .rpc(Operation::Status { index: 1 }, &mut bytes)?
            .body,
        Body::Status {
            name: "boot",
            phase: Phase::GroupActive,
            ..
        }
    ));
    recordln!(
        "[spark] referenced restart PASS job={} old={} new={}",
        job,
        old.pid,
        new.pid
    );
    Ok(())
}
fn run(argv: &[&str]) -> Result<()> {
    let Some(command) = argv.get(1).copied() else {
        return Err("usage: spark status|logs|start UNIT|stop UNIT|job ID");
    };
    let mut controller = Controller::bootstrap()?;
    if command == "--probe-status" {
        return probe_status(&mut controller);
    }
    if command == "--probe-manage" {
        return probe_manage(&mut controller);
    }
    if command == "--probe-pressure" {
        return probe_pressure(&mut controller);
    }
    if command == "--probe-restart" {
        return probe_restart(&mut controller);
    }
    let mut bytes = [0; abi::MAX_FRAME];
    match command {
        "status" => {
            for index in 0..64 {
                let reply = controller.rpc(Operation::Status { index }, &mut bytes)?;
                if matches!(reply.body, Body::End) {
                    break;
                }
                render(reply);
            }
        }
        "start" | "stop" | "restart" => {
            let name = argv.get(2).copied().ok_or("missing unit")?;
            let op = match command {
                "start" => Operation::Start(name),
                "stop" => Operation::Stop(name),
                _ => Operation::Restart(name),
            };
            render(controller.rpc(op, &mut bytes)?);
        }
        "job" => {
            let job = argv
                .get(2)
                .and_then(|v| v.parse().ok())
                .ok_or("invalid job ID")?;
            render(controller.rpc(Operation::Job { job }, &mut bytes)?);
        }
        "logs" => {
            let mut cursor = 0;
            let mut offset = 0;
            // A snapshot query is bounded even while other services keep logging.
            for _ in 0..512 {
                let reply = controller.rpc(Operation::Logs { cursor, offset }, &mut bytes)?;
                render(reply);
                match reply.body {
                    Body::Log {
                        sequence,
                        offset: start,
                        total,
                        text,
                        ..
                    } => {
                        let end = start as usize + text.len();
                        cursor = sequence;
                        offset = if end == total as usize { 0 } else { end as u16 };
                    }
                    _ => break,
                }
            }
        }
        _ => return Err("unknown command"),
    }
    Ok(())
}
fn main(_argc: usize, argv: &[&str]) {
    if let Err(error) = run(argv) {
        recordln!("[spark] {}", error);
        sys::exit(2);
    }
}
