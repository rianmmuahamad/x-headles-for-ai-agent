//! Store SQLite: kredensial akun, job, draft, snapshot metrics, cache queryId, event sesi.
//!
//! API di sini **sinkron** (rusqlite adalah library sinkron). Pemanggil async MUST
//! membungkus operasi berat dengan `tokio::task::spawn_blocking`.

use std::path::Path;

use rusqlite::{params, Connection, OptionalExtension};
use time::{format_description::well_known::Rfc3339, OffsetDateTime};

use crate::config::restrict_permissions;
use crate::domain::{AccountId, AccountSummary, Handle, Metrics};
use crate::error::XhlError;
use crate::session::Cookies;

const MIGRATIONS: &[&str] = &[
    // v1 — skema awal
    r#"
    CREATE TABLE accounts (
      id          TEXT PRIMARY KEY,
      handle      TEXT,
      user_id     TEXT,
      cookies     TEXT NOT NULL,
      created_at  TEXT NOT NULL,
      updated_at  TEXT NOT NULL
    );

    CREATE TABLE jobs (
      id                 TEXT PRIMARY KEY,
      account            TEXT NOT NULL,
      run_at             TEXT NOT NULL,
      payload            TEXT NOT NULL,
      state              TEXT NOT NULL,
      attempts           INTEGER NOT NULL DEFAULT 0,
      dedup_key          TEXT,
      idempotency_marker TEXT,
      created_at         TEXT NOT NULL,
      updated_at         TEXT NOT NULL
    );
    CREATE INDEX idx_jobs_due ON jobs(state, run_at);

    CREATE TABLE drafts (
      id         TEXT PRIMARY KEY,
      account    TEXT NOT NULL,
      body       TEXT NOT NULL,
      name       TEXT,
      origin     TEXT NOT NULL,
      created_at TEXT NOT NULL
    );

    CREATE TABLE metrics_snapshots (
      tweet_id    TEXT NOT NULL,
      captured_at TEXT NOT NULL,
      likes       INTEGER,
      reposts     INTEGER,
      replies     INTEGER,
      views       INTEGER,
      bookmarks   INTEGER,
      PRIMARY KEY (tweet_id, captured_at)
    );

    CREATE TABLE query_ids (
      operation   TEXT PRIMARY KEY,
      query_id    TEXT NOT NULL,
      source      TEXT NOT NULL,
      observed_at TEXT NOT NULL
    );

    CREATE TABLE session_events (
      at      TEXT NOT NULL,
      account TEXT NOT NULL,
      kind    TEXT NOT NULL,
      detail  TEXT
    );
    CREATE INDEX idx_session_events ON session_events(account, at);
    "#,
    // v2 — penambahan media draft dan log dedup posting
    r#"
    ALTER TABLE drafts ADD COLUMN media TEXT NOT NULL DEFAULT '[]';
    CREATE TABLE post_log (
      dedup_key TEXT PRIMARY KEY,
      tweet_id  TEXT NOT NULL,
      account   TEXT NOT NULL,
      at        TEXT NOT NULL
    );
    "#,
    // v3 — cache state signer anti-bot
    //
    // Bootstrap memerlukan 1–2 round-trip jaringan (halaman + bundle JS).
    // State-nya hanya bergantung pada bundle X, bukan pada request, sehingga
    // aman dipakai ulang antar proses. Disimpan per akun karena halaman yang
    // menghasilkan state diambil dengan sesi akun tersebut.
    r#"
    CREATE TABLE antibot_state (
      account   TEXT PRIMARY KEY,
      state     TEXT NOT NULL,
      source    TEXT NOT NULL,
      saved_at  TEXT NOT NULL
    );
    "#,
];

pub struct Store {
    conn: Connection,
}

impl Store {
    /// Buka (atau buat) database di `path`, lalu jalankan migrasi.
    pub fn open(path: &Path) -> Result<Self, XhlError> {
        let conn = Connection::open(path)
            .map_err(|e| XhlError::Config(format!("gagal membuka {}: {e}", path.display())))?;
        let store = Self::init(conn)?;
        // Cookie tersimpan di sini: batasi akses file.
        restrict_permissions(path, 0o600)?;
        Ok(store)
    }

