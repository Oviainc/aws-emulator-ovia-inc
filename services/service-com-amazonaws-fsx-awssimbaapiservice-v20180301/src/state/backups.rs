// SPDX-License-Identifier: CC0-1.0

//! The backup slice of this service's state: the records `CreateBackup` writes and
//! `DescribeBackups` reads.
//!
//! A backup is a resource of its own, not a view of a file system: AWS documents on
//! `Backup$FileSystem` that "the metadata of the file system associated with the backup ...
//! is persisted even if the file system is deleted", so a record keeps its own COPY of the
//! file-system record as it stood when the backup was taken. Everything a response reports
//! is read back out of that copy, so a backup taken of a file system that has since been
//! deleted (or reconfigured) still reports the configuration it was taken from.
//!
//! The lifecycle follows the same clock-owed pattern as the file-system slice: AWS
//! documents that "the CreateBackup operation returns while the backup's lifecycle state is
//! still CREATING", so a new record starts in `CREATING` with a transition to `AVAILABLE`
//! stored as due, materialised by whichever later call observes it.

use std::collections::BTreeMap;
use std::time::{Instant, SystemTime};

use sha2::{Digest, Sha256};

use crate::ops::types::{ActiveDirectoryBackupAttributes, Backup};

use super::{
    epoch_date_time, tag_list, within_error_message_length, ComAmazonawsFsxAwssimbaapiserviceV20180301State,
    FileSystemRecord, PendingTransition, TransitionTarget,
};

/// `BackupLifecycle` members this slice uses, from the modelled enum
/// (`smithy_query(query="shape", shape="BackupLifecycle")`: AVAILABLE | CREATING |
/// TRANSFERRING | DELETED | FAILED | PENDING | COPYING). A user-initiated backup of a
/// stored file system starts `CREATING` ("Amazon FSx is creating the new user-initiated
/// backup") and becomes `AVAILABLE` ("the backup is fully available"); the other members
/// describe work this slice never starts.
pub const BACKUP_LIFECYCLE_CREATING: &str = "CREATING";
/// See [`BACKUP_LIFECYCLE_CREATING`].
pub const BACKUP_LIFECYCLE_AVAILABLE: &str = "AVAILABLE";

/// See [`BACKUP_LIFECYCLE_CREATING`]. What `DeleteBackup` answers with: AWS documents on
/// `DeleteBackupResponse$Lifecycle` that "if the DeleteBackup operation is successful, the
/// status is DELETED".
pub const BACKUP_LIFECYCLE_DELETED: &str = "DELETED";

/// The `BackupType` of a backup a client asked for, from the modelled enum
/// (`smithy_query(query="shape", shape="BackupType")`: AUTOMATIC | USER_INITIATED |
/// AWS_BACKUP). `CreateBackup` is the client asking, so what it mints is user-initiated;
/// the other two are a backup plan's work, which nothing here runs.
pub const BACKUP_TYPE_USER_INITIATED: &str = "USER_INITIATED";

/// The `ResourceType` a backup of a file system reports, from the modelled enum
/// (`smithy_query(query="shape", shape="ResourceType")`: FILE_SYSTEM | VOLUME).
pub const BACKUP_RESOURCE_TYPE_FILE_SYSTEM: &str = "FILE_SYSTEM";

/// The resource type a backup's `ResourceARN` names, as the `<type>/<id>` half of an FSx
/// ARN (one of [`super::FSX_RESOURCE_TYPES`]).
pub const BACKUP_RESOURCE_TYPE: &str = "backup";

/// The size a backup of this emulator's file systems reports.
///
/// AWS documents `Backup$SizeInBytes` as "the amount of data that the file system would
/// contain if you restore this backup". This emulator serves no file data plane, so a file
/// system holds no data and a restore of it would contain none: the honest size is zero,
/// and it is reported rather than invented from the file system's provisioned capacity,
/// which is space and not data.
pub const BACKUP_SIZE_IN_BYTES: i64 = 0;

/// `Backup$ProgressPercent` of a backup that is still `CREATING`.
pub const PROGRESS_PERCENT_CREATING: i32 = 0;

/// `Backup$ProgressPercent` of a backup that has reached `AVAILABLE`.
///
/// Within the modelled `ProgressPercent` `range 0..100`. The value is DERIVED from the
/// stored lifecycle rather than measured: the record either still owes the clock its
/// transition (nothing of it is done yet) or has completed it, so the member cannot
/// disagree with the lifecycle beside it, and no intermediate fraction is reported for
/// work nothing is performing.
pub const PROGRESS_PERCENT_AVAILABLE: i32 = 100;

