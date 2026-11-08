extern crate alloc;
use rustos_audio_abi as abi;
use userlib::{audio, sys};
fn raw(n: u64, a: u64, b: u64, c: u64) -> u64 {
    // SAFETY: Test-only malformed scalar/memory arguments must be rejected by the
    // kernel without dereferencing them. No Rust reference is constructed here.
    unsafe { sys::syscall(n, a, b, c) }
}
fn main(_: usize, argv: &[&str]) {
    let mut mode = [0];
    let _ = sys::read_file("/TMP/AUDIO.MOD", &mut mode);
    if argv.contains(&"--missing") || mode[0] == b'M' {
        assert!(matches!(audio::Stream::open(), Err(audio::Error::NoDevice)));
        userlib::println!("[audiotest] PASS missing hardware");
        return;
    }
    if argv.get(1) == Some(&"--foreign") {
        let handle = argv[2].parse::<u64>().unwrap();
        assert!(matches!(audio::Stream::open(), Err(audio::Error::Busy)));
        assert_eq!(
            raw(abi::SYS_CONTROL, handle, abi::CLOSE, 0),
            abi::Error::NotOwner.encode()
        );
        let mut status = abi::Status::default();
        assert_eq!(
            raw(
                abi::SYS_STATUS,
                handle,
                core::ptr::addr_of_mut!(status) as u64,
                40
            ),
            abi::Error::NotOwner.encode()
        );
        return;
    }
    if argv.contains(&"--leak") || argv.contains(&"--hold") {
        let mut stream = audio::Stream::open().unwrap();
        stream.write(&[0; 4096]).unwrap();
        core::mem::forget(stream); // Deliberately rely on kernel exit/kill cleanup.
        if argv.contains(&"--hold") {
            userlib::println!("[audiotest] holding");
            loop {
                sys::sleep_ticks(1).unwrap();
            }
        }
        return;
    }
    let mut stream = audio::Stream::open().expect("native PCM stream");
    let status = stream.status().unwrap();
    assert_eq!(status.submitted_frames, 0);
    assert_eq!(status.played_frames, 0);
    assert!(matches!(audio::Stream::open(), Err(audio::Error::Busy)));
    assert_eq!(raw(abi::SYS_OPEN, 1, 0, 0), abi::Error::Invalid.encode());
    assert_eq!(raw(abi::SYS_WRITE, 999, 0, 4), abi::Error::Invalid.encode());
    assert_eq!(
        raw(abi::SYS_CONTROL, 999, 0, 0),
        abi::Error::NotOwner.encode()
    );
    assert_eq!(stream.write(&[0; 3]), Err(audio::Error::Invalid));
    stream.pause().unwrap();
    assert_eq!(stream.status().unwrap().flags & 2, 2);
    let mut data = [0u8; 4096];
    for (index, frame) in data.chunks_exact_mut(4).enumerate() {
        let value = if index % 48 < 24 { 6000i16 } else { -6000i16 };
        frame[..2].copy_from_slice(&value.to_le_bytes());
        frame[2..].copy_from_slice(&value.to_le_bytes());
    }
    for _ in 0..32 {
        assert_eq!(stream.write(&data), Ok(4096));
    }
    assert_eq!(stream.write(&data), Err(audio::Error::WouldBlock));
    let paused = stream.status().unwrap();
    assert_eq!(paused.submitted_frames, 32768);
    assert_eq!(paused.played_frames, 0);
    assert_eq!(paused.free_slots, 0);
    sys::sleep_ticks(2).unwrap();
    assert_eq!(stream.status().unwrap().played_frames, 0);
    stream.resume().unwrap();
    let started = sys::ticks();
    let mut played = 0;
    let mut paused_during_playback = false;
    loop {
        let current = stream.status().unwrap();
        assert!(
            current.played_frames >= played && current.played_frames <= current.submitted_frames
        );
        played = current.played_frames;
        if !paused_during_playback && played >= 2048 && current.queued_frames > 2048 {
            stream.pause().unwrap();
            let stopped = stream.status().unwrap().played_frames;
            sys::sleep_ticks(2).unwrap();
            assert_eq!(stream.status().unwrap().played_frames, stopped);
            stream.resume().unwrap();
            paused_during_playback = true;
        }
        if current.queued_frames == 0 {
            break;
        }
        assert!(
            sys::ticks().wrapping_sub(started) < 40,
            "PCM playback deadline"
        );
        sys::sleep_ticks(1).unwrap();
    }
    assert_eq!(played, 32768);
    assert!(paused_during_playback, "pause a genuinely running stream");
    // Enqueue after drain and while already running: repeated RUN writes must
    // not skip descriptors or consume stale samples from the previous wrap.
    for _ in 0..4 {
        stream.write(&data).unwrap();
    }
    let started = sys::ticks();
    while stream.status().unwrap().queued_frames != 0 {
        assert!(sys::ticks().wrapping_sub(started) < 20);
        sys::sleep_ticks(1).unwrap();
    }
    assert_eq!(stream.status().unwrap().played_frames, 36864);
    drop(stream);
    let handle = abi::decode(raw(abi::SYS_OPEN, 0, 0, 0)).unwrap();
    for (pointer, length) in [
        (0, 4),
        (u64::MAX - 1, 4),
        (0xffff800000000000, 4),
        (0x3ffffffff, 4),
        (data.as_ptr() as u64, 0),
        (data.as_ptr() as u64, 4097),
    ] {
        assert_eq!(
            raw(abi::SYS_WRITE, handle, pointer, length),
            abi::Error::Invalid.encode()
        );
    }
    for pointer in [0, 0x400000, u64::MAX - 20, 0x3ffffffff] {
        assert_eq!(
            raw(abi::SYS_STATUS, handle, pointer, 40),
            abi::Error::Invalid.encode()
        );
    }
    let text = alloc::format!("{handle}");
    let foreign = sys::spawn_with_args("/BIN/ZAUDIO.BIN", &["--foreign", &text]).unwrap();
    assert_eq!(sys::waitpid(foreign), Ok(0));
    assert_eq!(raw(abi::SYS_CONTROL, handle, 0, 0), 0);
    assert_eq!(
        raw(abi::SYS_CONTROL, handle, 0, 0),
        abi::Error::NotOwner.encode()
    );
    let scratch = alloc::vec![0u8;32768];
    core::hint::black_box(scratch.as_ptr());
    drop(scratch);
    let before = raw(107, 0, 0, 0);
    for _ in 0..3 {
        let child = sys::spawn_with_args("/BIN/ZAUDIO.BIN", &["--leak"]).unwrap();
        assert_eq!(sys::waitpid(child), Ok(0));
        let mut child =
            userlib::process::Child::spawn_with_args("/BIN/ZAUDIO.BIN", &["--hold"]).unwrap();
        let started = sys::ticks();
        let mut ready = false;
        let mut handshake = [0u8; 64];
        let mut used = 0;
        while !ready {
            match child.read() {
                Ok(Some((bytes, n))) => {
                    assert!(used + n <= handshake.len());
                    handshake[used..used + n].copy_from_slice(&bytes[..n]);
                    used += n;
                    ready = handshake[..used]
                        .windows(b"holding".len())
                        .any(|w| w == b"holding")
                }
                Ok(None) => {
                    sys::sleep_ticks(1).unwrap();
                }
                Err(_) => panic!("audio owner died before kill"),
            }
            assert!(sys::ticks().wrapping_sub(started) < 50);
        }
        assert!(matches!(audio::Stream::open(), Err(audio::Error::Busy)));
        drop(child);
        let deadline = sys::ticks() + 50;
        while raw(107, 0, 0, 0) != before {
            assert!(sys::ticks() < deadline, "audio owner frames reclaimed");
            sys::sleep_ticks(1).unwrap();
        }
        let stream = audio::Stream::open().unwrap();
        assert_eq!(stream.status().unwrap().submitted_frames, 0);
        drop(stream);
    }
    for _ in 0..3 {
        let stream = audio::Stream::open().unwrap();
        assert_eq!(stream.status().unwrap().submitted_frames, 0);
        drop(stream);
    }
    userlib::println!(
        "[audiotest] PASS PCM ownership invalid mappings pause backpressure playback positions exit kill and repeated frame recovery"
    );
}
