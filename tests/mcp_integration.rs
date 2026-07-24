//! Integration tests for the synctell MCP tools.
//!
//! These tests spawn the `synctell mcp` binary as a subprocess and
//! communicate via JSON-RPC over stdio, exercising the real MCP
//! transport layer end-to-end.

use std::io::{BufRead, BufReader, Write};
use std::path::PathBuf;
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

// ─── Helpers ───────────────────────────────────────────────────────

static NEXT_ID: AtomicU64 = AtomicU64::new(1);

/// An MCP client connected to a `synctell mcp` subprocess.
struct McpClient {
    stdin: ChildStdin,
    stdout: BufReader<std::process::ChildStdout>,
    _child: Child,
}

impl McpClient {
    /// Spawn `synctell mcp` and perform the initialize handshake.
    fn spawn() -> Self {
        // Find the binary relative to the target directory.
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_synctell"));
        cmd.arg("mcp")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());

        let mut child = cmd.spawn().expect("failed to spawn synctell mcp");

        let stdin = child.stdin.take().expect("stdin not captured");
        let stdout = BufReader::new(child.stdout.take().expect("stdout not captured"));

        let mut client = McpClient {
            stdin,
            stdout,
            _child: child,
        };

        // Perform the MCP initialize handshake.
        client.handshake();

        client
    }

    /// Perform the MCP initialize request/response + initialized notification.
    fn handshake(&mut self) {
        // Send initialize request.
        let req = serde_json::json!({
            "jsonrpc": "2.0",
            "id": self.next_id(),
            "method": "initialize",
            "params": {
                "protocolVersion": "2024-11-05",
                "capabilities": {},
                "clientInfo": {
                    "name": "synctell-integration-test",
                    "version": "0.1.0"
                }
            }
        });
        self.send(&req);

        // Read initialize response.
        let resp = self.recv();
        assert_eq!(resp["jsonrpc"], "2.0", "bad jsonrpc in init response: {resp}");
        assert!(
            resp.get("result").is_some(),
            "init response missing result: {resp}"
        );

        // Send initialized notification (no response expected).
        let notif = serde_json::json!({
            "jsonrpc": "2.0",
            "method": "notifications/initialized"
        });
        self.send(&notif);
    }

    fn next_id(&self) -> u64 {
        NEXT_ID.fetch_add(1, Ordering::Relaxed)
    }

    /// Send a JSON-RPC message (one line) to the server's stdin.
    fn send(&mut self, msg: &serde_json::Value) {
        let line = serde_json::to_string(msg).expect("serialize request");
        writeln!(self.stdin, "{line}").expect("write to stdin");
        self.stdin.flush().expect("flush stdin");
    }

    /// Read one JSON-RPC response line from the server's stdout.
    fn recv(&mut self) -> serde_json::Value {
        let mut line = String::new();
        loop {
            self.stdout
                .read_line(&mut line)
                .expect("read from stdout");
            let trimmed = line.trim().to_string();
            if !trimmed.is_empty() {
                line = trimmed;
                break;
            }
            line.clear();
        }
        // Use json5 for more lenient parsing
        match serde_json::from_str(&line) {
            Ok(v) => v,
            Err(e) => {
                panic!("Failed to parse JSON-RPC response: {e}\nRaw line: {line:?}");
            }
        }
    }

    /// Call an MCP tool and return the result content.
    fn call_tool(&mut self, name: &str, args: serde_json::Value) -> Result<String, String> {
        let req = serde_json::json!({
            "jsonrpc": "2.0",
            "id": self.next_id(),
            "method": "tools/call",
            "params": {
                "name": name,
                "arguments": args
            }
        });
        self.send(&req);
        let resp = self.recv();

        // Check for JSON-RPC error.
        if let Some(err) = resp.get("error") {
            let msg = err["message"]
                .as_str()
                .unwrap_or("unknown error")
                .to_string();
            return Err(msg);
        }

        // Extract result.
        if let Some(result) = resp.get("result") {
            // If result has isError, return the text content as error.
            let is_error = result.get("isError").and_then(|v| v.as_bool()).unwrap_or(false);

            let text = result
                .get("content")
                .and_then(|c| c.as_array())
                .map(|arr| {
                    arr.iter()
                        .filter_map(|c| c.get("text"))
                        .filter_map(|t| t.as_str())
                        .collect::<Vec<_>>()
                        .join("")
                })
                .unwrap_or_default();

            if is_error {
                return Err(text);
            }
            return Ok(text);
        }

        Ok(String::new())
    }
}

