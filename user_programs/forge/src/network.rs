//! Shell-owned HTTP/TLS and asynchronous ping commands, independent of the terminal.
use crate::ping;
use userlib::{println, sys};
pub struct Network {
    ping: Option<ping::Client>,
    ping_address: [u8; 4],
}
impl Network {
    pub fn new() -> Self {
        Self {
            ping: None,
            ping_address: [0; 4],
        }
    }
    fn push_line(&mut self, bytes: &[u8]) {
        sys::console_write(bytes);
        sys::console_write(b"\n");
    }
    fn push_args(&mut self, args: core::fmt::Arguments<'_>) {
        println!("{}", args);
    }
    pub fn command(&mut self, cmd: &[u8]) -> bool {
        if cmd == b"curl" || cmd.starts_with(b"curl ") {
            self.curl(core::str::from_utf8(cmd).unwrap_or(""));
        } else if cmd == b"ping" || cmd.starts_with(b"ping ") {
            if self.ping.is_some() {
                self.push_line(b"ping: already running");
            } else if let Ok(config) = ping::Config::parse(core::str::from_utf8(cmd).unwrap_or(""))
            {
                match ping::Client::start(config, sys::ticks(), &mut ping::Sockets) {
                    Ok(client) => {
                        self.ping_address = config.address;
                        self.ping = Some(client);
                        self.push_line(b"PING: 56 data bytes, bounded echo requests");
                    }
                    Err(error) => self.push_line(ping::error(error)),
                }
            } else {
                self.push_line(b"usage: ping [-c 1..16] IPv4");
            }
        } else {
            return false;
        }
        true
    }
    pub fn tick(&mut self) -> bool {
        let Some(client) = self.ping.as_mut() else {
            return false;
        };
        let Some(event) = client.step(sys::ticks(), &mut ping::Sockets) else {
            return false;
        };
        sys::console_write(b"\r");
        match event {
            ping::Event::Reply { sequence, ticks } => {
                let a = self.ping_address;
                if ticks == 0 {
                    self.push_args(format_args!(
                        "reply from {}.{}.{}.{} seq={} time=<55 ms",
                        a[0], a[1], a[2], a[3], sequence
                    ));
                } else {
                    self.push_args(format_args!(
                        "reply from {}.{}.{}.{} seq={} time=~{} ms",
                        a[0],
                        a[1],
                        a[2],
                        a[3],
                        sequence,
                        ticks.saturating_mul(55)
                    ));
                }
            }
            ping::Event::Timeout { sequence } => {
                self.push_args(format_args!("ping: seq={} timeout", sequence))
            }
            ping::Event::Done { sent, received } => {
                self.push_args(format_args!(
                    "{} transmitted, {} received, {}% loss",
                    sent,
                    received,
                    100 * (sent - received) / sent
                ));
                self.ping = None;
            }
            ping::Event::Failed(error) => {
                self.push_line(ping::error(error));
                self.ping = None;
            }
        }
        true
    }

    fn curl(&mut self, command: &str) {
        let mut words = command.split_ascii_whitespace();
        let _ = words.next();
        let Some(url) = words.next() else {
            self.push_line(b"usage: curl http[s]://host[:port]/path");
            return;
        };
        if words.next().is_some() {
            self.push_line(b"usage: curl http[s]://host[:port]/path");
            return;
        }
        let mut line = [0; 64];
        let mut len = 0;
        let result = userlib::http::get(url, |bytes| {
            for &byte in bytes {
                if byte == b'\r' {
                    continue;
                }
                if byte == b'\n' {
                    self.push_line(&line[..len]);
                    len = 0;
                    continue;
                }
                line[len] = if byte.is_ascii_graphic() || byte == b' ' {
                    byte
                } else {
                    b'.'
                };
                len += 1;
                if len == line.len() {
                    self.push_line(&line);
                    len = 0;
                }
            }
            Ok(())
        });
        if len != 0 {
            self.push_line(&line[..len]);
        }
        match result {
            Ok(response) => self.push_args(format_args!(
                "curl: HTTP {}, {} bytes",
                response.status, response.bytes
            )),
            Err(error) => self.push_args(format_args!(
                "curl: {}",
                userlib::http::error_message(error)
            )),
        }
    }
}
