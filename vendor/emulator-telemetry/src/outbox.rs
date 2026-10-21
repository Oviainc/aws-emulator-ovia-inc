// Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
// SPDX-License-Identifier: Apache-2.0
//! Durable state: the SQLite outbox plus the installation id persisted next
//! to it.
//!
//! Batches survive process restarts here until they are acknowledged by the
//! ingest endpoint, expire (14 days — the backend's dedup window), or are
//! evicted by the size cap. Retries keep their `BatchId` so the backend can
//! deduplicate; only a 400-split re-keys.

use crate::policy::{
    BACKOFF_BASE, BACKOFF_CAP, BACKOFF_MAX_DOUBLINGS, INSTALLATION_ID_CLAIM_ATTEMPTS,
    INSTALLATION_ID_FILE, OUTBOX_FILE, OUTBOX_MAX_AGE, OUTBOX_MAX_BATCHES, SQLITE_BUSY_TIMEOUT,
    SQLITE_CRASH_BUSY_TIMEOUT, SQLITE_JOURNAL_MODE,
};
use crate::types::{BatchId, InstallationId, StateArtifact, StatePurgeError};
use crate::wire::BuiltBatch;
use rusqlite::{params, Connection, OptionalExtension};
use std::path::{Path, PathBuf};
use std::time::Duration;

/// A batch read back from the outbox, due for sending.
#[derive(Debug, Clone)]
pub struct PendingBatch {
    pub batch_id: BatchId,
    pub body: String,
    pub attempts: u32,
}

/// The durable telemetry store rooted in one state directory.
pub struct Outbox {
    conn: Connection,
    state_dir: PathBuf,
}

enum PersistedInstallationId {
    Absent,
    Malformed,
    Valid(InstallationId),
}

/// Delete the local telemetry state every effective-off state leaves behind:
/// the installation id (including any quarantine/publish siblings), the
/// outbox database with its WAL/SHM journals, and — when that empties it —
/// the state directory itself. Going off DELETES existing state rather than
/// merely ceasing to write it.
///
/// The client stays off regardless of the result; the first failure is
/// returned as a typed error (artifact kind + I/O error kind, no path) so the
/// caller can surface it in the notice instead of ignoring it. Deletion is
/// verified by re-listing the directory afterwards.
pub(crate) fn purge_state(state_dir: &Path) -> Result<(), StatePurgeError> {
    purge_artifacts(state_dir, |_| true)?;
    let _ = std::fs::remove_dir(state_dir);
    Ok(())
}

/// Delete only the outbox (database plus journals), keeping any persisted
/// installation id: an installation the server does not sample must never
/// queue records, but keeps the id its deterministic sampling decision is
/// derived from.
pub(crate) fn purge_outbox(state_dir: &Path) -> Result<(), StatePurgeError> {
    purge_artifacts(state_dir, |artifact| artifact == StateArtifact::Outbox)
}

/// Attempt every selected artifact — one undeletable file never shields the
/// others — then re-list to verify. The first failure is the one reported.
fn purge_artifacts(
    state_dir: &Path,
    selected: impl Fn(StateArtifact) -> bool,
) -> Result<(), StatePurgeError> {
    let mut first_failure = None;
    for (artifact, path) in state_artifacts(state_dir)?
        .into_iter()
        .filter(|(artifact, _)| selected(*artifact))
    {
        if let Err(error) = std::fs::remove_file(&path) {
            first_failure.get_or_insert(StatePurgeError::Remove {
                artifact,
                kind: error.kind(),
            });
        }
    }
    if let Some(failure) = first_failure {
        return Err(failure);
    }
    match state_artifacts(state_dir)?
        .into_iter()
        .find(|(artifact, _)| selected(*artifact))
    {
        Some((artifact, _)) => Err(StatePurgeError::Remains(artifact)),
        None => Ok(()),
    }
}

/// Every persisted telemetry artifact currently in `state_dir`. A missing
/// directory holds nothing; any other listing failure is reported.
fn state_artifacts(state_dir: &Path) -> Result<Vec<(StateArtifact, PathBuf)>, StatePurgeError> {
    let entries = match std::fs::read_dir(state_dir) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(StatePurgeError::List(error.kind())),
    };
    let mut found = Vec::new();
    for entry in entries {
        let entry = entry.map_err(|error| StatePurgeError::List(error.kind()))?;
        let name = entry.file_name();
        let name = name.to_string_lossy();
        let artifact = if name.starts_with(INSTALLATION_ID_FILE) {
            StateArtifact::InstallationId
        } else if name.starts_with(OUTBOX_FILE) {
            StateArtifact::Outbox
        } else {
            continue;
        };
        found.push((artifact, entry.path()));
    }
    found.sort();
    Ok(found)
}

