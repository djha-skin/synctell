//! Integration tests for `synctell broadcast`.

use std::io::{Read, Write};
use std::process::{Child, Command, Stdio};
use std::time::Duration;

fn fifo_path(name: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join("synctell-cli-broadcast-tests");
    let _ = std::fs::create_dir_all(&dir);
    dir.join(name)
}

fn cleanup(path: &std::path::Path) {
    let _ = std::fs::remove_file(path);
}

/// Spawn `synctell broadcast` and return the child (don't wait).
fn spawn_broadcast(args: &[&str]) -> Child {
    Command::new(env!("CARGO_BIN_EXE_synctell"))
        .arg("broadcast")
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("failed to spawn synctell broadcast")
}

/// Wait for a child process to exit, with a hard timeout.
fn wait_with_timeout(child: &mut Child, timeout: Duration) -> Option<std::process::ExitStatus> {
    let start = std::time::Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(status)) => return Some(status),
            Ok(None) => {
                if start.elapsed() > timeout {
                    let _ = child.kill();
                    let _ = child.wait();
                    return None;
                }
                std::thread::sleep(Duration::from_millis(50));
            }
            Err(_) => return None,
        }
    }
}

/// Create a POSIX FIFO using libc (same as the production code).
fn create_test_fifo(path: &std::path::Path) {
    use std::os::unix::ffi::OsStrExt;
    let c = std::ffi::CString::new(path.as_os_str().as_bytes()).unwrap();
    let ret = unsafe { libc::mkfifo(c.as_ptr(), 0o666) };
    assert_eq!(ret, 0, "mkfifo failed: {:?}", std::io::Error::last_os_error());
}

/// Write directly to a FIFO (blocking open).
fn write_fifo(path: &std::path::Path, msg: &str) {
    use std::io::Write;
    let mut file = std::fs::File::options()
        .write(true)
        .open(path)
        .unwrap();
    file.write_all(msg.as_bytes()).unwrap();
}

/// Read from a FIFO (blocking open, read to EOF).
fn read_fifo(path: &std::path::Path) -> Vec<u8> {
    let mut buf = Vec::new();
    std::fs::File::options()
        .read(true)
        .open(path)
        .unwrap()
        .read_to_end(&mut buf)
        .unwrap();
    buf
}

// ─── Tests ─────────────────────────────────────────────────────────

