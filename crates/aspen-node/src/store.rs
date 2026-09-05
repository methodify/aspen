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
use rusqlite::{params, Connection, OptionalExtension};

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
  ingested_at   REAL,
  post          TEXT
);
CREATE INDEX IF NOT EXISTS idx_pending ON messages(recipient, delivered_at);
CREATE INDEX IF NOT EXISTS idx_ingest ON messages(ingest_uuid);
CREATE UNIQUE INDEX IF NOT EXISTS idx_uuid ON messages(uuid);
CREATE TABLE IF NOT EXISTS agents(
  name            TEXT PRIMARY KEY,  -- the address: bare@repo-handle
  repo            TEXT NOT NULL,
  channel         TEXT NOT NULL,     -- the repo handle (== repo channel)
  session_id      TEXT,
  charter         TEXT,
  created_at      REAL NOT NULL,
  last_spawned_at REAL,
  title           TEXT,
  extra_args      TEXT,
  live            INTEGER NOT NULL DEFAULT 0,
  last_exit_code  INTEGER,
  last_exit_at    REAL
);
CREATE TABLE IF NOT EXISTS repos(
  path             TEXT PRIMARY KEY,
  skip_permissions INTEGER NOT NULL DEFAULT 0,
  added_at         REAL NOT NULL,
  last_used_at     REAL,
  handle           TEXT
);
CREATE TABLE IF NOT EXISTS channels(
  name        TEXT PRIMARY KEY,
  topic       TEXT,
  created_at  REAL NOT NULL
);
CREATE TABLE IF NOT EXISTS channel_members(
  channel  TEXT NOT NULL,
  member   TEXT NOT NULL,
  PRIMARY KEY(channel, member)
);
CREATE INDEX IF NOT EXISTS idx_chanmem ON channel_members(channel);
CREATE TABLE IF NOT EXISTS bookmarks(
  id           INTEGER PRIMARY KEY,
  agent        TEXT NOT NULL,
  session_id   TEXT NOT NULL,
  message_uuid TEXT,
  label        TEXT,
  reason       TEXT NOT NULL,
  created_at   REAL NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_bookmark_agent ON bookmarks(agent);
CREATE TABLE IF NOT EXISTS events(
  id     INTEGER PRIMARY KEY,
  ts     REAL NOT NULL,
  agent  TEXT NOT NULL,
  kind   TEXT NOT NULL,
  detail TEXT
);
CREATE INDEX IF NOT EXISTS idx_events_ts ON events(ts);
CREATE INDEX IF NOT EXISTS idx_events_agent ON events(agent, ts);
CREATE TABLE IF NOT EXISTS links(
  id         INTEGER PRIMARY KEY,
  src        TEXT NOT NULL,
  dst        TEXT NOT NULL,
  two_way    INTEGER NOT NULL DEFAULT 0,
  purpose    TEXT,
  urgency    TEXT,
  created_at REAL NOT NULL
);
CREATE TABLE IF NOT EXISTS lineage(
  child_session  TEXT PRIMARY KEY,
  parent_session TEXT NOT NULL,
  fork_message   TEXT,
  agent          TEXT NOT NULL,
  created_at     REAL NOT NULL
);
CREATE TABLE IF NOT EXISTS seen_sessions(
  repo       TEXT NOT NULL,
  session_id TEXT NOT NULL,
  PRIMARY KEY(repo, session_id)
);
CREATE TABLE IF NOT EXISTS adoptions(
  id             INTEGER PRIMARY KEY,
  repo           TEXT NOT NULL,
  session_id     TEXT NOT NULL UNIQUE,
  kind           TEXT NOT NULL,
  of_agent       TEXT,
  parent_session TEXT,
  fork_message   TEXT,
  title          TEXT,
  entrypoint     TEXT,
  first_seen     REAL NOT NULL,
  resolved       TEXT,
  resolved_at    REAL,
  resolved_as    TEXT
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
    pub post: Option<String>,
}

/// One logical channel post (a fan-out collapsed to a single entry).
#[derive(Debug, Clone)]
pub struct ChannelPost {
    pub post: String,
    pub sender: String,
    pub urgency: String,
    pub body: String,
    pub thread: Option<String>,
    pub record_ref: Option<String>,
    pub created_at: f64,
    pub recipients: i64,
    pub delivered: i64,
    pub ingested: i64,
}

#[derive(Debug, Clone)]
pub struct RepoRow {
    pub path: PathBuf,
    pub skip_permissions: bool,
    pub last_used_at: Option<f64>,
    /// The repo's handle: its address segment and channel name. Defaults
    /// to the directory basename, unique per node, operator-renamable.
    pub handle: String,
}

/// One entry in the fleet event log.
#[derive(Debug, Clone, serde::Serialize)]
pub struct FleetEvent {
    pub id: i64,
    pub ts: f64,
    pub agent: String,
    pub kind: String,
    pub detail: serde_json::Value,
}

/// A session that appeared outside Aspen and relates to an agent: a fork
/// of its head, or its head being driven from elsewhere (adoption.rs).
#[derive(Debug, Clone, serde::Serialize)]
pub struct Adoption {
    pub id: i64,
    pub repo: PathBuf,
    pub session_id: String,
    /// fork | resumed
    pub kind: String,
    pub of_agent: Option<String>,
    pub parent_session: Option<String>,
    pub fork_message: Option<String>,
    pub title: Option<String>,
    pub entrypoint: Option<String>,
    pub first_seen: f64,
    /// carry | split | ignore | revive, once answered.
    pub resolved: Option<String>,
    pub resolved_at: Option<f64>,
    /// The agent that took the session (split/carry).
    pub resolved_as: Option<String>,
}

/// A declared pathway between two endpoints (see topology.rs).
#[derive(Debug, Clone, serde::Serialize)]
pub struct Link {
    pub id: i64,
    pub src: String,
    pub dst: String,
    pub two_way: bool,
    pub purpose: Option<String>,
    pub urgency: Option<String>,
    pub created_at: f64,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct Bookmark {
    pub id: i64,
    pub session_id: String,
    pub message_uuid: Option<String>,
    pub label: Option<String>,
    /// "branch" (the tip left behind by branch-here), "swap" (the tip left
    /// behind by resuming a bookmark), or "manual".
    pub reason: String,
    pub created_at: f64,
}

#[derive(Debug, Clone)]
pub struct AgentRow {
    pub name: String,
    pub repo: PathBuf,
    pub channel: String,
    pub session_id: Option<String>,
    pub charter: Option<String>,
    /// Operator-set display title (the agent name stays the bus identity).
    pub title: Option<String>,
    /// Per-session harness CLI args (a raw string, split at spawn time);
    /// re-applied on revive. Harness defaults live in settings, not here.
    pub extra_args: Option<String>,
    /// How and when the process last exited (None = never / clean daemon stop).
    pub last_exit_code: Option<i64>,
    pub last_exit_at: Option<f64>,
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
        // Additive column for stores created before `post` existed. New
        // stores already have it via SCHEMA; the duplicate-column error on
        // those is expected and ignored.
        for stmt in [
            "ALTER TABLE messages ADD COLUMN post TEXT",
            "ALTER TABLE agents ADD COLUMN title TEXT",
            "ALTER TABLE agents ADD COLUMN extra_args TEXT",
            "ALTER TABLE agents ADD COLUMN live INTEGER NOT NULL DEFAULT 0",
            "ALTER TABLE repos ADD COLUMN handle TEXT",
            "ALTER TABLE agents ADD COLUMN last_exit_code INTEGER",
            "ALTER TABLE agents ADD COLUMN last_exit_at REAL",
        ] {
            if let Err(e) = conn.execute(stmt, []) {
                if !e.to_string().contains("duplicate column") {
                    return Err(e.into());
                }
            }
        }
        conn.execute_batch("CREATE UNIQUE INDEX IF NOT EXISTS idx_repo_handle ON repos(handle)")?;
        conn.pragma_update(None, "user_version", SCHEMA_VERSION)?;
        Self::normalize_stored_paths(&conn)?;
        Self::assign_repo_handles(&conn)?;
        Self::scope_agent_names(&conn)?;
        Ok(Self {
            conn: Arc::new(Mutex::new(conn)),
        })
    }

    /// Fold every stored repo path onto its normalized form (see
    /// `node::normalize_repo`). Stores written before normalization hold
    /// paths as Claude Code wrote them or as canonicalize returned them:
    /// on Windows that means `c:\…` vs `C:\…`, 8.3 short names
    /// (`BRYONW~1`), and `\\?\` verbatim prefixes — several rows for one
    /// repo, and lookups that miss. Paths that no longer exist are left as
    /// they are.
    fn normalize_stored_paths(conn: &Connection) -> Result<()> {
        for (table, col) in [("repos", "path"), ("agents", "repo")] {
            let mut stmt = conn.prepare(&format!("SELECT {col} FROM {table}"))?;
            let rows: Vec<String> = stmt
                .query_map([], |r| r.get::<_, String>(0))?
                .collect::<std::result::Result<Vec<_>, _>>()?;
            drop(stmt);
            for old in rows {
                let plain = crate::node::normalize_repo(Path::new(&old))
                    .to_string_lossy()
                    .into_owned();
                if plain == old {
                    continue;
                }
                if table == "repos" {
                    let exists: bool = conn
                        .query_row("SELECT 1 FROM repos WHERE path=?1", params![plain], |_| {
                            Ok(true)
                        })
                        .optional()?
                        .unwrap_or(false);
                    if exists {
                        conn.execute("DELETE FROM repos WHERE path=?1", params![old])?;
                        continue;
                    }
                }
                conn.execute(
                    &format!("UPDATE {table} SET {col}=?2 WHERE {col}=?1"),
                    params![old, plain],
                )?;
            }
        }
        Ok(())
    }

    /// Give every repo a handle: the directory basename, suffixed `-2`,
    /// `-3`… when two repos on this node share a basename. Handles are the
    /// address segment and the channel name, so they must be unique.
    fn assign_repo_handles(conn: &Connection) -> Result<()> {
        let mut stmt =
            conn.prepare("SELECT path FROM repos WHERE handle IS NULL ORDER BY added_at, path")?;
        let paths: Vec<String> = stmt
            .query_map([], |r| r.get::<_, String>(0))?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        drop(stmt);
        for path in paths {
            let base = Path::new(&path)
                .file_name()
                .map(|s| s.to_string_lossy().into_owned())
                .filter(|s| !s.is_empty())
                .unwrap_or_else(|| "repo".into());
            let mut handle = base.clone();
            let mut n = 2;
            while conn
                .query_row(
                    "SELECT 1 FROM repos WHERE handle=?1",
                    params![handle],
                    |_| Ok(()),
                )
                .optional()?
                .is_some()
            {
                handle = format!("{base}-{n}");
                n += 1;
            }
            conn.execute(
                "UPDATE repos SET handle=?2 WHERE path=?1",
                params![path, handle],
            )?;
        }
        Ok(())
    }

    /// Agent names are per repo: the key is `bare@handle`. Stores from
    /// before scoped names hold bare keys; rewrite them (and every place a
    /// bare local name appears: channel members, bus rows).
    fn scope_agent_names(conn: &Connection) -> Result<()> {
        let mut stmt =
            conn.prepare("SELECT name, repo, channel FROM agents WHERE name NOT LIKE '%@%'")?;
        let rows: Vec<(String, String, String)> = stmt
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        drop(stmt);
        for (bare, repo, _channel) in rows {
            // The handle wins over the old channel (they only differ for
            // basename collisions, where the handle got a suffix).
            let handle: String = conn
                .query_row(
                    "SELECT handle FROM repos WHERE path=?1",
                    params![repo],
                    |r| r.get(0),
                )
                .optional()?
                .flatten()
                .unwrap_or_else(|| {
                    Path::new(&repo)
                        .file_name()
                        .map(|s| s.to_string_lossy().into_owned())
                        .unwrap_or_else(|| "repo".into())
                });
            let key = format!("{bare}@{handle}");
            conn.execute(
                "UPDATE agents SET name=?2, channel=?3 WHERE name=?1",
                params![bare, key, handle],
            )?;
            conn.execute(
                "UPDATE channel_members SET member=?2 WHERE member=?1 OR member=?3",
                params![bare, key, format!("@{bare}")],
            )?;
            for col in ["sender", "recipient"] {
                conn.execute(
                    &format!("UPDATE messages SET {col}=?2 WHERE {col}=?1"),
                    params![bare, key],
                )?;
            }
            conn.execute(
                "UPDATE messages SET to_display=?2 WHERE to_display=?1",
                params![format!("@{bare}"), format!("@{key}")],
            )?;
        }
        Ok(())
    }

    pub fn open_in_memory() -> Result<Self> {
        let conn = Connection::open_in_memory()?;
        conn.execute_batch(SCHEMA)?;
        Ok(Self {
            conn: Arc::new(Mutex::new(conn)),
        })
    }

    // ---------------------------------------------------------------- agents

    #[allow(clippy::too_many_arguments)]
    pub fn register_agent(
        &self,
        name: &str,
        repo: &Path,
        channel: &str,
        session_id: &str,
        charter: Option<&str>,
        extra_args: Option<&str>,
    ) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO agents(name,repo,channel,session_id,charter,extra_args,created_at,last_spawned_at)
             VALUES(?1,?2,?3,?4,?5,?6,?7,?7)
             ON CONFLICT(name) DO UPDATE SET repo=?2, channel=?3, session_id=?4,
               charter=COALESCE(?5, agents.charter),
               extra_args=COALESCE(?6, agents.extra_args), last_spawned_at=?7",
            params![
                name,
                repo.to_string_lossy(),
                channel,
                session_id,
                charter,
                extra_args,
                now_epoch()
            ],
        )?;
        Ok(())
    }

    pub fn agents(&self) -> Result<Vec<AgentRow>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT name, repo, channel, session_id, charter, title, extra_args, last_exit_code, last_exit_at FROM agents ORDER BY name",
        )?;
        let rows = stmt
            .query_map([], |r| {
                Ok(AgentRow {
                    name: r.get(0)?,
                    repo: PathBuf::from(r.get::<_, String>(1)?),
                    channel: r.get(2)?,
                    session_id: r.get(3)?,
                    charter: r.get(4)?,
                    title: r.get(5)?,
                    extra_args: r.get(6)?,
                    last_exit_code: r.get(7)?,
                    last_exit_at: r.get(8)?,
                })
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    /// The crash-safe resume ledger: `live` is set on spawn and cleared when
    /// the operator stops the agent or its process exits on its own — but
    /// NOT when the daemon itself shuts down. So whatever is still marked
    /// live at the next `aspen up` is exactly what was running when the
    /// daemon went away, cleanly or not.
    pub fn set_agent_exit(&self, name: &str, code: Option<i32>) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "UPDATE agents SET last_exit_code=?2, last_exit_at=?3 WHERE name=?1",
            params![name, code.map(|c| c as i64), now_epoch()],
        )?;
        Ok(())
    }

    pub fn set_agent_live(&self, name: &str, live: bool) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "UPDATE agents SET live=?2 WHERE name=?1",
            params![name, live as i64],
        )?;
        Ok(())
    }

    pub fn agents_marked_live(&self) -> Result<Vec<String>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare("SELECT name FROM agents WHERE live=1 ORDER BY name")?;
        let rows = stmt
            .query_map([], |r| r.get::<_, String>(0))?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    /// The runtime announced a session id for this agent (a fork, or a
    /// drift we should trust): move the head.
    pub fn set_agent_session(&self, name: &str, session_id: &str) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "UPDATE agents SET session_id=?2 WHERE name=?1",
            params![name, session_id],
        )?;
        Ok(())
    }

    /// Has `sender` ever messaged `recipient`? (Replies are always allowed.)
    pub fn has_messaged(&self, sender: &str, recipient: &str) -> Result<bool> {
        let conn = self.conn.lock().unwrap();
        Ok(conn
            .query_row(
                "SELECT 1 FROM messages WHERE sender=?1 AND recipient=?2 LIMIT 1",
                params![sender, recipient],
                |_| Ok(()),
            )
            .optional()?
            .is_some())
    }

    // ----------------------------------------------------------------- events

    /// Append to the fleet event log (History). `detail` is free-form JSON.
    pub fn record_event(&self, agent: &str, kind: &str, detail: serde_json::Value) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO events(ts, agent, kind, detail) VALUES(?1, ?2, ?3, ?4)",
            params![now_epoch(), agent, kind, detail.to_string()],
        )?;
        Ok(())
    }

    /// Events in [from, to], optionally for one agent, newest last.
    pub fn events(
        &self,
        from: f64,
        to: f64,
        agent: Option<&str>,
        limit: i64,
    ) -> Result<Vec<FleetEvent>> {
        let conn = self.conn.lock().unwrap();
        let mut sql = String::from(
            "SELECT id, ts, agent, kind, detail FROM events WHERE ts >= ?1 AND ts <= ?2",
        );
        if agent.is_some() {
            sql.push_str(" AND agent = ?3");
        }
        sql.push_str(" ORDER BY ts DESC LIMIT ?4");
        let mut stmt = conn.prepare(&sql)?;
        let map = |r: &rusqlite::Row<'_>| {
            Ok(FleetEvent {
                id: r.get(0)?,
                ts: r.get(1)?,
                agent: r.get(2)?,
                kind: r.get(3)?,
                detail: r
                    .get::<_, Option<String>>(4)?
                    .and_then(|s| serde_json::from_str(&s).ok())
                    .unwrap_or(serde_json::Value::Null),
            })
        };
        let rows = if let Some(a) = agent {
            stmt.query_map(params![from, to, a, limit], map)?
                .collect::<std::result::Result<Vec<_>, _>>()?
        } else {
            stmt.query_map(params![from, to, "", limit], map)?
                .collect::<std::result::Result<Vec<_>, _>>()?
        };
        let mut rows = rows;
        rows.reverse();
        Ok(rows)
    }

    /// Bus rows in a time window (for the History timeline).
    pub fn messages_between(&self, from: f64, to: f64, limit: i64) -> Result<Vec<StoredMessage>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT id,uuid,thread,sender,recipient,to_display,urgency,body,record_ref,
                    created_at,delivered_at,delivered_via,ingested_at,post
             FROM messages WHERE created_at >= ?1 AND created_at <= ?2 ORDER BY created_at DESC LIMIT ?3",
        )?;
        let rows = stmt
            .query_map(params![from, to, limit], row_to_message)?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        let mut rows = rows;
        rows.reverse();
        Ok(rows)
    }

    // ------------------------------------------------------------------ links

    pub fn add_link(
        &self,
        src: &str,
        dst: &str,
        two_way: bool,
        purpose: Option<&str>,
        urgency: Option<&str>,
    ) -> Result<i64> {
        let conn = self.conn.lock().unwrap();
        // One link per (src, dst); re-adding updates it.
        let existing: Option<i64> = conn
            .query_row(
                "SELECT id FROM links WHERE src=?1 AND dst=?2",
                params![src, dst],
                |r| r.get(0),
            )
            .optional()?;
        if let Some(id) = existing {
            conn.execute(
                "UPDATE links SET two_way=?2, purpose=?3, urgency=?4 WHERE id=?1",
                params![id, two_way as i64, purpose, urgency],
            )?;
            return Ok(id);
        }
        conn.execute(
            "INSERT INTO links(src, dst, two_way, purpose, urgency, created_at) VALUES(?1,?2,?3,?4,?5,?6)",
            params![src, dst, two_way as i64, purpose, urgency, now_epoch()],
        )?;
        Ok(conn.last_insert_rowid())
    }

    pub fn links(&self) -> Result<Vec<Link>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT id, src, dst, two_way, purpose, urgency, created_at FROM links ORDER BY created_at",
        )?;
        let rows = stmt
            .query_map([], |r| {
                Ok(Link {
                    id: r.get(0)?,
                    src: r.get(1)?,
                    dst: r.get(2)?,
                    two_way: r.get::<_, i64>(3)? != 0,
                    purpose: r.get(4)?,
                    urgency: r.get(5)?,
                    created_at: r.get(6)?,
                })
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    pub fn delete_link(&self, id: i64) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute("DELETE FROM links WHERE id=?1", params![id])?;
        Ok(())
    }

    /// Delete by endpoints (used when mirroring a peer's deletion).
    pub fn delete_link_by_ends(&self, src: &str, dst: &str) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "DELETE FROM links WHERE src=?1 AND dst=?2",
            params![src, dst],
        )?;
        Ok(())
    }

    // ------------------------------------------------------------ adoptions

    /// Sessions already looked at for a repo. Empty = the detector has never
    /// run here (it baselines instead of announcing history).
    pub fn seen_sessions(&self, repo: &Path) -> Result<Vec<String>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare("SELECT session_id FROM seen_sessions WHERE repo=?1")?;
        let rows = stmt
            .query_map(params![repo.to_string_lossy()], |r| r.get(0))?
            .collect::<std::result::Result<Vec<String>, _>>()?;
        Ok(rows)
    }

    pub fn mark_seen(&self, repo: &Path, sessions: &[String]) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        for s in sessions {
            conn.execute(
                "INSERT OR IGNORE INTO seen_sessions(repo, session_id) VALUES(?1, ?2)",
                params![repo.to_string_lossy(), s],
            )?;
        }
        Ok(())
    }

    /// Every session id Aspen has ever attached a name to: heads, bookmarks,
    /// lineage children. The detector treats these as "ours".
    pub fn known_sessions(&self) -> Result<std::collections::HashSet<String>> {
        let conn = self.conn.lock().unwrap();
        let mut out = std::collections::HashSet::new();
        for sql in [
            "SELECT session_id FROM agents WHERE session_id IS NOT NULL",
            "SELECT session_id FROM bookmarks",
            "SELECT child_session FROM lineage",
            "SELECT parent_session FROM lineage",
        ] {
            let mut stmt = conn.prepare(sql)?;
            for r in stmt.query_map([], |r| r.get::<_, String>(0))? {
                out.insert(r?);
            }
        }
        Ok(out)
    }

    /// Which agent owns a session: its head, or a bookmark of it.
    pub fn agent_for_session(&self, session: &str) -> Result<Option<String>> {
        let conn = self.conn.lock().unwrap();
        let head: Option<String> = conn
            .query_row(
                "SELECT name FROM agents WHERE session_id=?1",
                params![session],
                |r| r.get(0),
            )
            .optional()?;
        if head.is_some() {
            return Ok(head);
        }
        let bm: Option<String> = conn
            .query_row(
                "SELECT agent FROM bookmarks WHERE session_id=?1 ORDER BY created_at DESC LIMIT 1",
                params![session],
                |r| r.get(0),
            )
            .optional()?;
        if bm.is_some() {
            return Ok(bm);
        }
        Ok(conn
            .query_row(
                "SELECT agent FROM lineage WHERE child_session=?1",
                params![session],
                |r| r.get(0),
            )
            .optional()?)
    }

    /// Record (or reopen) an adoption. Returns the row id, or None if an
    /// unresolved row for the session already exists.
    #[allow(clippy::too_many_arguments)]
    pub fn upsert_adoption(
        &self,
        repo: &Path,
        session_id: &str,
        kind: &str,
        of_agent: Option<&str>,
        parent_session: Option<&str>,
        fork_message: Option<&str>,
        title: Option<&str>,
        entrypoint: Option<&str>,
    ) -> Result<Option<i64>> {
        let conn = self.conn.lock().unwrap();
        let existing: Option<(i64, Option<String>)> = conn
            .query_row(
                "SELECT id, resolved FROM adoptions WHERE session_id=?1",
                params![session_id],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()?;
        match existing {
            Some((_, None)) => Ok(None),
            Some((id, Some(_))) => {
                conn.execute(
                    "UPDATE adoptions SET kind=?2, of_agent=?3, title=?4, entrypoint=?5,
                     first_seen=?6, resolved=NULL, resolved_at=NULL, resolved_as=NULL WHERE id=?1",
                    params![id, kind, of_agent, title, entrypoint, now_epoch()],
                )?;
                Ok(Some(id))
            }
            None => {
                conn.execute(
                    "INSERT INTO adoptions(repo, session_id, kind, of_agent, parent_session,
                     fork_message, title, entrypoint, first_seen)
                     VALUES(?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
                    params![
                        repo.to_string_lossy(),
                        session_id,
                        kind,
                        of_agent,
                        parent_session,
                        fork_message,
                        title,
                        entrypoint,
                        now_epoch()
                    ],
                )?;
                Ok(Some(conn.last_insert_rowid()))
            }
        }
    }

    pub fn adoptions(&self, unresolved_only: bool) -> Result<Vec<Adoption>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(&format!(
            "SELECT id, repo, session_id, kind, of_agent, parent_session, fork_message, title,
                    entrypoint, first_seen, resolved, resolved_at, resolved_as
             FROM adoptions {} ORDER BY first_seen DESC LIMIT 200",
            if unresolved_only {
                "WHERE resolved IS NULL"
            } else {
                ""
            }
        ))?;
        let rows = stmt
            .query_map([], |r| {
                Ok(Adoption {
                    id: r.get(0)?,
                    repo: PathBuf::from(r.get::<_, String>(1)?),
                    session_id: r.get(2)?,
                    kind: r.get(3)?,
                    of_agent: r.get(4)?,
                    parent_session: r.get(5)?,
                    fork_message: r.get(6)?,
                    title: r.get(7)?,
                    entrypoint: r.get(8)?,
                    first_seen: r.get(9)?,
                    resolved: r.get(10)?,
                    resolved_at: r.get(11)?,
                    resolved_as: r.get(12)?,
                })
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    pub fn adoption(&self, id: i64) -> Result<Option<Adoption>> {
        Ok(self.adoptions(false)?.into_iter().find(|a| a.id == id))
    }

    pub fn resolve_adoption(&self, id: i64, how: &str, as_agent: Option<&str>) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "UPDATE adoptions SET resolved=?2, resolved_at=?3, resolved_as=?4 WHERE id=?1",
            params![id, how, now_epoch(), as_agent],
        )?;
        Ok(())
    }

    /// When an adoption for this session was last answered (any kind).
    pub fn adoption_resolved_at(&self, session: &str) -> Result<Option<f64>> {
        let conn = self.conn.lock().unwrap();
        Ok(conn
            .query_row(
                "SELECT resolved_at FROM adoptions WHERE session_id=?1",
                params![session],
                |r| r.get::<_, Option<f64>>(0),
            )
            .optional()?
            .flatten())
    }

    // ------------------------------------------------------ bookmarks/lineage

    pub fn add_bookmark(
        &self,
        agent: &str,
        session_id: &str,
        message_uuid: Option<&str>,
        label: Option<&str>,
        reason: &str,
    ) -> Result<i64> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO bookmarks(agent, session_id, message_uuid, label, reason, created_at)
             VALUES(?1, ?2, ?3, ?4, ?5, ?6)",
            params![agent, session_id, message_uuid, label, reason, now_epoch()],
        )?;
        Ok(conn.last_insert_rowid())
    }

    pub fn bookmarks(&self, agent: &str) -> Result<Vec<Bookmark>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT id, session_id, message_uuid, label, reason, created_at
             FROM bookmarks WHERE agent=?1 ORDER BY created_at DESC",
        )?;
        let rows = stmt
            .query_map(params![agent], |r| {
                Ok(Bookmark {
                    id: r.get(0)?,
                    session_id: r.get(1)?,
                    message_uuid: r.get(2)?,
                    label: r.get(3)?,
                    reason: r.get(4)?,
                    created_at: r.get(5)?,
                })
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    pub fn bookmark(&self, agent: &str, id: i64) -> Result<Option<Bookmark>> {
        Ok(self.bookmarks(agent)?.into_iter().find(|b| b.id == id))
    }

    pub fn delete_bookmark(&self, agent: &str, id: i64) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "DELETE FROM bookmarks WHERE agent=?1 AND id=?2",
            params![agent, id],
        )?;
        Ok(())
    }

    pub fn record_lineage(
        &self,
        agent: &str,
        child: &str,
        parent: &str,
        fork_message: Option<&str>,
    ) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT OR REPLACE INTO lineage(child_session, parent_session, fork_message, agent, created_at)
             VALUES(?1, ?2, ?3, ?4, ?5)",
            params![child, parent, fork_message, agent, now_epoch()],
        )?;
        Ok(())
    }

    /// Parent chain of a session, nearest first.
    pub fn lineage_of(&self, session: &str) -> Result<Vec<(String, Option<String>)>> {
        let conn = self.conn.lock().unwrap();
        let mut out = Vec::new();
        let mut cur = session.to_owned();
        for _ in 0..64 {
            let row: Option<(String, Option<String>)> = conn
                .query_row(
                    "SELECT parent_session, fork_message FROM lineage WHERE child_session=?1",
                    params![cur],
                    |r| Ok((r.get(0)?, r.get(1)?)),
                )
                .optional()?;
            let Some((parent, at)) = row else { break };
            out.push((parent.clone(), at));
            cur = parent;
        }
        Ok(out)
    }

    pub fn set_agent_title(&self, name: &str, title: Option<&str>) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        let n = conn.execute(
            "UPDATE agents SET title=?2 WHERE name=?1",
            params![name, title.filter(|t| !t.trim().is_empty())],
        )?;
        if n == 0 {
            anyhow::bail!("no agent named @{name} on record");
        }
        Ok(())
    }

    /// Update the stored charter. Takes effect at the next spawn/revive —
    /// a charter rides `appendSystemPrompt` and cannot change mid-session.
    pub fn set_agent_charter(&self, name: &str, charter: Option<&str>) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        let n = conn.execute(
            "UPDATE agents SET charter=?2 WHERE name=?1",
            params![name, charter.filter(|c| !c.trim().is_empty())],
        )?;
        if n == 0 {
            anyhow::bail!("no agent named @{name} on record");
        }
        Ok(())
    }

    // ------------------------------------------------------------------ repos

    /// Remember a repo (idempotent). Preserves an existing skip default
    /// unless `skip` is given.
    pub fn add_repo(&self, path: &Path, skip: Option<bool>) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO repos(path, skip_permissions, added_at, last_used_at)
             VALUES(?1, ?2, ?3, ?3)
             ON CONFLICT(path) DO UPDATE SET
               skip_permissions = COALESCE(?4, repos.skip_permissions),
               last_used_at = ?3",
            params![
                path.to_string_lossy(),
                skip.unwrap_or(false) as i64,
                now_epoch(),
                skip.map(|b| b as i64),
            ],
        )?;
        Ok(())
    }

    /// The handle for a repo path, registering the repo if needed. This is
    /// the one way a spawn learns its address segment / channel.
    pub fn ensure_handle(&self, path: &Path) -> Result<String> {
        self.add_repo(path, None)?;
        let conn = self.conn.lock().unwrap();
        Self::assign_repo_handles(&conn)?;
        let h: Option<String> = conn.query_row(
            "SELECT handle FROM repos WHERE path=?1",
            params![path.to_string_lossy()],
            |r| r.get(0),
        )?;
        h.ok_or_else(|| anyhow::anyhow!("no handle for {}", path.display()))
    }

    /// Rename a repo's handle. Refused while any agent in the repo is
    /// running (their addresses would change under them); otherwise every
    /// stored address that carries the old handle is rewritten.
    pub fn rename_handle(&self, path: &Path, new: &str, live_names: &[String]) -> Result<()> {
        let new = new.trim();
        if new.is_empty()
            || !new
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.')
        {
            anyhow::bail!("handle must be [A-Za-z0-9._-]+");
        }
        let conn = self.conn.lock().unwrap();
        let old: String = conn
            .query_row(
                "SELECT handle FROM repos WHERE path=?1",
                params![path.to_string_lossy()],
                |r| r.get::<_, Option<String>>(0),
            )
            .optional()?
            .flatten()
            .ok_or_else(|| anyhow::anyhow!("repo not registered: {}", path.display()))?;
        if old == new {
            return Ok(());
        }
        if conn
            .query_row("SELECT 1 FROM repos WHERE handle=?1", params![new], |_| {
                Ok(())
            })
            .optional()?
            .is_some()
        {
            anyhow::bail!("handle '{new}' is already used by another repo on this node");
        }
        if live_names.iter().any(|n| n.ends_with(&format!("@{old}"))) {
            anyhow::bail!("stop the sessions in #{old} before renaming it");
        }
        conn.execute(
            "UPDATE repos SET handle=?2 WHERE path=?1",
            params![path.to_string_lossy(), new],
        )?;
        let suffix_old = format!("@{old}");
        let suffix_new = format!("@{new}");
        // agents keyed bare@old → bare@new; channel column too
        let mut stmt = conn.prepare("SELECT name FROM agents WHERE channel=?1")?;
        let names: Vec<String> = stmt
            .query_map(params![old], |r| r.get::<_, String>(0))?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        drop(stmt);
        for name in names {
            let newname = name.replacen(&suffix_old, &suffix_new, 1);
            conn.execute(
                "UPDATE agents SET name=?2, channel=?3 WHERE name=?1",
                params![name, newname, new],
            )?;
            conn.execute(
                "UPDATE channel_members SET member=?2 WHERE member=?1",
                params![name, newname],
            )?;
            for col in ["sender", "recipient"] {
                conn.execute(
                    &format!("UPDATE messages SET {col}=?2 WHERE {col}=?1"),
                    params![name, newname],
                )?;
            }
        }
        // channel_members may reference the repo channel itself (#old)
        conn.execute(
            "UPDATE channel_members SET member=?2 WHERE member=?1",
            params![format!("#{old}"), format!("#{new}")],
        )?;
        Ok(())
    }

    pub fn set_repo_skip(&self, path: &Path, skip: bool) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        let n = conn.execute(
            "UPDATE repos SET skip_permissions=?2 WHERE path=?1",
            params![path.to_string_lossy(), skip as i64],
        )?;
        if n == 0 {
            anyhow::bail!("repo not registered: {}", path.display());
        }
        Ok(())
    }

    pub fn remove_repo(&self, path: &Path) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute("DELETE FROM repos WHERE path=?1", [path.to_string_lossy()])?;
        Ok(())
    }

    pub fn repos(&self) -> Result<Vec<RepoRow>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT path, skip_permissions, last_used_at, handle FROM repos ORDER BY last_used_at DESC",
        )?;
        let rows = stmt
            .query_map([], |r| {
                Ok(RepoRow {
                    path: PathBuf::from(r.get::<_, String>(0)?),
                    skip_permissions: r.get::<_, i64>(1)? != 0,
                    last_used_at: r.get(2)?,
                    handle: r.get::<_, Option<String>>(3)?.unwrap_or_default(),
                })
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    pub fn repo(&self, path: &Path) -> Result<Option<RepoRow>> {
        Ok(self.repos()?.into_iter().find(|r| r.path == path))
    }

    /// Members of a repo auto-channel: the local agents homed in it.
    pub fn channel_members(&self, channel: &str) -> Result<Vec<String>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare("SELECT name FROM agents WHERE channel=?1 ORDER BY name")?;
        let rows = stmt
            .query_map([channel], |r| r.get::<_, String>(0))?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    // --------------------------------------------------------------- channels

    /// Custom channels (explicit membership; may span repos and nodes) —
    /// distinct from a repo's implicit auto-channel.
    pub fn create_channel(&self, name: &str, topic: Option<&str>) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO channels(name, topic, created_at) VALUES(?1, ?2, ?3)
             ON CONFLICT(name) DO UPDATE SET topic=COALESCE(?2, channels.topic)",
            params![name, topic, now_epoch()],
        )?;
        Ok(())
    }

    pub fn delete_channel(&self, name: &str) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute("DELETE FROM channels WHERE name=?1", [name])?;
        conn.execute("DELETE FROM channel_members WHERE channel=?1", [name])?;
        Ok(())
    }

    pub fn set_channel_topic(&self, name: &str, topic: &str) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "UPDATE channels SET topic=?2 WHERE name=?1",
            params![name, topic],
        )?;
        Ok(())
    }

    /// Add a member address to a custom channel. `member` is a bare local
    /// agent name, a qualified `name@node`, or `@operator`.
    pub fn add_channel_member(&self, channel: &str, member: &str) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT OR IGNORE INTO channel_members(channel, member) VALUES(?1, ?2)",
            params![channel, member],
        )?;
        Ok(())
    }

    pub fn remove_channel_member(&self, channel: &str, member: &str) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "DELETE FROM channel_members WHERE channel=?1 AND member=?2",
            params![channel, member],
        )?;
        Ok(())
    }

    /// Explicit members of a custom channel (addresses, verbatim).
    pub fn custom_channel_members(&self, channel: &str) -> Result<Vec<String>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt =
            conn.prepare("SELECT member FROM channel_members WHERE channel=?1 ORDER BY member")?;
        let rows = stmt
            .query_map([channel], |r| r.get::<_, String>(0))?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    /// True if a custom channel row exists (vs an implicit repo channel).
    /// Rewrite legacy `bare@node` channel members (pre-scoped-names) to
    /// `bare@repo@node` once the node's roster tells us the repo — only
    /// when exactly one agent of that bare name lives there.
    pub fn heal_legacy_remote_members(&self, node: &str, keys: &[String]) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        let mut stmt =
            conn.prepare("SELECT DISTINCT member FROM channel_members WHERE member LIKE ?1")?;
        let members: Vec<String> = stmt
            .query_map(params![format!("%@{node}")], |r| r.get::<_, String>(0))?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        drop(stmt);
        for m in members {
            let raw = m.trim_start_matches('@');
            // Legacy = exactly two segments, the second being the node.
            if raw.matches('@').count() != 1 {
                continue;
            }
            let bare = raw.split('@').next().unwrap_or("");
            let matches: Vec<&String> = keys
                .iter()
                .filter(|k| k.split('@').next() == Some(bare))
                .collect();
            if let [key] = matches.as_slice() {
                conn.execute(
                    "UPDATE channel_members SET member=?2 WHERE member=?1",
                    params![m, format!("{key}@{node}")],
                )?;
            }
        }
        Ok(())
    }

    /// The custom channels an address belongs to (member stored as
    /// `key`, `@key`, or `key@node`).
    pub fn channels_of(&self, member: &str) -> Result<Vec<String>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT channel FROM channel_members WHERE member=?1 OR member=?2 ORDER BY channel",
        )?;
        let rows = stmt
            .query_map(params![member, format!("@{member}")], |r| {
                r.get::<_, String>(0)
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    pub fn channel_exists(&self, name: &str) -> Result<bool> {
        let conn = self.conn.lock().unwrap();
        Ok(conn
            .query_row("SELECT 1 FROM channels WHERE name=?1", [name], |_| Ok(()))
            .optional()?
            .is_some())
    }

    /// (name, topic, member_count) for every custom channel.
    pub fn channels(&self) -> Result<Vec<(String, Option<String>, i64)>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT c.name, c.topic, COUNT(m.member)
             FROM channels c LEFT JOIN channel_members m ON m.channel = c.name
             GROUP BY c.name, c.topic ORDER BY c.name",
        )?;
        let rows = stmt
            .query_map([], |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, Option<String>>(1)?,
                    r.get::<_, i64>(2)?,
                ))
            })?
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
        post: Option<&str>,
    ) -> Result<i64> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO messages(uuid,thread,sender,recipient,to_display,urgency,body,record_ref,created_at,post)
             VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10)",
            params![
                uuid::Uuid::new_v4().to_string(),
                thread,
                sender,
                recipient,
                to_display,
                urgency,
                body,
                record_ref,
                now_epoch(),
                post
            ],
        )?;
        Ok(conn.last_insert_rowid())
    }

    /// Insert a message that arrived over federation, keeping its origin
    /// uuid. Duplicate forwards (at-least-once) no-op on the unique index;
    /// returns whether a row was actually inserted.
    #[allow(clippy::too_many_arguments)]
    pub fn insert_federated(
        &self,
        uuid: &str,
        sender: &str,
        recipient: &str,
        to_display: &str,
        urgency: &str,
        body: &str,
        thread: Option<&str>,
        record_ref: Option<&str>,
        created_at: Option<f64>,
    ) -> Result<bool> {
        let conn = self.conn.lock().unwrap();
        let n = conn.execute(
            "INSERT INTO messages(uuid,thread,sender,recipient,to_display,urgency,body,record_ref,created_at,post)
             VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?1)
             ON CONFLICT(uuid) DO NOTHING",
            params![
                uuid,
                thread,
                sender,
                recipient,
                to_display,
                urgency,
                body,
                record_ref,
                created_at.unwrap_or_else(now_epoch)
            ],
        )?;
        Ok(n > 0)
    }

    /// Federation ack path: the home node stored it; the origin's row is
    /// done traveling.
    pub fn mark_delivered_by_uuid(&self, uuid: &str, via: &str) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "UPDATE messages SET delivered_at=?1, delivered_via=?2
             WHERE uuid=?3 AND delivered_at IS NULL",
            params![now_epoch(), via, uuid],
        )?;
        Ok(())
    }

    /// Undelivered messages for a recipient, in send order. Class never
    /// reorders.
    /// Distinct recipients with undelivered rows (for periodic re-ticks).
    pub fn pending_recipients(&self) -> Result<Vec<String>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt =
            conn.prepare("SELECT DISTINCT recipient FROM messages WHERE delivered_at IS NULL")?;
        let rows = stmt
            .query_map([], |r| r.get(0))?
            .collect::<std::result::Result<Vec<String>, _>>()?;
        Ok(rows)
    }

    pub fn pending_for(&self, recipient: &str) -> Result<Vec<StoredMessage>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT id,uuid,thread,sender,recipient,to_display,urgency,body,record_ref,
                    created_at,delivered_at,delivered_via,ingested_at,post
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

    /// One channel's conversation: posts to `#name`, grouped by post id so a
    /// fan-out to N members is one entry, with per-post delivery aggregates.
    pub fn channel_log(&self, display: &str, limit: i64) -> Result<Vec<ChannelPost>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT COALESCE(post, uuid) AS pid,
                    MIN(id) AS first_id,
                    sender, urgency, body, thread, record_ref, MIN(created_at),
                    COUNT(*) AS recipients,
                    SUM(CASE WHEN delivered_at IS NOT NULL THEN 1 ELSE 0 END) AS delivered,
                    SUM(CASE WHEN ingested_at IS NOT NULL THEN 1 ELSE 0 END) AS ingested
             FROM messages WHERE to_display=?1
             GROUP BY pid ORDER BY first_id DESC LIMIT ?2",
        )?;
        let mut rows = stmt
            .query_map(params![display, limit], |r| {
                Ok(ChannelPost {
                    post: r.get(0)?,
                    sender: r.get(2)?,
                    urgency: r.get(3)?,
                    body: r.get(4)?,
                    thread: r.get(5)?,
                    record_ref: r.get(6)?,
                    created_at: r.get(7)?,
                    recipients: r.get(8)?,
                    delivered: r.get(9)?,
                    ingested: r.get(10)?,
                })
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        rows.reverse();
        Ok(rows)
    }

    /// Direct-message conversations: every counterpart pair that has
    /// exchanged direct (non-channel) traffic, with last-activity ordering.
    /// Returned as (a, b, last_at, message_count) with a<b lexically so a
    /// pair appears once.
    pub fn dm_pairs(&self) -> Result<Vec<(String, String, f64, i64)>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT CASE WHEN sender < recipient THEN sender ELSE recipient END AS a,
                    CASE WHEN sender < recipient THEN recipient ELSE sender END AS b,
                    MAX(created_at), COUNT(*)
             FROM messages WHERE to_display NOT LIKE '#%'
             GROUP BY a, b ORDER BY MAX(created_at) DESC",
        )?;
        let rows = stmt
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    /// One direct conversation, both directions, chronological.
    pub fn dm_log(&self, a: &str, b: &str, limit: i64) -> Result<Vec<StoredMessage>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT id,uuid,thread,sender,recipient,to_display,urgency,body,record_ref,
                    created_at,delivered_at,delivered_via,ingested_at,post
             FROM messages
             WHERE to_display NOT LIKE '#%'
               AND ((sender=?1 AND recipient=?2) OR (sender=?2 AND recipient=?1))
             ORDER BY id DESC LIMIT ?3",
        )?;
        let mut rows = stmt
            .query_map(params![a, b, limit], row_to_message)?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        rows.reverse();
        Ok(rows)
    }

    /// Filtered lookback — the trail's query surface. Every filter is
    /// optional; `q` is a body substring.
    #[allow(clippy::too_many_arguments)]
    pub fn log_filtered(
        &self,
        sender: Option<&str>,
        recipient: Option<&str>,
        thread: Option<&str>,
        record: Option<&str>,
        urgency: Option<&str>,
        pending_only: bool,
        q: Option<&str>,
        limit: i64,
    ) -> Result<Vec<StoredMessage>> {
        let mut sql = String::from(
            "SELECT id,uuid,thread,sender,recipient,to_display,urgency,body,record_ref,
                    created_at,delivered_at,delivered_via,ingested_at,post
             FROM messages WHERE 1=1",
        );
        let mut params_vec: Vec<String> = Vec::new();
        let mut push = |sql: &mut String, clause: &str, v: &str| {
            params_vec.push(v.to_owned());
            sql.push_str(&clause.replace('?', &format!("?{}", params_vec.len())));
        };
        if let Some(v) = sender {
            push(&mut sql, " AND sender=?", v);
        }
        if let Some(v) = recipient {
            push(&mut sql, " AND recipient=?", v);
        }
        if let Some(v) = thread {
            push(&mut sql, " AND thread=?", v);
        }
        if let Some(v) = record {
            push(&mut sql, " AND record_ref LIKE ?", &format!("%{v}%"));
        }
        if let Some(v) = urgency {
            push(&mut sql, " AND urgency=?", v);
        }
        if pending_only {
            sql.push_str(" AND delivered_at IS NULL");
        }
        if let Some(v) = q {
            push(&mut sql, " AND body LIKE ?", &format!("%{v}%"));
        }
        params_vec.push(limit.to_string());
        sql.push_str(&format!(" ORDER BY id DESC LIMIT ?{}", params_vec.len()));

        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(&sql)?;
        let mut rows = stmt
            .query_map(
                rusqlite::params_from_iter(params_vec.iter()),
                row_to_message,
            )?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        rows.reverse();
        Ok(rows)
    }

    /// Per-recipient delivery receipts for one logical post — the
    /// watch-it-land view. Proof of ingestion per recipient.
    pub fn post_receipts(&self, post: &str) -> Result<Vec<StoredMessage>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT id,uuid,thread,sender,recipient,to_display,urgency,body,record_ref,
                    created_at,delivered_at,delivered_via,ingested_at,post
             FROM messages WHERE post=?1 OR uuid=?1 ORDER BY recipient",
        )?;
        let rows = stmt
            .query_map([post], row_to_message)?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    /// The lookback: chronology at a glance (`bus log`).
    pub fn log(&self, limit: i64) -> Result<Vec<StoredMessage>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT id,uuid,thread,sender,recipient,to_display,urgency,body,record_ref,
                    created_at,delivered_at,delivered_via,ingested_at,post
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
        post: r.get(13)?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn send_pending_deliver_ingest_roundtrip() {
        let s = BusStore::open_in_memory().unwrap();
        s.insert_message("arch", "impl", "@impl", "normal", "hello", None, None, None)
            .unwrap();
        s.insert_message(
            "arch",
            "impl",
            "@impl",
            "gating",
            "now!",
            Some("t-1"),
            None,
            None,
        )
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
        s.register_agent("a", Path::new("/r/proj"), "proj", "sid-a", None, None)
            .unwrap();
        s.register_agent("b", Path::new("/r/proj"), "proj", "sid-b", None, None)
            .unwrap();
        s.register_agent("c", Path::new("/r/other"), "other", "sid-c", None, None)
            .unwrap();
        assert_eq!(s.channel_members("proj").unwrap(), vec!["a", "b"]);
    }
}
