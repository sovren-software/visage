//! Async access to the daemon's single SQLite connection.
//!
//! This replaces `tokio-rusqlite`. Its newest release (0.8) pins `rusqlite`
//! ^0.40, and `libsqlite3-sys` is a `links` crate, so only one copy can be in
//! the graph. That pin blocked the bundled-SQLite fix for RUSTSEC-2026-0459,
//! which ships only with `rusqlite` 0.41.
//!
//! Semantics kept from `tokio-rusqlite`:
//!
//! - **Calls are serialised.** Each call holds the connection's mutex for its
//!   whole duration, as each call previously ran in turn on one background
//!   thread.
//! - **The async executor never blocks.** The closure runs on tokio's blocking
//!   pool, so a SQLite busy-wait cannot stall the D-Bus handlers.
//! - **A panic fails closed.** A panicking closure poisons the mutex, and every
//!   later call returns [`DbError::Poisoned`]. `tokio-rusqlite` behaved the
//!   same way: its worker thread died and every later call failed. Verify then
//!   errors, and PAM falls through to the password.

use std::path::Path;
use std::sync::{Arc, Mutex};

use thiserror::Error;

#[derive(Error, Debug)]
pub enum DbError {
    #[error(transparent)]
    Sqlite(#[from] rusqlite::Error),
    #[error("database task failed: {0}")]
    Task(#[from] tokio::task::JoinError),
    #[error("database connection unusable after an earlier panic")]
    Poisoned,
}

/// A cloneable handle to one SQLite connection. Clones share the connection.
#[derive(Clone)]
pub struct Connection {
    inner: Arc<Mutex<rusqlite::Connection>>,
}

impl Connection {
    /// Open (or create) the database at `path`. `:memory:` opens a private
    /// in-memory database, shared by every clone of the returned handle.
    pub async fn open(path: &Path) -> Result<Self, DbError> {
        let path = path.to_owned();
        let conn = tokio::task::spawn_blocking(move || rusqlite::Connection::open(path)).await??;
        Ok(Self {
            inner: Arc::new(Mutex::new(conn)),
        })
    }

    /// Run `f` against the connection on the blocking pool and return its result.
    pub async fn call<F, R>(&self, f: F) -> Result<R, DbError>
    where
        F: FnOnce(&mut rusqlite::Connection) -> Result<R, rusqlite::Error> + Send + 'static,
        R: Send + 'static,
    {
        let inner = Arc::clone(&self.inner);
        tokio::task::spawn_blocking(move || {
            let mut conn = inner.lock().map_err(|_| DbError::Poisoned)?;
            f(&mut conn).map_err(DbError::from)
        })
        .await?
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn memory() -> Connection {
        Connection::open(Path::new(":memory:")).await.unwrap()
    }

    #[tokio::test]
    async fn a_call_returns_the_closure_value_and_clones_share_the_database() {
        let a = memory().await;
        let b = a.clone();
        a.call(|c| c.execute_batch("CREATE TABLE t (v INTEGER NOT NULL);"))
            .await
            .unwrap();
        b.call(|c| c.execute("INSERT INTO t (v) VALUES (41)", []))
            .await
            .unwrap();
        let v: i64 = a
            .call(|c| c.query_row("SELECT v + 1 FROM t", [], |r| r.get(0)))
            .await
            .unwrap();
        assert_eq!(v, 42);
    }

    #[tokio::test]
    async fn a_sqlite_error_comes_back_as_a_sqlite_error() {
        let db = memory().await;
        let err = db
            .call(|c| c.execute("INSERT INTO missing (v) VALUES (1)", []))
            .await
            .unwrap_err();
        assert!(matches!(err, DbError::Sqlite(_)), "got {err:?}");
    }

    /// Read-modify-write in two statements loses updates unless calls are
    /// serialised. 64 concurrent tasks must leave exactly 64.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn concurrent_calls_are_serialised() {
        let db = memory().await;
        db.call(|c| {
            c.execute_batch("CREATE TABLE n (v INTEGER NOT NULL); INSERT INTO n VALUES (0);")
        })
        .await
        .unwrap();

        let tasks: Vec<_> = (0..64)
            .map(|_| {
                let db = db.clone();
                tokio::spawn(async move {
                    db.call(|c| {
                        let v: i64 = c.query_row("SELECT v FROM n", [], |r| r.get(0))?;
                        std::thread::yield_now();
                        c.execute("UPDATE n SET v = ?1", [v + 1])?;
                        Ok(())
                    })
                    .await
                })
            })
            .collect();
        for t in tasks {
            t.await.unwrap().unwrap();
        }

        let v: i64 = db
            .call(|c| c.query_row("SELECT v FROM n", [], |r| r.get(0)))
            .await
            .unwrap();
        assert_eq!(v, 64);
    }

    /// The fail-closed contract: after a panic, no later call may reach the
    /// connection, and none may hang.
    #[tokio::test]
    async fn a_panic_poisons_the_connection_and_later_calls_fail() {
        let db = memory().await;
        let first = db
            .call(|_| -> Result<(), rusqlite::Error> { panic!("deliberate test panic") })
            .await
            .unwrap_err();
        assert!(matches!(first, DbError::Task(_)), "got {first:?}");

        let second = db.call(|c| c.execute_batch("SELECT 1;")).await.unwrap_err();
        assert!(matches!(second, DbError::Poisoned), "got {second:?}");
    }
}
