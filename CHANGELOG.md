# Changelog

## [Unreleased]

### Fixed
- Fixed PTY buffer overflow when sending large inputs (>1500 bytes)
  - Large heredocs now work correctly without data loss or text scrambling
  - Inputs ≥1500 bytes are automatically chunked into 512-byte pieces with 10ms delays
  - Fixes issues with `gh pr create` and other commands with large heredocs
  - 100% backwards compatible, no breaking changes
  - Minimal performance impact (50-90ms added latency for large inputs only)

## [0.3.0] - Previous Release