    /// Database sementara di memori (test & mode fake).
    pub fn open_in_memory() -> Result<Self, XhlError> {
        Self::init(Connection::open_in_memory().map_err(|e| XhlError::Internal(e.to_string()))?)
    }

    fn init(conn: Connection) -> Result<Self, XhlError> {
        conn.busy_timeout(std::time::Duration::from_secs(5))
            .map_err(|e| XhlError::Internal(e.to_string()))?;
        // WAL tidak berlaku untuk in-memory; kegagalannya tidak fatal.
        if let Err(e) = conn.pragma_update(None, "journal_mode", "WAL") {
            tracing::debug!(error = %e, "journal_mode WAL tidak tersedia");
        }
        let mut store = Self { conn };
        store.migrate()?;
        Ok(store)
    }

    /// Migrasi berbasis `user_version`: terapkan yang belum dijalankan.
    fn migrate(&mut self) -> Result<(), XhlError> {
        let current: i64 = self
            .conn
            .query_row("PRAGMA user_version", [], |row| row.get(0))
            .map_err(|e| XhlError::Internal(format!("baca user_version: {e}")))?;

        for (idx, sql) in MIGRATIONS.iter().enumerate() {
            let version = (idx + 1) as i64;
            if version <= current {
                continue;
            }
            let tx = self
                .conn
                .transaction()
                .map_err(|e| XhlError::Internal(e.to_string()))?;
            tx.execute_batch(sql)
                .map_err(|e| XhlError::Internal(format!("migrasi v{version} gagal: {e}")))?;
            tx.pragma_update(None, "user_version", version)
                .map_err(|e| XhlError::Internal(e.to_string()))?;
            tx.commit().map_err(|e| XhlError::Internal(e.to_string()))?;
            tracing::info!(version, "migrasi diterapkan");
        }
        Ok(())
    }

    // ---- accounts ----------------------------------------------------------

    /// Simpan/timpa kredensial akun. Identitas lama dipertahankan.
    pub fn upsert_cookies(&self, account: &str, cookies: &Cookies) -> Result<(), XhlError> {
        let now = now_rfc3339();
        let json = serde_json::to_string(cookies)
            .map_err(|e| XhlError::Internal(format!("serialisasi cookie: {e}")))?;
        self.conn
            .execute(
                "INSERT INTO accounts (id, cookies, created_at, updated_at)
                 VALUES (?1, ?2, ?3, ?3)
                 ON CONFLICT(id) DO UPDATE SET cookies = excluded.cookies, updated_at = excluded.updated_at",
                params![account, json, now],
            )
            .map_err(|e| XhlError::Internal(format!("simpan akun: {e}")))?;
        Ok(())
    }

    pub fn cookies(&self, account: &str) -> Result<Option<Cookies>, XhlError> {
        let json: Option<String> = self
            .conn
            .query_row(
                "SELECT cookies FROM accounts WHERE id = ?1",
                params![account],
                |row| row.get(0),
            )
            .optional()
            .map_err(|e| XhlError::Internal(format!("baca akun: {e}")))?;

        match json {
            None => Ok(None),
            Some(raw) => serde_json::from_str(&raw)
                .map(Some)
                .map_err(|e| XhlError::Internal(format!("cookie akun tidak dapat dibaca: {e}"))),
        }
    }

    /// Catat identitas hasil health-check.
    pub fn update_identity(
        &self,
        account: &str,
        handle: Option<&str>,
        user_id: Option<&str>,
    ) -> Result<(), XhlError> {
        self.conn
            .execute(
                "UPDATE accounts SET handle = COALESCE(?2, handle), user_id = COALESCE(?3, user_id),
                 updated_at = ?4 WHERE id = ?1",
                params![account, handle, user_id, now_rfc3339()],
            )
            .map_err(|e| XhlError::Internal(format!("perbarui identitas: {e}")))?;
        Ok(())
    }

