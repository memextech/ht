#!/usr/bin/env python3
"""
Demo script to generate colorful terminal output for testing themes.

Sends commands to ht via STDIN to demonstrate color rendering with different
ANSI colors, allowing you to inspect how your custom theme appears.

Usage:
    python3 demo-colors.py | ht --listen 127.0.0.1:8080 --custom-css theme.css
"""

import sys
import json
import time

def send_command(command):
    """Send a JSON command to ht via STDIN"""
    print(json.dumps(command), flush=True)
    time.sleep(0.5)

def send_keys(*keys):
    """Send keys to terminal"""
    send_command({"type": "sendKeys", "keys": list(keys)})

def main():
    print("Sending commands to demonstrate terminal styling...", file=sys.stderr)
    time.sleep(1)
    
    # Clear screen first
    send_keys("clear", "Enter")
    time.sleep(0.5)
    
    # Show colorful ls output
    print("1. Listing files with colors...", file=sys.stderr)
    send_keys("ls --color=auto", "Enter")
    time.sleep(1)
    
    # Show some environment variables with grep highlighting
    print("2. Showing PATH with grep colors...", file=sys.stderr)
    send_keys("echo $PATH | tr ':' '\\n' | grep --color=auto bin", "Enter")
    time.sleep(1)
    
    # Show git status (if in a git repo)
    print("3. Git status (with colors)...", file=sys.stderr)
    send_keys("git status", "Enter")
    time.sleep(1)
    
    # Run a command that shows success/error colors
    print("4. Testing command colors...", file=sys.stderr)
    send_keys("echo -e '\\033[32mGREEN SUCCESS\\033[0m'", "Enter")
    time.sleep(0.5)
    send_keys("echo -e '\\033[31mRED ERROR\\033[0m'", "Enter")
    time.sleep(0.5)
    send_keys("echo -e '\\033[33mYELLOW WARNING\\033[0m'", "Enter")
    time.sleep(0.5)
    send_keys("echo -e '\\033[34mBLUE INFO\\033[0m'", "Enter")
    time.sleep(0.5)
    send_keys("echo -e '\\033[35mMAGENTA\\033[0m'", "Enter")
    time.sleep(0.5)
    send_keys("echo -e '\\033[36mCYAN\\033[0m'", "Enter")
    time.sleep(1)
    
    # Show a simple progress indicator
    print("5. Showing progress animation...", file=sys.stderr)
    send_keys("for i in {1..10}; do echo -n '█'; sleep 0.1; done; echo", "Enter")
    time.sleep(2)
    
    # Show some file content with cat
    print("6. Showing package.json or cargo.toml...", file=sys.stderr)
    send_keys("cat Cargo.toml 2>/dev/null || cat package.json 2>/dev/null || echo 'No config file found'", "Enter")
    time.sleep(1)
    
    print("Done! Terminal should now have colorful output to inspect.", file=sys.stderr)
    print("Keeping terminal open for inspection...", file=sys.stderr)
    
    # Keep STDIN open so ht doesn't exit
    # You can stop it with Ctrl-C
    try:
        while True:
            time.sleep(1)
    except KeyboardInterrupt:
        print("Shutting down...", file=sys.stderr)

if __name__ == "__main__":
    main()
