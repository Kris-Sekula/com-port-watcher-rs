#!/bin/sh
# Build the Rust COM Port Watcher on Linux -> target/release/com-port-watcher-rs
set -e
cd "$(dirname "$0")"
command -v cargo >/dev/null 2>&1 || { echo "cargo not found. Install Rust from https://rustup.rs first."; exit 1; }
for pc in gtk+-3.0 ayatana-appindicator3-0.1; do
  pkg-config --exists "$pc" 2>/dev/null || {
    echo "Missing build dependency: $pc"
    echo "Debian/Ubuntu: sudo apt install libgtk-3-dev libayatana-appindicator3-dev libxdo-dev pkg-config"
    exit 1
  }
done
cargo build --release
echo
echo "Done: target/release/com-port-watcher-rs"
ls -lh target/release/com-port-watcher-rs