    /// Ringkasan semua akun tersimpan (tanpa kredensial).
    pub fn list_accounts(&self) -> Result<Vec<AccountSummary>, XhlError> {
        let mut stmt = self
            .conn
            .prepare("SELECT id, handle, user_id FROM accounts ORDER BY id")
            .map_err(|e| XhlError::Internal(e.to_string()))?;
        let rows = stmt
            .query_map([], |row| {
                Ok(AccountSummary {
                    id: AccountId(row.get(0)?),
                    handle: row.get::<_, Option<String>>(1)?.map(Handle),
                    user_id: row.get(2)?,
                })
            })
            .map_err(|e| XhlError::Internal(e.to_string()))?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(|e| XhlError::Internal(e.to_string()))
    }

    // ---- session_events ----------------------------------------------------

    /// Catat peristiwa sesi (impor ulang, 403, rotasi, dll.) untuk diagnosis.
    pub fn record_event(&self, account: &str, kind: &str, detail: &str) -> Result<(), XhlError> {
        self.conn
            .execute(
                "INSERT INTO session_events (at, account, kind, detail) VALUES (?1, ?2, ?3, ?4)",
                params![now_rfc3339(), account, kind, detail],
            )
            .map_err(|e| XhlError::Internal(format!("catat event: {e}")))?;
        Ok(())
    }

    pub fn recent_events(&self, account: &str, limit: usize) -> Result<Vec<String>, XhlError> {
        let mut stmt = self
            .conn
            .prepare(
                "SELECT at || ' ' || kind || CASE WHEN detail IS NULL THEN '' ELSE ' — ' || detail END
                 FROM session_events WHERE account = ?1 ORDER BY at DESC LIMIT ?2",
            )
            .map_err(|e| XhlError::Internal(e.to_string()))?;
        let rows = stmt
            .query_map(params![account, limit as i64], |row| {
                row.get::<_, String>(0)
            })
            .map_err(|e| XhlError::Internal(e.to_string()))?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(|e| XhlError::Internal(e.to_string()))
    }

    // ---- antibot_state -----------------------------------------------------

    /// Simpan state signer. Timpa yang lama (state lama tidak berguna).
    pub fn save_antibot_state(
        &self,
        account: &str,
        state_json: &str,
        source: &str,
    ) -> Result<(), XhlError> {
        self.conn
            .execute(
                "INSERT INTO antibot_state (account, state, source, saved_at)
                 VALUES (?1, ?2, ?3, ?4)
                 ON CONFLICT(account) DO UPDATE SET
                     state = excluded.state,
                     source = excluded.source,
                     saved_at = excluded.saved_at",
                params![account, state_json, source, now_rfc3339()],
            )
            .map_err(|e| XhlError::Internal(format!("simpan antibot state: {e}")))?;
        Ok(())
    }

    /// State signer terakhir untuk akun. `None` bila belum pernah bootstrap.
    pub fn antibot_state(&self, account: &str) -> Result<Option<String>, XhlError> {
        self.conn
            .query_row(
                "SELECT state FROM antibot_state WHERE account = ?1",
                params![account],
                |row| row.get(0),
            )
            .optional()
            .map_err(|e| XhlError::Internal(format!("baca antibot state: {e}")))
    }

    // ---- query_ids ---------------------------------------------------------

    pub fn set_query_id(&self, op: &str, id: &str, source: &str) -> Result<(), XhlError> {
        let now = now_rfc3339();
        self.conn
            .execute(
                "INSERT INTO query_ids (operation, query_id, source, observed_at)
                 VALUES (?1, ?2, ?3, ?4)
                 ON CONFLICT(operation) DO UPDATE SET
                     query_id = excluded.query_id,
                     source = excluded.source,
                     observed_at = excluded.observed_at",
                params![op, id, source, now],
            )
            .map_err(|e| XhlError::Internal(format!("simpan query_id: {e}")))?;
        Ok(())
    }

    pub fn query_ids(&self) -> Result<Vec<(String, String)>, XhlError> {
        let mut stmt = self
            .conn
            .prepare("SELECT operation, query_id FROM query_ids ORDER BY operation")
            .map_err(|e| XhlError::Internal(e.to_string()))?;
        let rows = stmt
            .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
            .map_err(|e| XhlError::Internal(e.to_string()))?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(|e| XhlError::Internal(e.to_string()))
    }

