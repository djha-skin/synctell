use std::collections::HashMap;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use rmcp::handler::server::router::tool::ToolRouter;
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::{ServerCapabilities, ServerInfo};
use rmcp::{tool, tool_router, ServerHandler, ServiceExt};
use schemars::JsonSchema;

// ─── Request schemas ───────────────────────────────────────────────

#[derive(Debug, Clone, serde::Deserialize, JsonSchema)]
pub struct WriteRequest {
    /// Path to the FIFO to write to
    pub path: PathBuf,
    /// Message to send
    pub message: String,
}

#[derive(Debug, Clone, serde::Deserialize, JsonSchema)]
pub struct ReadOneshotRequest {
    /// Path for the FIFO to create
    pub path: PathBuf,
    /// Seconds to wait (None = block forever)
    pub timeout: Option<u64>,
}

#[derive(Debug, Clone, serde::Deserialize, JsonSchema)]
pub struct StartLingerRequest {
    /// Path for the FIFO to create
    pub path: PathBuf,
    /// Seconds to wait for first writer (None = block forever)
    pub timeout: Option<u64>,
}

#[derive(Debug, Clone, serde::Deserialize, JsonSchema)]
pub struct StopLingerRequest {
    /// Path of the FIFO to stop
    pub path: PathBuf,
}

// ─── MCP server handler ───────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct SynctellServer {
    #[allow(dead_code)] // populated by #[tool_router] macro, read by generated code
    tool_router: ToolRouter<Self>,
    /// Active linger readers keyed by FIFO path.
    readers: Arc<Mutex<HashMap<PathBuf, LingerReader>>>,
}

impl SynctellServer {
    pub fn new() -> Self {
        Self {
            tool_router: Self::tool_router(),
            readers: Arc::new(Mutex::new(HashMap::new())),
        }
    }
}

impl Default for SynctellServer {
    fn default() -> Self {
        Self::new()
    }
}

#[tool_router]
impl SynctellServer {
    #[tool(description = "Write a message to a FIFO")]
    fn synctell_write(
        &self,
        Parameters(WriteRequest { path, message }): Parameters<WriteRequest>,
    ) -> Result<String, String> {
        let result = write_to_fifo(&path, &message)?;
        Ok(format!("wrote {result} bytes"))
    }

    #[tool(description = "Create a FIFO, read one message, remove FIFO")]
    fn synctell_read_oneshot(
        &self,
        Parameters(ReadOneshotRequest { path, timeout }): Parameters<ReadOneshotRequest>,
    ) -> Result<String, String> {
        let msg = read_oneshot(&path, timeout)?;
        Ok(msg)
    }

    #[tool(description = "Create a FIFO, start a background reader accepting multiple writers")]
    fn synctell_read_start_linger(
        &self,
        Parameters(StartLingerRequest { path, timeout }): Parameters<StartLingerRequest>,
    ) -> Result<String, String> {
        let reader = start_linger(&path, timeout)?;
        self.readers.lock().unwrap().insert(path.clone(), reader);
        Ok(format!("linger reader started at '{}'", path.display()))
    }

    #[tool(description = "Stop a lingering reader, return buffered data")]
    fn synctell_read_stop_linger(
        &self,
        Parameters(StopLingerRequest { path }): Parameters<StopLingerRequest>,
    ) -> Result<String, String> {
        let reader = self
            .readers
            .lock()
            .unwrap()
            .remove(&path)
            .ok_or_else(|| format!("no linger reader found for '{}'", path.display()))?;
        let msgs = reader.stop()?;
        Ok(msgs.join(""))
    }
}

impl ServerHandler for SynctellServer {
    fn get_info(&self) -> ServerInfo {
        ServerInfo::new(ServerCapabilities::builder().enable_tools().build())
    }
}

// ─── Entry point ───────────────────────────────────────────────────

/// Run the MCP server over stdio transport.
pub async fn run() -> anyhow::Result<()> {
    let handler = SynctellServer::new();
    let service = handler.serve(rmcp::transport::stdio()).await?;
    service.waiting().await?;
    Ok(())
}

