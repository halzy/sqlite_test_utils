#![allow(clippy::doc_markdown)] // SQLite is a proper noun, not code

//! # SQLite Test Utilities
//!
//! A collection of utilities for testing SQLite database operations.
//!
//! This crate provides helper functions and types for:
//! - Creating and initializing test databases with random data
//! - Managing SQLite journal modes (WAL, DELETE, etc.)
//! - Performing CRUD operations on test data
//! - Running interactive sqlite3 processes for multi-process locking tests
//!
//! ## Quick Start
//!
//! ```rust
//! use sqlite_test_utils::{init_test_db, set_journal_mode, insert_test_db, update_test_db, read_row};
//! use rusqlite::Connection;
//!
//! // Create a file-based database with WAL mode
//! let dir = tempfile::tempdir().unwrap();
//! let db_path = dir.path().join("test.db");
//! let conn = Connection::open(&db_path).unwrap();
//!
//! // Initialize with test data (seed=42 for reproducibility)
//! init_test_db(&conn, "main", 42, 100, 10).unwrap();
//! set_journal_mode(&conn, "WAL", "main").unwrap();
//!
//! // CRUD operations
//! let new_id = insert_test_db(&conn, "main", 15).unwrap();
//! assert!(new_id == 101); // 100 rows from init + 1 new
//!
//! update_test_db(&conn, "main", 1, 20).unwrap();
//! let text = read_row(&conn, "main", 1).unwrap();
//! assert!(!text.is_empty());
//! ```

use std::error::Error as StdError;

use rusqlite::{params, Connection};

mod sqlite3process;
pub use sqlite3process::Sqlite3Process;

/// Latin words used for generating random test data.
const WORDS: [&str; 58] = [
    "Cras",
    "Fusce",
    "Lorem",
    "Maecenas",
    "Nunc",
    "Orci",
    "Pellentesque",
    "Ut",
    "adipiscing",
    "amet",
    "at",
    "bibendum",
    "commodo",
    "condimentum",
    "consectetur",
    "dapibus",
    "dis",
    "dolor",
    "egestas",
    "elit",
    "eros",
    "et",
    "eu",
    "fringilla",
    "iaculis",
    "id",
    "in",
    "ipsum",
    "lacinia",
    "lorem",
    "magnis",
    "malesuada",
    "mi",
    "montes",
    "nascetur",
    "natoque",
    "nec",
    "nisi",
    "nulla",
    "parturient",
    "pellentesque",
    "penatibus",
    "placerat",
    "purus",
    "quam",
    "ridiculus",
    "risus",
    "sagittis",
    "scelerisque",
    "sed",
    "sem",
    "sit",
    "tincidunt",
    "tortor",
    "ultrices",
    "varius",
    "vel",
    "venenatis",
];

/// Initializes an existing database connection with test data.
///
/// Creates a `notes` table in the specified schema and populates it with random data.
///
/// # Arguments
///
/// * `sqlite_connection` - An open database connection
/// * `schema` - The schema name (e.g., "main" for the default schema)
/// * `seed` - Random seed for reproducible data generation
/// * `row_count` - Number of rows to insert into the `notes` table
/// * `note_word_count` - Maximum number of words per note
///
/// # Errors
///
/// Returns an error if table creation or data insertion fails.
pub fn init_test_db<S: AsRef<str>>(
    sqlite_connection: &Connection,
    schema: S,
    seed: u64,
    row_count: usize,
    note_word_count: usize,
) -> Result<(), rusqlite::Error> {
    fastrand::seed(seed);
    let schema = schema.as_ref();

    // Create the table
    sqlite_connection.execute(
        &format!("CREATE TABLE {schema}.notes (id INTEGER PRIMARY KEY, text TEXT NOT NULL)"),
        [],
    )?;

    // Use a prepared statement for inserts
    let mut stmt =
        sqlite_connection.prepare(&format!("INSERT INTO {schema}.notes (text) VALUES (?)"))?;

    // Insert all rows in a transaction
    sqlite_connection.execute("BEGIN", [])?;
    for _ in 0..row_count {
        let note = create_note(note_word_count);
        stmt.execute(params![note])?;
    }
    sqlite_connection.execute("COMMIT", [])?;

    // Verify rows were inserted
    let count: i64 = sqlite_connection.query_row(
        &format!("SELECT COUNT(*) FROM {schema}.notes"),
        [],
        |row| row.get(0),
    )?;
    eprintln!("Row count after init_test_db: {count}");

    Ok(())
}