/// The largest page `DescribeBackups` returns, from the modelled `Backups` list's own
/// `length: max 50` (`smithy_query(query="operation", operation="DescribeBackups")`).
pub const MAX_BACKUPS_PER_PAGE: usize = 50;

/// The most user-initiated backups this store holds for ONE file system.
///
/// AWS publishes a default quota of 500 user-initiated backups per file system, and models
/// the refusal as `ServiceLimitExceeded` with
/// `ServiceLimit::TOTAL_USER_INITIATED_BACKUPS` (`smithy_query(query="shape",
/// shape="ServiceLimit")`). So the record store, which a caller grows one entry per
/// `CreateBackup`, is bounded by the service's own limit rather than by the size of the
/// process.
pub const MAX_USER_INITIATED_BACKUPS_PER_FILE_SYSTEM: usize = 500;

/// The most backups this store holds AT ALL, across every file system.
///
/// [`MAX_USER_INITIATED_BACKUPS_PER_FILE_SYSTEM`] alone does not bound the store: a backup
/// outlives the file system it was taken of (that is what makes its metadata copy useful),
/// so a caller could otherwise create file systems, fill each one's quota, delete them and
/// repeat. This is the quota over the account's own bound on live file systems
/// ([`super::MAX_FILE_SYSTEMS`]), which is the most a client can hold at once, and a create
/// past it is refused with the same modelled `ServiceLimitExceeded`; deleting backups makes
/// room again.
pub const MAX_BACKUPS: usize = super::MAX_FILE_SYSTEMS * MAX_USER_INITIATED_BACKUPS_PER_FILE_SYSTEM;

/// The modelled `ServiceLimit` value that goes with
/// [`MAX_USER_INITIATED_BACKUPS_PER_FILE_SYSTEM`].
pub const SERVICE_LIMIT_TOTAL_USER_INITIATED_BACKUPS: &str = "TOTAL_USER_INITIATED_BACKUPS";

/// The modelled `FilterName`s `DescribeBackups` selects backups by.
///
/// From the modelled `FilterName` enum (`smithy_query(query="shape", shape="FilterName")`:
/// file-system-id | backup-type | file-system-type | volume-id | data-repository-type |
/// file-cache-id | file-cache-type) — the enum is shared by every filtered describe of this
/// service, so only the four that name something a BACKUP carries are accepted here; the
/// rest select data-repository associations and file caches.
pub const BACKUP_FILTER_NAMES: &[&str] = &[
    FILTER_FILE_SYSTEM_ID,
    FILTER_BACKUP_TYPE,
    FILTER_FILE_SYSTEM_TYPE,
    FILTER_VOLUME_ID,
];

/// Selects backups by the file system they were taken of.
pub const FILTER_FILE_SYSTEM_ID: &str = "file-system-id";
/// Selects backups by their `BackupType`.
pub const FILTER_BACKUP_TYPE: &str = "backup-type";
/// Selects backups by the `FileSystemType` of the file system they were taken of.
pub const FILTER_FILE_SYSTEM_TYPE: &str = "file-system-type";
/// Selects backups by the ONTAP volume they were taken of.
pub const FILTER_VOLUME_ID: &str = "volume-id";

/// [`ComAmazonawsFsxAwssimbaapiserviceV20180301State::insert_backup`] refused the record,
/// and which bound it refused against. The caller answers with the modelled
/// `ServiceLimitExceeded` either way, worded from the bound that was reached.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BackupCountLimitReached {
    /// The file system already holds [`MAX_USER_INITIATED_BACKUPS_PER_FILE_SYSTEM`].
    PerFileSystem,
    /// The store already holds [`MAX_BACKUPS`] across every file system — including backups
    /// of file systems that have since been deleted, which outlive them.
    Store,
}

/// One stored backup. Mirrors the modelled `Backup` shape so [`BackupRecord::to_backup`]
/// is a projection and never a second source of truth.
#[derive(Debug, Clone)]
pub struct BackupRecord {
    pub backup_id: String,
    pub lifecycle: String,
    /// The modelled `Backup$Type` (named `r#type` on the generated shape).
    pub backup_type: String,
    pub creation_time: SystemTime,
    pub kms_key_id: Option<String>,
    pub resource_arn: String,
    pub owner_id: String,
    pub tags: BTreeMap<String, String>,
    /// The file system this backup was taken of, as its record stood at that moment. A
    /// COPY, because AWS documents that this metadata "is persisted even if the file system
    /// is deleted": the live record may change or be removed afterwards, and a backup still
    /// describes what it was taken from.
    pub file_system: FileSystemRecord,
    /// The modelled `ResourceType` of what was backed up.
    pub resource_type: String,
    pub pending: Option<PendingTransition>,
}

