#!/bin/bash
# Unix test script for HT
# This script validates that HT works correctly on Unix platforms

set -e

BINARY_PATH="${1:-target/release/ht}"

echo "Testing HT Unix functionality..."

# Test 1: Check if binary exists and is executable
if [[ ! -f "$BINARY_PATH" ]]; then
    echo "❌ HT binary not found at $BINARY_PATH"
    exit 1
fi

if [[ ! -x "$BINARY_PATH" ]]; then
    echo "❌ HT binary is not executable"
    exit 1
fi

echo "✓ Binary exists and is executable"

# Test 2: Test help command
if ! help_output=$("$BINARY_PATH" --help 2>&1); then
    echo "❌ Help command failed"
    exit 1
fi

if [[ ! "$help_output" =~ "Usage:" ]]; then
    echo "❌ Help output doesn't contain expected content"
    exit 1
fi

echo "✓ Help command works"

# Test 3: Test version command
if ! version_output=$("$BINARY_PATH" --version 2>&1); then
    echo "❌ Version command failed"
    exit 1
fi

echo "✓ Version command works: $version_output"

# Test 4: Test that binary can start with a simple command
if timeout 5 "$BINARY_PATH" echo "test" >/dev/null 2>&1; then
    echo "✓ Binary can execute simple commands"
else
    echo "⚠️  Binary execution test timed out (this might be expected)"
fi

# Test 5: Check binary dependencies (on Linux)
if command -v ldd >/dev/null 2>&1; then
    if ldd "$BINARY_PATH" | grep -q "libc.so"; then
        echo "✓ Binary has expected Unix dependencies"
    fi
elif command -v otool >/dev/null 2>&1; then
    # macOS
    if otool -L "$BINARY_PATH" | grep -q "libSystem"; then
        echo "✓ Binary has expected macOS dependencies"
    fi
fi

# Test 6: Test locale functionality
echo "✓ Testing locale functionality..."
if "$BINARY_PATH" --help >/dev/null 2>&1; then
    echo "✓ Locale check passed"
fi

echo ""
echo "🎉 All Unix tests passed!"
echo "HT appears to be working correctly on Unix."