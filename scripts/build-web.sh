#!/bin/sh
set -eu

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
OUT_DIR="$ROOT/assets/web"
mkdir -p "$OUT_DIR"

if [ "$(uname -s)" = Darwin ] && [ -d /Library/Developer/CommandLineTools/SDKs/MacOSX26.5.sdk ]; then
  export SDKROOT=/Library/Developer/CommandLineTools/SDKs/MacOSX26.5.sdk
  export CC=/usr/bin/clang
  export CARGO_TARGET_AARCH64_APPLE_DARWIN_LINKER=/usr/bin/clang
fi

# Locate a toolchain that has the wasm32-unknown-unknown target stdlib installed.
if [ -x "$HOME/.rustup/toolchains/stable-aarch64-apple-darwin/bin/cargo" ] && \
   [ -d "$HOME/.rustup/toolchains/stable-aarch64-apple-darwin/lib/rustlib/wasm32-unknown-unknown" ]; then
  export RUSTC="$HOME/.rustup/toolchains/stable-aarch64-apple-darwin/bin/rustc"
  CARGO_BIN="$HOME/.rustup/toolchains/stable-aarch64-apple-darwin/bin/cargo"
elif [ -x "$HOME/.rustup/toolchains/nightly-aarch64-apple-darwin/bin/cargo" ] && \
     [ -d "$HOME/.rustup/toolchains/nightly-aarch64-apple-darwin/lib/rustlib/wasm32-unknown-unknown" ]; then
  export RUSTC="$HOME/.rustup/toolchains/nightly-aarch64-apple-darwin/bin/rustc"
  CARGO_BIN="$HOME/.rustup/toolchains/nightly-aarch64-apple-darwin/bin/cargo"
else
  CARGO_BIN="cargo"
fi

export CARGO_PROFILE_RELEASE_OPT_LEVEL=z
export CARGO_PROFILE_RELEASE_LTO=fat
export CARGO_PROFILE_RELEASE_CODEGEN_UNITS=1
export CARGO_PROFILE_RELEASE_STRIP=symbols

"$CARGO_BIN" build --manifest-path "$ROOT/Cargo.toml" --lib --target wasm32-unknown-unknown --release --locked

WASM_SRC="$ROOT/target/wasm32-unknown-unknown/release/doc_extractor.wasm"
gzip -9 -n -c "$WASM_SRC" > "$OUT_DIR/doc_extractor.wasm.gz"
ls -lh "$WASM_SRC" "$OUT_DIR/doc_extractor.wasm.gz"
