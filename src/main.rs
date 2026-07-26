use std::ffi::CString;
use std::fs;
use std::io::{self, Read, Write};
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::FileTypeExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::thread;
use std::time::{Duration, Instant};

mod mcp;

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};

/// Flag set by signal handler to request graceful shutdown.
static QUIT: AtomicBool = AtomicBool::new(false);

/// POSIX signal handler: sets the QUIT flag.
///
/// # Safety
/// Only stores to an `AtomicBool`, which is signal-safe.
extern "C" fn handle_signal(_sig: libc::c_int) {
    QUIT.store(true, Ordering::Relaxed);
}

#[derive(Parser)]
#[command(
    name = "synctell",
    version,
    about = "Instantly create and use FIFO special files for inter-process messaging"
)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Create a FIFO and read messages from writers
    Read {
        /// Path for the FIFO to create
        path: PathBuf,

        /// Keep reading after the first message
        #[arg(short = 'l', long = "linger")]
        linger: bool,

        /// Seconds to wait for the first writer (0 = block forever)
        #[arg(short = 't', long = "timeout", value_name = "SECS")]
        timeout: Option<u64>,

        /// Hard deadline: quit after N seconds no matter what (0 = no limit)
        #[arg(short = 'm', long = "max-time", value_name = "SECS")]
        max_time: Option<u64>,
    },

    /// Poll for a FIFO and write a message to it
    Write {
        /// Path to the FIFO to write to
        path: PathBuf,

        /// Seconds to wait for the FIFO to appear (0 = must exist now)
        #[arg(short = 't', long = "timeout", value_name = "SECS")]
        timeout: Option<u64>,

        /// Message to write (if omitted, reads from stdin)
        message: Option<String>,
    },

    /// Broadcast: read from one FIFO, write to many
    Broadcast {
        /// Path for the input FIFO to create
        input: PathBuf,

        /// Paths to output FIFOs (must already exist)
        #[arg(required = true, num_args = 1..)]
        outputs: Vec<PathBuf>,

        /// Seconds to wait for the first writer (0 = block forever)
        #[arg(short = 't', long = "timeout", value_name = "SECS")]
        timeout: Option<u64>,

        /// Hard deadline: quit after N seconds no matter what (0 = no limit)
        #[arg(short = 'm', long = "max-time", value_name = "SECS")]
        max_time: Option<u64>,
    },

    /// Start an MCP server over stdio
    Mcp,
}

fn main() -> Result<()> {
    // Install signal handlers for graceful shutdown.
    unsafe {
        libc::signal(libc::SIGINT, handle_signal as *const () as libc::sighandler_t);
        libc::signal(libc::SIGTERM, handle_signal as *const () as libc::sighandler_t);
    }

    let cli = Cli::parse();

    match cli.command {
        Commands::Read {
            path,
            linger,
            timeout,
            max_time,
        } => cmd_input(&path, timeout, linger, max_time),
        Commands::Write {
            path,
            timeout,
            message,
        } => cmd_output(&path, message.as_deref(), timeout),
        Commands::Mcp => {
            // Block on the async MCP server.
            let rt = tokio::runtime::Runtime::new()?;
            rt.block_on(mcp::run())
        }
        Commands::Broadcast {
            input,
            outputs,
            timeout,
            max_time,
        } => cmd_broadcast(&input, &outputs, timeout, max_time),
    }
}

// ─── Output mode ───────────────────────────────────────────────────

/// Outcome of the background read-thread + condvar handoff.
type ReadOutcome = Arc<(Mutex<Option<io::Result<Vec<u8>>>>, Condvar)>;

/// Output mode: poll for a FIFO and write data to it.
///
/// Data comes from the positional `message` argument if provided,
/// otherwise from stdin.  stdin is fully buffered first so we don't
/// deadlock on a full pipe.
///
/// If `timeout` is `Some(secs)`, we poll for the FIFO's existence for
/// at most that many seconds.  If no FIFO appears in time, the
/// process exits with code 124.
/// Without a timeout, the FIFO must already exist or we exit with
/// code 1.
fn cmd_output(path: &Path, message: Option<&str>, timeout: Option<u64>) -> Result<()> {
    let data = match message {
        Some(msg) => msg.as_bytes().to_vec(),
        None => {
            let mut input = Vec::new();
            io::stdin()
                .read_to_end(&mut input)
                .context("failed to read from stdin")?;
            input
        }
    };

    let path_display = path.display().to_string();

    match timeout {
        Some(secs) => cmd_output_poll_and_write(path, &data, secs, &path_display),
        None => cmd_output_write(path, &data, &path_display),
    }
}

