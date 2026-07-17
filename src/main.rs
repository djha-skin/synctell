use std::ffi::CString;
use std::fs;
use std::io::{self, Read, Write};
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::FileTypeExt;
use std::path::PathBuf;
use std::sync::{Arc, Condvar, Mutex};
use std::thread;
use std::time::Duration;

use anyhow::{Context, Result};
use clap::Parser;

#[derive(Parser)]
#[command(
    name = "synctell",
    version,
    about = "Instantly create and use FIFO special files",
    long_about = "synctell creates and interacts with FIFO (named pipe) special files.\n\n\
                  Output mode (-o): creates a FIFO and writes a message (or stdin) to it.\n\
                  Input mode (-i): polls for the existence of a file and reads it.\n\n\
                  Output mode creates a FIFO automatically and removes it after use.\n\
                  Input mode does not create anything; it polls the filesystem for\n\
                  the target file every second.  Use -t to set a timeout.\n\
                  Without -t, input exits immediately if the file is absent."
)]
struct Cli {
    /// Create a FIFO at FILE and write to it
    #[arg(short = 'o', long = "output", value_name = "FILE", conflicts_with = "input")]
    output: Option<PathBuf>,

    /// Poll for FILE and read its contents once it appears
    #[arg(short = 'i', long = "input", value_name = "FILE", conflicts_with = "output")]
    input: Option<PathBuf>,

    /// Seconds to wait for FILE to appear before timing out (input mode)
    #[arg(short = 't', long = "timeout", value_name = "SECS")]
    timeout: Option<u64>,

    /// Message to write to the FIFO (if omitted, reads from stdin)
    message: Option<String>,
}

fn main() -> Result<()> {
    let cli = Cli::parse();

    match (cli.output, cli.input) {
        (Some(path), None) => cmd_output(&path, cli.message.as_deref(), cli.timeout),
        (None, Some(path)) => cmd_input(&path, cli.timeout),
        _ => {
            eprintln!("error: exactly one of -o or -i must be specified");
            eprintln!("try 'synctell --help' for more information");
            std::process::exit(1);
        }
    }
}

// ─── Output mode ───────────────────────────────────────────────────

/// Output mode: create a FIFO and write data to it.
///
/// Data comes from the positional `message` argument if provided,
/// otherwise from stdin.  stdin is fully buffered first so we don't
/// deadlock on a full pipe.
///
/// If `timeout` is `Some(secs)`, we wait at most that many seconds for
/// a reader to open the other end of the FIFO.  If no reader appears
/// in time, the process exits with code 124.
fn cmd_output(path: &PathBuf, message: Option<&str>, timeout: Option<u64>) -> Result<()> {
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

    create_fifo(path)?;

    let path_display = path.display().to_string();

    match timeout {
        Some(secs) => cmd_output_with_timeout(path, &data, secs, &path_display),
        None => cmd_output_blocking(path, &data, &path_display),
    }
}

/// Blocking open + write (no timeout).  The open call blocks until a
/// reader opens the other end of the FIFO — standard FIFO semantics.
fn cmd_output_blocking(path: &PathBuf, data: &[u8], path_display: &str) -> Result<()> {
    let mut file = fs::File::options()
        .write(true)
        .open(path)
        .with_context(|| format!("failed to open '{path_display}' for writing"))?;

    file.write_all(data)
        .with_context(|| format!("failed to write to '{path_display}'"))?;

    // Clean up the FIFO after successful write.
    let _ = fs::remove_file(path);

    Ok(())
}