impl Outbox {
    /// Open (creating if needed) the outbox in `state_dir`.
    pub fn open(state_dir: &Path) -> Result<Self, rusqlite::Error> {
        Self::open_with_busy_timeout(state_dir, SQLITE_BUSY_TIMEOUT)
    }

    pub(crate) fn open_for_crash(state_dir: &Path) -> Result<Self, rusqlite::Error> {
        Self::open_with_busy_timeout(state_dir, SQLITE_CRASH_BUSY_TIMEOUT)
    }

    fn open_with_busy_timeout(
        state_dir: &Path,
        busy_timeout: Duration,
    ) -> Result<Self, rusqlite::Error> {
        std::fs::create_dir_all(state_dir).map_err(|e| {
            rusqlite::Error::SqliteFailure(
                rusqlite::ffi::Error::new(rusqlite::ffi::SQLITE_CANTOPEN),
                Some(format!("create state dir {}: {e}", state_dir.display())),
            )
        })?;
        let conn = Connection::open(state_dir.join(OUTBOX_FILE))?;
        conn.busy_timeout(busy_timeout)?;
        conn.pragma_update(None, "journal_mode", SQLITE_JOURNAL_MODE)?;
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS outbox (
                 batch_id TEXT PRIMARY KEY,
                 body TEXT NOT NULL,
                 created_at INTEGER NOT NULL,
                 next_attempt_at INTEGER NOT NULL,
                 attempts INTEGER NOT NULL DEFAULT 0
             );
             CREATE TABLE IF NOT EXISTS kv (
                 k TEXT PRIMARY KEY,
                 v TEXT NOT NULL
             );",
        )?;
        Ok(Self {
            conn,
            state_dir: state_dir.to_path_buf(),
        })
    }

    /// Read the installation id persisted next to the outbox without touching
    /// the filesystem: `None` when there is none yet or the file is malformed
    /// (malformed files are only recovered by [`Self::claim_installation_id`]).
    pub fn load_installation_id(&self) -> Result<Option<InstallationId>, std::io::Error> {
        match self.read_installation_id()? {
            PersistedInstallationId::Valid(id) => Ok(Some(id)),
            PersistedInstallationId::Absent | PersistedInstallationId::Malformed => Ok(None),
        }
    }

    /// Persist `candidate` as the installation id unless one is already
    /// persisted, returning the id now on disk and whether this call wrote it
    /// (`firstRun`).
    ///
    /// The write is atomic across processes: the candidate is fully written
    /// to a unique temporary file and then PUBLISHED via `hard_link`, which
    /// fails when the target already exists (readers can never observe a
    /// partially written id). Concurrent claimers therefore converge on one
    /// persisted id, with exactly one observing `firstRun`.
    ///
    /// Malformed-file recovery never unlinks in place: the malformed file is
    /// first MOVED to a process-owned quarantine path, and if the quarantined
    /// content turns out to be a valid id from a concurrent recovery that won
    /// in between, that id is re-published and adopted instead of destroyed.
    pub fn claim_installation_id(
        &self,
        candidate: &InstallationId,
    ) -> Result<(InstallationId, bool), std::io::Error> {
        let path = self.state_dir.join(INSTALLATION_ID_FILE);
        for _ in 0..INSTALLATION_ID_CLAIM_ATTEMPTS {
            match self.read_installation_id()? {
                PersistedInstallationId::Valid(existing) => return Ok((existing, false)),
                PersistedInstallationId::Malformed => {
                    if let Some(stolen) = self.quarantine_malformed_installation_id(&path)? {
                        return Ok((stolen, false));
                    }
                }
                PersistedInstallationId::Absent => {}
            }
            if self.publish_installation_id(&path, candidate.as_str())? {
                return Ok((candidate.clone(), true));
            }
        }
        Err(std::io::Error::other(
            "installation id claim contention exhausted its bounded retries",
        ))
    }

    fn read_installation_id(&self) -> Result<PersistedInstallationId, std::io::Error> {
        match std::fs::read_to_string(self.state_dir.join(INSTALLATION_ID_FILE)) {
            Ok(existing) => Ok(InstallationId::new(existing.trim()).map_or(
                PersistedInstallationId::Malformed,
                PersistedInstallationId::Valid,
            )),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                Ok(PersistedInstallationId::Absent)
            }
            Err(error) => Err(error),
        }
    }

    /// Move a malformed installation-id file aside atomically. Between our
    /// read and the rename a concurrent recovery may have already published a
    /// fresh valid id at the same path; renaming would steal it, so the
    /// quarantined content is re-checked and a stolen valid id is
    /// re-published (or, if yet another process published meanwhile, the
    /// published one is adopted). Returns the adopted id when one was stolen.
    fn quarantine_malformed_installation_id(
        &self,
        path: &Path,
    ) -> Result<Option<InstallationId>, std::io::Error> {
        let quarantine = self.state_dir.join(format!(
            "{INSTALLATION_ID_FILE}.corrupt.{}",
            uuid::Uuid::new_v4()
        ));
        match std::fs::rename(path, &quarantine) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(error),
        }
        let stolen = std::fs::read_to_string(&quarantine)
            .ok()
            .and_then(|content| InstallationId::new(content.trim()).ok());
        let _ = std::fs::remove_file(&quarantine);
        let Some(stolen) = stolen else {
            return Ok(None);
        };
        if self.publish_installation_id(path, stolen.as_str())? {
            return Ok(Some(stolen));
        }
        match std::fs::read_to_string(path) {
            Ok(published) => Ok(InstallationId::new(published.trim()).ok()),
            Err(_) => Ok(None),
        }
    }

    fn publish_installation_id(&self, path: &Path, value: &str) -> Result<bool, std::io::Error> {
        let tmp = self.state_dir.join(format!(
            "{INSTALLATION_ID_FILE}.{}.tmp",
            uuid::Uuid::new_v4()
        ));
        std::fs::write(&tmp, format!("{value}\n"))?;
        let claim = std::fs::hard_link(&tmp, path);
        let _ = std::fs::remove_file(&tmp);
        match claim {
            Ok(()) => Ok(true),
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => Ok(false),
            Err(error) => Err(error),
        }
    }

    /// Read a kv-store value.
    pub fn kv_get(&self, key: &str) -> Option<String> {
        self.conn
            .query_row("SELECT v FROM kv WHERE k = ?1", params![key], |row| {
                row.get(0)
            })
            .optional()
            .ok()
            .flatten()
    }

    /// Write a kv-store value.
    pub fn kv_put(&self, key: &str, value: &str) -> Result<(), rusqlite::Error> {
        self.conn.execute(
            "INSERT INTO kv (k, v) VALUES (?1, ?2)
             ON CONFLICT(k) DO UPDATE SET v = excluded.v",
            params![key, value],
        )?;
        Ok(())
    }

    /// Enqueue a built batch for delivery, then enforce the age and size
    /// bounds.
    pub fn enqueue(
        &self,
        batch: &BuiltBatch,
        now_epoch_seconds: u64,
    ) -> Result<(), rusqlite::Error> {
        self.conn.execute(
            "INSERT OR REPLACE INTO outbox
                 (batch_id, body, created_at, next_attempt_at, attempts)
             VALUES (?1, ?2, ?3, ?3, 0)",
            params![
                batch.batch_id.as_str(),
                batch.body,
                epoch_i64(now_epoch_seconds)
            ],
        )?;
        self.enforce_bounds(now_epoch_seconds)
    }

    /// Batches whose next attempt is due, oldest first.
    pub fn due_batches(
        &self,
        now_epoch_seconds: u64,
        limit: usize,
    ) -> Result<Vec<PendingBatch>, rusqlite::Error> {
        let mut stmt = self.conn.prepare(
            "SELECT batch_id, body, attempts FROM outbox
             WHERE next_attempt_at <= ?1
             ORDER BY created_at ASC, batch_id ASC
             LIMIT ?2",
        )?;
        let rows = stmt.query_map(params![epoch_i64(now_epoch_seconds), limit as i64], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, i64>(2)?,
            ))
        })?;
        let mut out = Vec::new();
        let mut invalid_ids = Vec::new();
        for row in rows {
            let (id, body, attempts) = row?;
            if let Ok(batch_id) = BatchId::new(id.clone()) {
                out.push(PendingBatch {
                    batch_id,
                    body,
                    attempts: attempts.max(0) as u32,
                });
            } else {
                invalid_ids.push(id);
            }
        }
        drop(stmt);
        for invalid_id in invalid_ids {
            self.conn.execute(
                "DELETE FROM outbox WHERE batch_id = ?1",
                params![invalid_id],
            )?;
        }
        Ok(out)
    }

    /// The state directory this outbox is rooted in.
    pub(crate) fn state_dir(&self) -> &Path {
        &self.state_dir
    }

    /// Number of pending batches (test observability).
    #[cfg(test)]
    pub fn pending_count(&self) -> Result<u64, rusqlite::Error> {
        self.conn
            .query_row("SELECT COUNT(*) FROM outbox", [], |row| {
                row.get::<_, i64>(0)
            })
            .map(|n| n.max(0) as u64)
    }

    /// Remove a batch after the ingest endpoint acknowledged it (or it was
    /// classified as undeliverable).
    pub fn delete(&self, batch_id: &BatchId) -> Result<(), rusqlite::Error> {
        self.conn.execute(
            "DELETE FROM outbox WHERE batch_id = ?1",
            params![batch_id.as_str()],
        )?;
        Ok(())
    }

    /// Record a delivery failure: bump the attempt count and schedule the next
    /// attempt after `delay` (normally [`backoff_delay`]; tests inject zero).
    pub fn record_failure(
        &self,
        batch_id: &BatchId,
        now_epoch_seconds: u64,
        delay: Duration,
    ) -> Result<(), rusqlite::Error> {
        self.conn.execute(
            "UPDATE outbox
             SET attempts = attempts + 1, next_attempt_at = ?2
             WHERE batch_id = ?1",
            params![
                batch_id.as_str(),
                epoch_i64(now_epoch_seconds.saturating_add(delay.as_secs())),
            ],
        )?;
        Ok(())
    }

    /// Replace one batch with its two split halves atomically (the backend's
    /// 400-split contract). The halves are due immediately.
    pub fn replace_with_split(
        &self,
        original: &BatchId,
        halves: &[BuiltBatch; 2],
        now_epoch_seconds: u64,
    ) -> Result<(), rusqlite::Error> {
        self.conn.execute_batch("BEGIN")?;
        let result = (|| {
            let created_at = self.conn.query_row(
                "SELECT created_at FROM outbox WHERE batch_id = ?1",
                params![original.as_str()],
                |row| row.get::<_, i64>(0),
            )?;
            self.conn.execute(
                "DELETE FROM outbox WHERE batch_id = ?1",
                params![original.as_str()],
            )?;
            for half in halves {
                self.conn.execute(
                    "INSERT OR REPLACE INTO outbox
                         (batch_id, body, created_at, next_attempt_at, attempts)
                     VALUES (?1, ?2, ?3, ?4, 0)",
                    params![
                        half.batch_id.as_str(),
                        half.body,
                        created_at,
                        epoch_i64(now_epoch_seconds)
                    ],
                )?;
            }
            Ok(())
        })();
        match result {
            Ok(()) => self.conn.execute_batch("COMMIT"),
            Err(e) => {
                let _ = self.conn.execute_batch("ROLLBACK");
                Err(e)
            }
        }
    }

    /// Drop expired batches and evict the oldest beyond the size cap.
    pub fn enforce_bounds(&self, now_epoch_seconds: u64) -> Result<(), rusqlite::Error> {
        let cutoff = now_epoch_seconds.saturating_sub(OUTBOX_MAX_AGE.as_secs());
        self.conn.execute(
            "DELETE FROM outbox WHERE created_at < ?1",
            params![epoch_i64(cutoff)],
        )?;
        self.conn.execute(
            "DELETE FROM outbox WHERE batch_id NOT IN (
                 SELECT batch_id FROM outbox
                 ORDER BY created_at DESC, batch_id DESC
                 LIMIT ?1
             )",
            params![OUTBOX_MAX_BATCHES as i64],
        )?;
        Ok(())
    }
}

