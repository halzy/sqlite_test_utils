//! Interactive sqlite3 process wrapper for testing.

#![allow(clippy::doc_markdown)] // SQLite is a proper noun, not code

use std::io::{BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStderr, ChildStdin, ChildStdout, Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

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
    stderr: Option<ChildStderr>,
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
                        if !status.success() && !stderr_output.is_empty() {
                            eprintln!("sqlite3 process exited with error: {status}");
                            eprintln!("stderr: {stderr_output}");
                        }
                        return;
                    }
                    Ok(None) => {
                        if start.elapsed() > timeout {
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