    // ---- post_log (dedup) --------------------------------------------------

    pub fn record_post(
        &self,
        dedup_key: &str,
        tweet_id: &str,
        account: &str,
    ) -> Result<bool, XhlError> {
        let now = now_rfc3339();
        let affected = self
            .conn
            .execute(
                "INSERT OR IGNORE INTO post_log (dedup_key, tweet_id, account, at) VALUES (?1, ?2, ?3, ?4)",
                params![dedup_key, tweet_id, account, now],
            )
            .map_err(|e| XhlError::Internal(format!("record_post: {e}")))?;
        Ok(affected > 0)
    }

    pub fn posted_tweet(&self, dedup_key: &str) -> Result<Option<String>, XhlError> {
        self.conn
            .query_row(
                "SELECT tweet_id FROM post_log WHERE dedup_key = ?1",
                params![dedup_key],
                |row| row.get(0),
            )
            .optional()
            .map_err(|e| XhlError::Internal(format!("baca post_log: {e}")))
    }

    // ---- metrics_snapshots -------------------------------------------------

    /// Simpan satu snapshot metrics. Timestamp bertabrakan → ditimpa.
    pub fn insert_metrics(
        &self,
        tweet_id: &str,
        m: &Metrics,
        at: OffsetDateTime,
    ) -> Result<(), XhlError> {
        self.conn
            .execute(
                "INSERT OR REPLACE INTO metrics_snapshots
                 (tweet_id, captured_at, likes, reposts, replies, views, bookmarks)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                params![
                    tweet_id,
                    format_rfc3339(at),
                    m.likes.map(|v| v as i64),
                    m.reposts.map(|v| v as i64),
                    m.replies.map(|v| v as i64),
                    m.views.map(|v| v as i64),
                    m.bookmarks.map(|v| v as i64),
                ],
            )
            .map_err(|e| XhlError::Internal(format!("simpan metrics: {e}")))?;
        Ok(())
    }

    /// Snapshot metrics terbaru lebih dulu.
    pub fn metrics_history(
        &self,
        tweet_id: &str,
        limit: usize,
    ) -> Result<Vec<(OffsetDateTime, Metrics)>, XhlError> {
        let mut stmt = self
            .conn
            .prepare(
                "SELECT captured_at, likes, reposts, replies, views, bookmarks
                 FROM metrics_snapshots WHERE tweet_id = ?1
                 ORDER BY captured_at DESC LIMIT ?2",
            )
            .map_err(|e| XhlError::Internal(e.to_string()))?;

        let rows = stmt
            .query_map(params![tweet_id, limit as i64], |row| {
                let raw: String = row.get(0)?;
                Ok((
                    raw,
                    Metrics {
                        likes: row.get::<_, Option<i64>>(1)?.map(|v| v as u64),
                        reposts: row.get::<_, Option<i64>>(2)?.map(|v| v as u64),
                        replies: row.get::<_, Option<i64>>(3)?.map(|v| v as u64),
                        views: row.get::<_, Option<i64>>(4)?.map(|v| v as u64),
                        bookmarks: row.get::<_, Option<i64>>(5)?.map(|v| v as u64),
                    },
                ))
            })
            .map_err(|e| XhlError::Internal(e.to_string()))?;

        rows.collect::<Result<Vec<_>, _>>()
            .map_err(|e| XhlError::Internal(e.to_string()))?
            .into_iter()
            .map(|(raw, m)| {
                let at = parse_rfc3339(&raw).ok_or_else(|| {
                    XhlError::Internal(format!("timestamp metrics tidak valid: {raw}"))
                })?;
                Ok((at, m))
            })
            .collect()
    }

    // ---- jobs --------------------------------------------------------------

    pub fn enqueue_job(
        &self,
        account: &str,
        run_at: OffsetDateTime,
        payload: &str,
        dedup_key: Option<&str>,
    ) -> Result<String, XhlError> {
        let id = uuid::Uuid::new_v4().to_string();
        let now = now_rfc3339();
        self.conn
            .execute(
                "INSERT INTO jobs
                 (id, account, run_at, payload, state, attempts, dedup_key, created_at, updated_at)
                 VALUES (?1, ?2, ?3, ?4, 'Pending', 0, ?5, ?6, ?6)",
                params![id, account, format_rfc3339(run_at), payload, dedup_key, now],
            )
            .map_err(|e| XhlError::Internal(format!("enqueue job: {e}")))?;
        Ok(id)
    }

