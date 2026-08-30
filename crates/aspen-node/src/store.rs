//! The bus store: SQLite, WAL, one per node.
//!
//! The store memorializes passively — sent, delivered (when, via what),
//! ingested (the runtime's replay ack) — and answers questions. It never
//! asks agents to file receipts; there is deliberately no ack primitive
//! (plumb's scar: a skipped protocol with a monitor attached manufactures
//! false signal).

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result};
use rusqlite::{params, Connection};

const SCHEMA_VERSION: i64 = 1;

const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS messages(
  id            INTEGER PRIMARY KEY,
  uuid          TEXT NOT NULL,
  thread        TEXT,
  sender        TEXT NOT NULL,
  recipient     TEXT NOT NULL,
  to_display    TEXT NOT NULL,
  urgency       TEXT NOT NULL CHECK(urgency IN ('gating','normal','notice')),
  body          TEXT NOT NULL,
  record_ref    TEXT,
  created_at    REAL NOT NULL,
  delivered_at  REAL,
  delivered_via TEXT,
  ingest_uuid   TEXT,
  ingested_at   REAL
);
CREATE INDEX IF NOT EXISTS idx_pending ON messages(recipient, delivered_at);
CREATE INDEX IF NOT EXISTS idx_ingest ON messages(ingest_uuid);
CREATE TABLE IF NOT EXISTS agents(
  name            TEXT PRIMARY KEY,
  repo            TEXT NOT NULL,
  channel         TEXT NOT NULL,
  session_id      TEXT,
  charter         TEXT,
  created_at      REAL NOT NULL,
  last_spawned_at REAL
);
";

pub fn now_epoch() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs_f64())
        .unwrap_or(0.0)
}

#[derive(Debug, Clone)]
pub struct StoredMessage {
    pub id: i64,
    pub uuid: String,
    pub thread: Option<String>,
    pub sender: String,
    pub recipient: String,
    pub to_display: String,
    pub urgency: String,
    pub body: String,
    pub record_ref: Option<String>,
    pub created_at: f64,
    pub delivered_at: Option<f64>,
    pub delivered_via: Option<String>,
    pub ingested_at: Option<f64>,
}

#[derive(Debug, Clone)]
pub struct AgentRow {
    pub name: String,
    pub repo: PathBuf,
    pub channel: String,
    pub session_id: Option<String>,
    pub charter: Option<String>,
}

#[derive(Clone)]
pub struct BusStore {
    conn: Arc<Mutex<Connection>>,
}

