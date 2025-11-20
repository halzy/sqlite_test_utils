# sqlite_test_utils

Test utilities for SQLite database testing in Rust.

## Overview

A collection of utilities for testing SQLite database operations, providing helper functions for:

- Creating and initializing test databases with random data
- Managing SQLite journal modes (WAL, DELETE, etc.)
- Performing CRUD operations on test data
- Running interactive sqlite3 processes for multi-process locking tests

## Quick Start

```rust
use sqlite_test_utils::{init_test_db, set_journal_mode, insert_test_db, update_test_db, read_row};
use rusqlite::Connection;

// Create a file-based database with WAL mode
let dir = tempfile::tempdir().unwrap();
let db_path = dir.path().join("test.db");
let conn = Connection::open(&db_path).unwrap();

// Initialize with test data (seed=42 for reproducibility)
init_test_db(&conn, "main", 42, 100, 10).unwrap();
set_journal_mode(&conn, "WAL", "main").unwrap();

// CRUD operations
let new_id = insert_test_db(&conn, "main", 15).unwrap();
assert!(new_id == 101); // 100 rows from init + 1 new

update_test_db(&conn, "main", 1, 20).unwrap();
let text = read_row(&conn, "main", 1).unwrap();
assert!(!text.is_empty());
```
