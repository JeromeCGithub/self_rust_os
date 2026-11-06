// Native Ring3 PCM WAV player. Files/controls/decoding remain application policy.
extern crate alloc;
use alloc::{format, string::String, vec::Vec};
use rustos_media::Wav;
use userlib::{
    audio, fs, sys,
    ui::{self, Color, Event, Material, Rect, Window, WindowOptions},
};
fn read_clip(path: &str) -> Result<Vec<u8>, ()> {
    let mut file = fs::File::open(path, fs::O_RDONLY)?;
    let size = file.seek(0, fs::SEEK_END)?;
    if size > rustos_media::MAX_CLIP as u64 {
        return Err(());
    }
    file.seek(0, fs::SEEK_SET)?;
    let mut bytes = Vec::new();
    bytes.try_reserve_exact(size as usize).map_err(|_| ())?;
    bytes.resize(size as usize, 0);
    let mut at = 0;
    while at < bytes.len() {
        let end = (at + 4096).min(bytes.len());
        let n = file.read(&mut bytes[at..end])?;
        if n == 0 {
            return Err(());
        }
        at += n;
    }
    Ok(bytes)
}
struct Player<'a> {
    clip: Option<Wav<'a>>,
    stream: Option<audio::Stream>,
    base: u64,
    next: u64,
    position: u64,
    playing: bool,
    message: String,
}
impl<'a> Player<'a> {
    fn play(&mut self) -> Result<(), audio::Error> {
        let Some(clip) = self.clip else {
            return Err(audio::Error::Invalid);
        };
        if self.position >= clip.frames() {
            self.seek(0)?;
        }
        if let Some(stream) = self.stream.as_mut() {
            stream.resume()?;
        } else {
            self.stream = Some(audio::Stream::open()?);
            self.base = self.position;
            self.next = self.position;
        }
        self.playing = true;
        self.message = "Playing".into();
        userlib::println!("[jplay] playing");
        Ok(())
    }
    fn pause(&mut self) -> Result<(), audio::Error> {
        if let Some(stream) = self.stream.as_mut() {
            stream.pause()?;
            self.position = self.base + stream.status()?.played_frames;
        }
        self.playing = false;
        self.message = "Paused".into();
        userlib::println!("[jplay] paused");
        Ok(())
    }
    fn seek(&mut self, frame: u64) -> Result<(), audio::Error> {
        let resume = self.playing;
        self.stream.take();
        self.playing = false;
        self.position = frame.min(self.clip.map_or(0, |c| c.frames()));
        self.base = self.position;
        self.next = self.position;
        if resume && self.position < self.clip.map_or(0, |c| c.frames()) {
            self.play()?;
        }
        userlib::println!("[jplay] seek frame={}", self.position);
        Ok(())
    }
    fn pump(&mut self) -> Result<bool, audio::Error> {
        if !self.playing {
            return Ok(false);
        }
        let clip = self.clip.ok_or(audio::Error::Invalid)?;
        let stream = self.stream.as_mut().ok_or(audio::Error::Failed)?;
        let status = stream.status()?;
        self.position = self.base + status.played_frames;
        if self.position == clip.frames() && status.queued_frames == 0 {
            self.playing = false;
            self.stream.take();
            self.message = "Finished".into();
            userlib::println!("[jplay] finished");
            return Ok(true);
        }
        // Bounded fill work keeps UI/input responsive while hardware consumes.
        for _ in 0..8 {
            let chunk = clip
                .chunk(self.next, audio::MAX_WRITE)
                .map_err(|_| audio::Error::Invalid)?;
            if chunk.is_empty() {
                break;
            }
            match stream.write(chunk) {
                Ok(n) => self.next += (n / 4) as u64,
                Err(audio::Error::WouldBlock) => break,
                Err(error) => return Err(error),
            }
        }
        Ok(true)
    }
    fn draw(&self, canvas: &mut ui::Canvas<'_>) {
        canvas.clear(Color::rgb(0x172b43));
        ui::text(canvas, 16, 14, "jplay  /  Audio", Color::rgb(0xffffff), 1);
        ui::text(
            canvas,
            16,
            32,
            "Open a WAV file: jplay /path/to/clip.wav",
            Color::rgb(0xc6d9ed),
            1,
        );
        for (x, label) in [
            (16, "Play"),
            (128, "Pause"),
            (240, "-1 second"),
            (352, "+1 second"),
        ] {
            canvas.rounded_rect(Rect::new(x, 52, 100, 30), 4, Color::rgb(0x314c6d));
            ui::text(canvas, x + 8, 63, label, Color::rgb(0xffffff), 1);
        }
        let phase = if self.playing {
            0x2f795f
        } else if self.message == "Paused" {
            0x8b6f3e
        } else {
            0x315f87
        };
        let width = canvas.width().saturating_sub(32);
        canvas.fill_rect(Rect::new(16, 88, width, 20), Color::rgb(phase));
        ui::text(canvas, 24, 94, &self.message, Color::rgb(0xffffff), 1);
        canvas.fill_rect(Rect::new(16, 112, width, 12), Color::rgb(0x314c6d));
        let total = self.clip.map_or(0, |c| c.frames());
        let filled = if total == 0 {
            0
        } else {
            (self.position.min(total) * u64::from(width) / total) as u32
        };
        canvas.fill_rect(Rect::new(16, 112, filled, 12), Color::rgb(0x60d6b5));
        ui::text(
            canvas,
            16,
            140,
            &format!(
                "{} / {} ms",
                self.position * 1000 / 48000,
                total * 1000 / 48000
            ),
            Color::rgb(0xc6d9ed),
            1,
        );
    }
}
fn main(_: usize, argv: &[&str]) {
    let bytes = read_clip(argv.get(1).copied().unwrap_or("/HOME/TONE.WAV"));
    let clip = bytes.as_ref().ok().and_then(|bytes| Wav::parse(bytes).ok());
    let mut player = Player {
        clip,
        stream: None,
        base: 0,
        next: 0,
        position: 0,
        playing: false,
        message: if clip.is_some() {
            "Ready".into()
        } else {
            "File unavailable or unsupported WAV".into()
        },
    };
    let mut window = match Window::create(
        WindowOptions::new("jplay")
            .content_size(560, 180)
            .background(Material::Opaque),
    ) {
        Ok(window) => window,
        Err(_) => return,
    };
    if window.draw(|canvas| player.draw(canvas)).is_err() {
        return;
    }
    userlib::println!("[jplay] ready");
    let mut pressed = false;
    loop {
        let mut dirty = false;
        let mut action = None;
        for _ in 0..32 {
            match window.poll_event() {
                Ok(Some(Event::CloseRequested)) => return,
                Ok(Some(Event::Resized { .. })) => dirty = true,
                Ok(Some(Event::Pointer { x, y, buttons })) => {
                    if buttons & 1 != 0 {
                        pressed = true;
                    } else if pressed {
                        pressed = false;
                        for (n, left) in [16, 128, 240, 352].iter().enumerate() {
                            if Rect::new(*left, 52, 100, 30).contains(x, y) {
                                action = Some(n);
                            }
                        }
                    }
                }
                Ok(Some(_)) => {}
                Ok(None) => break,
                Err(_) => return,
            }
        }
        let result = match action {
            Some(0) => player.play(),
            Some(1) => player.pause(),
            Some(2) => player.seek(player.position.saturating_sub(48000)),
            Some(3) => player.seek(player.position.saturating_add(48000)),
            _ => Ok(()),
        };
        if action.is_some() {
            dirty = true;
        }
        if let Err(error) = result {
            player.playing = false;
            player.stream.take();
            player.message = format!("Audio unavailable: {error:?}");
            dirty = true;
        }
        match player.pump() {
            Ok(changed) => dirty |= changed,
            Err(error) => {
                player.stream.take();
                player.playing = false;
                player.message = format!("Playback failed: {error:?}");
                dirty = true;
            }
        }
        if dirty && window.draw(|canvas| player.draw(canvas)).is_err() {
            return;
        }
        sys::wait_events(sys::Events::MAILBOX, 1).ok();
    }
}
