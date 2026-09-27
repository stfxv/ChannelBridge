# Third-party source

The virtual HAL driver is based on BlackHole, Copyright (C) 2019–2026 Existential Audio Inc., licensed under GPL-3.0.

Source: https://github.com/ExistentialAudio/BlackHole
Pinned commit: 62953f570329839e6188f02cc28934904147f617
Retrieved: 2026-09-27

`vendor/blackhole/BlackHole.c` is the original source. `vendor/blackhole/LICENSE` is its original license notice. `driver/Config.h` supplies separate branding, UID, channel and rate settings. `driver/compatibility.patch` records our changes, dated 2026-09-27, for UID translation validation and non-formatted product names. No upstream icons or compiled binaries are redistributed. ChannelBridge is an unofficial derivative, not an Existential Audio product.

The application and changes are GPL-3.0-only. The complete corresponding source and build scripts are included in this folder and in the app bundle's Resources/ChannelBridge-source.tar.gz.