    pub fn due_jobs(&self, now: OffsetDateTime, limit: usize) -> Result<Vec<JobRow>, XhlError> {
        let mut stmt = self
            .conn
            .prepare(
                "SELECT id, run_at, payload, state, attempts FROM jobs
                 WHERE state = 'Pending' AND run_at <= ?1
                 ORDER BY run_at LIMIT ?2",
            )
            .map_err(|e| XhlError::Internal(e.to_string()))?;

        let rows = stmt
            .query_map(params![format_rfc3339(now), limit as i64], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, i64>(4)?,
                ))
            })
            .map_err(|e| XhlError::Internal(e.to_string()))?;

        rows.collect::<Result<Vec<_>, _>>()
            .map_err(|e| XhlError::Internal(e.to_string()))?
            .into_iter()
            .map(|(id, run_at, payload, state, attempts)| {
                Ok(JobRow {
                    id,
                    run_at: parse_rfc3339(&run_at).ok_or_else(|| {
                        XhlError::Internal(format!("run_at tidak valid: {run_at}"))
                    })?,
                    payload,
                    state,
                    attempts: attempts as u32,
                })
            })
            .collect()
    }

    /// Transisi `Pending` → `Running` secara atomik. `false` bila job sudah diklaim proses lain.
    pub fn claim_job(&self, id: &str) -> Result<bool, XhlError> {
        let affected = self
            .conn
            .execute(
                "UPDATE jobs SET state = 'Running', updated_at = ?2
                 WHERE id = ?1 AND state = 'Pending'",
                params![id, now_rfc3339()],
            )
            .map_err(|e| XhlError::Internal(format!("claim job: {e}")))?;
        Ok(affected > 0)
    }

    pub fn finish_job(&self, id: &str, marker: Option<&str>) -> Result<(), XhlError> {
        self.conn
            .execute(
                "UPDATE jobs SET state = 'Done', idempotency_marker = ?2, updated_at = ?3 WHERE id = ?1",
                params![id, marker, now_rfc3339()],
            )
            .map_err(|e| XhlError::Internal(format!("finish job: {e}")))?;
        Ok(())
    }

    pub fn fail_job(&self, id: &str, reason: &str, attempts: u32) -> Result<(), XhlError> {
        self.conn
            .execute(
                "UPDATE jobs SET state = 'Failed', payload = payload, attempts = ?3,
                 idempotency_marker = ?2, updated_at = ?4 WHERE id = ?1",
                params![id, reason, attempts as i64, now_rfc3339()],
            )
            .map_err(|e| XhlError::Internal(format!("fail job: {e}")))?;
        Ok(())
    }

    /// Kembalikan job ke `Pending` dengan jadwal baru (untuk retry).
    pub fn reschedule_job(
        &self,
        id: &str,
        run_at: OffsetDateTime,
        attempts: u32,
    ) -> Result<(), XhlError> {
        self.conn
            .execute(
                "UPDATE jobs SET state = 'Pending', run_at = ?2, attempts = ?3, updated_at = ?4
                 WHERE id = ?1",
                params![id, format_rfc3339(run_at), attempts as i64, now_rfc3339()],
            )
            .map_err(|e| XhlError::Internal(format!("reschedule job: {e}")))?;
        Ok(())
    }

    /// Job yang macet di `Running` (mis. proses mati) dikembalikan ke `Pending`.
    pub fn reset_stale_running(&self, older_than: OffsetDateTime) -> Result<usize, XhlError> {
        let affected = self
            .conn
            .execute(
                "UPDATE jobs SET state = 'Pending', updated_at = ?2
                 WHERE state = 'Running' AND updated_at < ?1",
                params![format_rfc3339(older_than), now_rfc3339()],
            )
            .map_err(|e| XhlError::Internal(format!("reset stale job: {e}")))?;
        Ok(affected)
    }

    /// Batalkan job yang belum selesai. `false` bila job tidak ada atau sudah final.
    pub fn cancel_job(&self, id: &str) -> Result<bool, XhlError> {
        let affected = self
            .conn
            .execute(
                "UPDATE jobs SET state = 'Cancelled', updated_at = ?2
                 WHERE id = ?1 AND state IN ('Pending', 'Running', 'Failed')",
                params![id, now_rfc3339()],
            )
            .map_err(|e| XhlError::Internal(format!("cancel job: {e}")))?;
        Ok(affected > 0)
    }

    pub fn list_jobs(&self, account: &str, limit: usize) -> Result<Vec<JobRow>, XhlError> {
        let mut stmt = self
            .conn
            .prepare(
                "SELECT id, run_at, payload, state, attempts FROM jobs
                 WHERE account = ?1 ORDER BY run_at DESC LIMIT ?2",
            )
            .map_err(|e| XhlError::Internal(e.to_string()))?;

        let rows = stmt
            .query_map(params![account, limit as i64], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, i64>(4)?,
                ))
            })
            .map_err(|e| XhlError::Internal(e.to_string()))?;

        rows.collect::<Result<Vec<_>, _>>()
            .map_err(|e| XhlError::Internal(e.to_string()))?
            .into_iter()
            .map(|(id, run_at, payload, state, attempts)| {
                Ok(JobRow {
                    id,
                    run_at: parse_rfc3339(&run_at).ok_or_else(|| {
                        XhlError::Internal(format!("run_at tidak valid: {run_at}"))
                    })?,
                    payload,
                    state,
                    attempts: attempts as u32,
                })
            })
            .collect()
    }

    /// Alasan kegagalan terakhir sebuah job (dari `idempotency_marker`).
    pub fn job_failure_reason(&self, id: &str) -> Result<Option<String>, XhlError> {
        self.conn
            .query_row(
                "SELECT idempotency_marker FROM jobs WHERE id = ?1 AND state = 'Failed'",
                params![id],
                |row| row.get(0),
            )
            .optional()
            .map_err(|e| XhlError::Internal(format!("baca alasan job: {e}")))
    }

    // ---- drafts ------------------------------------------------------------

    pub fn insert_draft(
        &self,
        id: &str,
        account: &str,
        body: &str,
        name: Option<&str>,
        origin: &str,
        media_json: &str,
    ) -> Result<(), XhlError> {
        self.conn
            .execute(
                "INSERT INTO drafts (id, account, body, name, origin, created_at, media)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                params![id, account, body, name, origin, now_rfc3339(), media_json],
            )
            .map_err(|e| XhlError::Internal(format!("simpan draft: {e}")))?;
        Ok(())
    }

    pub fn list_drafts(&self, account: &str, limit: usize) -> Result<Vec<DraftRow>, XhlError> {
        self.draft_query(
            "SELECT id, body, name, origin, created_at, media FROM drafts
             WHERE account = ?1 ORDER BY created_at DESC LIMIT ?2",
            params![account, limit as i64],
        )
    }

    pub fn get_draft(&self, id: &str) -> Result<Option<DraftRow>, XhlError> {
        let mut rows = self.draft_query(
            "SELECT id, body, name, origin, created_at, media FROM drafts WHERE id = ?1",
            params![id],
        )?;
        Ok(rows.pop())
    }

    fn draft_query(
        &self,
        sql: &str,
        params: impl rusqlite::Params,
    ) -> Result<Vec<DraftRow>, XhlError> {
        let mut stmt = self
            .conn
            .prepare(sql)
            .map_err(|e| XhlError::Internal(e.to_string()))?;
        let rows = stmt
            .query_map(params, |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, Option<String>>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, String>(4)?,
                    row.get::<_, String>(5)?,
                ))
            })
            .map_err(|e| XhlError::Internal(e.to_string()))?;

        rows.collect::<Result<Vec<_>, _>>()
            .map_err(|e| XhlError::Internal(e.to_string()))?
            .into_iter()
            .map(|(id, body, name, origin, created_at, media)| {
                Ok(DraftRow {
                    id,
                    body,
                    name,
                    origin,
                    created_at: parse_rfc3339(&created_at).ok_or_else(|| {
                        XhlError::Internal(format!("created_at draft tidak valid: {created_at}"))
                    })?,
                    media_json: media,
                })
            })
            .collect()
    }

    pub fn delete_draft(&self, id: &str) -> Result<bool, XhlError> {
        let affected = self
            .conn
            .execute("DELETE FROM drafts WHERE id = ?1", params![id])
            .map_err(|e| XhlError::Internal(format!("hapus draft: {e}")))?;
        Ok(affected > 0)
    }
}

