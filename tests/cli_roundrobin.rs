//! Integration tests for `synctell roundrobin` (alias `rr`).

use std::io::Read;
use std::process::{Child, Command, Stdio};
use std::time::Duration;

fn fifo_path(name: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join("synctell-cli-roundrobin-tests");
    let _ = std::fs::create_dir_all(&dir);
    dir.join(name)
}

fn cleanup(path: &std::path::Path) {
    let _ = std::fs::remove_file(path);
}

/// Spawn `synctell roundrobin` and return the child (don't wait).
fn spawn_roundrobin(args: &[&str]) -> Child {
    Command::new(env!("CARGO_BIN_EXE_synctell"))
        .arg("roundrobin")
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("failed to spawn synctell roundrobin")
}

/// Spawn `synctell rr` and return the child (don't wait).
fn spawn_rr(args: &[&str]) -> Child {
    Command::new(env!("CARGO_BIN_EXE_synctell"))
        .arg("rr")
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("failed to spawn synctell rr")
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

/// Create a POSIX FIFO using libc.
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

/// Round-robin distributes messages one at a time to outputs.
///
/// Uses `read_fifo` threads that each read the output FIFO until EOF
/// (the roundrobin closes the writer after each write, so each read_fifo
/// call gets one message).  We re-open between messages to simulate
/// what a real CLI consumer would do.
#[test]
fn roundrobin_distributes_round_robin() {
    let input = fifo_path("rr_dist_input.fifo");
    let out1 = fifo_path("rr_dist_o1.fifo");
    let out2 = fifo_path("rr_dist_o2.fifo");
    let out3 = fifo_path("rr_dist_o3.fifo");
    cleanup(&input);
    cleanup(&out1);
    cleanup(&out2);
    cleanup(&out3);

    // Create output FIFOs.
    create_test_fifo(&out1);
    create_test_fifo(&out2);
    create_test_fifo(&out3);

    // Start roundrobin.
    let mut child = spawn_roundrobin(&[
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

    // Read msg1 from out1, msg2 from out2, msg3 from out3 concurrently.
    let o1 = out1.clone();
    let r1 = std::thread::spawn(move || read_fifo(&o1));
    let o2 = out2.clone();
    let r2 = std::thread::spawn(move || read_fifo(&o2));
    let o3 = out3.clone();
    let r3 = std::thread::spawn(move || read_fifo(&o3));

    // Give reader threads time to block on open().
    std::thread::sleep(Duration::from_millis(100));

    write_fifo(&input, "msg1");
    std::thread::sleep(Duration::from_millis(200));
    write_fifo(&input, "msg2");
    std::thread::sleep(Duration::from_millis(200));
    write_fifo(&input, "msg3");
    std::thread::sleep(Duration::from_millis(200));

    let d1 = r1.join().unwrap();
    let d2 = r2.join().unwrap();
    let d3 = r3.join().unwrap();
    assert_eq!(d1, b"msg1\n", "output 1 should get msg1");
    assert_eq!(d2, b"msg2\n", "output 2 should get msg2");
    assert_eq!(d3, b"msg3\n", "output 3 should get msg3");

    // Now read msg4 from out1, msg5 from out2.
    let o1 = out1.clone();
    let r1 = std::thread::spawn(move || read_fifo(&o1));
    let o2 = out2.clone();
    let r2 = std::thread::spawn(move || read_fifo(&o2));
    std::thread::sleep(Duration::from_millis(100));

    write_fifo(&input, "msg4");
    std::thread::sleep(Duration::from_millis(200));
    write_fifo(&input, "msg5");
    std::thread::sleep(Duration::from_millis(200));

    let d1 = r1.join().unwrap();
    let d2 = r2.join().unwrap();
    assert_eq!(d1, b"msg4\n", "output 1 should get msg4");
    assert_eq!(d2, b"msg5\n", "output 2 should get msg5");

    // Kill and clean up.
    let _ = child.kill();
    let _ = child.wait();
    cleanup(&input);
    cleanup(&out1);
    cleanup(&out2);
    cleanup(&out3);
}

/// Round-robin alias `rr` works the same.
#[test]
fn rr_alias_works() {
    let input = fifo_path("rr_alias_input.fifo");
    let out1 = fifo_path("rr_alias_o1.fifo");
    let out2 = fifo_path("rr_alias_o2.fifo");
    cleanup(&input);
    cleanup(&out1);
    cleanup(&out2);

    create_test_fifo(&out1);
    create_test_fifo(&out2);

    let mut child = spawn_rr(&[
        input.to_str().unwrap(),
        out1.to_str().unwrap(),
        out2.to_str().unwrap(),
    ]);

    for _ in 0..50 {
        if input.exists() {
            break;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    assert!(input.exists(), "input FIFO should exist");

    let o1 = out1.clone();
    let r1 = std::thread::spawn(move || read_fifo(&o1));
    let o2 = out2.clone();
    let r2 = std::thread::spawn(move || read_fifo(&o2));
    std::thread::sleep(Duration::from_millis(100));

    write_fifo(&input, "first");
    std::thread::sleep(Duration::from_millis(200));
    write_fifo(&input, "second");
    std::thread::sleep(Duration::from_millis(200));

    let d1 = r1.join().unwrap();
    let d2 = r2.join().unwrap();
    assert_eq!(d1, b"first\n", "output 1 should get first");
    assert_eq!(d2, b"second\n", "output 2 should get second");

    let _ = child.kill();
    let _ = child.wait();
    cleanup(&input);
    cleanup(&out1);
    cleanup(&out2);
}

/// Round-robin with -t exits 124 when no writer connects.
#[test]
fn roundrobin_timeout_exits_124() {
    let input = fifo_path("rr_timeout_input.fifo");
    let out1 = fifo_path("rr_timeout_o1.fifo");
    cleanup(&input);
    cleanup(&out1);
    create_test_fifo(&out1);

    let mut child = spawn_roundrobin(&[
        "-t", "2",
        input.to_str().unwrap(),
        out1.to_str().unwrap(),
    ]);

    let status = wait_with_timeout(&mut child, Duration::from_secs(10));
    assert_eq!(
        status.unwrap().code(),
        Some(124),
        "should exit 124 on timeout"
    );
    assert!(!input.exists(), "input FIFO should be cleaned up");
    cleanup(&out1);
}

/// Round-robin with -m exits cleanly after max-time.
#[test]
fn roundrobin_max_time_exits_cleanly() {
    let input = fifo_path("rr_maxtime_input.fifo");
    let out1 = fifo_path("rr_maxtime_o1.fifo");
    cleanup(&input);
    cleanup(&out1);
    create_test_fifo(&out1);

    let mut child = spawn_roundrobin(&[
        "-m", "2",
        input.to_str().unwrap(),
        out1.to_str().unwrap(),
    ]);

    let status = wait_with_timeout(&mut child, Duration::from_secs(10));
    assert_eq!(
        status.unwrap().code(),
        Some(0),
        "should exit 0 on max-time"
    );
    assert!(!input.exists(), "input FIFO should be cleaned up");
    cleanup(&out1);
}

/// Round-robin continues when one output disappears.
///
/// Uses `-m 4` to ensure the roundrobin exits cleanly after a timeout.
/// Sends 3 messages: msg1→out1 (lost, out1 removed), msg2→out2 (received),
/// msg3→out1 (lost again).  Verifies out2 eventually gets msg2.
#[test]
fn roundrobin_survives_output_disconnect() {
    let input = fifo_path("rr_survive_input.fifo");
    let out1 = fifo_path("rr_survive_o1.fifo");
    let out2 = fifo_path("rr_survive_o2.fifo");
    cleanup(&input);
    cleanup(&out1);
    cleanup(&out2);
    create_test_fifo(&out1);
    create_test_fifo(&out2);

    let mut child = spawn_roundrobin(&[
        "-m", "4",
        input.to_str().unwrap(),
        out1.to_str().unwrap(),
        out2.to_str().unwrap(),
    ]);

    for _ in 0..50 {
        if input.exists() {
            break;
        }
        std::thread::sleep(Duration::from_millis(20));
    }

    // Remove out1 (simulates disconnect).
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

    // msg1→out1 (lost, out1 removed), msg2→out2 (received), msg3→out1 (lost again).
    write_fifo(&input, "msg1");
    std::thread::sleep(Duration::from_millis(200));
    write_fifo(&input, "msg2");
    std::thread::sleep(Duration::from_millis(200));
    write_fifo(&input, "msg3");
    std::thread::sleep(Duration::from_millis(200));

    // Wait for roundrobin to exit (max-time 4s).
    let status = wait_with_timeout(&mut child, Duration::from_secs(8));
    assert_eq!(status.unwrap().code(), Some(0), "should exit 0 on max-time");

    let d2 = r2.join().unwrap();
    assert_eq!(d2, b"msg2\n", "output 2 should receive msg2 (the one that lands on round 2)");

    cleanup(&input);
    cleanup(&out1);
    cleanup(&out2);
}

/// Round-robin cleans up input FIFO on exit.
#[test]
fn roundrobin_cleans_up_input() {
    let input = fifo_path("rr_cleanup_input.fifo");
    let out1 = fifo_path("rr_cleanup_o1.fifo");
    cleanup(&input);
    cleanup(&out1);
    create_test_fifo(&out1);

    let mut child = spawn_roundrobin(&[
        "-t", "1",
        input.to_str().unwrap(),
        out1.to_str().unwrap(),
    ]);

    for _ in 0..50 {
        if input.exists() {
            break;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    assert!(input.exists(), "input FIFO should be created");

    let status = wait_with_timeout(&mut child, Duration::from_secs(8));
    assert_eq!(status.unwrap().code(), Some(124), "should exit 124");
    assert!(!input.exists(), "input FIFO should be cleaned up");
    cleanup(&out1);
}