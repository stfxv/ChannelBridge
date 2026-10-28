# Rust implementation: source and licenses

ChannelBridge's Rust application, DSP and virtual driver are new implementations,
licensed GPL-3.0-only. They do not compile, link or include the legacy Go
application or BlackHole C source in their source archive or audio path.
The retired Go implementation and its original notices remain in
[Git history](https://github.com/stfxv/ChannelBridge/tree/f261f65d86d995b67b02942148f067d01365f148).
That snapshot includes [the legacy notices](https://github.com/stfxv/ChannelBridge/blob/f261f65d86d995b67b02942148f067d01365f148/THIRD_PARTY.md)
and [the original BlackHole license](https://github.com/stfxv/ChannelBridge/blob/f261f65d86d995b67b02942148f067d01365f148/vendor/blackhole/LICENSE).

Dependency versions and registry checksums are pinned in Cargo.lock. macOS access
uses objc2, its Foundation/AppKit/AVFoundation bindings and block2; configuration
uses serde/serde_json; Loom is used for development tests. The corresponding
source archive includes rust-vendor with the locked dependency sources and their
original license files, plus .cargo/config.toml for offline builds.

The original ChannelBridge monochrome icon is reused. No upstream BlackHole
icons or binaries are included. Rust and Apple's SDK are build prerequisites;
their toolchains are not bundled. The C files included under scripts are SDK ABI
and mock HAL verification programs, not product audio implementations.
