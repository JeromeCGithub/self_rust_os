# P6: native PCM output and userspace WAV playback

P6 is **in progress**. The OS now provides a real bounded PCM output device, and
`jplay` is an ordinary Ring3 WAV player with Play/Pause/seek/progress controls.
The browser still lacks HTML audio/video elements, compressed video/audio codecs,
MSE and A/V synchronization; this is not YouTube playback or completion of P6.

## Responsibility and ownership

- Kernel: optional Intel82801AA AC97 PCI discovery, I/O validation,33 boot-owned
  DMA frames,32 output descriptors, exclusive process-owned leases, complete
  user-buffer copies and close/exit/kill cleanup. No player/codec/URL policy.
- SDK: owned `audio::Stream`, typed errors, nonblocking writes, status, pause and
  resume. Drop requests close. Failed hardware is quarantined rather than reusing
  buffers while a device might still own them.
- Userspace: bounded RIFF/WAVE parsing in `crates/media`; file loading, playback
  queue filling, seeking, window input and painting in `user_programs/jplay`.
  No application imports kernel internals or sees a DMA physical address.

The initial output profile is fixed stereo signed16-bit little-endian48kHz.
One nonblocking call copies up to4096 bytes/1024 complete PCM frames into one
free descriptor.32 descriptors bound queued audio to32768 frames/about683ms.
The service never retains a user reference. All DMA addresses are aligned reserved
RAM below4GiB, never user-mapped; per-stream opens allocate no DMA frames. Those
33 device-lifetime frames are permanently reserved/reused, not a claim that they
are returned after every playback.

Lock order is scheduler metadata then audio; no audio operation calls scheduler
while holding its mutex, and no IRQ touches the device/service. PCI configuration
pairs are boot-exclusive or serialized under kernel entry during quarantine.
Register polling is bounded (10000 attempts for stop/reset, four for a coherent
cursor snapshot). CPU-owned stores are fenced before the LVI doorbell; the mutex,
not the fence, protects multi-field state across CPUs. Audio progress is polled
by callers; there is no new IRQ or background mixer thread.

Pause stops DMA, observes the remaining stereo samples, and rebases the quiesced
queue before resuming. This preserves a partial descriptor even when a controller
starts at its prefetched index after RUN. Repeated writes do not rewrite RUN on an
already running queue. Status counts consumed PCM frames, including the current
partial descriptor; submitted positions are never described as playback position.
End of queue is not an underrun. The underrun field counts hardware FIFO errors.

Opaque handles are assigned by a monotonic lease counter and checked
against current PID. Stale/foreign control fails. Owner exit/kill stops DMA before
process reclamation, including owned descendant teardown. A reset failure disables
bus mastering, leaves DMA storage reserved and rejects reuse until reboot. Hardware
reset-failure/high-memory allocation rollback were audited, not fault-injected.
See the exact [syscall contract](syscalls.md#pcm-audio-output).

## Userspace player

`jplay /path/to/clip.wav` reads a regular file with a16MiB cap, validates all RIFF
sizes/chunk arithmetic, padding, format and complete stereo frames, and borrows
samples without a decoder copy. Unknown metadata chunks are skipped within the
container; duplicate format/data chunks and unsupported rates/channel widths fail.
Only PCM48kHz/stereo16-bit WAVE is currently supported. No MP3/AAC/Opus/VP9/H.264,
resampler, volume mixer, video or network media streaming is supplied.

The player refills at most eight chunks per event-loop iteration, so queue work
is bounded. Pause preserves the stream; seek closes it and starts from a checked
clip frame, keeping device counters separate from the clip position. Close drops
the stream/file owners. Desktop launch defaults to `/HOME/TONE.WAV` if present;
otherwise the window explains that a supported file is needed. No test tone is
silently installed on a user's disk. The test runner installs its owned two-second
fixture only into a disposable image.

For a host audio backend, the platform accepts an explicit controller/backend:

```text
-audiodev pa,id=sound -device AC97,audiodev=sound
```

Select a backend actually available on your host. The current ordinary desktop
launcher does not add a sound device automatically. The E2E supplies AC97 with a
WAV capture backend. No microphone/input, real-hardware or manual listening check
was performed. Captured non-silent PCM proves guest output, not host speakers.

## Evidence and next gates

The [P6 evidence index](browser-p6/README.md) records red/green cases, native1/8/32
CPU positions/ownership/frame recovery, exact captured sample counts, the actual
player's pause/seek screenshots and recorded sound, kernel checks and remaining
manual gaps. Screenshots alone are not audio proof.

Next P6 work is demux/codec and HTML media-element integration, network ranges,
buffering/seek, user-gesture policy, media events and synchronized video. P7 must
still exercise a real YouTube watch page with sound/controls; P8 must retain this
failure until the complete public task passes. Google search results and Facebook
sessions remain independent earlier gates, not consequences of PCM support.

Reference targets checked2026-11-06:
[Intel AC97 register manual](https://www.intel.com.br/content/dam/doc/manual/io-controller-hub-7-hd-audio-ac97-manual.pdf),
[QEMU AC97 device source](https://github.com/qemu/qemu/blob/v9.2.0/hw/audio/ac97.c)
and [QEMU audio invocation](https://www.qemu.org/docs/master/system/qemu-manpage.html).
