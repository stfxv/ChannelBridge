#!/bin/bash
# Validate the locally built archive and rebuild using its vendored dependencies.
set -euo pipefail
source "$(dirname "$0")/rust-env.sh"
cd "$CHANNELBRIDGE_ROOT"
archive="${1:-$CHANNELBRIDGE_ROOT/work/rust-dist/ChannelBridgeRustTest.app/Contents/Resources/ChannelBridge-source.tar.gz}"
if [[ "$archive" != /* ]]; then archive="$CHANNELBRIDGE_ROOT/$archive"; fi
python3 - "$archive" <<'PY'
import pathlib, sys, tarfile
allowed = {'Cargo.toml', 'Cargo.lock', 'rust-toolchain.toml', 'LICENSE',
           'THIRD_PARTY_RUST.md', 'ARCHITECTURE.md', 'README.md', '.cargo',
           'crates', 'scripts', 'packaging', 'rust-vendor'}
with tarfile.open(sys.argv[1]) as archive:
    names = set()
    for member in archive:
        name = pathlib.PurePosixPath(member.name)
        if not name.parts:
            continue
        assert not name.is_absolute() and '..' not in name.parts, member.name
        top = name.parts[0]
        assert top in allowed or (top.startswith('RUST_') and top.endswith('.md')), member.name
        names.add(str(name))
    for required in ['Cargo.lock', 'README.md', 'THIRD_PARTY_RUST.md',
                     '.cargo/config.toml', 'scripts/build-rust-app.sh',
                     'scripts/package-rust-source.sh', 'scripts/verify-rust-source.sh',
                     'scripts/test-rust.sh']:
        assert required in names, required
    print('Rust archive roots and required sources verified')
PY
mkdir -p work/test/rust
stage="$(mktemp -d "$CHANNELBRIDGE_ROOT/work/test/rust/source-verification.XXXXXX")"
trap 'rm -rf "$stage"' EXIT
mkdir -p "$stage/source" "$stage/empty-cargo"
tar -xzf "$archive" -C "$stage/source"
export CARGO_HOME="$stage/empty-cargo"
cd "$stage/source"
cargo check --offline --locked --workspace --all-targets
printf 'Archive rebuilt offline with an empty Cargo cache\n'