/// Write data to an existing FIFO (no timeout).
///
/// If the FIFO does not exist, exits with code 1.
/// The open call blocks until a reader has the other end open —
/// standard FIFO semantics.
fn cmd_output_write(path: &Path, data: &[u8], path_display: &str) -> Result<()> {
    if !path.exists() {
        eprintln!("error: '{path_display}' does not exist (no reader listening)");
        std::process::exit(1);
    }

    let mut file = fs::File::options()
        .write(true)
        .open(path)
        .with_context(|| {
            // Handle race: FIFO removed between exists() and open().
            if !path.exists() {
                format!("'{path_display}' disappeared (no reader listening)")
            } else {
                format!("failed to open '{path_display}' for writing")
            }
        })?;

    file.write_all(data)
        .with_context(|| format!("failed to write to '{path_display}'"))?;

    // No cleanup — the reader (creator) handles that.
    Ok(())
}

/// Poll for a FIFO, then write data to it.
///
/// Checks once per second for the FIFO's existence.  If it appears,
/// opens it and writes.  If the timeout elapses first, exit with
/// code 124.
fn cmd_output_poll_and_write(
    path: &Path,
    data: &[u8],
    secs: u64,
    path_display: &str,
) -> Result<()> {
    for i in 0..=secs {
        if path.exists() {
            let mut file = fs::File::options()
                .write(true)
                .open(path)
                .with_context(|| {
                    if !path.exists() {
                        format!("'{path_display}' disappeared (no reader listening)")
                    } else {
                        format!("failed to open '{path_display}' for writing")
                    }
                })?;

            return file
                .write_all(data)
                .with_context(|| format!("failed to write to '{path_display}'"));
        }
        if i < secs {
            thread::sleep(Duration::from_secs(1));
        }
    }

    eprintln!(
        "error: timed out waiting for '{path_display}' to appear (no reader listening)"
    );
    std::process::exit(124);
}

// ─── Input mode ────────────────────────────────────────────────────

/// Result of a single read attempt from the FIFO.
enum ReadResult {
    /// Successfully read data from a writer.
    Data(Vec<u8>),
    /// No writer connected before the deadline expired.
    TimedOut,
    /// The max-time hard deadline was exceeded.
    MaxTimeReached,
    /// A signal requested shutdown.
    Interrupted,
}

/// Input mode: create a FIFO and read messages from writers.
///
/// Creates a FIFO at `path` and reads from it.
/// The FIFO's presence on disk signals that a reader is listening.
///
/// - Without `--linger`, reads one message then exits.
/// - With `--linger`, stays alive reading from one or more writers
///   until interrupted by a signal.
///
/// - Without a timeout, blocks until a writer appears.
/// - With a timeout, waits at most `secs` seconds for the first
///   writer.  If no writer connects in time, exits with code 124.
///
/// Each message is line-framed on stdout: if the writer's bytes do
/// not end with `'\n'`, the reader appends one.  This makes
/// many-writer output cleanly line-separated.
/// The FIFO is removed on exit.
fn cmd_input(
    path: &Path,
    timeout: Option<u64>,
    linger: bool,
    max_time: Option<u64>,
) -> Result<()> {
    let path_display = path.display().to_string();

    // Reader creates the FIFO — its presence signals "someone is listening".
    create_fifo(path)?;

    // -t 0 means "no timeout" (block forever), same as omitting -t.
    let mut deadline = timeout
        .filter(|&s| s > 0)
        .map(|s| Instant::now() + Duration::from_secs(s));

    // -m 0 means "no max-time" (block forever), same as omitting -m.
    // Unlike timeout, max-time is NEVER discarded — it's a hard deadline.
    let max_deadline = max_time
        .filter(|&s| s > 0)
        .map(|s| Instant::now() + Duration::from_secs(s));

    loop {
        match read_one_message(path, deadline, max_deadline, &path_display)? {
            ReadResult::Data(data) => {
                io::stdout()
                    .write_all(&data)
                    .context("failed to write to stdout")?;
                // Ensure each message ends with a newline so multiple
                // writers' output is line-framed on stdout.  If the
                // writer's bytes already end with '\n', this is a no-op.
                if data.last() != Some(&b'\n') {
                    io::stdout()
                        .write_all(b"\n")
                        .context("failed to write to stdout")?;
                }
                if !linger {
                    break;
                }
                // After the first successful read, remove the timeout.
                // The reader persists until interrupted (linger mode).
                // max_deadline is NOT removed — it's a hard deadline.
                deadline = None;
            }
            ReadResult::TimedOut => {
                // No writer ever connected in time — clean up and exit.
                let _ = fs::remove_file(path);
                eprintln!("error: timed out waiting for writer on '{path_display}'");
                std::process::exit(124);
            }
            ReadResult::MaxTimeReached => break,
            ReadResult::Interrupted => break,
        }
    }

    // Clean up the FIFO on exit.
    let _ = fs::remove_file(path);
    Ok(())
}

