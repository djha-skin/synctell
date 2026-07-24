# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/).

## [0.3.0] — 2026-07-23

### Added

- MCP server (`synctell mcp`): built-in JSON-RPC server exposing four FIFO
  tools for AI agent and programmatic use. Tools: `synctell_write`,
  `synctell_read_oneshot`, `synctell_read_start_linger`,
  `synctell_read_stop_linger`.
- Integration tests for all MCP tools.

### Fixed

- Linger reader deadlock during MCP implementation.
- MCP `timeout` parameter: changed from `Option<u64>` to `u64` with
  `#[serde(default)]` to fix JSON schema union type (`["integer", "null"]`)
  that some MCP bridges could not serialize. Convention: `0` = block forever,
  `>0` = timeout after N seconds.

## [0.2.0] — 2026-07-21

### Added

- `--linger` / `-l` flag: when reading with `-i`, keeps the reader alive
  for multiple writers until interrupted by SIGINT/SIGTERM. Without the
  flag (the new default), the reader exits after the first message.

### Changed

- **Default reader behavior (breaking):** `synctell -i <file>` now exits
  after reading the first message. Previously the reader stayed alive
  indefinitely. Use `-l` to restore the old behavior.

## [0.1.0] — 2026-07-17

### Added

- Input mode (`-i`): creates a FIFO, reads messages from writers, removes
  the FIFO on exit. The FIFO's presence on disk signals that a reader is
  listening.
- Output mode (`-o`): polls for a FIFO and writes a message to it.
- Timeout (`-t`): exit code 124 when the expected peer does not appear
  within the specified duration.
- Signal handling (SIGINT/SIGTERM) for graceful shutdown and FIFO cleanup.
- Newline-appending: the reader appends a trailing newline if the writer's
  data did not already end with one.
- Many-writer support: a single reader accepts messages from any number
  of writers.
