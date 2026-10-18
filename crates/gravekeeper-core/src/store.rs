//! Event history in SQLite, written on a dedicated thread so the engine never waits on disk.

use std::collections::HashMap;
use std::path::Path;
use std::sync::mpsc;
use std::thread::JoinHandle;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use rusqlite::{Connection, params};

use crate::api::{Event, EventKind, EventLevel};
use crate::config::AppId;
use crate::schedule::ScheduleId;

/// Older events beyond this many are deleted when the store is opened.
const RETAINED_EVENTS: i64 = 100_000;

/// Migrations, applied in order. `PRAGMA user_version` records how many have run.
const MIGRATIONS: &[&str] = &[
    "CREATE TABLE events (
        id       INTEGER PRIMARY KEY,
        at_ms    INTEGER NOT NULL,
        level    TEXT NOT NULL,
        kind     TEXT NOT NULL,
        app_id   TEXT,
        app_name TEXT,
        message  TEXT NOT NULL
    );
    CREATE INDEX events_at ON events (at_ms);",
    // When each schedule last ran, to detect runs missed while Gravekeeper was not running.
    "CREATE TABLE schedule_runs (
        schedule_id TEXT PRIMARY KEY,
        last_run_ms INTEGER NOT NULL
    );",
];

enum Write {
    Event(Event),
    ScheduleRun(ScheduleId, SystemTime),
}

/// What the store remembers from previous runs of Gravekeeper.
pub struct Recalled {
    /// Up to the requested number of latest events, oldest first.
    pub events: Vec<Event>,
    pub schedule_runs: HashMap<ScheduleId, SystemTime>,
}

pub struct Store {
    tx: Option<mpsc::Sender<Write>>,
    thread: Option<JoinHandle<()>>,
}

impl Store {
    /// Opens (creating or migrating) the database and recalls what it holds.
    pub fn open(path: &Path, recent: usize) -> rusqlite::Result<(Self, Recalled)> {
        let mut conn = Connection::open(path)?;
        conn.pragma_update(None, "journal_mode", "WAL")?;
        migrate(&mut conn)?;
        conn.execute(
            "DELETE FROM events WHERE id <= (SELECT MAX(id) FROM events) - ?1",
            [RETAINED_EVENTS],
        )?;
        let recalled = Recalled {
            events: latest(&conn, recent)?,
            schedule_runs: schedule_runs(&conn)?,
        };

        let (tx, rx) = mpsc::channel();
        let thread = std::thread::Builder::new()
            .name("gravekeeper-store".into())
            .spawn(move || writer(conn, rx))
            .map_err(|e| rusqlite::Error::ToSqlConversionFailure(Box::new(e)))?;
        Ok((
            Self {
                tx: Some(tx),
                thread: Some(thread),
            },
            recalled,
        ))
    }

    pub fn record(&self, event: Event) {
        self.send(Write::Event(event));
    }

    pub fn record_schedule_run(&self, id: ScheduleId, at: SystemTime) {
        self.send(Write::ScheduleRun(id, at));
    }

    fn send(&self, write: Write) {
        if let Some(tx) = &self.tx
            && tx.send(write).is_err()
        {
            tracing::warn!("history writer has stopped; not saved");
        }
    }
}

