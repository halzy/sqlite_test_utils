//! Interactive sqlite3 process wrapper for testing.

#![allow(clippy::doc_markdown)] // SQLite is a proper noun, not code

use std::io::{BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStderr, ChildStdin, ChildStdout, Command, Stdio};
use std::thread;
use std::time::Duration;

#[cfg(test)]
use mock_instant::global::Instant;
#[cfg(not(test))]
use std::time::Instant;

/// Wrapper for controlling an interactive sqlite3 process.
///
/// This struct spawns and interacts with an external `sqlite3` command-line process,
/// enabling tests that exercise SQLite's multi-process locking behavior. This is
/// essential for testing code paths that depend on how SQLite handles concurrent
/// access from separate processes (as opposed to multiple connections within the
/// same process, which have different locking semantics).
///
/// # Example
///
/// ```rust
/// use sqlite_test_utils::Sqlite3Process;
///
/// let dir = tempfile::tempdir().unwrap();
/// let db_path = dir.path().join("test.db");
/// let mut process = Sqlite3Process::new(&db_path).unwrap();
///
/// // Execute SQL commands
/// let result = process.execute("SELECT 1 + 1;").unwrap();
/// assert!(result.contains("2"));
///
/// // Process is automatically cleaned up on drop
/// ```
pub struct Sqlite3Process {
    child: Option<Child>,
    stdin: Option<ChildStdin>,
    stdout: Option<BufReader<ChildStdout>>,
    pub(crate) stderr: Option<ChildStderr>,
    db_path: PathBuf,
}

impl Sqlite3Process {
    /// Creates a new `Sqlite3Process` connected to the specified database.
    ///
    /// # Arguments
    ///
    /// * `db_path` - Path to the SQLite database file
    ///
    /// # Errors
    ///
    /// Returns an error if the sqlite3 process cannot be spawned or if
    /// the I/O handles cannot be obtained.
    ///
    pub fn new(db_path: &Path) -> Result<Self, String> {
        let mut child = Command::new("sqlite3")
            .arg(db_path)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|e| format!("Failed to spawn sqlite3: {e}"))?;

        let stdin = child.stdin.take().ok_or("Failed to get stdin handle")?;
        let stdout = child.stdout.take().ok_or("Failed to get stdout handle")?;
        let stderr = child.stderr.take();

        Ok(Sqlite3Process {
            child: Some(child),
            stdin: Some(stdin),
            stdout: Some(BufReader::new(stdout)),
            stderr,
            db_path: db_path.to_path_buf(),
        })
    }

    /// Enables WAL (Write-Ahead Logging) journal mode.
    ///
    /// # Panics
    ///
    /// Panics if the PRAGMA command fails.
    pub fn enable_wal_mode(&mut self) {
        self.execute("PRAGMA journal_mode=WAL;").unwrap();
    }

    /// Disables automatic WAL checkpointing.
    ///
    /// This is useful for testing scenarios where you want to control
    /// when checkpoints occur.
    ///
    /// # Panics
    ///
    /// Panics if the PRAGMA command fails.
    pub fn disable_wal_checkpointing(&mut self) {
        self.execute("PRAGMA wal_autocheckpoint=0;").unwrap();
    }

    /// Executes a SQL statement and returns the output.
    ///
    /// # Arguments
    ///
    /// * `sql` - The SQL statement to execute
    ///
    /// # Returns
    ///
    /// Returns the output from sqlite3 as a string.
    ///
    /// # Errors
    ///
    /// Returns an error if writing to stdin fails, reading from stdout fails,
    /// or the output marker is not found.
    ///
    pub fn execute(&mut self, sql: &str) -> Result<String, String> {
        let stdin = self.stdin.as_mut().ok_or("stdin is not available")?;
        let stdout = self.stdout.as_mut().ok_or("stdout is not available")?;

        writeln!(stdin, "{sql}").map_err(|e| format!("Failed to write: {e}"))?;
        stdin.flush().map_err(|e| format!("Failed to flush: {e}"))?;

        writeln!(stdin, "SELECT 'MARKER_END';")
            .map_err(|e| format!("Failed to write marker: {e}"))?;
        stdin.flush().map_err(|e| format!("Failed to flush: {e}"))?;

        let mut output = String::new();
        let mut found_marker = false;

        loop {
            let mut line = String::new();
            match stdout.read_line(&mut line) {
                Ok(0) => break,
                Ok(_) => {
                    if line.contains("MARKER_END") {
                        found_marker = true;
                        break;
                    }
                    if !line.contains("SELECT 'MARKER_END'") {
                        output.push_str(&line);
                    }
                }
                Err(e) => return Err(format!("Failed to read: {e}")),
            }
        }

        if !found_marker {
            return Err("Failed to read complete output".to_string());
        }

        Ok(output)
    }

    /// Creates a test table with dummy data.
    ///
    /// Creates a `test` table with `id` and `value` columns, then inserts
    /// 999 rows of test data.
    ///
    /// # Panics
    ///
    /// Panics if any SQL command fails.
    pub fn create_dummy_data(&mut self) {
        self.execute("CREATE TABLE test (id INTEGER PRIMARY KEY, value TEXT);")
            .unwrap();

        for number in 1..1000 {
            self.execute(&format!(
                "INSERT INTO test (value) VALUES ('Hello, World! {number}');"
            ))
            .unwrap();
        }
    }
}

/// Checks if elapsed time has exceeded the timeout.
fn is_timed_out(elapsed: Duration, timeout: Duration) -> bool {
    elapsed > timeout
}

