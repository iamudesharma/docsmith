#!/bin/sh
set -eu
cd "$(dirname "$0")/.."
mkdir -p target/helpers
xcrun swiftc scripts/apple-vision.swift -o target/helpers/apple-vision
printf 'Set DOC_APPLE_VISION=%s/target/helpers/apple-vision\n' "$PWD"
