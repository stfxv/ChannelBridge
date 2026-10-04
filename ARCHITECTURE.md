# ChannelBridge Rust architecture

ChannelBridge selects inputs from a multichannel CoreAudio device and sends
float32 stereo PCM to its own **ChannelBridge Mic** virtual microphone. The
production application and HAL driver are Rust implementations. The older Go
implementation is retained for reference and separate legacy builds; it is not
the architecture described below.

## Workspace responsibilities

| Crate | Responsibility |
| --- | --- |
| `app` | AppKit windows, menu-bar item, input selectors/meters, microphone authorization UI, diagnostic export, and application lifecycle notifications. |
| `core` | Saved routing policy, atomic configuration persistence, the control state machine, bounded UI commands, owned display snapshots, and diagnostic export data. No HAL calls or unsafe code. |
| `macos` | CoreAudio device/stream inspection, rate negotiation, microphone authorization checks, IOProc ownership, native buffer adapters, and session health observations. |
| `audio` | Channel routing, dry input metering, SPSC FIFO, source-clock estimation, windowed-sinc interpolation, drift correction, and the virtual driver's timeline ring/clock. |
| `contract` | Production/test identifiers, supported virtual sample rates, and the versioned app/driver diagnostic protocol. No unsafe code. |
| `hal-abi` | Rust declarations matching the Apple HAL ABI, including stream formats, timestamps, and the driver/host function tables. |
| `driver` | Independent HAL plug-in, properties and client lifetime, virtual clock/rate changes, and timestamped stereo transfers. It must not call the app-side CoreAudio client APIs. |

The `production` feature chooses the existing production device UID, bundle IDs,
and settings directory through `contract::DEPLOYMENT`. Test builds use separate
identifiers and settings. They can import production preferences read-only when
test preferences do not yet exist. Corrupt destination preferences are reported
without silently falling back to another file.

## Control and session ownership

The AppKit main thread owns all UI objects and authorization prompts. It sends
commands to `core::worker::Worker` and renders owned `Snapshot` values. It does
not register, start, stop, or destroy audio IOProcs. A timer in common run-loop
modes keeps snapshots updating during menu interaction.

One dedicated Rust control worker constructs and owns the native backend and
`Controller`, including pending rate negotiation and the current session. It
checks control/session health every 100 ms and refreshes device inventory and
driver diagnostics every second. The command queue is bounded to 32 entries;
Start/Stop intent, Quit, and system lifecycle notifications have separate atomic
paths so a full settings queue cannot prevent stopping or wake recovery. UI
bursts have a finite drain budget. Coalesced sleep/wake signals preserve the
latest power state; even a wake without a preceding sleep retires the old route.

The controller keeps the user's `wanted_send` intent separate from connection
state. A disconnected sending route can resume after recovery; a user-stopped
route resumes input preview only. Start/Stop does not recreate the IOProcs.
Settings are validated and saved before the committed model or live route changes.
Unavailable selected channels remain explicit selections and are never replaced
with another channel automatically.

Session creation first checks microphone permission and the live driver's
diagnostic protocol/initialization. It follows the physical source's nominal
sample rate by requesting a change on the virtual device only, then waits for
the nominal rate and every virtual/physical stream format to agree. Negotiation
is polled with a three-second deadline. A prepared session allocates its DSP,
FIFO, meters, and callback storage before registration. Each IOProc disables
its unused direction, then output and capture start with sending disabled.

Transient failure closes the old session and schedules a retry with exponential
backoff from one to thirty seconds. Successful opening resets the backoff. A
session whose callback teardown cannot be confirmed blocks new registration;
Stop/Destroy failures retain callback storage to avoid freeing memory still
reachable by HAL. Both directions are cleaned up even if the first fails.
Quit is asynchronous for AppKit and waits for the worker's completion snapshot
before completing application termination.

## Sleep, wake, and progress monitoring

