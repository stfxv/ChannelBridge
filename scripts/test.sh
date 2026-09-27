#!/bin/bash
set -euo pipefail
cd "$(dirname "$0")/.."
mkdir -p work/test
CGO_CFLAGS='-O2 -fobjc-arc' go test -race ./...
CGO_CFLAGS='-O2 -fobjc-arc' go vet ./...
clang -std=c11 -Wall -Wextra -Werror -fsanitize=address,undefined scripts/test_mixer.c -o work/test/mixer
work/test/mixer
cp vendor/blackhole/BlackHole.c work/test/ChannelBridge.c
patch -s work/test/ChannelBridge.c driver/compatibility.patch
clang -O1 -g -fblocks -fsanitize=address,undefined -Iwork/test -include driver/Config.h \
    scripts/test_driver.c -framework CoreAudio -framework CoreFoundation -framework Accelerate -o work/test/driver
work/test/driver
