#!/bin/bash
set -euo pipefail
cd "$(dirname "$0")/.."
bash scripts/package-rust-production.sh
mkdir -p dist
ditto work/rust-production-dist/ChannelBridge.app dist/ChannelBridge.app
cp work/rust-production-dist/ChannelBridge-Rust.pkg dist/ChannelBridge-Rust.pkg
