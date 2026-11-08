# ChannelBridge Rust architecture

ChannelBridge selects inputs from a multichannel CoreAudio device and sends
float32 stereo PCM to its own **ChannelBridge Mic** virtual microphone. The
production application and HAL driver are Rust implementations. `scripts/build.sh`
and `scripts/test.sh` use this Rust workspace.

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
route resumes input preview only by default. The persisted `auto_send_on_resume`
option may request sending at the next system wake, confirmed selected-input
return, or live-session health recovery, including after a manual Stop. It defaults
to false. Enabling it, changing input selection, and startup with an already
connected input do not request sending. Manual intent takes precedence over
automatic recovery detected in the same worker turn. Start/Stop does not recreate
the IOProcs.
Settings are validated and saved before the committed model or live route changes.
Unavailable selected channels remain explicit selections and are never replaced
with another channel automatically. Input inventory preserves usable choices
alongside a warning if individual devices cannot be read. A partial inventory can
prove that a selected UID is present, but cannot prove absence. A whole inventory
failure keeps the previous display and presence evidence.

Session creation first checks microphone permission and the live driver's
diagnostic protocol/initialization. It follows the physical source's nominal
sample rate by requesting a change on the virtual device only, then waits for
the nominal rate and every virtual/physical stream format to agree. Negotiation
is polled with a three-second deadline. A prepared session allocates its DSP,
FIFO, meters, and callback storage before registration. Each IOProc disables
its unused direction, then output and capture start with sending disabled.

Transient failure closes the old session and schedules a retry with exponential
backoff from one to thirty seconds. Successful opening resets the backoff.
Unknown teardown errors, active callbacks, or callback panics block new
registration. A removed-device `!dev` error may permit retry after callback
admission is closed and no handler remains active. Unconfirmed Destroy retains
its permanently disabled allocation, counted by `retained_io_contexts`, instead
of freeing storage that HAL might still reach. Both directions are cleaned up
even if the first fails.
Quit seals command admission and is asynchronous for AppKit. The worker closes
audio before attempting the remaining accepted configuration saves in FIFO order.
This persistence-only path cannot reopen or route audio. Final completion includes
configuration acknowledgements and failures. AppKit waits for that snapshot before
completing application termination. Completion has a separate atomic state and
is published only after backend/session owners have been destroyed. Poisoned
synchronization state is never inspected or repaired: admission closes, snapshot
reads return a blocked fallback, and the worker drains the accepted finite prefix
without waiting on an unavailable sender. Fallback configuration is initial
presentation metadata, and fallback counters do not establish observed telemetry.

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
the total physical-to-receiver latency. Each FIFO publication carries its offered
capture packet length alongside its samples and generation. The output keeps the
largest packet observed for the current generation. Its effective steady target
is at least that packet plus interpolation lookahead and one frame. Startup also
reserves the current complete output callback at the maximum permitted resampling
step. These reserves prevent output from exhausting a large packet before the
next packet arrives, even when output callbacks are smaller than capture callbacks.
Priming moves PCM into prepared interpolation history, so a reserve larger than
the FIFO can accumulate across capture callbacks without deadlock.

Packet growth raises the target without resetting running interpolation or clock
correction. A generation change clears the learned packet maximum. Old samples
and samples beyond a future-generation barrier do not raise the current target.
History is allocated before registration for the reserve and a complete packet
crossing the startup threshold. Partial FIFO writes still report their rejected
suffix; their reserve is bounded by FIFO capacity. A first unseen longer capture
interval can still cause a genuine shortage before its packet size is learned.
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

Audio callbacks use preallocated storage and atomic observations. App-side DSP
accepts at most 4096 frames per callback and 512 input channels. These data limits
do not guarantee a worst-case callback duration. Normal processing does not
allocate, log, wait, or take mutexes. Native callback
adapters reject concurrent DSP reentry. Native entry guards catch unexpected Rust panics as defense in depth when
unwinding is enabled. Buffer bounds, lifetimes, exclusive access, and ordinary
error reporting remain independent of those guards. Configuration I/O, enumeration, recovery, and diagnostic
serialization remain outside the audio callbacks. No sample waveforms are
recorded or sent outside the local virtual device.

## Verification and remaining acceptance

`bash scripts/test.sh` runs formatting, Clippy, workspace tests in test and
production profiles, DSP/native-adapter allocation checks, finite Loom models,
the release filter-response grid, SDK ABI comparison, and a mock HAL driver test
with a C ASan/UBSan harness. Actual PCM packet schedules cover different capture
and output blocks, callback phases, unequal rates, generation isolation, and
sustained packet growth. A fake-audio AppKit sleep/wake notification test
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
acceptance checks. See [README.md](README.md), [RUST_AUDIO_TEST.md](RUST_AUDIO_TEST.md), and
[RUST_HAL_TEST.md](RUST_HAL_TEST.md) for recorded evidence and its limits.
