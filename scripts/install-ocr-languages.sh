#!/bin/sh
set -eu
# Install checked-in Tesseract language data into the active tessdata directory.
# Usage: scripts/install-ocr-languages.sh [eng] [hin] ...
cd "$(dirname "$0")/.."

if ! command -v tesseract >/dev/null 2>&1; then
  echo "tesseract is not installed (brew install tesseract)" >&2
  exit 1
fi

if [ "$#" -gt 0 ]; then
  LANGUAGES="$*"
else
  LANGUAGES="eng hin osd"
fi

TARGET="${TESSDATA_PREFIX:-}"
if [ -z "$TARGET" ]; then
  TARGET="$(tesseract --list-langs 2>&1 | sed -n 's/.*"\(.*\)".*/\1/p' | head -1)"
fi
if [ -z "$TARGET" ] || [ ! -d "$TARGET" ]; then
  echo "Could not determine the Tesseract tessdata directory; set TESSDATA_PREFIX" >&2
  exit 1
fi

for language in $LANGUAGES; do
  source="models/tessdata/$language.traineddata"
  if [ ! -f "$source" ]; then
    echo "Missing $source; provision it first" >&2
    exit 1
  fi
  cp "$source" "$TARGET/$language.traineddata"
  echo "Installed $language.traineddata -> $TARGET"
done

echo "Available languages:"
tesseract --list-langs 2>&1