/// Baris job seperti tersimpan di database.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JobRow {
    pub id: String,
    pub run_at: OffsetDateTime,
    pub payload: String,
    pub state: String,
    pub attempts: u32,
}

/// Baris draft seperti tersimpan di database.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DraftRow {
    pub id: String,
    pub body: String,
    pub name: Option<String>,
    pub origin: String,
    pub created_at: OffsetDateTime,
    pub media_json: String,
}

fn now_rfc3339() -> String {
    OffsetDateTime::now_utc()
        .format(&Rfc3339)
        .unwrap_or_else(|_| "1970-01-01T00:00:00Z".to_owned())
}

/// Format waktu ke RFC3339 untuk penyimpanan.
pub fn format_rfc3339(at: OffsetDateTime) -> String {
    at.format(&Rfc3339)
        .unwrap_or_else(|_| "1970-01-01T00:00:00Z".to_owned())
}

/// Parse RFC3339 dari database. `None` bila rusak (bukan panic).
pub fn parse_rfc3339(raw: &str) -> Option<OffsetDateTime> {
    OffsetDateTime::parse(raw, &Rfc3339).ok()
}

/// Pemandu async untuk [`Store`].
///
/// `rusqlite::Connection` bersifat `Send` tapi tidak `Sync`, dan operasi SQLite
/// adalah blocking. Wrapper ini menahan store di balik `Mutex` dan menjalankan
/// setiap operasi di `spawn_blocking`, sehingga jalur async tidak pernah terblokir.
#[derive(Clone)]
pub struct StoreHandle {
    inner: std::sync::Arc<std::sync::Mutex<Store>>,
}

