// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-mailmanager-mailmanagersvc service.
//!
//! Timestamps are stored as epoch MILLISECONDS (`i64`) and rendered into the modelled
//! `Timestamp` members with [`::aws_smithy_types::DateTime::from_millis`] on the way
//! out. Keeping an integer in the store (rather than a `DateTime`) is what lets
//! [`bump_millis`] guarantee that an update is *observably* later than the create that
//! preceded it even when both land inside one clock tick.

use std::collections::BTreeMap;
use std::time::{SystemTime, UNIX_EPOCH};

use crate::ops::types::{ArchiveRetention, Tag};

/// The two `ArchiveState` values this service's model declares; see
/// `crate::ops::types::ARCHIVE_STATE_VALUES`, which is generated from the model's
/// `ArchiveState` enum.
pub const ARCHIVE_STATE_ACTIVE: &str = "ACTIVE";
/// See [`ARCHIVE_STATE_ACTIVE`]. The model documents it as "The archive has been marked
/// for deletion and will be permanently deleted in 30 days. No further modifications can
/// be made in this state."
pub const ARCHIVE_STATE_PENDING_DELETION: &str = "PENDING_DELETION";

/// How long an archive stays in `PENDING_DELETION` before it is permanently removed.
///
/// This duration is DOCUMENTED, not chosen: the model's `DeleteArchive` documentation
/// states "The archive and all of its contents will be permanently deleted 30 days after
/// entering the pending deletion state, regardless of the configured retention period",
/// and `Archive$ArchiveState` repeats "will be permanently deleted in 30 days".
pub const PENDING_DELETION_WINDOW_MILLIS: i64 = 30 * 24 * 60 * 60 * 1000;

/// The `RetentionPeriod` recorded for an archive created without a `Retention`.
///
/// The model declares no `@default` for `CreateArchiveRequest$Retention` yet makes
/// `GetArchiveResponse$Retention` **required**, so a record must carry one. `PERMANENT`
/// is the one `RetentionPeriod` value (see `crate::ops::types::RETENTION_PERIOD_VALUES`)
/// under which no mail is ever automatically deleted, so it is the only default that
/// cannot discard data the caller never asked to have discarded.
pub const DEFAULT_RETENTION_PERIOD: &str = "PERMANENT";

/// The region an ARN carries when the request names none; `::engine::emulator_arn`
/// documents `us-east-1` as the emulator default.
pub const DEFAULT_REGION: &str = "us-east-1";

/// The most archives this emulator keeps, i.e. its per-account archive quota.
///
/// `CreateArchive` is the only operation that grows this service's store, and it models
/// `ServiceQuotaExceededException` ("Occurs when an operation exceeds a predefined service
/// quota or limit") precisely because the real service caps the resource. The model does
/// not state the number, so this value is CHOSEN: it is large enough that ordinary use
/// never reaches it and small enough that a caller cannot grow the process's memory
/// without limit. It bounds the whole store, not just `archives`: `archive_names` holds
/// one entry per live archive, and `idempotency` is only written on a create that
/// succeeded, so neither can exceed this count, and `purge_permanently_deleted`'s walk is
/// bounded by it too.
pub const MAX_ARCHIVES: usize = 100;

/// One stored email archive. The single source of truth for every archive operation:
/// `CreateArchive` writes it, `GetArchive`/`ListArchives` read it back, and
/// `UpdateArchive`/`DeleteArchive` mutate it in place.
#[derive(Debug, Clone)]
pub struct ArchiveRecord {
    pub archive_id: String,
    pub archive_name: String,
    pub archive_arn: String,
    /// `ARCHIVE_STATE_ACTIVE` or `ARCHIVE_STATE_PENDING_DELETION`.
    pub archive_state: String,
    pub retention: ArchiveRetention,
    pub kms_key_arn: Option<String>,
    pub created_at_millis: i64,
    pub last_updated_at_millis: i64,
    pub tags: Vec<Tag>,
    /// When the archive becomes permanently deleted, set by `DeleteArchive` to
    /// [`PENDING_DELETION_WINDOW_MILLIS`] after the call. `None` while the archive is
    /// `ACTIVE`.
    pub pending_deletion_at_millis: Option<i64>,
}

