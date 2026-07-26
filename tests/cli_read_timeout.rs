//! Integration tests for the `synctell read` subcommand's -t flag.
//!
//! Spawns the binary as a subprocess to test timeout exit codes
//! (since cmd_input calls std::process::exit which kills the test).

use std::process::Command;
use std::time::Duration;

fn run_read(args: &[&str]) -> std::process::Output {
    let mut child = Command::new(env!("CARGO_BIN_EXE_synctell"))
        .arg("read")
        .args(args)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .expect("failed to spawn synctell read");

    // Hard kill after 15s to prevent test hangs.
    let start = std::time::Instant::now();
    let kill_timeout = Duration::from_secs(15);
    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                // Process exited — collect output.
                let mut stdout = child.stdout.take().unwrap();
                let mut stderr = child.stderr.take().unwrap();
                let mut stdout_buf = Vec::new();
                let mut stderr_buf = Vec::new();
                std::io::Read::read_to_end(&mut stdout, &mut stdout_buf).unwrap();
                std::io::Read::read_to_end(&mut stderr, &mut stderr_buf).unwrap();
                return std::process::Output {
                    status,
                    stdout: stdout_buf,
                    stderr: stderr_buf,
                };
            }
            Ok(None) => {
                if start.elapsed() > kill_timeout {
                    let _ = child.kill();
                    let _ = child.wait();
                    panic!("synctell read timed out after {kill_timeout:?} — likely hung");
                }
                std::thread::sleep(Duration::from_millis(50));
            }
            Err(e) => panic!("try_wait failed: {e}"),
        }
    }
}

/// Helper: unique FIFO path under /tmp.
fn fifo_path(name: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join("synctell-cli-read-tests");
    let _ = std::fs::create_dir_all(&dir);
    dir.join(name)
}

fn cleanup(path: &std::path::Path) {
    let _ = std::fs::remove_file(path);
}

// ─── Tests ─────────────────────────────────────────────────────────

/// `synctell read -t 2 <fifo>` with no writer → exit 124.
#[test]
fn read_timeout_no_writer_exits_124() {
    let path = fifo_path("timeout_no_writer.fifo");
    cleanup(&path);

    let start = std::time::Instant::now();
    let output = run_read(&["-t", "2", path.to_str().unwrap()]);
    let elapsed = start.elapsed();

    assert_eq!(
        output.status.code(),
        Some(124),
        "should exit 124 on timeout, got {:?}\nstderr: {}",
        output.status.code(),
        String::from_utf8_lossy(&output.stderr)
    );

    // Should have waited roughly 2 seconds (±1s tolerance).
    assert!(
        elapsed >= Duration::from_secs(1),
        "should wait for timeout, only took {elapsed:?}"
    );
    assert!(
        elapsed < Duration::from_secs(10),
        "took too long: {elapsed:?}"
    );

    // FIFO should be cleaned up.
    assert!(!path.exists(), "FIFO should be removed after timeout");
}

/// `synctell read -t 2 -L <fifo>` with no writer → exit 124 (no-linger mode).
#[test]
fn read_timeout_no_linger_no_writer_exits_124() {
    let path = fifo_path("timeout_no_linger.fifo");
    cleanup(&path);

    let start = std::time::Instant::now();
    let output = run_read(&["-t", "2", "-L", path.to_str().unwrap()]);
    let elapsed = start.elapsed();

    assert_eq!(
        output.status.code(),
        Some(124),
        "should exit 124 on timeout, got {:?}\nstderr: {}",
        output.status.code(),
        String::from_utf8_lossy(&output.stderr)
    );

    assert!(
        elapsed >= Duration::from_secs(1),
        "should wait for timeout, only took {elapsed:?}"
    );
    assert!(
        elapsed < Duration::from_secs(10),
        "took too long: {elapsed:?}"
    );
    assert!(!path.exists(), "FIFO should be removed after timeout");
}

/// `synctell read -t 2 -l <fifo>` with no writer → exit 124.
#[test]
fn read_timeout_linger_no_writer_exits_124() {
    let path = fifo_path("timeout_linger_no_writer.fifo");
    cleanup(&path);

    let start = std::time::Instant::now();
    let output = run_read(&["-t", "2", "-l", path.to_str().unwrap()]);
    let elapsed = start.elapsed();

    assert_eq!(
        output.status.code(),
        Some(124),
        "should exit 124 on timeout, got {:?}\nstderr: {}",
        output.status.code(),
        String::from_utf8_lossy(&output.stderr)
    );

    assert!(
        elapsed >= Duration::from_secs(1),
        "should wait for timeout, only took {elapsed:?}"
    );
    assert!(
        elapsed < Duration::from_secs(10),
        "took too long: {elapsed:?}"
    );
    assert!(!path.exists(), "FIFO should be removed after timeout");
}