impl Drop for Store {
    /// Flushes pending events before returning.
    fn drop(&mut self) {
        self.tx.take();
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

fn migrate(conn: &mut Connection) -> rusqlite::Result<()> {
    let applied: i64 = conn.pragma_query_value(None, "user_version", |r| r.get(0))?;
    let applied = usize::try_from(applied).unwrap_or(usize::MAX);
    if applied > MIGRATIONS.len() {
        return Err(rusqlite::Error::InvalidParameterName(format!(
            "history database version {applied} is newer than this build supports"
        )));
    }
    for (i, sql) in MIGRATIONS.iter().enumerate().skip(applied) {
        let tx = conn.transaction()?;
        tx.execute_batch(sql)?;
        tx.pragma_update(None, "user_version", (i + 1) as i64)?;
        tx.commit()?;
    }
    Ok(())
}

fn writer(mut conn: Connection, rx: mpsc::Receiver<Write>) {
    while let Ok(first) = rx.recv() {
        // Write whatever has queued up in one transaction.
        let batch: Vec<_> = std::iter::once(first).chain(rx.try_iter()).collect();
        if let Err(err) = insert(&mut conn, &batch) {
            tracing::error!(%err, count = batch.len(), "failed to save events");
        }
    }
}

fn insert(conn: &mut Connection, writes: &[Write]) -> rusqlite::Result<()> {
    let tx = conn.transaction()?;
    {
        let mut event = tx.prepare_cached(
            "INSERT INTO events (at_ms, level, kind, app_id, app_name, message)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        )?;
        let mut run = tx.prepare_cached(
            "INSERT INTO schedule_runs (schedule_id, last_run_ms) VALUES (?1, ?2)
             ON CONFLICT (schedule_id) DO UPDATE SET last_run_ms = excluded.last_run_ms",
        )?;
        for write in writes {
            match write {
                Write::Event(e) => event.execute(params![
                    to_millis(e.at),
                    e.level.as_str(),
                    e.kind.as_str(),
                    e.app_id.as_ref().map(|id| id.0.as_str()),
                    e.app_name,
                    e.message,
                ])?,
                Write::ScheduleRun(id, at) => run.execute(params![id.0, to_millis(*at)])?,
            };
        }
    }
    tx.commit()
}

fn schedule_runs(conn: &Connection) -> rusqlite::Result<HashMap<ScheduleId, SystemTime>> {
    let mut stmt = conn.prepare("SELECT schedule_id, last_run_ms FROM schedule_runs")?;
    let rows = stmt.query_map([], |row| {
        Ok((ScheduleId(row.get(0)?), from_millis(row.get(1)?)))
    })?;
    rows.collect()
}

fn latest(conn: &Connection, limit: usize) -> rusqlite::Result<Vec<Event>> {
    let mut stmt = conn.prepare(
        "SELECT at_ms, level, kind, app_id, app_name, message
         FROM events ORDER BY id DESC LIMIT ?1",
    )?;
    let rows = stmt.query_map([limit as i64], |row| {
        let level: String = row.get(1)?;
        let kind: String = row.get(2)?;
        Ok(Event {
            at: from_millis(row.get(0)?),
            // Unknown names can only come from a newer build; show them as plain info.
            level: EventLevel::parse(&level).unwrap_or(EventLevel::Info),
            kind: EventKind::parse(&kind).unwrap_or(EventKind::EngineStarted),
            app_id: row.get::<_, Option<String>>(3)?.map(AppId),
            app_name: row.get(4)?,
            message: row.get(5)?,
        })
    })?;
    let mut events = rows.collect::<rusqlite::Result<Vec<_>>>()?;
    events.reverse();
    Ok(events)
}

fn to_millis(t: SystemTime) -> i64 {
    t.duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as i64)
}

fn from_millis(ms: i64) -> SystemTime {
    UNIX_EPOCH + Duration::from_millis(ms.max(0) as u64)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn event(i: u64) -> Event {
        Event {
            at: UNIX_EPOCH + Duration::from_secs(1_700_000_000 + i),
            level: EventLevel::Warning,
            kind: EventKind::AppExited,
            app_id: Some(AppId("a".into())),
            app_name: Some("worker".into()),
            message: format!("event {i}"),
        }
    }

    #[test]
    fn persists_events_across_reopen() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("history.sqlite3");

        let (store, recalled) = Store::open(&path, 10).unwrap();
        assert!(recalled.events.is_empty());
        for i in 0..5 {
            store.record(event(i));
        }
        let run = UNIX_EPOCH + Duration::from_secs(1_700_000_000);
        store.record_schedule_run(ScheduleId("s".into()), UNIX_EPOCH);
        store.record_schedule_run(ScheduleId("s".into()), run);
        drop(store);

        let (_store, recalled) = Store::open(&path, 3).unwrap();
        let messages: Vec<_> = recalled.events.iter().map(|e| e.message.as_str()).collect();
        assert_eq!(messages, ["event 2", "event 3", "event 4"]);
        assert_eq!(recalled.events[2], event(4));
        assert_eq!(recalled.schedule_runs[&ScheduleId("s".into())], run);
    }

    #[test]
    fn rejects_a_corrupt_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("history.sqlite3");
        std::fs::write(
            &path,
            b"this is not a database, just some bytes that fill a page",
        )
        .unwrap();
        assert!(Store::open(&path, 10).is_err());
    }
}
