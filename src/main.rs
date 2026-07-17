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
    name = "tell",
    version,
    about = "Instantly create and use FIFO special files",
    long_about = "tell creates and interacts with FIFO (named pipe) special files.\n\n\
                  Output mode (-o): creates a FIFO and writes a message (or stdin) to it.\n\
                  Input mode (-i): reads from an existing FIFO and writes to stdout."
)]
struct Cli {
    /// Create a FIFO at FILE and write to it
    #[arg(short = 'o', long = "output", value_name = "FILE", conflicts_with = "input")]
    output: Option<PathBuf>,

    /// Read from an existing FIFO at FILE and write to stdout
    #[arg(short = 'i', long = "input", value_name = "FILE", conflicts_with = "output")]
    input: Option<PathBuf>,

    /// Seconds to wait for a reader before timing out (output mode only)
    #[arg(short = 't', long = "timeout", value_name = "SECS", conflicts_with = "input")]
    timeout: Option<u64>,

    /// Message to write to the FIFO (if omitted, reads from stdin)
    message: Option<String>,
}

fn main() -> Result<()> {
    let cli = Cli::parse();

    match (cli.output, cli.input) {
        (Some(path), None) => cmd_output(&path, cli.message.as_deref(), cli.timeout),
        (None, Some(path)) => cmd_input(&path),
        _ => {
            eprintln!("error: exactly one of -o or -i must be specified");
            eprintln!("try 'tell --help' for more information");
            std::process::exit(1);
        }
    }
}

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

/// Input mode: verify the path is a FIFO, then stream it to stdout.
fn cmd_input(path: &PathBuf) -> Result<()> {
    let metadata = fs::metadata(path)
        .with_context(|| format!("'{}' does not exist or is not accessible", path.display()))?;

    if !metadata.file_type().is_fifo() {
        let kind = if metadata.file_type().is_dir() {
            "directory"
        } else if metadata.file_type().is_file() {
            "regular file"
        } else {
            "other special file"
        };
        anyhow::bail!("'{}' exists but is not a FIFO (it is a {kind})", path.display());
    }

    // Open for reading.  Blocks until a writer opens the other end.
    let mut file = fs::File::open(path)
        .with_context(|| format!("failed to open '{}' for reading", path.display()))?;

    let mut stdout = io::stdout().lock();
    let mut buf = [0u8; 8192];

    loop {
        let n = file
            .read(&mut buf)
            .context("failed to read from FIFO")?;
        if n == 0 {
            break;
        }
        stdout
            .write_all(&buf[..n])
            .context("failed to write to stdout")?;
    }

    Ok(())
}

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