impl StoreHandle {
    pub fn open(path: &Path) -> Result<Self, XhlError> {
        Ok(Self::new(Store::open(path)?))
    }

    pub fn in_memory() -> Result<Self, XhlError> {
        Ok(Self::new(Store::open_in_memory()?))
    }

    pub fn new(store: Store) -> Self {
        Self {
            inner: std::sync::Arc::new(std::sync::Mutex::new(store)),
        }
    }

    /// Jalankan operasi store di thread blocking.
    pub async fn run<T, F>(&self, op: F) -> Result<T, XhlError>
    where
        F: FnOnce(&Store) -> Result<T, XhlError> + Send + 'static,
        T: Send + 'static,
    {
        let inner = std::sync::Arc::clone(&self.inner);
        tokio::task::spawn_blocking(move || {
            let guard = inner
                .lock()
                .map_err(|_| XhlError::Internal("lock store teracuni".into()))?;
            op(&guard)
        })
        .await
        .map_err(|e| XhlError::Internal(format!("task store gagal: {e}")))?
    }
}

#[cfg(test)]
mod handle_tests {
    use super::*;

    #[tokio::test]
    async fn handle_menjalankan_operasi_blocking() {
        let handle = StoreHandle::in_memory().unwrap();
        let cookies = Cookies {
            auth_token: "A".into(),
            ct0: "C".into(),
            twid: None,
            extras: vec![],
        };
        handle
            .run(move |store| store.upsert_cookies("default", &cookies))
            .await
            .unwrap();

        let found = handle
            .run(|store| store.cookies("default"))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(found.auth_token, "A");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cookies() -> Cookies {
        Cookies {
            auth_token: "A".into(),
            ct0: "C".into(),
            twid: None,
            extras: vec![],
        }
    }

    #[test]
    fn migrasi_idempoten() {
        let mut store = Store::open_in_memory().unwrap();
        assert_eq!(store.user_version().unwrap(), MIGRATIONS.len() as i64);
        // Menjalankan migrasi ulang pada DB yang sudah termigrasi harus no-op.
        store.migrate().unwrap();
        assert_eq!(store.user_version().unwrap(), MIGRATIONS.len() as i64);
    }

    #[test]
    fn upsert_dan_baca_cookie() {
        let store = Store::open_in_memory().unwrap();
        store.upsert_cookies("default", &cookies()).unwrap();
        assert_eq!(store.cookies("default").unwrap(), Some(cookies()));

        let updated = Cookies {
            auth_token: "A2".into(),
            ..cookies()
        };
        store.upsert_cookies("default", &updated).unwrap();
        assert_eq!(store.cookies("default").unwrap().unwrap().auth_token, "A2");
    }

    #[test]
    fn akun_tidak_dikenal_mengembalikan_none() {
        let store = Store::open_in_memory().unwrap();
        assert!(store.cookies("hantu").unwrap().is_none());
    }

    #[test]
    fn identity_dan_event() {
        let store = Store::open_in_memory().unwrap();
        store.upsert_cookies("default", &cookies()).unwrap();
        store
            .update_identity("default", Some("contoh"), Some("42"))
            .unwrap();
        let list = store.list_accounts().unwrap();
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].id, AccountId("default".into()));
        assert_eq!(
            list[0].handle.as_ref().map(|h| h.0.as_str()),
            Some("contoh")
        );
        assert_eq!(list[0].user_id.as_deref(), Some("42"));

