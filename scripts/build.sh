#!/bin/bash
set -euo pipefail
cd "$(dirname "$0")/.."
bash scripts/package-rust-production.sh
mkdir -p dist
rm -rf dist/ChannelBridge.app
ditto work/rust-production-dist/ChannelBridge.app dist/ChannelBridge.app
codesign --verify --strict dist/ChannelBridge.app
cp work/rust-production-dist/ChannelBridge-Rust.pkg dist/ChannelBridge-Rust.pkg
