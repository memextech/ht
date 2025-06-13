# HT Windows Support

This document describes the Windows support implementation for HT (headless terminal).

## Overview

HT now supports Windows through a cross-platform architecture that provides the same JSON API across Unix and Windows systems. While Unix systems use true PTY (pseudoterminal) functionality, Windows uses a process-based approach with pipes to provide similar functionality.

## Architecture

### Unix Implementation
- Uses `nix` crate for PTY operations (`forkpty`)
- Direct terminal control and signal handling
- Native pseudoterminal functionality

### Windows Implementation  
- Uses `tokio::process` with piped stdio
- Commands are executed via `cmd.exe`
- Pipes stdout/stderr for terminal output
- No true PTY, but provides similar functionality

## Platform Differences

| Feature | Unix | Windows | Notes |
|---------|------|---------|-------|
| PTY Support | ✅ True PTY | ⚠️ Process pipes | Windows uses cmd.exe with pipes |
| Terminal Control | ✅ Full support | ⚠️ Limited | Basic input/output, no terminal control sequences |
| Signal Handling | ✅ Unix signals | ❌ Not supported | Windows doesn't support Unix-style signals |
| Shell Integration | ✅ `/bin/sh` | ✅ `cmd.exe` | Different default shells |
| Performance | ✅ Native | ✅ Good | Process-based but efficient |

## Usage on Windows

### Installation

Download the Windows binary from releases or build from source:

```powershell
# Build from source
cargo build --release --target x86_64-pc-windows-msvc
```

### Basic Usage

```powershell
# Start a cmd.exe session
.\ht.exe

# Run a specific command
.\ht.exe echo "Hello Windows"

# Use with PowerShell
.\ht.exe powershell

# Set terminal size
.\ht.exe --size 100x30 cmd
```

### API Usage

The JSON API is identical across platforms:

```json
{"type": "sendKeys", "keys": ["dir", "Enter"]}
{"type": "takeSnapshot"}
{"type": "resize", "cols": 80, "rows": 24}
```

### Windows-Specific Examples

```powershell
# Directory listing
echo '{"type": "sendKeys", "keys": ["dir", "Enter"]}' | .\ht.exe --subscribe output

# PowerShell command
echo '{"type": "sendKeys", "keys": ["Get-Process", "Enter"]}' | .\ht.exe powershell --subscribe output

# Batch file execution
echo '{"type": "sendKeys", "keys": ["mybatch.bat", "Enter"]}' | .\ht.exe --subscribe output
```

## Testing

### Running Tests

```powershell
# Run all tests
cargo test

# Run Windows-specific tests
cargo test --test integration windows

# Run custom Windows validation
.\scripts\test-windows.ps1
```

### CI/CD

Windows builds are tested automatically via GitHub Actions:
- Compilation verification
- Unit and integration tests  
- Binary functionality validation
- Cross-platform compatibility checks

## Limitations

### Current Limitations
1. **No Terminal Control Sequences**: Windows implementation doesn't process VT100/ANSI escape sequences
2. **Limited Signal Support**: No Unix-style signal handling (SIGTERM, SIGHUP, etc.)
3. **Pipe-based I/O**: Uses process pipes instead of true PTY
4. **No Job Control**: Limited process group management

### Planned Improvements
1. Windows ConPTY integration for true PTY support
2. ANSI escape sequence processing
3. Better process management and cleanup
4. PowerShell integration improvements

## Development

### Building for Windows

From any platform with Rust installed:

```bash
# Add Windows target
rustup target add x86_64-pc-windows-msvc

# Cross-compile to Windows
cargo build --target x86_64-pc-windows-msvc --release
```

### Platform-Specific Code

The codebase uses conditional compilation:

```rust
#[cfg(windows)]
fn windows_specific_function() {
    // Windows implementation
}

#[cfg(unix)]  
fn unix_specific_function() {
    // Unix implementation
}
```

### Testing Changes

Always test on both platforms:

```bash
# Test on Unix
cargo test

# Test on Windows (if available)
cargo test --target x86_64-pc-windows-msvc

# Use GitHub Actions for Windows testing
git push origin windows-support
```

## Troubleshooting

### Common Issues

**Binary doesn't start**
- Ensure Windows Defender isn't blocking the executable
- Check that all dependencies are available
- Try running from PowerShell as Administrator

**Commands don't work as expected**
- Remember that Windows uses `cmd.exe` by default
- Use PowerShell explicitly if needed: `ht.exe powershell`
- Check that paths use Windows-style backslashes

**Performance issues**
- Windows process creation is slower than Unix fork()
- Consider using fewer, longer-running commands
- Monitor resource usage with Task Manager

### Getting Help

1. Check this documentation
2. Review the integration tests in `tests/integration.rs`
3. Look at Windows-specific tests in `src/tests.rs`
4. File an issue on GitHub with platform details

## Contributing

When contributing Windows-related changes:

1. Test on both Windows and Unix platforms
2. Use conditional compilation (`#[cfg(windows)]`/`#[cfg(unix)]`)
3. Update tests for both platforms
4. Document any platform-specific behavior
5. Ensure CI passes on all platforms

The Windows implementation aims to provide the same developer experience as Unix while working within Windows constraints.