/// Clamp epoch seconds into SQLite's signed integer domain.
fn epoch_i64(seconds: u64) -> i64 {
    seconds.min(i64::MAX as u64) as i64
}

/// Capped exponential backoff with deterministic ±25% jitter keyed off the
/// batch id and attempt number.
pub(crate) fn backoff_delay(batch_id: &BatchId, attempts_so_far: u32) -> Duration {
    let exp = attempts_so_far.min(BACKOFF_MAX_DOUBLINGS);
    let base = BACKOFF_BASE.as_secs().saturating_mul(1u64 << exp);
    let capped = base.min(BACKOFF_CAP.as_secs());
    let hash = fnv1a(batch_id.as_str().as_bytes()) ^ u64::from(attempts_so_far);
    let jitter_range = capped / 2;
    let jitter = if jitter_range == 0 {
        0
    } else {
        hash % (jitter_range + 1)
    };
    Duration::from_secs(capped - (jitter_range / 2) + jitter)
}

/// FNV-1a 64-bit: a stable, dependency-free hash (std's `DefaultHasher` is
/// explicitly not stable across releases, and this hash feeds persisted /
/// cross-run decisions).
pub(crate) fn fnv1a(bytes: &[u8]) -> u64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in bytes {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
}

#[cfg(test)]
mod tests {
    use super::*;

