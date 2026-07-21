# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/).

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