/// Determines if an error message should be logged based on exit status and stderr.
fn should_log_error(success: bool, stderr_empty: bool) -> bool {
    !success && !stderr_empty
}

impl Drop for Sqlite3Process {
    fn drop(&mut self) {
        if let Some(ref mut stdin) = self.stdin {
            let _ = writeln!(stdin, ".exit");
            let _ = stdin.flush();
        }

        let mut stderr_output = String::new();
        if let Some(mut stderr) = self.stderr.take() {
            let mut buffer = Vec::new();
            let _ = stderr.read_to_end(&mut buffer);
            stderr_output = String::from_utf8_lossy(&buffer).to_string();
        }

        let start = Instant::now();
        let timeout = Duration::from_secs(60);
        if let Some(ref mut child) = self.child {
            loop {
                match child.try_wait() {
                    Ok(Some(status)) => {
                        if should_log_error(status.success(), stderr_output.is_empty()) {
                            eprintln!("sqlite3 process exited with error: {status}");
                            eprintln!("stderr: {stderr_output}");
                        }
                        return;
                    }
                    Ok(None) => {
                        if is_timed_out(start.elapsed(), timeout) {
                            eprintln!("sqlite3 process failed to exit within 60 seconds!");
                            eprintln!("Database path: {}", self.db_path.display());
                            let _ = child.kill();
                            panic!("sqlite3 process hung for {}", self.db_path.display());
                        }
                        thread::sleep(Duration::from_millis(100));
                    }
                    Err(e) => {
                        eprintln!("Error waiting for sqlite3: {e}");
                        let _ = child.kill();
                        return;
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mock_instant::global::MockClock;
    use std::sync::mpsc;
    use tempfile::tempdir;

    fn new_test_process() -> (Sqlite3Process, tempfile::TempDir) {
        let dir = tempdir().unwrap();
        let db_path = dir.path().join("test.db");
        let process = Sqlite3Process::new(&db_path).unwrap();
        (process, dir)
    }

    #[test]
    fn test_is_timed_out() {
        let timeout = Duration::from_secs(60);

        // Not timed out: before and exactly at timeout
        assert!(!is_timed_out(Duration::from_secs(0), timeout));
        assert!(!is_timed_out(Duration::from_secs(59), timeout));
        assert!(!is_timed_out(Duration::from_secs(60), timeout));

        // Timed out: just after timeout
        assert!(is_timed_out(Duration::from_millis(60_001), timeout));
        assert!(is_timed_out(Duration::from_secs(61), timeout));
    }

    #[test]
    fn test_should_log_error() {
        // All four combinations of (success, stderr_empty)
        assert!(!should_log_error(true, true)); // success, no stderr
        assert!(!should_log_error(true, false)); // success, has stderr
        assert!(!should_log_error(false, true)); // failed, no stderr
        assert!(should_log_error(false, false)); // failed with stderr - only case that logs
    }

    #[test]
    fn test_enable_wal_mode() {
        let (mut process, _dir) = new_test_process();
        process.enable_wal_mode();

        let output = process.execute("PRAGMA journal_mode;").unwrap();
        assert!(
            output.to_lowercase().contains("wal"),
            "WAL mode should be enabled, got: {output}"
        );
    }

    #[test]
    fn test_disable_wal_checkpointing() {
        let (mut process, _dir) = new_test_process();
        process.enable_wal_mode();

        let before: i32 = process
            .execute("PRAGMA wal_autocheckpoint;")
            .unwrap()
            .trim()
            .parse()
            .unwrap();
        assert!(
            before > 0,
            "Default autocheckpoint should be > 0, got: {before}"
        );

        process.disable_wal_checkpointing();

        let after: i32 = process
            .execute("PRAGMA wal_autocheckpoint;")
            .unwrap()
            .trim()
            .parse()
            .unwrap();
        assert_eq!(after, 0, "Autocheckpoint should be 0 after disable");
    }

    #[test]
    fn test_create_dummy_data() {
        let (mut process, _dir) = new_test_process();
        process.create_dummy_data();

        let count: i32 = process
            .execute("SELECT COUNT(*) FROM test;")
            .unwrap()
            .trim()
            .parse()
            .unwrap();
        assert_eq!(count, 999, "Should have 999 rows");

        let output = process
            .execute("SELECT value FROM test WHERE id = 1;")
            .unwrap();
        assert!(
            output.contains("Hello, World! 1"),
            "First row should have expected content, got: {output}"
        );
    }

    #[test]
    #[should_panic(expected = "sqlite3 process hung")]
    fn test_drop_timeout_panics() {
        let (tx, rx) = mpsc::channel();

        let handle = std::thread::spawn(move || {
            let (mut process, _dir) = new_test_process();
            process.execute("SELECT 1;").unwrap();

            // Take stdin and stderr to prevent drop from exiting normally
            let _stdin = process.stdin.take();
            let _stderr = process.stderr.take();

            // Reset mock clock and advance past timeout in another thread
            MockClock::set_time(Duration::from_secs(0));
            let advance_handle = std::thread::spawn(|| {
                std::thread::sleep(Duration::from_millis(50));
                MockClock::advance(Duration::from_millis(60_001));
            });

            drop(process);
            let _ = advance_handle.join();
            tx.send(()).unwrap();
        });

        match rx.recv_timeout(Duration::from_secs(5)) {
            Ok(_) => panic!("Test completed without panic"),
            Err(mpsc::RecvTimeoutError::Timeout) => {
                panic!("Test timed out after 5 seconds");
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                if let Err(e) = handle.join() {
                    std::panic::resume_unwind(e);
                }
            }
        }
    }
}