    fn open_temp() -> (tempfile::TempDir, Outbox) {
        let dir = tempfile::tempdir().unwrap();
        let outbox = Outbox::open(dir.path()).unwrap();
        (dir, outbox)
    }

    fn batch(id: &str, body: &str) -> BuiltBatch {
        BuiltBatch {
            batch_id: BatchId::new(id).unwrap(),
            body: body.to_string(),
        }
    }

    /// A deterministic, valid UUID batch id for fixtures; `n` keeps ids
    /// distinct and readable in assertions.
    fn fixture_id(n: u64) -> String {
        format!("00000000-0000-4000-8000-{n:012}")
    }

    #[test]
    fn outbox_uses_wal_for_concurrent_worker_and_crash_connections() {
        let (_dir, outbox) = open_temp();
        let journal_mode: String = outbox
            .conn
            .query_row("PRAGMA journal_mode", [], |row| row.get(0))
            .unwrap();
        assert_eq!(journal_mode.to_ascii_uppercase(), SQLITE_JOURNAL_MODE);
    }

    #[test]
    fn crash_path_never_waits_for_the_worker_writer_lock() {
        let (dir, worker) = open_temp();
        worker.conn.execute_batch("BEGIN IMMEDIATE").unwrap();

        let started = std::time::Instant::now();
        let result = Outbox::open_for_crash(dir.path())
            .and_then(|crash| crash.enqueue(&batch(&fixture_id(999), "session"), 1000));
        let elapsed = started.elapsed();

        assert!(
            result.is_err(),
            "a held writer lock refuses the best-effort write"
        );
        assert!(
            elapsed < Duration::from_millis(250),
            "the panic path must return immediately, elapsed {elapsed:?}"
        );
        worker.conn.execute_batch("ROLLBACK").unwrap();
    }

