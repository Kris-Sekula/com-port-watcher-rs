#!/bin/sh
# Build the Rust COM Port Watcher on macOS -> target/release/com-port-watcher-rs
set -e
cd "$(dirname "$0")"
[ "$(uname -s)" = "Darwin" ] || { echo "This script must run on macOS."; exit 1; }
command -v cargo >/dev/null 2>&1 || { echo "cargo not found. Install Rust from https://rustup.rs first."; exit 1; }
cargo build --release
echo
echo "Done: target/release/com-port-watcher-rs"
ls -lh target/release/com-port-watcher-rs