        store
            .record_event("default", "auth_import", "sumber: string")
            .unwrap();
        let events = store.recent_events("default", 5).unwrap();
        assert_eq!(events.len(), 1);
        assert!(events[0].contains("auth_import"));
    }

    #[test]
    fn update_identity_tidak_menghapus_nilai_lama() {
        let store = Store::open_in_memory().unwrap();
        store.upsert_cookies("default", &cookies()).unwrap();
        store
            .update_identity("default", Some("contoh"), Some("42"))
            .unwrap();
        store.update_identity("default", None, None).unwrap();
        let list = store.list_accounts().unwrap();
        assert_eq!(
            list[0].handle.as_ref().map(|h| h.0.as_str()),
            Some("contoh")
        );
        assert_eq!(list[0].user_id.as_deref(), Some("42"));
    }

    #[test]
    fn query_id_upsert_dan_baca() {
        let store = Store::open_in_memory().unwrap();
        store
            .set_query_id("SearchTimeline", "hash1", "test")
            .unwrap();
        store.set_query_id("TweetDetail", "hash2", "test").unwrap();
        let ids = store.query_ids().unwrap();
        assert_eq!(ids.len(), 2);
        assert_eq!(ids[0], ("SearchTimeline".to_string(), "hash1".to_string()));
        assert_eq!(ids[1], ("TweetDetail".to_string(), "hash2".to_string()));

        // Update harus menimpa
        store
            .set_query_id("SearchTimeline", "hash1_baru", "discovery")
            .unwrap();
        let ids2 = store.query_ids().unwrap();
        assert_eq!(ids2[0].1, "hash1_baru");
    }

    #[test]
    fn post_log_dedup() {
        let store = Store::open_in_memory().unwrap();
        assert_eq!(store.posted_tweet("dedup1").unwrap(), None);

        assert!(store.record_post("dedup1", "12345", "default").unwrap());
        assert_eq!(
            store.posted_tweet("dedup1").unwrap(),
            Some("12345".to_string())
        );

        // Record kedua dengan dedup_key yang sama mengembalikan false (diabaikan)
        assert!(!store.record_post("dedup1", "99999", "default").unwrap());
        // Tweet id tetap yang pertama
        assert_eq!(
            store.posted_tweet("dedup1").unwrap(),
            Some("12345".to_string())
        );
    }
}

#[cfg(test)]
impl Store {
    fn user_version(&self) -> Result<i64, XhlError> {
        self.conn
            .query_row("PRAGMA user_version", [], |row| row.get(0))
            .map_err(|e| XhlError::Internal(e.to_string()))
    }
}