impl BackupRecord {
    /// This record as the modelled `Backup` shape. Every member comes from the stored
    /// record, so a response can never disagree with what a later read sees.
    pub fn to_backup(&self) -> Backup {
        Backup {
            backup_id: self.backup_id.clone(),
            lifecycle: self.lifecycle.clone(),
            // No failure is tracked: a backup this slice mints either owes the clock its
            // transition or has completed it, so there is nothing to explain.
            failure_details: None,
            r#type: self.backup_type.clone(),
            progress_percent: Some(self.progress_percent()),
            creation_time: epoch_date_time(self.creation_time),
            kms_key_id: self.kms_key_id.clone(),
            resource_arn: Some(self.resource_arn.clone()),
            tags: tag_list(&self.tags),
            file_system: self.file_system.to_file_system(),
            directory_information: self.directory_information(),
            owner_id: Some(self.owner_id.clone()),
            // Minted by this call, not copied from another backup, and not copied across
            // Regions: `CopyBackup` is the operation that sets these two.
            source_backup_id: None,
            source_backup_region: None,
            resource_type: Some(self.resource_type.clone()),
            // A volume is not a resource this slice stores, so a backup here is never of
            // one.
            volume: None,
            size_in_bytes: Some(BACKUP_SIZE_IN_BYTES),
        }
    }

    /// `Backup$ProgressPercent`, derived from the stored lifecycle: see
    /// [`PROGRESS_PERCENT_AVAILABLE`].
    fn progress_percent(&self) -> i32 {
        if self.lifecycle == BACKUP_LIFECYCLE_AVAILABLE {
            PROGRESS_PERCENT_AVAILABLE
        } else {
            PROGRESS_PERCENT_CREATING
        }
    }

    /// `Backup$DirectoryInformation` — "the configuration of the self-managed Microsoft
    /// Active Directory directory to which the Windows File Server instance is joined" —
    /// read off the file-system copy this backup carries, so it reports the directory the
    /// backed-up file system was actually joined to and nothing when it was joined to none.
    ///
    /// The modelled `ResourceARN` of the directory stays absent: a directory is Directory
    /// Service's resource, not one this service can name.
    fn directory_information(&self) -> Option<ActiveDirectoryBackupAttributes> {
        let windows = self.file_system.windows_configuration.as_ref()?;
        let domain_name = windows
            .self_managed_active_directory_configuration
            .as_ref()
            .and_then(|directory| directory.domain_name.clone());
        let active_directory_id = windows.active_directory_id.clone();
        if domain_name.is_none() && active_directory_id.is_none() {
            return None;
        }
        Some(ActiveDirectoryBackupAttributes {
            domain_name,
            active_directory_id,
            resource_arn: None,
        })
    }

    /// Whether this backup satisfies one `Filter` of `DescribeBackups`: the value the
    /// filter names, read off the stored record.
    ///
    /// `None` for a filter name this listing does not select by, which the caller refuses
    /// rather than silently ignores.
    pub fn filtered_value(&self, filter_name: &str) -> Option<&str> {
        match filter_name {
            FILTER_FILE_SYSTEM_ID => Some(self.file_system.file_system_id.as_str()),
            FILTER_BACKUP_TYPE => Some(self.backup_type.as_str()),
            FILTER_FILE_SYSTEM_TYPE => Some(self.file_system.file_system_type.as_str()),
            // A backup of an ONTAP volume carries a volume id; one of a file system
            // carries none, so no value of this filter selects it.
            FILTER_VOLUME_ID => None,
            _ => None,
        }
    }
}

/// What a `CreateBackup` `ClientRequestToken` replays.
#[derive(Debug, Clone)]
pub struct IdempotentBackup {
    /// The backup the first call with this token created.
    pub backup_id: String,
    /// A FIXED-SIZE digest of that call's parameters (see
    /// [`backup_parameter_fingerprint`]). A replay whose parameters differ is
    /// `IncompatibleParameterError`, which AWS documents for this operation.
    pub fingerprint: String,
}

