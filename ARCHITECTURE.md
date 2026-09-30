# Control and audio ownership

The AppKit main thread owns windows, menus, authorization prompts, and UI snapshots.
It enqueues user actions through `controlRunner.send`; it never starts or stops HAL I/O.
`cb_present` copies JSON before Go frees the string and schedules a main-run-loop block
in common modes, so snapshots continue during menu interaction.

One Go worker, pinned to an OS thread, owns the controller, configuration writes,
device inventory, and all HAL setup/teardown. Actions are ordered; the recovery clock
is independent of AppKit. Quit waits for this worker to release capture. Native APIs
other than atomic property callbacks and the real-time callback must stay on that worker.

`captureState` determines whether the view is running or monitoring. `Wanted` is the
user's send intent, kept separately so disconnects can recover without resuming an
explicitly stopped send. Unsupported formats block retry until selected-device metadata
changes or the user retries. Transient failures use bounded exponential backoff.

Device notifications mark a generation dirty. The worker enumerates on a generation
change, with a five-second fallback for missed/unsupported notifications. The GUI menu
signature only includes names, UIDs and channel counts; changing sample rate does not
rebuild open menus. Missing selected channels remain explicit disabled menu entries.

The HAL callback uses C float32 buffers only, with no Go callbacks, allocations, logging,
or mutexes. Mute is atomic and does not rebuild a route. Selected peaks use atomic
maximum/exchange. The overview scans physical channels only while its window is open,
with a fixed maximum of 512 meters. The Go controller handles three-second peak display
using elapsed time. No audio samples are saved or sent outside the local virtual device.

Configuration is validated and saved before any live change. A save failure keeps
the old route and model. The UI sends the full selected policy; unavailable channels
must never be silently replaced in the UI, controller, or native layer.

## Verification

Run `bash scripts/test.sh` and `bash scripts/build.sh`. Tests cover the control worker,
reconnection policy, save failure, mute without restart, all-channel C metering and
real AppKit selection behavior. `scripts/test_ui.m` overrides authorization checking;
tests must never request microphone access or start real audio I/O.

Hardware-specific acceptance remains separate: RME rate changes, SMUX channel changes,
sleep/wake, hotplug, and receiver apps must be tested with actual hardware. See README.
