# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/).

## [0.5.0] — 2026-07-26

### Added

- **`roundrobin`/`rr` subcommand:** reads from one input FIFO and
  distributes each message to one output FIFO in round-robin order.
  Supports `-t`/`--timeout` (inactivity timeout with exit 124) and
  `-m`/`--max-time` (hard deadline). Output FIFOs must already exist;
  missing ones are skipped per-message with a warning.
- **`-L`/`--oneshot` flag:** explicitly disables lingering on the
  `read` subcommand, causing it to exit after one message. The
  deprecated `-l`/`--linger` flag is retained for backward
  compatibility; `-l` and `-L` may be combined and the last one wins.
- **`synctell_roundrobin_start` / `synctell_roundrobin_stop` MCP tools:**
  start/stop pattern for round-robin distribution, mirroring the
  broadcast tools.

### Changed

- **Default linger (breaking):** `synctell read` now stays alive for
  multiple writers by default (linger is ON). Previously the default
  was to exit after one message. Use `-L`/`--oneshot` to restore the
  old one-shot behavior. This reverses the 0.2.0 breaking change.

## [0.4.0] — 2026-07-26

### Added

- `read`/`write` subcommands: CLI flags `-i`/`-o` refactored into
  `read` and `write` subcommands with a cleaner positional-argument
  interface. `read` accepts `-l`/`--linger` and `-t`/`--timeout`;
  `write` is a simple positional-argument command.
- `broadcast` subcommand (`synctell broadcast`): reads from one FIFO
  and writes each message to multiple output FIFOs. Linger is
  automatically active on the input. Supports `-t`/`--timeout` for
  inactivity timeout and `-m`/`--max-time` for hard deadline.
- `-m`/`--max-time` flag: hard deadline (exit after N seconds no
  matter what) on both `read` and `broadcast` subcommands. Distinct
  from `-t`/`--timeout` which only governs the wait for the first
  message.
- `synctell_read_still_linger` MCP tool: reads the next message from
  an active linger reader without stopping or closing it. Supports
  optional `timeout` parameter (0 = block forever).
- `synctell_broadcast_start` / `synctell_broadcast_stop` MCP tools:
  split from the original single-blocking-call broadcast tool into
  a start/stop pattern mirroring the linger reader tools. `_start`
  returns immediately; `_stop` cleans up and returns message count.
- FIFO-exists guard: `read`, `broadcast`, and their MCP counterparts
  now reject operations if the FIFO already exists, preventing
  conflicts with another listener on the same channel.
- Integration tests for all MCP tools.

### Fixed

- Broadcast timeout exit code: properly exits 124 on timeout.
- Reader thread unblock: correct cleanup on broadcast shutdown.
- Output write failures: broadcast continues gracefully when
  an output FIFO's reader disappears.

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