impl ComAmazonawsFsxAwssimbaapiserviceV20180301State {
    /// Apply every backup lifecycle transition the clock has already made due. Called from
    /// [`ComAmazonawsFsxAwssimbaapiserviceV20180301State::settle_due_transitions`], which
    /// every handler runs.
    pub(super) fn settle_due_backup_transitions(&mut self, now: Instant) {
        let mut removed: Vec<String> = Vec::new();
        for (id, record) in &mut self.backups {
            let Some(pending) = record.pending else {
                continue;
            };
            if pending.due > now {
                continue;
            }
            match pending.target {
                TransitionTarget::Available => {
                    record.lifecycle = BACKUP_LIFECYCLE_AVAILABLE.to_string();
                    record.pending = None;
                }
                TransitionTarget::Removed => removed.push(id.clone()),
            }
        }
        for id in removed {
            self.remove_backup(&id);
        }
    }

    /// Remove one stored backup, with the `CreateBackup` tokens that replayed it.
    ///
    /// The ONE removal path, so the per-file-system count stays in step with the records and
    /// a token whose backup is gone is always released with the record rather than held for
    /// the life of the process.
    pub fn remove_backup(&mut self, backup_id: &str) -> Option<BackupRecord> {
        let removed = self.backups.remove(backup_id)?;
        let file_system_id = removed.file_system.file_system_id.clone();
        if let Some(held) = self.backups_per_file_system.get_mut(&file_system_id) {
            *held = held.saturating_sub(1);
            if *held == 0 {
                // No record left for that file system, so the entry would be memory a
                // caller grew and nothing reclaims.
                self.backups_per_file_system.remove(&file_system_id);
            }
        }
        self.backup_idempotency_tokens
            .retain(|_, replay| replay.backup_id != backup_id);
        let held = &self.backup_idempotency_tokens;
        self.backup_token_order
            .retain(|token| held.contains_key(token));
        Some(removed)
    }

    /// Store a newly created backup, refusing once its file system holds
    /// [`MAX_USER_INITIATED_BACKUPS_PER_FILE_SYSTEM`] or the store holds [`MAX_BACKUPS`].
    ///
    /// The ONE write path for a new record, so neither bound can be bypassed by a later
    /// operation that also creates backups (`CopyBackup`, say). Replacing a record that is
    /// already stored is not growth, so it is always allowed.
    pub fn insert_backup(&mut self, record: BackupRecord) -> Result<(), BackupCountLimitReached> {
        if self.backups.contains_key(&record.backup_id) {
            self.backups.insert(record.backup_id.clone(), record);
            return Ok(());
        }
        if self.backups.len() >= MAX_BACKUPS {
            return Err(BackupCountLimitReached::Store);
        }
        let file_system_id = record.file_system.file_system_id.clone();
        if self.backup_count_of_file_system(&file_system_id)
            >= MAX_USER_INITIATED_BACKUPS_PER_FILE_SYSTEM
        {
            return Err(BackupCountLimitReached::PerFileSystem);
        }
        self.backups.insert(record.backup_id.clone(), record);
        *self
            .backups_per_file_system
            .entry(file_system_id)
            .or_default() += 1;
        Ok(())
    }

    /// How many stored backups were taken of `file_system_id`.
    ///
    /// Maintained by [`ComAmazonawsFsxAwssimbaapiserviceV20180301State::insert_backup`] and
    /// [`ComAmazonawsFsxAwssimbaapiserviceV20180301State::remove_backup`] — the only two
    /// paths that change the records — so asking it costs the same whatever the store holds,
    /// rather than walking every backup while the shared lock is held.
    pub fn backup_count_of_file_system(&self, file_system_id: &str) -> usize {
        self.backups_per_file_system
            .get(file_system_id)
            .copied()
            .unwrap_or_default()
    }

    /// Whether a backup of `file_system_id` is still being created, which AWS refuses a
    /// further backup of that file system with (`BackupInProgress`).
    pub fn backup_in_progress(&self, file_system_id: &str) -> bool {
        self.backup_count_of_file_system(file_system_id) != 0
            && self.backups.values().any(|record| {
                record.file_system.file_system_id == file_system_id
                    && record.lifecycle == BACKUP_LIFECYCLE_CREATING
            })
    }

    /// Remember a `CreateBackup` `ClientRequestToken` replay, dropping the oldest once
    /// [`super::MAX_IDEMPOTENCY_TOKENS`] are held.
    ///
    /// The ONE write path for `backup_idempotency_tokens`, so the bound cannot be bypassed.
    pub fn remember_idempotent_backup(&mut self, token: String, replay: IdempotentBackup) {
        super::remember_token(
            &mut self.backup_idempotency_tokens,
            &mut self.backup_token_order,
            token,
            replay,
        );
    }

