# HT Windows Fork - Project Rules

## Project Scope
Making the `ht` (headless terminal) tool work on Windows. The original project only supports Unix-like systems (macOS, Linux) due to its reliance on Unix PTY (pseudoterminal) system calls.

## User Requirements
- Cross-platform support for Windows
- Maintain existing Unix functionality
- Use Windows CI pipeline for testing since we don't have access to Windows machines
- Create a separate branch (`windows-support`) for development

## Architecture & Design

### Key Changes Made
1. **Platform-specific PTY implementations**:
   - Unix: Uses `nix` crate for `forkpty` and traditional Unix PTY management
   - Windows: Uses `tokio::process` with piped stdio as PTY substitute

2. **Cargo.toml structure**:
   - Conditional dependencies using `[target.'cfg(unix)'.dependencies]` and `[target.'cfg(windows)'.dependencies]`
   - Unix: `nix`, `mio` for PTY and async I/O
   - Windows: `windows` crate for Windows API access

3. **Unified pty.rs module**:
   - Single file with `#[cfg(unix)]` and `#[cfg(windows)]` conditional compilation
   - Common `Winsize` structure abstraction
   - Platform-specific `spawn()` function implementations

4. **Locale handling**:
   - Unix: Uses `nix::libc` for locale detection
   - Windows: Assumes UTF-8 support (no-op implementation)

### Key Files
- `src/pty.rs` - Platform-specific PTY implementations
- `src/locale.rs` - Platform-specific locale handling
- `src/nbio.rs` - Non-blocking I/O helpers (updated for Windows)
- `Cargo.toml` - Platform-specific dependencies
- `.github/workflows/windows-ci.yml` - Windows CI pipeline

## Development Process

### Testing Approach
1. Use GitHub Actions Windows CI for testing (no local Windows machine)
2. Compile checks on Unix to ensure cross-platform code works
3. Windows CI builds the binary and runs basic tests

### Current Implementation Status
- ✅ Unix functionality preserved
- ✅ Windows compilation working
- ✅ Basic Windows PTY substitute using tokio::process
- ✅ CI pipeline setup
- ✅ Successfully rebased onto latest fix/clippy-warnings branch (dbe4c75)
- ✅ All clippy warnings resolved and code properly formatted
- ✅ Fixed duplicate Default implementation conflicts during rebase
- ⚠️  Windows testing needs validation on actual Windows systems

### Known Limitations
1. Windows implementation uses cmd.exe pipes instead of true PTY
2. No terminal control sequences translation for Windows
3. Window resizing may not work properly on Windows
4. Signal handling differences between platforms

## Iteration Strategy
1. Get basic Windows build working (DONE)
2. Test basic functionality via CI
3. Improve Windows terminal experience
4. Add proper Windows terminal control sequence handling
5. Test with real Windows applications

## Deployment Notes
- Use CI to generate Windows binaries
- Windows builds target `x86_64-pc-windows-msvc`
- Binaries should be tested on real Windows systems before release

## Diagnosis Commands
- `cargo check` - Check compilation on current platform
- `cargo build --release` - Build release binary
- `git push origin windows-support` - Trigger Windows CI
- Check GitHub Actions for Windows build results