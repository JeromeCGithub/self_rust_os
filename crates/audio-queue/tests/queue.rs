use rustos_audio_abi::Error;
use rustos_audio_queue::Ring;
#[test]
fn backpressure_cursor_wrap_and_last_buffer_completion_preserve_pcm_frame_counts() {
    let mut ring = Ring::default();
    for round in 0..3 {
        for n in 0..32 {
            assert_eq!(ring.push(1024).unwrap(), n);
        }
        assert_eq!(ring.push(1), Err(Error::WouldBlock));
        ring.observe(16, false, 1024).unwrap();
        assert_eq!(ring.queued(), 16);
        assert_eq!(ring.played(1024), round * 32768 + 16 * 1024 + 512);
        ring.observe(31, true, 0).unwrap();
        assert_eq!(ring.queued(), 0);
        assert_eq!(ring.played(0), (round + 1) * 32768);
        assert_eq!(ring.submitted(), (round + 1) * 32768);
    }
}
#[test]
fn empty_partial_and_invalid_observations_cannot_invent_or_lose_samples() {
    let mut ring = Ring::default();
    for size in [0, 1025, u32::MAX] {
        assert_eq!(ring.push(size), Err(Error::Invalid));
    }
    assert_eq!(ring.submitted(), 0);
    ring.push(16).unwrap();
    assert_eq!(ring.observe(32, false, 0), Err(Error::Failed));
    assert_eq!(ring.observe(0, false, 33), Err(Error::Failed));
    assert_eq!(ring.queued(), 1);
    assert_eq!(ring.played(32), 0);
    ring.observe(0, false, 16).unwrap();
    assert_eq!(ring.played(16), 8);
    ring.observe(0, true, 0).unwrap();
    assert_eq!(ring.played(0), 16);
    ring.observe(0, true, 0).unwrap();
    assert_eq!(ring.played(0), 16);
}

#[test]
fn paused_partial_descriptor_rebases_without_replaying_or_losing_frames() {
    let mut ring = Ring::default();
    for _ in 0..4 {
        ring.push(1024).unwrap();
    }
    ring.observe(1, true, 1024).unwrap();
    assert_eq!(ring.played(1024), 1536);
    assert_eq!(ring.rebase(1024).unwrap(), (1, 512));
    assert_eq!(ring.completed(), 1536);
    assert_eq!(ring.queued(), 3);
    assert_eq!(ring.played(1024), 1536);
    assert_eq!(ring.push(16), Ok(3));
    ring.observe(3, true, 0).unwrap();
    assert_eq!(ring.played(0), 4112);
    assert_eq!(ring.submitted(), 4112);
    assert_eq!(ring.rebase(0).unwrap().1, 0);
}
