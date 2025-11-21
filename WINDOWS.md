# HT Windows Support

This document provides comprehensive information about Windows support in HT (headless terminal).

> **Quick Start**: See the [Windows Support section in README.md](README.md#windows-support) for basic usage instructions.

## Overview

HT now supports Windows through a cross-platform architecture that provides the same JSON API across Unix and Windows systems. While Unix systems use true PTY (pseudoterminal) functionality, Windows uses a process-based approach with pipes to provide similar functionality.

**Key Benefits:**
- ✅ Same JSON API across all platforms
- ✅ Native Windows process integration
- ✅ Support for both cmd.exe and PowerShell
- ✅ Automated Windows CI testing
- ✅ Cross-compilation support

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

# Run Windows-specific tests only
cargo test --test integration windows_integration

# Run unit tests with Windows conditionals
cargo test windows_tests

# Test cross-compilation (from Unix)
cargo check --target x86_64-pc-windows-msvc
```

### CI/CD

Windows builds are tested automatically via GitHub Actions in `.github/workflows/windows-ci.yml`:
- **Compilation verification**: Ensures Windows builds compile cleanly
- **Unit and integration tests**: Runs all Windows-specific test scenarios
- **Binary functionality validation**: Tests `--help`, `--version`, and basic commands
- **Cross-platform compatibility checks**: Verifies Unix functionality remains intact

The CI automatically tests:
- Windows cmd.exe integration
- PowerShell execution
- Process lifecycle management
- JSON API compatibility
- Error handling scenarios

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
# Add Windows target (if not already present)
rustup target add x86_64-pc-windows-msvc

# Cross-compile to Windows
cargo build --target x86_64-pc-windows-msvc --release

# Build debug version for testing
cargo build --target x86_64-pc-windows-msvc
```

### Platform-Specific Code Architecture

The codebase uses conditional compilation throughout:

```rust
// Platform-specific dependencies in Cargo.toml
[target.'cfg(windows)'.dependencies]
windows = { version = "0.58", features = [...] }

[target.'cfg(unix)'.dependencies]  
nix = { version = "0.28.0", features = [...] }

// Platform-specific implementations
#[cfg(windows)]
pub fn spawn(command: String, winsize: Winsize, ...) -> Result<...> {
    // Windows implementation using tokio::process
}

#[cfg(unix)]
pub fn spawn(command: String, winsize: Winsize, ...) -> Result<...> {
    // Unix implementation using nix::pty::forkpty
}
```

**Key Architecture Files:**
- `src/pty.rs` - Platform-specific PTY implementations
- `src/locale.rs` - Platform-specific locale handling
- `src/nbio.rs` - Non-blocking I/O abstractions
- `Cargo.toml` - Conditional dependency management

### Testing Changes

Development workflow for Windows support:

```bash
# 1. Test Unix functionality (ensure no regressions)
cargo test
cargo clippy
cargo fmt --check

# 2. Test Windows compilation (cross-compile check)
cargo check --target x86_64-pc-windows-msvc

# 3. Push to trigger Windows CI
git push origin windows-support

# 4. Check GitHub Actions for Windows test results
# Actions run Windows-specific integration tests automatically
```

### Code Style Guidelines

When adding Windows-specific code:

1. **Use conditional compilation**: Always use `#[cfg(windows)]`/`#[cfg(unix)]`
2. **Maintain API compatibility**: Same function signatures across platforms
3. **Document platform differences**: Add comments explaining Windows-specific behavior
4. **Test thoroughly**: Add Windows-specific tests in `src/tests.rs` and `tests/integration.rs`
5. **Update documentation**: Keep WINDOWS.md and README.md updated

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

## Future Improvements

### Planned Enhancements

1. **Windows ConPTY Integration**
   - Integrate with Windows Console Pseudoterminal (ConPTY) API
   - Provide true PTY functionality on Windows 10+
   - Better terminal application compatibility

2. **Enhanced Terminal Control**
   - ANSI/VT100 escape sequence processing
   - Cursor positioning and terminal manipulation
   - Color and formatting support

3. **Improved PowerShell Integration**
   - Native PowerShell object handling
   - Better cmdlet support
   - PowerShell ISE compatibility

4. **Performance Optimizations**
   - Reduce process creation overhead
   - Optimize pipe-based I/O
   - Better resource management

### Contributing to Windows Support

The Windows implementation is actively maintained. Contributions are welcome in:

- **Testing**: Real Windows hardware testing
- **Performance**: Optimization improvements
- **Compatibility**: Support for more Windows applications
- **Documentation**: Usage examples and troubleshooting

**Development Process:**
1. Create feature branch from `windows-support`
2. Implement changes with proper conditional compilation
3. Add platform-specific tests
4. Ensure CI passes on all platforms
5. Update documentation
6. Submit pull request

### Known Issues & Workarounds

**Issue**: Some terminal applications don't work correctly
**Workaround**: Use applications designed for pipe-based I/O

**Issue**: No signal handling for graceful shutdown
**Workaround**: Use process termination instead of signals

**Issue**: Limited job control features
**Workaround**: Manage processes individually rather than in groups

For the latest status and to report issues, see the [GitHub Issues](https://github.com/memextech/ht/issues) page.