# Rust production migration and rollback gates

The production installer is generated locally by:

```sh
bash scripts/package-rust-production.sh
```

It contains only `/Applications/ChannelBridge.app` and
`/Library/Audio/Plug-Ins/HAL/ChannelBridge.driver`, with the existing production
bundle IDs and device UID. It has no installation scripts and does not restart
CoreAudio. Build and payload verification do not establish actual migration success.
Do not install before the mandatory real-device acceptance gates in
RUST_REWRITE_PLAN.md are satisfied. The package is currently ad-hoc signed.

Before replacement, preserve the actual v0.4.0 app, v0.1.0 driver and the user's
`~/Library/Application Support/ChannelBridge/settings.json` in a private backup.
Record their versions and file hashes. The read-only source backup helper validates
those legacy versions and rejects an existing destination:

```sh
python3 scripts/backup-rust-migration.py --app /absolute/path/ChannelBridge.app --destination /absolute/path/private-backup
```

The destination directory is private (0700); settings and manifest are 0600.
It verifies copied bundle entries, modes, targets and hashes against the source,
checks settings bytes and confirms sources did not change during copying. The
manifest records original paths for restoration. Keep it private; it contains
local paths and settings in the backup, and is not a public release artifact.
 The currently running legacy app may live
outside `/Applications`; its actual location must be recorded and it must be quit
before switching versions. Keep the backup until rollback has been tested.

For rollback, quit the Rust app, restore the saved legacy app and driver to their
original locations with macOS administrator authorization for the HAL directory,
and restore settings only if they have changed and the user wants the saved state.
A normal Mac restart may be required to load the restored driver. Never leave two
HAL bundles with ChannelBridge_UID installed simultaneously. A package receipt or
on-disk plist is not proof of the currently loaded version. Verify the restored
virtual microphone and actual audio before declaring rollback successful.

Pending acceptance: actual installer upgrade, saved device references in Discord,
settings backup/restoration, uninstall, failed-install recovery and rollback.
No production replacement or rollback has been performed by these build scripts.