// ─── Core logic (testable without MCP) ────────────────────────────

/// Write a message to an existing FIFO.
///
/// The FIFO must already exist (a reader created it).
/// Returns Ok(bytes_written) or Err on failure.
pub fn write_to_fifo(path: &Path, message: &str) -> Result<usize, String> {
    use std::fs;
    use std::io::Write;

    if !path.exists() {
        return Err(format!("'{}' does not exist (no reader listening)", path.display()));
    }

    let mut file = fs::File::options()
        .write(true)
        .open(path)
        .map_err(|e| format!("failed to open '{}' for writing: {e}", path.display()))?;

    file.write_all(message.as_bytes())
        .map_err(|e| format!("failed to write to '{}': {e}", path.display()))?;

    Ok(message.len())
}

/// Create a FIFO, read one message from it, remove the FIFO, return the message.
///
/// If `timeout` is Some(secs), returns Err after that many seconds
/// with no writer.  The FIFO is always cleaned up before returning.
pub fn read_oneshot(path: &Path, timeout: Option<u64>) -> Result<String, String> {
    use std::fs;

    // Create the FIFO.
    create_mcp_fifo(path)?;

    // We must clean up the FIFO on every exit path.
    let result = read_oneshot_inner(path, timeout);
    let _ = fs::remove_file(path);
    result
}

fn read_oneshot_inner(path: &Path, timeout: Option<u64>) -> Result<String, String> {
    use std::fs;
    use std::io::Read;
    use std::time::{Duration, Instant};

    let deadline = timeout.map(|s| Instant::now() + Duration::from_secs(s));
    let path_display = path.display().to_string();

    // Use a blocking thread for the open+read so we can poll for timeout.
    let outcome: std::sync::Arc<
        (std::sync::Mutex<Option<std::io::Result<Vec<u8>>>>, std::sync::Condvar),
    > = std::sync::Arc::new((std::sync::Mutex::new(None), std::sync::Condvar::new()));
    let outcome_clone = outcome.clone();
    let path_buf = path.to_path_buf();

    std::thread::spawn(move || {
        let result = (|| -> std::io::Result<Vec<u8>> {
            let mut file = fs::File::options()
                .read(true)
                .open(&path_buf)
                .map_err(|e| {
                    std::io::Error::new(
                        e.kind(),
                        format!("failed to open '{}' for reading: {e}", path_buf.display()),
                    )
                })?;
            let mut data = Vec::new();
            file.read_to_end(&mut data).map_err(|e| {
                std::io::Error::new(
                    e.kind(),
                    format!("failed to read from '{}': {e}", path_buf.display()),
                )
            })?;
            Ok(data)
        })();

        let (lock, cvar) = &*outcome_clone;
        *lock.lock().unwrap() = Some(result);
        cvar.notify_one();
    });

    let (lock, cvar) = &*outcome;
    let mut guard = lock.lock().unwrap();

    loop {
        let (new_guard, _) = cvar.wait_timeout(guard, Duration::from_secs(1)).unwrap();
        guard = new_guard;

        if guard.is_some() {
            return match guard.take().unwrap() {
                Ok(data) => {
                    let mut msg = String::from_utf8_lossy(&data).into_owned();
                    if !msg.ends_with('\n') {
                        msg.push('\n');
                    }
                    Ok(msg)
                }
                Err(e) => Err(e.to_string()),
            };
        }

        if let Some(dl) = deadline {
            if Instant::now() >= dl {
                return Err(format!(
                    "timed out waiting for writer on '{path_display}'"
                ));
            }
        }
    }
}

