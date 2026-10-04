//! Opening connections.
//!
//! **sqlite-vec is registered once, before any connection opens.** It is an *auto
//! extension*: SQLite calls its init function for every connection made afterwards, so
//! `vec0` tables and `vec_*` functions exist everywhere without anything being loaded
//! from disk. The cast below is the one place that touches the FFI; it has broken across
//! rusqlite versions before, which is why [`crate::selfcheck`] exists.

use std::path::Path;
use std::sync::Once;

use rusqlite::{Connection, OpenFlags, ffi};

use crate::error::Result;

static REGISTER: Once = Once::new();

/// Make sqlite-vec part of every connection opened from now on. Idempotent.
pub fn register_extensions() {
    REGISTER.call_once(|| {
        type Init = unsafe extern "C" fn(
            *mut ffi::sqlite3,
            *mut *mut std::ffi::c_char,
            *const ffi::sqlite3_api_routines,
        ) -> std::ffi::c_int;
        // SAFETY: `sqlite3_vec_init` has the extension entry-point signature SQLite
        // expects; the crate declares it without arguments, hence the cast. It is
        // linked statically and lives for the whole process.
        unsafe {
            let init =
                std::mem::transmute::<*const (), Init>(sqlite_vec::sqlite3_vec_init as *const ());
            ffi::sqlite3_auto_extension(Some(init));
        }
    });
}

/// The settings every connection gets.
fn configure(conn: &Connection, read_only: bool) -> Result<()> {
    conn.pragma_update(None, "journal_mode", "WAL")?;
    conn.pragma_update(None, "foreign_keys", "ON")?;
    conn.pragma_update(None, "synchronous", "NORMAL")?;
    conn.pragma_update(None, "busy_timeout", 5000)?;
    if read_only {
        conn.pragma_update(None, "query_only", "ON")?;
    }
    Ok(())
}

/// The read-write connection. One per process (ADR-0046).
pub fn open_rw(path: &Path) -> Result<Connection> {
    register_extensions();
    let conn = Connection::open(path)?;
    configure(&conn, false)?;
    Ok(conn)
}

/// A connection for queries.
pub fn open_ro(path: &Path) -> Result<Connection> {
    register_extensions();
    let conn = Connection::open_with_flags(
        path,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )?;
    configure(&conn, true)?;
    Ok(conn)
}

/// A private in-memory database, with the same extensions.
pub fn open_memory() -> Result<Connection> {
    register_extensions();
    Ok(Connection::open_in_memory()?)
}
