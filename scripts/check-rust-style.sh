#!/bin/bash
set -euo pipefail
root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
source "$root/scripts/rust-env.sh"
mode="${1:-check}"
if [[ $# -gt 0 ]]; then shift; fi
case "$mode" in
    check|inventory|fix-imports) ;;
    *) echo "usage: $0 [check|inventory|fix-imports] [FILE.rs ...]" >&2; exit 2 ;;
esac
if [[ $# -eq 0 ]]; then
    while IFS= read -r path; do
        set -- "$@" "$root/$path"
    done < <(git -C "$root" ls-files '*.rs')
fi
cargo run --quiet --locked --bin channelbridge-rust-style --manifest-path "$root/tools/rust-style/Cargo.toml" -- "$mode" "$@"
if [[ "$mode" == check ]]; then
    cargo fmt --manifest-path "$root/Cargo.toml" --all -- --check
    cargo fmt --manifest-path "$root/tools/rust-style/Cargo.toml" -- --check
fi
