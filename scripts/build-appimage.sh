#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"

# This workspace carries a local Rust/Node toolchain and compile-time metadata
# stubs because the host does not provide the GTK/WebKit development packages.
if [[ -d "$ROOT/.tools/rustup" ]]; then
  export RUSTUP_HOME="$ROOT/.tools/rustup"
  export CARGO_HOME="$ROOT/.tools/cargo"
  TOOLCHAIN_BIN="$ROOT/.tools/rustup/toolchains/stable-x86_64-unknown-linux-gnu/bin"
  if [[ -d "$TOOLCHAIN_BIN" ]]; then
    export PATH="$TOOLCHAIN_BIN:$PATH"
  fi
fi

if [[ -d "$ROOT/.tools/link-libs" ]]; then
  export LIBRARY_PATH="$ROOT/.tools/link-libs${LIBRARY_PATH:+:$LIBRARY_PATH}"
  export LD_LIBRARY_PATH="$ROOT/.tools/link-libs${LD_LIBRARY_PATH:+:$LD_LIBRARY_PATH}"
fi

# linuxdeploy's GTK plugin resolves runtime directories through pkg-config.
# The local stubs intentionally carry no install paths, so put corrected
# multiarch metadata ahead of them for the bundling step.
export PKG_CONFIG_PATH="$ROOT/scripts/appimage-pkgconfig${PKG_CONFIG_PATH:+:$PKG_CONFIG_PATH}"
if [[ -d "$ROOT/.tools/pkgconfig" ]]; then
  export PKG_CONFIG_PATH="$PKG_CONFIG_PATH:$ROOT/.tools/pkgconfig"
fi

GTK_LIBDIR="/usr/lib/x86_64-linux-gnu"
if [[ -d "$GTK_LIBDIR/gtk-3.0" ]]; then
  export LD_GTK_LIBRARY_PATH="$GTK_LIBDIR"
fi

BUNDLE_DIR="$ROOT/src-tauri/target/release/bundle/appimage"
APPDIR="$BUNDLE_DIR/Janas Studio.AppDir"
ARTIFACT="$BUNDLE_DIR/Janas Studio_0.1.0_amd64.AppImage"

# The fake server is a test-only binary. Remove stale Cargo/bundle copies left
# by older builds so Tauri cannot mistake one for a production binary.
rm -rf "$APPDIR"
rm -rf "$ROOT/src-tauri/target/release/bundle/appimage_deb"
rm -f "$ROOT/src-tauri/target/release/fake-llama-server" \
      "$ROOT/src-tauri/target/release/fake-llama-server.d"
rm -rf "$ROOT/src-tauri/target/release/deps"/fake_llama_server-*
rm -rf "$ROOT/src-tauri/target/release/.fingerprint"/*/bin-fake-llama-server* \
      "$ROOT/src-tauri/target/release/.fingerprint"/*/dep-bin-fake-llama-server
rm -rf "$ROOT/src-tauri/target/debug/deps"/fake_llama_server-*
rm -rf "$ROOT/src-tauri/target/debug/.fingerprint"/*/test-bin-fake-llama-server* \
      "$ROOT/src-tauri/target/debug/.fingerprint"/*/dep-test-bin-fake-llama-server

"$ROOT/node_modules/.bin/tauri" build --bundles appimage

if [[ ! -f "$ARTIFACT" ]]; then
  echo "AppImage was not produced at $ARTIFACT" >&2
  exit 1
fi

if [[ -e "$APPDIR/usr/bin/fake-llama-server" ]]; then
  echo "Refusing to leave the test-only fake-llama-server in the AppDir" >&2
  exit 1
fi

echo
echo "AppImage built successfully:"
echo "  $ARTIFACT"