/// Create a POSIX FIFO.  No-op if it already exists as a FIFO.
fn create_mcp_fifo(path: &Path) -> Result<(), String> {
    use std::ffi::CString;
    use std::fs;
    use std::io;
    use std::os::unix::ffi::OsStrExt;
    use std::os::unix::fs::FileTypeExt;

    let c_path =
        CString::new(path.as_os_str().as_bytes()).map_err(|e| format!("path contains null byte: {e}"))?;

    let ret = unsafe { libc::mkfifo(c_path.as_ptr(), 0o666) };
    if ret == 0 {
        return Ok(());
    }

    let err = io::Error::last_os_error();
    let err_msg = err.to_string();
    if err.raw_os_error() == Some(libc::EEXIST) {
        let meta = fs::metadata(path)
            .map_err(|e| format!("cannot stat '{}': {e}", path.display()))?;
        if meta.file_type().is_fifo() {
            return Ok(());
        }
        return Err(format!("'{}' already exists but is not a FIFO", path.display()));
    }

    Err(format!("failed to create FIFO at '{}': {err_msg}", path.display()))
}

// ─── Linger reader ────────────────────────────────────────────────

use std::sync::atomic::{AtomicBool, Ordering};

/// Handle to a running linger reader.
pub struct LingerReader {
    stop: Arc<AtomicBool>,
    buffer: Arc<Mutex<Vec<String>>>,
    handle: Option<std::thread::JoinHandle<()>>,
    path: PathBuf,
}

impl std::fmt::Debug for LingerReader {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LingerReader")
            .field("path", &self.path)
            .field("running", &self.handle.is_some())
            .finish()
    }
}

impl LingerReader {
    /// Signal the reader to stop and return all buffered messages.
    /// The FIFO is removed on exit.
    ///
    /// Opens the FIFO as a writer (O_RDWR, which succeeds immediately on
    /// Linux) to unblock the reader thread blocked on `open()`, then
    /// removes the FIFO and joins the thread.
    pub fn stop(mut self) -> Result<Vec<String>, String> {
        self.stop.store(true, Ordering::Relaxed);

        // Open the FIFO as a writer.  On Linux, O_RDWR always succeeds
        // immediately on a FIFO, which unblocks the reader's blocking
        // O_RDONLY open().  Closing immediately causes the reader's
        // read() to see EOF (if no real writer is active), letting it
        // loop back, check the stop flag, and exit.
        if let Ok(file) = std::fs::File::options()
            .read(true)
            .write(true)
            .open(&self.path)
        {
            drop(file);
        }

        // Remove the FIFO — subsequent open() calls in the reader will
        // fail with ENOENT, causing it to exit.
        let _ = std::fs::remove_file(&self.path);

        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }

        let buffer = std::mem::take(&mut *self.buffer.lock().unwrap());
        Ok(buffer)
    }
}

impl Drop for LingerReader {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);

        // Open as writer (O_RDWR) to unblock any thread blocked on open().
        if let Ok(file) = std::fs::File::options()
            .read(true)
            .write(true)
            .open(&self.path)
        {
            drop(file);
        }

        // Remove FIFO — subsequent open() calls will fail with ENOENT.
        let _ = std::fs::remove_file(&self.path);

        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}

