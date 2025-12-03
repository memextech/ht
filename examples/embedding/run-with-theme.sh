#!/bin/bash
# Run ht with Tailwind neutral theme example
set -e

SCRIPT_DIR="$( cd "$( dirname "${BASH_SOURCE[0]}" )" && pwd )"
HT_BINARY="${SCRIPT_DIR}/../../target/release/ht"
CUSTOM_CSS="${SCRIPT_DIR}/tailwind-neutral.css"

if [ ! -f "$HT_BINARY" ]; then
    echo "Error: ht binary not found at $HT_BINARY" >&2
    echo "Build it first: cargo build --release" >&2
    exit 1
fi

if [ ! -f "$CUSTOM_CSS" ]; then
    echo "Error: CSS file not found at $CUSTOM_CSS" >&2
    exit 1
fi

echo "Starting ht with Tailwind neutral theme..."
echo "URL: http://127.0.0.1:8080"
echo ""

exec "$HT_BINARY" --listen 127.0.0.1:8080 --custom-css "$CUSTOM_CSS" "$@"