/// Broadcast fans out one message to 3 outputs.
#[test]
fn broadcast_fans_out() {
    let input = fifo_path("fans_out_input.fifo");
    let out1 = fifo_path("fans_out_o1.fifo");
    let out2 = fifo_path("fans_out_o2.fifo");
    let out3 = fifo_path("fans_out_o3.fifo");
    cleanup(&input);
    cleanup(&out1);
    cleanup(&out2);
    cleanup(&out3);

    // Create output FIFOs.
    create_test_fifo(&out1);
    create_test_fifo(&out2);
    create_test_fifo(&out3);

    // Start broadcast.
    let mut child = spawn_broadcast(&[
        input.to_str().unwrap(),
        out1.to_str().unwrap(),
        out2.to_str().unwrap(),
        out3.to_str().unwrap(),
    ]);

    // Wait for input FIFO to appear.
    for _ in 0..50 {
        if input.exists() {
            break;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    assert!(input.exists(), "input FIFO should exist");

    // Spawn readers on each output FIFO (blocking open).
    let o1 = out1.clone();
    let r1 = std::thread::spawn(move || read_fifo(&o1));
    let o2 = out2.clone();
    let r2 = std::thread::spawn(move || read_fifo(&o2));
    let o3 = out3.clone();
    let r3 = std::thread::spawn(move || read_fifo(&o3));

    // Give reader threads time to block on open().
    std::thread::sleep(Duration::from_millis(100));

    // Write to input.
    write_fifo(&input, "hello broadcast");

    // Collect results.
    let d1 = r1.join().unwrap();
    let d2 = r2.join().unwrap();
    let d3 = r3.join().unwrap();

    assert_eq!(d1, b"hello broadcast\n", "output 1 mismatch");
    assert_eq!(d2, b"hello broadcast\n", "output 2 mismatch");
    assert_eq!(d3, b"hello broadcast\n", "output 3 mismatch");

    // Kill broadcast and clean up.
    let _ = child.kill();
    let _ = child.wait();
    cleanup(&input);
    cleanup(&out1);
    cleanup(&out2);
    cleanup(&out3);
}

/// Broadcast with -t exits 124 when no writer connects.
#[test]
fn broadcast_timeout_exits_124() {
    let input = fifo_path("timeout_124_input.fifo");
    let out1 = fifo_path("timeout_124_o1.fifo");
    cleanup(&input);
    cleanup(&out1);
    create_test_fifo(&out1);

    let start = std::time::Instant::now();
    let mut child = spawn_broadcast(&[
        "-t", "2",
        input.to_str().unwrap(),
        out1.to_str().unwrap(),
    ]);

    let status = wait_with_timeout(&mut child, Duration::from_secs(10));
    let elapsed = start.elapsed();

    assert_eq!(
        status.unwrap().code(),
        Some(124),
        "should exit 124 on timeout"
    );
    assert!(
        elapsed >= Duration::from_secs(1),
        "should wait for timeout, took {elapsed:?}"
    );
    assert!(
        elapsed < Duration::from_secs(8),
        "took too long: {elapsed:?}"
    );
    assert!(!input.exists(), "input FIFO should be cleaned up");
    cleanup(&out1);
}

/// Broadcast continues when one output disappears.
#[test]
fn broadcast_survives_output_disconnect() {
    let input = fifo_path("survive_input.fifo");
    let out1 = fifo_path("survive_o1.fifo");
    let out2 = fifo_path("survive_o2.fifo");
    cleanup(&input);
    cleanup(&out1);
    cleanup(&out2);
    create_test_fifo(&out1);
    create_test_fifo(&out2);

    let mut child = spawn_broadcast(&[
        input.to_str().unwrap(),
        out1.to_str().unwrap(),
        out2.to_str().unwrap(),
    ]);

    // Wait for input FIFO to appear.
    for _ in 0..50 {
        if input.exists() {
            break;
        }
        std::thread::sleep(Duration::from_millis(20));
    }

    // Remove out1 (simulates disconnect).  Broadcast should ignore it.
    cleanup(&out1);

    // Open out2 for reading.
    let o2 = out2.clone();
    let r2 = std::thread::spawn(move || {
        let mut buf = Vec::new();
        std::fs::File::options()
            .read(true)
            .open(&o2)
            .unwrap()
            .read_to_end(&mut buf)
            .unwrap();
        buf
    });
    std::thread::sleep(Duration::from_millis(200));

    // Write to input — should succeed (out1 gone, out2 still alive).
    {
        let mut file = std::fs::File::options()
            .write(true)
            .open(&input)
            .unwrap();
        file.write_all(b"still alive").unwrap();
    }

    let d2 = r2.join().unwrap();
    assert_eq!(d2, b"still alive\n", "output 2 should still receive data");

    // Kill broadcast and clean up.
    let _ = child.kill();
    let _ = child.wait();
    cleanup(&input);
    cleanup(&out1);
    cleanup(&out2);
}

/// Broadcast cleans up input FIFO on exit.
#[test]
fn broadcast_cleans_up_input() {
    let input = fifo_path("cleanup_input.fifo");
    let out1 = fifo_path("cleanup_o1.fifo");
    cleanup(&input);
    cleanup(&out1);
    create_test_fifo(&out1);

    let mut child = spawn_broadcast(&[
        "-t", "1",
        input.to_str().unwrap(),
        out1.to_str().unwrap(),
    ]);

    // Wait for input FIFO to appear.
    for _ in 0..50 {
        if input.exists() {
            break;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    assert!(input.exists(), "input FIFO should be created");

    // Wait for timeout.
    let status = wait_with_timeout(&mut child, Duration::from_secs(8));
    assert_eq!(status.unwrap().code(), Some(124), "should exit 124");
    assert!(!input.exists(), "input FIFO should be cleaned up after exit");
    cleanup(&out1);
}

/// Broadcast with -t 0 blocks forever (same as omitting -t).
#[test]
fn broadcast_timeout_zero_blocks_forever() {
    let input = fifo_path("timeout0_input.fifo");
    let out1 = fifo_path("timeout0_o1.fifo");
    cleanup(&input);
    cleanup(&out1);
    create_test_fifo(&out1);

    let mut child = spawn_broadcast(&[
        "-t", "0",
        input.to_str().unwrap(),
        out1.to_str().unwrap(),
    ]);

    // Wait for input FIFO.
    for _ in 0..50 {
        if input.exists() {
            break;
        }
        std::thread::sleep(Duration::from_millis(20));
    }

    // Set up output reader.
    let o1 = out1.clone();
    let r1 = std::thread::spawn(move || read_fifo(&o1));
    std::thread::sleep(Duration::from_millis(50));

    // Write after 2 seconds — broadcast should still be alive.
    std::thread::sleep(Duration::from_secs(2));
    write_fifo(&input, "still here");

    let d1 = r1.join().unwrap();
    assert_eq!(d1, b"still here\n", "should receive message after 2s");

    let _ = child.kill();
    let _ = child.wait();
    cleanup(&input);
    cleanup(&out1);
}

// ─── Max-time (-m) tests ───────────────────────────────────────────

/// Broadcast with -m exits cleanly (exit 0) after max-time, even with no writer.
#[test]
fn broadcast_max_time_no_writer_exits_cleanly() {
    let input = fifo_path("bcast_mt_nowriter_input.fifo");
    let out1 = fifo_path("bcast_mt_nowriter_o1.fifo");
    cleanup(&input);
    cleanup(&out1);
    create_test_fifo(&out1);

    let start = std::time::Instant::now();
    let mut child = spawn_broadcast(&[
        "-m", "2",
        input.to_str().unwrap(),
        out1.to_str().unwrap(),
    ]);

    let status = wait_with_timeout(&mut child, Duration::from_secs(10));
    let elapsed = start.elapsed();

    // -m exits 0, not 124 — it's a hard deadline, not a timeout error.
    assert_eq!(
        status.unwrap().code(),
        Some(0),
        "should exit 0 on max-time"
    );
    assert!(
        elapsed >= Duration::from_secs(1),
        "should wait ~2s, took {elapsed:?}"
    );
    assert!(
        elapsed < Duration::from_secs(8),
        "took too long: {elapsed:?}"
    );
    assert!(!input.exists(), "input FIFO should be cleaned up");
    cleanup(&out1);
}

/// Broadcast with -m exits cleanly after max-time, even if messages were flowing.
///
/// Each write_fifo_blocking opens, writes, and closes the output FIFO,
/// so we read each message individually by opening the output FIFO
/// after each write (open blocks until the writer-side thread connects,
/// then read_to_end returns when the writer closes).
#[test]
fn broadcast_max_time_receives_then_exits() {
    let input = fifo_path("bcast_mt_receive_input.fifo");
    let out1 = fifo_path("bcast_mt_receive_o1.fifo");
    cleanup(&input);
    cleanup(&out1);
    create_test_fifo(&out1);

    let mut child = spawn_broadcast(&[
        "-m", "4",
        input.to_str().unwrap(),
        out1.to_str().unwrap(),
    ]);

    // Wait for input FIFO.
    for _ in 0..50 {
        if input.exists() {
            break;
        }
        std::thread::sleep(Duration::from_millis(20));
    }

    // Write first message, then read it.
    write_fifo(&input, "first message");
    std::thread::sleep(Duration::from_millis(300));
    let d1 = read_fifo(&out1);
    assert_eq!(
        String::from_utf8_lossy(&d1),
        "first message\n",
        "first message mismatch"
    );

    // Write second message, then read it.
    write_fifo(&input, "second message");
    std::thread::sleep(Duration::from_millis(300));
    let d2 = read_fifo(&out1);
    assert_eq!(
        String::from_utf8_lossy(&d2),
        "second message\n",
        "second message mismatch"
    );

    // Wait for process to exit on max-time.
    let status = wait_with_timeout(&mut child, Duration::from_secs(10));
    assert_eq!(status.unwrap().code(), Some(0), "should exit 0 on max-time");

    assert!(!input.exists(), "input FIFO should be cleaned up");
    cleanup(&out1);
}

/// Broadcast with -m 0 behaves same as no -m (blocks forever).
#[test]
fn broadcast_max_time_zero_blocks_forever() {
    let input = fifo_path("bcast_mt_zero_input.fifo");
    let out1 = fifo_path("bcast_mt_zero_o1.fifo");
    cleanup(&input);
    cleanup(&out1);
    create_test_fifo(&out1);

    let mut child = spawn_broadcast(&[
        "-m", "0",
        input.to_str().unwrap(),
        out1.to_str().unwrap(),
    ]);

    // Wait for input FIFO.
    for _ in 0..50 {
        if input.exists() {
            break;
        }
        std::thread::sleep(Duration::from_millis(20));
    }

    // Set up output reader.
    let o1 = out1.clone();
    let r1 = std::thread::spawn(move || {
        let mut buf = Vec::new();
        std::fs::File::options()
            .read(true)
            .open(&o1)
            .unwrap()
            .read_to_end(&mut buf)
            .unwrap();
        buf
    });
    std::thread::sleep(Duration::from_millis(50));

    // Write after 2 seconds — broadcast should still be alive.
    std::thread::sleep(Duration::from_secs(2));
    write_fifo(&input, "still here after delay");

    let d1 = r1.join().unwrap();
    assert_eq!(d1, b"still here after delay\n", "should receive message after 2s");

    let _ = child.kill();
    let _ = child.wait();
    cleanup(&input);
    cleanup(&out1);
}