    #[test]
    fn loading_never_creates_an_installation_id() {
        let (dir, outbox) = open_temp();
        assert_eq!(outbox.load_installation_id().unwrap(), None);
        assert!(!dir.path().join(INSTALLATION_ID_FILE).exists());
    }

    #[test]
    fn installation_id_is_claimed_once_and_survives_reopen() {
        let (dir, outbox) = open_temp();
        let candidate = InstallationId::random();
        let (id, first_run) = outbox.claim_installation_id(&candidate).unwrap();
        assert!(first_run);
        assert_eq!(id, candidate);

        drop(outbox);
        let reopened = Outbox::open(dir.path()).unwrap();
        assert_eq!(reopened.load_installation_id().unwrap(), Some(id.clone()));
        let (again, first_run) = reopened
            .claim_installation_id(&InstallationId::random())
            .unwrap();
        assert!(!first_run);
        assert_eq!(id, again);
    }

    #[test]
    fn concurrent_installation_id_creation_converges_on_one_id() {
        let dir = tempfile::tempdir().unwrap();
        let barrier = std::sync::Arc::new(std::sync::Barrier::new(4));
        let mut handles = Vec::new();
        for _ in 0..4 {
            let outbox = Outbox::open(dir.path()).unwrap();
            let barrier = std::sync::Arc::clone(&barrier);
            handles.push(std::thread::spawn(move || {
                barrier.wait();
                outbox
                    .claim_installation_id(&InstallationId::random())
                    .unwrap()
            }));
        }
        let results: Vec<(InstallationId, bool)> =
            handles.into_iter().map(|h| h.join().unwrap()).collect();
        let first_id = results[0].0.clone();
        assert!(
            results.iter().all(|(id, _)| *id == first_id),
            "every concurrent starter must converge on one persisted id: {results:?}"
        );
        assert_eq!(
            results.iter().filter(|(_, first_run)| *first_run).count(),
            1,
            "exactly one starter observes firstRun: {results:?}"
        );
    }