/// Sets the journal mode to a specified value for the given schema.
///
/// Supported journal modes include: DELETE, TRUNCATE, PERSIST, MEMORY, WAL, OFF.
///
/// # Arguments
///
/// * `conn` - An open database connection
/// * `mode` - The journal mode to set (case-insensitive)
/// * `schema` - The schema name (e.g., "main" for the default schema)
///
/// # Errors
///
/// Returns an error if the journal mode cannot be changed to the requested mode.
///
pub fn set_journal_mode<S: AsRef<str>>(
    conn: &Connection,
    mode: &str,
    schema: S,
) -> Result<(), Box<dyn StdError>> {
    let schema = schema.as_ref();
    let mut mode = mode.to_string();
    mode.make_ascii_lowercase();

    let mut journal_mode: String =
        conn.pragma_update_and_check(Some(schema), "journal_mode", &mode, |row| row.get(0))?;

    journal_mode.make_ascii_lowercase();
    if journal_mode.as_str() == mode {
        Ok(())
    } else {
        Err(format!("Could not set journal mode for {schema} to {mode}").into())
    }
}

/// Updates a row in a test database using an existing connection.
///
/// # Arguments
///
/// * `sqlite_connection` - An open database connection
/// * `schema` - The schema name (e.g., "main" for the default schema)
/// * `row_id` - The ID of the row to update
/// * `word_count` - Maximum number of words for the new note content
///
/// # Errors
///
/// Returns an error if the update fails.
pub fn update_test_db<S: AsRef<str>>(
    sqlite_connection: &Connection,
    schema: S,
    row_id: i64,
    word_count: usize,
) -> Result<(), Box<dyn StdError>> {
    let note = create_note(word_count);
    let schema = schema.as_ref();
    let sql = format!("UPDATE {schema}.notes SET text = ? WHERE id = ?");

    sqlite_connection.execute(&sql, params![note, row_id])?;

    Ok(())
}

/// Inserts a new row with random data into a test database.
///
/// # Arguments
///
/// * `sqlite_connection` - An open database connection
/// * `schema` - The schema name (e.g., "main" for the default schema)
/// * `word_count` - Maximum number of words for the note content
///
/// # Returns
///
/// Returns the row ID of the newly inserted row.
///
/// # Errors
///
/// Returns an error if the insert fails.
pub fn insert_test_db<S: AsRef<str>>(
    sqlite_connection: &Connection,
    schema: S,
    word_count: usize,
) -> Result<i64, Box<dyn StdError>> {
    let schema = schema.as_ref();
    let note = create_note(word_count);
    let sql = format!("INSERT INTO {schema}.notes (text) values (?)");
    sqlite_connection.execute(&sql, params![note])?;
    let row_id = sqlite_connection.last_insert_rowid();

    Ok(row_id)
}

/// Creates a random note string with up to the specified number of words.
fn create_note(word_count: usize) -> String {
    let mut note = String::new();
    let words_len = WORDS.len();
    let words_for_note = fastrand::usize(..word_count);
    for _ in 0..words_for_note {
        note.push_str(WORDS[fastrand::usize(..words_len)]);
        note.push(' ');
    }
    note
}

/// Reads a row from the test database by ID.
///
/// # Arguments
///
/// * `conn` - An open database connection
/// * `schema` - The schema name (e.g., "main" for the default schema)
/// * `row_id` - The ID of the row to read
///
/// # Returns
///
/// Returns the text content of the specified row.
///
/// # Errors
///
/// Returns an error if the row is not found or the query fails.
pub fn read_row<S: AsRef<str>>(
    conn: &Connection,
    schema: S,
    row_id: i64,
) -> Result<String, Box<dyn std::error::Error>> {
    let schema = schema.as_ref();
    let data = conn.query_row_and_then(
        &format!("SELECT text FROM {schema}.notes WHERE id = ?1"),
        params![row_id],
        |row| row.get(0),
    )?;
    Ok(data)
}
