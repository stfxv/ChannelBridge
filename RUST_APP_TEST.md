# Rust test app

Build the separate universal app on macOS:

```sh
bash scripts/build-rust-app.sh
```

The output is `work/rust-dist/ChannelBridgeRustTest.app`. Its bundle identifier is
`local.channelbridge.rust.app.test`; settings are saved separately under
`~/Library/Application Support/ChannelBridge Rust Test/settings.json`.
If that file is absent, the legacy settings are imported read-only. A malformed
file is displayed as an error rather than overwritten. The existing application
and driver are not replaced by this script.

The build verifies both architectures, an ad-hoc signature, the runtime bundle
identifier and microphone description, and non-visible UI/worker smoke checks.
Those checks do not request microphone access or open audio. For a build host
without audio devices, `--build-only` skips inventory-dependent UI smoke but
still verifies bundle metadata. CI uses this explicit build-only mode.

The app reuses the existing C monochrome icon. Its source archive includes
project sources, locked dependencies and their original license files. Extract
it and run `cargo check --offline --locked -p channelbridge-app` to check the
vendored dependency configuration; Rust and Apple's SDK must already exist.
The macOS deployment target is 13.0, but this is not proof of execution on macOS
13 or an Intel Mac. Developer ID signing/notarization is not completed.

To open the test UI manually:

```sh
open work/rust-dist/ChannelBridgeRustTest.app
```

Launching does not request microphone permission. The explicit permission button
may show Apple's permission dialog. Once authorized, a valid saved input selection
starts input preview; sending requires the send button. It requires the separate
Rust test HAL driver to be recognized. Do not treat a successful app build or
non-visible smoke as proof of HAL transfer, actual UI interactions, microphone
permission handling, or long-call stability.