/// Open the FIFO, read one complete message, and return it.
///
/// Uses a background thread for the blocking open + read so that the
/// calling thread can poll the result with 1-second intervals, checking
/// the QUIT flag each time for responsive signal handling.
fn read_one_message(
    path: &Path,
    deadline: Option<Instant>,
    max_deadline: Option<Instant>,
    path_display: &str,
) -> Result<ReadResult> {
    let outcome: ReadOutcome = Arc::new((Mutex::new(None), Condvar::new()));
    let outcome_clone = outcome.clone();
    let path_buf = path.to_path_buf();
    let display = path_display.to_string();

    // Spawn a thread to perform the blocking open + read.
    thread::spawn(move || {
        let result = (|| -> io::Result<Vec<u8>> {
            let mut file = fs::File::options()
                .read(true)
                .open(&path_buf)
                .map_err(|e| {
                    io::Error::new(
                        e.kind(),
                        format!("failed to open '{display}' for reading: {e}"),
                    )
                })?;
            let mut data = Vec::new();
            file.read_to_end(&mut data).map_err(|e| {
                io::Error::new(e.kind(), format!("failed to read from '{display}': {e}"))
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
        // Poll every second so we stay responsive to signals.
        let (new_guard, _) = cvar.wait_timeout(guard, Duration::from_secs(1)).unwrap();
        guard = new_guard;

        // Signal received — caller should break out of the read loop.
        if QUIT.load(Ordering::Relaxed) {
            return Ok(ReadResult::Interrupted);
        }

        // Read completed — return the data or the error.
        if guard.is_some() {
            return match guard.take().unwrap() {
                Ok(data) => Ok(ReadResult::Data(data)),
                Err(e) => Err(e.into()),
            };
        }

        // Deadline exceeded — no writer connected in time.
        if let Some(dl) = deadline
            && Instant::now() >= dl
        {
            return Ok(ReadResult::TimedOut);
        }

        // Max-time hard deadline exceeded — exit cleanly.
        if let Some(dl) = max_deadline
            && Instant::now() >= dl
        {
            return Ok(ReadResult::MaxTimeReached);
        }
    }
}

// ─── Broadcast mode ───────────────────────────────────────────────

/// Write data to a FIFO, blocking until a reader is ready.
///
/// Spawns a background thread for the blocking open+write so the
/// caller never hangs.  The thread is detached (no join) so the
/// fan-out loop in the broadcast can dispatch to all outputs
/// concurrently without waiting for any single output's reader.
/// Returns `Ok(bytes_written)` or `Err(message)` — but note that
/// when the FIFO has no reader, the thread will block on open()
/// indefinitely, so the Err case is only for immediate failures
/// (e.g. nonexistent path).
fn write_fifo_blocking(path: PathBuf, data: Vec<u8>) -> Result<usize, String> {
    if !path.exists() {
        return Err(format!(
            "'{}' does not exist (no reader listening)",
            path.display()
        ));
    }

    let n = data.len();
    let path_display = path.display().to_string();
    let _ = std::thread::spawn(move || {
        match std::fs::File::options()
            .write(true)
            .open(&path)
        {
            Ok(mut file) => {
                if let Err(e) = file.write_all(&data) {
                    eprintln!("warning: failed to write to '{}': {e}", path_display);
                }
            }
            Err(e) => {
                eprintln!("warning: failed to open '{}' for writing: {e}", path_display);
            }
        }
    });

    Ok(n)
}

/// Broadcast mode: create an input FIFO, read messages, fan out to
/// all output FIFOs.
///
/// Linger is implicitly always on for the input.
/// `-t N` timeout governs the wait for the first writer only — once
/// a message arrives the timeout is discarded.
/// Output FIFOs must already exist; missing ones are skipped per-write.
/// The input FIFO is removed on exit.
fn cmd_broadcast(
    input_path: &Path,
    output_paths: &[PathBuf],
    timeout: Option<u64>,
    max_time: Option<u64>,
) -> Result<()> {
    if output_paths.is_empty() {
        anyhow::bail!("broadcast requires at least one output FIFO");
    }

    create_fifo(input_path)?;

    let input_display = input_path.display().to_string();

    // Set up a channel: reader thread → broadcast loop.
    let (tx, rx) = std::sync::mpsc::channel::<Vec<u8>>();
    let stop = Arc::new(AtomicBool::new(false));
    let stop_clone = stop.clone();
    let input_clone = input_path.to_path_buf();

    // Reader thread: loops blocking on open → read_to_end → send.
    let reader_handle = thread::Builder::new()
        .name(format!("broadcast-reader-{input_display}"))
        .spawn(move || {
            loop {
                if stop_clone.load(Ordering::Relaxed) {
                    return;
                }
                let mut file = match fs::File::open(&input_clone) {
                    Ok(f) => f,
                    Err(_) => return, // FIFO removed — exit.
                };
                let mut data = Vec::new();
                if file.read_to_end(&mut data).is_err() {
                    return;
                }
                drop(file);
                if data.is_empty() {
                    continue; // Writer disconnected with no data.
                }
                if tx.send(data).is_err() {
                    return; // Receiver dropped — exit.
                }
            }
        })
        .context("failed to spawn broadcast reader thread")?;

    // -t 0 means "no timeout" (block forever), same as omitting -t.
    let deadline = timeout
        .filter(|&s| s > 0)
        .map(|s| Instant::now() + Duration::from_secs(s));

    // -m 0 means "no max-time" (block forever), same as omitting -m.
    // Unlike timeout, max-time is NEVER discarded — it's a hard deadline.
    let max_deadline = max_time
        .filter(|&s| s > 0)
        .map(|s| Instant::now() + Duration::from_secs(s));

    // Main loop: receive messages from the reader and fan out.
    loop {
        // Check max-time hard deadline first.
        if let Some(dl) = max_deadline
            && Instant::now() >= dl
        {
            break;
        }
        match rx.recv_timeout(Duration::from_secs(1)) {
            Ok(data) => {
                // Ensure trailing newline.
                let mut msg = data;
                if msg.last() != Some(&b'\n') {
                    msg.push(b'\n');
                }

                for output in output_paths {
                    let out = output.clone();
                    let msg_clone = msg.clone();
                    if let Err(e) = write_fifo_blocking(out, msg_clone) {
                        eprintln!("warning: {e}");
                    }
                }
            }
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
                // Check timeout deadline.
                if let Some(dl) = deadline {
                    if Instant::now() >= dl {
                        // Clean up before exiting (124 = timeout).
                        stop.store(true, Ordering::Relaxed);
                        if let Ok(file) = std::fs::File::options()
                            .read(true)
                            .write(true)
                            .open(input_path)
                        {
                            drop(file);
                        }
                        let _ = fs::remove_file(input_path);
                        let _ = reader_handle.join();
                        eprintln!(
                            "error: timed out waiting for writer on '{input_display}'"
                        );
                        std::process::exit(124);
                    }
                }
                // Check signal.
                if QUIT.load(Ordering::Relaxed) {
                    break;
                }
            }
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
                // Reader thread exited (FIFO removed or error).
                break;
            }
        }
    }

    // Cleanup.
    stop.store(true, Ordering::Relaxed);

    // Open the FIFO as a writer (O_RDWR) to unblock the reader thread
    // which is blocked on open(). On Linux, O_RDWR always succeeds on
    // a FIFO regardless of whether a reader is connected, which
    // unblocks the reader's O_RDONLY open(). Once the reader sees the
    // stop flag, it exits.
    if let Ok(file) = std::fs::File::options()
        .read(true)
        .write(true)
        .open(input_path)
    {
        drop(file);
    }

    let _ = fs::remove_file(input_path);
    let _ = reader_handle.join();

    Ok(())
}

// ─── FIFO creation ─────────────────────────────────────────────────

/// Create a POSIX FIFO at `path` with mode 0666 (umask-applied).
///
/// If the path already exists and is a FIFO this is a no-op.
/// If the path exists but is *not* a FIFO, an error is returned.
fn create_fifo(path: &Path) -> Result<()> {
    let c_path = CString::new(path.as_os_str().as_bytes())
        .context("path contains a null byte")?;

    let ret = unsafe { libc::mkfifo(c_path.as_ptr(), 0o666) };

    if ret == 0 {
        return Ok(());
    }

    let err = io::Error::last_os_error();

    // EEXIST — file already exists.  We used to treat an existing FIFO
    // as a no-op, but that's a bug: if someone else already created that
    // FIFO, they own the channel.  Always reject pre-existing files.
    if err.raw_os_error() == Some(libc::EEXIST) {
        let meta = fs::metadata(path)
            .with_context(|| format!("cannot stat '{}'", path.display()))?;

        if meta.file_type().is_fifo() {
            anyhow::bail!(
                "'{}' already exists as a FIFO — another listener may own this channel",
                path.display()
            );
        }

        anyhow::bail!("'{}' already exists but is not a FIFO", path.display());
    }

    Err(err).context(format!("failed to create FIFO at '{}'", path.display()))
}