/// Get a unique FIFO path for a test.
fn fifo_path(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join("synctell-integration-tests");
    let _ = std::fs::create_dir_all(&dir);
    dir.join(name)
}

/// Clean up a FIFO path if it still exists.
fn cleanup(path: &PathBuf) {
    let _ = std::fs::remove_file(path);
}

// ─── Tests ─────────────────────────────────────────────────────────

#[test]
fn test_list_tools() {
    let mut client = McpClient::spawn();

    let req = serde_json::json!({
        "jsonrpc": "2.0",
        "id": client.next_id(),
        "method": "tools/list",
        "params": {}
    });
    client.send(&req);
    let resp = client.recv();

    // Should have a result with tools array.
    let result = resp.get("result").expect("tools/list should have result");
    let tools = result["tools"]
        .as_array()
        .expect("tools/list result should have tools array");
    let tool_names: Vec<&str> = tools
        .iter()
        .filter_map(|t| t["name"].as_str())
        .collect();

    assert!(
        tool_names.contains(&"synctell_write"),
        "should have synctell_write"
    );
    assert!(
        tool_names.contains(&"synctell_read_oneshot"),
        "should have synctell_read_oneshot"
    );
    assert!(
        tool_names.contains(&"synctell_read_start_linger"),
        "should have synctell_read_start_linger"
    );
    assert!(
        tool_names.contains(&"synctell_read_stop_linger"),
        "should have synctell_read_stop_linger"
    );
}

#[test]
fn test_write_no_reader_errors() {
    let mut client = McpClient::spawn();

    let result = client.call_tool(
        "synctell_write",
        serde_json::json!({
            "path": "/tmp/nonexistent-test-fifo-12345",
            "message": "hello"
        }),
    );

    assert!(result.is_err(), "write to non-existent FIFO should error");
    let err = result.unwrap_err();
    assert!(
        err.contains("does not exist"),
        "error should mention FIFO does not exist: {err}"
    );
}

#[test]
fn test_oneshot_timeout_errors() {
    let mut client = McpClient::spawn();
    let path = fifo_path("test_oneshot_timeout.fifo");
    cleanup(&path); // Ensure it's gone.

    // This should timeout after 1 second with no writer.
    let result = client.call_tool(
        "synctell_read_oneshot",
        serde_json::json!({
            "path": path,
            "timeout": 1
        }),
    );

    assert!(result.is_err(), "read_oneshot with no writer should timeout");
    let err = result.unwrap_err();
    assert!(
        err.contains("timed out") || err.contains("timeout"),
        "error message should mention timeout: {err}"
    );

    // FIFO should be cleaned up.
    assert!(!path.exists(), "FIFO should be removed after timeout");
}

#[test]
fn test_linger_creates_fifo() {
    let mut client = McpClient::spawn();
    let path = fifo_path("test_linger_creates_fifo.fifo");
    cleanup(&path);

    let result = client.call_tool(
        "synctell_read_start_linger",
        serde_json::json!({
            "path": path,
        }),
    );

    assert!(result.is_ok(), "start_linger should succeed: {:?}", result);
    assert!(path.exists(), "FIFO should exist after start_linger");

    // Stop the linger.
    let result = client.call_tool(
        "synctell_read_stop_linger",
        serde_json::json!({
            "path": path,
        }),
    );
    assert!(result.is_ok(), "stop_linger should succeed: {:?}", result);
    assert!(!path.exists(), "FIFO should be removed after stop_linger");
}

