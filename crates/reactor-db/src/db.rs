//! One write path, and connections for reading.
//!
//! SQLite allows one writer at a time, so a [`Db`] owns exactly one read-write
//! connection on a thread of its own and everything that changes the database is a
//! closure sent to it. That gives one place where a write becomes a transaction, and
//! callers on any thread — the agent's runtime, the UI — never hold a connection across
//! an await or a paint. Reads take a read-only connection from a small pool.
//!
//! Other processes on the same file (a second window on the case) are SQLite's to
//! arbitrate: WAL, `BEGIN IMMEDIATE` and a busy timeout (ADR-0046).

use std::path::{Path, PathBuf};
use std::sync::mpsc::{Sender, channel, sync_channel};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;

use rusqlite::{Connection, Transaction, TransactionBehavior};

use crate::error::{Error, Result};
use crate::{migrate, sqlite};

type Job = Box<dyn FnOnce(&mut Connection) + Send>;

/// How many idle read connections to keep.
const POOL: usize = 4;

struct Inner {
    path: PathBuf,
    jobs: Mutex<Option<Sender<Job>>>,
    writer: Mutex<Option<JoinHandle<()>>>,
    readers: Mutex<Vec<Connection>>,
}

/// A handle to one open database. Cheap to clone; the database closes when the last
/// clone is dropped.
#[derive(Clone)]
pub struct Db {
    inner: Arc<Inner>,
}

impl Db {
    /// Open (creating if absent) and migrate.
    pub fn open(path: &Path) -> Result<Db> {
        let mut conn = sqlite::open_rw(path)?;
        migrate::migrate(&mut conn)?;
        let (tx, rx) = channel::<Job>();
        let writer = std::thread::Builder::new()
            .name("reactor-db-writer".into())
            .spawn(move || {
                while let Ok(job) = rx.recv() {
                    job(&mut conn);
                }
                // Dropping the connection checkpoints the WAL.
            })
            .map_err(|e| Error::Io(format!("could not start the database writer: {e}")))?;
        Ok(Db {
            inner: Arc::new(Inner {
                path: path.to_path_buf(),
                jobs: Mutex::new(Some(tx)),
                writer: Mutex::new(Some(writer)),
                readers: Mutex::new(Vec::new()),
            }),
        })
    }

    pub fn path(&self) -> &Path {
        &self.inner.path
    }

    /// Run `f` in one transaction on the writer. It commits if `f` returns `Ok`.
    pub fn write<T, F>(&self, f: F) -> Result<T>
    where
        T: Send + 'static,
        F: FnOnce(&Transaction) -> Result<T> + Send + 'static,
    {
        self.on_writer(move |conn| {
            let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
            let value = f(&tx)?;
            tx.commit()?;
            Ok(value)
        })
    }

    /// Run `f` on the writer's connection, outside any transaction.
    fn on_writer<T, F>(&self, f: F) -> Result<T>
    where
        T: Send + 'static,
        F: FnOnce(&mut Connection) -> Result<T> + Send + 'static,
    {
        let (reply, answer) = sync_channel::<Result<T>>(1);
        let job: Job = Box::new(move |conn| {
            let _ = reply.send(f(conn));
        });
        self.inner
            .jobs
            .lock()
            .unwrap()
            .as_ref()
            .ok_or_else(|| Error::Sqlite("the database is closed".into()))?
            .send(job)
            .map_err(|_| Error::Sqlite("the database writer has stopped".into()))?;
        answer
            .recv()
            .map_err(|_| Error::Sqlite("the database writer has stopped".into()))?
    }

    /// Run `f` on a read-only connection.
    pub fn read<T>(&self, f: impl FnOnce(&Connection) -> Result<T>) -> Result<T> {
        let pooled = self.inner.readers.lock().unwrap().pop();
        let conn = match pooled {
            Some(c) => c,
            None => sqlite::open_ro(&self.inner.path)?,
        };
        let result = f(&conn);
        let mut pool = self.inner.readers.lock().unwrap();
        if pool.len() < POOL {
            pool.push(conn);
        }
        result
    }

    /// A consistent copy of the whole database in one file.
    pub fn backup_to(&self, dest: &Path) -> Result<()> {
        if dest.exists() {
            return Err(Error::Io(format!("{}: already exists", dest.display())));
        }
        let dest = dest.to_string_lossy().into_owned();
        // `VACUUM` cannot run inside a transaction, nor on a `query_only` connection.
        self.on_writer(move |conn| {
            conn.execute("VACUUM INTO ?1", [&dest])?;
            Ok(())
        })
    }
}

impl Drop for Inner {
    fn drop(&mut self) {
        self.readers.lock().unwrap().clear();
        // Hang up, then wait: the file is closed by the time the last handle is gone,
        // so a caller may move or delete the directory straight away.
        self.jobs.lock().unwrap().take();
        if let Some(writer) = self.writer.lock().unwrap().take() {
            let _ = writer.join();
        }
    }
}
