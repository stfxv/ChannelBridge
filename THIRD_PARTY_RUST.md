# Rust implementation: source and licenses

ChannelBridge's Rust application, DSP and virtual driver are new implementations,
licensed GPL-3.0-only. They do not compile, link or include the legacy Go
application or BlackHole C source in their source archive or audio path.
The repository retains the legacy implementation and its original notices for
the existing production release and rollback; see the repository's THIRD_PARTY.md.

Dependency versions and registry checksums are pinned in Cargo.lock. macOS access
uses objc2, its Foundation/AppKit/AVFoundation bindings and block2; configuration
uses serde/serde_json; Loom is used for development tests. The corresponding
source archive includes rust-vendor with the locked dependency sources and their
original license files, plus .cargo/config.toml for offline builds.

The original ChannelBridge monochrome icon is reused. No upstream BlackHole
icons or binaries are included. Rust and Apple's SDK are build prerequisites;
their toolchains are not bundled. The C files included under scripts are SDK ABI
and mock HAL verification programs, not product audio implementations.