/// Open + write with a timeout.  A background thread performs the
/// blocking `open` + `write`; the main thread waits on a Condvar with
/// a deadline.  If the deadline expires first we exit with code 124.
fn cmd_output_with_timeout(
    path: &PathBuf,
    data: &[u8],
    secs: u64,
    path_display: &str,
) -> Result<()> {
    let pair: Arc<(Mutex<Option<io::Result<()>>>, Condvar)> =
        Arc::new((Mutex::new(None), Condvar::new()));
    let pair_clone = pair.clone();
    let data = data.to_vec();
    let path_clone = path.clone();
    let display = path_display.to_string();

    let _handle = thread::spawn(move || {
        let result = (|| -> io::Result<()> {
            let mut file = fs::File::options()
                .write(true)
                .open(&path_clone)
                .map_err(|e| io::Error::new(e.kind(), format!("failed to open '{display}' for writing: {e}")))?;
            file.write_all(&data)
                .map_err(|e| io::Error::new(e.kind(), format!("failed to write to '{display}': {e}")))
        })();

        let (lock, cvar) = &*pair_clone;
        *lock.lock().unwrap() = Some(result);
        cvar.notify_one();
    });

    let (lock, cvar) = &*pair;
    let guard = lock.lock().unwrap();
    let (mut guard, wait_result) = cvar
        .wait_timeout(guard, Duration::from_secs(secs))
        .unwrap();

    if wait_result.timed_out() {
        // The thread is still blocked on the FIFO open — clean up
        // the FIFO before exiting.
        let _ = fs::remove_file(path);
        eprintln!("error: timed out waiting for reader on '{path_display}'");
        std::process::exit(124);
    }

    let result = guard.take().unwrap();
    // Clean up the FIFO after successful write.
    let _ = fs::remove_file(path);
    result.context(format!("failed to write to '{path_display}'"))
}

// ─── Input mode ────────────────────────────────────────────────────

/// Input mode: poll for the existence of a file and read it.
///
/// Does **not** create anything — the file (typically a FIFO created
/// by a concurrent `-o` invocation) must already exist on disk.
///
/// - Without a timeout, exits immediately with code 1 if the file is
///   not already present.
/// - With a timeout, polls once per second.  If the file appears, it
///   is read and its contents are written to stdout.  If the timeout
///   elapses first the process exits with code 124.
fn cmd_input(path: &PathBuf, timeout: Option<u64>) -> Result<()> {
    let path_display = path.display().to_string();

    match timeout {
        Some(secs) => cmd_input_poll(path, secs, &path_display),
        None => {
            // No timeout — file must already exist or we bail out.
            if !path.exists() {
                eprintln!("error: '{path_display}' does not exist");
                std::process::exit(1);
            }
            read_and_stream(path, &path_display)
        }
    }
}

/// Poll for file existence every second, then read it.
///
/// Checks immediately, then sleeps 1 s between subsequent checks.
/// The total elapsed time is approximately `secs` seconds.
/// If the file has not appeared by then, exit with code 124.
fn cmd_input_poll(path: &PathBuf, secs: u64, path_display: &str) -> Result<()> {
    for i in 0..=secs {
        if path.exists() {
            return read_and_stream(path, path_display);
        }
        // Sleep between checks, but not after the very last one.
        if i < secs {
            thread::sleep(Duration::from_secs(1));
        }
    }

    eprintln!(
        "error: timed out waiting for '{path_display}' to appear"
    );
    std::process::exit(124);
}

/// Open an existing file (FIFO or regular) and stream its contents
/// to stdout.
fn read_and_stream(path: &PathBuf, path_display: &str) -> Result<()> {
    let mut file = fs::File::open(path)
        .with_context(|| format!("failed to open '{path_display}' for reading"))?;

    let mut stdout = io::stdout().lock();
    let mut buf = [0u8; 8192];

    loop {
        let n = file
            .read(&mut buf)
            .context(format!("failed to read from '{path_display}'"))?;
        if n == 0 {
            break;
        }
        stdout
            .write_all(&buf[..n])
            .context("failed to write to stdout")?;
    }

    Ok(())
}

// ─── FIFO creation ─────────────────────────────────────────────────

/// Create a POSIX FIFO at `path` with mode 0666 (umask-applied).
///
/// If the path already exists and is a FIFO this is a no-op.
/// If the path exists but is *not* a FIFO, an error is returned.
fn create_fifo(path: &PathBuf) -> Result<()> {
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