    #[test]
    fn malformed_persisted_installation_id_is_replaced_with_a_uuid() {
        let (dir, outbox) = open_temp();
        std::fs::write(dir.path().join(INSTALLATION_ID_FILE), "not-a-uuid\n").unwrap();
        assert_eq!(outbox.load_installation_id().unwrap(), None);
        assert_eq!(
            std::fs::read_to_string(dir.path().join(INSTALLATION_ID_FILE)).unwrap(),
            "not-a-uuid\n",
            "loading must leave a malformed file untouched"
        );
        let candidate = InstallationId::random();
        let (id, first_run) = outbox.claim_installation_id(&candidate).unwrap();
        assert!(first_run);
        assert_eq!(id, candidate);
    }

    #[test]
    fn concurrent_malformed_installation_id_recovery_converges_on_one_id() {
        for _ in 0..8 {
            let dir = tempfile::tempdir().unwrap();
            std::fs::write(dir.path().join(INSTALLATION_ID_FILE), "not-a-uuid\n").unwrap();
            let barrier = std::sync::Arc::new(std::sync::Barrier::new(2));
            let mut handles = Vec::new();
            for _ in 0..2 {
                let outbox = Outbox::open(dir.path()).unwrap();
                let barrier = std::sync::Arc::clone(&barrier);
                handles.push(std::thread::spawn(move || {
                    barrier.wait();
                    outbox
                        .claim_installation_id(&InstallationId::random())
                        .unwrap()
                }));
            }
            let results: Vec<(InstallationId, bool)> =
                handles.into_iter().map(|h| h.join().unwrap()).collect();
            let first_id = results[0].0.clone();
            assert!(
                results.iter().all(|(id, _)| *id == first_id),
                "both recovering starters must converge on one id: {results:?}"
            );
            assert_eq!(
                results.iter().filter(|(_, first_run)| *first_run).count(),
                1,
                "exactly one recoverer observes firstRun: {results:?}"
            );
            let persisted = std::fs::read_to_string(dir.path().join(INSTALLATION_ID_FILE)).unwrap();
            assert_eq!(
                InstallationId::new(persisted.trim()).unwrap(),
                first_id,
                "the persisted file must end valid and match the adopted id"
            );
        }
    }

    #[test]
    fn enqueued_batches_survive_reopen_and_ack_deletes() {
        let (dir, outbox) = open_temp();
        outbox
            .enqueue(&batch(&fixture_id(1), "line1\nline2"), 1000)
            .unwrap();
        outbox
            .enqueue(&batch(&fixture_id(2), "line1"), 1001)
            .unwrap();
        drop(outbox);

        let reopened = Outbox::open(dir.path()).unwrap();
        let due = reopened.due_batches(2000, 10).unwrap();
        assert_eq!(due.len(), 2);
        assert_eq!(due[0].batch_id.as_str(), fixture_id(1), "oldest first");
        assert_eq!(due[0].body, "line1\nline2");

        reopened.delete(&due[0].batch_id).unwrap();
        assert_eq!(reopened.pending_count().unwrap(), 1);
    }

    #[test]
    fn failures_back_off_and_keep_the_batch_id() {
        let (_dir, outbox) = open_temp();
        outbox.enqueue(&batch(&fixture_id(1), "x"), 1000).unwrap();
        let id = BatchId::new(fixture_id(1)).unwrap();
        outbox
            .record_failure(&id, 1000, backoff_delay(&id, 0))
            .unwrap();
        assert!(
            outbox.due_batches(1000, 10).unwrap().is_empty(),
            "failed batch is not immediately due"
        );
        let far_future = 1000 + BACKOFF_CAP.as_secs() * 2;
        let due = outbox.due_batches(far_future, 10).unwrap();
        assert_eq!(due.len(), 1);
        assert_eq!(due[0].batch_id, id, "retry keeps the id");
        assert_eq!(due[0].attempts, 1);
    }