/// Per-service state for com-amazonaws-mailmanager-mailmanagersvc. A handler reaches it through [`service_state`].
///
/// Every map is a `BTreeMap`, so `ListArchives` walks the archives in one stable,
/// deterministic order and a pagination token can name the next key to resume from.
///
/// BOUNDED: `CreateArchive` refuses a create that would take `archives` past
/// [`MAX_ARCHIVES`], and the other two maps carry at most one entry per live archive (see
/// [`MAX_ARCHIVES`]), so no caller can grow this store without limit.
#[derive(Debug, Default)]
pub struct ComAmazonawsMailmanagerMailmanagersvcState {
    /// Archives by `ArchiveId`.
    pub archives: BTreeMap<String, ArchiveRecord>,
    /// `ArchiveName` -> `ArchiveId`; the index behind the name-uniqueness
    /// `ConflictException`.
    pub archive_names: BTreeMap<String, String>,
    /// `ClientToken` -> the create it completed; what makes a retried `CreateArchive`
    /// return the id the first call minted instead of creating a second archive.
    pub idempotency: BTreeMap<String, IdempotentCreate>,
    /// Monotonic counter feeding [`ComAmazonawsMailmanagerMailmanagersvcState::mint_archive_id`];
    /// it is what makes a minted id unique even within one clock tick.
    pub archive_sequence: u64,
}

impl ComAmazonawsMailmanagerMailmanagersvcState {
    /// Materialise the removals the clock already owes: drop every archive whose
    /// pending-deletion window has elapsed, along with its name and idempotency index
    /// entries.
    ///
    /// Called by the reporting operations too. That is the one write a read is allowed:
    /// the archive was due to be gone at this instant whether or not anybody called, so
    /// applying it here cannot change what a later caller would otherwise have seen.
    pub fn purge_permanently_deleted(&mut self, now_millis: i64) {
        let expired: Vec<String> = self
            .archives
            .values()
            .filter(|record| {
                record
                    .pending_deletion_at_millis
                    .is_some_and(|due| due <= now_millis)
            })
            .map(|record| record.archive_id.clone())
            .collect();
        if expired.is_empty() {
            return;
        }
        for id in &expired {
            if let Some(record) = self.archives.remove(id) {
                // Only drop the name index entry if it still points at THIS archive: a
                // later archive may legitimately have taken the freed name.
                if self.archive_names.get(&record.archive_name) == Some(&record.archive_id) {
                    self.archive_names.remove(&record.archive_name);
                }
            }
        }
        let live: std::collections::BTreeSet<String> = self.archives.keys().cloned().collect();
        self.idempotency
            .retain(|_, create| live.contains(&create.archive_id));
    }

    /// Mint a fresh `ArchiveId`: 32 lowercase hex characters, well inside
    /// `ArchiveIdString`'s modelled 1..66 length.
    ///
    /// The low half is a bijection of the per-service sequence number, so two archives
    /// can never share an id; the high half mixes in the creation instant so ids are not
    /// consecutive and a caller cannot derive the next one from the last.
    pub fn mint_archive_id(&mut self, now_millis: i64) -> String {
        self.archive_sequence = self.archive_sequence.wrapping_add(1);
        let sequence = self.archive_sequence;
        let high = splitmix64((now_millis as u64) ^ sequence.rotate_left(32));
        let low = splitmix64(sequence);
        format!("{high:016x}{low:016x}")
    }
}

/// The message the archive handlers report when the shared store cannot be reached.
///
/// Each handler still builds its own modelled error (the generated error enums are
/// per-operation), so this is the one part of that answer they can share.
pub const STORE_UNAVAILABLE: &str = "archive store is unavailable; retry the request";

/// What one `ClientToken` recorded: which archive the first call created, and a
/// description of the request it created it from.
#[derive(Debug, Clone)]
pub struct IdempotentCreate {
    pub archive_id: String,
    pub fingerprint: String,
}