/// `synctell read -t 0 -L <fifo>` with no writer → blocks until writer arrives
/// (treated as no timeout, same as omitting -t, but with oneshot mode).
#[test]
fn read_timeout_zero_blocks_forever() {
    let path = fifo_path("timeout_zero.fifo");
    cleanup(&path);

    let path_clone = path.clone();
    let writer = std::thread::spawn(move || {
        // Wait for FIFO to appear.
        for _ in 0..50 {
            if path_clone.exists() {
                break;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        assert!(path_clone.exists(), "FIFO should exist");

        // Wait a bit for reader to block on open, then write.
        std::thread::sleep(Duration::from_millis(100));
        let mut file = std::fs::File::options()
            .write(true)
            .open(&path_clone)
            .unwrap();
        std::io::Write::write_all(&mut file, b"hello from writer").unwrap();
    });

    let output = run_read(&["-t", "0", "-L", path.to_str().unwrap()]);
    writer.join().unwrap();

    assert_eq!(
        output.status.code(),
        Some(0),
        "should exit 0 on success, got {:?}\nstderr: {}",
        output.status.code(),
        String::from_utf8_lossy(&output.stderr)
    );

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("hello from writer"),
        "should contain the message: {stdout:?}"
    );

    assert!(!path.exists(), "FIFO should be removed after read");
}

// ─── Max-time (-m) tests ───────────────────────────────────────────

/// `synctell read <fifo>` (default linger) stays alive for multiple writers.
#[test]
fn read_default_linger_multiple_writers() {
    let path = fifo_path("default_linger.fifo");
    cleanup(&path);

    let path_clone = path.clone();
    let writer = std::thread::spawn(move || {
        for _ in 0..50 {
            if path_clone.exists() {
                break;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        assert!(path_clone.exists(), "FIFO should exist");
        std::thread::sleep(Duration::from_millis(100));

        // Write first message.
        let mut file = std::fs::File::options()
            .write(true)
            .open(&path_clone)
            .unwrap();
        std::io::Write::write_all(&mut file, b"first").unwrap();
        drop(file);

        std::thread::sleep(Duration::from_millis(200));

        // Write second message — default linger should receive it.
        let mut file = std::fs::File::options()
            .write(true)
            .open(&path_clone)
            .unwrap();
        std::io::Write::write_all(&mut file, b"second").unwrap();
    });

    // Run read with no -l flag (default linger = on).
    let output = run_read(&["-m", "6", path.to_str().unwrap()]);
    writer.join().unwrap();

    assert_eq!(
        output.status.code(),
        Some(0),
        "should exit 0, got {:?}",
        output.status.code()
    );

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("first"),
        "should have received first message: {stdout:?}"
    );
    assert!(
        stdout.contains("second"),
        "should have received second message (default linger): {stdout:?}"
    );

    assert!(!path.exists(), "FIFO should be removed");
}

/// `synctell read -L <fifo>` exits after the first message (no linger).
#[test]
fn read_no_linger_exits_after_one() {
    let path = fifo_path("no_linger.fifo");
    cleanup(&path);

    let path_clone = path.clone();
    let writer = std::thread::spawn(move || {
        for _ in 0..50 {
            if path_clone.exists() {
                break;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        assert!(path_clone.exists(), "FIFO should exist");
        std::thread::sleep(Duration::from_millis(100));
        let mut file = std::fs::File::options()
            .write(true)
            .open(&path_clone)
            .unwrap();
        std::io::Write::write_all(&mut file, b"only one").unwrap();
    });

    // -L = no linger, should exit after one message.
    let output = run_read(&["-L", path.to_str().unwrap()]);
    writer.join().unwrap();

    assert_eq!(
        output.status.code(),
        Some(0),
        "should exit 0, got {:?}",
        output.status.code()
    );

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("only one"),
        "should contain the message: {stdout:?}"
    );

    assert!(!path.exists(), "FIFO should be removed");
}

/// `synctell read -l -L <fifo>` — last flag wins: -L means no-linger.
#[test]
fn read_linger_then_no_linger_last_wins() {
    let path = fifo_path("linger_then_no_linger.fifo");
    cleanup(&path);

    let path_clone = path.clone();
    let writer = std::thread::spawn(move || {
        for _ in 0..50 {
            if path_clone.exists() {
                break;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        assert!(path_clone.exists(), "FIFO should exist");
        std::thread::sleep(Duration::from_millis(100));
        let mut file = std::fs::File::options()
            .write(true)
            .open(&path_clone)
            .unwrap();
        std::io::Write::write_all(&mut file, b"only one").unwrap();
    });

    // -l then -L — last one wins, so no-linger.
    let output = run_read(&["-l", "-L", path.to_str().unwrap()]);
    writer.join().unwrap();

    assert_eq!(
        output.status.code(),
        Some(0),
        "should exit 0, got {:?}",
        output.status.code()
    );

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("only one"),
        "should contain the message: {stdout:?}"
    );

    assert!(!path.exists(), "FIFO should be removed");
}

/// `synctell read -L -l <fifo>` — last flag wins: -l means linger.
#[test]
fn read_no_linger_then_linger_last_wins() {
    let path = fifo_path("no_linger_then_linger.fifo");
    cleanup(&path);

    let path_clone = path.clone();
    let writer = std::thread::spawn(move || {
        for _ in 0..50 {
            if path_clone.exists() {
                break;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        assert!(path_clone.exists(), "FIFO should exist");
        std::thread::sleep(Duration::from_millis(100));
        let mut file = std::fs::File::options()
            .write(true)
            .open(&path_clone)
            .unwrap();
        std::io::Write::write_all(&mut file, b"first").unwrap();
        drop(file);
        std::thread::sleep(Duration::from_millis(200));
        let mut file = std::fs::File::options()
            .write(true)
            .open(&path_clone)
            .unwrap();
        std::io::Write::write_all(&mut file, b"second").unwrap();
    });

    // -L then -l — last one wins, so linger.
    let output = run_read(&["-L", "-l", "-m", "6", path.to_str().unwrap()]);
    writer.join().unwrap();

    assert_eq!(
        output.status.code(),
        Some(0),
        "should exit 0, got {:?}",
        output.status.code()
    );

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("first"),
        "should contain first message: {stdout:?}"
    );
    assert!(
        stdout.contains("second"),
        "should contain second message (linger): {stdout:?}"
    );

    assert!(!path.exists(), "FIFO should be removed");
}

/// `synctell read -m 2 <fifo>` with no writer → exits after ~2s (exit 0, no data).
/// Unlike -t, -m is a hard deadline: exit 0 even if no writer connects.
#[test]
fn read_max_time_no_writer_exits_cleanly() {
    let path = fifo_path("max_time_no_writer.fifo");
    cleanup(&path);

    let start = std::time::Instant::now();
    let output = run_read(&["-m", "2", path.to_str().unwrap()]);
    let elapsed = start.elapsed();

    // -m exits 0, not 124 — it's a hard deadline, not a timeout error.
    assert_eq!(
        output.status.code(),
        Some(0),
        "should exit 0 on max-time, got {:?}",
        output.status.code()
    );

    assert!(
        elapsed >= Duration::from_secs(1),
        "should wait ~2s, only took {elapsed:?}"
    );
    assert!(
        elapsed < Duration::from_secs(10),
        "took too long: {elapsed:?}"
    );

    assert!(!path.exists(), "FIFO should be removed");
}

/// `synctell read -l -m 2 <fifo>` with a writer: receives messages, then exits cleanly after 2s.
#[test]
fn read_max_time_linger_receives_then_exits() {
    let path = fifo_path("max_time_linger.fifo");
    cleanup(&path);

    let path_clone = path.clone();
    let writer = std::thread::spawn(move || {
        // Wait for FIFO to appear.
        for _ in 0..50 {
            if path_clone.exists() {
                break;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        assert!(path_clone.exists(), "FIFO should exist");

        // Give reader time to block on open, then write.
        std::thread::sleep(Duration::from_millis(100));
        let mut file = std::fs::File::options()
            .write(true)
            .open(&path_clone)
            .unwrap();
        std::io::Write::write_all(&mut file, b"first message").unwrap();
        drop(file);

        // After a short delay, write another message.
        std::thread::sleep(Duration::from_millis(500));
        let mut file = std::fs::File::options()
            .write(true)
            .open(&path_clone)
            .unwrap();
        std::io::Write::write_all(&mut file, b"second message").unwrap();
    });

    // -m 4 to give time for both messages, but not forever.
    let output = run_read(&["-l", "-m", "4", path.to_str().unwrap()]);
    writer.join().unwrap();

    assert_eq!(
        output.status.code(),
        Some(0),
        "should exit 0 on max-time, got {:?}",
        output.status.code()
    );

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("first message"), "should have received first message: {stdout:?}");
    assert!(stdout.contains("second message"), "should have received second message: {stdout:?}");

    assert!(!path.exists(), "FIFO should be removed");
}

/// `synctell read -m 0 -L <fifo>` behaves same as no -m (blocks forever).
#[test]
fn read_max_time_zero_blocks_forever() {
    let path = fifo_path("max_time_zero.fifo");
    cleanup(&path);

    let path_clone = path.clone();
    let writer = std::thread::spawn(move || {
        for _ in 0..50 {
            if path_clone.exists() {
                break;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        assert!(path_clone.exists(), "FIFO should exist");
        std::thread::sleep(Duration::from_millis(100));
        let mut file = std::fs::File::options()
            .write(true)
            .open(&path_clone)
            .unwrap();
        std::io::Write::write_all(&mut file, b"hello after wait").unwrap();
    });

    let output = run_read(&["-m", "0", "-L", path.to_str().unwrap()]);
    writer.join().unwrap();

    assert_eq!(
        output.status.code(),
        Some(0),
        "should exit 0, got {:?}",
        output.status.code()
    );

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("hello after wait"), "should contain message: {stdout:?}");

    assert!(!path.exists(), "FIFO should be removed");
}
