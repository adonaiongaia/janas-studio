#!/usr/bin/env bash
set -euo pipefail

cd "$(dirname "$0")/.."
cargo test --manifest-path src-tauri/Cargo.toml dual_engine_harness -- --nocapture --test-threads=1 "$@"