/// A stable description of the members an archive is created from.
///
/// An `@idempotencyToken` identifies a RETRY of one request, so a token presented with a
/// different request is not a retry: comparing this description is what lets
/// `CreateArchive` tell the two apart instead of answering the second with the first
/// one's id. `Tags` are part of it too, because the record stores them.
pub fn create_archive_fingerprint(
    archive_name: &str,
    retention: Option<&ArchiveRetention>,
    kms_key_arn: Option<&str>,
    tags: Option<&[Tag]>,
) -> String {
    let retention = match retention {
        Some(ArchiveRetention::RetentionPeriod(period)) => period.as_str(),
        None => "",
    };
    // `\u{1f}` (unit separator) and `\u{1e}` (record separator) cannot appear in any of
    // these members, so the joined description is unambiguous. Tags are sorted: the same
    // set in a different order is the same request.
    let mut rendered_tags: Vec<String> = tags
        .unwrap_or_default()
        .iter()
        .map(|tag| format!("{}\u{1f}{}", tag.key, tag.value))
        .collect();
    rendered_tags.sort();
    format!(
        "{archive_name}\u{1e}{retention}\u{1e}{}\u{1e}{}",
        kms_key_arn.unwrap_or_default(),
        rendered_tags.join("\u{1e}")
    )
}

/// SplitMix64, a standard 64-bit mixing function: a bijection on `u64`, so distinct
/// inputs always give distinct outputs. Used only to scatter minted identifiers.
///
/// Per-service, like [`now_millis`] and [`request_region`], because the framework has no
/// shared home for any of them: `vendor/` is read-only from here, and a shared crate under
/// `services/` would exist for three helpers of a few lines each. Other bootstrapped
/// services carry their own copies for the same reason.
fn splitmix64(input: u64) -> u64 {
    let mut z = input.wrapping_add(0x9E37_79B9_7F4A_7C15);
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

/// The current wall-clock instant as epoch milliseconds. Per-service for the reason given
/// on [`splitmix64`].
pub fn now_millis() -> i64 {
    let since_epoch = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default();
    i64::try_from(since_epoch.as_millis()).unwrap_or(i64::MAX)
}

/// The new `LastUpdatedTimestamp` for a record last written at `previous`.
///
/// `now` unless the clock has not advanced since `previous`, in which case one
/// millisecond past `previous` — so an update is always observably later than the write
/// before it, which is what a caller comparing two reads relies on.
pub fn bump_millis(previous: i64, now: i64) -> i64 {
    now.max(previous.saturating_add(1))
}

/// Render a stored epoch-millisecond instant as the modelled `Timestamp`.
pub fn timestamp(millis: i64) -> ::aws_smithy_types::DateTime {
    ::aws_smithy_types::DateTime::from_millis(millis)
}

/// The region this request was signed for, for the ARNs the response carries.
///
/// A signed AWS SDK request names it in the SigV4 credential scope; an unsigned one
/// names no region, so the emulator's default stands in.
///
/// Per-service by necessity: the natural shared home for this is beside
/// `emulator_server::sigv4_credential_scope`, and that crate is vendored and read-only
/// from here, so each bootstrapped service wraps it in its own `state.rs` until the
/// framework grows the accessor. See also the note on [`splitmix64`].
pub fn request_region(ctx: &smithy_server_runtime::OperationContext<'_>) -> String {
    ::emulator_server::sigv4_credential_scope(&ctx.parts().headers)
        .map(|scope| scope.region)
        .unwrap_or_else(|| DEFAULT_REGION.to_string())
}

/// The ARN of an archive, in this service's ARN namespace.
///
/// `smithy_query(query="service_info")` reports `arnNamespace: ses` for MailManager, and
/// the archive resource type is `mail-manager-archive`.
pub fn archive_arn(region: &str, archive_id: &str) -> String {
    ::engine::emulator_arn("ses", region, &format!("mail-manager-archive/{archive_id}"))
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsMailmanagerMailmanagersvcState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsMailmanagerMailmanagersvcState>)
}