impl BusStore {
    pub fn open(path: &Path) -> Result<Self> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).ok();
        }
        let conn = Connection::open(path)
            .with_context(|| format!("opening bus store {}", path.display()))?;
        conn.pragma_update(None, "journal_mode", "WAL")?;
        let found: i64 = conn.pragma_query_value(None, "user_version", |r| r.get(0))?;
        if found != 0 && found != SCHEMA_VERSION {
            anyhow::bail!(
                "bus store {} was written by schema v{found}, this build speaks v{SCHEMA_VERSION}; \
                 messages are ephemeral coordination — retire the old store (delete the .db and \
                 its -wal/-shm sidecars) and a fresh one is created on the next run",
                path.display()
            );
        }
        conn.execute_batch(SCHEMA)?;
        conn.pragma_update(None, "user_version", SCHEMA_VERSION)?;
        Ok(Self {
            conn: Arc::new(Mutex::new(conn)),
        })
    }

    pub fn open_in_memory() -> Result<Self> {
        let conn = Connection::open_in_memory()?;
        conn.execute_batch(SCHEMA)?;
        Ok(Self {
            conn: Arc::new(Mutex::new(conn)),
        })
    }

    // ---------------------------------------------------------------- agents

    pub fn register_agent(
        &self,
        name: &str,
        repo: &Path,
        channel: &str,
        session_id: &str,
        charter: Option<&str>,
    ) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO agents(name,repo,channel,session_id,charter,created_at,last_spawned_at)
             VALUES(?1,?2,?3,?4,?5,?6,?6)
             ON CONFLICT(name) DO UPDATE SET repo=?2, channel=?3, session_id=?4,
               charter=COALESCE(?5, agents.charter), last_spawned_at=?6",
            params![
                name,
                repo.to_string_lossy(),
                channel,
                session_id,
                charter,
                now_epoch()
            ],
        )?;
        Ok(())
    }

    pub fn agents(&self) -> Result<Vec<AgentRow>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt =
            conn.prepare("SELECT name, repo, channel, session_id, charter FROM agents ORDER BY name")?;
        let rows = stmt
            .query_map([], |r| {
                Ok(AgentRow {
                    name: r.get(0)?,
                    repo: PathBuf::from(r.get::<_, String>(1)?),
                    channel: r.get(2)?,
                    session_id: r.get(3)?,
                    charter: r.get(4)?,
                })
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    pub fn channel_members(&self, channel: &str) -> Result<Vec<String>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare("SELECT name FROM agents WHERE channel=?1 ORDER BY name")?;
        let rows = stmt
            .query_map([channel], |r| r.get::<_, String>(0))?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    // -------------------------------------------------------------- messages

    /// Insert one message bound for one resolved recipient. Fan-out (channel
    /// → members) happens above this layer, which sees only resolved rows.
    #[allow(clippy::too_many_arguments)]
    pub fn insert_message(
        &self,
        sender: &str,
        recipient: &str,
        to_display: &str,
        urgency: &str,
        body: &str,
        thread: Option<&str>,
        record_ref: Option<&str>,
    ) -> Result<i64> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO messages(uuid,thread,sender,recipient,to_display,urgency,body,record_ref,created_at)
             VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9)",
            params![
                uuid::Uuid::new_v4().to_string(),
                thread,
                sender,
                recipient,
                to_display,
                urgency,
                body,
                record_ref,
                now_epoch()
            ],
        )?;
        Ok(conn.last_insert_rowid())
    }

    /// Undelivered messages for a recipient, in send order. Class never
    /// reorders.
    pub fn pending_for(&self, recipient: &str) -> Result<Vec<StoredMessage>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT id,uuid,thread,sender,recipient,to_display,urgency,body,record_ref,
                    created_at,delivered_at,delivered_via,ingested_at
             FROM messages WHERE recipient=?1 AND delivered_at IS NULL ORDER BY id",
        )?;
        let rows = stmt
            .query_map([recipient], row_to_message)?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    pub fn pending_count(&self, recipient: &str) -> Result<i64> {
        let conn = self.conn.lock().unwrap();
        Ok(conn.query_row(
            "SELECT COUNT(*) FROM messages WHERE recipient=?1 AND delivered_at IS NULL",
            [recipient],
            |r| r.get(0),
        )?)
    }

    pub fn mark_delivered(&self, ids: &[i64], via: &str, ingest_uuid: Option<&str>) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        let now = now_epoch();
        for id in ids {
            conn.execute(
                "UPDATE messages SET delivered_at=?1, delivered_via=?2, ingest_uuid=?3 WHERE id=?4",
                params![now, via, ingest_uuid, id],
            )?;
        }
        Ok(())
    }

    /// The runtime acknowledged the user message that carried these rows:
    /// ingestion proven.
    pub fn mark_ingested(&self, ingest_uuid: &str) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "UPDATE messages SET ingested_at=?1 WHERE ingest_uuid=?2 AND ingested_at IS NULL",
            params![now_epoch(), ingest_uuid],
        )?;
        Ok(())
    }

    /// The lookback: chronology at a glance (`bus log`).
    pub fn log(&self, limit: i64) -> Result<Vec<StoredMessage>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT id,uuid,thread,sender,recipient,to_display,urgency,body,record_ref,
                    created_at,delivered_at,delivered_via,ingested_at
             FROM messages ORDER BY id DESC LIMIT ?1",
        )?;
        let mut rows = stmt
            .query_map([limit], row_to_message)?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        rows.reverse();
        Ok(rows)
    }
}

fn row_to_message(r: &rusqlite::Row<'_>) -> std::result::Result<StoredMessage, rusqlite::Error> {
    Ok(StoredMessage {
        id: r.get(0)?,
        uuid: r.get(1)?,
        thread: r.get(2)?,
        sender: r.get(3)?,
        recipient: r.get(4)?,
        to_display: r.get(5)?,
        urgency: r.get(6)?,
        body: r.get(7)?,
        record_ref: r.get(8)?,
        created_at: r.get(9)?,
        delivered_at: r.get(10)?,
        delivered_via: r.get(11)?,
        ingested_at: r.get(12)?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn send_pending_deliver_ingest_roundtrip() {
        let s = BusStore::open_in_memory().unwrap();
        s.insert_message("arch", "impl", "@impl", "normal", "hello", None, None)
            .unwrap();
        s.insert_message("arch", "impl", "@impl", "gating", "now!", Some("t-1"), None)
            .unwrap();
        let pending = s.pending_for("impl").unwrap();
        assert_eq!(pending.len(), 2);
        // Send order, never class order.
        assert_eq!(pending[0].urgency, "normal");
        let ids: Vec<i64> = pending.iter().map(|m| m.id).collect();
        s.mark_delivered(&ids, "wake", Some("wire-uuid-1")).unwrap();
        assert_eq!(s.pending_count("impl").unwrap(), 0);
        s.mark_ingested("wire-uuid-1").unwrap();
        let log = s.log(10).unwrap();
        assert!(log.iter().all(|m| m.ingested_at.is_some()));
    }

    #[test]
    fn channel_membership_via_agent_registry() {
        let s = BusStore::open_in_memory().unwrap();
        s.register_agent("a", Path::new("/r/proj"), "proj", "sid-a", None)
            .unwrap();
        s.register_agent("b", Path::new("/r/proj"), "proj", "sid-b", None)
            .unwrap();
        s.register_agent("c", Path::new("/r/other"), "other", "sid-c", None)
            .unwrap();
        assert_eq!(s.channel_members("proj").unwrap(), vec!["a", "b"]);
    }
}