    /// Forget one `CreateBackup` `ClientRequestToken`.
    pub fn forget_backup_idempotency_token(&mut self, token: &str) {
        super::forget_token(
            &mut self.backup_idempotency_tokens,
            &mut self.backup_token_order,
            token,
        );
    }

    /// Remember that a `DeleteBackup` carrying `token` removed `backup_id`, dropping the
    /// oldest once [`super::MAX_IDEMPOTENCY_TOKENS`] are held.
    ///
    /// The ONE write path for `deleted_backup_tokens`, so the bound cannot be bypassed.
    pub fn remember_backup_deletion(&mut self, token: String, backup_id: String) {
        super::remember_token(
            &mut self.deleted_backup_tokens,
            &mut self.deleted_backup_token_order,
            token,
            backup_id,
        );
    }

    /// Mint the next `BackupId`.
    ///
    /// Shaped to the modelled `BackupId` pattern `^(backup-[0-9a-f]{8,})$` with
    /// `length 12..128`: `backup-` plus 17 lowercase hex digits, the form AWS documents
    /// for a backup identifier.
    pub fn mint_backup_id(&mut self) -> String {
        self.next_backup_id_counter += 1;
        format!("backup-{:017x}", self.next_backup_id_counter)
    }
}

/// The `BackupNotFound$Message` an operation addressed by a `BackupId` answers with: a
/// sentence naming the id the caller asked about, single-quoted and terminated with a
/// period.
///
/// The same rule as [`super::file_system_not_found_message`] — the identifier in the
/// request is what tells a client WHICH of the ids it named was missing — so it is
/// templated from the requested id and clamped to the modelled `ErrorMessage`
/// `length 1..256`.
pub fn backup_not_found_message(backup_id: &str) -> String {
    within_error_message_length(format!("Backup '{backup_id}' does not exist."))
}

/// The `VolumeNotFound$Message` an operation addressed by a `VolumeId` answers with. Same
/// rule as [`backup_not_found_message`].
pub fn volume_not_found_message(volume_id: &str) -> String {
    within_error_message_length(format!("Volume '{volume_id}' does not exist."))
}

/// The `BadRequest$Message` for a `DescribeBackups` `Filter` whose name this listing does
/// not select by: the name the request carried, and the names it accepts.
///
/// The accepted names are rendered from [`BACKUP_FILTER_NAMES`] itself rather than written
/// out again, so the sentence cannot drift from what the handler applies. Clamped to the
/// modelled `ErrorMessage` `length 1..256`, because the name comes from the request.
pub fn unsupported_filter_name_message(filter_name: &str) -> String {
    within_error_message_length(format!(
        "The filter name '{filter_name}' is not supported by DescribeBackups. The supported \
         names are {}.",
        BACKUP_FILTER_NAMES.join(", ")
    ))
}

/// A fixed-size digest of a `CreateBackup` call's parameters, for the
/// `@idempotencyToken` comparison.
///
/// Every input member except `ClientRequestToken` itself takes part, so two calls carrying
/// one token are "the same parameters" exactly when every other member agrees. Tags are
/// sorted first, so the same tag set in a different order compares equal. The members are
/// rendered and then DIGESTED, so what the service remembers per token is a fixed number of
/// bytes whatever the request's size; the digest is only ever compared against another
/// digest from this same function, and is never parsed, stored as a resource value, or
/// returned.
pub fn backup_parameter_fingerprint(
    file_system_id: Option<&str>,
    volume_id: Option<&str>,
    tags: &BTreeMap<String, String>,
) -> String {
    let rendered = format!("{file_system_id:?}|{volume_id:?}|{tags:?}");
    hex::encode(Sha256::digest(rendered.as_bytes()))
}

/// The pagination scope of one `DescribeBackups` listing.
///
/// A cursor only means something over the same ordered set of keys, so a token minted for
/// one selection is not accepted for another: the requested `BackupIds` and the `Filters`
/// both take part. They are caller-supplied text of unbounded shape, so the scope is their
/// DIGEST — a fixed-length, separator-free string — rather than the values themselves.
/// `selected` must be sorted and deduplicated, and `filters` normalised the same way, so
/// one selection written in a different order is the same scope.
pub fn backups_page_scope(selected: &[String], filters: &[(String, Vec<String>)]) -> String {
    if selected.is_empty() && filters.is_empty() {
        return "backups".to_string();
    }
    let rendered = format!("{selected:?}|{filters:?}");
    format!("backups/{}", hex::encode(Sha256::digest(rendered.as_bytes())))
}