    #[test]
    fn backoff_grows_and_is_capped() {
        let id = BatchId::new(fixture_id(77)).unwrap();
        let d0 = backoff_delay(&id, 0);
        let d3 = backoff_delay(&id, 3);
        let d20 = backoff_delay(&id, 20);
        assert!(d0 >= BACKOFF_BASE / 2, "d0={d0:?}");
        assert!(d3 > d0, "backoff grows: {d0:?} -> {d3:?}");
        assert!(
            d20 <= BACKOFF_CAP + BACKOFF_CAP / 2,
            "capped with jitter: {d20:?}"
        );
    }

    #[test]
    fn expired_batches_are_purged_after_the_dedup_window() {
        let (_dir, outbox) = open_temp();
        let created = 1_000_000;
        outbox
            .enqueue(&batch(&fixture_id(10), "x"), created)
            .unwrap();
        let just_before_expiry = created + OUTBOX_MAX_AGE.as_secs() - 1;
        outbox.enforce_bounds(just_before_expiry).unwrap();
        assert_eq!(outbox.pending_count().unwrap(), 1);
        outbox
            .enforce_bounds(created + OUTBOX_MAX_AGE.as_secs() + 1)
            .unwrap();
        assert_eq!(outbox.pending_count().unwrap(), 0);
    }

    #[test]
    fn the_size_cap_evicts_oldest_first() {
        let (_dir, outbox) = open_temp();
        for i in 0..(OUTBOX_MAX_BATCHES + 10) {
            outbox
                .enqueue(&batch(&fixture_id(i as u64), "x"), 1000 + i as u64)
                .unwrap();
        }
        assert_eq!(outbox.pending_count().unwrap(), OUTBOX_MAX_BATCHES as u64);
        let due = outbox
            .due_batches(u64::MAX, OUTBOX_MAX_BATCHES + 10)
            .unwrap();
        let evicted: Vec<String> = (0..10).map(|i| fixture_id(i as u64)).collect();
        assert!(
            due.iter()
                .all(|b| !evicted.iter().any(|old| old == b.batch_id.as_str())),
            "the oldest ten batches were evicted"
        );
    }

    #[test]
    fn split_replaces_the_original_atomically() {
        let (_dir, outbox) = open_temp();
        outbox
            .enqueue(&batch(&fixture_id(100), "s\nm1\nm2"), 1000)
            .unwrap();
        let halves = [
            batch(&fixture_id(101), "s\nm1"),
            batch(&fixture_id(102), "s\nm2"),
        ];
        outbox
            .replace_with_split(&BatchId::new(fixture_id(100)).unwrap(), &halves, 1001)
            .unwrap();
        let due = outbox.due_batches(1001, 10).unwrap();
        let ids: Vec<String> = due
            .iter()
            .map(|b| b.batch_id.as_str().to_string())
            .collect();
        assert_eq!(ids, vec![fixture_id(101), fixture_id(102)]);
        outbox
            .enforce_bounds(1000 + OUTBOX_MAX_AGE.as_secs() + 1)
            .unwrap();
        assert_eq!(
            outbox.pending_count().unwrap(),
            0,
            "splitting preserves the original retention deadline"
        );
    }

    #[test]
    fn malformed_batch_ids_are_purged_when_read() {
        let (_dir, outbox) = open_temp();
        outbox
            .conn
            .execute(
                "INSERT INTO outbox
                     (batch_id, body, created_at, next_attempt_at, attempts)
                 VALUES (?1, ?2, ?3, ?3, 0)",
                params!["not/a/batch/id", "body", 1000_i64],
            )
            .unwrap();
        assert!(outbox.due_batches(1000, 10).unwrap().is_empty());
        assert_eq!(outbox.pending_count().unwrap(), 0);
    }

    #[test]
    fn kv_roundtrips_and_overwrites() {
        let (_dir, outbox) = open_temp();
        assert_eq!(outbox.kv_get("etag"), None);
        outbox.kv_put("etag", "\"abc\"").unwrap();
        outbox.kv_put("etag", "\"def\"").unwrap();
        assert_eq!(outbox.kv_get("etag").as_deref(), Some("\"def\""));
    }

    #[test]
    fn fnv1a_is_stable() {
        assert_eq!(fnv1a(b""), 0xcbf2_9ce4_8422_2325);
        assert_eq!(fnv1a(b"a"), 0xaf63_dc4c_8601_ec8c);
    }
}