#[test]
fn test_linger_write_roundtrip() {
    let mut client = McpClient::spawn();
    let path = fifo_path("test_linger_write_roundtrip.fifo");
    cleanup(&path);

    // Start linger.
    let result = client.call_tool(
        "synctell_read_start_linger",
        serde_json::json!({
            "path": path,
        }),
    );
    assert!(result.is_ok(), "start_linger: {:?}", result);
    assert!(path.exists(), "FIFO should exist");

    // Give the background reader time to block on open().
    std::thread::sleep(Duration::from_millis(100));

    // Write a message.
    let result = client.call_tool(
        "synctell_write",
        serde_json::json!({
            "path": path,
            "message": "hello from test"
        }),
    );
    assert!(result.is_ok(), "write: {:?}", result);

    // Give the background reader time to consume.
    std::thread::sleep(Duration::from_millis(100));

    // Write another message.
    let result = client.call_tool(
        "synctell_write",
        serde_json::json!({
            "path": path,
            "message": "second message"
        }),
    );
    assert!(result.is_ok(), "write: {:?}", result);

    // Give the background reader time to consume.
    std::thread::sleep(Duration::from_millis(100));

    // Stop linger and get buffered data.
    let result = client.call_tool(
        "synctell_read_stop_linger",
        serde_json::json!({
            "path": path,
        }),
    );
    assert!(result.is_ok(), "stop_linger: {:?}", result);

    let data = result.unwrap();
    assert!(
        data.contains("hello from test"),
        "should contain first message: {data:?}"
    );
    assert!(
        data.contains("second message"),
        "should contain second message: {data:?}"
    );
    assert!(!path.exists(), "FIFO should be removed after stop_linger");
}

#[test]
fn test_linger_rejects_duplicate() {
    let mut client = McpClient::spawn();
    let path = fifo_path("test_linger_rejects_duplicate.fifo");
    cleanup(&path);

    // Start first linger.
    let result = client.call_tool(
        "synctell_read_start_linger",
        serde_json::json!({
            "path": path,
        }),
    );
    assert!(result.is_ok(), "first start_linger: {:?}", result);

    // Start second linger on same path — should fail.
    let result = client.call_tool(
        "synctell_read_start_linger",
        serde_json::json!({
            "path": path,
        }),
    );
    assert!(result.is_err(), "duplicate start_linger should error");
    assert!(
        result.unwrap_err().contains("already active"),
        "error should mention already active"
    );

    // Clean up.
    let result = client.call_tool(
        "synctell_read_stop_linger",
        serde_json::json!({
            "path": path,
        }),
    );
    assert!(result.is_ok(), "stop_linger: {:?}", result);
}

#[test]
fn test_stop_linger_rejects_invalid() {
    let mut client = McpClient::spawn();
    let path = fifo_path("test_stop_linger_rejects_invalid.fifo");
    cleanup(&path);

    // Stop a non-existent linger.
    let result = client.call_tool(
        "synctell_read_stop_linger",
        serde_json::json!({
            "path": path,
        }),
    );
    assert!(result.is_err(), "stop_linger on unknown path should error");
    assert!(
        result.unwrap_err().contains("no linger reader"),
        "error should mention no linger reader"
    );
}

/// Test: write → read_oneshot roundtrip.
#[test]
fn test_oneshot_roundtrip() {
    let mut client = McpClient::spawn();
    let path = fifo_path("test_oneshot_roundtrip.fifo");
    cleanup(&path);

    let path2 = path.clone();

    // Spawn a writer thread that waits for the FIFO to appear, then writes.
    let writer = std::thread::spawn(move || {
        for _ in 0..50 {
            if path2.exists() {
                break;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        assert!(path2.exists(), "FIFO should have been created");
        // Write directly to the FIFO.
        let result = write_to_fifo_direct(&path2, "hello from roundtrip");
        result
    });

    // Call read_oneshot — this will block until the writer connects.
    let result = client.call_tool(
        "synctell_read_oneshot",
        serde_json::json!({
            "path": path,
            "timeout": 10
        }),
    );

    let write_result = writer.join().unwrap();
    assert!(
        write_result.is_ok(),
        "write should succeed: {:?}",
        write_result
    );

    assert!(result.is_ok(), "read_oneshot should succeed: {:?}", result);
    let data = result.unwrap();
    assert!(
        data.contains("hello from roundtrip"),
        "should contain the written message: {data:?}"
    );

    // FIFO should be cleaned up.
    assert!(!path.exists(), "FIFO should be removed after oneshot read");
}

/// Write to a FIFO directly (used by the roundtrip test helper).
fn write_to_fifo_direct(path: &std::path::Path, message: &str) -> Result<usize, String> {
    use std::io::Write;
    if !path.exists() {
        return Err(format!("'{}' does not exist", path.display()));
    }
    let mut file = std::fs::File::options()
        .write(true)
        .open(path)
        .map_err(|e| format!("failed to open: {e}"))?;
    file.write_all(message.as_bytes())
        .map_err(|e| format!("failed to write: {e}"))?;
    Ok(message.len())
}