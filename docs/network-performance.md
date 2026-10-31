# Network receive performance

The first HTTP receive bottleneck was TCP receive credit. The e1000 driver
reported `max_burst_size = Some(1)` despite having a 32-descriptor RX ring.
smoltcp 0.12.0 uses that capability to cap the advertised TCP window. The
captured SYN window was 1,462 bytes; the largest captured window was 1,474 bytes.
Together with caller-driven service and approximately 55 ms sleeps, this made
large responses advance in small bursts separated by ACK/polling delays.

## Measured change

| Measurement | Before | After |
| --- | --- | --- |
| Local 128 KiB GET, discarded output, round 1 | 185 PIT ticks (~10.16 s) | 3 ticks (~0.165 s) |
| Local 128 KiB GET, discarded output, round 2 | 184 ticks (~10.11 s) | 2 ticks (~0.110 s) |
| Largest advertised TCP window in capture | 1,474 bytes | 32,768 bytes |
| First delivery of local body, wire spans | 9,941 / 9,942 ms | 3 / 41 ms |
| Portal `curl http://google.com/`, including redirect/output | 7.18 s, HTTP 200, 87,420 bytes | 0.96 s, HTTP 200, 87,447 bytes |

A separate host direct HTTP request took 0.20 s, including one redirect.
Internet measurements are individual observations, with different Google
backends and slightly different response bodies. They are not a controlled
estimate of Internet latency. Local transfers use identical 131,072-byte bodies,
two sequential rounds and a checked byte sum of 15,728,640. Both cases check
socket counts after every request. Guest tick measurements have about 55 ms
resolution; packet captures provide finer wire timestamps.

The decoder distinguishes first delivery from TCP retransmissions. Counting a
late retransmission as body delivery would exaggerate transfer duration, so
synthetic capture tests verify that distinction. Captures still record late
retransmissions after socket cleanup; these measurements do not establish
graceful TCP shutdown.

## Implementation

- [e1000 capabilities](../src/drivers/net/e1000_caps.rs) now advertise the usable
  ring capacity, 31 descriptors, leaving the legacy ring's head/tail separation.
- [TCP sockets](../crates/net-stack/src/lib.rs) allocate a 32 KiB receive buffer,
  with the existing 4 KiB transmit buffer. The advertised window fits a burst
  within the usable DMA ring. The wire regression checks both a useful window
  and this upper bound using the real driver capability profile.
- Socket allocation remains fallible and quotas remain four per PID / sixteen
  globally. Each live TCP endpoint needs 28 KiB more heap memory. At sixteen TCP
  endpoints that is an extra 448 KiB, allocated on open and released on cleanup.
- Ingress remains bounded to eight frames per service call. Driver DMA ownership,
  descriptor counts, packet copies and IRQ masks were unchanged by this window
  optimization. The service remains serialized on the native SMP platform.
  No busy polling or background network thread was introduced.

The NIC already uses DMA. Its pair of 32-entry rings and 64 packet buffers use
66 physical frames (264 KiB) reserved at boot. DMA moves Ethernet frames between
the device and those buffers. smoltcp processing and copies into owned socket
and user records still run on the CPU. More DMA or multithreading would not
remove the receive-window limit identified here.

## Reproduction and red/green evidence

```bash
./tools/build-grub-disk.sh
python3 -B e2e/net_perf.py --label current --assert-fast
# Optional, separate Internet observation:
python3 -B e2e/net_perf.py --label public --assert-fast --site http://google.com/
```

The [benchmark](../e2e/net_perf.py) uses independent host TCP sockets, a private
disk copy, a private QMP socket and the actual Ring 3 SDK. It preserves
`results.json`, `serial.log` and `ethernet.pcap` under `target/net-perf-<label>/`.
Its `--assert-fast` gate requires each 128 KiB transfer to complete within 25 PIT
ticks. The baseline failed this unchanged gate at 185/184 ticks; optimized runs
passed at 3/2 ticks. The initial fixture discovery error was corrected before
recording the isolated `baseline-clean` comparison; it is not performance evidence.

- Before: `target/net-perf-baseline-clean/`, `/tmp/rustos-net-perf-before.log`.
- After: `target/net-perf-window/`, `/tmp/rustos-net-perf-window.log`.
- Repeated local verification with the final analyzer: `target/net-perf-verified/`,
  `/tmp/rustos-net-perf-verified.log`, again 3/2 ticks with byte sums checked.
- Wire regression red: `/tmp/rustos-net-window-red.log`, advertised only 1,462 bytes.
- Wire regression green: `/tmp/rustos-net-window-green.log`, all five stack tests pass.
- Allocation fault injection: `/tmp/rustos-net-perf-allocation.log`, ICMP/UDP/TCP
  return NoResources and recover after allocator failure.
- Packet analyzer regressions: four cases in
  [host tooling tests](../tools/tests/test_net_perf.py), including direction,
  timing, retransmission deduplication and truncated captures.

Both kernel boot builds, release build, all 61 unit cases and applicable kernel
integration executables are checked alongside HTTP, Internet ping, no-NIC,
Ring 3 socket ownership/lifecycle, repeated ring wrap and desktop/input/CPU
regressions. Logs use `/tmp/rustos-net-perf-*.log`. Existing debug heap stress and
process frame/stack reclamation limitations remain in
[networking validation](networking-validation.md). Host GUI and physical NIC
behavior are not verified by these headless QEMU runs. Packet captures, checked
bytes and timings establish throughput; a desktop screenshot cannot show these
flow-control or latency measurements.

## Next improvements to measure

1. NIC event delivery and socket readiness waits. Masked NIC IRQs and the 18.2 Hz
   PIT still impose coarse polling delays. A bounded IRQ handler could acknowledge
   events and wake a kernel worker, which would service protocols in ordinary
   context and wake waiting processes. Preserve timer/lock order and test exit,
   kill, readiness races and absence of traffic. Do not process allocating protocol
   work inside the IRQ handler.
2. Flush egress after application queue mutations. Today the syscall progress pass
   precedes connect/send/recv; later calls publish new data and window changes.
   Measure latency for callers that sleep immediately after send before changing
   the bounded-work contract.
3. A bounded userspace DNS cache and reusable HTTP connections for repeated hosts.
   Respect TTLs and peer/redirect policy; compare cold and warm requests separately.
4. Larger application I/O records or fewer copies only after profiling CPU costs.
   DMA ownership and process mapping checks must remain explicit. HTTP compression
   is another userspace option for large textual responses.

The current OS supports 1–32 QEMU CPUs; the network service uses one lock. Additional
application threads do not enlarge a TCP window or make locked protocol work
parallel. A single event-driven network worker is a useful next architecture step.
Per-connection concurrency must preserve synchronization and resource ownership;
this benchmark does not establish a benefit from it.
