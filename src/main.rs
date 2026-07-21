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

use anyhow::{Context, Result};
use clap::Parser;

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
    about = "Instantly create and use FIFO special files for inter-process messaging",
    long_about = "synctell creates and interacts with FIFO (named pipe) special files.\n\n\
                  Input mode (-i): creates a FIFO and reads messages from writers.\n\
                  Output mode (-o): polls for a FIFO and writes a message to it.\n\n\
                  Input mode creates a FIFO automatically and stays alive,\n\
                  reading messages from one or more writers until interrupted.\n\
                  Without --linger, the reader exits after the first message.\n\
                  With --linger, it stays alive for more writers until interrupted.\n\
                  Its presence on disk signals that a reader is listening.\n\
                  The FIFO is removed when the reader exits.\n\n\
                  Output mode does not create anything; it polls the filesystem for\n\
                  the target file every second.  Use -t to set a timeout.\n\
                  Without -t, output exits immediately if the file is absent."
)]
struct Cli {
    /// Poll for FILE and write to it once it appears
    #[arg(short = 'o', long = "output", value_name = "FILE", conflicts_with = "input")]
    output: Option<PathBuf>,

    /// Create a FIFO at FILE and read from it
    #[arg(short = 'i', long = "input", value_name = "FILE", conflicts_with = "output")]
    input: Option<PathBuf>,

    /// Keep reading after the first message (only with -i)
    #[arg(short = 'l', long = "linger", conflicts_with = "output")]
    linger: bool,

    /// Seconds to wait before timing out
    #[arg(short = 't', long = "timeout", value_name = "SECS")]
    timeout: Option<u64>,

    /// Message to write to the FIFO (if omitted, reads from stdin)
    message: Option<String>,
}

fn main() -> Result<()> {
    // Install signal handlers for graceful shutdown.
    unsafe {
        libc::signal(libc::SIGINT, handle_signal as *const () as libc::sighandler_t);
        libc::signal(libc::SIGTERM, handle_signal as *const () as libc::sighandler_t);
    }

    let cli = Cli::parse();

    match (cli.output, cli.input) {
        (Some(path), None) => cmd_output(&path, cli.message.as_deref(), cli.timeout),
        (None, Some(path)) => cmd_input(&path, cli.timeout, cli.linger),
        _ => {
            eprintln!("error: exactly one of -o or -i must be specified");
            eprintln!("try 'synctell --help' for more information");
            std::process::exit(1);
        }
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
fn cmd_input(path: &Path, timeout: Option<u64>, linger: bool) -> Result<()> {
    let path_display = path.display().to_string();

    // Reader creates the FIFO — its presence signals "someone is listening".
    create_fifo(path)?;

    let mut deadline = timeout.map(|s| Instant::now() + Duration::from_secs(s));

    loop {
        match read_one_message(path, deadline, &path_display)? {
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
                deadline = None;
            }
            ReadResult::TimedOut => {
                // No writer ever connected in time — clean up and exit.
                let _ = fs::remove_file(path);
                eprintln!("error: timed out waiting for writer on '{path_display}'");
                std::process::exit(124);
            }
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
    }
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

    // EEXIST — file already exists; confirm it is actually a FIFO.
    if err.raw_os_error() == Some(libc::EEXIST) {
        let meta = fs::metadata(path)
            .with_context(|| format!("cannot stat '{}'", path.display()))?;

        if meta.file_type().is_fifo() {
            return Ok(());
        }

        anyhow::bail!("'{}' already exists but is not a FIFO", path.display());
    }

    Err(err).context(format!("failed to create FIFO at '{}'", path.display()))
}