/// Create a FIFO and start a background reader that accepts multiple writers.
///
/// Returns a `LingerReader` handle.  Call `.stop()` to stop reading and
/// retrieve all buffered messages.  The FIFO is cleaned up on stop (or drop).
///
/// The reader thread uses a blocking `open()` — it waits for a writer.
/// To stop the reader, call `stop()` which opens the FIFO as a writer
/// itself to unblock the reader's `open()`, then removes the FIFO.
pub fn start_linger(path: &Path, _timeout: Option<u64>) -> Result<LingerReader, String> {
    create_mcp_fifo(path)?;

    let stop = Arc::new(AtomicBool::new(false));
    let buffer = Arc::new(Mutex::new(Vec::<String>::new()));

    let stop_clone = stop.clone();
    let buffer_clone = buffer.clone();
    let path_buf = path.to_path_buf();
    let path_display = path.display().to_string();

    let handle = std::thread::Builder::new()
        .name(format!("linger-{}", path_display))
        .spawn(move || {
            loop {
                if stop_clone.load(Ordering::Relaxed) {
                    return;
                }

                // Blocking open() — waits for a writer (or until stop()
                // opens the FIFO as a writer to unblock us, then removes
                // the FIFO so the next open() fails with ENOENT).
                let mut file = match std::fs::File::open(&path_buf) {
                    Ok(f) => f,
                    Err(_) => {
                        // FIFO was removed — exit.
                        return;
                    }
                };

                let mut data = Vec::new();
                if let Err(_) = file.read_to_end(&mut data) {
                    return;
                }

                // Drop the file so the next open() connects to a new writer.
                drop(file);

                if data.is_empty() {
                    // Writer disconnected without data — wait for next.
                    continue;
                }

                let mut msg = String::from_utf8_lossy(&data).into_owned();
                if !msg.ends_with('\n') {
                    msg.push('\n');
                }

                buffer_clone.lock().unwrap().push(msg);
            }
        })
        .map_err(|e| format!("failed to spawn linger thread: {e}"))?;

    Ok(LingerReader {
        stop,
        buffer,
        handle: Some(handle),
        path: path.to_path_buf(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::io::Read;
    use std::os::unix::ffi::OsStrExt;
    use std::thread;
    use std::time::Duration;

    /// Helper: create a FIFO in a temp directory.
    fn make_fifo(dir: &Path, name: &str) -> PathBuf {
        let path = dir.join(name);
        // Use libc::mkfifo like the main code does.
        let c = std::ffi::CString::new(path.as_os_str().as_bytes()).unwrap();
        let ret = unsafe { libc::mkfifo(c.as_ptr(), 0o666) };
        assert_eq!(ret, 0, "mkfifo failed: {:?}", std::io::Error::last_os_error());
        path
    }

    #[test]
    fn test_write_to_fifo_happy_path() {
        let tmp = tempfile::tempdir().unwrap();
        let fifo = make_fifo(tmp.path(), "test.fifo");

        // Spawn a reader that opens the FIFO and reads whatever comes.
        let fifo_clone = fifo.clone();
        let reader = thread::spawn(move || {
            let mut buf = Vec::new();
            fs::File::options()
                .read(true)
                .open(&fifo_clone)
                .unwrap()
                .read_to_end(&mut buf)
                .unwrap();
            buf
        });

        // Give the reader thread time to block on open().
        thread::sleep(Duration::from_millis(50));

        // Write to the FIFO.
        let result = write_to_fifo(&fifo, "hello world");
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), 11);

        // Verify the reader got the data.
        let data = reader.join().unwrap();
        assert_eq!(data, b"hello world");
    }

    #[test]
    fn test_write_to_fifo_no_exist() {
        let tmp = tempfile::tempdir().unwrap();
        let fifo = tmp.path().join("nonexistent.fifo");

        let result = write_to_fifo(&fifo, "hello");
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("does not exist"));
    }

    #[test]
    fn test_write_to_fifo_not_a_fifo() {
        let tmp = tempfile::tempdir().unwrap();
        let file = tmp.path().join("regular_file");
        fs::write(&file, "not a fifo").unwrap();

        let result = write_to_fifo(&file, "hello");
        // open() succeeds on a regular file — that's fine, it's still valid.
        assert!(result.is_ok());
    }

    // ─── read_oneshot tests ────────────────────────────────────────

    #[test]
    fn test_read_oneshot_happy_path() {
        let tmp = tempfile::tempdir().unwrap();
        let fifo = tmp.path().join("oneshot.fifo");

        // Spawn a writer that waits for the FIFO to appear, then writes.
        let fifo_clone = fifo.clone();
        let writer = thread::spawn(move || {
            // Poll for FIFO existence (mimics CLI output mode).
            for _ in 0..10 {
                if fifo_clone.exists() {
                    break;
                }
                thread::sleep(Duration::from_millis(20));
            }
            write_to_fifo(&fifo_clone, "hello from writer").unwrap();
        });

        // Read one message — should create FIFO, read, then remove it.
        let result = read_oneshot(&fifo, None);
        writer.join().unwrap();

        assert!(result.is_ok());
        let msg = result.unwrap();
        assert_eq!(msg, "hello from writer\n");

        // FIFO should have been cleaned up.
        assert!(!fifo.exists(), "FIFO should be removed after oneshot read");
    }

    #[test]
    fn test_read_oneshot_timeout() {
        let tmp = tempfile::tempdir().unwrap();
        let fifo = tmp.path().join("timeout.fifo");

        // No writer — should time out after 1 second.
        let result = read_oneshot(&fifo, Some(1));
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("timed out"));

        // FIFO should have been cleaned up.
        assert!(!fifo.exists(), "FIFO should be removed on timeout");
    }

    #[test]
    fn test_read_oneshot_trailing_newline() {
        let tmp = tempfile::tempdir().unwrap();
        let fifo = tmp.path().join("newline.fifo");

        let fifo_clone = fifo.clone();
        let writer = thread::spawn(move || {
            for _ in 0..10 {
                if fifo_clone.exists() {
                    break;
                }
                thread::sleep(Duration::from_millis(20));
            }
            // Write without trailing newline.
            write_to_fifo(&fifo_clone, "no newline").unwrap();
        });

        let result = read_oneshot(&fifo, None);
        writer.join().unwrap();

        // Should have a trailing newline appended.
        let msg = result.unwrap();
        assert!(msg.ends_with('\n'), "message should end with newline");
    }

    // ─── linger reader tests ──────────────────────────────────────

    #[test]
    fn test_linger_single_writer() {
        let tmp = tempfile::tempdir().unwrap();
        let fifo = tmp.path().join("linger1.fifo");

        let reader = start_linger(&fifo, None).unwrap();
        assert!(fifo.exists(), "FIFO should exist after start_linger");

        // Give the reader thread time to block on open().
        thread::sleep(Duration::from_millis(50));

        write_to_fifo(&fifo, "msg one").unwrap();
        thread::sleep(Duration::from_millis(50));

        let msgs = reader.stop().unwrap();
        assert_eq!(msgs, vec!["msg one\n"]);
        assert!(!fifo.exists(), "FIFO should be removed after stop");
    }

    #[test]
    fn test_linger_multiple_writers() {
        let tmp = tempfile::tempdir().unwrap();
        let fifo = tmp.path().join("linger2.fifo");

        let reader = start_linger(&fifo, None).unwrap();
        thread::sleep(Duration::from_millis(50));

        write_to_fifo(&fifo, "first").unwrap();
        thread::sleep(Duration::from_millis(50));
        write_to_fifo(&fifo, "second").unwrap();
        thread::sleep(Duration::from_millis(50));
        write_to_fifo(&fifo, "third").unwrap();
        thread::sleep(Duration::from_millis(50));

        let msgs = reader.stop().unwrap();
        assert_eq!(msgs, vec!["first\n", "second\n", "third\n"]);
    }

    #[test]
    fn test_linger_stop_unblocks_fifo() {
        let tmp = tempfile::tempdir().unwrap();
        let fifo = tmp.path().join("linger3.fifo");

        let reader = start_linger(&fifo, None).unwrap();
        thread::sleep(Duration::from_millis(50));

        // Stop while no writer is connected — should not hang.
        let msgs = reader.stop().unwrap();
        assert!(msgs.is_empty());
    }

    #[test]
    fn test_linger_drop_cleans_up() {
        let tmp = tempfile::tempdir().unwrap();
        let fifo = tmp.path().join("linger4.fifo");

        {
            let _reader = start_linger(&fifo, None).unwrap();
            thread::sleep(Duration::from_millis(50));
            assert!(fifo.exists());
        }
        // _reader was dropped — FIFO should be gone.
        thread::sleep(Duration::from_millis(50));
        assert!(!fifo.exists(), "FIFO should be removed on drop");
    }

    #[test]
    fn test_linger_no_messages() {
        let tmp = tempfile::tempdir().unwrap();
        let fifo = tmp.path().join("linger5.fifo");

        let reader = start_linger(&fifo, None).unwrap();
        thread::sleep(Duration::from_millis(50));

        let msgs = reader.stop().unwrap();
        assert!(msgs.is_empty());
    }
}