AppKit workspace sleep/wake notifications are lifecycle inputs to the control
worker. Sleep suspends connection attempts and releases the current session
while preserving `wanted_send`, selected channels, and mute. Wake clears stale
negotiation/retry state and re-enumerates, re-negotiates, and opens a fresh session.
The workspace observer only publishes atomic signals, including when a
notification arrives off the main thread; HAL teardown and opening remain on
their owner thread. Screen wake requests an immediate health check and inventory
refresh without interrupting a healthy route or undoing system-sleep suspension.

Lifecycle notifications complement the native health check. Health observations
compare the selected device IDs, rates, and stream formats and check both audio
directions for callback failure or stalled progress. Capture and output progress
are tracked independently. An unrelated callback continuing must not conceal a
stalled direction. Rejected buffers or callbacks that keep arriving without
usable audio must not count as a recovered stream. Silence, mute, and stopped
input preview remain valid operating states; recovery must not use amplitude as
an indication that a callback is healthy.

## Audio data flow and real-time rules

The physical input IOProc validates native float buffers and timestamps, updates
dry selected-channel meters, and applies one of three policies: mono duplicates
the selected input, stereo preserves the selected left/right pair, and mix sends
their average to both outputs. Overview metering scans all physical inputs only
while requested, with an upper bound of 512 channels.

Selected stereo frames cross a bounded single-producer/single-consumer FIFO.
The virtual output IOProc uses the observed source-clock speed and queue/history
fill level to adjust a windowed-sinc interpolator smoothly. The provisional
input target is 16 ms, with a minimum of 256 frames; this does not establish
the total physical-to-receiver latency. Startup waits for the safety margin.
Real shortages preserve valid output, fade the boundary, emit silence for the
missing frames, and re-prime interpolation history.

Routing is published as one atomic snapshot. Stop/Start, source timestamp
discontinuities, and FIFO overflow advance its generation; older FIFO frames
and interpolation history cannot cross into the new stream. Mute consumes
audio normally and changes gain over 64 frames, avoiding a stale audio backlog
on unmute. Input levels remain available before sending and while muted.

The app writes rendered stereo PCM to the virtual driver's output stream. The
driver stores it by absolute sample frame and generation in an atomic timeline
ring. Receiver applications read the matching frames from the input stream;
missing or invalid positions produce silence. Rate changes and the first
active driver client reset the virtual clock and invalidate old generations.
Multiple readers do not consume or erase each other's frames.

Audio callbacks use preallocated storage, bounded work, and atomic observations;
normal processing does not allocate, log, wait, or take mutexes. Native callback
adapters reject concurrent DSP reentry. Rust panics are contained before an FFI
callback returns to HAL. Configuration I/O, enumeration, recovery, and diagnostic
serialization remain outside the audio callbacks. No sample waveforms are
recorded or sent outside the local virtual device.

## Verification and remaining acceptance

`bash scripts/test.sh` runs formatting, Clippy, workspace tests in test and
production profiles, DSP/native-adapter allocation checks, finite Loom models,
the release filter-response grid, SDK ABI comparison, and a mock HAL driver test
with a C ASan/UBSan harness. A fake-audio AppKit sleep/wake notification test
runs in both profiles. Broader AppKit smoke/bundle tests and Universal packaging
have separate scripts. `bash scripts/build.sh` creates distribution artifacts;
it does not install them or restart the audio service. Sleep notifications are
asynchronous worker signals; completion of HAL teardown before the OS suspends
the worker is not guaranteed. Wake always retires a still-existing session.

Automated state-machine and watchdog tests verify the recovery policy with
controlled time and backend observations. They do not prove that a physical
device releases/reacquires streams correctly after actual Mac sleep. Real
sleep/wake, disconnect/reconnect, physical sample-rate or SMUX channel changes,
receiver applications, long-running audio, and Intel hardware remain separate
acceptance checks. See [README.md](README.md), [RUST_AUDIO_TEST.md](RUST_AUDIO_TEST.md),
[RUST_HAL_TEST.md](RUST_HAL_TEST.md), and [RUST_PROGRESS.md](RUST_PROGRESS.md) for
recorded evidence and its limits.
