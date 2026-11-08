# Back up, upgrade, and restore ChannelBridge

Build the production installer without installing it.

```sh
bash scripts/build.sh
```

The output is `dist/ChannelBridge-Rust.pkg`. It contains
`/Applications/ChannelBridge.app` and
`/Library/Audio/Plug-Ins/HAL/ChannelBridge.driver`, with the existing production
bundle IDs and device UID. The installer has no installation scripts and does
not restart CoreAudio. The app and driver have ad-hoc signatures; the installer
package is unsigned.

## Back up the installed version

1. Quit the running ChannelBridge app and record its actual path. The app may
   run from a location outside `/Applications`.
2. Copy that app and the installed
   `/Library/Audio/Plug-Ins/HAL/ChannelBridge.driver` into a new private backup
   directory. Preserve the bundles' files, permissions, and symbolic links.
3. Copy `~/Library/Application Support/ChannelBridge/settings.json` into the
   backup if the file exists. Record an absent settings file as absent.
4. Record the original paths, app and driver versions, and SHA-256 file hashes.
   Compare the backup files against the originals before replacement.

Back up the versions actually installed, including an existing Rust release.
Keep local paths and settings private. Retain the backup until you have verified
rollback with the saved app and driver.

## Upgrade and check audio

1. Open `dist/ChannelBridge-Rust.pkg` and authorize the macOS installer.
2. Restart the Mac so CoreAudio can load the installed driver.
3. Open `/Applications/ChannelBridge.app`. Check the saved input device,
   selected channels, mute, and automatic resume setting.
4. Verify that the intended input reaches **ChannelBridge Mic** in the receiver
   application. Check saved device references in applications such as Discord.

Keep only one HAL bundle with `ChannelBridge_UID` installed. A package receipt
or an on-disk plist does not identify the driver currently loaded by CoreAudio.
Check the live virtual microphone and actual audio.

## Restore the saved version

1. Quit ChannelBridge.
2. Restore the saved app and driver to their recorded paths. Authorize the HAL
   directory change through macOS when required.
3. To restore the saved configuration, copy the saved settings back to their
   recorded path. If settings were originally absent, remove the new settings
   file only if you want to return to that state.
4. Restart the Mac so CoreAudio can load the restored driver.
5. Verify the restored virtual microphone, selected inputs, and actual audio in
   the receiver application.

## Acceptance limits

Build and payload checks do not establish a successful upgrade or rollback.
Installer upgrade, settings restoration, uninstall, failed-install recovery,
receiver device references, and rollback require checks on the actual
installation. See [RUST_AUDIO_TEST.md](RUST_AUDIO_TEST.md) for audio acceptance
procedures.
