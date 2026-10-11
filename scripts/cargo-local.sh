#!/bin/sh
set -eu
# Local Xcode 17 linker cannot read the macOS 27 SDK installed beside SDK 26.5.
# Keep the workaround per command rather than changing the machine's toolchain.
if [ "$(uname -s)" = Darwin ] && [ -d /Library/Developer/CommandLineTools/SDKs/MacOSX26.5.sdk ]; then
  export SDKROOT=/Library/Developer/CommandLineTools/SDKs/MacOSX26.5.sdk
  export CC=/usr/bin/clang
  export CARGO_TARGET_AARCH64_APPLE_DARWIN_LINKER=/usr/bin/clang
fi
exec cargo "$@"
