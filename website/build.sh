#!/bin/sh
# Generates website/dist. Wrangler runs this before every deploy; the
# Workers Builds image has no Rust, so a missing toolchain is installed.
set -eu
cd "$(dirname "$0")/.."

# The pinned toolchain, without the Windows targets and lint components
# rust-toolchain.toml adds for the app.
RUSTUP_TOOLCHAIN=$(sed -n 's/^channel = "\(.*\)"/\1/p' rust-toolchain.toml)
export RUSTUP_TOOLCHAIN

if ! command -v rustup >/dev/null 2>&1; then
  curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs |
    sh -s -- -y --profile minimal --default-toolchain none
  . "$HOME/.cargo/env"
fi
rustup toolchain install "$RUSTUP_TOOLCHAIN" --profile minimal
cargo run --locked -p blinzel-site -- website
