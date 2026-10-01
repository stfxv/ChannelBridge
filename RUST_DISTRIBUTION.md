# ChannelBridge Rust source distribution

This archive contains the independent Rust implementation, test app/driver build
scripts, original ChannelBridge icon, verification programs and locked dependency
sources. It excludes the legacy Go application and BlackHole implementation.
The project is GPL-3.0-only; see LICENSE and THIRD_PARTY_RUST.md.

Rust 1.98.1 and Apple's command-line developer tools are prerequisites. The
minimum supported Rust version is 1.85.0. Install the arm64 and x86_64 Apple targets
before building universal binaries. The deployment target is macOS 13; build
success is not proof of execution on every supported OS/architecture.

The included .cargo/config.toml selects rust-vendor, enabling:

```sh
cargo check --offline --locked -p channelbridge-app
bash scripts/test-rust.sh
bash scripts/build-rust-app.sh --build-only
bash scripts/package-rust-test-driver.sh
bash scripts/package-rust-test-app.sh
```

Run Cargo from the extracted directory so it discovers the included .cargo
configuration. The source verification script additionally uses Python 3 to check
archive contents and builds all workspace targets with an empty Cargo cache:

```sh
bash scripts/verify-rust-source.sh
```

The app and installer are produced under work/rust-dist. These are separately
named development artifacts with test bundle IDs and a test device UID. Building
does not replace the existing application, install a driver or request microphone
access. Opening the installer requires the user's macOS administrator approval.
The driver may require a normal Mac restart before CoreAudio loads an update.

The combined ChannelBridge-Rust-Test.pkg contains the separately named test app
under /Applications and the test driver under /Library/Audio/Plug-Ins/HAL.
It preserves the production app/driver namespaces. See RUST_APP_TEST.md for the
build and payload checks, installation status and remaining acceptance tests.

Signing is ad-hoc; Developer ID signing and notarization are not verified.
See RUST_APP_TEST.md, RUST_HAL_TEST.md, RUST_AUDIO_TEST.md and RUST_PROGRESS.md
for test procedures and the distinction between simulated and real-device results.
Real rate-following and long-call acceptance are not implied by this archive.

## Production profile build (not installation)

```sh
bash scripts/build-rust-driver.sh --production
bash scripts/build-rust-app.sh --production
bash scripts/verify-rust-source.sh work/rust-production-dist/ChannelBridge.app/Contents/Resources/ChannelBridge-source.tar.gz --production
```

These commands generate ChannelBridge.app and ChannelBridge.driver under
work/rust-production-dist using a separate Cargo target directory. The production
feature selects the existing production IDs and settings directory. The default
build continues to select the separate test namespace. Neither build installs
anything. The app smoke test opens no audio and requests no microphone permission.
The driver check uses a fake HAL, not the running audio service. Production
installation, rollback, signing and real-device acceptance remain incomplete.

Generate the production package (without installing it) with
`bash scripts/package-rust-production.sh`. The verified output is
`work/rust-production-dist/ChannelBridge-Rust.pkg`. See RUST_MIGRATION.md for
replacement prerequisites, retained legacy artifacts and rollback acceptance.
