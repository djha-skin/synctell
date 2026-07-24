# TODO — MCP Subcommand

## Beads

- [x] **synctell-kbk** — Add rmcp dependency and MCP subcommand skeleton
- [x] **synctell-8qo** — Implement synctell_write MCP tool
- [x] **synctell-257** — Implement synctell_read_oneshot MCP tool
- [x] **synctell-d6j** — Implement synctell_read_start_linger MCP tool
- [x] **synctell-cdx** — Implement synctell_read_stop_linger MCP tool
- [ ] **synctell-320** — Add integration tests for all MCP tools

## Progress

All 4 MCP tools implemented and passing unit tests (11 tests, all pass).
The linger reader was rewritten to use O_RDWR unblocking technique to avoid
deadlocks on FIFO open().

## Notes

The MCP server serves four tools:
- `synctell_write` — write a message to a FIFO
- `synctell_read_oneshot` — create FIFO, read one message, remove FIFO
- `synctell_read_start_linger` — create FIFO, background reader accepting multiple writers
- `synctell_read_stop_linger` — stop a lingering reader, return buffered data

Uses rmcp 2.2.0 with `#[tool_router]` + `#[tool]` macros, `ToolRouter<Self>`, `Parameters<T>` for schema extraction, `ServiceExt::serve` for stdio transport.