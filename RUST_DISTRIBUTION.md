# ChannelBridge Rust source distribution

This archive contains the independent Rust implementation, test app/driver build
scripts, original ChannelBridge icon, verification programs and locked dependency
sources. It excludes the legacy Go application and BlackHole implementation.
The project is GPL-3.0-only; see LICENSE and THIRD_PARTY_RUST.md.

Rust 1.98.1 and Apple's command-line developer tools are prerequisites. The
minimum supported Rust version is 1.85.0. Build on an Apple Silicon Mac with the
`aarch64-apple-darwin` Rust target. The minimum deployment target is macOS 13.

The included .cargo/config.toml selects rust-vendor for these commands.

```sh
cargo check --offline --locked -p channelbridge-app
bash scripts/test-rust.sh
bash scripts/build-rust-app.sh --build-only
bash scripts/package-rust-test-driver.sh
bash scripts/package-rust-test-app.sh
```

Run Cargo from the extracted directory so it discovers the included .cargo
configuration. The source verification script additionally uses Python 3 to check
archive contents and runs `cargo check` on all workspace targets with an empty
Cargo cache. It then builds an arm64 app from the extracted source, including
its vendored source archive, with network access disabled for Cargo. Pass the path to the original archive, which must still be available
after extraction.

```sh
bash scripts/verify-rust-source.sh /absolute/path/to/ChannelBridge-source.tar.gz
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

The app and driver bundles have ad-hoc signatures; the installer package is
unsigned. The Developer ID release procedure below applies to the production package.
See RUST_APP_TEST.md, RUST_HAL_TEST.md and RUST_AUDIO_TEST.md for test procedures
and the distinction between simulated and real-device results.
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
The driver check uses a fake HAL, not the running audio service. Build and payload
checks do not establish upgrade, rollback, or real-device acceptance. See the Developer ID release procedure below.

Generate the production package (without installing it) with
`bash scripts/package-rust-production.sh`. The output is
`work/rust-production-dist/ChannelBridge-Rust.pkg`. See RUST_MIGRATION.md for
backup, upgrade, rollback, and acceptance procedures.

## Style verification

The archive includes `RUST_STYLE.md`, stable formatting and Clippy configuration,
and the standalone syntax checker. Python 3 is required for source enumeration
and archive verification. The style gate works without Git.

```sh
bash scripts/check-rust-style.sh check
```

## Developer ID release

Set `CHANNELBRIDGE_APPLICATION_IDENTITY` and `CHANNELBRIDGE_INSTALLER_IDENTITY`
to the Developer ID Application and Developer ID Installer certificates with private
keys in the signing keychain. Run `bash scripts/package-rust-production.sh --developer-id`.
This produces `work/rust-production-dist/ChannelBridge-Developer-ID.pkg`. The
app uses the audio input entitlement under Hardened Runtime. The local build
command continues to generate the unsigned development installer.

For a public release, submit that signed package to Apple using a configured
notarytool keychain profile. After the service returns Accepted, staple the ticket
and verify the same package before distribution.

```sh
pkg=work/rust-production-dist/ChannelBridge-Developer-ID.pkg
xcrun notarytool submit "$pkg" --keychain-profile "$NOTARY_PROFILE" --wait
xcrun stapler staple "$pkg"
xcrun stapler validate "$pkg"
spctl --assess --type install --verbose "$pkg"
```

[Apple notarization workflow](https://developer.apple.com/documentation/security/customizing-the-notarization-workflow)
