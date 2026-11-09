use initator_host::Runtime;
use std::{io, path::Path, time::Duration};
fn run() -> io::Result<()> {
    let arguments: Vec<_> = std::env::args().collect();
    if arguments.len() != 3 {
        return Err(io::Error::other(
            "usage: initator-host DIRECTORY DURATION_MS (1..60000)",
        ));
    }
    let milliseconds = arguments[2]
        .parse::<u64>()
        .ok()
        .filter(|&n| n > 0 && n <= 60_000)
        .ok_or_else(|| io::Error::other("invalid bounded duration"))?;
    let catalog = initator_host::config::load(Path::new(&arguments[1]))?;
    let mut runtime = Runtime::new(catalog)?;
    runtime
        .manager
        .start("boot")
        .map_err(|error| io::Error::other(format!("boot: {error:?}")))?;
    match runtime.drive_until(Duration::from_millis(milliseconds), |_| false) {
        Err(error) if error.kind() == io::ErrorKind::TimedOut => {}
        result => result?,
    }
    let events: Vec<_> = runtime.events().copied().collect();
    // Cleanup precedes diagnostic output: a slow stdout consumer cannot retain the
    // managed children after this finite development run has completed.
    drop(runtime);
    for event in events {
        println!("{event:?}");
    }
    Ok(())
}
fn main() {
    if let Err(error) = run() {
        eprintln!("initator-host: {error}");
        std::process::exit(2);
    }
}
