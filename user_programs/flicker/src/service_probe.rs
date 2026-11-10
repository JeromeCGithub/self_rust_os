//! Trusted fixtures for explicit readiness and work-path watchdog behavior.
use userlib::{
    ipc, recordln,
    service::{Client, Level},
    sys,
};
pub fn run(mode: &str) {
    if matches!(mode, "--wrong-ready" | "--log-ready" | "--foreign-ready") {
        forged(mode);
        return;
    }
    let mut client = Client::bootstrap().expect("explicit notify grant");
    while client.run().is_none() {
        while let Some(event) = ipc::next().expect("service event") {
            client.on_event(&event).expect("service initialization");
        }
        if client.run().is_none() {
            sys::wait_events(sys::Events::PORT, 100).expect("initialization wait");
        }
    }
    client
        .log(Level::Info, "initializing")
        .expect("service initialization log");
    recordln!(
        "[service-probe] initialized run={} tick={}",
        client.run().unwrap(),
        sys::ticks()
    );
    if mode == "--never-ready" {
        loop {
            sys::wait_events(sys::Events::PORT, 0).expect("intentionally no Ready");
        }
    }
    sys::sleep_ticks(3).expect("simulate bounded application setup");
    client.ready().expect("explicit Ready after setup");
    recordln!(
        "[service-probe] ready run={} tick={}",
        client.run().unwrap(),
        sys::ticks()
    );
    if mode == "--stop-close" {
        drop(client);
        loop {
            sys::wait_events(sys::Events::CHILD, 0).unwrap();
        }
    }
    if mode == "--cooperative" {
        loop {
            while let Some(event) = ipc::next().unwrap() {
                client
                    .on_event(&event)
                    .expect("cooperative service control");
            }
            if client.stop_requested() {
                recordln!(
                    "[service-probe] cooperative-cleaned run={} tick={}",
                    client.run().unwrap(),
                    sys::ticks()
                );
                return;
            }
            sys::wait_events(sys::Events::PORT, 0).unwrap();
        }
    }
    if mode == "--watchdog" {
        loop {
            sys::wait_events(sys::Events::PORT, 0).expect("intentionally wedged work path");
        }
    }
    if matches!(mode, "--log-flood" | "--log-exit") {
        flood(&client, mode == "--log-exit");
    }
    let mut work = 0;
    loop {
        sys::sleep_ticks(2).expect("work deadline");
        work += 1;
        // Heartbeats follow completed work; no independent background pinger.
        if client.watchdog().is_some() {
            client.heartbeat().expect("work-path watchdog pulse");
        }
        recordln!(
            "[service-probe] work={} run={}",
            work,
            client.run().unwrap()
        );
    }
}

fn forged(mode: &str) {
    use userlib::service::abi;
    let notify = ipc::Endpoint::bootstrap(abi::NOTIFY_TAG).unwrap();
    let log = ipc::Endpoint::bootstrap(abi::LOG_TAG).unwrap();
    let run = loop {
        if let Some(event) = ipc::next().unwrap() {
            assert_eq!(Some(event.endpoint), notify.raw());
            if let abi::Message::Init { run, .. } =
                abi::Message::decode(&event.payload[..event.length]).unwrap()
            {
                break run;
            }
            panic!("unexpected initialization");
        }
        sys::wait_events(sys::Events::PORT, 100).unwrap();
    };
    let mut bytes = [0; abi::MAX_FRAME];
    let token = if mode == "--wrong-ready" {
        run + 1
    } else {
        run
    };
    let n = abi::Message::Ready { run: token }
        .encode(&mut bytes)
        .unwrap();
    if mode == "--log-ready" {
        log.send(&bytes[..n]).unwrap();
    } else if mode == "--foreign-ready" {
        // The first native manager-owned server is handle 1 in this private
        // fixture. Knowing its numeric value must not confer ownership.
        // SAFETY: The complete live payload is readable for this scalar ABI call;
        // only the endpoint identity is deliberately foreign and must be denied.
        assert_eq!(
            unsafe { sys::syscall(ipc::abi::SYS_SEND, 1, bytes.as_ptr() as u64, n as u64) },
            sys::SYSCALL_ERROR
        );
    } else {
        notify.send(&bytes[..n]).unwrap();
    }
    recordln!("[service-probe] forged mode={} run={}", mode, run);
    loop {
        sys::wait_events(sys::Events::PORT, 0).unwrap();
    }
}

/// Deliberately saturate only the log grant. The completed work path continues
/// to pulse its independent watchdog; a second fixture exits with queued logs.
fn flood(client: &Client, exit: bool) -> ! {
    let mut work = 0u64;
    loop {
        let mut accepted = 0;
        let mut full = 0;
        for _ in 0..512 {
            match client.log(Level::Info, "bounded flood fixture") {
                Ok(()) => accepted += 1,
                Err(userlib::service::Error::Full) => full += 1,
                Err(error) => panic!("unexpected log failure: {:?}", error),
            }
        }
        if work == 0 {
            recordln!(
                "[service-probe] log-pressure accepted={} full={}",
                accepted,
                full
            );
        }
        if exit {
            recordln!("[service-probe] log-exit status=23");
            sys::exit(23);
        }
        work += 1;
        client.heartbeat().expect("completed flood work heartbeat");
        recordln!(
            "[service-probe] flood-work={} run={}",
            work,
            client.run().unwrap()
        );
        sys::sleep_ticks(2).expect("bounded flood cadence");
    }
}
